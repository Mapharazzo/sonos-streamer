//! NDJSON debug logs for Cursor debug mode (session 2feb3d).
// #region agent log
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

fn log_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".cursor")
        .join("debug-2feb3d.log")
}

/// Append one NDJSON line. `data` should be a small JSON object literal, e.g. `{}` or `{"k":1}`.
pub fn agent_log(hypothesis_id: &str, location: &str, message: &str, data: &str) {
    let path = log_path();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let msg_esc = message.replace('\\', "\\\\").replace('"', "\\\"");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(
            f,
            r#"{{"sessionId":"2feb3d","hypothesisId":"{hypothesis_id}","location":"{location}","message":"{msg_esc}","data":{data},"timestamp":{ts}}}"#
        );
    }
}
// #endregion
