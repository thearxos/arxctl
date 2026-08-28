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
    // RunAsDaemon makes `tor -f` fork and return once the config is validated.
    run("tor", &["-f", &torrc_path()]).context("start tor (is tor installed?)")?;
    Ok(())
}

pub fn wait_bootstrap(timeout: Duration) -> Result<()> {
    let start = Instant::now();
    loop {
        let log = std::fs::read_to_string(log_path()).unwrap_or_default();
        if log.contains("Bootstrapped 100%") { return Ok(()); }
        if log.contains("[err]") { bail!("tor reported an error during bootstrap (see {})", log_path()); }
        if start.elapsed() > timeout { bail!("tor bootstrap timed out after {}s (staying blocked)", timeout.as_secs()); }
        std::thread::sleep(Duration::from_millis(500));
    }
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

/// true if a Tor process we started is alive.
pub fn running() -> bool {
    std::fs::read_to_string(pid_path()).ok()
        .map(|p| !out("kill", &["-0", p.trim()]).is_empty() || std::path::Path::new(&format!("/proc/{}", p.trim())).exists())
        .unwrap_or(false)
}
