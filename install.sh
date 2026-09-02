#!/usr/bin/env bash
# arxctl installer. Places the native Control Center + its bundled tools, the menu
# entry, the Plank dock item, and the user timers (news + the worldwide update ping).
# Binaries are prebuilt (public dist ships them); if only sources are present, build.
set -u
D="$(cd "$(dirname "$0")" && pwd)"
S=""; [ "$(id -u)" -ne 0 ] && S=sudo

# --- binaries ----------------------------------------------------------------
# arxctl (Tauri GUI) + arxos-notify (update ping) come prebuilt in a dist; from a
# source checkout, build the release binaries first.
ARXCTL="$D/arxctl"; NOTIFY="$D/arxos-notify"; ANOND="$D/anond"; ONION="$D/arxonion"
[ -x "$ARXCTL" ] || ARXCTL="$D/target/release/arxctl"
[ -x "$NOTIFY" ] || NOTIFY="$D/target/release/arxos-notify"
[ -x "$ANOND" ]  || ANOND="$D/target/release/anond"
[ -x "$ONION" ]  || ONION="$D/target/release/arxonion"
if [ ! -x "$ARXCTL" ] || [ ! -x "$NOTIFY" ] || [ ! -x "$ANOND" ]; then
  if command -v cargo >/dev/null 2>&1 && [ -f "$D/Cargo.toml" ]; then
    # source checkout: build the whole workspace (arxctl + arxos-notify + anond + arxonion)
    ( cd "$D" && cargo build --release ) \
      && ARXCTL="$D/target/release/arxctl" && NOTIFY="$D/target/release/arxos-notify" && ANOND="$D/target/release/anond" && ONION="$D/target/release/arxonion"
  else
    # binary-only dist (public -dist mirror ships no source): download the prebuilt
    # binaries from the public release. Source never ships; only the compiled artifacts.
    VER="$(cat "$D/VERSION" 2>/dev/null || echo 0.0.1)"
    REPO="${ARXCTL_GITHUB_REPO:-thearxos/arxctl-dist}"
    BASE="https://github.com/${REPO}/releases/download/v${VER}"
    dl() { if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"; else wget -qO "$2" "$1"; fi; }
    TMP="$(mktemp -d)"
    dl "$BASE/arxctl"       "$TMP/arxctl"       && chmod +x "$TMP/arxctl"       && ARXCTL="$TMP/arxctl"
    dl "$BASE/arxos-notify" "$TMP/arxos-notify" && chmod +x "$TMP/arxos-notify" && NOTIFY="$TMP/arxos-notify"
    dl "$BASE/anond"        "$TMP/anond"        && chmod +x "$TMP/anond"        && ANOND="$TMP/anond"
    dl "$BASE/arxonion"     "$TMP/arxonion"     && chmod +x "$TMP/arxonion"     && ONION="$TMP/arxonion"
  fi
fi
[ -x "$ARXCTL" ] && $S install -Dm755 "$ARXCTL" /usr/local/bin/arxctl
[ -x "$NOTIFY" ] && $S install -Dm755 "$NOTIFY" /usr/local/bin/arxos-notify
# anond (the anonymity daemon) ships bundled inside the Control Center
[ -x "$ANOND" ]  && $S install -Dm755 "$ANOND"  /usr/local/bin/anond
# arxonion (per-app Tor-only network-namespace isolation), driven from the Privacy panel
[ -x "$ONION" ]  && $S install -Dm755 "$ONION"  /usr/local/bin/arxonion
# anond's transparent-proxy kill-switch needs a real netfilter backend + conntrack: it flushes
# the connection-tracking table when arming so no pre-existing flow can survive in the clear
# (the classic transproxy leak). These MUST be present or a user hits a missing package mid-arm
# and the kill-switch silently can't flush — install them up front, never leave it to chance.
$S pacman -S --noconfirm --needed nftables iptables-nft conntrack-tools tor >/dev/null 2>&1 || true
$S install -Dm755 "$D/arxos-news"   /usr/local/bin/arxos-news   2>/dev/null
$S install -Dm755 "$D/arxos-kernel" /usr/local/bin/arxos-kernel 2>/dev/null

# --- hardware-info cache (memory type/speed for the dashboard; dmidecode needs root) --------
command -v dmidecode >/dev/null 2>&1 || $S pacman -S --noconfirm --needed dmidecode 2>/dev/null || true
$S install -Dm755 "$D/data/arxos-hwinfo.sh"              /usr/local/bin/arxos-hwinfo
$S install -Dm644 "$D/data/systemd/arxos-hwinfo.service" /usr/lib/systemd/system/arxos-hwinfo.service
$S systemctl enable arxos-hwinfo.service 2>/dev/null || true
$S systemctl start  arxos-hwinfo.service 2>/dev/null || true

# --- icon + menu entry -------------------------------------------------------
ICON="$D/src-tauri/icons/icon.png"; [ -f "$ICON" ] || ICON="$D/assets/icons/arxctl.png"
[ -f "$ICON" ] && $S install -Dm644 "$ICON" /usr/share/icons/hicolor/512x512/apps/arxctl.png
# the AnonKit icon (the arxos-anonkit mark) — the icon for the anonkit toolkit everywhere
[ -f "$D/assets/anonkit.png" ] && $S install -Dm644 "$D/assets/anonkit.png" /usr/share/icons/ArxOS/arxos-anonkit.png
$S install -Dm644 "$D/arxos-control.desktop" /usr/share/applications/arxos-control.desktop
$S gtk-update-icon-cache -f /usr/share/icons/hicolor 2>/dev/null || true
$S update-desktop-database 2>/dev/null || true

# --- Plank dock item (system default, seeded to every user via /etc/skel) ----
_plank() { # $1 = target home
  local h="$1"
  local f="$h/.config/plank/dock1/launchers/00-arxctl.dockitem"
  $S install -d "$(dirname "$f")"
  printf '[PlankDockItemPreferences]\nLauncher=file:///usr/share/applications/arxos-control.desktop\n' | $S tee "$f" >/dev/null
}
for home in /etc/skel /home/*; do [ -d "$home" ] && _plank "$home"; done

# --- user timers: ArxOS news + the worldwide update ping ---------------------
$S install -Dm644 "$D/arxos-news.service"        /usr/lib/systemd/user/arxos-news.service   2>/dev/null
$S install -Dm644 "$D/arxos-news.timer"          /usr/lib/systemd/user/arxos-news.timer     2>/dev/null
$S install -Dm644 "$D/data/systemd/arxos-notify.service" /usr/lib/systemd/user/arxos-notify.service
$S install -Dm644 "$D/data/systemd/arxos-notify.timer"   /usr/lib/systemd/user/arxos-notify.timer
$S systemctl --global enable arxos-news.timer arxos-notify.timer 2>/dev/null || true

echo "ArxOS Control Center installed: menu + Plank + update-ping timer enabled."
