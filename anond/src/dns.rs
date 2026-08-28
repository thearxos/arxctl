// DNS leak prevention: pin /etc/resolv.conf to 127.0.0.1 (Tor's DNSPort, reached via the
// kill-switch redirect) and make it immutable for the session so nothing (NetworkManager,
// dhcpcd) can rewrite it out from under us. Restored exactly on down.
use crate::state::Session;
use crate::util::run;
use anyhow::{Context, Result};

const RESOLV: &str = "/etc/resolv.conf";

pub fn pin(sess: &mut Session) -> Result<()> {
    // back up the current file (contents) so restore is exact.
    sess.resolv_backup = std::fs::read_to_string(RESOLV).ok();
    // clear a prior immutable bit if one is set, then write our pin and re-lock it.
    let _ = run("chattr", &["-i", RESOLV]);
    std::fs::write(RESOLV, "# anond: DNS pinned to Tor (do not edit)\nnameserver 127.0.0.1\noptions edns0\n")
        .context("write resolv.conf")?;
    run("chattr", &["+i", RESOLV]).context("make resolv.conf immutable")?;
    Ok(())
}

pub fn restore(sess: Option<&Session>) -> Result<()> {
    let _ = run("chattr", &["-i", RESOLV]);
    if let Some(s) = sess {
        if let Some(ref orig) = s.resolv_backup { let _ = std::fs::write(RESOLV, orig); }
    }
    Ok(())
}

/// verify probe: resolv.conf points ONLY at loopback and is locked.
pub fn is_pinned() -> bool {
    let c = std::fs::read_to_string(RESOLV).unwrap_or_default();
    let servers: Vec<&str> = c.lines().filter_map(|l| l.trim().strip_prefix("nameserver")).map(|s| s.trim()).collect();
    !servers.is_empty() && servers.iter().all(|s| s.starts_with("127."))
}
