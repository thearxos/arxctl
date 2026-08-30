// i2p layer (opt-in via `anond up --i2p`). i2p is a separate overlay with no TransPort, so it
// is reached through i2pd's proxies (HTTP 4444 / SOCKS 4447), not transparent IP NAT. i2pd
// egresses DIRECTLY under its own uid (the kill-switch exempts it, exactly like Tor), building
// its own tunnels; the kill-switch still contains everything else. `.i2p` names resolve via
// i2pd's addressbook through the proxy, never a clearnet resolver, so there is no DNS leak.
use crate::util::{out, run, STATE_DIR};
use anyhow::{bail, Context, Result};
use std::time::{Duration, Instant};

pub const HTTP_PROXY: u16 = 4444;
pub const SOCKS_PROXY: u16 = 4447;
pub const CONSOLE: u16 = 7070;

fn data_dir() -> String { format!("{STATE_DIR}/i2pd") }
fn pid_path() -> String { format!("{}/i2pd.pid", data_dir()) }
fn log_path() -> String { format!("{}/i2pd.log", data_dir()) }

pub fn start() -> Result<()> {
    std::fs::create_dir_all(data_dir()).ok();
    run("chown", &["-R", "i2pd:i2pd", &data_dir()]).ok();
    // run AS the i2pd user so its clearnet egress carries the i2pd uid (kill-switch exempts it).
    let status = std::process::Command::new("runuser")
        .args(["-u", "i2pd", "--", "i2pd",
            &format!("--datadir={}", data_dir()),
            &format!("--pidfile={}", pid_path()),
            "--log=file", &format!("--logfile={}", log_path()), "--loglevel=warn", "--daemon",
            "--http.address=127.0.0.1", &format!("--http.port={CONSOLE}"),
            "--httpproxy.address=127.0.0.1", &format!("--httpproxy.port={HTTP_PROXY}"),
            "--socksproxy.address=127.0.0.1", &format!("--socksproxy.port={SOCKS_PROXY}"),
            "--sam.enabled=false"])
        .status().context("start i2pd (is i2pd installed?)")?;
    if !status.success() { bail!("i2pd failed to start"); }
    Ok(())
}

/// wait for i2pd's HTTP proxy to accept connections (the router process is up and listening).
/// Full tunnel build (needed to actually load eepsites) can take minutes beyond this.
pub fn wait_ready(timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() <= timeout {
        if proxy_up() { return Ok(()); }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!("i2pd did not open its proxy within {}s", timeout.as_secs())
}

pub fn stop() -> Result<()> {
    if let Ok(pid) = std::fs::read_to_string(pid_path()) {
        let pid = pid.trim();
        if !pid.is_empty() { let _ = run("kill", &[pid]); }
    }
    // i2pd is the only process owned by the i2pd user, so this is a safe catch-all.
    let _ = std::process::Command::new("pkill").args(["-u", "i2pd"]).status();
    // i2pd's graceful shutdown (flushing netdb + closing tunnels) is slow; give it a moment,
    // then force any straggler so `down` actually leaves nothing behind.
    std::thread::sleep(Duration::from_secs(2));
    let _ = std::process::Command::new("pkill").args(["-9", "-u", "i2pd"]).status();
    let _ = std::fs::remove_file(pid_path());
    Ok(())
}

pub fn running() -> bool {
    std::fs::read_to_string(pid_path()).ok()
        .and_then(|p| p.trim().parse::<u32>().ok())
        .map(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists())
        .unwrap_or(false)
}

pub fn proxy_up() -> bool {
    "127.0.0.1:4444".parse().ok()
        .and_then(|a| std::net::TcpStream::connect_timeout(&a, Duration::from_millis(400)).ok())
        .is_some()
}

/// best-effort proof that the overlay works: load a known eepsite through the HTTP proxy.
/// i2p tunnels are slow to build, so this is informational, not a hard gate on Tor anonymity.
pub fn eepsite_ok() -> bool {
    let code = out("curl", &["-s", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "60",
        "-x", &format!("http://127.0.0.1:{HTTP_PROXY}"), "http://i2p-projekt.i2p/"]);
    code.starts_with('2') || code.starts_with('3')
}
