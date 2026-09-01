// vm_tools.rs — the VM Tools panel backend. One-click turnkey setup for running virtual
// machines ON ArxOS: QEMU/KVM (+ virt-manager), VirtualBox, and VMware Workstation
// (AUR-built, no official Arch package exists). Each install hands off to a terminal
// (same pattern as Weapons/Kernels/Update) so the user watches every step and
// authenticates there — the GUI never holds root.
use serde::Serialize;
use std::process::Command;

fn have(bin: &str) -> bool {
    std::env::var("PATH").unwrap_or_default().split(':').any(|d| std::path::Path::new(d).join(bin).exists())
}

fn in_group(group: &str) -> bool {
    Command::new("id").arg("-nG").output()
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().any(|g| g == group))
        .unwrap_or(false)
}

fn systemd_active(unit: &str) -> bool {
    Command::new("systemctl").args(["is-active", "--quiet", unit]).status().map(|s| s.success()).unwrap_or(false)
}

// ArxOS ships its kernel flavor's own headers package (linux-arxos-headers /
// linux-arxos-rt-headers), not the generic "linux-headers" meta-package official Arch
// uses — DKMS (VirtualBox's kernel module) has to build against whichever is actually
// running, so this is read from `uname -r`, never hardcoded.
fn headers_package() -> String {
    let rel = Command::new("uname").arg("-r").output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    if rel.ends_with("-arxos-rt") { "linux-arxos-rt-headers".into() }
    else if rel.ends_with("-arxos") { "linux-arxos-headers".into() }
    else { "linux-headers".into() } // a non-ArxOS kernel is running; fall back to the stock meta-package
}

#[derive(Serialize)]
pub struct VmEngine {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub ready: bool,       // installed AND fully set up (service running / module loaded / group membership)
    pub detail: String,    // what's missing, or confirmation it's ready
}

#[tauri::command]
pub fn vm_status() -> Vec<VmEngine> {
    let mut out = Vec::new();

    // QEMU/KVM + virt-manager
    let qemu_installed = have("qemu-system-x86_64");
    let qemu_ready = qemu_installed && have("virsh") && systemd_active("libvirtd") && in_group("libvirt");
    out.push(VmEngine {
        id: "qemu".into(), name: "QEMU/KVM".into(), installed: qemu_installed, ready: qemu_ready,
        detail: if qemu_ready { "libvirtd running, ready to launch VMs".into() }
            else if qemu_installed { "installed, but needs group/service setup".into() }
            else { "not installed".into() },
    });

    // VirtualBox
    let vbox_installed = have("VBoxManage");
    let vbox_module_loaded = std::fs::read_to_string("/proc/modules").map(|m| m.contains("vboxdrv")).unwrap_or(false);
    let vbox_ready = vbox_installed && vbox_module_loaded && in_group("vboxusers");
    out.push(VmEngine {
        id: "virtualbox".into(), name: "VirtualBox".into(), installed: vbox_installed, ready: vbox_ready,
        detail: if vbox_ready { "kernel module loaded, ready to launch VMs".into() }
            else if vbox_installed { "installed, but the kernel module or group membership is missing".into() }
            else { "not installed".into() },
    });

    // VMware Workstation (AUR-only; no official Arch package)
    let vmware_installed = have("vmware");
    out.push(VmEngine {
        id: "vmware".into(), name: "VMware Workstation".into(), installed: vmware_installed, ready: vmware_installed,
        detail: if vmware_installed { "installed".into() } else { "AUR-only — this builds from source, no prebuilt package exists".into() },
    });

    out
}

fn wrap_close(cmd: &str) -> String {
    format!("{cmd}; __rc=$?; echo; if [ $__rc -eq 0 ]; then echo '  ✔ done — closing…'; sleep 3; \
             else echo '  ✖ finished with errors'; read -r -t 120 -p '  press Enter to close… ' _; fi")
}

fn spawn_terminal(bash_cmd: &str) -> Result<(), String> {
    let mut cmd = if have("konsole") { let mut c = Command::new("konsole"); c.args(["-e", "bash", "-c", bash_cmd]); c }
        else if have("xterm") { let mut c = Command::new("xterm"); c.args(["-e", "bash", "-c", bash_cmd]); c }
        else if have("x-terminal-emulator") { let mut c = Command::new("x-terminal-emulator"); c.args(["-e", "bash", "-c", bash_cmd]); c }
        else { return Err("no terminal emulator found".into()); };
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vm_setup(target: String) -> Result<(), String> {
    let user = std::env::var("USER").or_else(|_| std::env::var("SUDO_USER")).map_err(|_| "could not determine the current user".to_string())?;
    if !user.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') { return Err("invalid username".into()); }

    let script = match target.as_str() {
        "qemu" => format!(
            "set -e; echo '== QEMU/KVM + virt-manager =='; \
             sudo pacman -S --needed --noconfirm qemu-full virt-manager virt-viewer dnsmasq vde2 bridge-utils openbsd-netcat dmidecode; \
             sudo usermod -aG libvirt,kvm {user}; \
             sudo systemctl enable --now libvirtd.service; \
             sudo virsh net-autostart default 2>/dev/null || true; \
             sudo virsh net-start default 2>/dev/null || true; \
             echo; echo 'Done. Log out and back in for the new group membership to take effect.'"
        ),
        "virtualbox" => {
            let headers = headers_package();
            format!(
                "set -e; echo '== VirtualBox =='; \
                 sudo pacman -S --needed --noconfirm {headers} virtualbox virtualbox-host-dkms; \
                 sudo usermod -aG vboxusers {user}; \
                 sudo modprobe vboxdrv; \
                 echo; echo 'Done. Log out and back in for the new group membership to take effect.'"
            )
        }
        "vmware" => "set -e; echo '== VMware Workstation (AUR build — this compiles from source and takes a while) =='; arx aur vmware-workstation".to_string(),
        _ => return Err("unknown VM engine".into()),
    };
    spawn_terminal(&wrap_close(&script))
}
