# ros2_steamdeck_remote_joy

Drive a **ROS 2** robot from a **Steam Deck** over Wi-Fi: live camera, robot status, and robot actions on the Deck's screen.

| Part | Runs on | Built with | Does |
|------|---------|------------|------|
| **Controller app** (`controller/`) | Steam Deck (SteamOS) | Rust, egui, Zenoh, **no ROS** | Reads the Deck's controls, shows camera, status and actions, sends control input |
| **Robot gateway** (`robot/`) | Robot (x86, ROS 2 Jazzy or Lyrical) | C++, rclcpp, zenoh-cpp | Converts between the Deck interface and standard ROS messages. Owns the safety logic (watchdog, stop latch). Streams the camera. |
| **Interface** (`interface/`) | both | Protobuf | The versioned Deck ↔ robot API |

```
 ┌──── Steam Deck ────┐   Zenoh over Wi-Fi (TLS)   ┌──────────────────────── Robot ─────────────────────────┐
 │                    │                            │ zenoh router (TLS, ACL) :7450                          │
 │  egui app          │ ── control 50 Hz ────────► │   ├─ deck_gateway  ── /joy ──► teleop_twist_joy ──► /cmd_vel
 │   drive / status / │ ◄─ robot state ─────────── │   │    safety: watchdog + stop latch                   │
 │   actions          │ ◄─ status 2 Hz ─────────── │   │    status: topics + /diagnostics → Deck            │
 │                    │ ── action queries ───────► │   │    actions: Deck → ROS services                    │
 │                    │ ◄─ video 30 fps ────────── │   └─ deck_video   ◄── /camera/image_raw (JPEG)          │
 └────────────────────┘                            └────────────────────────────────────────────────────────┘
```

## Why this design

- **No ROS on the Deck.** SteamOS has a read-only system, and ROS 2 discovery over Wi-Fi is unreliable. The Deck app is one self-contained Rust binary.
- **MVP: Deck and robot on the same network,** one robot per Deck. The design already has hooks for post-MVP robot switching, per-robot access points, other pairing methods and Bluetooth.
- **Zenoh** is built for unreliable links. Each kind of data gets its own delivery setting: newest-wins for control and video, reliable for STOP and actions. Security is standard TLS. It's also an official ROS 2 middleware option (`rmw_zenoh`).
- **A C++ gateway** gives an explicit, versioned interface (Protobuf) instead of exposing raw ROS topics, puts the safety logic in one process with predictable timing, and lets video run as a composable node with zero copies.

Details: [docs/architecture.md](docs/architecture.md).

## Milestones

| | Scope |
|--|-------|
| **M0 Spike** ✅ | gilrs + egui in Game Mode, Zenoh control/video behaviour on the real Wi-Fi. Passed: [spike/README.md](spike/README.md) |
| **M1 Drive** | Deck controls → `/joy`, deadman, turbo, watchdog, stop latch, auto-reconnect with REARM, handover safety rules. One robot, Deck and robot on the same network. |
| **M2 Video** | One camera, JPEG 480 px wide at 30 fps, latency overlay |
| **M3 Status** | Robot status panel (battery, speed, diagnostics, chosen topics) |
| **M4 Actions** | Panel actions → ROS services (`Trigger`/`SetBool`), plus button hints for robot-side `/joy` listeners on free buttons |
| **M5 Packaging** | Deck tarball with a stable install path, robot systemd service |
| *Post-MVP* | Switching robots at runtime, one access point per robot with app-managed Wi-Fi, online pairing methods, discovery, Bluetooth backup ([future.md](docs/future.md#connecting-switching-and-pairing-post-mvp)) |

M1–M4 can be built against the simulated robot (`deck_sim`) before the real robot is ready.

Beyond these (H.264/WebRTC, FEC, multiple cameras, maps, Flatpak, ...): [docs/future.md](docs/future.md).

> ⚠️ **Not a safety-rated emergency stop.** It runs over Wi-Fi. Any robot that can hurt someone needs an independent hardware E-stop.

## Default controls

| Input | Action |
|-------|--------|
| **LB** hold (or L4 back grip) | Deadman: enable driving |
| **RB** hold | Turbo |
| Left stick ↕ / right stick ↔ | Forward-back / turn |
| **B** or touchscreen **STOP** | Latch teleop STOP (always, on every screen). Other `cmd_vel` sources aren't blocked unless wired to `/steamdeck/stop_latched`. |
| **View + Menu** held 1 s, sticks centered | Clear STOP |
| **Menu** (tap) | Open/close the Status & Actions panel. Driving input is neutral while it's open. |
| **A, X, Y, D-pad, L3/R3, triggers** | Free for the robot (e.g. horn, lights), labelled on screen by the robot's button hints |

## Repository layout

```
.
├── docs/                  # architecture, interface, controller, robot, deployment, future
├── interface/             # ✅ ament_cmake pkg remote_joy_interface: .proto files + generated C++ library
├── controller/            # Cargo workspace (COLCON_IGNORE): deck_msgs ✅, deck_core, deck_app, packaging/
├── robot/
│   ├── deck_gateway/      # C++: safety, status, actions, info. Router config, launch, pair tool.
│   ├── deck_router/       # ✅ Rust: the Zenoh router (QUIC + mTLS, access control), see O17
│   ├── deck_video/        # C++ composable node: Image → JPEG → Zenoh
│   └── deck_sim/          # ✅ simulated robot for development and CI (turtlesim, test camera, fake status)
├── spike/                 # M0 spike and the turtlesim demo (throwaway)
└── deploy/systemd/
```

## Setup in one screen (target — not implemented yet)

```bash
# Robot
colcon build --packages-up-to deck_gateway deck_video deck_router && source install/setup.bash
ros2 run deck_gateway pair --deck deck1 --host 192.168.1.50    # → deck1.bundle.tar
ros2 launch deck_gateway robot.launch.py image_topic:=/camera/image_raw

# Steam Deck (Desktop Mode), with the tarball from controller/packaging/build.sh
tar xf steamdeck-remote-joy-*.tar.gz -C ~ && ~/steamdeck-remote-joy/install.sh
scp robot@192.168.1.50:deck1.bundle.tar ~ && steamdeck-remote-joy robot add --bundle ~/deck1.bundle.tar
```

Then launch **Steam Deck Remote Joy** from Game Mode. Full steps: [docs/deployment.md](docs/deployment.md).

## Docs

| Doc | Contents |
|-----|----------|
| [architecture.md](docs/architecture.md) | Components, safety model, decisions, open items, spike plan |
| [interface.md](docs/interface.md) | Zenoh keys, delivery settings, Protobuf messages, security, versioning |
| [controller.md](docs/controller.md) | Steam Deck app: crates, threads, screens, controls |
| [robot.md](docs/robot.md) | `deck_gateway`, `deck_video`, router, configuration |
| [deployment.md](docs/deployment.md) | Build, pairing, Wi-Fi setup, install, commissioning |
| [future.md](docs/future.md) | Beyond the milestones, plus fallback designs |
| [devlog.md](docs/devlog.md) | How the design evolved and how it was tested, kept for a write-up |

## Status

M0 passed. **M1 in progress:** the Protobuf interface (`interface/`, `controller/deck_msgs`) and the simulated robot (`robot/deck_sim`) are built and tested. Next: `deck_gateway`, then the Deck app. The docs are the spec.

```bash
scripts/setup_dev.sh            # once: install dependencies (--check to only report)
# Build and test what exists (from the repo root, ROS 2 Jazzy or Lyrical sourced)
colcon build --symlink-install && colcon test && colcon test-result --verbose
(cd controller && cargo test)
ros2 launch deck_sim sim.launch.py
```
