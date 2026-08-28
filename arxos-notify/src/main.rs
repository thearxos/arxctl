// arxos-notify — the ArxOS update ping. A tiny, idle-friendly check (run by a systemd
// user timer, not a resident daemon) that:
//   1. counts the system + kernel + ArxOS-tool updates waiting locally, and
//   2. fetches the GLOBAL update channel from the CDN (a Cloudflare-fronted JSON),
// then fires ONE desktop notification when there is something new for the user — updates
// waiting, or a worldwide broadcast (a security notice, a new release). No push socket
// to keep open per user: a cheap poll of an edge-cached file reaches every ArxOS machine
// on earth within the interval, at basically zero cost or idle draw. It never nags: it
// re-notifies about updates only when the pending set actually changes, and about a
// broadcast only once per message id.
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

const CHANNEL_URL: &str = "https://arxos.uk/update-channel.json";

fn have(bin: &str) -> bool { std::env::var("PATH").unwrap_or_default().split(':').any(|d| std::path::Path::new(d).join(bin).exists()) }
fn out(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

fn cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| format!("{}/.cache", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())));
    let d = PathBuf::from(base).join("arxos");
    let _ = std::fs::create_dir_all(&d);
    d
}

// how many updates are waiting: prefer checkupdates (pacman-contrib, safe/no-sudo),
// fall back to arx's own count.
fn pending_updates() -> usize {
    let text = if have("checkupdates") { out("checkupdates", &[]) } else { out("arx", &["outdated"]) };
    text.lines().filter(|l| !l.trim().is_empty()).count()
}

// the global update channel: a small JSON at the CDN edge. Absent/unreachable is fine
// (we just skip the broadcast this run).
fn fetch_channel() -> Option<Value> {
    let o = Command::new("curl").args(["-fsSL", "--max-time", "15", CHANNEL_URL]).output().ok()?;
    if !o.status.success() { return None; }
    serde_json::from_slice(&o.stdout).ok()
}

fn notify(title: &str, body: &str, icon: &str, urgent: bool) {
    if !have("notify-send") { return; }
    let mut a = vec!["-a", "ArxOS", "-i", icon, "-u", if urgent { "critical" } else { "normal" }, title, body];
    // keep it on-screen a touch longer for a real notice
    if urgent { a.splice(0..0, ["-t", "0"]); }
    let _ = Command::new("notify-send").args(&a).status();
}

fn main() {
    let cache = cache_dir();
    let state_path = cache.join("notify-state.json");
    let state: Value = std::fs::read_to_string(&state_path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let last_count = state.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let last_bcast = state.get("broadcast").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let updates = pending_updates();
    // the count file arxctl reads for its badge (written every run, always current)
    let _ = std::fs::write(cache.join("update-count"), updates.to_string());

    let channel = fetch_channel();
    let mut lines: Vec<String> = Vec::new();
    let mut urgent = false;

    // (1) local updates — notify only when the pending set CHANGED (0->N, or a new N).
    if updates > 0 && updates != last_count {
        lines.push(format!("{updates} update{} ready. Open the Control Center or run: arx update", if updates == 1 { "" } else { "s" }));
    }

    // (2) global broadcast — notify once per new message id (a security notice, a release).
    let mut cur_bcast = last_bcast.clone();
    if let Some(b) = channel.as_ref().and_then(|c| c.get("broadcast")) {
        let id = b.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if !id.is_empty() && id != last_bcast {
            let title = b.get("title").and_then(|v| v.as_str()).unwrap_or("ArxOS");
            let body = b.get("body").and_then(|v| v.as_str()).unwrap_or("");
            let sev = b.get("severity").and_then(|v| v.as_str()).unwrap_or("info");
            if sev == "critical" || sev == "security" { urgent = true; }
            lines.push(if body.is_empty() { title.to_string() } else { format!("{title} — {body}") });
            cur_bcast = id.to_string();
        }
    }

    if !lines.is_empty() && !std::env::args().any(|a| a == "--silent") {
        let icon = if urgent { "security-high" } else { "software-update-available" };
        notify("ArxOS", &lines.join("\n"), icon, urgent);
    }

    let new_state = serde_json::json!({ "count": updates, "broadcast": cur_bcast });
    let _ = std::fs::write(&state_path, new_state.to_string());

    // a --check run prints a one-line status (used by the installer + for debugging)
    if std::env::args().any(|a| a == "--check") {
        println!("updates={updates} broadcast={cur_bcast} notified={}", lines.len());
    }
}
