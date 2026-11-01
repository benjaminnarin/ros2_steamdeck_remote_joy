#!/usr/bin/env bash
# End-to-end demo on the robot computer:
#   Deck (deck_spike) -> Zenoh -> demo_bridge -> /joy -> teleop_twist_joy -> turtlesim
#   webcam -> demo_bridge -> Zenoh -> Deck screen
# Usage: spike/demo/demo.sh [extra demo_bridge args, e.g. --camera -1]
# Ctrl+C stops everything.
set -eo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SPIKE="$(dirname "$HERE")"
# shellcheck disable=SC1091
source /opt/ros/lyrical/setup.bash
[ -x "$SPIKE/.venv/bin/python" ] || { echo "missing $SPIKE/.venv (see demo/README.md)" >&2; exit 1; }

pids=()
cleanup() { kill "${pids[@]}" 2>/dev/null || true; wait 2>/dev/null || true; }
trap cleanup EXIT INT TERM

ros2 run turtlesim turtlesim_node &
pids+=($!)
ros2 run teleop_twist_joy teleop_node --ros-args \
  --params-file "$HERE/teleop_turtle.yaml" -r cmd_vel:=/turtle1/cmd_vel &
pids+=($!)
"$SPIKE/.venv/bin/python" "$HERE/demo_bridge.py" "$@" &
pids+=($!)

echo "demo running: hold LB and move the left stick on the Deck. Ctrl+C to stop."
wait -n
