#!/usr/bin/env bash
# Launcher on the Deck. Add this file to Steam as a Non-Steam Game and launch it
# from Game Mode. Extra arguments come from ~/rj-spike/args (one line, optional)
# and from the command line, e.g. ./run.sh --headless
cd "$(dirname "$0")" || exit 1
mkdir -p logs
# shellcheck disable=SC2046
exec ./deck_spike --robot "$(cat robot_ip)" $(cat args 2>/dev/null) "$@" >>"logs/deck_spike-$(date +%Y%m%d).log" 2>&1
