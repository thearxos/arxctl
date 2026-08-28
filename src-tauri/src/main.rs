// ArxOS Control Center — native Rust (Tauri v2) backend. The command deck for the
// system: live stats, updates, kernels, the weapons arsenal (installed LIVE with
// per-step progress), performance, privacy (anonkit), services, and info. Every
// handler talks to the real ArxOS tools (arx, arxos-kernel) or /proc; nothing is
// mocked. Privileged actions go through pkexec so the GUI never holds root.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;

mod perf;

// ---------- small helpers ----------

fn read(path: &str) -> String { std::fs::read_to_string(path).unwrap_or_default() }

fn run(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd).args(args).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

// ---------- read-only system state ----------

#[derive(Serialize)]
struct SysInfo { host: String, distro: String, kernel: String, uptime: String, cpu: String, mem_used: u64, mem_total: u64, load: String }

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
    let load = read("/proc/loadavg").split_whitespace().take(3).collect::<Vec<_>>().join(" ");
    SysInfo { host, distro, kernel, uptime, cpu, mem_used: total.saturating_sub(avail), mem_total: total, load }
}

#[tauri::command]
fn updates_count() -> usize {
    // available updates without hitting the network hard (uses the local sync DBs)
    run("arx", &["outdated"]).lines().filter(|l| !l.trim().is_empty()).count()
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

// launch the OS terminal running `arx <args>`, held open after it finishes.
fn launch_arx(arx_args: &[&str]) -> Result<(), String> {
    let mut cmd = if have("konsole") {
        let mut c = std::process::Command::new("konsole"); c.args(["--hold", "-e", "arx"]); c
    } else if have("xterm") {
        let mut c = std::process::Command::new("xterm"); c.args(["-hold", "-e", "arx"]); c
    } else if have("x-terminal-emulator") {
        let mut c = std::process::Command::new("x-terminal-emulator"); c.args(["-e", "arx"]); c
    } else { return Err("no terminal emulator found".into()); };
    cmd.args(arx_args);
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
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
fn system_update() -> Result<(), String> { launch_arx(&["upgrade"]) }
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
            system_info, updates_count, kernels_list, weapons_categories, services_status,
            weapons_install, weapons_remove, system_update, kernel_install, kernel_remove,
            perf::perf_status, perf::perf_set_governor, perf::perf_set_epp, perf::perf_set_turbo, perf::perf_apply_profile
        ])
        .run(tauri::generate_context!())
        .expect("error while running the ArxOS Control Center");
}
