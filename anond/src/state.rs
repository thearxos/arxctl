// The fail-closed state machine + on-disk session (0600). One source of truth for what is
// up, so `down` can restore exactly what `up` changed, in reverse, kill-switch last.
//
//   Down ─lock▶ Locked ─bootstrap▶ Bootstrapping ─probes pass▶ Active
//     ▲                                  │                        │
//     └──────── unlock ◀── Draining ◀────┴──── any layer unhealthy┘
//   (egress stays BLOCKED from Locked through Draining)
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub enum State { Down, Locked, Bootstrapping, Active, Draining }

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub state: State,
    pub tor_uid: u32,
    pub since: String,
    /// backup of /etc/resolv.conf (raw contents), restored on down.
    pub resolv_backup: Option<String>,
    /// interface -> original MAC, restored on down (only set when --mac was used).
    pub mac_backup: Vec<(String, String)>,
    /// swap was on before the session, so we re-enable it on down.
    pub swap_was_on: bool,
}

impl Session {
    pub fn new(tor_uid: u32) -> Self {
        Session {
            state: State::Down, tor_uid,
            since: now(),
            resolv_backup: None, mac_backup: Vec::new(), swap_was_on: false,
        }
    }
    pub fn save(&self) -> Result<()> {
        crate::util::ensure_dirs()?;
        let path = format!("{}/session.json", crate::util::STATE_DIR);
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true)
            .mode(0o600).open(&path).context("open session file")?;
        f.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }
}

pub fn load() -> Option<Session> {
    let path = format!("{}/session.json", crate::util::STATE_DIR);
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn clear() -> Result<()> {
    let _ = std::fs::remove_file(format!("{}/session.json", crate::util::STATE_DIR));
    Ok(())
}

fn now() -> String {
    // seconds since epoch is enough; the UI formats it. Avoids a chrono dependency.
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string()).unwrap_or_default()
}
