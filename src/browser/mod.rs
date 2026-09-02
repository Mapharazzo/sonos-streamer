pub mod audio;
pub mod auth;
pub mod bridge;
pub mod calibration;
pub mod commands;
pub mod mode;
pub mod protocol;

pub const DEFAULT_BROWSER_BRIDGE_PORT: u16 = 5902;
pub const BROWSER_SAMPLE_RATE: u32 = 48_000;
pub const BROWSER_CHANNELS: u16 = 2;
