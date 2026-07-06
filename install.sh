#!/usr/bin/env bash
D=$(cd "$(dirname "$0")" && pwd); S=""; [ "$(id -u)" -ne 0 ] && S=sudo
$S install -Dm755 "$D/arxctl-gui" /usr/local/bin/arxctl-gui
[ -f "$D/arxctl" ] && $S install -Dm755 "$D/arxctl" /usr/local/bin/arxctl
$S install -Dm755 "$D/arxos-news" /usr/local/bin/arxos-news
$S install -Dm755 "$D/arxos-kernel" /usr/local/bin/arxos-kernel
$S install -Dm644 "$D/arxos-news.service" /usr/lib/systemd/user/arxos-news.service
$S install -Dm644 "$D/arxos-news.timer"   /usr/lib/systemd/user/arxos-news.timer
$S systemctl --global enable arxos-news.timer 2>/dev/null || true
echo "arxctl + arxos-news installed (news timer enabled for all users)"

# ---- ArxOS desktop branding (ArxOSICONS + rebranded launchers) ----
_D="$(cd "$(dirname "$0")" && pwd)"; _S=""; [ "$(id -u)" -ne 0 ] && _S=sudo
$_S install -d /usr/share/icons/ArxOS
[ -d "$_D/assets/icons" ] && $_S cp "$_D"/assets/icons/*.png /usr/share/icons/ArxOS/ 2>/dev/null
[ -d "$_D/assets/applications" ] && $_S cp "$_D"/assets/applications/*.desktop /usr/share/applications/ 2>/dev/null
$_S gtk-update-icon-cache -f /usr/share/icons/ArxOS 2>/dev/null || true
$_S update-desktop-database 2>/dev/null || true
echo "ArxOS branding installed"
