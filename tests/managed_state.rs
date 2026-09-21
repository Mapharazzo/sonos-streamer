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
fn reconnected_browser_must_confirm_cleanup_before_capture_can_restart() {
    let mut session = configured(Source::Chrome);
    session.on_event(Event::BrowserConnected {
        owner: "old-browser".into(),
    });
    let prepare = command(&session, "prepare", Action::PrepareBrowser);
    session.apply(&caller("old-browser", Role::Browser), prepare);
    session.on_event(Event::BrowserDisconnected {
        owner: "old-browser".into(),
    });
    let effects = session.on_event(Event::BrowserConnected {
        owner: "new-browser".into(),
    });
    let transition = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::ReleaseBrowser {
                owner,
                transition_id,
            } if owner == "new-browser" => Some(transition_id.clone()),
            _ => None,
        })
        .expect("request cleanup from reconnected extension");
    let ack = command(
        &session,
        "clean",
        Action::CaptureReleased {
            transition_id: transition,
        },
    );
    assert_eq!(
        session
            .apply(&caller("new-browser", Role::Browser), ack)
            .0
            .status,
        Status::Ok
    );
    assert_eq!(session.snapshot().phase, Phase::Idle);
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
