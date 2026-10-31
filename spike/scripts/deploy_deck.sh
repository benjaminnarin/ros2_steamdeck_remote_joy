#!/usr/bin/env bash
# Build deck_spike here and copy it to the Steam Deck.
# Usage: scripts/deploy_deck.sh [DECK_SSH] [ROBOT_IP]
#   defaults: deck@steamdeck 192.168.1.131
# On the Deck: ~/rj-spike/{deck_spike,run.sh,certs/,robot_ip}. Logs go to ~/rj-spike/logs/.
set -euo pipefail

DECK=${1:-deck@steamdeck}
ROBOT_IP=${2:-192.168.1.131}
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

[ -f certs/deck.pem ] || scripts/gen_certs.sh "$ROBOT_IP"
cargo build --release -p deck_spike

ssh "$DECK" 'mkdir -p ~/rj-spike/certs ~/rj-spike/logs'
# Copy to a temp name, then rename: replacing a running binary in place fails.
scp -q target/release/deck_spike "$DECK:rj-spike/deck_spike.new"
scp -q scripts/run.sh scripts/rj-spike.desktop "$DECK:rj-spike/"
scp -q certs/ca.pem certs/deck.pem certs/deck.key "$DECK:rj-spike/certs/"
ssh "$DECK" "cd ~/rj-spike && mv deck_spike.new deck_spike && chmod +x deck_spike run.sh \
  && chmod 600 certs/deck.key && echo '$ROBOT_IP' > robot_ip && ./deck_spike --version"
echo "deployed to $DECK:~/rj-spike (robot $ROBOT_IP)"
if [ "${ADD_TO_STEAM:-0}" = 1 ]; then
  # Adds a Non-Steam Game shortcut (once); Steam must be running on the Deck.
  ssh "$DECK" 'grep -qs rj-spike ~/.steam/steam/userdata/*/config/shortcuts.vdf || steamos-add-to-steam ~/rj-spike/rj-spike.desktop'
fi
