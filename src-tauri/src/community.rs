// community.rs — the opt-in gate for the arx community registry (arx-pub-db / community.db).
// Community repos are PUBLIC but UNVETTED, so the stream is OFF by default and arx refuses
// `arx pub install` until this flag exists. The flag lives under /etc/arxos, so writing it needs
// root — it goes through pkexec, the same privileged-write path the Performance and Network
// panels use. Reading the state is unprivileged (a plain file-exists check).
use std::path::Path;

const FLAG: &str = "/etc/arxos/community.enabled";

/// Is the community registry currently enabled? (unprivileged)
#[tauri::command]
pub fn community_status() -> bool {
    Path::new(FLAG).exists()
}

/// Enable or disable the community registry by creating/removing the flag, as root via pkexec.
/// Returns the resulting state so the UI reflects what actually happened (a cancelled pkexec
/// leaves the state unchanged and returns an error the UI shows).
#[tauri::command]
pub fn community_set(enabled: bool) -> Result<bool, String> {
    let script = if enabled {
        "mkdir -p /etc/arxos && : > /etc/arxos/community.enabled"
    } else {
        "rm -f /etc/arxos/community.enabled"
    };
    let ok = std::process::Command::new("pkexec")
        .args(["bash", "-c", script]).status()
        .map(|s| s.success()).unwrap_or(false);
    if ok { Ok(Path::new(FLAG).exists()) }
    else { Err("could not change the community setting (elevation cancelled or failed)".into()) }
}
