#!/usr/bin/env bash
# Capture DDR memory type + speed once at boot into a world-readable cache, so the Control
# Center (running as the desktop user) can show it without root. SMBIOS/dmidecode needs root;
# the GUI does not, so we resolve it here and cache it. On a VM the SMBIOS often reports no DDR
# type — we then fall back to a neutral label rather than showing nothing.
set -u
mkdir -p /run/arxos
t=""; s=""
if command -v dmidecode >/dev/null 2>&1; then
  info="$(dmidecode -t 17 2>/dev/null)"
  # first populated module's Type / Speed (exact "Type:"/"Speed:", not "Type Detail:" etc.)
  t="$(printf '%s\n' "$info" | grep -m1 -E '^[[:space:]]+Type:[[:space:]]' | sed 's/.*:[[:space:]]*//')"
  s="$(printf '%s\n' "$info" | grep -m1 -E '^[[:space:]]+Speed:[[:space:]]' | sed 's/.*:[[:space:]]*//')"
fi
case "$t" in Unknown|Other|"") t="" ;; esac
case "$s" in Unknown|"") s="" ;; esac
{
  echo "MEMTYPE=${t}"
  echo "MEMSPEED=${s}"
} > /run/arxos/hwinfo
chmod 644 /run/arxos/hwinfo
