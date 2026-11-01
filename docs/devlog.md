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
- **Zenoh version:** Jazzy (and later Lyrical) vendor **Zenoh 1.10.1**, built with QUIC, TLS, QUIC datagrams and multilink. *(True of the source; the Lyrical binary on apt was older. See 3.3.)*
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
- **Discovered:** the dev machine, which is the robot computer for initial testing, runs **ROS 2 Lyrical** on Ubuntu 26.04, not Jazzy. Lyrical's `zenoh_cpp_vendor` pins the same Zenoh 1.10.1, so the plan holds. *(Corrected in 3.3: only from release 0.10.7 on.)*
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

### 2.4 First end-to-end demo

- **Goal:** prove the whole chain before writing the real gateway: Deck → Zenoh → ROS → something moving, with video back.
- **Chain:** RJ Spike on the Deck → Zenoh (QUIC, mTLS, `mixed_rel=1`) → `spike/demo/demo_bridge.py` → `/joy` → `teleop_twist_joy` (LB deadman, RB turbo) → `/turtle1/cmd_vel` → **turtlesim** on the robot computer. Webcam → JPEG → Deck screen.
- **The bridge** (Python, rclpy + zenoh-python 1.10.1, about 150 lines) stands in for `deck_gateway` and `deck_video`. It:
  - converts gilrs axes to `joy` conventions (X negated, triggers 0 → −1);
  - runs a 250 ms watchdog that sends neutral `Joy`;
  - echoes each control back so the Deck can measure RTT.

  It speaks the spike messages, not Protobuf.
- **Setup snags:**
  - The system Python has no pip, so `pip` was bootstrapped into a project venv (`--system-site-packages`, so `rclpy` and `cv2` stay visible).
  - ROS setup must be sourced from bash, not zsh.
  - The webcam needed MJPEG to get past 15 fps in raw mode.
- **Bonus:** the Deck app, still running from earlier tests, reconnected to the new bridge by itself when it replaced `robot_spike` on the same port.
- **Results:**
  - Operator drove the turtle with the Deck: it went from (5.54, 5.54, 0 rad) to (4.80, 5.53, 0.65 rad), and stopped with 0 velocity on deadman release.
  - Control steady at 50/s, watchdog never tripped.
  - RTT 2.5 / 4.4 / 7.0 ms (min/avg/p95).
  - Webcam 15 fps (likely auto-exposure in low light), about 11 KB/frame, 96% delivered, about 13 ms capture → received.
- **Bug found after the demo:** when the operator quit the Deck app, the bridge's watchdog fired and crashed on `logger.warn()`, which Lyrical's rclpy no longer has (it's `warning()`). `demo.sh` then stopped everything, so it failed safe and the turtle stopped. Fixed. Lesson for M1: the real gateway's link-loss paths need automated tests (the planned `launch_testing` suites), not just manual runs.

### 2.5 Still open after M0

- Video at the edge of Wi-Fi range.
- The `pair` → bundle flow (needs the real gateway).
- Re-measuring with Wi-Fi power saving off.
- Drive stack choice (O13).

## Phase 3: M1 Drive (started 2026-11-01)

### 3.1 Protobuf interface and simulated robot

- **Order:** the interface and `deck_sim` came first, so the gateway and the Deck app both have something real to build and test against.
- **`interface/` (`remote_joy_interface`):**
  - The design-doc sketch, split into five files (`control`, `info`, `status`, `action`, `video`).
  - New: `Axis` and `Button` enums. They're constants only, so the joy indices have one source of truth in both languages.
  - A hand-written `keys.hpp` holds the key names and encodings.
  - The `ZENOH_VERSION` pin is exported to CMake.
- **`controller/deck_msgs` (Rust):** prost types generated at build time by **protox**, a pure-Rust `.proto` compiler. The Deck build needs no `protoc`, and the `.proto` files stay the only definition.
- **Wire-format pinning:** golden vectors in `interface/test/golden/`, as text-format messages plus the bytes `protoc` encodes from them. The C++ and Rust tests must each reproduce those exact bytes. That's three independent implementations agreeing, which catches drift before it reaches the robot. A `Control` is 45 bytes, versus 40 for the spike's fixed struct.
- **`robot/deck_sim`:**
  - turtlesim as the base;
  - `fake_robot`: battery that drains faster while moving and charges when docked, odometry, diagnostics, `/dock` and `/lights` services, and `/joy` A = horn, X = lights;
  - `test_camera`: a moving test pattern showing the frame counter and timestamp. It replaced `image_publisher`, which isn't installed here, and makes dropped frames and latency visible.
  - `sim.launch.py` includes `deck_gateway`'s launch once that package exists. Until then it starts `teleop_twist_joy` itself.
- **Tests:**
  - Golden tests: C++ (gtest) 9/9 and Rust 9/9 pass. The `.bin` files are byte-identical whether encoded by `protoc` 35.1 (the `grpcio-tools` build) or by Ubuntu's 3.21.12, the version ROS builds against.
  - A scratch downstream CMake package found `remote_joy_interface`, linked it, and read `REMOTE_JOY_ZENOH_VERSION` (1.10.1), so `deck_gateway` can depend on it as is.
  - `deck_sim` pytest: 12/12 pass.
  - Sim smoke test:
    - `/joy` with LB held drove the turtle 4.6 m.
    - A logged a horn, and X turned the lights on.
    - `/dock` sent the turtle home and refused a second dock.
    - Diagnostics reported correctly, and the camera ran at about 25 fps.
- **Snags:**
  - New setuptools ignores `tests_require`, so colcon silently ran 0 tests until it moved to `extras_require`.
  - `pkill -f` matched its own shell again; use exact process names.

### 3.2 Reproducible dev setup

- **Problem:** so far everything had been installed by hand: rustup user-level, a pip venv for the demo, and protobuf via a manual `apt`. A fresh machine had no record of any of it, and `deployment.md`'s apt list was Jazzy-only and missing protobuf.
- **Fix:**
  - `scripts/setup_dev.sh` (and `--check`) installs everything declared in the `package.xml` files via rosdep, plus colcon/rosdep themselves and Rust via rustup.
  - Rust is pinned to 1.99.1 in `controller/rust-toolchain.toml`.
  - The script holds no package list of its own. **Rule: every new dependency goes into a `package.xml` or `Cargo.toml`.**
- **First `--check` run** found `libprotoc-dev` still missing (rosdep's `protobuf-dev` key includes it) and the pinned Rust not yet installed. The pinned toolchain then passed the Rust tests and clippy.

### 3.3 Checking open questions before the gateway: a wrong Zenoh version

- **How it was found:** before writing the gateway I checked that zenoh-cpp was available. It wasn't installed, so I downloaded and unpacked the `.deb` files without installing them, and read `zenoh_configure.h` and the library's strings.
- **Finding:** Lyrical's apt `zenoh_cpp_vendor` **0.10.6** is zenoh-c **1.8.0** / zenoh-cpp 1.9.0 (Zenoh commit 2687c51, tag v1.8.0). Zenoh 1.8.0's QUIC link has **no `mixed_rel`**: it isn't in the source at that tag or in the binary. Zenoh ignores unknown endpoint options, so the gateway would have built and run, and the M0 stale burst would quietly have come back.
- **Why the design said 1.10.1:** the research read the `zenoh_cpp_vendor` CMakeLists on the source branches, which do pin 1.10.1, and rosdistro already lists **0.10.7** for Lyrical (0.2.12 for Jazzy). But 0.10.7 was only in `ros2-testing` (built 2026-10-24) and hadn't synced to the main repo yet. Unpacking the testing `.deb` confirmed zenoh-c 1.10.1, Zenoh commit 9fcd9cb (matching upstream), and `mixed_rel` present.
- **Workaround:**
  - Install just the two 0.10.7 `.deb` files from `ros2-testing`.
  - An apt pin was the first idea, but it can't work: the two repos publish identical `Release` metadata (origin, label, suite), so apt can't tell them apart.
  - All of 0.10.7's dependencies were already installed from the main repo.
- **Guard rails added:**
  - `setup_dev.sh --check` compares the installed zenoh-c with `interface/ZENOH_VERSION`.
  - `deck_gateway`'s CMake will fail on a mismatch.
  - The docs now state the minimum package versions instead of "Lyrical vendors 1.10.1".
- **Lesson:** check the binary you'll actually install, not the source branch. Version claims from source can run ahead of what apt ships.
- **Other open items recorded in this pass:**
  - O6 (access control) was only answered from docs, so it's now "verify in M1".
  - New O17: `rmw_zenohd` has never been used as our router.
  - New O18: the Deck tarball can't be built in Docker without root, and relying on equal glibc versions is fragile.

### 3.4 O17: `rmw_zenohd` as the router, and why it's `deck_router` instead

- **Setup:**
  - A draft product `router.json5`, with QUIC + mTLS + `mixed_rel=1`, a 2 s lease, default-deny access control by certificate CN, and per-key size limits. TLS paths come from `ZENOH_CONFIG_OVERRIDE`, which is how the launch file will pass them.
  - The spike binaries got `--v1-keys` (product key names, so the product access rules apply unchanged) and `robot_spike --local` (a client of the router's TCP endpoint, like the gateway).
  - `spike/o17/acl_test.py` plays the gateway and four Decks: paired, intruder (robot CA, unlisted CN), rogue (other CA, *same* CN) and no certificate.
- **Access control and limits on `rmw_zenohd` (0.10.7, zenoh-c 1.10.1):** 28/28 checks passed:
  - the rogue and no-certificate clients are refused at TLS;
  - the intruder connects but can't put, subscribe, query or declare tokens;
  - a paired Deck can't snoop control or spoof `robot_state`;
  - size limits are exact (1024 B control passes, 1025 B is dropped; 512 KB video, 64 KB status);
  - liveliness tokens appear and disappear as expected.

  **Control run:** with access control and limits off, exactly the 12 deny checks fail, so the suite really measures them.
- **On real Wi-Fi it failed.** Deck → `rmw_zenohd` → `robot_spike`:
  - the round trip climbed from 5 ms to 2.6 s p95, with 459 stale echoes;
  - then the router **aborted**: `quinn-proto-0.11.17 … datagrams.rs:47: datagrams.outgoing.payload_bytes desynchronized`.

  This is [quinn #2805](https://github.com/quinn-rs/quinn/issues/2805): with `drop = true`, evicting an outgoing datagram subtracted its size twice. The accounting drifts, the queue grows, and eventually it pops an empty queue. Loopback never fills the buffer, which is why the local tests passed.
- **Same conditions, the spike router** (crates.io Zenoh 1.10.1, `quinn-proto` 0.11.19), a minute later: round trip 4 / 6.7 / 8 ms, 0 stale. So the router build was the cause, not Wi-Fi.
- **Scope of the problem:**
  - Every current ROS zenoh-c lock (Jazzy, Lyrical, rolling) has `quinn-proto` 0.11.17; zenoh-c `main` has 0.11.19.
  - crates.io Zenoh **1.10.2** (2026-10-28) also has QUIC fixes the ROS build lacks: a symmetric datagram MTU in mixed reliability, and an acceptor DoS fix.
- **Decision: `deck_router`.**
  - About 100 lines of Rust on Zenoh 1.10.2, the same `ZENOH_ROUTER_CONFIG_URI` / `ZENOH_CONFIG_OVERRIDE` interface as `rmw_zenohd`, built by colcon through `cargo --locked`.
  - It refuses non-router modes and QUIC endpoints without `mixed_rel=1`.
  - Tests pin `zenoh` to `interface/ZENOH_VERSION` and require `quinn-proto` ≥ 0.11.18.
  - The gateway and video stay on ROS's zenoh-c, which is fine because they only use local TCP.
  - `ZENOH_VERSION` became `series=1.10`, `crates=1.10.2`, `ros_min=1.10.1`, with an exported CMake check (it accepts 1.10.1 and 1.10.7, rejects 1.8.0 and 1.11.0).
  - The Rust toolchain pin moved to the repo root.
  - We go back to `rmw_zenohd` once ROS ships a fixed zenoh-c.
- **`deck_router` results:**
  - 28/28 checks (and the control run: 16/28).
  - Python Zenoh 1.10.1 clients and the 1.10.1 spike interoperate with the 1.10.2 router.
  - Deck over Wi-Fi for 2 min: round trip 4.5 / 7.1 / 10 ms, 0 lost, 0 stale, video 30 fps with 99.7–99.9% delivered and about 23 ms latency.
  - Stalls:

    | `stall.sh` | Seen by the Deck | After recovery |
    |---|---|---|
    | 1000 ms | 1323 ms | 1 stale echo at 251 ms (the M0 residual send queue; the late-control guard would drop it) |
    | 1500 ms | 2020 ms | 0 stale |
    | 3000 ms | 4209 ms | 0 stale; the lease expired and it reconnected, with the token re-declared. 209 controls during the gap were dropped (newest wins), no burst |
- **Also:**
  - Zenoh warns that interface-based ACL subjects may misapply on multi-interface transports, so the local subject matches only `tcp` (bound to loopback).
  - `steamdeck` now resolves through Tailscale, whose SSH asks for a periodic re-check.
  - A Deck test started over SSH died when the session closed, even with `nohup`/`setsid`. The first stall run measured nothing until it was rerun under `systemd-run --user`.
- **Lesson:** a test that passes on loopback can still fail under real link conditions. Buffer-full paths only show up on a real, slower link.

### 3.5 `deck_gateway` step 1: the safety core

- **What:** `deck_gateway_core`, a static C++17 library with no ROS, Zenoh or Protobuf. Every call takes `now_us`, so tests control time.
  - `SafetyCore`: ownership, the 250 ms watchdog, the owner timeout, REARM vs `link_lost` around `resume_grace_ms`, the stop latch with the `released_seq` rule, `clear_stop`, and action gating.
  - `LateControlGuard`: a sliding-window minimum of `arrival − deck_time` over 10 s, kept in a monotonic deque.
  - `to_joy`: the `joy` layout, with trigger mapping, clamping, and NaN or inf mapped to 0.
  - The node will only translate between Protobuf/ROS and these types.
- **Spec gaps found while writing it,** now fixed in `interface.md`:
  - **BUSY can't be broadcast:** `RobotState` goes to every Deck, so a `BUSY` state would also reach the owner. The Deck now derives BUSY from `owner_session`.
  - "Deadman" means LB *or* RB, because `teleop_twist_joy` also drives with RB alone.
  - Late controls stay in sequence: they're echoed and keep ownership, but don't feed the watchdog.
  - `clear_stop` is also refused with no live control, or while the Deck still sends `stop = true` at `seq >= released_seq`.
  - When the latch is already set, the first reason is kept.
- **Tests:** 25 gtests with a fake clock, covering every rule, including the clear-stop race (the query overtakes in-flight `stop = true` controls), and a new owner not inheriting the previous owner's `released_seq`. They passed on the first run.
- **Mutation check:** I broke six rules one at a time: no `released_seq` guard, short drops latching, the late guard off, REARM resuming with the deadman still held, a new owner not latched, `clear_stop` ignoring the deadman. Every one made at least one test fail. They're cheap to rerun after changes.

### 3.6 `deck_gateway` step 2: the node, launch and `fake_deck`

- **`gateway_node`:**
  - Wires `SafetyCore` to ROS (`/joy`, the latched `steamdeck/stop_latched` and `owner_connected` Bools, the `steamdeck/set_stop` service, `/diagnostics` "deck_gateway: link") and to zenoh-cpp:
    - subscribers on `control` and `stop`;
    - liveliness on `deck/*`;
    - queryables `clear_stop` and `info`;
    - the `robot_state` publisher (best effort, express, `RealTime`).
  - All callbacks and the 50 Hz wall timer go through one mutex around the core.
  - CMake fails unless ROS's zenoh-c is in the pinned series (`remote_joy_check_ros_zenoh`, 1.10.1 here).
  - Builds with no warnings under `-Wall -Wextra -Wconversion`.
- **`robot.launch.py`:**
  - Starts `deck_router` with `ZENOH_ROUTER_CONFIG_URI` plus TLS paths from `tls_dir` in `ZENOH_CONFIG_OVERRIDE`, then the gateway, then teleop. `deck_sim` includes it and passes `tls_dir`.
  - The router and gateway respawn.
- **`fake_deck`:** a C++ stand-in Deck on the real Protobuf interface, scriptable (`info`, `drive`, `gap`, `hold`, `stop`, `clear`, `watch`). It's the basis for the `launch_testing` suites.
- **Smoke test against `deck_sim` with the spike certificates:**
  - `info` replies; `drive` clears the stop and moves the turtle (5.54, 5.54) → (6.38, 6.01); the round trip through router and gateway averages 2 ms.
  - A 500 ms gap goes NO_CONTROL → REARM → OK after release and re-press.
  - A 1500 ms gap latches `link_lost`.
  - A second Deck sees BUSY; STOP from an observing Deck latches `deck_stop`.
  - Neutral `/joy` runs at exactly 50 Hz with no Deck, and `/diagnostics` has every counter.
  - Killing `gateway_node`: respawned within about 1 s, latched `startup`.
- **Bugs found:**
  - `launch_ros` appends `--ros-args …` to every `Node`, which `deck_router` rejected (respawn loop). It now ignores that tail (with a test).
  - A new owner inherited the previous owner's stop reason (`link_lost`) when the latch was still set. An ownership change now always reports `new_owner` (core test added, 26 total).
- **Not yet:** `robot_id` in `Info` (comes with `pair`), status, actions and hints (M3/M4), video (M2).

### 3.7 Spike app on the real interface, first Deck-in-the-loop demo, deadman → L2

- **`deck_spike --v1`:** a stopgap so the Deck can drive the real gateway before the Deck app exists. It sends Protobuf `Control` (X negated), B = STOP (local latch plus a reliable `Stop`), View + Menu held 1 s = `clear_stop` with `released_seq`, shows `RobotState`, and sends STOP on quit.
- **Demo (Deck → Wi-Fi → `deck_router` → `deck_gateway` → teleop → turtlesim):** all worked first time: clear, drive, release to stop, B → `deck_stop`, clear again, quit.
- **Decision: the deadman moves from LB to L2 (left trigger).** It's analog, and `teleop_twist_joy` needs a button, so:
  - The gateway publishes `/joy` button **21** = "deadman held" (above 0.5, held until below 0.3), and teleop uses `enable_button: 21`.
  - **Turbo bypass found:** `teleop_twist_joy` drives on its turbo button alone. With the deadman off LB, RB alone would have driven at turbo speed, so the gateway now clears RB from `/joy` unless the deadman is held.
  - REARM and `clear_stop` use "deadman held". LB becomes a free button; LT becomes reserved.
  - A `deadman` parameter (LT, RT, LB, RB).
  - Tests: 31 core tests, with the generic rules still on a button deadman and 5 new ones for the trigger. A mutation check (no RB gate, no hysteresis, no button 21) was caught by the tests.
  - In the sim: `/joy` has 22 buttons and teleop logged "enable button 21".
  - On the Deck: cleared, then drove with L2. The turtle went from (6.30, 5.54) to (3.99, 6.91).

### 3.8 `deck_gateway` step 3: `pair`

- **`pair`** (Python, using `python3-cryptography` rather than the `openssl` command):
  - the robot CA (EC P-256, 10 years) and a server certificate for the `--host` values plus `127.0.0.1`/`localhost`;
  - per-Deck client certificates with CN `<deck>-<6 hex>`;
  - a 0600 bundle (`ca.pem`, `client.pem`, `client.key`, `robot.toml` with the robot ID and endpoints);
  - a generated `router.json5` with the allow list, checked with `deck_router --check`;
  - `--list`, `--revoke`, `--reissue-server` (same CA), `--no-restart`.
  - Keys are written 0600 from the start, with no window where they're readable.
- **`robot.launch.py`:**
  - reads `pair`'s state from `config_dir` (TLS files, generated router config, robot name);
  - passes `robot_id` (SHA-256 of the CA's DER) to the gateway, which returns it in `Info`.
- **Tests:** 12 pytest cases in temporary directories:
  - certificate chains and EKUs, SANs, file modes, bundle contents, and that the key matches the certificate;
  - CNs never reused, the allow list after pairing and revoking, the empty-list placeholder;
  - a new host needs `--reissue-server` and keeps the CA;
  - bad names rejected, the robot name fixed after the first run, `--list`.
  - One test bug: argparse reads `--deck -deck` as two options, so the test now uses `--deck=-deck`.
- **On the Deck, with a scratch `--dir`:**
  - With a paired bundle, the Deck connects (6.4 ms round trip) and shows STOPPED `new_owner`.
  - The old spike certificates (another CA) can't connect at all.
  - After `--revoke`, the router restarts and the revoked certificate passes TLS but is blocked by access control (DOWN, no `RobotState`).
  - Re-pairing gives a new CN that works while the old one stays locked out.
  - `Info.robot_id` matches the bundle's `id`.

---

## How to keep this log

- Add an entry whenever a decision changes the design, a test produces numbers, or something fails in an instructive way.
- Record the setup (versions, network, power settings) next to any measurements.
- Upstream problems (bugs, doc gaps in other projects) go into [upstream.md](upstream.md) as drafts with their evidence. Nothing is filed until the end-of-project review.
- Link commits by hash, and keep raw logs (e.g. Deck logs under `~/rj-spike/logs/`) if a result might be questioned later.
