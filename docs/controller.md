# Controller app (Steam Deck, Rust)

## Goals

- A single self-contained binary. Installs on stock SteamOS with no root, no ROS, and no packages.
- Launches from Game Mode like a game and needs no keyboard.
- Driving screen: the robot's camera full-screen, and you can tell at a glance whether you're connected, whether the robot will move, and whether it's stopped.
- Status and actions are one button away, and never interfere with STOP.

## Stack

| Concern | Crate |
|---------|-------|
| Transport | `zenoh = "=1.10.2"` (`crates=` in `interface/ZENOH_VERSION`; `Cargo.lock` must hold `quinn-proto` ≥ 0.11.18, checked by a test like `robot/deck_router/tests/pins.rs`) with features `transport_quic`, `transport_tls`, `transport_quic_datagram` ([interface.md § Connection](interface.md#connection)) |
| Messages | `prost` (generated from `interface/proto` by `prost-build`) |
| Window, GUI, touch | `eframe` / `egui` (glow backend) |
| Gamepad | `gilrs` (Steam Input virtual gamepad, SDL mapping database) |
| JPEG decode | `zune-jpeg` |
| Thread handoff | `arc-swap`, `crossbeam-channel` |
| CLI / config | `clap`, `serde` + `toml` |

## Crates

```
controller/
├── Cargo.toml                 # workspace; zenoh pinned to interface/ZENOH_VERSION crates=
├── COLCON_IGNORE
├── deck_msgs/                 # build.rs: protox + prost over ../../interface/proto  ✅
├── deck_core/                 # no UI dependency  ✅ (M1 parts)
│   ├── src/lib.rs             # DeckCore::start(profile, log, pad) → snapshot(); session + control threads,
│   │                          #   Info/robot-ID/version check, clear_stop worker, shutdown(); probe()
│   ├── src/input.rs           # gilrs snapshot → wire axes, deadzones, STOP/CLEAR combos, Menu tap,
│   │                          #   panel-neutral; StopLatch (released_seq handshake). Pure, unit-tested.
│   ├── src/health.rs          # RobotState → link up/lost, RTT, clock offset, ownership, overlay. Pure, tested.
│   ├── src/profiles.rs        # pair bundle import (checks id = SHA-256 of ca.pem), list, forget, active
│   ├── src/events.rs          # JSON-lines event log, rotated
│   └── tests/                 # pins.rs (zenoh pin, quinn-proto >= 0.11.18), profiles.rs
│   ├── src/video.rs           # rj/v1/video → newest-only slot → zune-jpeg decode thread → latest RGB + stats ✅
│   (later: status.rs M3, actions in queries M4; locator/links/pairing traits post-MVP)
├── deck_app/                  # egui Drive screen + CLI (binary: steamdeck-remote-joy)  ✅ (M1)
│   ├── src/main.rs            # CLI: run, robot add/list/forget, probe, log
│   ├── src/ui.rs              # Drive screen: full-screen video, overlay strips (status, stats toggle), banners, clear progress, touch STOP, Menu screen, Inputs screen (development)
│   └── src/pad.rs             # gilrs → RawPad (joy button order)
├── scripts/                   # deploy_deck.sh (dev deploy), run.sh, .desktop
└── packaging/                 # M5: build.sh, install.sh, uninstall.sh, steam_input/
```

`deck_core` holds everything that matters for safety and networking, so the UI could be replaced later without touching it.

**Dev deploy** (until M5 packaging): `controller/scripts/deploy_deck.sh [deck@steamdeck]` builds here and copies the binary, `run.sh` and the `.desktop` to `~/steamdeck-remote-joy` on the Deck. `BUNDLE=<file>` also runs `robot add --bundle`, and `ADD_TO_STEAM=1` adds the Non-Steam Game entry. Every deploy also installs the Library artwork (a little TurtleBot-style robot; `controller/assets/steam/`, drawn by `make_art.py`) into Steam's grid folder for that entry with `scripts/steam_art.py`. Steam shows it after a restart. The icon is only set while Steam is closed, because Steam rewrites `shortcuts.vdf` from memory.

## Connection

**MVP: one robot on the same network as the Deck.** The app doesn't manage Wi-Fi: the Deck joins the network in SteamOS settings.

- **On launch,** `connect()` uses the active profile:
  1. open a Zenoh session to the profile's endpoints with its certificates;
  2. query `Info` and check `Info.robot_id` against the profile. A mismatch aborts with a "wrong robot" error.

  The robot is STOPPED on arrival (new-owner rule), so the operator sees the camera and the robot's name before clearing STOP.
- **Reconnect:** after a drop, the app retries automatically (backoff up to 2 s) and shows LINK LOST until it's back. It keeps the same `session_id` for the whole app run. After a short drop the robot is in **REARM** (the banner says "release L2, then press it again"). After a drop over 1 s it's STOPPED (`link_lost`).
- **`disconnect()`** runs on quit: send `Stop` (reliable), withdraw the liveliness token, close the session. The robot stops and releases control straight away.
- **Two links:** each profile endpoint becomes one QUIC link per priority band (`prio=0-2` for control, stop, state and video feedback; `prio=3-7` for video and status) in a Zenoh `allOf` group, with `max_links: 2`. Endpoints must carry `mixed_rel=1` and `multistream=1` ([interface.md § Connection](interface.md#connection)).
- **Video feedback:** every 200 ms once a frame has arrived, the control thread sends `VideoFeedback` (newest frame id, its age, frames received, counted as they arrive), so `deck_video` can adapt its rate. It's sent with or without a gamepad.
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

Steam Input presents the Deck's controls as a standard gamepad, so the app is launched **through Steam** with a gamepad layout (the default Gamepad template works: L2 is the deadman). With no gamepad, the app shows "No controller — launch from Steam" and sends nothing.

| Input | Handled on | Effect |
|-------|-----------|--------|
| Sticks, L2, RB | Robot (in `Control`) | Driving via `teleop_twist_joy`: L2 (left trigger) = deadman, RB = turbo (only with L2), left stick ↕ = forward, right stick ↔ = turn |
| A/X/Y, LB, D-pad, L3/R3, R2, right stick ↕ | Robot (in `Control`) | **Free buttons** for robot-side `/joy` listeners, labelled by the robot's button hints ([interface.md § Reserved and free buttons](interface.md#reserved-and-free-buttons)) |
| **B** / touchscreen **STOP** | Deck, **on every screen** | Sends `Stop` (reliable), latches locally, and sets `Control.stop` while latched |
| **View + Menu** held 1 s, sticks centered, L2 released | Deck | Drops the local latch, sends `clear_stop` with `released_seq` ([interface.md § Rules](interface.md#rules)), and shows the result (e.g. "sticks not centered"). A refusal re-latches locally. |
| **Menu** tap (released without View) | Deck | Opens or closes the Menu screen (status, Inputs (dev), Close, Quit) |
| In panel: D-pad / A / touch | Deck | Navigate / select. **Driving input is sent neutral while the panel is open.** |

Input processing: 0.12 radial stick deadzone (rescaled; the Deck's sticks rest at up to ±0.08), triggers normalized to 0..1, and X axes negated so left = +1. gilrs sees a single `Steam Virtual Gamepad`. The back grips only produce input once the Steam Input layout maps them to one of the gamepad's 15 buttons (e.g. L4 → LB): the virtual pad has no grip buttons, and `pad.rs` reads bits 0–14 only. R4/R5 are unmapped today, so they send nothing (docs/next.md). The Inputs screen greys out bits 15–20 as "not read".

## Threads

| Thread | Job |
|--------|-----|
| **Control** (20 ms tick, plus wakes on input) | Owns `gilrs`. Polls, runs combos, reads the UI's touch-STOP and panel-open flags, puts `Control` (and `Stop` on press). Never waits on the UI. |
| **Zenoh callbacks** | `robot_state` and `status` go into `ArcSwap`s. `video` goes into a capacity-1 channel that drops the older frame. They do no heavy work. |
| **Video decode** | JPEG → RGBA (`zune-jpeg`), stores the latest frame with stats, calls `ctx.request_repaint()` |
| **Query worker** | `info` on connect, `clear_stop`, `action/<id>`, each with a timeout. Results go to the UI. |
| **UI** (main) | Uploads the frame to a texture when it changes, draws screens |

## Screens (1280×800)

**Version:** `steamdeck-remote-joy --version` and the Menu screen show the package version plus the git commit the build came from, e.g. `0.1.0 (9988d30 2026-11-02)`, with `-dirty` when built from uncommitted changes (`deck_app/build.rs`). `deploy_deck.sh` prints it after each deploy.

**Scale:** every screen is laid out for **1280×800 points**, and the app zooms so the window is 1280 points wide (1 point = 1 pixel on the Deck). Without that, egui uses winit's display scale, and on the Deck that's **2.17**: gamescope's Xwayland reports the panel's real size (100 × 150 mm, about 210 DPI against the desktop's 96), so a 1280×800 layout came out 2.17× too big and ran off the screen (seen 2026-11-02). `--screenshot DIR` (development) saves each screen as a PNG at the window's real size and prints the scale, to check layouts on the Deck: `DISPLAY=:0 ./steamdeck-remote-joy --screenshot /tmp/shots` over SSH while Game Mode runs.

**Drive** (always underneath): the video as large as the screen allows (scaled to fit, never cropped: a 4:3 camera is 1067×800 at full height), with semi-transparent strips over it:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│▒● CONNECTED  mybot   DEADMAN ●  TURBO ○                  Deck ⏻ 74% [stats ▾]▒│  top strip (always)
│▒RTT 6 ms   video 30 fps   A Horn · X Lights   B STOP · View+Menu 1 s clear ··▒│  stats strip (toggled)
│▒▒▒▒▒▒▒▒▒▒▒▒ STOP (teleop): STOP pressed · hold View + Menu 1 s … ▒▒▒▒▒▒▒▒▒▒▒▒▒│  banner (when there is one)
│          ( robot camera, full height )                                       │
│                                                                ┌──────────┐  │
│ weak link: reduced video                                       │   STOP   │  │
└────────────────────────────────────────────────────────────────┴──────────┴──┘
```

- **Top strip:** status, robot name, DEADMAN and TURBO, the Deck's battery, and the **stats** button. Tap the status to toggle the debug overlay.
- **Stats strip** (the stats button, off at start): RTT, video fps, the video level when reduced, the robot's **button hints** from `Info.button_hints`, and the help line when it fits. Hints are dimmed while those buttons are gated (STOPPED, REARM, BUSY, panel open), so the operator can see they won't work.
- **Touch STOP:** 170×64, semi-transparent red, bottom right over the video, on every screen.
- The top strip will also show one "pinned" status item, e.g. robot battery, chosen by the robot's status config (M3).

**Menu screen** (Menu button): a semi-transparent card over the video. Driving input is neutral while it's open; STOP (B or the touch button, which stays visible) still works.
- **Status:** connection, robot, session, round trip, video (fps, % received, level), Deck battery, **app** version and **interface** versions (the app's and the robot's `Info.interface_version`.`interface_minor`, amber when they differ). Robot status items (M3) go here: `Status.items` grouped by `group`, with a coloured level (OK/WARN/ERROR) and STALE when no update has arrived for more than 3 s.
- **Actions** (M4): buttons from `Info.actions`. Ones with `confirm` ask "A to confirm" first. Ones with `moves_robot` are greyed out while STOPPED or REARM, and all of them are greyed out when this Deck isn't in control. The reply is shown as a notification.
- **Inputs (development)**, **Close (Menu)** and **Quit (stops the robot)**.

**Inputs screen** (development, from the Menu screen; Back returns): its own screen with nothing from the Drive screen. Every axis and button with its index (the `Control`/`/joy` index), name and live value, read from the pad before the Menu's neutral output, so inputs can be checked without moving the robot. Also shows the controller name and egui's display scale. Driving stays neutral; B still sends STOP.

The STOP button stays visible.

When the robot has lowered the video rate (`VideoFrame.level` > 0), the bottom-left corner says "weak link: reduced video".

| Overlay state | When | Look |
|---------------|------|------|
| NO CONTROLLER | No gamepad | Grey, centered message |
| CONNECTING | Zenoh session not up, or no `RobotState` yet | Amber |
| CONNECTED | `RobotState` within 300 ms while we own the robot (50 Hz echoes), or within 600 ms otherwise (5 Hz heartbeat) | Green |
| LINK LOST | No `RobotState` within that time | Red bar |
| BUSY | `RobotState.owner_session` is another session's (the gateway never sends `BUSY`) | Amber, "another Deck is driving" |
| STOPPED | Local latch or `state = STOPPED` | Red border + banner "STOP (teleop)" showing `stop_reason` |
| REARM | `state = REARM` | Amber banner: "release and press L2 to resume" |
| NO VIDEO | No frame for 1 s | Last frame dimmed with "NO VIDEO" (the controls still work) |
| VERSION | `Info.interface_version` not supported | Full-screen "update the app" |

The Deck's own battery is read from `/sys/class/power_supply`.

### Debug overlay

Tap the status in the top strip to toggle it. It shows stats over the last 5 s:

```
capture→encoded   9 ms     frames received   98.7 %   (gaps in frame_id)
encoded→received  4 ms     fps               30.0
received→shown    7 ms     frame size        18 KB
capture→shown    20 ms     control RTT       6 ms (min 3)   clock offset ±1 ms
session …  controls sent …  pad: …  video level 0
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
steamdeck-remote-joy bench [--seconds N]      # headless link test: neutral Control, prints RTT, video, level
steamdeck-remote-joy log [--tail N]           # show the local event log
```

## Testing

- `deck_core::input`: deadzones, STOP/CLEAR combos, Menu tap vs. combo, panel-neutral, on synthetic snapshots.
- `deck_core::health`: RTT and offset estimation, ownership and state transitions, on synthetic `RobotState` streams.
- `deck_core::session`: robot-ID mismatch is refused. Reconnect after an injected drop keeps the `session_id` and re-declares liveliness. Quit sends `Stop` and drops liveliness before closing.
- Integration: a fake robot (a Rust test binary on a local Zenoh router) with injected stalls and dropped frames.
- On the Deck: `probe`, then the commissioning checklist.
