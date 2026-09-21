use crate::browser::auth::PairingToken;
use serde::Deserialize;
use std::{
    io,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedConfig {
    pub version: u32,
    pub control_address: SocketAddr,
    pub http_port: u16,
    pub token_path: PathBuf,
    pub preferences_path: PathBuf,
    #[serde(default)]
    pub interface_address: Option<IpAddr>,
    #[serde(default)]
    pub output_device: Option<String>,
    #[serde(default)]
    pub microphone: Option<String>,
}
impl ManagedConfig {
    pub fn load(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(invalid("managed config path must be absolute"));
        }
        Self::from_json(&std::fs::read_to_string(path)?)
    }
    pub fn from_json(json: &str) -> io::Result<Self> {
        if json.len() > 65536 {
            return Err(invalid("managed config too large"));
        }
        let value: Self =
            serde_json::from_str(json).map_err(|_| invalid("invalid managed config"))?;
        if value.version != 1
            || !value.control_address.ip().is_loopback()
            || value.control_address.port() == 0
            || value.http_port == 0
            || !value.token_path.is_absolute()
            || !value.preferences_path.is_absolute()
            || value.token_path == value.preferences_path
        {
            return Err(invalid("unsupported or unsafe managed config"));
        }
        Ok(value)
    }
    pub fn token(&self) -> io::Result<PairingToken> {
        let encoded = std::fs::read_to_string(&self.token_path)?;
        PairingToken::from_hex(encoded.trim()).ok_or_else(|| invalid("invalid pairing token file"))
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Re-evaluated for each playback/discovery attempt so an unavailable interface
/// does not prevent local control from starting or later recovering.
pub fn select_interface(configured: Option<IpAddr>) -> io::Result<IpAddr> {
    let interfaces = if_addrs::get_if_addrs()?;
    if let Some(address) = configured {
        return interfaces
            .iter()
            .any(|i| i.ip() == address)
            .then_some(address)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "configured interface unavailable",
                )
            });
    }
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("239.255.255.250:1900")?;
    let address = socket.local_addr()?.ip();
    if address.is_loopback() || address.is_unspecified() {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "LAN interface unavailable",
        ));
    }
    Ok(address)
}
