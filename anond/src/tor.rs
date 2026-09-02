// Tor supervision: generate a torrc (TransPort + DNSPort + SocksPort, IPv6 off, strong
// stream isolation), start Tor under its own uid, and WAIT for a real bootstrap before we
// let the caller declare Active. If bootstrap stalls, we return Err and the caller stays
// Locked (blocked) — no code path leaks on a stalled Tor.
use crate::killswitch::{DNS_PORT, TRANS_PORT};
use crate::util::{out, run, RUN_DIR, STATE_DIR};
use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

fn torrc_path() -> String { format!("{RUN_DIR}/torrc") }
fn log_path() -> String { format!("{STATE_DIR}/tor.log") }
fn pid_path() -> String { format!("{RUN_DIR}/tor.pid") }
fn data_dir() -> String { format!("{STATE_DIR}/tor") }

fn write_torrc() -> Result<()> {
    crate::util::ensure_dirs()?;
    std::fs::create_dir_all(data_dir()).ok();
    // tor drops to its own user, so everything it WRITES (data dir, pidfile dir, log) must be
    // tor-owned. session.json stays in STATE_DIR root-owned, so we chown targets surgically
    // rather than the whole STATE_DIR.
    run("chown", &["-R", "tor:tor", &data_dir()]).ok();  // DataDirectory
    run("chown", &["tor:tor", RUN_DIR]).ok();            // PidFile lives here
    let _ = std::fs::File::create(log_path());
    run("chown", &["tor:tor", &log_path()]).ok();
    let rc = format!(
        "User tor\n\
         DataDirectory {data}\n\
         PidFile {pid}\n\
         Log notice file {log}\n\
         RunAsDaemon 1\n\
         SocksPort 9050 IsolateDestAddr IsolateDestPort\n\
         TransPort {trans}\n\
         DNSPort {dns}\n\
         ControlPort 9051\n\
         CookieAuthentication 1\n\
         AutomapHostsOnResolve 1\n\
         VirtualAddrNetworkIPv4 10.192.0.0/10\n\
         ClientUseIPv6 0\n\
         ClientPreferIPv6ORPort 0\n\
         AvoidDiskWrites 1\n",
        data = data_dir(), pid = pid_path(), log = log_path(), trans = TRANS_PORT, dns = DNS_PORT,
    );
    std::fs::write(torrc_path(), rc).context("write torrc")?;
    Ok(())
}

pub fn start() -> Result<()> {
    write_torrc()?;
    // truncate the log so bootstrap detection reads THIS run.
    let _ = std::fs::write(log_path(), b"");
    start_inner()
}

// Launch tor with the already-written torrc, truncating the log first so crash-restart
// detection reads only the CURRENT attempt (a stale "Caught signal" from a prior run must not
// re-trigger the restart logic). Used by start() and by wait_bootstrap()'s crash-restart.
fn start_inner() -> Result<()> {
    let _ = std::fs::write(log_path(), b"");
    // RunAsDaemon makes `tor -f` fork and return once the config is validated.
    run("tor", &["-f", &torrc_path()]).context("start tor (is tor installed?)")?;
    Ok(())
}

/// The latest Tor bootstrap percentage from the log (0-100), for a live progress display.
pub fn bootstrap_pct() -> u8 {
    let log = std::fs::read_to_string(log_path()).unwrap_or_default();
    log.rmatch_indices("Bootstrapped ").next()
        .and_then(|(i, _)| log[i + 13..].split('%').next())
        .and_then(|s| s.trim().parse::<u8>().ok())
        .unwrap_or(0)
}

/// Called each poll while waiting, so a GUI reading the world-readable snapshot can show live
/// progress ("Bootstrapping 56% — this can take a few minutes") instead of a frozen-looking UI.
pub fn wait_bootstrap_with<F: FnMut(u8)>(timeout: Duration, mut on_pct: F) -> Result<()> {
    let start = Instant::now();
    let mut restarts = 0;
    const MAX_RESTARTS: u32 = 4;
    loop {
        let log = std::fs::read_to_string(log_path()).unwrap_or_default();
        on_pct(bootstrap_pct());
        if log.contains("Bootstrapped 100%") { return Ok(()); }
        if log.contains("[err]") { bail!("tor reported an error during bootstrap (see {})", log_path()); }
        // Detect a DEAD Tor process (a crash logs "died: Caught signal N", NOT "[err]", so the
        // old loop polled a corpse to the timeout and reported a false failure). The primary
        // cause of the crash — our own health-check probes connecting to Tor's TransPort, which
        // made Tor 0.4.9.11 SIGSEGV on getsockopt(SO_ORIGINAL_DST)=127.0.0.1 — is FIXED (all
        // probes now hit the SOCKS port only). This restart is the remaining safety net for any
        // OTHER Tor death: give the just-started process a grace period before judging it dead
        // (so a slow fork/init is not misread as a crash), restart a bounded number of times,
        // then fail closed. It never truncates the log mid-flight (that erased progress evidence
        // and caused false re-restarts).
        let signaled = log.contains("Caught signal");
        let dead = !running() && start.elapsed() > Duration::from_secs(3);
        if signaled || dead {
            if restarts >= MAX_RESTARTS {
                bail!("tor died repeatedly during bootstrap ({restarts} restarts; see {}). Staying blocked.", log_path());
            }
            restarts += 1;
            let _ = stop();
            std::thread::sleep(Duration::from_millis(500));
            start_inner()?; // relaunch tor with the same torrc (start_inner truncates the log)
            std::thread::sleep(Duration::from_secs(2)); // let it fork + start writing before re-judging
            continue;
        }
        if start.elapsed() > timeout { bail!("tor bootstrap timed out after {}s (staying blocked)", timeout.as_secs()); }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Back-compat: wait with no progress callback.
pub fn wait_bootstrap(timeout: Duration) -> Result<()> {
    wait_bootstrap_with(timeout, |_| {})
}

pub fn stop() -> Result<()> {
    if let Ok(pid) = std::fs::read_to_string(pid_path()) {
        let pid = pid.trim();
        if !pid.is_empty() { let _ = run("kill", &[pid]); }
    }
    // belt and braces: our torrc is unique to anond, so pkill by config path is safe.
    let _ = std::process::Command::new("pkill").args(["-f", &torrc_path()]).status();
    let _ = std::fs::remove_file(pid_path());
    Ok(())
}

/// request a fresh circuit (NEWNYM) over the control port with cookie auth.
pub fn new_identity() -> Result<()> {
    crate::util::require_root()?;
    let cookie = std::fs::read(format!("{}/control_auth_cookie", data_dir()))
        .context("read tor control cookie (is anond up?)")?;
    let hex: String = cookie.iter().map(|b| format!("{b:02x}")).collect();
    let mut s = std::net::TcpStream::connect("127.0.0.1:9051").context("connect tor control port")?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(format!("AUTHENTICATE {hex}\r\nSIGNAL NEWNYM\r\nQUIT\r\n").as_bytes())?;
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    if resp.contains("250 OK") { println!("new identity requested"); Ok(()) }
    else { bail!("tor refused NEWNYM: {}", resp.lines().next().unwrap_or("").trim()) }
}

/// true if a Tor process we started is alive. Checks /proc/<pid> directly — the previous
/// `!out("kill","-0",pid).is_empty()` was a bug: `kill -0` prints NOTHING on success (its result
/// is the exit code, and any error goes to stderr), so that operand was always false and liveness
/// rested entirely on the /proc fallback. Verify it is actually a tor process (not a recycled pid)
/// by reading /proc/<pid>/comm, so a stale PidFile pointing at some other process cannot read as
/// "tor running".
pub fn running() -> bool {
    let Some(pid) = std::fs::read_to_string(pid_path()).ok().map(|p| p.trim().to_string()) else { return false };
    if pid.is_empty() { return false; }
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|c| c.trim() == "tor").unwrap_or(false)
}
