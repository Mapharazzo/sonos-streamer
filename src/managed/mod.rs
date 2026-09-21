//! Supervised playback with independent controller and audio-producer ownership.
pub mod audio;
pub mod bridge;
pub mod config;
pub mod devices;
pub mod events;
pub mod leases;
pub mod preferences;
pub mod protocol;
pub mod runtime;
pub mod state;
pub mod streaming;

pub use runtime::run;
