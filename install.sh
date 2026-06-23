#!/usr/bin/env bash
set -e; D="$(cd "$(dirname "$0")" && pwd)"; S=""; [ "$(id -u)" -ne 0 ] && S=sudo
$S install -Dm755 "$D/arxctl"     /usr/local/bin/arxctl
$S install -Dm755 "$D/arxctl-gui" /usr/local/bin/arxctl-gui
$S install -Dm644 "$D/arxos-control.desktop" /usr/share/applications/arxos-control.desktop
python3 -c 'import gi; gi.require_version("Gtk","3.0")' 2>/dev/null || $S pacman -S --noconfirm --needed python-gobject gtk3 >/dev/null 2>&1 || true
echo "arxctl (TUI) + arxctl-gui (GUI) + launcher installed"
