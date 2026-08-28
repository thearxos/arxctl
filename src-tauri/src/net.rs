// net.rs — the Network panel backend. Real-time throughput per connected interface,
// measured straight from the kernel's /proc/net/dev byte counters. Each call diffs the
// live counters against this session's previous call over the ACTUAL elapsed time, so the
// rate is the true instantaneous rate (accurate to the byte over the real interval), not a
// modelled guess. Read-only — no privilege needed.
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Serialize)]
pub struct Iface {
    pub name: String,
    pub ip: String,
    pub up: bool,
    pub rx_bps: u64,    // bytes/sec down, this instant
    pub tx_bps: u64,    // bytes/sec up, this instant
    pub rx_total: u64,  // bytes received since boot
    pub tx_total: u64,  // bytes sent since boot
    pub link_mbps: i64, // negotiated link speed in Mbit/s, -1 if the kernel won't say
    pub kind: String,   // ethernet | wireless | virtual
}

struct Prev { rx: u64, tx: u64, t: Instant }
fn state() -> &'static Mutex<HashMap<String, Prev>> {
    static M: OnceLock<Mutex<HashMap<String, Prev>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn rd(p: &str) -> String { std::fs::read_to_string(p).unwrap_or_default().trim().to_string() }

// iface -> first IPv4, from `ip -o -4 addr show`
fn ipv4_map() -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(o) = std::process::Command::new("ip").args(["-o", "-4", "addr", "show"]).output() {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if let Some(pos) = f.iter().position(|x| *x == "inet") {
                if let (Some(ifn), Some(cidr)) = (f.get(1), f.get(pos + 1)) {
                    m.entry(ifn.to_string()).or_insert_with(|| cidr.split('/').next().unwrap_or("").to_string());
                }
            }
        }
    }
    m
}

fn kind_of(name: &str) -> String {
    if std::path::Path::new(&format!("/sys/class/net/{name}/wireless")).exists() { return "wireless".into(); }
    if std::path::Path::new(&format!("/sys/class/net/{name}/device")).exists() { return "ethernet".into(); }
    "virtual".into()
}

#[tauri::command]
pub fn net_status() -> Vec<Iface> {
    let dev = rd("/proc/net/dev");
    let ips = ipv4_map();
    let now = Instant::now();
    let mut prev = state().lock().unwrap();
    let mut out = Vec::new();
    for line in dev.lines() {
        let line = line.trim();
        let (name, rest) = match line.split_once(':') { Some(x) => x, None => continue };
        let name = name.trim().to_string();
        if name.is_empty() || name == "lo" { continue; } // skip loopback
        let f: Vec<u64> = rest.split_whitespace().map(|v| v.parse().unwrap_or(0)).collect();
        if f.len() < 9 { continue; }
        let (rx, tx) = (f[0], f[8]); // rx_bytes is field 0, tx_bytes is field 8
        // instantaneous rate against the previous sample of THIS interface
        let (mut rx_bps, mut tx_bps) = (0u64, 0u64);
        if let Some(p) = prev.get(&name) {
            let dt = now.duration_since(p.t).as_secs_f64();
            if dt > 0.05 {
                rx_bps = ((rx.saturating_sub(p.rx)) as f64 / dt) as u64;
                tx_bps = ((tx.saturating_sub(p.tx)) as f64 / dt) as u64;
            }
        }
        prev.insert(name.clone(), Prev { rx, tx, t: now });
        let up = rd(&format!("/sys/class/net/{name}/operstate")) == "up" || rd(&format!("/sys/class/net/{name}/carrier")) == "1";
        let link_mbps = rd(&format!("/sys/class/net/{name}/speed")).parse::<i64>().unwrap_or(-1);
        out.push(Iface {
            ip: ips.get(&name).cloned().unwrap_or_default(),
            kind: kind_of(&name),
            name, up, rx_bps, tx_bps, rx_total: rx, tx_total: tx, link_mbps,
        });
    }
    // connected interfaces first, then alphabetical
    out.sort_by(|a, b| b.up.cmp(&a.up).then_with(|| a.name.cmp(&b.name)));
    out
}

// ---------- listening ports + services (network hardening) ----------

#[derive(Serialize)]
pub struct PortRow {
    pub proto: String,   // tcp | udp
    pub addr: String,    // bind address
    pub port: u32,
    pub service: String, // friendly label (process name, else well-known port name)
    pub process: String, // owning process name, empty if not visible to us
    pub pid: u32,        // 0 if unknown
    pub unit: String,    // owning systemd .service unit, best-effort ("" = none)
    pub exposed: bool,   // bound off-loopback = reachable from the network
}

// "sshd" + 812 out of a `users:(("sshd",pid=812,fd=3))` process field.
fn parse_proc(s: &str) -> (String, u32) {
    let name = s.split('"').nth(1).unwrap_or("").to_string();
    let pid = s.split("pid=").nth(1)
        .and_then(|t| t.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|d| d.parse().ok()).unwrap_or(0);
    (name, pid)
}

// a pid's owning systemd unit, from its cgroup (world-readable), e.g. "sshd.service".
fn unit_of(pid: u32) -> String {
    let cg = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap_or_default();
    for part in cg.replace('\n', "/").split('/') {
        if part.ends_with(".service") { return part.to_string(); }
    }
    String::new()
}

fn service_label(port: u32, process: &str) -> String {
    if !process.is_empty() { return process.to_string(); }
    match port {
        22 => "ssh", 80 => "http", 443 => "https", 53 => "dns", 21 => "ftp", 23 => "telnet",
        25 => "smtp", 110 => "pop3", 143 => "imap", 3306 => "mysql", 5432 => "postgres",
        6379 => "redis", 27017 => "mongodb", 631 => "cups", 139 | 445 => "smb", 111 => "rpcbind",
        3389 => "rdp", 5900 => "vnc", 8080 => "http-alt", 9050 => "tor", _ => "",
    }.to_string()
}

// Listening sockets (tcp + udp), each mapped to its process/unit where the kernel lets us
// see it. Read-only, unprivileged: bind address, port and proto always resolve; the owning
// process shows for our own sockets, and well-known ports get a friendly label regardless.
#[tauri::command]
pub fn net_ports() -> Vec<PortRow> {
    let out = match std::process::Command::new("ss").args(["-tulnpH"]).output() { Ok(o) => o, Err(_) => return vec![] };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut rows: Vec<PortRow> = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 5 { continue; }
        let proto = f[0].to_string();
        if proto != "tcp" && proto != "udp" { continue; }
        let local = f[4]; // Netid State Recv-Q Send-Q Local:Port Peer Process
        let (addr, port_s) = match local.rsplit_once(':') { Some(x) => x, None => continue };
        let port: u32 = match port_s.parse() { Ok(p) => p, Err(_) => continue };
        let addr = addr.trim_matches(|c| c == '[' || c == ']').to_string();
        let (process, pid) = parse_proc(line.split("users:").nth(1).unwrap_or(""));
        let unit = if pid > 0 { unit_of(pid) } else { String::new() };
        let exposed = !(addr.starts_with("127.") || addr == "::1" || addr.starts_with("::ffff:127"));
        rows.push(PortRow { service: service_label(port, &process), proto, addr, port, process, pid, unit, exposed });
    }
    // exposed-to-the-network first, then by port; collapse the v4/v6 duplicate of one service.
    rows.sort_by(|a, b| b.exposed.cmp(&a.exposed).then(a.port.cmp(&b.port)).then(a.proto.cmp(&b.proto)));
    rows.dedup_by(|a, b| a.port == b.port && a.proto == b.proto && a.process == b.process);
    rows
}

fn safe_unit(u: &str) -> bool {
    !u.is_empty() && u.len() <= 128 && u.ends_with(".service")
        && u.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'@')
}

// Disable the owning service the clean, reversible way: stop it now AND stop it starting at
// boot. Privileged, so it goes through pkexec (the GUI never holds root).
#[tauri::command]
pub fn net_disable_service(unit: String) -> Result<(), String> {
    if !safe_unit(&unit) { return Err("invalid unit".into()); }
    let ok = std::process::Command::new("pkexec").args(["systemctl", "disable", "--now", &unit])
        .status().map(|s| s.success()).unwrap_or(false);
    if ok { Ok(()) } else { Err(format!("could not disable {unit}")) }
}

// Block a port at the firewall without touching any existing ruleset: everything lands in
// our own isolated `inet arxos_harden` table, so it is trivially reversible and never
// collides with the user's firewall. The service keeps listening; the port just goes dark.
#[tauri::command]
pub fn net_block_port(proto: String, port: u32) -> Result<(), String> {
    if (proto != "tcp" && proto != "udp") || port == 0 || port > 65535 { return Err("invalid port".into()); }
    let script = format!(
        "nft add table inet arxos_harden 2>/dev/null; \
         nft 'add chain inet arxos_harden input {{ type filter hook input priority 0 ; policy accept ; }}' 2>/dev/null; \
         nft add rule inet arxos_harden input {proto} dport {port} drop"
    );
    let ok = std::process::Command::new("pkexec").args(["bash", "-c", &script])
        .status().map(|s| s.success()).unwrap_or(false);
    if ok { Ok(()) } else { Err(format!("could not block {proto}/{port}")) }
}
