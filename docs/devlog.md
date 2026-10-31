# Development log

A running record of how the project was designed, built and tested, kept for a write-up later. Newest entries go at the bottom of each phase. Each entry says **what was decided or done, why, and what evidence supported it.** The design docs say what the system *is*. This log says how we got there, including dead ends.

**Setup:**
- **Steam Deck:** SteamOS 3.9.5, glibc 2.43, Game Mode.
- **Robot computer for initial testing:** this dev machine (x86, Ubuntu 26.04, ROS 2 Lyrical).
- **Network:** both on the same 5 GHz Wi-Fi network.

---

## Phase 1: Design (2026-10-31)

### 1.1 Starting point and first architecture

- **Goal:** use a Steam Deck as a handheld teleop controller for a ROS 2 robot, with two parts: the controller app and the robot side.
- **Key early decision: no ROS on the Deck.** SteamOS has a read-only system and no ROS packages, and ROS 2 (DDS) discovery over Wi-Fi is unreliable. The Deck talks to a robot-side node that converts its messages into standard ROS messages.
- **First design:** Python on both sides (pygame on the Deck, rclpy on the robot), a custom UDP protocol, `sensor_msgs/Joy` out, and stock `teleop_twist_joy` for `cmd_vel`.
- **Safety model**, layered and failing to "no motion":
  - deadman button;
  - link-loss watchdog;
  - stop latch that is set at startup;
  - the base's own `cmd_vel` timeout;
  - a hardware E-stop.

  It is explicitly *not* a safety-rated E-stop.

### 1.2 Scope changes

| Change | Reason |
|--------|--------|
| Added a deployment strategy (install, keys, boot service, commissioning checklist) | Requested early. The checklist is run with the wheels off the ground. |
| Briefly expanded to a telemetry gateway plus video, then **narrowed** to joystick + one camera with minimal configuration | Keep the first version simple |
| Video: JPEG frames rather than H.264/WebRTC | x86 robot, so encoding is cheap. Every frame decodes on its own, and the Deck needs no codec dependencies. |
| Video tuning for Wi-Fi: 480 px at 30 fps, priority marking, paced sending, latency overlay. FEC, adaptive quality and H.264 moved to `future.md`. | MVP vs. later split |
| **Deck app moved from Python to Rust** | Single self-contained binary, predictable timing |
| GUI: **egui** chosen over Tauri | One video texture with overlays suits immediate mode. Tauri brings WebKitGTK and rendering risk under gamescope. |
| Packaging: tarball with a stable install path (Steam shortcut added once), Flatpak later, including private distribution options | Steam ties the controller layout to the shortcut's path and name |

### 1.3 Transport: custom UDP → Zenoh

- **Review of the custom protocol** found a real flaw: recorded sessions could be replayed after they ended, plus a spoofed-request flood vector. The fix (a handshake with a robot nonce, HKDF session keys, ChaCha20-Poly1305) made it a small security protocol of our own.
- **Alternatives considered:**
  - **gRPC:** TCP only, so a stall delivers a burst of stale data.
  - **Game networking libraries** (GameNetworkingSockets, netcode, ENet): no ROS integration.
  - **Sunshine/Moonlight:** inverts the design, and has no guaranteed safe behaviour on link loss. Kept as a benchmark.
  - **WebRTC:** a "buy, not build" option for video later.
- **Side research:** why ROS 2 didn't choose gRPC (timing, pub/sub with discovery, QoS, DDS as an existing standard), and why ROS later added Zenoh. YouTube vs. interactive streaming: seconds of buffering vs. UDP with newest-wins.
- **Decision:** with telemetry and actions coming soon, **Zenoh** (Rust on the Deck, zenoh-cpp on the robot).
- **Follow-on decisions:**
  - a **C++ gateway** instead of `zenoh-bridge-ros2dds`, giving an explicit versioned API, predictable safety timing, and zero-copy video;
  - **Protobuf** for the interface;
  - a Zenoh router on the robot with TLS and access control;
  - a `pair` tool producing a certificate bundle per Deck.

### 1.4 Connecting, pairing and networks

- **Hot connect/disconnect and pairing** made method-agnostic: one enrollment protocol where methods carry only a bundle or a one-time secret. QR, NFC, typed codes and operator approval would all be plug-ins. The Deck has no camera or NFC, so nothing may assume either.
- **Network:** one access point per robot with app-managed Wi-Fi was designed, plus Bluetooth PAN as a low-bandwidth backup. Everything runs over any IP link.
- **MVP rescoped:** one robot, Deck and robot on the **same network**. Switching robots, other connection methods, per-robot access points and Bluetooth moved to post-MVP. The hooks (profiles keyed by robot ID, `connect()`/`disconnect()`, locator/link/pairing interfaces) stay in the MVP.

### 1.5 Full design review

- **Cleanup:** post-MVP material moved out of the MVP docs into `future.md`.
- **Spec bugs found and fixed:**
  - a race where clearing STOP could undo itself (fix: `released_seq`);
  - revocation could be undone by re-pairing under the same name (fix: unique, never-reused certificate names);
  - the pinned status item had no field to travel in (fix: `StatusItem.pinned`);
  - the list of things that set STOP was incomplete.
- **Decisions from your answers:**
  - STOP is **teleop-only** (other `cmd_vel` sources can opt in through `/steamdeck/stop_latched`);
  - **short-drop grace** with a new REARM state;
  - actions are owner-only, and `moves_robot` actions are refused while stopped;
  - drive base undecided, so both `Twist` and `TwistStamped` are supported.
- **Added after review:**
  - `deck_sim`, a simulated robot;
  - gateway `/diagnostics` and a Deck event log;
  - fixed IPs;
  - **two kinds of robot commands:** quick controls through `/joy` listeners on free buttons, with button hints, and deliberate commands as gateway actions.
  - Rumble was dropped as not possible on the Deck.
- **Scores at review:** about 8/10 overall. Weakest area was testability, addressed by the simulated robot.

### 1.6 Research before building

Answered from upstream source and config:
- **Zenoh version:** Jazzy (and later Lyrical) vendor **Zenoh 1.10.1**, built with QUIC, TLS, QUIC datagrams and multilink.
- **Router:** `rmw_zenohd` accepts our config through `ZENOH_ROUTER_CONFIG_URI`.
- **Access control:** supports `cert_common_names`.
- **Joystick conventions:** `joy` uses 6 axes / 21 buttons with up/left = +1, and triggers go 0 → −1. `teleop_twist_joy` parameter names confirmed.
- **mDNS:** SteamOS doesn't resolve `.local` out of the box.
- **Lease:** the default 10 s lease is too slow, so 2 s was planned.

Commits: `8f4f7db` (initial design docs), `2ac8872` (research findings).

---

## Phase 2: M0 spike (2026-10-31)

**Purpose:** answer the open items that can't be settled on paper:
- O1: does gilrs see the Steam Input gamepad?
- O2: does egui work in Game Mode?
- O3: is there a stale burst after a Wi-Fi stall?
- O15: how quickly does the link reconnect?

### 2.1 Setup

- **Build happens on the dev machine, the binary runs on the Deck.** The Deck has no compiler. Both have glibc 2.43, so binaries copy over directly.
- **Rust toolchain** installed with rustup in `~/.cargo`. Docker wasn't usable without root.
- **Discovered:** the dev machine, which is the robot computer for initial testing, runs **ROS 2 Lyrical** on Ubuntu 26.04, not Jazzy. Lyrical's `zenoh_cpp_vendor` pins the same Zenoh 1.10.1, so the plan holds.
- **Code** (`spike/`, throwaway):
  - `robot_spike`: Zenoh router with mTLS, echoes every control message, streams synthetic JPEG;
  - `deck_spike`: egui app with a gamepad view, touch test, link and video stats, stale-burst detection, and a headless mode.
  - Messages are small fixed structs, not Protobuf.
- **Build problems:**
  - eframe 0.37 changed its API (`App::ui`, `egui::Panel`);
  - binaries were 358 MB with debug info, fixed by stripping;
  - `steamos-add-to-steam` rejected a `.sh` over SSH, fixed with a `.desktop` file;
  - a nested Zenoh config key path was rejected, fixed by setting the whole `connect/retry` object.

### 2.2 Tests and results

**Method:** the operator held the Deck, and results were read back from logs over SSH. Stalls were simulated with `scripts/stall.sh`, which uses iptables to drop UDP 7450 in both directions for an exact time and is run with sudo by the operator. That's more repeatable than walking out of range.

| Test | Result |
|------|--------|
| Loopback (dev machine only) | RTT about 1.5 ms, 0 lost, 30 fps video |
| Deck ↔ robot over Wi-Fi, steady state | RTT about 2.5 / 5 / 7 ms (min/avg/p95), 0 lost. Video 30 fps, 100% delivered, 17 KB/frame (4.2 Mbps), about 20 ms capture → received |
| **O1**, button-by-button walkthrough in Game Mode | ✅ One pad, "Steam Virtual Gamepad". All 15 buttons and analog triggers correct. **X axes need negating.** **Back grips send nothing** without a Steam Input layout. Stick noise is up to ±0.08, so the deadzone becomes 0.12. |
| **O2** | ✅ Fullscreen 1280×800, touch, smooth video |
| **O3**, default QUIC endpoint | ❌ **Stale burst:** a 1 s stall replayed 73 queued echoes up to 1.64 s old, and video was about 1 s behind |
| **O3**, plus a second `?rel=0` endpoint | ❌ No change |
| Investigation | Reading the zenoh-link-quic 1.10.1 source showed best-effort data only uses QUIC datagrams when **`mixed_rel=1`** is negotiated, and it's off by default |
| **O3**, `?mixed_rel=1` on both listener and connector | ✅ 1.0 / 1.5 / 3 s stalls: **0 stale**, messages sent during the stall dropped (newest wins), and a 1.5 s stall recovers without reconnecting. Residual backlog ≤ about 170 ms. |
| **O15**, 3 s stall and 5 s airplane mode, with a 2 s lease and 200 ms → 1 s retry | ✅ Zenoh is back about **0.8 s** after Wi-Fi returns. Airplane total about 5.5 s, of which 3.8 s is SteamOS scanning. Operator observed "about 3 seconds". |
| Observation | Natural 300–500 ms Wi-Fi delay spikes with no loss, likely power saving (on at both ends). **Parked** at your request. |

### 2.3 Design changes from the spike

- **Interface:** `?mixed_rel=1` is mandatory on both sides. Lease and retry values are specified.
- **Deck input:** negate X axes, deadzone 0.12, and back grips need the Steam Input layout.
- **Gateway:** new **late-control guard**. It compares each control's delay against the recent minimum, without synchronized clocks, and discards controls more than 150 ms later than that.
- **Deployment:** `steamos-add-to-steam` must be given the `.desktop` file.
- **Robot side:** Jazzy and Lyrical are both supported.

### 2.4 Still open after M0

- Video at the edge of Wi-Fi range.
- The `pair` → bundle flow (needs the real gateway).
- Re-measuring with Wi-Fi power saving off.
- Drive stack choice (O13).

---

## How to keep this log

- Add an entry whenever a decision changes the design, a test produces numbers, or something fails in an instructive way.
- Record the setup (versions, network, power settings) next to any measurements.
- Link commits by hash, and keep raw logs (e.g. Deck logs under `~/rj-spike/logs/`) if a result might be questioned later.
