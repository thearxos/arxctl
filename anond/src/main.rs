// anond — the ArxOS anonymity daemon. Brings up Tor behind a fail-closed nftables kill-switch
// as ONE system: the kill-switch goes up BEFORE Tor and comes down LAST, so if any layer is
// unhealthy, traffic is BLOCKED, never leaked. Rust so a bug is a compile error, not a silent
// deanonymization. i2p is a planned second layer (design step 2). Ships bundled inside arxctl
// and as a standalone downloadable binary (private source).
mod util;
mod state;
mod killswitch;
mod tor;
mod i2p;
mod dns;
mod harden;
mod verify;

use anyhow::{bail, Result};
use state::State;

// A world-readable snapshot (no secrets — just the state and exit IP) so the GUI, running as
// the desktop user, can show status without reading the root-only session file.
fn write_pub(state: &str, exit_ip: &str) {
    let _ = util::ensure_dirs();
    let p = format!("{}/pub.json", util::RUN_DIR);
    if std::fs::write(&p, format!("{{\"state\":\"{state}\",\"exit_ip\":\"{exit_ip}\"}}")).is_ok() {
        let _ = std::process::Command::new("chmod").args(["644", &p]).status();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("status");
    let rest = if args.is_empty() { &[][..] } else { &args[1..] };
    let r = match cmd {
        "up" | "start" | "on" => up(rest),
        "down" | "stop" | "off" | "-k" => down(),
        "status" => status(),
        "verify" | "safe" | "--safe" | "test" => verify::run().map(|_| ()),
        "new-identity" | "newid" | "newnym" => tor::new_identity(),
        "version" | "--version" | "-V" => { println!("anond {}", env!("CARGO_PKG_VERSION")); Ok(()) }
        _ => { eprintln!("anond: up [--mac] | down | status | verify | new-identity"); std::process::exit(2); }
    };
    if let Err(e) = r { eprintln!("anond: {e:#}"); std::process::exit(1); }
}

fn up(args: &[String]) -> Result<()> {
    util::require_root()?;
    if state::load().map(|s| s.state == State::Active).unwrap_or(false) {
        println!("anond is already active"); return Ok(());
    }
    let tor_uid = util::uid_of("tor")?;
    let want_i2p = args.iter().any(|a| a == "--i2p");
    let mut sess = state::Session::new(tor_uid);
    sess.i2p = want_i2p;
    // kill-switch uid exemptions: Tor always; i2pd too when the overlay is requested (both
    // egress DIRECTLY under their own uid to build their circuits/tunnels).
    let mut uids = vec![tor_uid];
    if want_i2p { uids.push(util::uid_of("i2pd")?); }

    // 1. LOCK FIRST: nothing egresses until we are proven anonymous.
    println!("[1/5] arming the kill-switch (traffic blocked until verified)…");
    killswitch::up(&uids)?;
    sess.state = State::Locked; sess.save()?;

    // From here, ANY failure must leave the kill-switch up. We tear down cleanly only via `down`.
    let result = (|| -> Result<verify::Verify> {
        // 2. harden (reversible)
        println!("[2/5] hardening the host (swap off, sysctl{})…", if args.iter().any(|a| a == "--mac") { ", MAC spoof" } else { "" });
        harden::apply(&mut sess, args.iter().any(|a| a == "--mac"))?; sess.save()?;
        // 3. pin DNS to Tor and lock it
        println!("[3/5] pinning DNS to Tor…");
        dns::pin(&mut sess)?; sess.save()?;
        // 4. bring up Tor and WAIT for a real bootstrap
        println!("[4/5] starting Tor and waiting for bootstrap…");
        sess.state = State::Bootstrapping; sess.save()?;
        tor::start()?;
        tor::wait_bootstrap(std::time::Duration::from_secs(120))?;
        // 4b. optional i2p overlay. i2pd builds tunnels in the background; we wait only for its
        // proxy to come up (fast), not the slow full tunnel build (eepsites take a few minutes).
        if want_i2p {
            // i2p is ADDITIVE: never let a slow/failed overlay drop the working Tor session.
            println!("      + starting i2pd (i2p overlay)…");
            match i2p::start() {
                Ok(_) => if i2p::wait_ready(std::time::Duration::from_secs(75)).is_err() {
                    println!("      note: i2pd still initialising (tunnels build in the background)");
                },
                Err(e) => println!("      note: i2p unavailable ({e:#}); continuing on Tor only"),
            }
        }
        // 5. PROVE it before declaring Active
        println!("[5/5] verifying (no path to Active until every probe passes)…");
        Ok(verify::run()?)
    })();

    match result {
        Ok(v) if v.active() => {
            sess.state = State::Active; sess.save()?;
            write_pub("Active", &v.exit_ip);
            println!("\nanond ACTIVE — you exit via Tor at {}. DNS pinned, IPv6 blocked, kill-switch armed.", v.exit_ip);
            Ok(())
        }
        Ok(_) => {
            // probes failed: STAY LOCKED (blocked), do not leak. `anond down` to release.
            sess.state = State::Locked; sess.save()?; write_pub("Locked", "");
            bail!("verification failed — staying LOCKED (all traffic blocked). Run `anond verify` for detail, or `anond down` to release.")
        }
        Err(e) => {
            sess.state = State::Locked; sess.save()?; write_pub("Locked", "");
            bail!("bring-up failed ({e:#}) — staying LOCKED (all traffic blocked). Run `anond down` to release.")
        }
    }
}

fn down() -> Result<()> {
    util::require_root()?;
    let sess = state::load();
    // DRAIN in reverse; egress stays blocked until the kill-switch comes down LAST.
    if sess.as_ref().map(|s| s.i2p).unwrap_or(false) || i2p::running() { let _ = i2p::stop(); }
    let _ = tor::stop();
    let _ = dns::restore(sess.as_ref());
    if let Some(ref s) = sess { let _ = harden::restore(s); }
    killswitch::down()?; // last
    state::clear()?;
    write_pub("Down", "");
    println!("anond DOWN — Tor stopped, DNS/host restored, kill-switch removed last.");
    Ok(())
}

fn status() -> Result<()> {
    match state::load() {
        Some(s) => {
            println!("state\t{:?}", s.state);
            println!("tor\t{}", if tor::running() { "running" } else { "stopped" });
            println!("killswitch\t{}", if killswitch::is_up() { "armed" } else { "down" });
            println!("dns\t{}", if dns::is_pinned() { "pinned" } else { "open" });
            println!("i2p\t{}", if i2p::running() { "running" } else { "off" });
            if s.state == State::Active {
                if let Ok(ip) = verify::exit_ip() { println!("exit_ip\t{ip}"); }
            }
        }
        None => println!("state\tDown"),
    }
    Ok(())
}
