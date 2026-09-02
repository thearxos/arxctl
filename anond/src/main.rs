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

// A world-readable snapshot (no secrets — state, exit IP, and each layer's live state) so the
// GUI, running as the desktop user, can show the FULL picture without reading the root-only
// session file and without shelling out to a terminal for it.
fn write_pub(state: &str, exit_ip: &str, leaking: bool) {
    write_pub_full(state, exit_ip, leaking, 0);
}

fn write_pub_full(state: &str, exit_ip: &str, leaking: bool, bootstrap_pct: u8) {
    let _ = util::ensure_dirs();
    let p = format!("{}/pub.json", util::RUN_DIR);
    let json = format!(
        "{{\"state\":\"{state}\",\"exit_ip\":\"{exit_ip}\",\"tor\":\"{}\",\"killswitch\":\"{}\",\"dns\":\"{}\",\"i2p\":\"{}\",\"leaking\":{leaking},\"bootstrap_pct\":{bootstrap_pct}}}",
        if tor::running() { "running" } else { "stopped" },
        if killswitch::is_up() { "armed" } else { "down" },
        if dns::is_pinned() { "pinned" } else { "open" },
        if i2p::running() { "running" } else { "off" },
    );
    if std::fs::write(&p, json).is_ok() {
        let _ = std::process::Command::new("chmod").args(["644", &p]).status();
    }
}

// Update the live snapshot during bootstrap so the GUI shows the real % (never a dead-looking panel).
fn write_pub_bootstrapping(pct: u8) { write_pub_full("Bootstrapping", "", false, pct); }

// arx-style segmented loader bar: filled ▰ / empty ▱, 20 segments, for the terminal progress line.
fn bar(pct: u8) -> String {
    let segs = 20usize;
    let filled = (pct as usize * segs / 100).min(segs);
    format!("{}{}", "▰".repeat(filled), "▱".repeat(segs - filled))
}

/// The reconciled truth about the session, NOT the state persisted on disk. This exists
/// because the persisted `State::Active` is only a *claim*: if Tor later dies (crash, OOM,
/// kill, suspend/resume) the on-disk state still says Active, and reporting that verbatim is
/// the one failure an anonymity tool must never have — telling the user they are protected
/// when they are not. Everything user- or GUI-facing goes through here so a dead session can
/// never masquerade as Active.
struct Health { effective: State, leaking: bool }
fn health() -> Health {
    let persisted = state::load().map(|s| s.state).unwrap_or(State::Down);
    // A claimed-Active session whose Tor process is gone is NOT active. What it actually is
    // depends on the kill-switch: if the switch still holds, traffic is BLOCKED (safe but not
    // anonymous) = Locked; if the switch is also down, traffic egresses in the clear = the
    // dangerous leak, reported as Down + leaking so nothing shows a false green.
    if persisted == State::Active && !tor::running() {
        let ks = killswitch::is_up();
        Health { effective: if ks { State::Locked } else { State::Down }, leaking: !ks }
    } else {
        Health { effective: persisted, leaking: false }
    }
}

/// Is a VPN tunnel already carrying the default route? Detects the common VPN link types
/// (WireGuard `wg*`, OpenVPN/other `tun*`, `proton*`, `mullvad*`) as the egress device, so the
/// VPN->Tor advisory is only shown when the user is NOT already tunnelled.
fn vpn_layer_present() -> bool {
    // the interface the default route currently leaves through
    let route = util::out("ip", &["-o", "route", "get", "1.1.1.1"]);
    let dev = route.split_whitespace().skip_while(|t| *t != "dev").nth(1).unwrap_or("");
    dev.starts_with("tun") || dev.starts_with("wg") || dev.starts_with("proton")
        || dev.starts_with("mullvad") || dev.starts_with("nordlynx") || dev.starts_with("tap")
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
    // Use the RECONCILED state, not the raw persisted claim: a session that says Active on disk
    // but whose Tor has died must NOT short-circuit here as "already active" — that would leave
    // the user unprotected while telling them everything is fine. Only a genuinely-live Active
    // session skips re-establishment.
    if health().effective == State::Active {
        // Already anonymous. One thing we still honor on a live session: ATTACHING the i2p overlay
        // (the GUI Privacy toggle flips i2p ON while already Active; a bare `up --i2p` used to
        // short-circuit here and silently do nothing). i2pd needs its uid in the kill-switch exempt
        // set to egress, so re-arm ATOMICALLY (no leak window), then start i2pd additively — a
        // failure never disturbs the working Tor session.
        if args.iter().any(|a| a == "--i2p") && !i2p::running() {
            let tor_uid = util::uid_of("tor")?;
            let i2pd_uid = util::uid_of("i2pd")?;
            println!("attaching the i2p overlay to the live session…");
            killswitch::rearm(&[tor_uid, i2pd_uid])?;
            match i2p::start() {
                Ok(_) => if i2p::wait_ready(std::time::Duration::from_secs(75)).is_err() {
                    println!("note: i2pd still initialising (tunnels build in the background)");
                },
                Err(e) => {
                    // additive/non-fatal: i2pd did not start, so drop its uid back out of the
                    // exempt set (leave the kill-switch exactly as tight as before) and keep Tor.
                    let _ = killswitch::rearm(&[tor_uid]);
                    println!("note: i2p unavailable ({e:#}); staying on Tor only");
                }
            }
            let running = i2p::running();
            if let Some(mut s) = state::load() { s.i2p = running; let _ = s.save(); }
            // refresh the world-readable snapshot so the GUI reflects i2p immediately (write_pub_full
            // recomputes tor/killswitch/dns/i2p live); keep the Active state, exit IP unchanged.
            write_pub("Active", &verify::exit_ip().unwrap_or_default(), false);
            println!("i2p overlay {}", if running { "attached (Tor session intact)" } else { "not attached (Tor session intact)" });
            return Ok(());
        }
        println!("anond is already active"); return Ok(());
    }
    // VPN->Tor advisory: Tor traffic is recognisable to the ISP (its TLS signature + known guard
    // IPs), so "this subscriber uses Tor" is itself a metadata leak in a targeted threat model.
    // Running a VPN FIRST (VPN->Tor) means the ISP only ever sees encrypted VPN traffic and never
    // learns Tor is in use. We advise it unless a tunnel is already up. Skippable with --no-advice.
    if !args.iter().any(|a| a == "--no-advice") && !vpn_layer_present() {
        eprintln!("\n  ADVISORY: no VPN layer detected in front of Tor.");
        eprintln!("  Your ISP can see that you are USING Tor (not what you do). To hide even that,");
        eprintln!("  connect a VPN FIRST so the ISP sees only encrypted VPN traffic, then re-run");
        eprintln!("  `anond up`. Use the VPN provider's own client with ITS kill-switch on, and a");
        eprintln!("  no-logs, anonymous-payment provider — Mullvad is the reference (cash/crypto,");
        eprintln!("  account-number only, audited, RAM-only). (Native VPN chaining is planned.)");
        eprintln!("  Continuing WITHOUT a VPN layer in 3s (Ctrl-C to stop, or pass --no-advice)…\n");
        std::thread::sleep(std::time::Duration::from_secs(3));
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
        println!("[4/5] starting Tor (this can take a few minutes on a slow connection)…");
        sess.state = State::Bootstrapping; sess.save()?;
        tor::start()?;
        // 300s not 120s: on a slow network/VM, Tor's descriptor-fetch phase (50-56%) can take
        // 3+ minutes behind the kill-switch (measured: a clean VM bootstrap reached 100% at ~209s).
        // The old 120s timeout fired mid-bootstrap and reported a false failure -> Locked, even
        // though Tor was healthy and still climbing. Fail-closed still holds: nothing egresses
        // until 100% + verify pass, so a longer wait costs latency, never safety.
        // Live progress: an arx-style segmented bar with the REAL % in the terminal, AND the %
        // pushed into the world-readable snapshot so the GUI shows "Bootstrapping NN%" instead
        // of a frozen-looking panel (the user asked: don't let it look dead).
        let mut last_pct = 255u8;
        tor::wait_bootstrap_with(std::time::Duration::from_secs(300), |pct| {
            if pct != last_pct {
                last_pct = pct;
                print!("\r  {}  bootstrapping Tor {pct:>3}%   ", bar(pct));
                use std::io::Write; let _ = std::io::stdout().flush();
                write_pub_bootstrapping(pct);   // GUI reads this live from pub.json
            }
        })?;
        println!();   // finish the progress line
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
            write_pub("Active", &v.exit_ip, false);
            println!("\nanond ACTIVE — you exit via Tor at {}. DNS pinned, IPv6 blocked, kill-switch armed.", v.exit_ip);
            Ok(())
        }
        Ok(_) => {
            // probes failed: STAY LOCKED (blocked), do not leak. `anond down` to release.
            sess.state = State::Locked; sess.save()?; write_pub("Locked", "", false);
            bail!("verification failed — staying LOCKED (all traffic blocked). Run `anond verify` for detail, or `anond down` to release.")
        }
        Err(e) => {
            sess.state = State::Locked; sess.save()?; write_pub("Locked", "", false);
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
    // RAM hygiene on stop (anonsurf-style): flush filesystem buffers, then drop the page cache,
    // dentries, and inodes so cached session data (fetched pages, resolved names, tmp reads) does
    // not linger in RAM after the anonymous session ends. Best-effort; needs the kernel knob.
    let _ = util::run("sync", &[]);
    let _ = std::fs::write("/proc/sys/vm/drop_caches", "3");
    write_pub("Down", "", false);
    println!("anond DOWN — Tor stopped, DNS/host restored, kill-switch removed last; RAM caches flushed.");
    Ok(())
}

fn status() -> Result<()> {
    let h = health();
    let claimed = state::load().map(|s| s.state);
    // Report the RECONCILED state, never the raw persisted claim. If reconciliation downgraded
    // a claimed-Active session, say so out loud rather than silently — the user needs to know
    // their protection dropped, not just see a quietly different word.
    println!("state\t{:?}", h.effective);
    if claimed == Some(State::Active) && h.effective != State::Active {
        println!("\t↳ was Active, but Tor is not running — anonymity has DROPPED.");
    }
    println!("tor\t{}", if tor::running() { "running" } else { "stopped" });
    println!("killswitch\t{}", if killswitch::is_up() { "armed" } else { "down" });
    println!("dns\t{}", if dns::is_pinned() { "pinned" } else { "open" });
    println!("i2p\t{}", if i2p::running() { "running" } else { "off" });
    if h.leaking {
        println!("\n*** LEAK: this session was Active, Tor is DEAD, and the kill-switch is DOWN.");
        println!("*** Traffic is egressing in the clear. Run `anond down` then `anond up` to restore,");
        println!("*** or `anond down` to stop and clean up.");
    }
    let mut ip = String::new();
    if h.effective == State::Active {
        if let Ok(v) = verify::exit_ip() { println!("exit_ip\t{v}"); ip = v; }
    }
    // refresh the public snapshot with the RECONCILED state so the GUI never shows a false green
    write_pub(&format!("{:?}", h.effective), &ip, h.leaking);
    Ok(())
}
