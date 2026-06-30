#!/usr/bin/env bash
D=$(cd "$(dirname "$0")" && pwd); S=""; [ "$(id -u)" -ne 0 ] && S=sudo
$S install -Dm755 "$D/arxctl-gui" /usr/local/bin/arxctl-gui
[ -f "$D/arxctl" ] && $S install -Dm755 "$D/arxctl" /usr/local/bin/arxctl
$S install -Dm755 "$D/arxos-news" /usr/local/bin/arxos-news
$S install -Dm644 "$D/arxos-news.service" /usr/lib/systemd/user/arxos-news.service
$S install -Dm644 "$D/arxos-news.timer"   /usr/lib/systemd/user/arxos-news.timer
$S systemctl --global enable arxos-news.timer 2>/dev/null || true
echo "arxctl + arxos-news installed (news timer enabled for all users)"
