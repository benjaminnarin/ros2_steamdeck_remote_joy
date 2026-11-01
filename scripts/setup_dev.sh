#!/usr/bin/env bash
# Set up (or check) a development machine for this repo: the robot-side ROS packages
# and the Deck app's Rust toolchain.
#
#   scripts/setup_dev.sh           install what's missing (asks for sudo for apt/rosdep)
#   scripts/setup_dev.sh --check   only report what's missing; exit 1 if anything is
#
# Dependencies are declared, not listed here:
#   ROS/system packages  -> package.xml files under interface/ and robot/ (rosdep keys)
#   Rust crates          -> controller/Cargo.toml + Cargo.lock
#   Rust version         -> controller/rust-toolchain.toml
# Add new dependencies there; this script picks them up.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
check=false
case "${1:-}" in
  --check) check=true ;;
  "") ;;
  -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
esac

missing=()
step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
ok() { printf '  ok      %s\n' "$*"; }
need() { printf '  MISSING %s\n' "$*"; missing+=("$1"); }

# ── ROS 2 ───────────────────────────────────────────────────
step "ROS 2"
distro=${ROS_DISTRO:-}
if [[ -z $distro ]]; then
  for d in lyrical jazzy; do
    [[ -f /opt/ros/$d/setup.bash ]] && { distro=$d; break; }
  done
fi
if [[ -z $distro || ! -f /opt/ros/$distro/setup.bash ]]; then
  need "ROS 2 (Jazzy on Ubuntu 24.04 or Lyrical on Ubuntu 26.04): install ros-<distro>-ros-base, see docs.ros.org"
  echo; echo "Install ROS 2 first, then rerun." >&2
  exit 1
fi
ok "ROS 2 $distro"
set +u  # ROS setup scripts reference unset variables
# shellcheck disable=SC1090
source "/opt/ros/$distro/setup.bash"
set -u

# Build tools that no package.xml declares.
tools=()
command -v colcon >/dev/null || tools+=(python3-colcon-common-extensions)
command -v rosdep >/dev/null || tools+=(python3-rosdep)
if ((${#tools[@]})); then
  if $check; then
    for t in "${tools[@]}"; do need "$t (apt)"; done
  else
    echo "  installing ${tools[*]}"
    sudo apt-get install -y "${tools[@]}"
  fi
else
  ok "colcon, rosdep"
fi

# ── rosdep: everything declared in package.xml ──────────────
step "Package dependencies (rosdep, from package.xml)"
if command -v rosdep >/dev/null; then
  if [[ ! -e /etc/ros/rosdep/sources.list.d/20-default.list ]]; then
    if $check; then need "rosdep init (run: sudo rosdep init && rosdep update)"; else sudo rosdep init; fi
  fi
  if [[ -e /etc/ros/rosdep/sources.list.d/20-default.list ]]; then
    if ! $check; then
      rosdep update --rosdistro "$distro" >/dev/null
    elif [[ ! -d ~/.ros/rosdep/sources.cache ]]; then
      need "rosdep cache (run: rosdep update)"
    fi
    paths=("$repo/interface" "$repo/robot")
    if $check; then
      if out=$(rosdep check --rosdistro "$distro" --from-paths "${paths[@]}" --ignore-src 2>&1); then
        ok "all package.xml dependencies installed"
      else
        while read -r line; do
          [[ $line =~ ^(apt|pip)[[:space:]]+(.+)$ ]] && need "${BASH_REMATCH[2]} (${BASH_REMATCH[1]}, via rosdep)"
        done <<<"$out"
        ((${#missing[@]})) || need "rosdep check failed: $out"
      fi
    else
      rosdep install --rosdistro "$distro" --from-paths "${paths[@]}" --ignore-src -y
      ok "package.xml dependencies installed"
    fi
  fi
fi

# ── Zenoh version (the vendored zenoh-c must match interface/ZENOH_VERSION) ──
step "Zenoh (zenoh_cpp_vendor)"
want=$(head -1 "$repo/interface/ZENOH_VERSION")
hdr=/opt/ros/$distro/opt/zenoh_cpp_vendor/include/zenoh_configure.h
if [[ -f $hdr ]]; then
  have=$(sed -n 's/^#define ZENOH_C "\(.*\)"/\1/p' "$hdr")
  if [[ $have == "$want" ]]; then
    ok "zenoh-c $have"
  else
    # Not installable by rosdep alone: see docs/deployment.md § Zenoh package version.
    need "zenoh-c $want (installed: $have; Lyrical needs zenoh_cpp_vendor >= 0.10.7, see docs/deployment.md § Zenoh package version)"
  fi
else
  echo "  (not installed yet: rosdep installs it once a package depends on zenoh_cpp_vendor; it must be zenoh-c $want)"
fi

# ── Rust (Deck app) ─────────────────────────────────────────
step "Rust (controller/)"
export PATH="$HOME/.cargo/bin:$PATH"
pinned=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$repo/controller/rust-toolchain.toml")
if ! command -v rustup >/dev/null; then
  if $check; then
    need "rustup (user-level, no sudo: https://rustup.rs)"
  else
    echo "  installing rustup to ~/.cargo (user-level, PATH not modified)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
      sh -s -- -y --no-modify-path --profile minimal --default-toolchain none
  fi
fi
if command -v rustup >/dev/null; then
  if rustup toolchain list | grep -q "^$pinned"; then
    ok "Rust $pinned (from rust-toolchain.toml)"
  elif $check; then
    need "Rust toolchain $pinned (rustup installs it on the first cargo command in controller/)"
  else
    (cd "$repo/controller" && rustup toolchain install)  # reads rust-toolchain.toml
    ok "Rust $pinned installed"
  fi
  grep -qs '.cargo/bin' ~/.profile ~/.bashrc ~/.zshrc 2>/dev/null ||
    echo "  note: ~/.cargo/bin isn't on your shell's PATH; add: export PATH=\"\$HOME/.cargo/bin:\$PATH\""
fi
# The Deck build compiles .proto files with protox (pure Rust): no protoc needed there.

# ── Summary ─────────────────────────────────────────────────
step "Summary"
if ((${#missing[@]})); then
  echo "  ${#missing[@]} missing. Run scripts/setup_dev.sh (without --check) to install."
  exit 1
fi
cat <<EOF
  Everything is installed. Build and test from the repo root:
    source /opt/ros/$distro/setup.bash
    colcon build --symlink-install && colcon test && colcon test-result --verbose
    (cd controller && cargo test)
EOF
