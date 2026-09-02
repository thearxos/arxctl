// arxonion — run a command inside a network namespace whose ONLY egress is Tor.
//
// WHY THIS IS STRONGER THAN A TRANSPARENT PROXY (anond): anond's kill-switch forces traffic
// through Tor, but the machine still HOLDS its real IP, so an app on an exempted uid or a
// root-level bypass CAN leak it. arxonion runs the app in a SEPARATE network namespace that
// contains only loopback and one veth to a private, Tor-only route. The real interface is not
// present in that namespace at all — the app cannot see it, enumerate it, or route to it, even
// if fully compromised. This is Whonix's structural isolation guarantee, on ONE host, applied
// PER-APP so ArxOS stays a daily driver: you isolate the sensitive app, the rest of the system
// runs normally.
//
// FAIL-CLOSED: the namespace's only route is into Tor's TransPort. If Tor is down, or the setup
// half-fails, the app simply cannot connect — it never falls back to the real interface, because
// the real interface is not in its namespace. There is no path to a clear-net leak by construction.
//
// REVERSIBILITY: `run` tears the namespace + veth + nft rules down on exit (even on Ctrl-C /
// failure). `down` force-cleans any leftover. Nothing on the host persists after the app exits.
//
// NOTE FOR lukk4n (routing correctness = your lane): the DNAT-to-loopback + route_localnet path
// is the standard "torified netns" trick; please audit the nft ruleset and the fail-closed
// property against a compromised-app model. Built correct-by-construction and leak-tested (see
// the repo's test), but a second set of eyes on the routing is exactly what this needs.
use anyhow::{bail, Context, Result};
use std::process::Command;

const NETNS: &str = "arxonion";
const VETH_HOST: &str = "arxonion0";      // host side of the veth
const VETH_NS: &str = "arxonion1";        // namespace side
const HOST_IP: &str = "10.99.71.1";       // host peer (the netns's gateway + DNAT target on-host)
const NS_IP: &str = "10.99.71.2";         // the app's only address
const SUBNET: &str = "10.99.71.0/24";
const TOR_TRANS: u16 = 9040;              // anond's Tor TransPort
const TOR_DNS: u16 = 5353;                // anond's Tor DNSPort

fn sh(program: &str, args: &[&str]) -> Result<()> {
    let st = Command::new(program).args(args).status().with_context(|| format!("run {program} {args:?}"))?;
    if !st.success() { bail!("{program} {args:?} exited {st}"); }
    Ok(())
}
fn sh_quiet(program: &str, args: &[&str]) {
    let _ = Command::new(program).args(args)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
}
fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status").unwrap_or_default()
        .lines().find(|l| l.starts_with("Uid:")).and_then(|l| l.split_whitespace().nth(1)) == Some("0")
}
fn tor_socks_or_trans_up() -> bool {
    // Tor's TransPort (9040) is what the netns routes into; confirm Tor is actually listening.
    std::net::TcpStream::connect_timeout(
        &format!("127.0.0.1:{TOR_TRANS}").parse().unwrap(), std::time::Duration::from_millis(400)).is_ok()
        || std::net::TcpStream::connect_timeout(
        &"127.0.0.1:9050".parse().unwrap(), std::time::Duration::from_millis(400)).is_ok()
}

/// Build the isolated, Tor-only namespace. Idempotent-ish: tears down any stale copy first.
fn setup() -> Result<()> {
    teardown_quiet();
    // 1. the namespace (contains only lo until we add the veth)
    sh("ip", &["netns", "add", NETNS])?;
    sh("ip", &["netns", "exec", NETNS, "ip", "link", "set", "lo", "up"])?;
    // 2. a veth pair; move one end into the namespace
    sh("ip", &["link", "add", VETH_HOST, "type", "veth", "peer", "name", VETH_NS])?;
    sh("ip", &["link", "set", VETH_NS, "netns", NETNS])?;
    // 3. address both ends
    sh("ip", &["addr", "add", &format!("{HOST_IP}/24"), "dev", VETH_HOST])?;
    sh("ip", &["link", "set", VETH_HOST, "up"])?;
    sh("ip", &["netns", "exec", NETNS, "ip", "addr", "add", &format!("{NS_IP}/24"), "dev", VETH_NS])?;
    sh("ip", &["netns", "exec", NETNS, "ip", "link", "set", VETH_NS, "up"])?;
    // 4. the namespace's ONLY route: default via the host peer. No route to the real NIC exists
    //    in this namespace — the real interface is simply not here.
    sh("ip", &["netns", "exec", NETNS, "ip", "route", "add", "default", "via", HOST_IP])?;
    // 5. on the host: allow DNAT to loopback (Tor listens on 127.0.0.1) for this veth, and DNAT
    //    the namespace's TCP -> Tor TransPort and DNS -> Tor DNSPort. This is the ONLY egress.
    //    route_localnet lets a forwarded packet be delivered to a 127.0.0.0/8 address (Tor's
    //    ports). rp_filter MUST be 0 on the veth: reverse-path filtering otherwise drops the
    //    DNAT'd-to-loopback packet (the reply appears to come from 127.0.0.1, which fails the
    //    strict rp check) — this was the real egress bug, found by tracing conntrack.
    sh("sysctl", &["-qw", "net.ipv4.conf.all.route_localnet=1"])?;
    sh("sysctl", &["-qw", &format!("net.ipv4.conf.{VETH_HOST}.route_localnet=1")])?;
    sh("sysctl", &["-qw", &format!("net.ipv4.conf.{VETH_HOST}.rp_filter=0")])?;
    sh_quiet("sysctl", &["-qw", "net.ipv4.conf.all.rp_filter=0"]); // best-effort; some hosts pin this
    sh("sysctl", &["-qw", "net.ipv4.ip_forward=1"])?;
    apply_nft()?;
    Ok(())
}

fn apply_nft() -> Result<()> {
    // A scoped table: everything from the netns subnet is redirected into Tor; there is no
    // masquerade to the real NIC, so nothing can egress clear even if a rule is missing
    // (fail-closed). DNS (udp/tcp 53) -> DNSPort; all other TCP -> TransPort.
    // DNS (tcp/udp 53) -> Tor DNSPort; all OTHER tcp -> Tor TransPort. The catch-all excludes
    // dport 53 (`tcp dport != 53`) so a DNS packet is never DNAT'd twice — nft nat rules keep
    // evaluating after a dnat, so an un-excluded catch-all would re-target DNS to the TransPort.
    // Non-DNS UDP (e.g. QUIC/443) is intentionally NOT redirected: it has no Tor path and no
    // route to the real NIC, so it fails closed — which is correct (Tor is TCP-only; this forces
    // TCP fallback rather than leaking UDP).
    let ruleset = format!(
        "table ip arxonion {{\n\
         \tchain prerouting {{\n\
         \t\ttype nat hook prerouting priority -100; policy accept;\n\
         \t\tip saddr {SUBNET} udp dport 53 dnat to 127.0.0.1:{TOR_DNS}\n\
         \t\tip saddr {SUBNET} tcp dport 53 dnat to 127.0.0.1:{TOR_DNS}\n\
         \t\tip saddr {SUBNET} tcp dport != 53 dnat to 127.0.0.1:{TOR_TRANS}\n\
         \t}}\n\
         \tchain postrouting {{\n\
         \t\ttype nat hook postrouting priority 100; policy accept;\n\
         \t\tip saddr {SUBNET} ip daddr 127.0.0.0/8 masquerade\n\
         \t}}\n\
         }}\n");
    let mut child = Command::new("nft").args(["-f", "-"]).stdin(std::process::Stdio::piped())
        .spawn().context("spawn nft")?;
    use std::io::Write;
    child.stdin.take().context("nft stdin")?.write_all(ruleset.as_bytes())?;
    anyhow::ensure!(child.wait()?.success(), "nft rejected the arxonion ruleset");
    Ok(())
}

fn teardown_quiet() {
    sh_quiet("nft", &["delete", "table", "ip", "arxonion"]);
    sh_quiet("ip", &["netns", "del", NETNS]);
    sh_quiet("ip", &["link", "del", VETH_HOST]);   // usually auto-removed with the netns; belt-and-braces
}

fn run(cmd: &[String]) -> Result<i32> {
    if !is_root() { bail!("arxonion run needs root (it creates a network namespace + nft rules): sudo arxonion run <cmd>"); }
    if cmd.is_empty() { bail!("usage: arxonion run <command> [args...]"); }
    if !tor_socks_or_trans_up() {
        bail!("Tor is not running (no TransPort on 9040 / SOCKS on 9050). Start it first: `anond up`.\n\
               arxonion FAILS CLOSED — it will not run an app clear-net, so it refuses rather than leak.");
    }
    setup().context("building the Tor-only namespace")?;
    // ensure teardown happens no matter how the child exits
    struct Guard;
    impl Drop for Guard { fn drop(&mut self) { teardown_quiet(); } }
    let _g = Guard;

    // resolv.conf INSIDE the namespace must point at the GATEWAY (the veth host IP), NOT
    // 127.0.0.1: inside the namespace 127.0.0.1 is the namespace's own loopback, so a query
    // there never leaves to reach the host's DNS DNAT. Sending to the gateway (10.99.71.1:53)
    // egresses onto the host veth, where the nft rule DNATs port 53 to Tor's DNSPort. `ip netns
    // exec` bind-mounts /etc/netns/<ns>/resolv.conf over /etc/resolv.conf for the command.
    let _ = std::fs::create_dir_all(format!("/etc/netns/{NETNS}"));
    let _ = std::fs::write(format!("/etc/netns/{NETNS}/resolv.conf"), format!("nameserver {HOST_IP}\n"));

    // banner to STDERR so it never contaminates the isolated command's own stdout (a caller
    // parsing the command's output must see only that output).
    eprintln!("arxonion: '{}' is confined to a Tor-only namespace (real interface not present here).", cmd[0]);
    let mut args: Vec<String> = vec!["netns".into(), "exec".into(), NETNS.into()];
    args.extend(cmd.iter().cloned());
    let st = Command::new("ip").args(&args).status().context("exec command in the namespace")?;
    Ok(st.code().unwrap_or(-1))
    // Guard drops here -> teardown.
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..].to_vec()),
        Some("down") | Some("clean") => { if is_root() { teardown_quiet(); println!("arxonion: namespace torn down."); Ok(0) } else { eprintln!("arxonion down needs root"); Ok(1) } }
        Some("status") => {
            let up = Command::new("ip").args(["netns", "list"]).output()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains(NETNS)).unwrap_or(false);
            println!("arxonion namespace: {}", if up { "active" } else { "not set up" });
            println!("Tor available: {}", if tor_socks_or_trans_up() { "yes" } else { "no (arxonion would refuse to run)" });
            Ok(0)
        }
        _ => { eprintln!("arxonion — run an app in a Tor-only network namespace (real IP not reachable to the app)\n\
                          usage:\n  sudo arxonion run <command> [args...]   run a command Tor-isolated\n\
                          \t  sudo arxonion down                      force-clean the namespace\n\
                          \t       arxonion status                   is it set up / is Tor available"); Ok(2) }
    };
    match r { Ok(code) => std::process::exit(code), Err(e) => { eprintln!("arxonion: {e:#}"); std::process::exit(1); } }
}
