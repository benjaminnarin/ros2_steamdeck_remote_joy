# M0 spike

Throwaway code that answers the open items the design can't settle on paper
([docs/architecture.md § M0 spike](../docs/architecture.md#m0-spike)). It is **not** the product:
messages are tiny fixed structs instead of Protobuf, and there's no ROS.

| Binary | Runs on | Does |
|--------|---------|------|
| `robot_spike` | This machine (the robot's computer for initial testing) | Zenoh router on `quic/0.0.0.0:7450` with mTLS. Echoes every control message, streams synthetic JPEG (480×270, 30 fps, ~17 KB/frame) |
| `deck_spike` | Steam Deck | egui app: live gilrs gamepad view (O1), fullscreen + touch test (O2), 50 Hz control with RTT, stall and stale-burst detection (O3, O15), video with fps, delivery and latency. `--headless` prints the same stats once per second. |

Both use Zenoh **1.10.1**, the version ROS 2 Jazzy and Lyrical vendor from `zenoh_cpp_vendor` 0.2.12 / 0.10.7 on (see [deployment.md § Zenoh package version](../docs/deployment.md#zenoh-package-version)).

## Build and deploy (from this machine)

```bash
cd spike
scripts/gen_certs.sh 192.168.1.131          # throwaway CA + robot/deck certs → certs/ (gitignored)
cargo build --release                       # needs ~/.cargo/bin on PATH
ADD_TO_STEAM=1 scripts/deploy_deck.sh deck@steamdeck 192.168.1.131
#   copies deck_spike, run.sh, rj-spike.desktop and certs to ~/rj-spike on the Deck
#   ADD_TO_STEAM=1 adds "RJ Spike" as a Non-Steam Game (only if not already there)
```

The Deck has the same glibc (2.43) as this machine, so the binary built here runs there unchanged.

## Run

```bash
# Robot side (this machine)
./target/release/robot_spike --certs certs
#   options: --reliable (echo/video), --fps 0 (no video), --quality, --noise, --lease-ms

# Deck, headless over SSH (quick link check)
ssh deck@steamdeck '~/rj-spike/deck_spike --robot 192.168.1.131 --certs ~/rj-spike/certs --headless --duration-s 30'

# Deck, GUI: Game Mode → Library → Non-Steam → "RJ Spike"
#   logs: ~/rj-spike/logs/deck_spike-YYYYMMDD.log on the Deck
```

Useful variants:
- `--reliable`: send control as reliable, for comparison in the stall test.
- **`mixed_rel=1` is required** for correct stall behaviour. Both binaries now default to it (robot listens on `quic/0.0.0.0:7450?mixed_rel=1`, the Deck connects to `quic/<robot>:7450?mixed_rel=1`). Pass `--listen`/`--connect` without it to reproduce the stale burst.
- Extra Deck arguments for the Steam launch go in `~/rj-spike/args` (one line, read by `run.sh`).
- `--no-tls` with `--connect udp/...` (the robot side then needs `--no-tls --listen udp/0.0.0.0:7450`): plain baseline.
- `--lease-ms 10000`: Zenoh's default lease, to compare reconnect times (O15).

## End-to-end demo (turtlesim)

Deck → Zenoh → `demo/demo_bridge.py` → `/joy` → `teleop_twist_joy` → turtlesim, plus webcam video back to the Deck. The bridge is a Python stand-in for `deck_gateway` and `deck_video` (watchdog, `joy` axis conventions, echo for RTT). It replaces `robot_spike` on port 7450, so stop that first.

```bash
# once: venv that can also see ROS's rclpy and the system's cv2 (this Python has no pip, so bootstrap it)
python3 -m venv --system-site-packages --without-pip .venv
curl -sSf https://bootstrap.pypa.io/get-pip.py | .venv/bin/python
.venv/bin/python -m pip install eclipse-zenoh==1.10.1

demo/demo.sh                # starts turtlesim, teleop_twist_joy and the bridge; Ctrl+C stops all
demo/demo.sh --camera -1    # no webcam
```

On the Deck, launch RJ Spike, **hold LB** and use the left stick (forward/back) and right stick (turn). RB is turbo. Releasing LB stops the turtle.

## Checks

| # | Check | How | Pass if |
|---|-------|-----|---------|
| O1 | gilrs sees the Steam Input gamepad **when launched from Steam** | Launch "RJ Spike" in Game Mode, move every stick and press every button. Check the Gamepad panel. | One pad, all axes and buttons live, sensible signs. Note what L4/R4 map to with the default layout. |
| O2 | egui under gamescope | Same launch | Fullscreen at 1280×800, smooth video, TAP ME counts touches, Quit works |
| O3 | No stale burst after a Wi-Fi stall | `sudo scripts/stall.sh 1000` (also 1500, 3000) on the robot computer drops UDP 7450 both ways. Walking out of range works too. | Events show `OK, no stale burst` with best effort. Reliable is expected to show `STALE BURST`, which is the comparison. |
| O15 | 2 s lease: drops detected and recovered quickly | Airplane mode for 5 s, then off. Repeat with `--lease-ms 10000`. | Reconnects within about 3 s of Wi-Fi returning, and no false LINK LOST during normal use |
| Video | 480p JPEG at 30 fps over Zenoh | Drive around the real area | > 95% delivered, capture→received latency in the tens of ms |

## Results (2026-10-31)

Setup: the robot computer (this machine) and the Deck both on 5 GHz Wi-Fi (channel 108, −56 to −62 dBm) through the same access point. Wi-Fi power saving was **on** at both ends.

| Check | Result |
|-------|--------|
| Build | ✅ zenoh 1.10.1 (`unstable` for per-publisher reliability), eframe 0.37, gilrs 0.11, zune-jpeg 0.4. A binary built on this machine runs on the Deck unchanged (both glibc 2.43). |
| **O1** gamepad | ✅ gilrs sees exactly **one** pad, `"Steam Virtual Gamepad"` (`Microsoft X-Box 360 pad 0`, `SdlMappings`). All 15 buttons map to the SDL/`joy` bits. Triggers are analog 0 → 1. Sticks: **up = +, right = +**, so the product negates X. **L4/R4/L5/R5 send nothing** with the default layout. Resting stick noise is up to ±0.08, so the deadzone should be 0.12. |
| **O2** egui in Game Mode | ✅ Fullscreen 1280×800, touch (3/3 taps), smooth 30 fps video |
| Link, steady state | ✅ RTT about 2.5 / 5 / 7 ms (min/avg/p95), 50 echoes/s, 0 lost. Video 30 fps, 100% delivered, 17 KB/frame (4.2 Mbps), about 20 ms capture → received |
| **O3** stalls, default QUIC | ❌ A 1 s stall caused a **stale burst**: 73 echoes delivered late, up to **1.64 s** old, 0 lost. Video was briefly 0.3–1 s behind. Adding a second endpoint with `?rel=0` changed nothing. |
| **O3** stalls, `?mixed_rel=1` on both sides | ✅ 1.0 / 1.5 / 3 s stalls: **0 stale**. Control during the stall is dropped (newest wins). A 1.5 s stall recovers without reconnecting. Residual backlog ≤ about 170 ms from Zenoh's send queue. |
| **O15** reconnect | ✅ With a 2 s lease and retry 200 ms → 1 s, Zenoh is back about **0.8 s** after Wi-Fi returns. Airplane mode for 5 s: about 5.5 s from radio-on to CONNECTED, of which 3.8 s is SteamOS scanning and 0.9 s is association/DHCP. |
| Natural Wi-Fi spikes | ⚠️ A few 300–500 ms delay spikes with **no loss**: the radio holds packets, likely power saving. The design adds a late-control guard in the gateway; re-measure with power saving off. |
| `steamos-add-to-steam` | ✅ Works over SSH with a `.desktop` file. A bare `.sh` is rejected ("Unsupported file type"). |
| ROS on the robot computer | This machine runs **ROS 2 Lyrical** (Ubuntu 26.04). Lyrical's `zenoh_cpp_vendor` source pins the **same Zenoh 1.10.1** with the unstable API. *Correction (2026-11-01):* only from release 0.10.7 on; the 0.10.6 binary in the main apt repo is Zenoh 1.8.0 without `mixed_rel`. |
| Not done | Video at the edge of range. `pair` → bundle flow (needs the real gateway). |

Endpoint used for the passing runs: robot `--listen 'quic/0.0.0.0:7450?mixed_rel=1'`, and on the Deck `~/rj-spike/args` = `--connect quic/192.168.1.131:7450?mixed_rel=1`.
