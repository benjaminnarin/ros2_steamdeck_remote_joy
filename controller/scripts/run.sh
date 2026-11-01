#!/usr/bin/env bash
# Steam launches this (via steamdeck-remote-joy.desktop). Extra arguments go in ./args.
cd "$(dirname "$0")"
mkdir -p logs
# shellcheck disable=SC2046
exec ./steamdeck-remote-joy $(cat args 2>/dev/null) "$@" >>"logs/app-$(date +%Y%m%d).log" 2>&1
