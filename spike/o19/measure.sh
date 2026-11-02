#!/usr/bin/env bash
# O19: one measurement run from the Deck against the real gateway (deck_sim or a robot).
#   spike/o19/measure.sh <label> [--no-video] [seconds=20]
# Runs deck_spike --v1 headless on the Deck under systemd-run (survives SSH), then prints:
#   label  rtt_avg(median)  rtt_p95(max)  rtt_max  lost  stale  down_s  video_kbps
set -euo pipefail
label=$1; shift
novideo=""; [[ ${1:-} == --no-video ]] && { novideo=--no-video; shift; }
secs=${1:-20}; shift || true
# ZCFG=<local file>: Zenoh config overrides (KEY=JSON5 per line), copied to the Deck.
DECK=${DECK:-deck@steamdeck}; ROBOT=${ROBOT:-192.168.1.131}
log=o19-$label.log
extra=""
if [[ -n ${ZCFG:-} ]]; then
  scp -q "$ZCFG" "$DECK:rj-spike/logs/o19-$label.zcfg"; extra="--zcfg-file logs/o19-$label.zcfg"
fi
ssh "$DECK" "cd ~/rj-spike && systemctl --user reset-failed o19 2>/dev/null; rm -f logs/$log; \
  systemd-run --user --quiet --unit=o19 --working-directory=/home/deck/rj-spike sh -c \
  'exec ./deck_spike --robot $ROBOT --certs certs-new --headless --duration-s $secs --v1 $novideo $extra > logs/$log 2>&1'"
sleep $((secs + 3))
ssh "$DECK" "cat ~/rj-spike/logs/$log" | python3 "$(dirname "$0")/summarize.py" "$label"
