# Deployment

```
1. Robot:   build  ─►  network setup (stable address)  ─►  pair (per Deck)  ─►  boot service
2. Dev box: build Deck tarball
3. Deck:    join the network  ─►  install.sh  ─►  robot add --bundle  ─►  add to Steam + layout
4. Both:    probe  ─►  video tuning  ─►  commissioning checklist (wheels off the ground)
```

Releases are git tags `vX.Y.Z`. Within an interface version, the Deck and the robot can be upgraded independently ([interface.md § Versioning](interface.md#versioning)). The Deck tells you if the robot needs a newer app.

## Development machine

```bash
scripts/setup_dev.sh --check   # report what's missing (no changes)
scripts/setup_dev.sh           # install it: colcon/rosdep, every package.xml dependency, Rust via rustup
```

The script lists no packages itself. ROS and system dependencies come from the `package.xml` files (rosdep keys), Rust crates from `controller/Cargo.toml`/`Cargo.lock`, and the Rust version from `rust-toolchain.toml` (repo root). **Every new dependency goes into one of those files**, never only into a manual install, so a fresh machine (or CI) gets the same setup. Rust is installed user-level in `~/.cargo`, with no sudo. The Deck build compiles `.proto` files with protox, so it doesn't need `protoc`.

## Robot (x86, Ubuntu 24.04 + ROS 2 Jazzy, or Ubuntu 26.04 + ROS 2 Lyrical)

### Build

```bash
# ROS 2 base install first (ros-<distro>-ros-base, see docs.ros.org), then:
sudo apt install python3-colcon-common-extensions python3-rosdep
mkdir -p ~/ros2_ws/src && cd ~/ros2_ws/src
git clone --branch vX.Y.Z <repo-url> ros2_steamdeck_remote_joy
cd ~/ros2_ws && rosdep install --from-paths src --ignore-src -y   # everything else, from package.xml
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
#   Rust for deck_router, user-level; the repo's rust-toolchain.toml picks the version
colcon build --packages-up-to deck_gateway deck_video deck_router
source install/setup.bash
```

`controller/` has a `COLCON_IGNORE` file, so colcon skips it. The robot's own ROS middleware doesn't matter: Fast DDS, Cyclone or `rmw_zenoh` all work, because the gateway uses `rclcpp` for ROS and a separate Zenoh session for the Deck.

### Zenoh package version

`deck_gateway` and `deck_video` need `zenoh_cpp_vendor` built from **Zenoh 1.10.x** (`interface/ZENOH_VERSION`): **Jazzy ≥ 0.2.12, Lyrical ≥ 0.10.7**. (The router is `deck_router`, built from crates.io Zenoh; `rmw_zenoh_cpp` isn't needed for it, see [architecture.md](architecture.md) O17.) Older binaries lack `mixed_rel=1`, and Zenoh ignores the unknown option silently, so control and video would replay stale data after Wi-Fi stalls. Check with:

```bash
grep 'define ZENOH_C "' /opt/ros/$ROS_DISTRO/opt/zenoh_cpp_vendor/include/zenoh_configure.h   # must say 1.10.x (>= 1.10.1)
scripts/setup_dev.sh --check                                                                   # also checks it
```

**Lyrical, until 0.10.7 reaches the main repo:** install the 0.10.7 packages from `ros2-testing` (`rmw-zenoh-cpp` is only needed if ROS itself runs on `rmw_zenoh`). An apt pin can't do this, because `ros2` and `ros2-testing` publish identical release metadata (same origin, label and suite), so apt can't tell them apart. Their dependencies are all in the main repo.

```bash
cd "$(mktemp -d)"
base=http://packages.ros.org/ros2-testing/ubuntu/pool/main/r
wget $base/ros-lyrical-zenoh-cpp-vendor/ros-lyrical-zenoh-cpp-vendor_0.10.7-1resolute.20261024.164443_amd64.deb \
     $base/ros-lyrical-rmw-zenoh-cpp/ros-lyrical-rmw-zenoh-cpp_0.10.7-1resolute.20261024.194644_amd64.deb
sudo apt install ./ros-lyrical-zenoh-cpp-vendor_*.deb ./ros-lyrical-rmw-zenoh-cpp_*.deb
```

When the main repo catches up, normal upgrades take over: it's the same version, and apt never downgrades on its own. If the files have been replaced by a newer testing build, take the current file names from `http://packages.ros.org/ros2-testing/ubuntu/dists/resolute/main/binary-amd64/Packages.gz`.

### Pair each Deck

```bash
ros2 run deck_gateway pair --deck deck1 --host 192.168.1.50 [--robot-name mybot]
# first run: creates the robot CA + server cert (for --host, plus 127.0.0.1/localhost) in ~/.config/deck_gateway/tls/
# every run: issues a client cert with a unique CN (e.g. deck1-3fa9c2), regenerates
#            ~/.config/deck_gateway/router.json5 with the allow list → ~/deck1.bundle.tar (mode 0600)
ros2 run deck_gateway pair --list
ros2 run deck_gateway pair --revoke deck1      # removes all of deck1's CNs from the allow list
```

- **State** lives in `~/.config/deck_gateway/` (`--dir` to change it): `tls/` (CA and server keys, mode 0600), `pairing.json` (robot name, hosts, every certificate issued, revoked ones marked), and the generated `router.json5`. `robot.launch.py` reads it from there by default (`config_dir:=`). **Back this directory up:** losing `tls/ca.key` means re-pairing every Deck.
- **Applying changes:** pairing and revoking restart `deck_router` (`pkill -x deck_router`; the launch file respawns it with the new allow list). Connected Decks reconnect within about a second, and a robot being driven stops. Use `--no-restart` to apply later.
- **New addresses:** a `--host` the server certificate doesn't cover is an error. Add `--reissue-server` to issue a new server certificate with the same CA, so paired Decks keep working.
- **Re-pairing** a Deck issues a new CN. Its old certificate stays allowed until `--revoke`, which removes all of that Deck's CNs.
- **The bundle** holds `ca.pem`, `client.pem`, `client.key` and `robot.toml` (robot name, robot ID = SHA-256 of the CA, QUIC endpoints, the Deck's CN). Copy it to the Deck, then delete it on the robot.

Use the robot's stable name and/or address on the shared network ([§ Network setup](#network-setup)). Add a `--host` for anything else the Deck might use, such as a bench Ethernet address.

This is the **file pairing method**, the only one in the MVP. Online pairing methods are post-MVP ([future.md](future.md#online-enrollment)).

### Run on boot

`deploy/systemd/deck-gateway.service`:

```ini
[Unit]
Description=Steam Deck remote gateway (ROS 2)
After=network-online.target
Wants=network-online.target

[Service]
User=robot
ExecStart=/bin/bash -lc 'source /opt/ros/jazzy/setup.bash && \
  source ~/ros2_ws/install/setup.bash && \
  exec ros2 launch deck_gateway robot.launch.py image_topic:=/camera/image_raw'
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
```

```bash
sudo cp deploy/systemd/deck-gateway.service /etc/systemd/system/   # edit User / image_topic first
sudo systemctl daemon-reload && sudo systemctl enable --now deck-gateway
journalctl -u deck-gateway -f
```

If the robot already has a bring-up launch file, include `robot.launch.py` from it instead. The gateway always starts with STOP latched.

## Network setup

**MVP: the Deck and the robot are on the same network.** Any of these works:

| Option | Notes |
|--------|-------|
| An existing **5 GHz** Wi-Fi network (robot on Wi-Fi or Ethernet) | Simplest if coverage is good where the robot drives |
| A **dedicated 5 GHz router** for the robot(s) | Best reliability: no other traffic |
| The robot's **own hotspot**, joined from SteamOS Wi-Fi settings | Good for one robot in the field: `nmcli device wifi hotspot ifname wlan0 band a ssid mybot-ap password '<passphrase>'`. The robot is then `10.42.0.1`. |

| Do | Why |
|----|-----|
| Use **5 GHz**, on an uncrowded channel | Bandwidth and interference |
| **Wi-Fi power saving off on the robot** (`nmcli connection modify <conn> 802-11-wireless.powersave 2`) **and the Deck** (see below) | Power saving causes periodic 100–300 ms stalls |
| Give the robot a **fixed IP** with a DHCP reservation on the router, and pass it to `pair --host`. Use `mybot.local` only once it's confirmed that SteamOS resolves `.local` names (open item O14). | The Deck's profile and the server certificate both use it |
| Open **UDP 7450** on the robot (plus **TCP 7450** if using the `tls/` endpoint): `sudo ufw allow 7450/udp` | Zenoh endpoint |

**Deck:** join the network in SteamOS Wi-Fi settings as usual. Then Settings → System → enable Developer Mode, and Settings → Developer → turn off **Wi-Fi Power Management**.

*Post-MVP:* one access point per robot with the app managing Wi-Fi (an `ap` setup tool), and an optional Bluetooth backup ([future.md](future.md#connecting-switching-and-pairing-post-mvp)).

## Steam Deck

### Build the tarball (dev machine)

```bash
./controller/packaging/build.sh     # cargo build --release in an Ubuntu 22.04 container
# → dist/steamdeck-remote-joy-vX.Y.Z-x86_64.tar.gz (binary, install.sh, uninstall.sh, .desktop, steam_input/)
```

The build uses an older distro so the binary doesn't require a newer glibc than SteamOS has. It links only system libraries already on SteamOS (GL/EGL, X11/Wayland, udev). The `zenoh` crate version is checked against `interface/ZENOH_VERSION`.

### Install (Deck, Desktop Mode → Konsole)

```bash
tar xf ~/steamdeck-remote-joy-vX.Y.Z-x86_64.tar.gz -C ~ && ~/steamdeck-remote-joy/install.sh
scp robot@192.168.1.50:deck1.bundle.tar ~/
steamdeck-remote-joy robot add --bundle ~/deck1.bundle.tar && rm ~/deck1.bundle.tar
steamdeck-remote-joy probe     # expect: Info (robot name, actions), state STOPPED, RTT a few ms, video ~30 fps
```

Everything lives in `/home`, so SteamOS updates don't remove it.

### Install layout

```
~/.local/share/steamdeck-remote-joy/
├── versions/
│   ├── v0.1.0/steamdeck-remote-joy
│   └── v0.2.0/steamdeck-remote-joy
└── current -> versions/v0.2.0
~/.local/bin/steamdeck-remote-joy -> ~/.local/share/steamdeck-remote-joy/current/steamdeck-remote-joy
~/.local/share/applications/steamdeck-remote-joy.desktop   # Exec=~/.local/bin/steamdeck-remote-joy
~/.config/steamdeck-remote-joy/                            # config.toml + robots/<robot_id>/ profiles (0700)
```

### Stable-path rules: the Steam entry is added once

Steam identifies a Non-Steam Game by its target path and name, and ties the controller layout, artwork and settings to that entry. So `install.sh` must:

1. **Never change the launch path or the name.** Steam always points at `~/.local/bin/steamdeck-remote-joy`, and the entry is always named **Steam Deck Remote Joy**. Version numbers only appear under `versions/`.
2. **Switch versions atomically.** Extract into `versions/vX.Y.Z/`, then repoint `current` with `ln -sfn`. Never `cp` over a binary that might be running ("Text file busy").
3. **Add to Steam only once, using the `.desktop` file.** `steamos-add-to-steam` rejects a bare script or binary when run over SSH ("Unsupported file type", because `kmimetypefinder` can't classify it there), but a `.desktop` entry always works (verified in M0). Run it only if no shortcut for the launch path exists yet (search for it in `~/.steam/steam/userdata/*/config/shortcuts.vdf`). Otherwise you get a duplicate entry with no layout.
4. **Keep the previous two versions** for rollback.
5. **Be safe to re-run.**

*Future: Flatpak packaging ([future.md](future.md#flatpak-packaging)).*

### Add to Steam

1. If it wasn't added automatically: Steam → *Add a Non-Steam Game* → **Steam Deck Remote Joy**.
2. *Controller Settings* → use the template from `steam_input/`. Or use the default **Gamepad** template (L2 is the deadman). Optionally map a back grip (L4) to L2 as well.
3. Go back to Game Mode and launch it. You should see the camera and a **STOPPED** banner.

*To find out:* whether the Steam Input layout can be installed from a file. Until then, step 2 is manual.

## Video tuning

1. Open the **debug overlay** (tap the top bar) and drive around the area you'll actually use. Check *frames received* (aim for > 95%) and the latency of each stage.
2. **Glass-to-glass:** point the robot's camera at a millisecond stopwatch and take one photo that shows both the stopwatch and the Deck screen.
3. If capture → encoded is high, or glass-to-glass is much higher than the overlay's capture → shown, reduce the camera driver's buffer count ([robot.md](robot.md#camera-driver-settings)).
4. If frame delivery is low, check the Wi-Fi setup first, then lower `jpeg_quality` or `max_width`.
5. Write down the results. If they're still not good enough, see [future.md § Video](future.md#video).

## Logs

- **Robot:** `journalctl -u deck-gateway` (safety events at INFO), and `/diagnostics` → `deck_gateway: link` (`ros2 run rqt_robot_monitor rqt_robot_monitor`).
- **Deck:** `steamdeck-remote-joy log --tail 50`.

Both carry the session ID, so they can be lined up after an incident.

## Commissioning checklist

Do this with the drive wheels **off the ground** the first time, and after every upgrade.

- [ ] `probe` shows the robot's name and actions. A Deck that isn't paired (or whose bundle has been revoked) can't connect.
- [ ] The camera shows on the Deck, live.
- [ ] After the gateway starts, the Deck shows STOPPED (`startup`). View+Menu (1 s) clears it only with sticks centered and L2 released. Otherwise the Deck shows the reason it was refused.
- [ ] Sticks without L2 → no motion. LB or RB alone → no motion.
- [ ] L2 plus left stick forward → drives forward. L2 + RB → turbo. Right stick right → turns right. If not, fix the signs in `teleop_twist_joy.yaml`.
- [ ] Opening the panel (Menu) while driving → the robot stops (neutral input).
- [ ] **Watchdog:** while driving, put the Deck in airplane mode → stops within about 0.25 s.
- [ ] **Deck sleep:** press the power button while driving → stops. On wake, the app reconnects and the robot is STOPPED.
- [ ] **App quit:** while driving, quit from Steam → stops.
- [ ] **Gateway stop:** `pkill -f deck_gateway` while driving → the robot's base `cmd_vel` timeout stops it. *If it doesn't, fix the base before going any further.*
- [ ] **Router stop:** kill the router while driving → stops (watchdog), and the Deck shows LINK LOST.
- [ ] **Video loss:** kill `deck_video` → the Deck shows NO VIDEO, and driving still works.
- [ ] B and the touchscreen STOP both latch on every screen, including with the panel open.
- [ ] A second paired Deck shows BUSY while the first one is driving.
- [ ] **Handover:** quit the app on the first Deck → the robot stops at once. The second Deck can take control immediately and starts STOPPED (`new_owner`).
- [ ] **Short drop:** briefly block traffic (e.g. `sudo iptables -I INPUT -p udp --dport 7450 -j DROP` on the robot for about 0.5 s, then `-D`) → the robot stops, the Deck shows REARM, and driving resumes only after releasing and pressing LB.
- [ ] **Long drop:** put the Deck in airplane mode for 3 s while driving → the robot stops, and after reconnecting the Deck shows STOPPED (`link_lost`).
- [ ] **STOP is teleop-only:** confirm that's acceptable for this robot. If other `cmd_vel` sources must also halt, wire `/steamdeck/stop_latched` into them (e.g. a `twist_mux` lock), then test that.
- [ ] **Action gating:** while STOPPED, a `moves_robot` action (e.g. Dock) is greyed out and refused. On a second Deck that isn't in control, all actions are refused.
- [ ] **Base limits:** with teleop scales temporarily set high, the robot still doesn't exceed the base's speed limit.
- [ ] Each configured action works, and ones marked `confirm` ask first.
- [ ] Button hints match what each free button actually does. Those buttons do nothing while STOPPED, REARM or with the panel open, and the hint line is dimmed then.
- [ ] No robot node is bound to a reserved button (B, View, Menu, L2/LT, RB). Check the gateway's startup warnings.
- [ ] The hardware E-stop works independently.

## Upgrade

```bash
# Robot
cd ~/ros2_ws/src/ros2_steamdeck_remote_joy && git fetch && git checkout vX.Y.Z
cd ~/ros2_ws && colcon build --packages-up-to deck_gateway deck_video deck_router && sudo systemctl restart deck-gateway

# Deck: build the tarball on the dev machine, then:
tar xf ~/steamdeck-remote-joy-vX.Y.Z-x86_64.tar.gz -C ~ && ~/steamdeck-remote-joy/install.sh
```

The Steam entry, controller layout and pairing stay as they are, so there's nothing to re-add.

**Roll back on the Deck:** `~/steamdeck-remote-joy/install.sh --rollback`. By hand: `ln -sfn versions/v0.1.0 ~/.local/share/steamdeck-remote-joy/current`.

Run the checklist again after any upgrade.

## Uninstall

- **Deck:** `~/steamdeck-remote-joy/uninstall.sh` removes `versions/`, the links, and the `.desktop` file. Add `--purge` to also delete the pairing and config. Then remove the game from Steam.
- **Robot:** `sudo systemctl disable --now deck-gateway`, then delete the unit file and the packages. Delete `~/.config/deck_gateway/tls/` to remove the CA and all pairings.
