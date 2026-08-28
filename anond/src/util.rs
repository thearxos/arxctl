// Small shared helpers. Orchestration shells out to the system tools (tor, nft, ip, chattr)
// which is the honest, reliable way to drive them; the safety logic stays in Rust.
use anyhow::{bail, Context, Result};
use std::process::Command;

pub const STATE_DIR: &str = "/var/lib/anond";
pub const RUN_DIR: &str = "/run/anond";

// one-call libc shim so we do not pull the whole `libc` crate just for the uid check
extern "C" { fn geteuid() -> u32; }

pub fn require_root() -> Result<()> {
    // SAFETY: geteuid() only reads our own effective uid; no memory is touched, cannot UB.
    let euid = unsafe { geteuid() };
    if euid != 0 { bail!("anond must run as root (it manages nftables and Tor)"); }
    Ok(())
}

/// numeric uid of a system user (e.g. "tor"), via getent/id.
pub fn uid_of(user: &str) -> Result<u32> {
    let out = Command::new("id").args(["-u", user]).output().context("run id")?;
    if !out.status.success() { bail!("no such user '{user}' (is tor installed?)"); }
    String::from_utf8_lossy(&out.stdout).trim().parse().context("parse uid")
}

/// run a command, returning Err with context on non-zero exit.
pub fn run(bin: &str, args: &[&str]) -> Result<()> {
    let st = Command::new(bin).args(args).status().with_context(|| format!("spawn {bin}"))?;
    if !st.success() { bail!("{bin} {} failed", args.join(" ")); }
    Ok(())
}

/// run a command, capturing stdout (trimmed). Errors are swallowed to an empty string.
pub fn out(bin: &str, args: &[&str]) -> String {
    Command::new(bin).args(args).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

pub fn ensure_dirs() -> Result<()> {
    std::fs::create_dir_all(STATE_DIR).ok();
    std::fs::create_dir_all(RUN_DIR).ok();
    Ok(())
}
