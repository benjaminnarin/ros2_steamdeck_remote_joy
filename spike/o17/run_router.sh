#!/usr/bin/env bash
# O17: run the robot's router with the product config (robot/deck_gateway/config/router.json5),
# passing the TLS files through ZENOH_CONFIG_OVERRIDE the way the launch file will.
#   spike/o17/run_router.sh [certs-dir]        deck_router (default; build with colcon first)
#   ROUTER=rmw spike/o17/run_router.sh         rmw_zenohd instead (crashes under Wi-Fi load, quinn #2805)
#   EXTRA_OVERRIDE='access_control/enabled=false;low_pass_filter=[]'   appended to the override
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
certs=$(cd "${1:-$repo/spike/certs}" && pwd)
distro=${ROS_DISTRO:-lyrical}
set +u; source "/opt/ros/$distro/setup.bash"; [[ -f $repo/install/setup.bash ]] && source "$repo/install/setup.bash"; set -u

export ZENOH_ROUTER_CONFIG_URI=$repo/robot/deck_gateway/config/router.json5
export ZENOH_CONFIG_OVERRIDE="transport/link/tls/root_ca_certificate=\"$certs/ca.pem\";transport/link/tls/listen_certificate=\"$certs/robot.pem\";transport/link/tls/listen_private_key=\"$certs/robot.key\"${EXTRA_OVERRIDE:+;$EXTRA_OVERRIDE}"
export RUST_LOG=${RUST_LOG:-info}
echo "router ${ROUTER:-deck}: config $ZENOH_ROUTER_CONFIG_URI, certs $certs" >&2
case ${ROUTER:-deck} in
  deck) exec ros2 run deck_router deck_router ;;
  rmw)  exec ros2 run rmw_zenoh_cpp rmw_zenohd ;;
  *) echo "ROUTER must be deck or rmw" >&2; exit 2 ;;
esac
