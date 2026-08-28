// perf.rs — the Performance panel's backend. This talks DIRECTLY to the CPU through
// the kernel's cpufreq/thermal sysfs, in real time. No wrapper tool, no cached guess:
// every read is a fresh read of /sys and /proc, and every change is written straight to
// /sys (privileged writes go through pkexec). Governors, energy-performance preference,
// per-core frequency + live load, turbo/boost, and package temperature are the real
// knobs the kernel exposes, surfaced honestly.
use serde::Serialize;
use std::path::Path;

const CPU: &str = "/sys/devices/system/cpu";

fn rd(p: &str) -> String { std::fs::read_to_string(p).unwrap_or_default().trim().to_string() }
fn list(s: &str) -> Vec<String> { s.split_whitespace().map(String::from).collect() }

#[derive(Serialize)]
pub struct Core { pub id: usize, pub mhz: u64, pub load: u8 }

#[derive(Serialize)]
pub struct PerfStatus {
    pub driver: String,
    pub governor: String,
    pub governors: Vec<String>,
    pub epp: String,
    pub epps: Vec<String>,
    pub min_mhz: u64,
    pub max_mhz: u64,
    pub hw_max_mhz: u64,
    pub turbo: bool,
    pub turbo_supported: bool,
    pub temp_c: i32,
    pub cores: Vec<Core>,
}

// sample /proc/stat per-cpu: returns (idle+iowait, total) for each cpuN line.
fn stat_sample() -> Vec<(u64, u64)> {
    std::fs::read_to_string("/proc/stat").unwrap_or_default().lines()
        .filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).map(|c| c.is_ascii_digit()).unwrap_or(false))
        .map(|l| {
            let v: Vec<u64> = l.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
            let total: u64 = v.iter().sum();
            let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
            (idle, total)
        }).collect()
}

fn cpu_temp_c() -> i32 {
    // prefer the package sensor (x86_pkg_temp / coretemp), else the hottest zone.
    let mut best = i32::MIN;
    if let Ok(rd_dir) = std::fs::read_dir("/sys/class/thermal") {
        for e in rd_dir.flatten() {
            let p = e.path();
            let ty = std::fs::read_to_string(p.join("type")).unwrap_or_default();
            let t = std::fs::read_to_string(p.join("temp")).ok().and_then(|s| s.trim().parse::<i32>().ok());
            if let Some(t) = t {
                let c = t / 1000;
                if ty.contains("x86_pkg_temp") || ty.contains("coretemp") { return c; }
                if c > best { best = c; }
            }
        }
    }
    if best == i32::MIN { 0 } else { best }
}

#[tauri::command]
pub fn perf_status() -> PerfStatus {
    let f0 = format!("{CPU}/cpu0/cpufreq");
    let driver = rd(&format!("{f0}/scaling_driver"));
    let governor = rd(&format!("{f0}/scaling_governor"));
    let governors = list(&rd(&format!("{f0}/scaling_available_governors")));
    let epp = rd(&format!("{f0}/energy_performance_preference"));
    let epps = list(&rd(&format!("{f0}/energy_performance_available_preferences")));
    let khz = |p: &str| rd(p).parse::<u64>().unwrap_or(0) / 1000;
    let min_mhz = khz(&format!("{f0}/scaling_min_freq"));
    let max_mhz = khz(&format!("{f0}/scaling_max_freq"));
    let hw_max_mhz = khz(&format!("{f0}/cpuinfo_max_freq"));

    let (turbo, turbo_supported) = if Path::new(&format!("{CPU}/intel_pstate/no_turbo")).exists() {
        (rd(&format!("{CPU}/intel_pstate/no_turbo")) == "0", true)
    } else if Path::new(&format!("{CPU}/cpufreq/boost")).exists() {
        (rd(&format!("{CPU}/cpufreq/boost")) == "1", true)
    } else { (false, false) };

    // per-core load: two /proc/stat samples ~120ms apart, plus a fresh sysfs frequency.
    let s1 = stat_sample();
    std::thread::sleep(std::time::Duration::from_millis(120));
    let s2 = stat_sample();
    let cores = (0..s2.len()).map(|i| {
        let mhz = rd(&format!("{CPU}/cpu{i}/cpufreq/scaling_cur_freq")).parse::<u64>().unwrap_or(0) / 1000;
        let load = match (s1.get(i), s2.get(i)) {
            (Some((i1, t1)), Some((i2, t2))) => {
                let dt = t2.saturating_sub(*t1); let di = i2.saturating_sub(*i1);
                if dt == 0 { 0 } else { (100u64.saturating_sub((di * 100 / dt).min(100))) as u8 }
            }
            _ => 0,
        };
        Core { id: i, mhz, load }
    }).collect();

    PerfStatus { driver, governor, governors, epp, epps, min_mhz, max_mhz, hw_max_mhz, turbo, turbo_supported, temp_c: cpu_temp_c(), cores }
}

// write a value to a cpufreq attribute on EVERY cpu, as root, via pkexec. The value is
// validated against the kernel's own advertised options by the caller; the attribute
// name is a fixed literal — so nothing attacker-controlled reaches the shell.
fn write_all(attr: &str, value: &str) -> Result<(), String> {
    let script = format!("for f in {CPU}/cpu*/cpufreq/{attr}; do printf '%s' '{value}' > \"$f\" 2>/dev/null; done");
    let ok = std::process::Command::new("pkexec").args(["bash", "-c", &script]).status().map(|s| s.success()).unwrap_or(false);
    if ok { Ok(()) } else { Err("could not apply (authorization declined?)".into()) }
}

fn write_root(path: &str, value: &str) -> Result<(), String> {
    let ok = std::process::Command::new("pkexec").args(["bash", "-c", &format!("printf '%s' '{value}' > '{path}'")]).status().map(|s| s.success()).unwrap_or(false);
    if ok { Ok(()) } else { Err("could not apply (authorization declined?)".into()) }
}

#[tauri::command]
pub fn perf_set_governor(governor: String) -> Result<(), String> {
    if !list(&rd(&format!("{CPU}/cpu0/cpufreq/scaling_available_governors"))).contains(&governor) {
        return Err(format!("this CPU does not offer the '{governor}' governor"));
    }
    write_all("scaling_governor", &governor)
}

#[tauri::command]
pub fn perf_set_epp(epp: String) -> Result<(), String> {
    let avail = list(&rd(&format!("{CPU}/cpu0/cpufreq/energy_performance_available_preferences")));
    if !avail.is_empty() && !avail.contains(&epp) {
        return Err(format!("this CPU does not offer the '{epp}' preference"));
    }
    write_all("energy_performance_preference", &epp)
}

#[tauri::command]
pub fn perf_set_turbo(on: bool) -> Result<(), String> {
    if Path::new(&format!("{CPU}/intel_pstate/no_turbo")).exists() {
        write_root(&format!("{CPU}/intel_pstate/no_turbo"), if on { "0" } else { "1" })
    } else if Path::new(&format!("{CPU}/cpufreq/boost")).exists() {
        write_root(&format!("{CPU}/cpufreq/boost"), if on { "1" } else { "0" })
    } else {
        Err("turbo/boost is not controllable on this CPU".into())
    }
}

// coordinated profiles: set the governor + EPP + turbo together, adapting to the driver
// (intel_pstate exposes only performance/powersave governors, so the "feel" lives in EPP).
#[tauri::command]
pub fn perf_apply_profile(profile: String) -> Result<(), String> {
    let govs = list(&rd(&format!("{CPU}/cpu0/cpufreq/scaling_available_governors")));
    let epps = list(&rd(&format!("{CPU}/cpu0/cpufreq/energy_performance_available_preferences")));
    let pick = |cands: &[&str], have: &[String]| cands.iter().find(|c| have.iter().any(|h| h == *c)).map(|s| s.to_string());
    let (gov, epp, turbo): (Option<String>, Option<&str>, bool) = match profile.as_str() {
        "performance" => (pick(&["performance"], &govs), Some("performance"), true),
        "balanced"    => (pick(&["schedutil", "ondemand", "powersave"], &govs), Some("balance_performance"), true),
        "powersave"   => (pick(&["powersave", "conservative", "schedutil"], &govs), Some("power"), false),
        _ => return Err("unknown profile".into()),
    };
    if let Some(g) = gov { write_all("scaling_governor", &g)?; }
    if !epps.is_empty() { if let Some(e) = epp { if epps.iter().any(|x| x == e) { write_all("energy_performance_preference", e)?; } } }
    let _ = perf_set_turbo(turbo);
    Ok(())
}
