// The kill-switch: typed, fail-closed nftables, applied via `nft -f` (reliable, and the
// design's sanctioned path). INVARIANT: default-drop egress. Only loopback, Tor's own uid,
// established flows, the LAN, and the transparent redirects are permitted. IPv6 is dropped
// entirely (a common leak vector). It is installed BEFORE Tor and removed LAST, so an
// unhealthy layer can never open a clear path.
//
// Transparent proxy flow: clearnet TCP from any non-Tor uid is REDIRECTed to Tor's TransPort
// (9040); DNS (53) is REDIRECTed to Tor's DNSPort (5353). After redirect the packet's daddr
// is 127.0.0.1, which the filter chain accepts; Tor then egresses under its own uid, which is
// the one uid the filter chain lets out. Nothing else escapes.
use anyhow::{Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

pub const TRANS_PORT: u16 = 9040;
pub const DNS_PORT: u16 = 5353;
// The local network stays reachable so an admin session (e.g. SSH) is not cut. This mirrors
// anonkit's behaviour; it is a deliberate usability/lockout tradeoff, documented as such.
const LAN: &str = "192.168.0.0/16";

fn ruleset(tor_uid: u32) -> String {
    format!(
        "table inet anond {{\n\
         \tchain output_nat {{\n\
         \t\ttype nat hook output priority -100; policy accept;\n\
         \t\tmeta skuid {tor_uid} return\n\
         \t\tudp dport 53 redirect to :{DNS_PORT}\n\
         \t\ttcp dport 53 redirect to :{DNS_PORT}\n\
         \t\tip daddr {{ 127.0.0.0/8, {LAN} }} return\n\
         \t\ttcp flags & (fin|syn|rst|ack) == syn redirect to :{TRANS_PORT}\n\
         \t}}\n\
         \tchain output {{\n\
         \t\ttype filter hook output priority 0; policy drop;\n\
         \t\toif \"lo\" accept\n\
         \t\tmeta skuid {tor_uid} accept\n\
         \t\tct state established,related accept\n\
         \t\tip daddr {{ 127.0.0.0/8, {LAN} }} accept\n\
         \t\tudp dport {DNS_PORT} accept\n\
         \t\ttcp dport {{ {TRANS_PORT}, {DNS_PORT} }} accept\n\
         \t}}\n\
         }}\n\
         table ip6 anond6 {{\n\
         \tchain output6 {{\n\
         \t\ttype filter hook output priority 0; policy drop;\n\
         \t\toif \"lo\" accept\n\
         \t}}\n\
         }}\n"
    )
}

pub fn up(tor_uid: u32) -> Result<()> {
    // clear any stale copy first, then apply atomically from stdin.
    down_quiet();
    let mut child = Command::new("nft").args(["-f", "-"]).stdin(Stdio::piped())
        .spawn().context("spawn nft (is nftables installed?)")?;
    child.stdin.take().context("nft stdin")?.write_all(ruleset(tor_uid).as_bytes())?;
    let st = child.wait().context("wait nft")?;
    anyhow::ensure!(st.success(), "kill-switch ruleset was rejected by nft");
    Ok(())
}

pub fn down() -> Result<()> { down_quiet(); Ok(()) }

fn down_quiet() {
    // deleting a table that isn't there is expected on the first `up`; hide nft's noise.
    let _ = Command::new("nft").args(["delete", "table", "inet", "anond"]).stderr(Stdio::null()).status();
    let _ = Command::new("nft").args(["delete", "table", "ip6", "anond6"]).stderr(Stdio::null()).status();
}

/// is the kill-switch table currently installed? (a verify probe)
pub fn is_up() -> bool {
    Command::new("nft").args(["list", "table", "inet", "anond"]).output()
        .map(|o| o.status.success()).unwrap_or(false)
}
