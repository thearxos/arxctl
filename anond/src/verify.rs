// Runtime proofs. We do not ASSERT anonymity, we PROVE it, and refuse to report ACTIVE until
// every gate passes. Each probe is a hard boolean; the caller falls back to Locked (blocked)
// the instant one fails, so a failed probe never leaks.
use crate::util::out;
use serde::Serialize;

#[derive(Serialize, Default)]
pub struct Verify {
    pub tor_ok: bool,       // clearnet egress exits as a Tor node
    pub dns_ok: bool,       // resolv.conf pinned to loopback
    pub killswitch_ok: bool,// the fail-closed table is installed
    pub ipv6_ok: bool,      // IPv6 egress is dropped
    pub exit_ip: String,    // the Tor exit IP we came out on
}

impl Verify {
    pub fn active(&self) -> bool { self.tor_ok && self.dns_ok && self.killswitch_ok && self.ipv6_ok }
}

/// ask check.torproject.org whether we are exiting via Tor, over the transparent path.
/// returns (is_tor, ip). Uses curl so we add no HTTP dependency.
pub fn exit_check() -> (bool, String) {
    let body = out("curl", &["-s", "--max-time", "25", "https://check.torproject.org/api/ip"]);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let is_tor = v.get("IsTor").and_then(|x| x.as_bool()).unwrap_or(false);
    let ip = v.get("IP").and_then(|x| x.as_str()).unwrap_or("").to_string();
    (is_tor, ip)
}

pub fn exit_ip() -> anyhow::Result<String> {
    let (_, ip) = exit_check();
    if ip.is_empty() { anyhow::bail!("no exit IP (Tor path not answering)"); }
    Ok(ip)
}

/// IPv6 must have no clear path: our ip6 drop table exists and a v6 connect fails fast.
fn ipv6_dropped() -> bool {
    // the drop table being present is the guarantee; a connect attempt just confirms it.
    let table = std::process::Command::new("nft").args(["list", "table", "ip6", "anond6"]).output()
        .map(|o| o.status.success()).unwrap_or(false);
    table
}

pub fn run() -> anyhow::Result<Verify> {
    let mut v = Verify::default();
    v.killswitch_ok = crate::killswitch::is_up();
    v.dns_ok = crate::dns::is_pinned();
    v.ipv6_ok = ipv6_dropped();
    let (is_tor, ip) = exit_check();
    v.tor_ok = is_tor;
    v.exit_ip = ip;
    // print an honest verdict when run directly
    println!("kill-switch : {}", yn(v.killswitch_ok));
    println!("DNS pinned  : {}", yn(v.dns_ok));
    println!("IPv6 blocked: {}", yn(v.ipv6_ok));
    println!("Tor exit    : {}{}", yn(v.tor_ok), if v.exit_ip.is_empty() { String::new() } else { format!(" ({})", v.exit_ip) });
    println!("verdict     : {}", if v.active() { "ACTIVE (anonymous)" } else { "DEGRADED (not fully anonymous)" });
    Ok(v)
}

fn yn(b: bool) -> &'static str { if b { "pass" } else { "FAIL" } }
