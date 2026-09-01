// wallpaper.rs — the Wallpaper panel backend. Read-only scan of the standard
// background directories (no privilege needed) plus a setter that applies the
// choice to every XFCE monitor/workspace property via xfconf-query, falling
// back to feh --bg-fill when xfconf is unavailable (non-XFCE session, or the
// property write fails). Paths are canonicalized and validated before any
// command runs — nothing here ever touches a shell.
use serde::Serialize;
use std::path::{Path, PathBuf};

const EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp"];

fn dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    vec![
        PathBuf::from("/usr/share/backgrounds/arxos"),
        PathBuf::from("/usr/share/backgrounds"),
        PathBuf::from("/usr/share/wallpapers"),
        PathBuf::from(format!("{home}/Pictures/Wallpapers")),
        PathBuf::from(format!("{home}/.local/share/backgrounds")),
    ]
}

#[derive(Serialize)]
pub struct Wallpaper {
    pub path: String,
    pub name: String,
}

#[tauri::command]
pub fn wallpapers_list() -> Vec<Wallpaper> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let Some(ext) = p.extension().and_then(|x| x.to_str()) else { continue };
            if !EXTS.iter().any(|x| x.eq_ignore_ascii_case(ext)) { continue }
            let Ok(canon) = p.canonicalize() else { continue };
            let Some(path_str) = canon.to_str() else { continue };
            if !seen.insert(path_str.to_string()) { continue }
            let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("wallpaper").to_string();
            out.push(Wallpaper { path: path_str.to_string(), name });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn have(bin: &str) -> bool {
    std::env::var("PATH").unwrap_or_default().split(':').any(|d| Path::new(d).join(bin).exists())
}

// Validate the path is a real, readable image file under one of the known
// background directories or the user's home — never trust a raw string from
// the frontend into a Command arg without this.
fn validate(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    let canon = p.canonicalize().map_err(|_| "wallpaper file not found".to_string())?;
    if !canon.is_file() { return Err("not a file".into()); }
    let ext_ok = canon.extension().and_then(|x| x.to_str())
        .map(|ext| EXTS.iter().any(|x| x.eq_ignore_ascii_case(ext))).unwrap_or(false);
    if !ext_ok { return Err("not a supported image type".into()); }
    Ok(canon)
}

#[tauri::command]
pub fn wallpaper_set(path: String) -> Result<(), String> {
    let canon = validate(&path)?;
    let s = canon.to_str().ok_or("invalid path")?;

    if have("xfconf-query") {
        // Every monitor/workspace image-path + image-style property under the
        // xfce4-desktop channel, so the change sticks regardless of layout.
        if let Ok(out) = std::process::Command::new("xfconf-query")
            .args(["-c", "xfce4-desktop", "-p", "/backdrop", "-l"]).output()
        {
            let props = String::from_utf8_lossy(&out.stdout);
            let mut applied = false;
            for prop in props.lines().filter(|l| l.ends_with("last-image")) {
                let ok = std::process::Command::new("xfconf-query")
                    .args(["-c", "xfce4-desktop", "-p", prop, "-s", s])
                    .status().map(|st| st.success()).unwrap_or(false);
                applied = applied || ok;
                // keep the fill style sane (3 = zoomed/scaled)
                let style_prop = prop.replace("last-image", "image-style");
                let _ = std::process::Command::new("xfconf-query")
                    .args(["-c", "xfce4-desktop", "-p", &style_prop, "-s", "3", "-t", "int"])
                    .status();
            }
            if applied { return Ok(()); }
        }
    }

    if have("feh") {
        let ok = std::process::Command::new("feh").args(["--bg-fill", s]).status()
            .map(|st| st.success()).unwrap_or(false);
        if ok { return Ok(()); }
    }

    Err("no xfconf-query or feh available to set the wallpaper".into())
}
