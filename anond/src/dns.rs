// DNS leak prevention: pin /etc/resolv.conf to 127.0.0.1 (Tor's DNSPort, reached via the
// kill-switch redirect) and make it immutable for the session so nothing (NetworkManager,
// dhcpcd) can rewrite it out from under us. Restored on down.
use crate::state::Session;
use crate::util::{out, run};
use anyhow::{Context, Result};

const RESOLV: &str = "/etc/resolv.conf";
const PIN: &str = "# anond: DNS pinned to Tor (do not edit)\nnameserver 127.0.0.1\noptions edns0\n";
const MARKER: &str = "# anond:";

pub fn pin(sess: &mut Session) -> Result<()> {
    // Back up the current file so restore is exact — but ONLY if it is a real resolver config.
    // If it is ALREADY our pin (a prior session crashed or was killed before restore, leaving the
    // pin in place), capturing it as the "original" would make restore write `nameserver 127.0.0.1`
    // back — a dead resolver once Tor stops — and silently break DNS after `down`. In that case
    // leave the backup untouched (a fresh Session has None, so restore reconstructs a working
    // resolver instead of resurrecting the dead pin).
    let current = std::fs::read_to_string(RESOLV).unwrap_or_default();
    if !current.contains(MARKER) && !current.trim().is_empty() {
        sess.resolv_backup = Some(current);
    }
    // clear a prior immutable bit if one is set, then write our pin and re-lock it.
    let _ = run("chattr", &["-i", RESOLV]);
    std::fs::write(RESOLV, PIN).context("write resolv.conf")?;
    run("chattr", &["+i", RESOLV]).context("make resolv.conf immutable")?;
    Ok(())
}

pub fn restore(sess: Option<&Session>) -> Result<()> {
    let _ = run("chattr", &["-i", RESOLV]);
    // 1. exact restore when we have a REAL backup (not our own pin, not empty).
    if let Some(s) = sess {
        if let Some(orig) = s.resolv_backup.as_ref()
            .filter(|o| !o.contains(MARKER) && !o.trim().is_empty())
        {
            std::fs::write(RESOLV, orig).context("restore resolv.conf")?;
            return Ok(());
        }
    }
    // 2. No usable backup (a prior session was killed before restore, so the pin got captured as
    //    the "original", or none was captured). Do NOT leave the dead `nameserver 127.0.0.1` pin
    //    in place — that would break all name resolution after `down`. Reconstruct a working
    //    resolver so the box has DNS again (the user ran `down` to be back on normal clearnet).
    reconstruct_resolver();
    Ok(())
}

// Best-effort reconstruction of a working /etc/resolv.conf when no pre-session backup survives.
// Ordered by correctness: systemd-resolved's stub if it runs, else the default gateway (home
// routers and the libvirt NAT both forward DNS), else a public resolver as a last resort so the
// box is never left with no DNS at all. Marked so a later real session can tell it apart.
fn reconstruct_resolver() {
    let write = |body: &str| { let _ = std::fs::write(RESOLV, body); };
    // systemd-resolved owns DNS on many systems; its stub listener is 127.0.0.53.
    if run("systemctl", &["is-active", "--quiet", "systemd-resolved"]).is_ok() {
        write("# anond: no pre-session backup; pointed at systemd-resolved stub\nnameserver 127.0.0.53\noptions edns0 trust-ad\n");
        return;
    }
    // else derive the default-route gateway (it typically forwards DNS).
    let route = out("ip", &["-o", "route", "show", "default"]);
    let gw = route.split_whitespace().skip_while(|t| *t != "via").nth(1).unwrap_or("");
    if !gw.is_empty() {
        write(&format!("# anond: no pre-session backup; pointed at the default gateway\nnameserver {gw}\n"));
        return;
    }
    // last resort: a box with no DNS is worse than a public resolver post-`down` (clearnet already).
    write("# anond: no pre-session backup; public resolver fallback\nnameserver 1.1.1.1\n");
}

/// verify probe: resolv.conf points ONLY at loopback and is locked.
pub fn is_pinned() -> bool {
    let c = std::fs::read_to_string(RESOLV).unwrap_or_default();
    let servers: Vec<&str> = c.lines().filter_map(|l| l.trim().strip_prefix("nameserver")).map(|s| s.trim()).collect();
    !servers.is_empty() && servers.iter().all(|s| s.starts_with("127."))
}
