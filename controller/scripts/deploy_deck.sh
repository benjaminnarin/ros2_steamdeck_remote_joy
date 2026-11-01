#!/usr/bin/env bash
# Development deploy (packaging proper is M5): build here, copy to ~/steamdeck-remote-joy on the Deck.
#   scripts/deploy_deck.sh [deck@steamdeck]
#   BUNDLE=~/deck1.bundle.tar scripts/deploy_deck.sh     also install a pairing bundle (robot add)
#   ADD_TO_STEAM=1 scripts/deploy_deck.sh                add the Non-Steam Game entry if missing
# The Deck runs the same glibc as this machine today (2.43); see docs/architecture.md O18.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
DECK=${1:-deck@steamdeck}
DIR=steamdeck-remote-joy
export PATH="$HOME/.cargo/bin:$PATH"
(cd "$here" && cargo build --release -p deck_app)
ssh "$DECK" "mkdir -p ~/$DIR/logs"
scp -q "$here/target/release/steamdeck-remote-joy" "$DECK:$DIR/steamdeck-remote-joy.new"
scp -q "$here/scripts/run.sh" "$here/scripts/steamdeck-remote-joy.desktop" "$DECK:$DIR/"
ssh "$DECK" "cd ~/$DIR && chmod +x run.sh steamdeck-remote-joy.new && mv steamdeck-remote-joy.new steamdeck-remote-joy"
if [[ -n ${BUNDLE:-} ]]; then
  scp -q "$BUNDLE" "$DECK:$DIR/bundle.tar"
  ssh "$DECK" "cd ~/$DIR && ./steamdeck-remote-joy robot add --bundle bundle.tar; rm -f bundle.tar"
fi
if [[ ${ADD_TO_STEAM:-0} == 1 ]]; then
  ssh "$DECK" "grep -qs 'Steam Deck Remote Joy' ~/.local/share/Steam/userdata/*/config/shortcuts.vdf \
    || steamos-add-to-steam ~/$DIR/steamdeck-remote-joy.desktop"
fi
echo "deployed to $DECK:~/$DIR ($(ssh "$DECK" "~/$DIR/steamdeck-remote-joy --version"))"
