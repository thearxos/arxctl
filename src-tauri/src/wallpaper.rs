// wallpaper.rs — the Wallpaper panel backend. This is a thin driver over the
// real wallpaper engine, `arxos-wallpaper-engine` (native Go, per the ARXOS
// language policy: hot paths are compiled, not Python). It owns the curated
// background dirs and the exact xfconf-query + xfdesktop --reload sequence
// that actually applies a background across every monitor/workspace. We never
// reimplement that logic here — one engine, one set of semantics. The GTK
// browser (arxos-wallpaper, Python glue) shells into the same binary.
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

fn engine() -> Option<&'static str> {
    for cand in ["/usr/local/bin/arxos-wallpaper-engine", "arxos-wallpaper-engine"] {
        if cand.starts_with('/') {
            if Path::new(cand).is_file() { return Some(cand); }
        } else if std::env::var("PATH").unwrap_or_default().split(':').any(|d| Path::new(d).join(cand).exists()) {
            return Some(cand);
        }
    }
    None
}

#[derive(Serialize, Deserialize)]
pub struct Wallpaper {
    pub path: String,
    pub name: String,
    pub source: String,          // which curated dir it came from: arxos | system | user
    #[serde(default)]
    pub width: u32,               // 0 when the format's header couldn't be decoded (webp)
    #[serde(default)]
    pub height: u32,
}

#[derive(Serialize, Deserialize)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Deserialize)]
pub struct WallpaperCatalog {
    pub wallpapers: Vec<Wallpaper>,
    // every image-style the desktop manager itself supports (none/centered/tiled/
    // stretched/scaled/zoomed) — the engine is the single source of truth for this list.
    pub styles: Vec<String>,
    pub default_style: String,
    // the live X screen size (from xrandr), so the UI can flag which wallpapers
    // already match the desktop's actual resolution.
    pub desktop: Screen,
}

#[tauri::command]
pub fn wallpapers_list() -> Result<WallpaperCatalog, String> {
    let bin = engine().ok_or("arxos-wallpaper-engine is not installed")?;
    let out = Command::new(bin).arg("--list-json").output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn wallpaper_set(path: String, style: Option<String>) -> Result<(), String> {
    let bin = engine().ok_or("arxos-wallpaper-engine is not installed")?;
    // the engine itself validates the path is a real image and the style name before
    // touching xfconf
    let mut args = vec!["--set".to_string(), path];
    if let Some(s) = style { args.push("--style".to_string()); args.push(s); }
    let st = Command::new(bin).args(&args).status().map_err(|e| e.to_string())?;
    if st.success() { Ok(()) } else { Err("arxos-wallpaper --set failed".into()) }
}

#[derive(Serialize, Deserialize)]
pub struct FetchResult {
    pub added: u32,
    pub limit: u32,
    pub found: u32,
    pub dest: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
}

// "Download more" — curated GitHub sources + Unsplash, ported feature-for-feature
// from the original standalone wallpaper tool. This blocks until the batch
// finishes (the engine parallelizes internally); the panel shows a spinner.
#[tauri::command]
pub fn wallpaper_fetch(limit: u32, source: String) -> Result<FetchResult, String> {
    let bin = engine().ok_or("arxos-wallpaper-engine is not installed")?;
    if !["curated", "unsplash", "both"].contains(&source.as_str()) {
        return Err("source must be curated, unsplash, or both".into());
    }
    let out = Command::new(bin)
        .args(["--fetch", "--limit", &limit.to_string(), "--source", &source])
        .output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

#[derive(Serialize, Deserialize)]
pub struct CycleStatus {
    pub enabled: bool,
    pub shuffle: bool,
    pub interval_secs: u32,
}

#[tauri::command]
pub fn wallpaper_cycle_status() -> Result<CycleStatus, String> {
    let bin = engine().ok_or("arxos-wallpaper-engine is not installed")?;
    let out = Command::new(bin).arg("--cycle-status").output().map_err(|e| e.to_string())?;
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn wallpaper_cycle_set(enabled: Option<bool>, shuffle: Option<bool>) -> Result<CycleStatus, String> {
    let bin = engine().ok_or("arxos-wallpaper-engine is not installed")?;
    let mut args = vec!["--cycle-set".to_string()];
    match enabled { Some(true) => args.push("--on".into()), Some(false) => args.push("--off".into()), None => {} }
    match shuffle { Some(true) => args.push("--shuffle-on".into()), Some(false) => args.push("--shuffle-off".into()), None => {} }
    let out = Command::new(bin).args(&args).output().map_err(|e| e.to_string())?;
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}
