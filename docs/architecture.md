# Architecture

## Overview

The Deck app (Rust) handles the GUI and controls and knows nothing about ROS. It talks to a **Zenoh router on the robot** over TLS. Two C++ processes on the robot connect to the same router locally. They translate between the **Protobuf Deck interface** ([interface.md](interface.md)) and standard ROS 2 messages.

```mermaid
flowchart LR
  subgraph Deck["Steam Deck — Rust app (no ROS)"]
    CT[Control thread<br/>gilrs, combos, 50 Hz + on change]
    SUB[Zenoh subscribers<br/>robot_state, status, video]
    VD[Video decode thread]
    Q[Query worker<br/>info, clear_stop, actions]
    UI[egui UI<br/>drive · status · actions]
    CT --> UI
    SUB --> UI
    SUB --> VD --> UI
    UI --> Q
  end

  subgraph Robot["Robot — ROS 2 Jazzy"]
    R[zenoh router<br/>quic :7450 TLS + ACL<br/>tcp 127.0.0.1:7451]
    subgraph GW["deck_gateway (C++)"]
      SAFE[safety<br/>watchdog, stop latch]
      STAT[status]
      ACT[actions + info]
    end
    VID[deck_video (C++ component)<br/>resize + JPEG]
    SAFE -->|sensor_msgs/Joy| JOY[/joy/] --> TTJ[teleop_twist_joy] --> CMD[/cmd_vel/]
    TOP[/battery, odom, /diagnostics, .../] --> STAT
    ACT -->|service calls| SRV[ROS services]
    CAM[/camera/image_raw/] --> VID
    GW <--> R
    VID <--> R
  end

  CT -- "rj/v1/control, rj/v1/stop" --> R
  R -- "rj/v1/robot_state, rj/v1/status, rj/v1/video" --> SUB
  Q -- "queries: rj/v1/info, clear_stop, action/*" --> R
```

### Robot processes

| Process | Role | Why it's separate |
|---------|------|-------------------|
| **Zenoh router** (`deck_router`) | The single network entry point: TLS, Deck certificates, access control. Gateway and video connect to it over localhost. | One port and one place for security. Restarting the gateway or video doesn't drop the Deck's connection. |
| **`deck_gateway`** | Safety (control → `/joy`), status, actions, info | Small, and the only robot code involved in safety. Predictable C++ timers. |
| **`deck_video`** | Camera → JPEG → Deck | Heavy work. If it crashes or stalls, the safety path isn't affected. Can be loaded into the camera driver's process for zero-copy images. |

All safety and session logic lives in a **ROS-free C++ library** (`deck_gateway_core`), unit-tested with gtest. The nodes are thin wrappers around it. `deck_sim` provides a simulated robot (turtlesim, a test camera, fake battery, diagnostics and services) for development and CI ([robot.md § Simulation](robot.md#simulation)).

### Deck app

Crates: `deck_msgs` (Protobuf types generated with prost), `deck_core` (Zenoh session, input, safety combos, video decode, with no UI dependency), and `deck_app` (egui + CLI). Details: [controller.md](controller.md).

### Data flows

| Flow | Direction | Delivery | Rate |
|------|-----------|----------|------|
| Control (sticks, buttons, STOP flag) | Deck → robot | Best effort, drop when congested, highest priority, sent without batching ("express") | 50 Hz + immediately on button change |
| STOP | Deck → robot | **Reliable** | On press |
| Robot state (echo for RTT, OK/STOPPED/BUSY) | robot → Deck | Best effort, drop | On each control message, and 5 Hz when idle |
| Status | robot → Deck | Best effort, drop | 2 Hz (configurable) |
| Video | robot → Deck | Best effort, drop, lower priority | ≤ 30 fps, only while the Deck is subscribed |
| Info, clear STOP, actions | Deck → robot queries | Request/reply | On demand |

In every streaming flow, the newest data wins: Zenoh drops instead of queueing, so a Wi-Fi stall never delivers a burst of stale commands. Full spec: [interface.md](interface.md).

## Safety model

The Deck link is a convenience control, **not a safety system**. Every failure has to end in *no motion*:

1. **Deadman.** `teleop_twist_joy` only moves while L2 (the left trigger) is held. The gateway publishes it as `/joy` button 21 and only passes turbo (RB) while it's held.
2. **Watchdog.** No valid control for **250 ms** → `deck_gateway` publishes neutral `Joy` at 50 Hz. The Deck's liveliness token disappearing triggers the same thing.
   - **Short drop** (control back within `resume_grace_ms`, 1 s): the robot stays in **REARM**, with neutral output until the operator releases and presses the deadman again. Flaky Wi-Fi doesn't force a STOP clear, but motion never resumes on its own.
   - **Longer drop:** the stop latch is set (`link_lost`).
3. **Stop latch (teleop STOP).** STOP comes in two ways: a flag on every control message *and* a separate reliable message. It latches in the gateway, and only neutral `Joy` goes out. Clearing it requires a `clear_stop` query, which the gateway accepts **only** if the latest control has sticks centered and LB/RB released. The gateway **starts latched**.
4. **One controller at a time.** The first Deck session to send control owns it until it's been silent for 2 s, or until its liveliness token disappears. Other sessions see `BUSY`. A Deck keeps **one session ID per app run**, so reconnecting after a drop resumes its own ownership with no `BUSY` delay.
5. **A new controlling Deck always starts STOPPED.** Whenever ownership changes (a different Deck, or the same Deck after an app restart), the stop latch is set. A Deck that has just taken control can't drive until its operator has seen the camera and cleared STOP.
6. **Quitting stops the robot right away.** When the app quits, the Deck sends `Stop`, then withdraws its liveliness token. The gateway releases control immediately rather than after the 2 s timeout, so another Deck can take over cleanly.
7. **Menu panel = neutral.** While the Deck's Status/Actions panel is open, the Deck sends neutral driving input.
8. **Motion actions are gated.** Only the controlling Deck can run actions, and actions marked `moves_robot` are refused while STOPPED or REARM.
9. **Base requirements (your robot's job).** The motor controller zeroes velocity when `cmd_vel` goes stale, which covers the gateway itself crashing. It also enforces a hard speed limit that doesn't depend on the teleop scale settings.
10. **Hardware E-stop.** Required on anything that can cause harm.

**Scope of STOP: teleop only.** The Deck's STOP neutralizes the `/joy` path (and gates motion actions). It does **not** block other `cmd_vel` sources such as Nav2 or scripts. Robot software that should also halt on a Deck STOP subscribes to `/steamdeck/stop_latched` (for example as a `twist_mux` lock). The UI calls it "STOP (teleop)" so nobody mistakes it for a robot-wide stop.

Video, status and actions are never on the safety path. Losing video shows **NO VIDEO** on the Deck, and driving continues.

## Robots and connection

### MVP: one robot, same network

- **Network:** the Deck and the robot are on the **same IP network**. That can be an existing 5 GHz Wi-Fi network, a dedicated router, or a single robot's hotspot that the Deck joins in SteamOS Wi-Fi settings. The app doesn't manage Wi-Fi.
- **Address:** the robot needs a stable address on that network (a DHCP reservation, or a `.local` hostname via Avahi). It goes into `pair --host` so the server certificate matches.
- **One robot profile** on the Deck, installed from a pairing bundle (the file method). The app connects at launch, and **reconnects automatically** after drops, keeping its session ID. Short drops resume through REARM, and longer ones come back STOPPED.
- **Robot identity:** robot ID = SHA-256 fingerprint of the robot's CA certificate. The Deck checks it against `Info.robot_id` after connecting, so it never drives a robot that only resembles the expected one.

### Built in now, so switching and pairing methods can be added later

The MVP stays simple, but it doesn't close any doors:

| The MVP already has | So that post-MVP can add |
|---------------------|--------------------------|
| Profiles stored per robot ID (`robots/<id>/`) | Many robots on one Deck |
| `connect()` / `disconnect()` in `deck_core`, used at launch, quit and reconnect | Switching robots at runtime from a Robots screen |
| Handover safety rules (new owner starts STOPPED, deliberate disconnect releases control at once) | Safe handover between Decks and between robots |
| Endpoints from the profile, behind `RobotLocator` / `Link` interfaces with only a static implementation | Discovery (mDNS), one access point per robot with app-managed Wi-Fi, Bluetooth PAN |
| `Info.robot_id` check and a trust-domain field in the profile | Fleet CA |
| File pairing bundle via `pair` | Online enrollment with plug-in pairing methods (QR, NFC, codes, approval, ...), sketched in [future.md § Online enrollment](future.md#online-enrollment) |

Post-MVP designs: [future.md § Connecting, switching and pairing](future.md#connecting-switching-and-pairing-post-mvp).

### Links

Everything runs over **any IP link**: Wi-Fi in the MVP, and Ethernet/USB tethering for bench tests. The same security and interface apply on every link. Bluetooth PAN as a low-bandwidth backup is post-MVP ([future.md](future.md#bluetooth-pan-backup-link)).

## Design decisions

- **D1: ROS-free Deck.** SteamOS updates wipe system changes, and DDS over Wi-Fi is unreliable. The Deck speaks Zenoh + Protobuf only.
- **D2: Zenoh as the transport.** Each flow gets its own delivery setting (best effort or reliable, drop when congested, priority, express), plus request/reply queries, liveliness, TLS/mTLS, and access control. It has a native Rust API on the Deck and `zenoh-cpp` on the robot. Considered and rejected:
  - **gRPC:** TCP only, so a Wi-Fi stall delivers a burst of stale data. It's built for request/response, not streaming.
  - **A custom UDP protocol:** we would own the transport and the security design.
  - **Game networking libraries** (GameNetworkingSockets, netcode, ENet): no ROS integration, weaker Rust support.
  - **Sunshine/Moonlight:** inverts the design, and has no guaranteed safe behaviour when the link drops.

  The custom UDP and QUIC-datagram designs are kept as fallbacks in [future.md](future.md#fallback-transports).
- **D3: A C++ gateway instead of `zenoh-bridge-ros2dds`.** The Deck gets an explicit, versioned API rather than raw ROS topics. Safety, status and actions live in one supervised process. The Deck never decodes ROS types. The bridge stays useful as a debugging tool.
- **D4: Protobuf for the interface.** One `.proto` source generates both Rust (prost) and C++ code. Fields can be added within `v1` without breaking older Decks.
- **D5: A Zenoh router on the robot.** One external port, one TLS and access-control configuration, and processes can restart without the Deck reconnecting.
- **D6: The gateway outputs `/joy`, and stock `teleop_twist_joy` makes `cmd_vel`.** The layout matches ROS 2 `joy`'s `game_controller_node` (SDL order).
- **D7: Rust + egui on the Deck.** A single binary, predictable timing, and an immediate-mode GUI that suits a video texture with overlays. Tauri was considered: better for rich UIs, but it brings WebKitGTK on SteamOS and rendering risk under gamescope. `deck_core` keeps the UI swappable.
- **D8: JPEG video.** Each frame decodes on its own, and the Deck needs no codec dependencies. `deck_video` is C++, so H.264 or WebRTC via GStreamer is a contained upgrade ([future.md](future.md#video)).
- **D9: TLS with a private CA per robot.** A `pair` tool creates a certificate bundle for each Deck. The router only accepts allowed certificates. Setup stays at "copy one file". A fleet CA is an option.
- **D10: The MVP uses one robot on a shared network.** The Deck and the robot share an IP network, the Deck has one profile from a pairing bundle, and the app doesn't manage Wi-Fi. This keeps the MVP focused on driving, video, status and actions.
- **D11: Seams for what comes next.** Profiles keyed by robot ID, `connect()`/`disconnect()`, the handover safety rules, and the locator/link/pairing interfaces are all in the MVP, so switching robots and connection methods can be added post-MVP without rework.
- **D12: Two kinds of robot commands.** Quick controls while driving go through `/joy` to standard ROS listeners on free buttons. They're gated by `/joy` automatically, and the robot labels them with button hints. Deliberate commands that need feedback are gateway actions, with confirm, a visible result and logging. The gateway stays small, and robot developers use tools they already know.

## Open items (to resolve in M0 or during build)

| # | Question | Resolved by |
|---|----------|-------------|
| O1 | ✅ **Answered (M0):** gilrs sees exactly one pad, `Steam Virtual Gamepad` (Xbox 360, SDL mapping). All 15 buttons, sticks and analog triggers are correct. X axes need negating. Back grips L4/R4/L5/R5 send nothing until the Steam Input layout maps them. Resting stick noise is up to ±0.08, so the deadzone is 0.12. | Done |
| O2 | ✅ **Answered (M0):** eframe 0.37 runs fullscreen at 1280×800 under gamescope, touch works, and video plays smoothly at 30 fps. | Done |
| O3 | ✅ **Answered (M0):** with default QUIC, a 1 s stall replayed every queued echo up to 1.64 s late (stale burst). `?rel=0` on a second link didn't help. **`?mixed_rel=1` on both the listener and the endpoint** fixes it: 1.0, 1.5 and 3 s stalls show **0 stale messages**, lost messages are simply dropped, and a 1.5 s stall recovers without reconnecting. A small residual backlog (≤ about 170 ms) comes from Zenoh's send queue. | Done |
| O4 | DSCP and socket buffers. **Partly answered:** Zenoh documents `#dscp=` for TCP/UDP links and `so_sndbuf`/`so_rcvbuf` for TCP/TLS, but not for QUIC. Fallback for robot → Deck traffic: mark DSCP with an `nftables` rule on UDP port 7450. The Deck → robot direction is low-bandwidth, and marking it would need root on the Deck, so it's skipped. | Spike |
| O5 | ✅ **Answered, with a correction (2026-11-01):** the source of Jazzy's and Lyrical's `zenoh_cpp_vendor` pins zenoh-c/zenoh-cpp to **1.10.1** (plus fixes), but the *binary* must be new enough: Jazzy ≥ 0.2.12, Lyrical ≥ 0.10.7. Lyrical 0.10.6, the main apt repo's version until 0.10.7 syncs, contains Zenoh **1.8.0 without `mixed_rel`**. Until then, install 0.10.7 from `ros2-testing` ([deployment.md § Zenoh package version](deployment.md#zenoh-package-version)). The builds check the version. 1.10.1 is built with its default features (TCP, UDP, TLS, QUIC, QUIC datagrams, multilink). `rmw_zenohd` reads `ZENOH_ROUTER_CONFIG_URI`, so it works as our router with `router.json5`. The Deck pins the `zenoh` crate to `=1.10.1`. The spike confirms that crates.io 1.10.1 interoperates with the vendored build. | Done; recheck on Jazzy syncs |
| O6 | ✅ **Verified (O17 tests, 2026-11-01):** access-control subjects support `cert_common_names` (plus `interfaces` and `link_protocols`). ZIDs aren't authenticated, so they're never used for access. `low_pass_filter` (per `key_exprs`, `size_limit`) provides our per-key payload limits. Tested with the product `router.json5`: an unlisted CN signed by the robot CA can connect but can't put, subscribe, query or declare tokens; a Deck can't snoop control or spoof robot keys; limits are exact at the boundary (1024 B passes, 1025 B is dropped). | Done (`spike/o17/acl_test.py`, 28 checks) |
| O7 | ✅ **Mostly answered** from source: `game_controller_node` publishes **6 axes / 21 buttons** in SDL order, with all axes inverted so stick up/left = **+1** (same as our wire format). Triggers come out as **0 released → −1.0 fully pressed**, so the gateway maps our 0..1 to 0..−1. `teleop_twist_joy` parameters: `require_enable_button`, `enable_button`, `enable_turbo_button`, `axis_linear.x`, `scale_linear(_turbo).x`, `axis_angular.yaw`, `publish_stamped_twist` (default `false`), `frame`. Confirm trigger sign on hardware in M1. | M1 (quick check) |
| O14 | ✅ **Answered (no):** community reports say SteamOS doesn't resolve `.local` out of the box (mDNS is off in the read-only image). Robots use a **fixed IP**. If names are wanted later, the Deck app can resolve mDNS itself (`mdns-sd` crate) without touching SteamOS. | Done |
| O15 | ✅ **Answered (M0):** with a 2 s lease and the faster retry, Zenoh reconnects about **0.8 s** after Wi-Fi returns. After airplane mode, the total is about 5.5 s, dominated by SteamOS rescanning (3.8 s) and associating (0.9 s). No false disconnects on normal Wi-Fi. | Done |
| O16 | Natural Wi-Fi delay spikes of 300–500 ms with no loss, likely Wi-Fi power saving (on at both ends during M0). Handled by the late-control guard ([interface.md § Rules](interface.md#rules)). Re-measure with power saving off. | M1 |
| O13 | Drive stack not chosen yet: ros2_control `diff_drive_controller` (`TwistStamped`) or a custom driver (`Twist`). The launch argument `cmd_vel_stamped` supports both, and either way the base must provide the stale-command timeout and speed limit. | Before M1 on the real robot |
| O17 | ✅ **Answered (2026-11-01): `rmw_zenohd` can't be the router yet; we use `deck_router`.** `rmw_zenohd` handles the config correctly (mTLS, access control, size limits: 28/28), but over Wi-Fi with `mixed_rel=1` it **aborted after about 20 s**: `quinn-proto` 0.11.17, locked by every current ROS zenoh-c (Jazzy, Lyrical, rolling), panics in `Datagrams::send` once the datagram buffer fills ([quinn #2805](https://github.com/quinn-rs/quinn/issues/2805), fixed in 0.11.18), after a backlog builds up to seconds of round trip. `deck_router` (`robot/deck_router`, about 100 lines of Rust on crates.io Zenoh **1.10.2** with `quinn-proto` 0.11.19) runs the same `router.json5`: 28/28, 2 min on Wi-Fi with round trip 4.5 / 7.1 / 10 ms and 0 stale, and stalls of 1.0 / 1.5 / 3 s with 0–1 stale messages (the one was 251 ms, the M0 residual queue) and reconnect after the lease expired. 1.10.2 also has QUIC fixes the ROS build lacks (datagram MTU in mixed reliability, an acceptor DoS). Switch back when ROS ships zenoh-c ≥ 1.10.2 with `quinn-proto` ≥ 0.11.18. | Done; recheck on each `zenoh_cpp_vendor` release |
| O18 | Building the Deck tarball. `deployment.md` plans an Ubuntu 22.04 container, but Docker isn't usable without root on the dev machine. The Deck's glibc (2.43) currently equals the dev machine's, so a local build runs there, but that breaks as soon as the dev machine's glibc is newer than the Deck's. Options: rootless Podman, building on an older-glibc toolchain (e.g. `cargo zigbuild` targeting an older glibc), or building in CI. | Before M5 |
