use super::{
    leases::random_id,
    protocol::{Snapshot, Source, valid_id},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Write},
    path::Path,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub source: Source,
    pub renderer_id: Option<String>,
    pub manual_offset_ms: i32,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            source: Source::Chrome,
            renderer_id: None,
            manual_offset_ms: 0,
        }
    }
}
impl Preferences {
    pub fn load(path: &Path) -> io::Result<Self> {
        let json = match std::fs::read_to_string(path) {
            Ok(json) => json,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        let value: Self = serde_json::from_str(&json).map_err(io::Error::other)?;
        if json.len() > 65536
            || !(-5000..=5000).contains(&value.manual_offset_ms)
            || value.renderer_id.as_ref().is_some_and(|id| !valid_id(id))
        {
            return Err(io::Error::other("invalid preferences"));
        }
        Ok(value)
    }
    pub fn from_snapshot(state: &Snapshot) -> Self {
        Self {
            source: state.source,
            renderer_id: state.renderer_id.clone(),
            manual_offset_ms: state.manual_offset_ms,
        }
    }
    pub fn save_atomic(&self, path: &Path) -> io::Result<()> {
        let temp = path.with_extension(format!(
            "{}.tmp",
            random_id().map_err(|_| io::Error::other("randomness unavailable"))?
        ));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&serde_json::to_vec(self).map_err(io::Error::other)?)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }
}
