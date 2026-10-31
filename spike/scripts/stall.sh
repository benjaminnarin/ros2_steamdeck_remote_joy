#!/usr/bin/env bash
# Simulate a Wi-Fi stall on the robot: drop all spike traffic in both directions
# for MS milliseconds, then restore it. Run on the robot computer.
# Usage: sudo scripts/stall.sh [MS=1500] [PORT=7450]
set -euo pipefail

MS=${1:-1500}
PORT=${2:-7450}
[ "$(id -u)" = 0 ] || { echo "run with sudo" >&2; exit 1; }

RULES=("INPUT -p udp --dport $PORT -j DROP" "OUTPUT -p udp --sport $PORT -j DROP")
restore() {
  for r in "${RULES[@]}"; do
    # shellcheck disable=SC2086
    while iptables -D $r 2>/dev/null; do :; done
  done
}
trap restore EXIT

for r in "${RULES[@]}"; do
  # shellcheck disable=SC2086
  iptables -I $r
done
echo "$(date +%T.%3N) stall: dropping UDP $PORT both ways for $MS ms"
sleep "$(awk "BEGIN { print $MS / 1000 }")"
restore
trap - EXIT
echo "$(date +%T.%3N) stall: restored"
