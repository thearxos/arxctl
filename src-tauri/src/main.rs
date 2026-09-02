// ArxOS Control Center — native Rust (Tauri v2) backend. The command deck for the
// system: live stats, updates, kernels, the weapons arsenal (installed LIVE with
// per-step progress), performance, privacy (anonkit), services, and info. Every
// handler talks to the real ArxOS tools (arx, arxos-kernel) or /proc; nothing is
// mocked. Privileged actions go through pkexec so the GUI never holds root.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;

mod perf;
mod net;
mod wallpaper;
mod vm_tools;

// ---------- small helpers ----------

fn read(path: &str) -> String { std::fs::read_to_string(path).unwrap_or_default() }

fn run(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd).args(args).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

// Is Tor's SOCKS port listening? (127.0.0.1:9050 — anond's Tor, or a standalone tor).
fn tor_socks_up() -> bool {
    std::net::TcpStream::connect_timeout(
        &"127.0.0.1:9050".parse().unwrap(),
        std::time::Duration::from_millis(300),
    ).is_ok()
}

// curl for an ArxOS-identifying fetch (the kernel manifest, arsenal index): route it through
// Tor's SOCKS proxy whenever Tor is available, so GitHub/the arsenal host never ties the
// ArxOS-specific fetch to the user's real IP (the "this box runs ArxOS" beacon). Falls back to
// a direct fetch only when Tor is not running (the user is not anonymised then anyway).
fn run_private_curl(args: &[&str]) -> String {
    let mut full: Vec<String> = Vec::new();
    if tor_socks_up() { full.push("--socks5-hostname".into()); full.push("127.0.0.1:9050".into()); }
    for a in args { full.push((*a).to_string()); }
    let refs: Vec<&str> = full.iter().map(String::as_str).collect();
    run("curl", &refs)
}

// ---------- read-only system state ----------

#[derive(Serialize)]
struct SysInfo { host: String, distro: String, kernel: String, uptime: String, cpu: String, mem_used: u64, mem_total: u64, mem_type: String, load: String }

#[tauri::command]
fn system_info() -> SysInfo {
    let host = read("/etc/hostname").trim().to_string();
    let distro = read("/etc/os-release").lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim_matches('"').to_string()).unwrap_or_else(|| "ArxOS".into());
    let kernel = run("uname", &["-r"]).trim().to_string();
    // uptime (seconds -> h m)
    let up = read("/proc/uptime").split_whitespace().next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) as u64;
    let uptime = format!("{}h {}m", up / 3600, (up % 3600) / 60);
    let cpu = read("/proc/cpuinfo").lines().find_map(|l| l.strip_prefix("model name")).map(|v| v.trim_start_matches(|c| c == ':' || c == ' ').to_string()).unwrap_or_default();
    // memory (kB)
    let mi = read("/proc/meminfo");
    let g = |k: &str| mi.lines().find_map(|l| l.strip_prefix(k)).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let (total, avail) = (g("MemTotal:"), g("MemAvailable:"));
    // memory TYPE + speed from the boot-cached hwinfo (dmidecode needs root; the GUI does not).
    let hw = read("/run/arxos/hwinfo");
    let hg = |k: &str| hw.lines().find_map(|l| l.strip_prefix(k)).map(|v| v.trim().to_string()).unwrap_or_default();
    let (mt, ms) = (hg("MEMTYPE="), hg("MEMSPEED="));
    let mem_type = if !mt.is_empty() && !ms.is_empty() { format!("{mt} · {ms}") } else { mt };
    let load = read("/proc/loadavg").split_whitespace().take(3).collect::<Vec<_>>().join(" ");
    SysInfo { host, distro, kernel, uptime, cpu, mem_used: total.saturating_sub(avail), mem_total: total, mem_type, load }
}

#[tauri::command]
fn updates_count() -> usize {
    // prefer the count arxos-notify already computed on its last ping (instant, and it's
    // what the desktop notification was based on); fall back to a fresh local check.
    let cache = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| format!("{}/.cache", std::env::var("HOME").unwrap_or_default()));
    if let Ok(n) = read(&format!("{cache}/arxos/update-count")).trim().parse::<usize>() { return n; }
    run("arx", &["outdated"]).lines().filter(|l| !l.trim().is_empty()).count()
}

// The real per-source breakdown (official repos / AUR / ArxOS tool repos), computed
// live by `arx updates-json` — this is what the Update panel shows, so the number
// there is always current, never a stale notification cache.
#[tauri::command]
fn updates_breakdown() -> serde_json::Value {
    let out = run("arx", &["updates-json"]);
    serde_json::from_str(&out).unwrap_or(serde_json::json!({"pacman":0,"aur":0,"tools":0,"total":0}))
}

#[derive(Serialize)]
struct Kernel { flavor: String, version: String, status: String, role: String, running: bool }

#[tauri::command]
fn kernels_list() -> Vec<Kernel> {
    // `arx kernels list` prints: flavor  version  status  role[ *running]
    run("arx", &["kernels", "list"]).lines().filter_map(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        if t.len() < 3 || !t[0].starts_with("linux-arxos") { return None; }
        let running = l.contains("*running");
        let role = t[3..].join(" ").replace("*running", "").trim().to_string();
        Some(Kernel { flavor: t[0].into(), version: t[1].into(), status: t[2].into(), role, running })
    }).collect()
}

// The public ArxOS kernel history (arxos-kernels/kernels.json on GitHub): flavors, the
// tuning features every ArxOS kernel carries, and the full per-version changelog. This is
// what makes the Kernels panel a real loader (ukui-style) rather than just a list.
#[derive(Serialize)] struct KFlavor { name: String, role: String, base: String, current: String }
#[derive(Serialize)] struct KTune { name: String, advantage: String }
#[derive(Serialize)] struct KHistory { flavor: String, version: String, upstream: String, date: String, status: String, changes: String }
#[derive(Serialize)] struct KManifest { updated: String, flavors: Vec<KFlavor>, tunes: Vec<KTune>, history: Vec<KHistory> }

#[tauri::command]
fn kernels_manifest() -> KManifest {
    // Tor-routed when Tor is up: this fetch is ArxOS-identifying (the arxos-kernels manifest),
    // so it must not tie the user's real IP to "runs ArxOS" at the GitHub side.
    let raw = run_private_curl(&["-fsSL", "--max-time", "30",
        "https://raw.githubusercontent.com/thearxos/arxos-kernels/main/kernels.json"]);
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    let s = |x: &serde_json::Value, k: &str| x.get(k).and_then(|y| y.as_str()).unwrap_or("").to_string();
    let flavors = v.get("flavors").and_then(|x| x.as_object()).map(|o| o.iter().map(|(name, f)| KFlavor {
        name: name.clone(), role: s(f, "role"), base: s(f, "base"), current: s(f, "current"),
    }).collect()).unwrap_or_default();
    let tunes = v.get("tunes").and_then(|x| x.as_array()).map(|a| a.iter()
        .map(|t| KTune { name: s(t, "name"), advantage: s(t, "advantage") }).collect()).unwrap_or_default();
    let history = v.get("kernels").and_then(|x| x.as_array()).map(|a| a.iter().map(|k| KHistory {
        flavor: s(k, "flavor"), version: s(k, "version"), upstream: s(k, "upstream"),
        date: s(k, "date"), status: s(k, "status"), changes: s(k, "changes"),
    }).collect()).unwrap_or_default();
    KManifest { updated: s(&v, "updated"), flavors, tunes, history }
}

#[derive(Serialize)]
struct Category { name: String, count: usize }

#[tauri::command]
fn weapons_categories() -> Vec<Category> {
    // parse tools.db (name|MENU|SUB|cat|desc), grouped by cat (fallback SUB)
    let db = std::env::var("ARXOS_TOOLS").unwrap_or_else(|_| "/usr/share/arxos/tools.db".into());
    let mut map: std::collections::BTreeMap<String, usize> = Default::default();
    for l in read(&db).lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') || !l.contains('|') { continue; }
        let f: Vec<&str> = l.split('|').collect();
        if f.len() < 4 { continue; }
        let cat = { let c = f[3].trim(); if c.is_empty() { f[2].trim() } else { c } }.to_ascii_lowercase();
        *map.entry(cat).or_insert(0) += 1;
    }
    map.into_iter().map(|(name, count)| Category { name, count }).collect()
}

#[derive(Serialize)]
struct ArsenalTotals { curated: usize, total: usize, other: usize }

fn parse_totals(out: &str) -> ArsenalTotals {
    let f: Vec<usize> = out.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    ArsenalTotals { curated: f.first().copied().unwrap_or(0), total: f.get(1).copied().unwrap_or(0), other: f.get(2).copied().unwrap_or(0) }
}

#[tauri::command]
fn arsenal_totals() -> ArsenalTotals {
    // `arx weapons totals` pings the live arsenal index and prints: curated<TAB>total<TAB>other
    // (all zero if offline, so the UI just falls back to the curated categories). Cached
    // up to 6h by arx-core so a repeat call is instant.
    parse_totals(&run("arx", &["weapons", "totals"]))
}

#[tauri::command]
fn arsenal_totals_refresh() -> ArsenalTotals {
    // bypasses the 6h cache — a tool added to the live repo just now shouldn't need a
    // wait to show up when the user explicitly asks for a refresh.
    parse_totals(&run("arx", &["weapons", "totals", "--force"]))
}

// Rebuild the XFCE Weapons menu from tools.db so the desktop menu tracks the same
// arsenal the Control Center shows. Needs root to write the system menu, so it hands
// off to a terminal like every other privileged action here.
#[tauri::command]
fn weapons_menu_rebuild() -> Result<(), String> {
    spawn_terminal(&wrap_close("sudo arx weapons menu"))
}

#[derive(Serialize)]
struct Service { name: String, active: bool }

#[tauri::command]
fn services_status() -> Vec<Service> {
    ["NetworkManager", "tor-anonkit", "lightdm", "arxupd.timer", "sshd"].iter().map(|n| {
        let active = run("systemctl", &["is-active", n]).trim() == "active";
        Service { name: (*n).into(), active }
    }).collect()
}

// ---------- actions: hand off to a real terminal ----------
// We do NOT reimplement arx's progress UI in the GUI. arx already renders installs
// beautifully in a terminal (its loader, per-step lines, and precise result), and it
// handles its own privilege escalation. The deck just launches the OS terminal running
// the arx command, held open so the user watches it to completion and sees the result.

fn safe_token(s: &str) -> bool { !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b' ') }
fn have(bin: &str) -> bool { std::env::var("PATH").unwrap_or_default().split(':').any(|d| std::path::Path::new(d).join(bin).exists()) }

// Wrap a shell command so the terminal TRULY exits when it finishes: on success it pauses
// briefly (so the result is readable) then the window closes on its own; on failure it waits
// for the user so the error can be read. We deliberately do NOT use the terminal's --hold
// (that leaves a dead window open forever — the "hang" users hit).
fn wrap_close(cmd: &str) -> String {
    format!("{cmd}; __rc=$?; echo; if [ $__rc -eq 0 ]; then echo '  ✔ done — closing…'; sleep 3; \
             else echo '  ✖ finished with errors'; read -r -t 120 -p '  press Enter to close… ' _; fi")
}

// open the OS terminal running a bash command, NOT held open (wrap_close handles the exit).
fn spawn_terminal(bash_cmd: &str) -> Result<(), String> {
    let mut cmd = if have("konsole") { let mut c = std::process::Command::new("konsole"); c.args(["-e", "bash", "-c", bash_cmd]); c }
        else if have("xterm") { let mut c = std::process::Command::new("xterm"); c.args(["-e", "bash", "-c", bash_cmd]); c }
        else if have("x-terminal-emulator") { let mut c = std::process::Command::new("x-terminal-emulator"); c.args(["-e", "bash", "-c", bash_cmd]); c }
        else { return Err("no terminal emulator found".into()); };
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

// run `arx <args>` in the OS terminal; it closes itself on success. Args are safe_token-validated.
fn launch_arx(arx_args: &[&str]) -> Result<(), String> {
    spawn_terminal(&wrap_close(&format!("arx {}", arx_args.join(" "))))
}

#[tauri::command]
fn weapons_install(category: String) -> Result<(), String> {
    if !safe_token(&category) { return Err("invalid category".into()); }
    let mut a = vec!["weapons", "install"]; a.extend(category.split_whitespace()); launch_arx(&a)
}
#[tauri::command]
fn weapons_remove(category: String) -> Result<(), String> {
    if !safe_token(&category) { return Err("invalid category".into()); }
    let mut a = vec!["weapons", "remove"]; a.extend(category.split_whitespace()); launch_arx(&a)
}
#[tauri::command]
fn weapons_browse() -> Result<(), String> { launch_arx(&["weapons", "list-all"]) }

// ---------- privacy: anond (the anonymity daemon) ----------
// Status is read from anond's world-readable snapshot (no root needed to show state). The
// privileged actions (up/down/verify/new-identity) hand off to a terminal running `sudo anond`
// so the user watches the fail-closed bring-up and authenticates there. We never hold root.
#[derive(Serialize)]
struct AnondStatus {
    state: String,
    exit_ip: String,
    // live Tor bootstrap percentage (0-100) while state is Bootstrapping, so the panel shows
    // real progress instead of looking frozen during the multi-minute bring-up.
    bootstrap_pct: u8,
    // each layer, read from anond's world-readable snapshot (no root, no terminal needed)
    tor: String,
    killswitch: String,
    dns: String,
    i2p: String,
    // local identity, so the panel can show what the network actually sees
    mac: String,
    iface: String,
    resolver: String,
}

// The MAC and interface anond's spoofing acts on: the one carrying the default route.
fn primary_iface() -> (String, String) {
    let route = run("ip", &["-o", "route", "get", "1.1.1.1"]);
    let iface = route.split_whitespace().skip_while(|t| *t != "dev").nth(1).unwrap_or("").to_string();
    if iface.is_empty() { return (String::new(), String::new()); }
    let mac = read(&format!("/sys/class/net/{iface}/address")).trim().to_string();
    (iface, mac)
}

#[tauri::command]
fn anond_status() -> AnondStatus {
    let v: serde_json::Value = serde_json::from_str(&read("/run/anond/pub.json")).unwrap_or(serde_json::Value::Null);
    let s = |k: &str, d: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or(d).to_string();
    let (iface, mac) = primary_iface();
    // the first nameserver actually in effect (anond pins this to Tor when it is up)
    let resolver = read("/etc/resolv.conf").lines()
        .find_map(|l| l.strip_prefix("nameserver ").map(|v| v.trim().to_string()))
        .unwrap_or_default();
    let pct = v.get("bootstrap_pct").and_then(|x| x.as_u64()).unwrap_or(0).min(100) as u8;
    AnondStatus {
        state: s("state", "Down"), exit_ip: s("exit_ip", ""), bootstrap_pct: pct,
        tor: s("tor", "stopped"), killswitch: s("killswitch", "down"),
        dns: s("dns", "open"), i2p: s("i2p", "off"),
        mac, iface, resolver,
    }
}

// Where the exit node actually is. Only meaningful while anond is up, and the lookup itself
// rides the same Tor-routed path as everything else, so it does not deanonymise the request.
#[tauri::command]
fn anond_exit_location(ip: String) -> String {
    if ip.is_empty() || !ip.bytes().all(|b| b.is_ascii_hexdigit() || b == b'.' || b == b':') { return String::new(); }
    let out = run("curl", &["-s", "--max-time", "8", &format!("https://ipinfo.io/{ip}/json")]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap_or(serde_json::Value::Null);
    let g = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let (city, region, country, org) = (g("city"), g("region"), g("country"), g("org"));
    let place: Vec<String> = [city, region, country].into_iter().filter(|s| !s.is_empty()).collect();
    if place.is_empty() && org.is_empty() { return String::new(); }
    if org.is_empty() { place.join(", ") } else { format!("{} · {}", place.join(", "), org) }
}

// run `sudo <bin> <args>` in the OS terminal; sudo authenticates there and the window closes
// itself when the command finishes (wrap_close). Args are safe_token-validated.
fn launch_priv(bin: &str, args: &[&str]) -> Result<(), String> {
    spawn_terminal(&wrap_close(&format!("sudo {bin} {}", args.join(" "))))
}

#[tauri::command]
fn anond_action(action: String) -> Result<(), String> {
    match action.as_str() {
        "up" | "down" | "verify" | "new-identity" => launch_priv("anond", &[&action]),
        "up-i2p" => launch_priv("anond", &["up", "--i2p"]), // Tor + the i2p overlay
        _ => Err("invalid action".into()),
    }
}

// ---------- arxonion: per-app Tor-only isolation (the strong, structural layer over anond) ----
#[derive(Serialize)]
struct OnionStatus { up: bool, tor_available: bool }

// Is the arxonion namespace up, and is Tor available for it? (read-only, no root needed)
#[tauri::command]
fn arxonion_status() -> OnionStatus {
    let up = std::process::Command::new("ip").args(["netns", "list"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("arxonion")).unwrap_or(false);
    // Probe the SOCKS port (9050) ONLY, never the TransPort (9040): a plain TCP connect to the
    // TransPort makes Tor 0.4.9.11 getsockopt(SO_ORIGINAL_DST)=127.0.0.1 and SIGSEGV. Our own
    // status probe to 9040 was crashing Tor. SOCKS-up implies the same process's TransPort is up.
    let tor_available = std::net::TcpStream::connect_timeout(
        &"127.0.0.1:9050".parse().unwrap(), std::time::Duration::from_millis(300)).is_ok();
    OnionStatus { up, tor_available }
}

// Flip isolation on/off. ON keeps the Tor-only namespace up (persistent) so every app launched
// isolated shares it; OFF tears it down. Privileged, via pkexec (no terminal to auth in).
#[tauri::command]
fn arxonion_toggle(on: bool) -> Result<(), String> {
    launch_priv("arxonion", &[if on { "up" } else { "down" }])
}

// Open an isolated terminal — an arxonion shell where EVERY command routes through Tor and the
// real interface is unreachable. This is the "all commands after this are isolated" surface.
#[tauri::command]
fn arxonion_shell() -> Result<(), String> {
    spawn_terminal("pkexec arxonion shell")
}

// Launch one detected browser inside the isolation namespace (Tor-only, real IP unreachable).
#[tauri::command]
fn arxonion_launch_browser(browser: String) -> Result<(), String> {
    // map the display name back to its real launcher binary; reject anything not in the set so
    // an arbitrary command can never be injected into the privileged launch.
    let bin = match browser.as_str() {
        "Firefox" => "firefox", "Waterfox" => "waterfox", "Brave" => "brave",
        "LibreWolf" => "librewolf", "Mullvad Browser" => "mullvad-browser",
        "Chromium" => "chromium", "Tor Browser" => "torbrowser-launcher",
        _ => return Err("unknown browser".into()),
    };
    spawn_terminal(&format!("pkexec arxonion run {bin}"))
}

// Run an ARBITRARY app/command inside the Tor-only namespace (the general form of the per-browser
// launch above). arxonion drops to the invoking user and confines the app to Tor. Uses a terminal
// + `sudo --preserve-env=DISPLAY,XAUTHORITY` (not pkexec) so a GUI app's X display actually reaches
// it; the terminal also surfaces any startup error to the user.
#[tauri::command]
fn arxonion_run_app(app: String) -> Result<(), String> {
    let app = app.trim().to_string();
    if app.is_empty() { return Err("type an app or command to run in isolation".into()); }
    if app.len() > 200 { return Err("command too long".into()); }
    // The command is interpolated into a `bash -c` string, so reject every shell metacharacter.
    // Allow only what appears in real commands, flags, paths and URLs: letters, digits, space,
    // and . _ - / : = @ + — nothing a shell would interpret (no ; | & $ ` < > ( ) ~ * ? quotes).
    if !app.chars().all(|c| c.is_ascii_alphanumeric() || " ._:/=@+-".contains(c)) {
        return Err("only letters, digits, spaces and . _ - / : = @ + are allowed".into());
    }
    // Ensure Tor is up FIRST, showing the anond bootstrap loader if it must start — isolation fails
    // closed without Tor, so rather than error we bring it up and the user watches the same
    // segmented Tor loader as `anond up`. If Tor's SOCKS port is already listening this is a fast
    // no-op and the app launches immediately. Then arxonion drops to the user and confines the app
    // to the Tor-only namespace. sudo --preserve-env carries the X display so a GUI app can show.
    let script = format!(
        "if ! ss -ltn 2>/dev/null | grep -q '127.0.0.1:9050'; then \
           echo '  Tor is not running yet — starting it (isolation needs Tor):'; \
           anond up --no-advice || exit 1; \
         fi; \
         arxonion run {app}"
    );
    spawn_terminal(&wrap_close(&format!("sudo --preserve-env=DISPLAY,XAUTHORITY bash -c \"{script}\"")))
}

// Run an anond action and STREAM its output back into the Privacy panel line by line, so
// everything the external terminal would print (each fail-closed step, the leak test, the new
// exit IP) is visible in the app itself. pkexec is used rather than a terminal handoff because
// there is no terminal here to authenticate in.
#[tauri::command]
async fn anond_action_streamed(app: tauri::AppHandle, action: String) -> Result<i32, String> {
    use tauri::Emitter;
    // owned, because these move into the worker thread below
    let args: Vec<String> = match action.as_str() {
        "up" | "down" | "verify" | "new-identity" => vec![action.clone()],
        "up-i2p" => vec!["up".into(), "--i2p".into()],
        _ => return Err("invalid action".into()),
    };
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let handle = std::thread::spawn(move || -> i32 {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;
        let mut cmd = std::process::Command::new("pkexec");
        cmd.arg("anond").args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match cmd.spawn() { Ok(c) => c, Err(e) => { let _ = tx.send(format!("could not start anond: {e}")); return -1; } };
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) { let _ = tx.send(line); }
        }
        if let Some(err) = child.stderr.take() {
            for line in BufReader::new(err).lines().map_while(Result::ok) { let _ = tx.send(line); }
        }
        child.wait().ok().and_then(|s| s.code()).unwrap_or(-1)
    });
    for line in rx { let _ = app.emit("anond-line", line); }
    handle.join().map_err(|_| "anond worker panicked".to_string())
}
// Re-apply the ArxOS browser hardening (Firefox, Waterfox, Brave): WebRTC leak protection,
// browser DoH off so DNS defers to anond's Tor pin, telemetry and tracking closed.
#[tauri::command]
fn browser_harden() -> Result<(), String> {
    spawn_terminal(&wrap_close("sudo /usr/lib/arxos/harden-browsers.sh"))
}

// Which browsers are actually installed on this machine, so the panel offers/reports exactly
// the browsers present — never claims to patch one that is absent, never misses one that is
// there. Detection is robust: the real launcher binary anywhere on PATH, OR any known install
// root (a browser installed via a package, /opt, or a lib dir is still found). Each browser
// lists several aliases/roots because distros and managers place them differently.
#[tauri::command]
fn browser_status() -> Vec<String> {
    // (display name, [binary names to look up on PATH], [absolute roots to probe])
    let browsers: &[(&str, &[&str], &[&str])] = &[
        ("Firefox",   &["firefox", "firefox-esr", "firefox-bin"], &["/usr/lib/firefox", "/usr/lib64/firefox", "/usr/share/firefox"]),
        ("Waterfox",  &["waterfox", "waterfox-bin"],              &["/opt/waterfox", "/usr/lib/waterfox"]),
        ("Brave",     &["brave", "brave-browser", "brave-bin"],  &["/etc/brave", "/opt/brave.com", "/usr/lib/brave-browser", "/usr/lib/brave-bin"]),
        ("LibreWolf", &["librewolf"],                            &["/usr/lib/librewolf", "/opt/librewolf"]),
        ("Mullvad Browser", &["mullvad-browser"],                &["/opt/mullvad-browser", "/usr/lib/mullvad-browser"]),
        ("Chromium",  &["chromium", "chromium-browser"],         &["/usr/lib/chromium", "/etc/chromium"]),
        ("Tor Browser", &["torbrowser-launcher", "tor-browser"], &["/opt/tor-browser", "/usr/lib/torbrowser"]),
    ];
    let on_path = |bin: &str| std::env::var("PATH").unwrap_or_default()
        .split(':').any(|d| std::path::Path::new(d).join(bin).exists());
    let mut found = Vec::new();
    for (name, bins, roots) in browsers {
        let present = bins.iter().any(|b| on_path(b))
            || roots.iter().any(|r| std::path::Path::new(r).exists());
        if present { found.push(name.to_string()); }
    }
    found
}

#[tauri::command]
fn system_update() -> Result<(), String> { launch_arx(&["upgrade"]) }
#[tauri::command]
fn sync_databases() -> Result<(), String> { launch_arx(&["refresh"]) } // re-syncs repo metadata only, no upgrade
#[tauri::command]
fn kernel_install(flavor: String) -> Result<(), String> {
    if !safe_token(&flavor) { return Err("invalid flavor".into()); }
    launch_arx(&["kernel", "install", &flavor])
}
#[tauri::command]
fn kernel_remove(flavor: String) -> Result<(), String> {
    if !safe_token(&flavor) { return Err("invalid flavor".into()); }
    launch_arx(&["kernel", "remove", &flavor])
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            system_info, updates_count, updates_breakdown, kernels_list, kernels_manifest, weapons_categories, arsenal_totals, arsenal_totals_refresh, weapons_menu_rebuild, browser_harden, browser_status, services_status,
            weapons_install, weapons_remove, weapons_browse, system_update, sync_databases, kernel_install, kernel_remove,
            anond_status, anond_action, anond_action_streamed, anond_exit_location,
            arxonion_status, arxonion_toggle, arxonion_shell, arxonion_launch_browser, arxonion_run_app,
            perf::perf_status, perf::perf_set_governor, perf::perf_set_epp, perf::perf_set_turbo, perf::perf_apply_profile,
            net::net_status, net::net_ports, net::net_disable_service, net::net_block_port,
            wallpaper::wallpapers_list, wallpaper::wallpaper_set, wallpaper::wallpaper_fetch,
            wallpaper::wallpaper_cycle_status, wallpaper::wallpaper_cycle_set,
            vm_tools::vm_status, vm_tools::vm_setup
        ])
        .run(tauri::generate_context!())
        .expect("error while running the ArxOS Control Center");
}
