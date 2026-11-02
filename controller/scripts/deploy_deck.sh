#!/usr/bin/env bash
# Development deploy (packaging proper is M5): build here, copy to ~/steamdeck-remote-joy on the Deck.
#   scripts/deploy_deck.sh [deck@steamdeck]
#   BUNDLE=~/deck1.bundle.tar scripts/deploy_deck.sh     also install a pairing bundle (robot add)
#   ADD_TO_STEAM=1 scripts/deploy_deck.sh                add the Non-Steam Game entry if missing
# Every deploy also installs the Library artwork (assets/steam, scripts/steam_art.py) if the
# entry exists. Steam shows it after a restart; the icon is set only while Steam is closed.
# SSH: one shared connection (10 min); a login URL from Tailscale is shown up front, and the
# script gives up after SSH_WAIT seconds (default 120) instead of hanging.
# The Deck runs the same glibc as this machine today (2.43); see docs/architecture.md O18.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
DECK=${1:-deck@steamdeck}
DIR=steamdeck-remote-joy
export PATH="$HOME/.cargo/bin:$PATH"
(cd "$here" && cargo build --release -p deck_app)

# One shared SSH connection for every step below, kept open 10 min after the deploy, so an
# interactive check (Tailscale SSH "check" mode prints a login URL and waits) happens at most
# once, up front, and the next deploys reuse it.
SSH_OPTS=(-o ControlMaster=auto -o "ControlPath=$HOME/.ssh/cm-deploy-%C" -o ControlPersist=10m
          -o ConnectTimeout=10)
ssh() { command ssh "${SSH_OPTS[@]}" "$@"; }
scp() { command scp "${SSH_OPTS[@]}" "$@"; }
SSH_WAIT=${SSH_WAIT:-120}
echo "connecting to $DECK (if Tailscale prints a login URL, open it to approve; waiting up to ${SSH_WAIT}s)..."
if ! timeout "$SSH_WAIT" ssh "$DECK" true; then
  echo "deploy_deck: no SSH to $DECK within ${SSH_WAIT}s. Approve the Tailscale URL above, or wake the" >&2
  echo "Deck and check that it's on the network, then run this again." >&2
  exit 2
fi
echo "connected"
ssh "$DECK" "mkdir -p ~/$DIR/logs ~/$DIR/art"
scp -q "$here/target/release/steamdeck-remote-joy" "$DECK:$DIR/steamdeck-remote-joy.new"
scp -q "$here/scripts/run.sh" "$here/scripts/steamdeck-remote-joy.desktop" "$here/scripts/steam_art.py" "$DECK:$DIR/"
scp -q "$here"/assets/steam/*.png "$DECK:$DIR/art/"
ssh "$DECK" "cd ~/$DIR && chmod +x run.sh steamdeck-remote-joy.new && mv steamdeck-remote-joy.new steamdeck-remote-joy"
if [[ -n ${BUNDLE:-} ]]; then
  scp -q "$BUNDLE" "$DECK:$DIR/bundle.tar"
  ssh "$DECK" "cd ~/$DIR && ./steamdeck-remote-joy robot add --bundle bundle.tar; rm -f bundle.tar"
fi
if [[ ${ADD_TO_STEAM:-0} == 1 ]]; then
  ssh "$DECK" "grep -qs 'Steam Deck Remote Joy' ~/.local/share/Steam/userdata/*/config/shortcuts.vdf \
    || steamos-add-to-steam ~/$DIR/steamdeck-remote-joy.desktop"
fi
ssh "$DECK" "python3 ~/$DIR/steam_art.py ~/$DIR/art"
echo "deployed to $DECK:~/$DIR ($(ssh "$DECK" "~/$DIR/steamdeck-remote-joy --version"))"
