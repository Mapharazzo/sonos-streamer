use swyh_rs::browser::protocol::RendererSummary;
use swyh_rs::managed::protocol::{
    Action, Caller, Command, Phase, Role, Source, Status, parse_command,
};
use swyh_rs::managed::state::{Effect, Event, Session};

fn caller(id: &str, role: Role) -> Caller {
    Caller {
        connection_id: id.into(),
        role,
    }
}

fn command(session: &Session, id: &str, action: Action) -> Command {
    Command {
        request_id: id.into(),
        instance_id: session.snapshot().instance_id.clone(),
        expected_revision: session.snapshot().revision,
        action,
    }
}

fn room() -> RendererSummary {
    RendererSummary {
        id: "room-1".into(),
        name: "Office".into(),
        model: "Sonos".into(),
        address: "192.0.2.10".into(),
    }
}

fn configured(source: Source) -> Session {
    let mut session = Session::new("instance-1".into());
    session.on_event(Event::Renderers(vec![room()]));
    let native = caller("native", Role::Controller);
    let select = command(
        &session,
        "room",
        Action::SelectRenderer {
            renderer_id: "room-1".into(),
        },
    );
    assert_eq!(session.apply(&native, select).0.status, Status::Ok);
    let select = command(&session, "source", Action::SelectSource { source });
    assert_eq!(session.apply(&native, select).0.status, Status::Ok);
    session
}

#[test]
fn settings_reservation_excludes_playback_even_if_controller_disconnects() {
    let mut session = configured(Source::Windows);
    let native = caller("native", Role::Controller);
    let browser = caller("browser", Role::Browser);
    for owner in [&browser, &native] {
        let reserve = command(
            &session,
            &format!("reserve-{}", owner.connection_id),
            Action::PrepareSettings,
        );
        let result = session.apply(owner, reserve).0;
        assert_eq!(
            result.status,
            if owner.role == Role::Browser {
                Status::Rejected
            } else {
                Status::Ok
            }
        );
    }
    let start = command(&session, "start-blocked", Action::StartWindows);
    assert_eq!(session.apply(&native, start).0.code, "settings-in-progress");
    let release = command(&session, "foreign-release", Action::ReleaseSettings);
    assert_eq!(
        session
            .apply(&caller("other", Role::Controller), release)
            .0
            .status,
        Status::Rejected
    );
    let release = command(&session, "release", Action::ReleaseSettings);
    assert_eq!(session.apply(&native, release).0.status, Status::Ok);
    let reserve = command(&session, "reserve-again", Action::PrepareSettings);
    assert_eq!(session.apply(&native, reserve).0.status, Status::Ok);
    session.forget_controller("native");
    let blocked = command(&session, "disconnected-start", Action::StartWindows);
    assert_eq!(
        session.apply(&native, blocked).0.code,
        "settings-in-progress"
    );
    let release = command(
        &session,
        "release-after-disconnect",
        Action::ReleaseSettings,
    );
    assert_eq!(session.apply(&native, release).0.status, Status::Ok);
    let start = command(&session, "start", Action::StartWindows);
    assert_eq!(session.apply(&native, start).0.status, Status::Ok);
    let reserve = command(&session, "reserve-active", Action::PrepareSettings);
    assert_eq!(
        session.apply(&native, reserve).0.code,
        "stop-playback-first"
    );
}

#[test]
fn startup_is_idle_and_cannot_capture_without_room_and_start() {
    let mut session = Session::new("instance-1".into());
    assert_eq!(session.snapshot().phase, Phase::Idle);
    assert_eq!(session.snapshot().generation, 1);
    let start = command(&session, "start", Action::StartWindows);
    let (result, effects) = session.apply(&caller("native", Role::Controller), start);
    assert_eq!(result.status, Status::Rejected);
    assert!(effects.is_empty());
}

#[test]
fn multiple_browsers_share_controls_but_cleanup_and_reload_follow_the_capture() {
    let (mut session, active) = active_browser();
    let other = caller("brave", Role::Browser);
    let before = session.snapshot().clone();
    assert!(
        session
            .on_event(Event::BrowserConnected {
                owner: other.connection_id.clone()
            })
            .is_empty()
    );
    assert_eq!(session.snapshot().generation, before.generation);
    assert_eq!(session.snapshot().phase, before.phase);
    for (browser, recovery) in [(&active, true), (&other, false)] {
        let status = command(
            &session,
            "status",
            Action::BrowserStatus {
                shortcut: "Ctrl+Shift+8".into(),
                supported_tab: true,
                has_recovery_tab: recovery,
                video_sync_supported: recovery,
            },
        );
        assert_eq!(session.apply(browser, status).0.status, Status::Ok);
    }
    let reload = command(&session, "reload-other", Action::ReloadTab);
    assert!(session.apply(&other, reload).1.iter().any(
        |effect| matches!(effect, Effect::ReloadTab {owner,..} if owner == &active.connection_id)
    ));
    let stop = command(&session, "stop-other", Action::Stop);
    let effects = session.apply(&other, stop).1;
    let transition = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::ReleaseBrowser {
                owner,
                transition_id,
            } if owner == &active.connection_id => Some(transition_id.clone()),
            _ => None,
        })
        .unwrap();
    let wrong = command(
        &session,
        "wrong-ack",
        Action::CaptureReleased {
            transition_id: transition.clone(),
        },
    );
    assert_eq!(session.apply(&other, wrong).0.status, Status::Rejected);
    let ack = command(
        &session,
        "owner-ack",
        Action::CaptureReleased {
            transition_id: transition,
        },
    );
    assert_eq!(session.apply(&active, ack).0.status, Status::Ok);
    let prepare = command(&session, "other-prepare", Action::PrepareBrowser);
    assert_eq!(session.apply(&other, prepare).0.status, Status::Ok);
    let generation = session.snapshot().generation;
    assert!(
        session
            .on_event(Event::BrowserDisconnected {
                owner: active.connection_id
            })
            .is_empty()
    );
    assert_eq!(session.snapshot().generation, generation);
    assert_eq!(session.snapshot().phase, Phase::Starting);
}

#[test]
fn reconnected_browser_does_not_inherit_capture_or_interrupt_another_browser() {
    let (mut session, old) = active_browser();
    let generation = session.snapshot().generation;
    let stopped = session.on_event(Event::BrowserDisconnected {
        owner: old.connection_id.clone(),
    });
    assert!(stopped.contains(&Effect::StopPlayback { generation }));
    assert_eq!(session.snapshot().phase, Phase::Failed);
    let effects = session.on_event(Event::BrowserConnected {
        owner: "new-browser".into(),
    });
    assert!(effects.is_empty());
    let stale = command(
        &session,
        "stale-start",
        Action::StartBrowser {
            lease_id: "lease-1".into(),
        },
    );
    assert_eq!(
        session
            .apply(&caller("new-browser", Role::Browser), stale)
            .0
            .status,
        Status::Rejected
    );
    let prepare = command(&session, "explicit-cast", Action::PrepareBrowser);
    assert_eq!(
        session
            .apply(&caller("new-browser", Role::Browser), prepare)
            .0
            .status,
        Status::Ok
    );
    let generation = session.snapshot().generation;
    assert!(
        session
            .on_event(Event::BrowserConnected {
                owner: old.connection_id
            })
            .is_empty()
    );
    assert_eq!(session.snapshot().generation, generation);
}
#[test]
fn producer_loss_requests_cleanup_from_its_connected_browser() {
    let mut session = configured(Source::Chrome);
    session.on_event(Event::BrowserConnected {
        owner: "browser".into(),
    });
    let prepare = command(&session, "prepare", Action::PrepareBrowser);
    session.apply(&caller("browser", Role::Browser), prepare);
    let generation = session.snapshot().generation;
    let effects = session.on_event(Event::ProducerDisconnected {
        owner: "browser".into(),
        generation,
    });
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect,Effect::ReleaseBrowser{owner,..} if owner=="browser"))
    );
}

#[test]
fn video_sync_capability_follows_the_active_browser_only() {
    let (mut session, active) = active_browser();
    session.on_event(Event::BrowserConnected {
        owner: "observer".into(),
    });
    for enabled in [false, true] {
        for (caller, video_sync_supported) in [
            (&active, enabled),
            (&caller("observer", Role::Browser), !enabled),
        ] {
            let status = command(
                &session,
                &format!("video-{enabled}"),
                Action::BrowserStatus {
                    shortcut: String::new(),
                    supported_tab: true,
                    has_recovery_tab: true,
                    video_sync_supported,
                },
            );
            assert_eq!(session.apply(caller, status).0.status, Status::Ok);
        }
        assert_eq!(
            session.snapshot().capabilities.calibration_available,
            enabled
        );
        let calibrate = command(&session, &format!("calibrate-{enabled}"), Action::Calibrate);
        assert_eq!(
            session
                .apply(&caller("observer", Role::Browser), calibrate)
                .0
                .status,
            if enabled {
                Status::Ok
            } else {
                Status::Rejected
            }
        );
    }
}

#[test]
fn windows_start_is_explicit_and_streaming_waits_for_matching_renderer_feedback() {
    let mut session = configured(Source::Windows);
    assert_eq!(session.snapshot().phase, Phase::Idle);
    let start = command(&session, "start", Action::StartWindows);
    let (result, effects) = session.apply(&caller("native", Role::Controller), start);
    assert_eq!(result.status, Status::Ok);
    assert_eq!(session.snapshot().phase, Phase::Starting);
    let generation = session.snapshot().generation;
    assert!(effects.contains(&Effect::OpenWindowsCapture { generation }));
    session.on_event(Event::StreamStarted {
        generation: generation - 1,
    });
    assert_eq!(session.snapshot().phase, Phase::Starting);
    session.on_event(Event::StreamStarted { generation });
    assert_eq!(session.snapshot().phase, Phase::Streaming);
}

#[test]
fn stale_instance_revision_and_invalid_actions_have_no_effect() {
    let mut session = configured(Source::Windows);
    let before = session.snapshot().clone();
    let native = caller("native", Role::Controller);
    let mut stale = command(&session, "old-instance", Action::StartWindows);
    stale.instance_id = "old".into();
    assert_eq!(session.apply(&native, stale).0.code, "stale-instance");
    let mut stale = command(&session, "old-revision", Action::StartWindows);
    stale.expected_revision -= 1;
    assert_eq!(session.apply(&native, stale).0.code, "stale-revision");
    for (id, action) in [
        ("bad-volume", Action::SetVolume { value: 101 }),
        ("negative-volume", Action::SetVolume { value: -1 }),
        ("bad-offset", Action::SetOffset { milliseconds: 5001 }),
        (
            "unknown-room",
            Action::SelectRenderer {
                renderer_id: "other".into(),
            },
        ),
        ("calibration", Action::Calibrate),
    ] {
        let request = command(&session, id, action);
        let (result, effects) = session.apply(&native, request);
        assert_eq!(result.status, Status::Rejected, "{id}");
        assert!(effects.is_empty(), "{id}");
    }
    assert_eq!(session.snapshot(), &before);
}

#[test]
fn duplicate_start_is_cached_per_controller_without_repeating_capture() {
    let mut session = configured(Source::Windows);
    let native = caller("native", Role::Controller);
    let start = command(&session, "request-1", Action::StartWindows);
    let first = session.apply(&native, start.clone());
    let second = session.apply(&native, start.clone());
    assert_eq!(first.0, second.0);
    assert!(second.1.is_empty());
    let mut conflicting = start;
    conflicting.action = Action::Stop;
    assert_eq!(
        session.apply(&native, conflicting).0.code,
        "request-id-conflict"
    );
    let other = command(&session, "request-1", Action::Stop);
    assert_eq!(
        session
            .apply(&caller("other", Role::Controller), other)
            .0
            .status,
        Status::Ok
    );
    assert_eq!(session.snapshot().phase, Phase::Idle);
}

fn active_browser() -> (Session, Caller) {
    let mut session = configured(Source::Chrome);
    let browser = caller("chrome", Role::Browser);
    session.on_event(Event::BrowserConnected {
        owner: browser.connection_id.clone(),
    });
    let prepare = command(&session, "prepare", Action::PrepareBrowser);
    assert_eq!(session.apply(&browser, prepare).0.status, Status::Ok);
    let generation = session.snapshot().generation;
    session.on_event(Event::ProducerAttached {
        owner: "chrome".into(),
        generation,
        lease_id: "lease-1".into(),
    });
    let start = command(
        &session,
        "cast",
        Action::StartBrowser {
            lease_id: "lease-1".into(),
        },
    );
    assert_eq!(session.apply(&browser, start).0.status, Status::Ok);
    session.on_event(Event::StreamStarted { generation });
    (session, browser)
}

#[test]
fn calibration_applies_measurement_and_persists_cleared_manual_adjustment() {
    let (mut session, browser) = active_browser();
    let offset = command(&session, "offset", Action::SetOffset { milliseconds: 5000 });
    session.apply(&browser, offset);
    let calibrate = command(&session, "calibrate", Action::Calibrate);
    assert_eq!(session.apply(&browser, calibrate).0.status, Status::Ok);
    assert_eq!(session.snapshot().manual_offset_ms, 5000);
    let effects = session.on_event(Event::Calibration {
        generation: session.snapshot().generation,
        milliseconds: 513,
    });
    assert_eq!(session.snapshot().measured_delay_ms, 513);
    assert_eq!(session.snapshot().manual_offset_ms, 0);
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::PersistPreferences))
    );
}

#[test]
fn failed_or_stale_calibration_keeps_existing_adjustment() {
    let (mut session, browser) = active_browser();
    let offset = command(&session, "offset", Action::SetOffset { milliseconds: 5000 });
    session.apply(&browser, offset);
    session.on_event(Event::OperationFailed {
        generation: session.snapshot().generation,
        code: "calibration-timeout",
    });
    assert_eq!(session.snapshot().manual_offset_ms, 5000);
    let effects = session.on_event(Event::Calibration {
        generation: session.snapshot().generation - 1,
        milliseconds: 513,
    });
    assert_eq!(session.snapshot().manual_offset_ms, 5000);
    assert_eq!(session.snapshot().measured_delay_ms, 0);
    assert!(effects.is_empty());
}

#[test]
fn source_switch_waits_for_owner_release_then_remains_idle() {
    let (mut session, browser) = active_browser();
    let native = caller("native", Role::Controller);
    let select = command(
        &session,
        "switch",
        Action::SelectSource {
            source: Source::Windows,
        },
    );
    let (_, effects) = session.apply(&native, select);
    assert_eq!(session.snapshot().phase, Phase::Switching);
    let transition = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::ReleaseBrowser {
                owner,
                transition_id,
            } if owner == "chrome" => Some(transition_id.clone()),
            _ => None,
        })
        .expect("release request");
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::OpenWindowsCapture { .. }))
    );
    let forged = command(
        &session,
        "forged",
        Action::CaptureReleased {
            transition_id: transition.clone(),
        },
    );
    assert_eq!(session.apply(&native, forged).0.code, "not-browser-owner");
    let ack = command(
        &session,
        "released",
        Action::CaptureReleased {
            transition_id: transition,
        },
    );
    let (_, effects) = session.apply(&browser, ack);
    assert_eq!(session.snapshot().source, Source::Windows);
    assert_eq!(session.snapshot().phase, Phase::Idle);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::OpenWindowsCapture { .. }))
    );
}

#[test]
fn timed_out_or_disconnected_browser_never_falls_back_to_windows() {
    for disconnect in [false, true] {
        let (mut session, browser) = active_browser();
        let request = command(
            &session,
            "switch",
            Action::SelectSource {
                source: Source::Windows,
            },
        );
        let (_, effects) = session.apply(&caller("native", Role::Controller), request);
        let transition_id = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::ReleaseBrowser { transition_id, .. } => Some(transition_id.clone()),
                _ => None,
            })
            .unwrap();
        let effects = if disconnect {
            session.on_event(Event::BrowserDisconnected {
                owner: browser.connection_id.clone(),
            })
        } else {
            session.on_event(Event::TransitionTimedOut {
                transition_id: transition_id.clone(),
            })
        };
        assert_eq!(session.snapshot().phase, Phase::Failed);
        assert_eq!(session.snapshot().source, Source::Chrome);
        assert_eq!(
            session.snapshot().error_code.as_deref(),
            Some("browser-cleanup-required")
        );
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::OpenWindowsCapture { .. }))
        );
        let late = command(&session, "late", Action::CaptureReleased { transition_id });
        assert_eq!(session.apply(&browser, late).0.status, Status::Rejected);
    }
}

#[test]
fn stop_during_transition_invalidates_late_ack_and_blocks_a_second_switch() {
    let (mut session, browser) = active_browser();
    let native = caller("native", Role::Controller);
    let request = command(
        &session,
        "switch",
        Action::SelectSource {
            source: Source::Windows,
        },
    );
    let (_, effects) = session.apply(&native, request);
    let transition_id = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::ReleaseBrowser { transition_id, .. } => Some(transition_id.clone()),
            _ => None,
        })
        .unwrap();
    let second = command(
        &session,
        "switch-again",
        Action::SelectSource {
            source: Source::Chrome,
        },
    );
    assert_eq!(
        session.apply(&native, second).0.code,
        "transition-in-progress"
    );
    let stop = command(&session, "stop", Action::Stop);
    session.apply(&native, stop);
    let late = command(&session, "late", Action::CaptureReleased { transition_id });
    assert_eq!(session.apply(&browser, late).0.status, Status::Rejected);
    assert_eq!(session.snapshot().source, Source::Chrome);
}

#[test]
fn malformed_wire_commands_are_rejected_before_dispatch() {
    let good = include_str!("fixtures/managed-select-source.json");
    assert!(
        parse_command(good).is_ok(),
        "{:?}",
        serde_json::from_str::<Command>(good)
    );
    for bad in [
        good.replace("\"command\"", "\"other\""),
        good.replace("select-source", "run-shell"),
        good.replace("windows", "microphone"),
        good.replace("\"expectedRevision\": 1", "\"expectedRevision\": 0"),
        good.replace("\"request-1\"", "\"\""),
        good.replace(
            "\"source\": \"windows\"",
            "\"source\": \"windows\", \"extra\": true",
        ),
        good.replace(
            "\"type\": \"command\"",
            "\"type\": \"command\", \"extra\": true",
        ),
    ] {
        assert!(parse_command(&bad).is_err(), "accepted {bad}");
    }
}

#[test]
fn losing_a_room_stops_its_capture_and_does_not_select_another_room() {
    let mut session = configured(Source::Windows);
    let start = command(&session, "start", Action::StartWindows);
    session.apply(&caller("native", Role::Controller), start);
    let effects = session.on_event(Event::Renderers(vec![]));
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::StopPlayback { .. }))
    );
    assert_eq!(session.snapshot().phase, Phase::Failed);
    assert_eq!(session.snapshot().renderer_id.as_deref(), Some("room-1"));
}

#[test]
fn a_second_browser_start_cannot_repeat_renderer_play() {
    let (mut session, browser) = active_browser();
    let retry = command(
        &session,
        "cast-again",
        Action::StartBrowser {
            lease_id: "lease-1".into(),
        },
    );
    assert_eq!(session.apply(&browser, retry).0.status, Status::Rejected);

    let mut session = configured(Source::Chrome);
    session.on_event(Event::BrowserConnected {
        owner: "chrome".into(),
    });
    let prepare = command(&session, "prepare", Action::PrepareBrowser);
    session.apply(&browser, prepare);
    let generation = session.snapshot().generation;
    session.on_event(Event::ProducerAttached {
        owner: "chrome".into(),
        generation,
        lease_id: "lease-2".into(),
    });
    let cast = command(
        &session,
        "cast",
        Action::StartBrowser {
            lease_id: "lease-2".into(),
        },
    );
    session.apply(&browser, cast);
    let second = command(
        &session,
        "cast-again",
        Action::StartBrowser {
            lease_id: "lease-2".into(),
        },
    );
    let (result, effects) = session.apply(&browser, second);
    assert_eq!(result.status, Status::Rejected);
    assert!(effects.is_empty());
}

#[test]
fn renderer_feedback_cannot_start_playback_before_browser_cast() {
    let mut session = configured(Source::Chrome);
    let browser = caller("chrome", Role::Browser);
    session.on_event(Event::BrowserConnected {
        owner: "chrome".into(),
    });
    let prepare = command(&session, "prepare", Action::PrepareBrowser);
    session.apply(&browser, prepare);
    session.on_event(Event::StreamStarted {
        generation: session.snapshot().generation,
    });
    assert_eq!(session.snapshot().phase, Phase::Starting);
}
