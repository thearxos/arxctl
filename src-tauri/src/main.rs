// ArxOS Control Center — native Rust (Tauri v2) backend. The command deck for the
// system: live stats, updates, kernels, the weapons arsenal (installed LIVE with
// per-step progress), performance, privacy (anonkit), services, and info. Every
// handler talks to the real ArxOS tools (arx, arxos-kernel) or /proc; nothing is
// mocked. Privileged actions go through pkexec so the GUI never holds root.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as AsyncCommand;

// ---------- small helpers ----------

fn read(path: &str) -> String { std::fs::read_to_string(path).unwrap_or_default() }

fn run(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd).args(args).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

// strip ANSI escapes + carriage returns so streamed tool output renders cleanly in the UI
fn clean_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' { if it.peek() == Some(&'[') { it.next(); while let Some(&n) = it.peek() { it.next(); if ('@'..='~').contains(&n) { break; } } } else { it.next(); } continue; }
        if c == '\r' { continue; }
        out.push(c);
    }
    out.trim_end().to_string()
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

// ---------- live, streamed actions (the "watch it happen" part) ----------

// spawn a privileged command and stream every output line to the frontend as a
// `deck://progress` event, then a final `deck://done` with success. `topic` scopes
// the events so different panels (weapons, update, kernel) can listen independently.
async fn stream(app: AppHandle, topic: String, cmd: &str, args: Vec<String>) -> Result<(), String> {
    let mut child = AsyncCommand::new(cmd).args(&args)
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .spawn().map_err(|e| format!("could not start {cmd}: {e}"))?;
    let _ = app.emit(&format!("{topic}:progress"), format!("$ {cmd} {}", args.join(" ")));
    // merge stdout + stderr line streams (arx renders its UI on stderr)
    if let Some(out) = child.stdout.take() {
        let (app2, topic2) = (app.clone(), topic.clone());
        tokio::spawn(async move { let mut l = BufReader::new(out).lines(); while let Ok(Some(x)) = l.next_line().await { let c = clean_line(&x); if !c.is_empty() { let _ = app2.emit(&format!("{topic2}:progress"), c); } } });
    }
    if let Some(err) = child.stderr.take() {
        let (app2, topic2) = (app.clone(), topic.clone());
        tokio::spawn(async move { let mut l = BufReader::new(err).lines(); while let Ok(Some(x)) = l.next_line().await { let c = clean_line(&x); if !c.is_empty() { let _ = app2.emit(&format!("{topic2}:progress"), c); } } });
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    let ok = status.success();
    let _ = app.emit(&format!("{topic}:done"), ok);
    if ok { Ok(()) } else { Err(format!("{topic} exited with an error")) }
}

// arsenal names are validated by arx's own guard; here we only allow the shape the UI
// produces (a category token or a keyword) so nothing odd reaches pkexec.
fn safe_token(s: &str) -> bool { !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b' ') }

#[tauri::command]
async fn weapons_install(app: AppHandle, category: String) -> Result<(), String> {
    if !safe_token(&category) { return Err("invalid category".into()); }
    let args: Vec<String> = vec!["arx".into(), "weapons".into(), "install".into()].into_iter().chain(category.split_whitespace().map(String::from)).collect();
    stream(app, "weapons".into(), "pkexec", args).await
}

#[tauri::command]
async fn weapons_remove(app: AppHandle, category: String) -> Result<(), String> {
    if !safe_token(&category) { return Err("invalid category".into()); }
    let args: Vec<String> = vec!["arx".into(), "weapons".into(), "remove".into()].into_iter().chain(category.split_whitespace().map(String::from)).collect();
    stream(app, "weapons".into(), "pkexec", args).await
}

#[tauri::command]
async fn system_update(app: AppHandle) -> Result<(), String> {
    stream(app, "update".into(), "pkexec", vec!["arx".into(), "upgrade".into()]).await
}

#[tauri::command]
async fn kernel_install(app: AppHandle, flavor: String) -> Result<(), String> {
    if !safe_token(&flavor) { return Err("invalid flavor".into()); }
    stream(app, "kernel".into(), "pkexec", vec!["arx".into(), "kernel".into(), "install".into(), flavor]).await
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            system_info, updates_count, kernels_list, weapons_categories, services_status,
            weapons_install, weapons_remove, system_update, kernel_install
        ])
        .run(tauri::generate_context!())
        .expect("error while running the ArxOS Control Center");
}
