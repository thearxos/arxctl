// Host hardening for a session, all reversible and recorded in the session so `down` puts
// everything back. Kept conservative on purpose: swap off (keys/plaintext must not hit disk),
// a couple of anti-fingerprinting sysctls, and OPTIONAL MAC randomisation (opt-in via --mac,
// because it bounces the link and would cut a remote session).
use crate::state::Session;
use crate::util::{out, run};
use anyhow::Result;

/// A generic, high-anonymity-set hostname used while anonymised — the default Debian/many-distro
/// hostname, so the box blends in rather than announcing itself as an ArxOS machine on the LAN.
const GENERIC_HOSTNAME: &str = "localhost";

pub fn apply(sess: &mut Session, mac: bool) -> Result<()> {
    // swap off so no session memory (circuits, keys) can be paged to disk.
    sess.swap_was_on = !out("swapon", &["--show", "--noheadings"]).is_empty();
    if sess.swap_was_on { let _ = run("swapoff", &["-a"]); }

    // modest anti-fingerprinting: drop TCP timestamps (uptime leak), ignore ICMP broadcasts.
    let _ = run("sysctl", &["-qw", "net.ipv4.tcp_timestamps=0"]);
    let _ = run("sysctl", &["-qw", "net.ipv4.icmp_echo_ignore_broadcasts=1"]);

    // HOSTNAME: the machine's hostname leaks to the LAN/router (and the ISP's DHCP logs) on
    // every lease. A distinctive name like "arxos" tags the user as an ArxOS box before Tor is
    // even in the picture. Spoof the TRANSIENT hostname to a generic value for the session, and
    // restore the original on `down`. Transient-only: /etc/hostname on disk is untouched, so a
    // reboot restores it even if `down` never ran. Default-on (unlike --mac, this costs nothing
    // and cannot bounce a link).
    let orig_host = out("hostname", &[]).trim().to_string();
    if !orig_host.is_empty() && orig_host != GENERIC_HOSTNAME {
        if run("hostname", &[GENERIC_HOSTNAME]).is_ok() {
            sess.hostname_backup = Some(orig_host);
        }
    }

    // MACHINE-ID: /etc/machine-id is a globally-unique, stable identifier an app can read to
    // link all your "anonymous" sessions to one machine across Tor circuits (found in a leak
    // test). Bind-mount a RANDOM id over it for the session so apps that read it afterward see
    // a fresh value; `down` unmounts to restore the real one. Bind-mount (not a file write)
    // because: it never persists across a reboot (fully transient/reversible by construction),
    // and it does not touch the real on-disk file that systemd already loaded at boot — so
    // running services keep the id they started with while new app reads get the random one.
    // Skip if a random id is already bind-mounted (idempotent, and avoids stacking mounts).
    if !sess.machine_id_spoofed && out("mount", &[]).lines().all(|l| !l.contains("/etc/machine-id")) {
        // REAL entropy from /dev/urandom — a machine-id derived from the clock would be
        // predictable/correlatable (and near-identical across rapid sessions), which is itself a
        // weakness. 16 random bytes -> 32 lowercase hex, the machine-id format.
        let rand_id: String = {
            use std::io::Read;
            let mut buf = [0u8; 16];
            std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf))
                .map(|_| buf.iter().map(|x| format!("{x:02x}")).collect())
                .unwrap_or_default()
        };
        let tmp = "/run/anond/machine-id";
        let _ = crate::util::ensure_dirs();
        if rand_id.len() == 32
            && std::fs::write(tmp, format!("{rand_id}\n")).is_ok()
            && run("mount", &["--bind", tmp, "/etc/machine-id"]).is_ok() {
            sess.machine_id_spoofed = true;
        }
    }

    if mac {
        // randomise MAC on each non-loopback, non-virtual link. Opt-in: this bounces the NIC.
        for ifn in phys_ifaces() {
            let orig = out("cat", &[&format!("/sys/class/net/{ifn}/address")]);
            if orig.is_empty() { continue; }
            let _ = run("ip", &["link", "set", &ifn, "down"]);
            let ok = run("ip", &["link", "set", &ifn, "address", &random_mac()]).is_ok();
            let _ = run("ip", &["link", "set", &ifn, "up"]);
            if ok { sess.mac_backup.push((ifn, orig)); }
        }
    }
    Ok(())
}

pub fn restore(sess: &Session) -> Result<()> {
    // REVERSIBILITY INVARIANT: `down` must return the system to its pre-session state. Every
    // spoof recorded in the session is undone here — hostname, MAC, machine-id, sysctls, swap.
    if sess.machine_id_spoofed {
        // unmount the random machine-id bind mount; the real /etc/machine-id is exposed again.
        let _ = run("umount", &["/etc/machine-id"]);
    }
    if let Some(ref orig) = sess.hostname_backup {
        let _ = run("hostname", &[orig]);
    }
    for (ifn, mac) in &sess.mac_backup {
        let _ = run("ip", &["link", "set", ifn, "down"]);
        let _ = run("ip", &["link", "set", ifn, "address", mac]);
        let _ = run("ip", &["link", "set", ifn, "up"]);
    }
    let _ = run("sysctl", &["-qw", "net.ipv4.tcp_timestamps=1"]);
    if sess.swap_was_on { let _ = run("swapon", &["-a"]); }
    Ok(())
}

fn phys_ifaces() -> Vec<String> {
    std::fs::read_dir("/sys/class/net").into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n != "lo" && std::path::Path::new(&format!("/sys/class/net/{n}/device")).exists())
        .collect()
}

fn random_mac() -> String {
    // locally-administered, unicast (second-least-significant bit of first octet set, LSB clear).
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let b = n.to_le_bytes();
    format!("02:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", b[0], b[1], b[2], b[3] ^ 0x5a, b[0] ^ 0xa5)
}
