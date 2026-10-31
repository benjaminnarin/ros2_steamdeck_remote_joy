# Deck ↔ robot interface (v1)

The Deck and the robot talk over **Zenoh**, with **Protobuf** payloads. The `.proto` files in `interface/proto/remote_joy/v1/` are the source of truth, and this document explains them.

## Connection

| | |
|--|--|
| Robot endpoint | `quic/<host>:7450`. TLS 1.3, the Deck must present a client certificate (mTLS). *Open item O3:* confirm stall behaviour, with `tls/<host>:7450` as the alternative. |
| Local endpoint | `tcp/127.0.0.1:7451`, used only by `deck_gateway` and `deck_video`. Not reachable from the network. |
| Deck mode | Zenoh `client`, connecting only to the robot endpoint. Multicast scouting off. |
| Trust | The Deck trusts only the robot's private CA from its pairing bundle. The router accepts only client certificates on its allowed list ([§ Security](#security)). |
| Reserved | UDP 7452 for post-MVP online pairing ([future.md](future.md#online-enrollment)). |

## Key space

All keys are under `rj/v1/`. Each robot has its own router, so robot names don't appear in keys.

| Key | Kind | Direction | Message | Delivery | Congestion | Priority | Express | Rate |
|-----|------|-----------|---------|----------|------------|----------|---------|------|
| `rj/v1/control` | put | Deck → robot | `Control` | best effort | drop | `RealTime` | yes | 50 Hz + on button change |
| `rj/v1/stop` | put | Deck → robot | `Stop` | **reliable** | block | `RealTime` | yes | on STOP press |
| `rj/v1/robot_state` | put | robot → Deck | `RobotState` | best effort | drop | `RealTime` | yes | on each `Control` + 5 Hz |
| `rj/v1/status` | put | robot → Deck | `Status` | best effort | drop | `Data` | no | 2 Hz (config) |
| `rj/v1/video` | put | robot → Deck | `VideoFrame` | best effort | drop | `InteractiveLow` | no | ≤ 30 fps, only while subscribed |
| `rj/v1/info` | queryable | robot | `InfoRequest` → `Info` | | | | | on connect |
| `rj/v1/clear_stop` | queryable | robot | `ClearStopRequest` → `ClearStopReply` | | | | | on combo |
| `rj/v1/action/<id>` | queryable | robot | `ActionRequest` → `ActionReply` | | | | | on demand, 10 s timeout |
| `rj/v1/deck/<session_id>` | liveliness token | Deck | none | | | | | while running |

Each payload carries a Zenoh encoding of `application/protobuf;remote_joy.v1.<Message>`, so Zenoh tools can show what it is.

**Video on demand:** `deck_video` uses a Zenoh *matching listener* on its publisher, so it only subscribes to the camera and encodes while some Deck is subscribed to `rj/v1/video`. The Deck needs no separate "I'm watching" message.

## Messages (sketch of `remote_joy/v1/*.proto`)

```proto
syntax = "proto3";
package remote_joy.v1;

// ── control ────────────────────────────────────────────────
message Control {
  fixed32 session_id   = 1;  // random per Deck app run; kept across reconnects
  uint32  seq          = 2;  // +1 per message
  uint64  deck_time_us = 3;  // Deck monotonic clock
  repeated float axes  = 4;  // see axis table; 6 used, up to 12
  fixed32 buttons      = 5;  // bit n = button n (see button table)
  bool    stop         = 6;  // Deck's local stop latch is set
}

message Stop {
  fixed32 session_id   = 1;
  uint64  deck_time_us = 2;
  string  source       = 3;  // "button_b", "touch"
}

message RobotState {
  enum State { STATE_UNSPECIFIED = 0; OK = 1; STOPPED = 2; NO_CONTROL = 3; BUSY = 4; REARM = 5; }  // REARM: release + press deadman to resume
  uint64  robot_time_us     = 1;  // robot monotonic clock
  State   state             = 2;
  fixed32 owner_session     = 3;  // session currently in control (0 = none)
  uint32  echo_seq          = 4;  // last Control seq from *this* owner
  uint64  echo_deck_time_us = 5;  // its deck_time_us, unchanged → RTT
  string  stop_reason       = 6;  // "startup", "new_owner", "link_lost", "deck_stop", "service", ...
}

message ClearStopRequest {
  fixed32 session_id   = 1;
  uint32  released_seq = 2;  // first Control seq sent with stop = false (see Rules)
}
message ClearStopReply   { bool ok = 1; string reason = 2; }  // "sticks_not_centered", "not_owner", ...

// ── info ───────────────────────────────────────────────────
message InfoRequest {}
message Info {
  string robot_name             = 1;
  uint32 interface_version      = 2;  // 1
  uint32 interface_minor        = 3;  // additive revisions within v1
  repeated ActionInfo actions   = 4;
  CameraInfo camera             = 5;
  bytes  robot_id               = 6;  // SHA-256 of the robot CA certificate; Deck checks it against the profile
  repeated ButtonHint button_hints = 7;  // what robot-side /joy listeners do with free buttons
}
message ActionInfo { string id = 1; string label = 2; bool confirm = 3; bool moves_robot = 4; }  // Deck greys out moves_robot actions while STOPPED/REARM
message CameraInfo { bool available = 1; uint32 width = 2; uint32 height = 3; uint32 max_fps = 4; }
message ButtonHint { uint32 button = 1; string label = 2; }  // button = bit index (see button table); shown on the Drive screen

// ── status ─────────────────────────────────────────────────
message Status {
  uint64 robot_time_us       = 1;
  repeated StatusItem items  = 2;
}
message StatusItem {
  enum Level { LEVEL_UNSPECIFIED = 0; OK = 1; WARN = 2; ERROR = 3; STALE = 4; }
  string key   = 1;  // stable id, e.g. "battery.pct"
  string label = 2;  // "Battery"
  oneof value { double number = 3; string text = 4; bool flag = 5; }
  string unit  = 6;  // "%", "m/s", "V"
  Level  level = 7;
  string group = 8;  // UI grouping, e.g. "Power", "Diagnostics"
  bool   pinned = 9; // show in the Drive screen's top bar (e.g. robot battery)
}

// ── actions ────────────────────────────────────────────────
message ActionRequest { fixed32 session_id = 1; string id = 2; }
message ActionReply   { bool ok = 1; string message = 2; }  // refusals: "not_owner", "stopped", "timeout", ...

// ── video ──────────────────────────────────────────────────
message VideoFrame {
  enum Codec { CODEC_UNSPECIFIED = 0; JPEG = 1; }
  uint32 frame_id              = 1;
  uint64 capture_time_us       = 2;  // robot monotonic clock
  uint32 encode_done_offset_us = 3;  // capture → encode finished
  uint32 width                 = 4;
  uint32 height                = 5;
  Codec  codec                 = 6;
  bytes  data                  = 7;  // one complete JPEG
}
```

### Axis and button indices

Same order as ROS 2 `joy`'s `game_controller_node` (SDL GameController). On the wire, +1 = left/up, and triggers run 0 (released) to 1 (pressed). `deck_gateway` converts to `joy`'s conventions (open item O7).

| Axis | | Axis | |
|--:|--|--:|--|
| 0 | Left stick X | 3 | Right stick Y |
| 1 | Left stick Y | 4 | Left trigger |
| 2 | Right stick X | 5 | Right trigger |
| 6–11 | reserved (trackpads, gyro) | | |

| Bit | Button | Bit | Button |
|--:|--|--:|--|
| 0 | A | 9 | LB |
| 1 | B (STOP) | 10 | RB |
| 2 | X | 11–14 | D-pad up/down/left/right |
| 3 | Y | 15 | Misc (Quick Access, normally taken by Steam) |
| 4 | View | 16–19 | Paddles (reserved) |
| 5 | Steam (normally taken by Steam) | 20 | Touchpad click |
| 6 | Menu | 21–31 | reserved |
| 7 / 8 | L3 / R3 | | |

### Reserved and free buttons

| Group | Buttons | Owner |
|-------|---------|-------|
| **Reserved for the Deck** | B (STOP), View, Menu (panel, STOP clear), Steam, Quick Access | The Deck app or SteamOS. Robot software must not bind them. |
| **Reserved for driving** | LB (deadman), RB (turbo), left stick, right stick X | `teleop_twist_joy` |
| **Free for the robot** | A, X, Y, D-pad (4), L3, R3, LT, RT, right stick Y | Robot-side `/joy` listeners (e.g. `joy_teleop`). Describe them with button hints. |

Free buttons reach the robot only through `/joy`, so they inherit its gating automatically: nothing gets through while this Deck isn't in control, while STOPPED or REARM, or with the panel open.

## Rules

**Control ownership (gateway):** the first session to send `Control` becomes the owner. Messages from other sessions are ignored, and `RobotState` reports `BUSY` with `owner_session` set. Ownership is released after 2 s without control from the owner, or **immediately** when the owner's liveliness token disappears (a deliberate disconnect). Within a session, messages with `seq <= last_seq` are ignored.

**Disconnect (Deck):** on quit, send `Stop` (reliable), withdraw the liveliness token, then close the session.

**Watchdog (gateway):** no valid `Control` from the owner for 250 ms, or the owner's liveliness token disappearing → state `NO_CONTROL`, and neutral `Joy` at 50 Hz.

**Resume after a drop (gateway):** when the owner's `Control` comes back (same `session_id`):
- **Gap < `resume_grace_ms` (1000):** state `REARM`. Neutral `Joy` continues until a `Control` with LB and RB released is followed by one with the deadman (LB or RB) pressed, then `OK`. No STOP clear is needed.
- **Gap ≥ `resume_grace_ms`:** the stop latch is set (`link_lost`) and needs a normal `clear_stop`.
- **Gap > 2 s:** ownership was already released, so the returning Deck is a new owner (`new_owner`).

**Stop latch (gateway):**
- **Set by:**
  - `Control.stop = true`;
  - any `Stop` message, from *any* paired session (so an observing Deck can stop the robot too);
  - the gateway's `set_stop` service;
  - gateway startup (`startup`);
  - **any ownership change** (`new_owner`), including the same Deck reconnecting with a new session.
- **Cleared only by** `clear_stop` from the owner, and only when that session's latest `Control` has sticks within 0.1 and LB/RB released.
- **Clearing without a race:**
  1. The Deck first stops setting `Control.stop`.
  2. It then sends `clear_stop` with `released_seq` = the first seq it sent with `stop = false`.
  3. After a successful clear, a `Control.stop = true` re-latches only if its `seq >= released_seq`. Older ones still in flight are ignored.
  4. If the clear is refused, the Deck sets its local latch again.
- A new session or reconnect never clears it.

**Actions (gateway):** only the owner session may run actions (others get `not_owner`). Actions configured with `moves_robot` are refused (`stopped`) while the state is `STOPPED`, `REARM` or `NO_CONTROL`.

**Deck:**
- Uses one `session_id` for the whole app run, and re-declares its liveliness token after reconnecting.
- Sends `Control` immediately on any button change, plus every 20 ms.
- On a STOP press, sends `Stop` (reliable) *and* sets `Control.stop`.
- Ignores `RobotState` fields that refer to another owner.

**Limits:**
- The Deck drops a `VideoFrame` whose `data` is over 512 KB, and keeps at most one frame queued for decode.
- The router limits payload size per key in its configuration: control, stop and queries ≤ 1 KB, status ≤ 64 KB, video ≤ 512 KB.

## Clock offset and latency

`RobotState` echoes the Deck's timestamp, so:

- **RTT** = `now − echo_deck_time_us`
- **robot clock offset** ≈ `robot_time_us − (echo_deck_time_us + RTT/2)`, taken from the lowest-RTT samples in a sliding window

`deck_gateway` and `deck_video` run on the same host and both use the monotonic clock, so the offset converts `VideoFrame.capture_time_us` to Deck time. That gives the per-stage latency in the debug overlay.

## Security

- **Pairing, file method (MVP):** `ros2 run deck_gateway pair --deck <name> --host <name-or-ip>...` creates the robot's private CA and server certificate on first use. It then issues a client certificate with a **unique** CN `<name>-<6 random hex>` (e.g. `deck1-3fa9c2`) and adds that exact CN to the router's allowed list. Output: `<name>.bundle.tar` containing `ca.pem`, `client.pem`, `client.key`, and `robot.toml` (robot ID, name, endpoints). Online pairing methods are post-MVP ([future.md](future.md#online-enrollment)).
- **Router access control:** on the network endpoint, only allowed certificate CNs may put `rj/v1/control` and `rj/v1/stop`, query `rj/v1/info|clear_stop|action/*`, declare `rj/v1/deck/*` tokens, and subscribe to robot keys. Only the local endpoint may publish robot keys or declare queryables (open item O6: confirm the syntax).
- **Revoke a Deck:** `pair --revoke <name>` removes its CN from the allowed list and reloads the router. CNs are never reissued, so re-pairing a Deck with the same name gives it a new CN, and the old certificate stays locked out. (The CA isn't checked for revoked certificates, so the allow-list *is* the revocation mechanism.) Reloading drops connected Decks, and a robot being driven stops.
- **What it gives:** encryption, authentication of both ends, replay protection (all from TLS), and no access for devices that haven't been paired.

## Versioning

- **Additive changes** (new fields, new optional keys or actions) stay within `v1`: bump `Info.interface_minor`. Old Decks ignore unknown fields.
- **Breaking changes** go under a new `rj/v2/` key prefix. The gateway may serve both prefixes during a transition.
- The Deck reads `Info` on connect and shows "robot needs a newer app" if `interface_version` is unsupported.
- **Zenoh version:** the Deck's `zenoh` crate is pinned to the version vendored in Jazzy (`zenoh_cpp_vendor`). The pinned version is recorded in `interface/ZENOH_VERSION` and checked by both builds.
