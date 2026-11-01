# Robot side

ROS 2 Jazzy (Ubuntu 24.04) or Lyrical (Ubuntu 26.04, the initial test robot), x86. `deck_gateway` and `deck_video` need `zenoh_cpp_vendor` **≥ 0.2.12 on Jazzy or ≥ 0.10.7 on Lyrical** (Zenoh 1.10.x, [deployment.md § Zenoh package version](deployment.md#zenoh-package-version)). The router is our own `deck_router`, which needs Rust to build. Three processes, started by one launch file:

| Process | Package | Language |
|---------|---------|----------|
| Zenoh router | `deck_router` (config in `deck_gateway`) | Rust, crates.io Zenoh 1.10.2 (O17) |
| `deck_gateway` | `deck_gateway` | C++ (rclcpp + zenoh-cpp) |
| `deck_video` | `deck_video` | C++ composable node |

Plus `teleop_twist_joy`, which turns `/joy` into `cmd_vel`.

## Packages

```
interface/                       # ament_cmake: remote_joy_interface
├── proto/remote_joy/v1/*.proto  # source of truth: control, info, status, action, video
│                                #   (also compiled by the Deck's Rust build, controller/deck_msgs)
├── include/remote_joy_interface/keys.hpp   # Zenoh key names and encodings
├── test/golden/                 # protoc-encoded vectors that pin the wire format for C++ and Rust
├── ZENOH_VERSION                # exported as REMOTE_JOY_ZENOH_VERSION
└── CMakeLists.txt               # protoc → libremote_joy_interface (C++), exported

robot/deck_gateway/              # ament_cmake
├── include/ src/
│   ├── core/                    # deck_gateway_core: NO ROS, NO Zenoh. Ownership, watchdog,
│   │                            #   stop latch, Control→Joy mapping, status formatting. gtest.
│   ├── gateway_node.cpp         # rclcpp node + zenoh-cpp session, wires core to ROS and Zenoh
│   ├── status_sources.cpp       # built-in readers for supported message types
│   └── actions.cpp              # Trigger/SetBool service clients
├── config/
│   ├── gateway.yaml             # status items, actions, timeouts
│   ├── router.json5             # Zenoh router: endpoints, TLS, ACL, payload limits
│   └── teleop_twist_joy.yaml
├── launch/robot.launch.py
├── scripts/pair                 # certificate tool (Python, python3-cryptography): `ros2 run deck_gateway pair`
└── test/

robot/deck_router/               # ament_cmake wrapper around cargo: the Zenoh router (O17)
├── src/main.rs                  # load router.json5 + ZENOH_CONFIG_OVERRIDE, validate, run
├── tests/pins.rs                # zenoh == interface/ZENOH_VERSION crates=, quinn-proto >= 0.11.18
└── Cargo.lock                   # built with --locked

robot/deck_video/                # ament_cmake, rclcpp_components
└── src/video_publisher.cpp      # component deck_video::VideoPublisher

robot/deck_sim/                  # ament_python: development and CI without a real robot
├── deck_sim/fake_robot.py       # BatteryState (slow drain), Odometry from cmd_vel, /diagnostics, /dock + /lights services,
│                                #   and a /joy listener (A = "horn" log line, X = lights) to exercise button hints
├── deck_sim/test_camera.py      # moving test pattern on /camera/image_raw
├── deck_sim/model.py            # ROS-free battery/odometry models (pytest)
├── config/gateway_sim.yaml      # gateway status items, actions and hints matching the sim
└── launch/sim.launch.py
```

## Network

**MVP:** the robot and the Deck are on the same network (an existing 5 GHz Wi-Fi network, a dedicated router, or the robot's own hotspot joined from SteamOS settings). The robot needs a stable address there, which goes into `pair --host`. Prefer a **fixed IP from a DHCP reservation**. A `.local` hostname only works if SteamOS resolves it (open item O14). The router listens on all interfaces. See [deployment.md § Network setup](deployment.md#network-setup).

*Post-MVP:* one access point per robot and an optional Bluetooth backup ([future.md](future.md#connecting-switching-and-pairing-post-mvp)).

## Zenoh router

- Listens on `quic/0.0.0.0:7450?mixed_rel=1` (TLS, client certificates required; `mixed_rel=1` is mandatory, see [interface.md § Connection](interface.md#connection)) for Decks, and on `tcp/127.0.0.1:7451` for the local processes.
- Access control as in [interface.md § Security](interface.md#security). Payload size limits per key.
- Multicast scouting off. It's a separate key space from any `rmw_zenoh` router the robot may run for ROS itself (that one uses 7447 by default).
- Started by the launch file and respawned if it exits.
- Binary: **`deck_router`** (`robot/deck_router`), started with `ZENOH_ROUTER_CONFIG_URI=<share>/config/router.json5` and the TLS file paths in `ZENOH_CONFIG_OVERRIDE` (`key=value;...`), the same interface as `rmw_zenohd`. It's a separate process and port from any `rmw_zenohd` the robot runs for ROS itself.
  - **Why not `rmw_zenohd`:** it reads the same config, but every current ROS zenoh-c locks `quinn-proto` 0.11.17, which crashes the router under Wi-Fi load with `mixed_rel=1` (O17). `deck_router` is built from crates.io Zenoh 1.10.2 with `cargo --locked`, and a test fails the build if `quinn-proto` is older than 0.11.18. Switching back is a launch file change.
  - It refuses to start in anything but router mode, or with a QUIC endpoint missing `mixed_rel=1`. `deck_router --check <file>` validates a config without starting.
- `router.json5`: 2 s lease, `access_control` by `cert_common_names`, `low_pass_filter` size limits per key, and on the robot an optional `nftables` rule that marks outgoing UDP 7450 with DSCP (open item O4).

## `deck_gateway`

### Safety

| ROS output | Type | QoS | |
|------------|------|-----|--|
| `joy` | `sensor_msgs/Joy` | default | One per accepted `Control`. Neutral at 50 Hz while `NO_CONTROL` or `STOPPED`. 6 axes, 21 buttons. |
| `steamdeck/stop_latched` | `std_msgs/Bool` | reliable, transient-local | On change |
| `steamdeck/owner_connected` | `std_msgs/Bool` | reliable, transient-local | On change |

| Service | Type | |
|---------|------|--|
| `steamdeck/set_stop` | `std_srvs/Trigger` | Lets other robot software latch STOP |

Diagnostics: publishes its own status to `/diagnostics` (via `diagnostic_updater`, 1 Hz) as `deck_gateway: link`:
- owner session and certificate CN;
- state and `stop_reason`;
- control rate and RTT;
- watchdog trips, REARM and `link_lost` counts;
- last action and its result.

Each safety event (STOP set or cleared, REARM, owner change, watchdog trip, action run or refused) is also logged at INFO with the session ID, so `ros2 bag` or journald keep a record.

Zenoh side:
- Subscribes to `rj/v1/control`, `rj/v1/stop`, and liveliness `rj/v1/deck/*`.
- Publishes `rj/v1/robot_state`: on each owner `Control`, and at 5 Hz otherwise.
- Answers the queryables `rj/v1/clear_stop` and `rj/v1/info`.

Rules (ownership, 250 ms watchdog, REARM after short drops, stop latch, starts latched, action gating) are in [interface.md § Rules](interface.md#rules). **STOP is teleop-only:** it neutralizes `/joy` and gates `moves_robot` actions. Other `cmd_vel` sources can opt in through `steamdeck/stop_latched`, for example as a `twist_mux` lock. The node uses a steady-clock timer at 50 Hz for neutral output, independent of message arrival. It also applies the **late-control guard** (`max_control_delay_ms`, default 150).

### Status

Publishes `rj/v1/status` at `status_rate_hz`, built from configured items. **Supported source types** (no generic introspection in v1, see [future.md](future.md#generic-telemetry)):

| Type | Fields you can pick |
|------|---------------------|
| `sensor_msgs/BatteryState` | `percentage`, `voltage`, `current`, `power_supply_status` |
| `nav_msgs/Odometry` | `linear_x`, `angular_z`, `speed` (= ‖linear‖) |
| `geometry_msgs/Twist`, `TwistStamped` | `linear_x`, `angular_z` |
| `std_msgs/Bool`, `String`, `Float32/64`, `Int32/64` | `data` |
| `diagnostic_msgs/DiagnosticArray` (`/diagnostics`) | All statuses → one item each (level + message), grouped "Diagnostics" |

Items are marked `STALE` if their topic hasn't published for 3 s.

### Two kinds of robot commands

| Use | For | How |
|-----|-----|-----|
| **`/joy` listeners** | Quick, frequent, low-risk controls while driving: horn, lights toggle, speed mode, camera tilt, gripper | Robot nodes (e.g. `joy_teleop`) bound to **free buttons** ([interface.md § Reserved and free buttons](interface.md#reserved-and-free-buttons)). Instant, and any service, topic or ROS action works. Gated by `/joy` automatically. Describe them with `button_hints` so the Deck can show labels. |
| **Gateway actions** | Deliberate commands, ones with consequences, or ones that need feedback: dock, start or abort a mission, reset faults, shutdown | `actions` in `gateway.yaml` (below). Picked from the Deck panel, with an optional confirm step and a visible result. Non-motion actions also work while STOPPED. |

### Button hints

`button_hints` in `gateway.yaml` maps free buttons to short labels. They're sent in `Info.button_hints`, and the Deck shows them on the Drive screen. The gateway **only labels** these buttons: the robot's own `/joy` listeners do the actual work. At startup the gateway logs a warning for any hint on a reserved button, and for any configured listener binding it knows about (e.g. `teleop_twist_joy`'s enable buttons) that collides with a hint.

### Actions

Configured actions become `rj/v1/action/<id>` queryables and are listed in `Info`. Only the controlling Deck may run them. Actions with `moves_robot: true` are refused while STOPPED, REARM or NO_CONTROL. **Supported in v1:** `std_srvs/Trigger` and `std_srvs/SetBool` (with a fixed value). Long-running ROS actions (e.g. Nav2 goals) are listed in [future.md](future.md#long-running-ros-actions). Calls time out after 10 s, and the reply carries the service's `success`/`message`.

### Configuration (`gateway.yaml`)

```yaml
deck_gateway:
  ros__parameters:
    robot_name: mybot
    watchdog_ms: 250
    resume_grace_ms: 1000        # drops shorter than this resume via deadman re-press (REARM)
    deadman: LT                  # LT (L2, default), RT, LB or RB -> /joy button 21
    max_control_delay_ms: 150    # late-control guard: controls delayed more than this vs. the recent minimum are ignored
    status_rate_hz: 2.0
    status_items: |               # YAML string → parsed by the node
      - {key: battery.pct, label: Battery, topic: /battery_state, type: BatteryState,
         field: percentage, unit: "%", group: Power, pin: true, warn_below: 25, error_below: 10}
      - {key: speed, label: Speed, topic: /odom, type: Odometry, field: speed, unit: m/s, group: Motion}
      - {key: diagnostics, topic: /diagnostics, type: DiagnosticArray}
    actions: |
      - {id: dock, label: Dock, service: /dock, type: Trigger, confirm: true, moves_robot: true}
      - {id: lights_on, label: Lights on, service: /lights, type: SetBool, value: true}
    button_hints: |               # labels only; robot-side /joy listeners implement them
      - {button: A, label: Horn}
      - {button: X, label: Lights}
      - {button: Y, label: Speed mode}
```

The only required settings for driving are `robot_name` and the router's TLS files, which `pair` creates. Status items, actions and button hints are optional.

## `deck_video`

A composable node, `deck_video::VideoPublisher`.

| Parameter | Default | |
|-----------|---------|--|
| `image_topic` | `/camera/image_raw` | `sensor_msgs/Image` |
| `max_width` | `480` | Downscaled, keeping aspect ratio |
| `fps` | `30` | Upper limit |
| `jpeg_quality` | `70` | |

Behavior:
- **On demand:** a Zenoh matching listener on `rj/v1/video` (`Publisher::declare_matching_listener`, part of zenoh-cpp's stable API on the zenoh-c backend). With no Deck subscribed, it unsubscribes from the camera and costs nothing.
- **No stale frames:** sensor-data QoS (best effort, **depth 1**). A timer at `fps` takes the latest image, resizes it, and JPEG-encodes it (OpenCV with libjpeg-turbo). If encoding falls behind, frames are skipped.
- **Zero-copy option:** load it into the camera driver's component container to get images in-process. A camera-driver crash then also stops video, which is fine because video isn't on the safety path. Default: its own container process.
- **Timestamps:** capture time converted from the image's ROS `header.stamp` to the monotonic clock (`steady_now − (ros_now − stamp)`), plus capture → encode time, written into `VideoFrame`.
- About 15–25 KB per frame, roughly 5 Mbps at the defaults.

### Camera driver settings

- Set the driver's frame **buffer/queue count to 1–2** if it has one (defaults of 4 or more add 100+ ms).
- Run the camera at the fps you stream at, or at most 2×. Check with `v4l2-ctl --list-formats-ext`.

## Launch

```bash
ros2 launch deck_gateway robot.launch.py tls_dir:=<dir with ca.pem robot.pem robot.key>
```

Starts `deck_router`, `deck_gateway` (`gateway_node`) and `teleop_twist_joy`, plus `deck_video` from M2 (own container, or `video_container:=<name>` to load it into an existing one). The router and the gateway are respawned after 1 s if they exit. The gateway restarts with STOP latched (`startup`), so a respawn never resumes driving by itself. Teleop uses `config/teleop_twist_joy.yaml`:

```yaml
teleop_twist_joy_node:
  ros__parameters:
    require_enable_button: true
    enable_button: 21         # "deadman held", made by deck_gateway (default deadman: L2)
    enable_turbo_button: 10   # RB: the gateway only passes it while the deadman is held
    axis_linear:  {x: 1}      # left stick Y
    scale_linear: {x: 0.5}
    scale_linear_turbo: {x: 1.0}
    axis_angular: {yaw: 2}    # right stick X
    scale_angular: {yaw: 1.0}
    scale_angular_turbo: {yaw: 2.0}
```

Speeds are placeholders. The parameter names were confirmed on Lyrical in the M0 demo.

| Launch arg | Default | |
|---|---|---|
| `config_dir` | `~/.config/deck_gateway` | `pair`'s state. The robot name comes from its `pairing.json`, and `robot_id` (SHA-256 of `tls/ca.pem`) is passed to the gateway for `Info`. |
| `tls_dir` | `<config_dir>/tls` | Router TLS files `ca.pem`, `robot.pem`, `robot.key`. The launch fails with a clear message if any is missing. |
| `router_config` | `<config_dir>/router.json5` if `pair` made one, else the installed template | |
| `gateway_config` | installed `config/gateway.yaml` | |
| `use_router` | `true` | `false` to use a router started elsewhere (local endpoint `tcp/127.0.0.1:7451`) |
| `use_teleop`, `use_video` | `true` | |
| `cmd_vel_topic` | `cmd_vel` | |
| `cmd_vel_stamped` | `true` | sets `teleop_twist_joy`'s `publish_stamped_twist` |
| `image_topic`, `video_container` | `/camera/image_raw`, none | for `deck_video` (M2) |

### Testing tool: `fake_deck`

`ros2 run deck_gateway fake_deck [--endpoint E] <command>` is a stand-in Deck that speaks the real interface. By default it connects to the router's local endpoint, so no TLS is needed. It prints one line per result (`STATE ...`, `CLEAR ok=1 ...`, `SUMMARY states=STOPPED>OK ... rtt_ms_avg=...`).

| Command | Does |
|---|---|
| `info` | query `rj/v1/info` |
| `drive [s] [ly] [rx]` | neutral, `clear_stop`, L2 + sticks for `s` seconds, release |
| `gap [before] [silence_ms] [after]` | drive, go silent, return with the deadman held, release, re-press (REARM or `link_lost`) |
| `hold [s] [buttons_hex]` | send a button mask without clearing (e.g. as a second Deck: BUSY) |
| `stop`, `clear`, `watch [s]` | publish `rj/v1/stop`; neutral + `clear_stop`; print state changes |

**Drive base (not chosen yet, open item O13).** `cmd_vel_stamped` sets `teleop_twist_joy`'s `publish_stamped_twist`:
- `true` for ros2_control `diff_drive_controller` (`TwistStamped`);
- `false` for a custom driver taking `Twist`.

Either way the base must zero velocity on stale commands (e.g. `cmd_vel_timeout`) and enforce a hard speed limit (e.g. the controller's velocity limits) that doesn't depend on the teleop scales.

## Simulation

`ros2 launch deck_sim sim.launch.py` runs the full robot side with nothing physical attached:
- the router, `deck_gateway`, `deck_video` and `teleop_twist_joy`, by including `deck_gateway`'s `robot.launch.py` with `gateway_config:=deck_sim/config/gateway_sim.yaml`;
- **turtlesim** as the "base" (`cmd_vel_topic:=/turtle1/cmd_vel`, `cmd_vel_stamped:=false`, which also exercises the plain-`Twist` path);
- **`test_camera`**: a moving test pattern (frame counter, timestamp, a bar sweeping once per second, colour bars) at 640×480 and 30 fps, so dropped frames and latency are visible on the Deck. It replaces `image_publisher`, which isn't installed by default;
- **`fake_robot`**: a battery that slowly drains (faster while moving, charges when docked), odometry, `/diagnostics`, `/lights_on`, and `std_srvs/Trigger` `/dock` (`moves_robot`; teleports the turtle home) plus `SetBool` `/lights` services, so actions can be tested. On `/joy`, A logs a horn and X toggles the lights.

Launch arguments: `turtlesim` (default `true`; needs a display), `camera` (default `true`), `camera_fps`, and `gateway` (`auto` | `true` | `false`). With `auto`, the robot side is included only if `deck_gateway` is built. Otherwise the sim starts `teleop_twist_joy` itself, so anything publishing `/joy` drives the turtle.

A real Deck (or a laptop running `deck_app`) connects to it like a real robot, so M1–M4 can be built before the robot is ready. CI uses the same launch file headless for the `launch_testing` suites. Swap turtlesim for a Gazebo model when you need physics or a simulated camera.

## Testing

- `deck_gateway_core` (gtest, fake clock): ownership, watchdog timing, stop latch set/clear rules (including new-owner latch, REARM vs `link_lost` around `resume_grace_ms`, immediate release on liveliness loss, owner-only and `moves_robot` action gating), Control → Joy mapping, the clear-stop `released_seq` rule, status levels and staleness.
- `launch_testing` on top of `sim.launch.py`: a fake Deck (zenoh-cpp test client) → assert the `/joy` output, neutral on timeout, REARM, `clear_stop` refusals, an action round trip and its gating, and the `/diagnostics` contents.
- `deck_video`: encode sizes and fps limit on a test image stream, and on-demand subscribe/unsubscribe.
