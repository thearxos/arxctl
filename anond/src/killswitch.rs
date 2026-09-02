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
//
// ALL private ranges, not just 192.168/16: traffic to a private address must be RETURNed (not
// SYN-redirected to Tor's TransPort). If a private-dest packet reaches the TransPort, Tor
// rejects it ("Rejecting request for anonymous connection to private address ... Possible loop
// in your NAT rules?") and stalls bootstrap at 0% (and, on some tor builds, SIGSEGVs). A box
// that talks to a 10.x/172.16.x LAN, or does any private-range probe during bring-up, hit
// exactly this. Covering every RFC1918 + link-local + CGNAT range fixes the intermittent
// "stuck at Bootstrapped 0%" and is also correct (private dests are LAN, never Tor's job).
const PRIVATE: &str = "127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16, 100.64.0.0/10";

fn ruleset(exempt_uids: &[u32]) -> String {
    // exempted uids (Tor, and i2pd when --i2p) egress DIRECTLY: their traffic is neither
    // redirected into Tor nor dropped. Everything else clearnet is forced through Tor.
    let uids = exempt_uids.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(", ");
    format!(
        "table inet anond {{\n\
         \tchain output_nat {{\n\
         \t\ttype nat hook output priority -100; policy accept;\n\
         \t\t# Tor's OWN traffic (and i2pd's) is returned FIRST, before any redirect — otherwise\n\
         \t\t# Tor's DNS lookups to its own DNSPort, and connections to its AutomapHostsOnResolve\n\
         \t\t# virtual range, get redirected back into its TransPort. A redirected connection whose\n\
         \t\t# original dest is that virtual/private address makes Tor 0.4.9.11 reject it ('private\n\
         \t\t# address on a TransPort ... Possible loop in your NAT rules?') and SIGSEGV. Exempting\n\
         \t\t# the tor uid up front breaks that loop.\n\
         \t\tmeta skuid {{ {uids} }} return\n\
         \t\t# private/LAN/loopback AND Tor's automap virtual range are never redirected to Tor:\n\
         \t\t# they are LAN or Tor-internal, never Tor's job, and a private original-dest crashes\n\
         \t\t# Tor's TransPort handler.\n\
         \t\tip daddr {{ {PRIVATE}, 10.192.0.0/10 }} return\n\
         \t\tudp dport 53 redirect to :{DNS_PORT}\n\
         \t\ttcp dport 53 redirect to :{DNS_PORT}\n\
         \t\ttcp flags & (fin|syn|rst|ack) == syn redirect to :{TRANS_PORT}\n\
         \t}}\n\
         \tchain output {{\n\
         \t\ttype filter hook output priority 0; policy drop;\n\
         \t\toif \"lo\" accept\n\
         \t\tmeta skuid {{ {uids} }} accept\n\
         \t\tct state established,related accept\n\
         \t\tip daddr {{ {PRIVATE} }} accept\n\
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

pub fn up(exempt_uids: &[u32]) -> Result<()> {
    // clear any stale copy first, then apply atomically from stdin.
    down_quiet();
    let mut child = Command::new("nft").args(["-f", "-"]).stdin(Stdio::piped())
        .spawn().context("spawn nft (is nftables installed?)")?;
    child.stdin.take().context("nft stdin")?.write_all(ruleset(exempt_uids).as_bytes())?;
    let st = child.wait().context("wait nft")?;
    anyhow::ensure!(st.success(), "kill-switch ruleset was rejected by nft");
    // CRITICAL, and the reason a kill-switch is more than just a ruleset: flush the conntrack
    // table the instant the rules are live. nftables NAT only transforms the FIRST packet of a
    // flow, so any connection that was ESTABLISHED before this table existed has no Tor redirect
    // recorded — its ongoing packets would skip the redirect and match `ct state established
    // accept`, leaking in the clear to their real peer while we report Active. Flushing forces
    // every existing flow to be re-evaluated against the new rules: a mid-stream packet is not a
    // SYN, so it is neither redirected nor established, and hits policy-drop — the pre-existing
    // clear flow dies (fail-closed) and the app must reopen, whose SYN is pulled into Tor. This
    // is exactly what a bullet-proof kill-switch (Mullvad et al.) does at arm time.
    flush_conntrack();
    Ok(())
}

// Flush the kernel connection-tracking table so no pre-existing flow survives the kill-switch.
// `conntrack -F` (conntrack-tools) is the reliable path; the /proc fallback covers a minimal
// system without the CLI. Absence is a REAL leak risk, so it is reported loudly, not swallowed.
fn flush_conntrack() {
    if Command::new("conntrack").arg("-F").stderr(Stdio::null()).stdout(Stdio::null()).status()
        .map(|s| s.success()).unwrap_or(false) { return; }
    // fallback: some kernels expose a flush via this sysctl-style knob; best effort.
    if std::fs::write("/proc/sys/net/netfilter/nf_conntrack_count", "0").is_ok() { /* not a true flush, ignore */ }
    eprintln!("anond: WARNING — could not flush conntrack (install conntrack-tools). \
               Connections open BEFORE this session may continue in the clear until they close. \
               Close your browser/apps and reopen them after `anond up`.");
}

/// Atomically REPLACE the live ruleset in a single nft transaction, to change the exempt-uid set on
/// an ALREADY-ARMED session (e.g. attaching the i2p overlay to a live session needs i2pd's uid in
/// the exempt set). Because nft applies the whole `-f` input as one kernel transaction, there is
/// never a moment where the default-drop policy is absent — no leak window. Unlike `up()`, it does
/// NOT flush conntrack: on an already-armed session there are no pre-existing clear flows to purge,
/// and the exempted uids egress by uid match regardless of conntrack state, so Tor's live
/// connections survive the swap untouched. Adding a uid only ever WIDENS the exempt set by that
/// uid; it opens nothing else.
pub fn rearm(exempt_uids: &[u32]) -> Result<()> {
    // `add table` before `delete table` makes the delete safe whether or not the table exists
    // (add is idempotent); then the full ruleset recreates it — all in one atomic transaction.
    let atomic = format!(
        "add table inet anond\ndelete table inet anond\n\
         add table ip6 anond6\ndelete table ip6 anond6\n{}",
        ruleset(exempt_uids)
    );
    let mut child = Command::new("nft").args(["-f", "-"]).stdin(Stdio::piped())
        .spawn().context("spawn nft (is nftables installed?)")?;
    child.stdin.take().context("nft stdin")?.write_all(atomic.as_bytes())?;
    let st = child.wait().context("wait nft")?;
    anyhow::ensure!(st.success(), "kill-switch re-arm was rejected by nft");
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
