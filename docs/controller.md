# Controller app (Steam Deck, Rust)

## Goals

- A single self-contained binary. Installs on stock SteamOS with no root, no ROS, and no packages.
- Launches from Game Mode like a game and needs no keyboard.
- Driving screen: the robot's camera full-screen, and you can tell at a glance whether you're connected, whether the robot will move, and whether it's stopped.
- Status and actions are one button away, and never interfere with STOP.

## Stack

| Concern | Crate |
|---------|-------|
| Transport | `zenoh = "=1.10.1"` (matches Jazzy's vendored version) with features `transport_quic`, `transport_tls`, `transport_quic_datagram` ([interface.md § Connection](interface.md#connection)) |
| Messages | `prost` (generated from `interface/proto` by `prost-build`) |
| Window, GUI, touch | `eframe` / `egui` (glow backend) |
| Gamepad | `gilrs` (Steam Input virtual gamepad, SDL mapping database) |
| JPEG decode | `zune-jpeg` |
| Thread handoff | `arc-swap`, `crossbeam-channel` |
| CLI / config | `clap`, `serde` + `toml` |

## Crates

```
controller/
├── Cargo.toml                 # workspace; zenoh version pinned (see interface/ZENOH_VERSION)
├── COLCON_IGNORE
├── crates/
│   ├── deck_msgs/             # build.rs: prost-build over ../../interface/proto
│   ├── deck_core/             # no UI dependency
│   │   ├── session.rs         # Zenoh session (client, TLS from profile), connect()/disconnect(), auto-reconnect
│   │   ├── profiles.rs        # robot profiles keyed by robot ID, stored under ~/.config/.../robots/
│   │   ├── locator.rs         # trait RobotLocator; StaticLocator only in the MVP
│   │   ├── links/             # trait Link; Static only in the MVP (Wi-Fi AP, Bluetooth post-MVP)
│   │   ├── pairing/           # trait PairingMethod; FileBundle only in the MVP (online enrollment post-MVP)
│   │   ├── input.rs           # gilrs → snapshot, deadzones, STOP/CLEAR combos, menu-neutral (pure, unit-tested)
│   │   ├── control.rs         # control thread: 50 Hz + on change, Stop on press
│   │   ├── health.rs          # RobotState → link state, RTT, clock offset, ownership
│   │   ├── video.rs           # frame queue (capacity 1) → decode thread → latest RGBA frame + stats
│   │   ├── status.rs          # latest Status, items marked stale when old
│   │   ├── queries.rs         # info, clear_stop, actions (worker thread, results to UI)
│   │   ├── events.rs          # local event log (JSON lines)
│   │   └── lib.rs             # DeckCore::start(config) → handle the UI reads from
│   └── deck_app/              # egui screens + CLI (binary: steamdeck-remote-joy)
└── packaging/                 # build.sh, install.sh, uninstall.sh, .desktop, steam_input/
```

`deck_core` holds everything that matters for safety and networking, so the UI could be replaced later without touching it.

## Connection

**MVP: one robot on the same network as the Deck.** The app doesn't manage Wi-Fi: the Deck joins the network in SteamOS settings.

- **On launch,** `connect()` uses the active profile:
  1. open a Zenoh session to the profile's endpoints with its certificates;
  2. query `Info` and check `Info.robot_id` against the profile. A mismatch aborts with a "wrong robot" error.

  The robot is STOPPED on arrival (new-owner rule), so the operator sees the camera and the robot's name before clearing STOP.
- **Reconnect:** after a drop, the app retries automatically (backoff up to 2 s) and shows LINK LOST until it's back. It keeps the same `session_id` for the whole app run. After a short drop the robot is in **REARM** (the banner says "release and press LB to resume"). After a drop over 1 s it's STOPPED (`link_lost`).
- **`disconnect()`** runs on quit: send `Stop` (reliable), withdraw the liveliness token, close the session. The robot stops and releases control straight away.
- **Wrong-robot guard:** the robot's name is shown prominently in the top bar, and the STOP-clear prompt names it ("Clear STOP on **mybot**?").

**Extension points**, present in the MVP with one simple implementation each, so post-MVP features slot in ([future.md § Connecting, switching and pairing](future.md#connecting-switching-and-pairing-post-mvp)):

```rust
trait RobotLocator { fn locate(&self, profile: &Profile) -> Option<Sighting>; }  // MVP: StaticLocator
trait Link {                                       // MVP: Static (already on the network)
    fn up(&mut self, profile: &Profile) -> Result<Vec<Endpoint>>;
    fn down(&mut self);
}
trait PairingMethod {                              // MVP: FileBundle
    fn kind(&self) -> MethodKind;                  // Bundle | SecretFromRobot | SecretToRobot
    fn ui(&mut self, ui: &mut egui::Ui) -> MethodState;
}
```

## Controls

Steam Input presents the Deck's controls as a standard gamepad, so the app is launched **through Steam** with a gamepad layout where **L4 → LB**. With no gamepad, the app shows "No controller — launch from Steam" and sends nothing.

| Input | Handled on | Effect |
|-------|-----------|--------|
| Sticks, LB, RB | Robot (in `Control`) | Driving via `teleop_twist_joy`: LB = deadman, RB = turbo, left stick ↕ = forward, right stick ↔ = turn |
| A/X/Y, D-pad, L3/R3, triggers, right stick ↕ | Robot (in `Control`) | **Free buttons** for robot-side `/joy` listeners, labelled by the robot's button hints ([interface.md § Reserved and free buttons](interface.md#reserved-and-free-buttons)) |
| **B** / touchscreen **STOP** | Deck, **on every screen** | Sends `Stop` (reliable), latches locally, and sets `Control.stop` while latched |
| **View + Menu** held 1 s, sticks centered, LB/RB released | Deck | Drops the local latch, sends `clear_stop` with `released_seq` ([interface.md § Rules](interface.md#rules)), and shows the result (e.g. "sticks not centered"). A refusal re-latches locally. |
| **Menu** tap (released without View) | Deck | Opens or closes the panel |
| In panel: D-pad / A / touch | Deck | Navigate / select. **Driving input is sent neutral while the panel is open.** |

Input processing: 0.12 radial stick deadzone (rescaled; the Deck's sticks rest at up to ±0.08), triggers normalized to 0..1, and X axes negated so left = +1. gilrs sees a single `Steam Virtual Gamepad`. The back grips only produce input once the Steam Input layout maps them (L4 → LB).

## Threads

| Thread | Job |
|--------|-----|
| **Control** (20 ms tick, plus wakes on input) | Owns `gilrs`. Polls, runs combos, reads the UI's touch-STOP and panel-open flags, puts `Control` (and `Stop` on press). Never waits on the UI. |
| **Zenoh callbacks** | `robot_state` and `status` go into `ArcSwap`s. `video` goes into a capacity-1 channel that drops the older frame. They do no heavy work. |
| **Video decode** | JPEG → RGBA (`zune-jpeg`), stores the latest frame with stats, calls `ctx.request_repaint()` |
| **Query worker** | `info` on connect, `clear_stop`, `action/<id>`, each with a timeout. Results go to the UI. |
| **UI** (main) | Uploads the frame to a texture when it changes, draws screens |

## Screens (1280×800)

**Drive** (always underneath):

```
┌────────────────────────────────────────────────────────────────────────────┐
│ ● CONNECTED  mybot   RTT 6 ms   video 30 fps   ⚡ 87% robot          ⏻ 74% │
│                                                                            │
│                         ( robot camera, scaled to fit )                    │
│                                                                            │
│                                                          ┌──────────────┐  │
│ DEADMAN ●  TURBO ○                                       │     STOP     │  │
│ A Horn · X Lights · Y Speed mode                         └──────────────┘  │
│ B = STOP · View+Menu 1 s = clear · Menu = panel                            │
└────────────────────────────────────────────────────────────────────────────┘
```

The top bar can show one "pinned" status item, e.g. robot battery, chosen by the robot's status config. The help line shows the robot's **button hints** from `Info.button_hints`. It's dimmed while those buttons are gated (STOPPED, REARM, BUSY, panel open), so the operator can see they won't work.

**Panel** (Menu): a right-side overlay with two tabs, Status and Actions (a Robots tab comes post-MVP).
- **Status:** `Status.items` grouped by `group`, with a coloured level (OK/WARN/ERROR) and STALE when no update has arrived for more than 3 s.
- **Actions:** buttons from `Info.actions`. Ones with `confirm` ask "A to confirm" first. Ones with `moves_robot` are greyed out while STOPPED or REARM, and all of them are greyed out when this Deck isn't in control. The reply is shown as a notification.

The video and the STOP button stay visible.

| Overlay state | When | Look |
|---------------|------|------|
| NO CONTROLLER | No gamepad | Grey, centered message |
| CONNECTING | Zenoh session not up, or no `RobotState` yet | Amber |
| CONNECTED | `RobotState` for our session within 300 ms | Green |
| LINK LOST | No `RobotState` for 300 ms | Red flashing bar |
| BUSY | `RobotState.state = BUSY` | Amber, "another Deck is driving" |
| STOPPED | Local latch or `state = STOPPED` | Red border + banner "STOP (teleop)" showing `stop_reason` |
| REARM | `state = REARM` | Amber banner: "release and press LB to resume" |
| NO VIDEO | No frame for 1 s | Last frame dimmed with "NO VIDEO" (the controls still work) |
| VERSION | `Info.interface_version` not supported | Full-screen "update the app" |

The Deck's own battery is read from `/sys/class/power_supply`.

### Debug overlay

Tap the top bar to toggle it. It shows stats over the last 5 s:

```
capture→encoded   9 ms     frames received   98.7 %   (gaps in frame_id)
encoded→received  4 ms     fps               30.0
received→shown    7 ms     frame size        18 KB
capture→shown    20 ms     control RTT       6 ms (min 3)   clock offset ±1 ms
```

This doesn't include camera exposure or driver delay before the capture timestamp. For that, use the glass-to-glass test in [deployment.md](deployment.md#video-tuning).

## Event log

`deck_core::events` appends one JSON line per event to `~/.local/state/steamdeck-remote-joy/events.log`, rotated at 5 MB with 3 files kept. Events:
- connect, disconnect, LINK LOST and recovery;
- STOP (with source) and its clear (accepted or refused, with reason);
- REARM;
- BUSY;
- actions (id, result);
- version mismatch.

Each line carries Deck wall-clock time, monotonic time, robot ID, `session_id` and the latest RTT, so it lines up with the gateway's log. `steamdeck-remote-joy log [--tail N]` prints it.

## Configuration

```
~/.config/steamdeck-remote-joy/          (mode 0700)
├── config.toml                          # active_robot, stick_deadzone, ...
└── robots/<robot_id_hex>/
    ├── profile.toml                     # name, trust domain, endpoints
    ├── ca.pem
    ├── client.pem
    └── client.key
```

```toml
# robots/<id>/profile.toml
name = "mybot"
trust = "robot"                          # "robot" (per-robot CA); "fleet:<name>" post-MVP
endpoints = ["quic/192.168.1.50:7450"]   # robot's fixed IP on the shared network (DHCP reservation)
```

The layout already holds one directory per robot, so post-MVP multi-robot support (and `[wifi]`/`[bluetooth]` link settings) needs no migration. In the MVP, `config.toml` names the single active robot.

## CLI

```
steamdeck-remote-joy                          # run the app (what Steam launches)
steamdeck-remote-joy robot add --bundle FILE  # file pairing method; becomes the active robot
steamdeck-remote-joy robot list | forget NAME
steamdeck-remote-joy probe                    # connect, print Info, RTT, state, video fps, then exit
steamdeck-remote-joy log [--tail N]           # show the local event log
```

## Testing

- `deck_core::input`: deadzones, STOP/CLEAR combos, Menu tap vs. combo, panel-neutral, on synthetic snapshots.
- `deck_core::health`: RTT and offset estimation, ownership and state transitions, on synthetic `RobotState` streams.
- `deck_core::session`: robot-ID mismatch is refused. Reconnect after an injected drop keeps the `session_id` and re-declares liveliness. Quit sends `Stop` and drops liveliness before closing.
- Integration: a fake robot (a Rust test binary on a local Zenoh router) with injected stalls and dropped frames.
- On the Deck: `probe`, then the commissioning checklist.
