#![cfg(feature = "cli")]
//! Interactive CLI setup: network, renderer, capture device, mic, streaming format.

use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use hashbrown::HashMap;
use log::info;

use crate::globals::statics::get_config_mut;
use crate::openhome::rendercontrol::{Renderer, discover};
use crate::utils::audiodevices::{get_input_audio_devices, get_output_audio_devices};
use crate::utils::configuration::Configuration;
use crate::utils::local_ip_address::get_interfaces;
use crate::{enums::streaming::StreamingFormat, globals::statics::get_config};

/// Run interactive prompts and return an updated configuration (also written to disk).
pub fn run_interactive_wizard(mut c: Configuration) -> Result<Configuration, String> {
    let theme = ColorfulTheme::default();

    let nets = get_interfaces();
    if nets.is_empty() {
        return Err("No IPv4 network interfaces found.".into());
    }
    let net_idx = Select::with_theme(&theme)
        .with_prompt("Network interface for SSDP + stream URL")
        .items(&nets)
        .default(0)
        .interact()
        .map_err(|e| e.to_string())?;
    c.last_network = Some(nets[net_idx].clone());
    {
        let mut g = get_config_mut();
        g.last_network = c.last_network.clone();
    }

    let agent = ureq::agent();
    let rmap = HashMap::<String, Renderer>::new();
    println!("Discovering renderers (about 3s)…");
    let mut renderers =
        discover(&agent, &rmap).ok_or_else(|| "SSDP discovery failed".to_string())?;
    if renderers.is_empty() {
        return Err("No renderers found. Check network / firewall.".into());
    }
    let labels: Vec<String> = renderers
        .iter()
        .map(|r| format!("{} — {}", r.dev_name, r.remote_addr))
        .collect();
    let ridx = Select::with_theme(&theme)
        .with_prompt("Speaker / renderer")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(|e| e.to_string())?;
    let chosen = renderers.swap_remove(ridx);
    c.last_renderer = Some(chosen.remote_addr.clone());
    c.active_renderers = vec![chosen.remote_addr.clone()];

    let outs = get_output_audio_devices();
    if outs.is_empty() {
        return Err("No audio output devices to capture.".into());
    }
    let onames: Vec<String> = outs.iter().map(|d| d.name().to_string()).collect();
    let mut def_o = 0usize;
    for (i, n) in onames.iter().enumerate() {
        let u = n.to_uppercase();
        if u.contains("CABLE") || u.contains("VB-AUDIO") {
            def_o = i;
            break;
        }
    }
    let oidx = Select::with_theme(&theme)
        .with_prompt("Loopback capture device (e.g. VB-Cable Output)")
        .items(&onames)
        .default(def_o)
        .interact()
        .map_err(|e| e.to_string())?;
    c.sound_source = Some(outs[oidx].name().to_string());
    c.sound_source_index = Some(oidx as i32);

    let ins = get_input_audio_devices();
    if ins.is_empty() {
        c.input_device = None;
        println!("No input devices enumerated; latency listener will use the OS default input.");
    } else {
        let inames: Vec<String> = ins.iter().map(|d| d.name().to_string()).collect();
        let iidx = Select::with_theme(&theme)
            .with_prompt("Input device for latency detection (mic / line-in)")
            .items(&inames)
            .default(0)
            .interact()
            .map_err(|e| e.to_string())?;
        c.input_device = Some(ins[iidx].name().to_string());
    }

    let formats = ["WAV (recommended for Sonos)", "LPCM", "RF64"];
    let fidx = Select::with_theme(&theme)
        .with_prompt("Streaming format")
        .items(&formats)
        .default(0)
        .interact()
        .map_err(|e| e.to_string())?;
    c.streaming_format = Some(match fidx {
        0 => StreamingFormat::Wav,
        1 => StreamingFormat::Lpcm,
        _ => StreamingFormat::Rf64,
    });

    let cal = Confirm::with_theme(&theme)
        .with_prompt("Run startup latency calibration?")
        .default(get_config().auto_calibrate.unwrap_or(true))
        .interact()
        .map_err(|e| e.to_string())?;
    c.auto_calibrate = Some(cal);
    if cal {
        let def = c.latency_threshold_ms.unwrap_or(500).to_string();
        let thr: String = Input::with_theme(&theme)
            .with_prompt("Latency threshold (ms)")
            .default(def)
            .interact_text()
            .map_err(|e| e.to_string())?;
        c.latency_threshold_ms = Some(
            thr.parse()
                .map_err(|_| "Invalid latency threshold".to_string())?,
        );
    }

    c.update_config().map_err(|e| e.to_string())?;
    {
        let mut g = get_config_mut();
        *g = c.clone();
    }
    info!("Configuration saved from setup wizard.");
    Ok(c)
}
