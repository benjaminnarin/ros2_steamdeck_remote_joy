# Future work and fallbacks

Ideas beyond the MVP milestones M0–M5 ([README](../README.md#milestones)). Each entry gives why it's worth doing, a sketch, and the signal that should prompt it. Nothing here is committed to. Measure first, using the Deck's debug overlay ([controller.md](controller.md#debug-overlay)).

---

## Fallback transports

Kept in case the M0 spike shows that Zenoh can't carry control or video well enough over Wi-Fi (open item O3 in [architecture.md](architecture.md#open-items-to-resolve-in-m0-or-during-build)). Zenoh would still carry status, actions and info. The Protobuf messages stay the same, and only the transport for the affected flow changes.

### QUIC datagrams

- `quinn` on the Deck, `msquic` or `quiche` in the C++ gateway.
- Unreliable QUIC datagrams for `Control`/`RobotState` and video. Reliable streams for STOP and queries.
- TLS 1.3, using the same certificates from `pair`.
- Standard, reviewed security, and no head-of-line blocking (data held up behind a lost packet) for datagrams.

### Custom UDP

The earlier design, in short:
- Fixed little-endian structs: a 24-byte header (magic, version, type, flags, session, seq, monotonic µs timestamp).
- Control at 50 Hz with per-packet ACKs, plus video split into ≤1100-byte UDP fragments.
- **It must include:**
  - a handshake in which the robot contributes a nonce, with per-session keys derived by HKDF (otherwise recorded sessions can be replayed after they end);
  - ChaCha20-Poly1305 encryption;
  - immediate send on STOP;
  - size limits on the receiving side;
  - golden test vectors shared by both implementations.
- Smallest possible dependency footprint, but we'd own both the protocol and its security.

---

## Video

### FEC (forward error correction)

- **Why:** over a lossy link, one lost packet loses the whole frame.
- **Sketch:** Reed–Solomon repair data per frame, so a frame can be rebuilt despite a few lost packets. This only makes sense on a transport where we control packetization: QUIC datagrams, custom UDP, or RTP. Zenoh splits messages internally, so FEC doesn't apply there.
- **Do it when:** frames received is below about 95% at normal range after Wi-Fi tuning.

### Adaptive quality

- **Why:** keep video smooth at the edge of Wi-Fi range.
- **Sketch:** the Deck publishes `rj/v1/video_feedback` (frames received %, fps) at 2 Hz. `deck_video` steps `jpeg_quality` and then `max_width` down when delivery drops below 90%, and back up after about 5 s of clean delivery, with hysteresis. This is an additive v1 change.
- **Do it when:** stutter that depends on range is the main complaint.

### Camera MJPEG passthrough

- **Why:** USB cameras can output MJPEG in hardware, so forwarding it removes the decode/resize/encode steps.
- **Sketch:** a `deck_video` mode that reads V4L2 MJPEG directly with 1–2 buffers and also publishes `CompressedImage` to ROS. Resolution is then whatever the camera provides.
- **Do it when:** capture → encoded dominates latency.

### H.264 with low-latency settings

- **Why:** about 1–2 Mbps instead of about 5 Mbps. Less airtime means fewer losses and better range.
- **Sketch:**
  - GStreamer in `deck_video`: VAAPI hardware encoding or x264 `zerolatency`, no B-frames, intra-refresh instead of periodic keyframes.
  - New `VideoFrame.codec = H264`.
  - Deck decodes with `openh264` (compiled into the binary).
  - Needs a keyframe-request query, and works better with FEC.
- **Do it when:** JPEG plus Wi-Fi tuning still isn't good enough. This is a significant jump in complexity.

### WebRTC

- **Why:** buy rather than build. Congestion control, FEC, retransmission of lost packets (NACK) and jitter buffering, as used by Google Meet.
- **Sketch:** GStreamer `webrtcbin` in `deck_video`, `webrtc-rs` on the Deck. Session setup (signalling) could run over Zenoh queries.
- **Cost:** heavy, plus connection setup (ICE) and codec dependencies on the Deck.
- **Do it when:** we'd otherwise be building FEC, adaptive bitrate and H.264 ourselves.

### Benchmark: Sunshine + Moonlight

Not a product option, because there's no guaranteed safe behaviour when the link drops. As a reference, though: Sunshine on the robot streaming `rqt_image_view`, viewed with Moonlight on the Deck, shows what latency your Wi-Fi and camera can achieve.

---

## Connecting, switching and pairing (post-MVP)

The MVP connects one robot on a shared network ([architecture.md § Robots and connection](architecture.md#robots-and-connection)). It already has the hooks for everything below: profiles keyed by robot ID, `connect()`/`disconnect()`, the handover safety rules, and the locator/link/pairing interfaces.

### Post-MVP decisions and open items

Planned decisions:

- **Pairing doesn't depend on any method.** One enrollment protocol, and methods only carry a bundle or a one-time secret. The Deck's lack of a camera or NFC doesn't constrain the design.
- **One Wi-Fi access point per robot, with the Deck app managing Wi-Fi.** Bluetooth PAN as an optional backup. Identity comes from certificates, so robots can share the same `10.42.0.1` address.

Open items:

| # | Question | Resolve before |
|---|----------|----------------|
| O8 | Can the app scan for and activate Wi-Fi connections through NetworkManager over D-Bus in **Game Mode** (polkit permissions for the `deck` user)? | Before per-robot access points |
| O9 | Which pairing methods to ship first, and what hardware they need (the Deck has no camera or NFC) | Before online pairing |
| O10 | Can a Zenoh router serve an mTLS endpoint and a server-only TLS enrollment endpoint side by side, each with its own access rules? | Before online pairing |
| O11 | Bluetooth PAN: does a BlueZ NAP on Ubuntu, joined by NetworkManager from SteamOS, work reliably? What latency and jitter does it give? | Before Bluetooth |
| O12 | Does each robot's Wi-Fi chip support a 5 GHz access point, and on which channels? | Before per-robot access points |

### Switching robots at runtime

- **Robots screen** (shown on launch when no robot is connected, and as a panel tab):

  ```
  ┌ Robots ────────────────────────────────────────────────┐
  │ ● mybot     Wi-Fi ▂▄▆█  connected · 6 ms  [Disconnect] │
  │ ○ rover2    Wi-Fi ▂▄__  in range          [Connect]    │
  │ ○ arm-cart  — not in range ·  BT paired   [Connect ▾]  │
  │                                                        │
  │ [ Pair new robot… ]  [ Join robot network… ]           │
  │ A = select · X = link (Wi-Fi/BT) · Y = forget          │
  └────────────────────────────────────────────────────────┘
  ```

- **Connect flow** (each step shown in the row):
  1. `disconnect()` the current robot (Stop, release);
  2. bring the link up (e.g. join the robot's Wi-Fi);
  3. open a Zenoh session with that profile's certificates;
  4. check `Info.robot_id`;
  5. request the video profile for the link;
  6. the robot is STOPPED (new-owner rule).
- **Wrong-robot guards:** the robot's name in the top bar, a colour per profile, and the STOP-clear prompt naming the robot ("Clear STOP on **mybot**?").
- **Driving input is neutral** while the Robots screen is open.
- **Tests:** connect A → drive → switch to B. Assert A got `Stop` and a liveliness drop before B's session opens.

### One access point per robot (app-managed Wi-Fi)

The planned network setup for several robots:

- Every robot runs its own 5 GHz access point, and the Deck joins the access point of the robot it's driving. **Switching robots switches Wi-Fi networks**, and the app does that through NetworkManager over D-Bus (`zbus`). *To check (O8):* polkit permissions in Game Mode.
- **Locating:** a Wi-Fi locator scans for each profile's SSID/BSSID, which gives the robot's range and signal on the Robots screen.
- **Addressing:** every robot is usually `10.42.0.1` on its own network. That's fine, because identity comes from the certificate and `Info.robot_id`, never the address. The endpoint is the profile's address, then the network's default gateway.
- **The app owns reconnecting:**
  - it creates robot Wi-Fi connections with `autoconnect=no`, so the Deck never drifts onto a robot network;
  - while a robot is selected, it rejoins after drops and switches back from a fallback network (e.g. home Wi-Fi);
  - after `disconnect()` it releases the network, and normal autoconnect takes over.
- **No internet while connected.** Steam works offline in Game Mode.
- **Several robots:** one non-overlapping 5 GHz channel each. The robot gets internet over Ethernet or cellular if it needs it.
- **Robot tooling:** `ros2 run deck_gateway ap setup --ssid rj-mybot --channel 36` wraps `nmcli`. It creates a WPA2/WPA3 hotspot with a generated passphrase, turns power saving off, and records the Wi-Fi details for pairing bundles. `ap show` and `ap rotate-passphrase` manage it. *To check (O12):* 5 GHz access-point support and channels per Wi-Fi chip.
- **Profile and interface:** profiles gain `[wifi] ssid/passphrase/bssid`. Bundles and `EnrollReply` carry `NetworkInfo` ([§ Protocol sketch](#protocol-sketch)).
- **Join robot network…** screen: enter an SSID and passphrase by hand. This is the first step for online pairing methods.

### Online enrollment

One protocol for every pairing method ([§ Protocol sketch](#protocol-sketch)). A method only carries one of two things:

1. **A bundle** (as the MVP's file method does), or
2. **A one-time, high-entropy secret**, in either direction. Then:
   1. the Deck generates its own key pair;
   2. it connects to the robot's restricted enrollment endpoint (server-side TLS only, `rj/enroll/v1/**` only);
   3. both sides prove they know the secret (HMAC over the robot's CA fingerprint and the certificate request);
   4. the robot signs the Deck's certificate and adds it to the access list.

- **Pairing window:** time-limited, opened deliberately (service, button, or `pair --open`). Each secret works once, and attempts are rate-limited.
- **Robot-side plug-ins:** ROS nodes in any language that use the gateway's pairing services (`open`, `close`, `offer_secret`, `create_secret`, `pending`/`approve`) and the `pairing/state` topic.
- **Deck-side plug-ins:** implementations of the `PairingMethod` trait, each with its own small screen.
- **Wi-Fi comes first:** with per-robot access points, the Deck must join the robot's network before it can enroll. The Wi-Fi details come from the method, a bundle, or manual entry.
- **Low-entropy secrets** (short codes) need a PAKE variant. **No-secret methods** (approval) need the fingerprints compared visually.

#### Protocol sketch

On a separate enrollment endpoint, `quic/<host>:7452`. It uses server-side TLS only, its access rules allow only `rj/enroll/v1/**`, and it's open only during a pairing window (open item O10).


**Before:** a pairing method has put the same one-time secret `S` (≥ 128 bits) on both sides, in either direction. The pairing window is open on the robot.

| Key (on the enrollment endpoint) | Kind | Message |
|-----|------|---------|
| `rj/enroll/v1/request` | queryable (robot) | `EnrollRequest` → `EnrollReply` |

```proto
message EnrollRequest {
  string deck_name   = 1;  // becomes the certificate CN, e.g. "deck1"
  bytes  csr_der     = 2;  // Deck-generated key pair; private key never leaves the Deck
  bytes  secret_id   = 3;  // first 8 bytes of SHA-256(S), lets the robot pick the right pending secret
  string method      = 4;  // "file", "camera_code", "nfc", "typed_code", "approval", ... (for logs and UI)
  bytes  deck_proof  = 5;  // HMAC-SHA256(S, "deck"  ‖ robot_ca_fpr ‖ SHA-256(csr_der))
}
message EnrollReply {
  bool   ok              = 1;
  string reason          = 2;  // "window_closed", "bad_proof", "secret_used", "rate_limited", ...
  bytes  deck_cert_der   = 3;
  bytes  ca_cert_der     = 4;  // per-robot CA, or fleet CA
  bytes  robot_id        = 5;
  string robot_name      = 6;
  repeated string endpoints = 7;  // e.g. "quic/10.42.0.1:7450", plus Bluetooth PAN / bench addresses
  NetworkInfo network    = 8;
  bytes  robot_proof     = 9;  // HMAC-SHA256(S, "robot" ‖ robot_ca_fpr ‖ SHA-256(deck_cert_der))
}

message NetworkInfo {           // post-MVP: link settings for per-robot access points / Bluetooth
  WifiAp wifi          = 1;
  BluetoothPan bt      = 2;   // optional backup link
}
message WifiAp {
  string ssid       = 1;
  string passphrase = 2;
  bytes  bssid      = 3;      // optional: pin to this robot's radio
  uint32 channel    = 4;      // informational
}
message BluetoothPan { bytes address = 1; }
```

The pairing bundle's `robot.toml` carries the same `NetworkInfo` fields. `EnrollReply` sends the Wi-Fi passphrase again even though the Deck is already on the network: that way it gets stored in the profile, along with any rotated value.

- `robot_ca_fpr` is the SHA-256 of the CA certificate the enrollment endpoint presents over TLS. Both proofs are tied to it, so an impostor endpoint can't answer correctly without knowing `S`. That holds whichever direction `S` travelled.
- The Deck accepts the reply only if `robot_proof` checks out, and then stores the profile under `robot_id`.
- The robot accepts the request only while the window is open, the secret is pending and unused, and `deck_proof` checks out. On success it adds `deck_name` to the access list and marks `S` as used.
- Rate limit: 5 attempts per window, then the window closes.
- Low-entropy secrets (short codes) **must not** use this message directly. They need a PAKE-based variant, which would be `rj/enroll/v1/pake/*` (future).


#### Robot-side pairing services

The gateway runs the robot side of the protocol and never knows *how* a secret was obtained. Pairing methods are separate ROS nodes, in any language, that use these services:

| Service | Type (in `remote_joy_interface`) | Used by |
|---------|----------------------------------|---------|
| `steamdeck/pairing/open` | `OpenPairing` (duration_s, default 120) | Operator, a physical button node, `pair --open` |
| `steamdeck/pairing/close` | `std_srvs/Trigger` | Operator |
| `steamdeck/pairing/offer_secret` | `OfferSecret` (secret, method, expires_s) | Secret methods that *obtain* a secret on the robot (e.g. read from a sensor) |
| `steamdeck/pairing/create_secret` | `CreateSecret` (method) → secret | Secret methods that *present* a secret from the robot (e.g. a display, an NFC tag writer) |
| `steamdeck/pairing/pending` / `approve` | `ListPending` / `Approve` | Approval-style methods (future, needs a fingerprint comparison) |

| Topic | Type | |
|-------|------|--|
| `steamdeck/pairing/state` | `PairingState` (open, expires, method, last result) | Transient-local. For robot UIs, LEDs, logs. |

Secrets are kept only in memory, work once, and expire when the window closes. On success the gateway signs the Deck's certificate request with the robot CA, adds the CN to the router's access list, and reloads the router. `pair --list`/`--revoke` work for Decks paired by any method.

### Pairing methods

Candidate plug-ins. None of them is chosen yet (O9).

| Method | Shape | Secret direction | Hardware | Notes |
|--------|-------|------------------|----------|-------|
| **File bundle** | bundle | (n/a) | none | **MVP.** `pair` + scp/USB |
| **Code shown on the Deck, read by the robot** (QR or similar, read by the robot's camera) | secret | Deck → robot | robot camera | Robot method node decodes it and calls `offer_secret`. Physical presence = permission. |
| **NFC** | secret | either | USB NFC reader on the Deck, and/or a reader or tag writer on the robot | The Deck has no NFC built in |
| **Short code typed on the Deck** | secret (low entropy) | robot → Deck | a robot display, LED or speaker | **Needs a PAKE variant** (SPAKE2) |
| **Long code / sticker** | secret | robot → Deck | printed label | Plain HMAC works, but typing it is tedious. A static secret, so rotate it after use. |
| **Operator approval** | no secret | (n/a) | a robot UI or ROS service | **Must compare fingerprints** visually |
| **Physical button + time window** | combined with any of the above | (n/a) | a button on the robot | Adds proof of physical access |
| **USB cable / Bluetooth** | bundle or secret | either | cable, or BLE | Possible, not obviously worth it |

### Discovery (locators)

- **Wi-Fi scan:** with per-robot access points (above).
- **mDNS / DNS-SD** on a shared network: the gateway advertises `_steamdeck-rj._udp` with TXT `id=<robot_id>`, `name=`, `v=1`, `port=7450`, and the Deck browses with `mdns-sd`.
- **Zenoh scouting:** multicast router discovery.
- **Hints from pairing:** `EnrollReply.endpoints`.
- Discovery only supplies *addresses*. Trust always comes from the profile, so an advertised name never grants access.

### Bluetooth PAN backup link

- **What:** IP over Bluetooth Classic. The robot runs a BlueZ network access point (NAP, via `ros2 run deck_gateway bt setup`), and the Deck joins as a client through NetworkManager. Zenoh, TLS and the interface stay the same, and its endpoint is just another profile entry.
- **Limits:** about 1–2 Mbps in practice, roughly 10 m range, more jitter. Control, status and actions fit easily.
- **Video:** drops to the **LOW** profile (about 320 px at 10 fps, around 0.5 Mbps) or **OFF**. The Deck asks for this with a new reliable put (an additive v1 change):

  ```proto
  message VideoRequest {               // rj/v1/video_request, Deck → robot; owner session only
    fixed32 session_id = 1;
    enum Profile { PROFILE_UNSPECIFIED = 0; NORMAL = 1; LOW = 2; OFF = 3; }
    Profile profile    = 2;
  }
  ```

  `deck_video` adds `low_max_width` (320), `low_fps` (10) and `low_jpeg_quality` (50). The same request is useful on weak Wi-Fi, from the panel.
- **Switching links is a reconnect,** so the robot is STOPPED afterwards.
- **Radio coexistence:** the Deck's Wi-Fi and Bluetooth share one radio chip. Keeping robot Wi-Fi on 5 GHz avoids most interference.
- *To check (O11):* reliability, latency and jitter between Ubuntu and SteamOS.

### Automatic link failover

- **First version:** manual link selection on the Robots screen.
- **Later:** when Wi-Fi disappears, the app joins the robot's Bluetooth PAN, requests LOW video, and shows a "BACKUP LINK" banner. The robot is STOPPED after any reconnect, so failover never resumes motion by itself.
- **Do it when:** Bluetooth proves usable and Wi-Fi dropouts are a real problem in the field.

### Fleet CA

One organization CA (kept off the robots) signs every robot's certificate and every enrolled Deck. A Deck is enrolled once and can then connect to any fleet robot, with access rules per robot or per group. Profiles already carry a `trust` field.

### Non-IP links

BLE GATT, LoRa or serial RF modems don't carry IP. They would need Zenoh's serial link or a fallback transport, and they lose TLS. Not planned.

## Features

### Generic telemetry

Status items for any message type, read through ROS type introspection (`GenericSubscription` + `rosidl_typesupport_introspection_cpp`) and a field path such as `pose.pose.position.x`. This lifts the v1 limit on supported types.

### Long-running ROS actions

Nav2 goals, docking actions with feedback. The query returns accepted or rejected, then progress is published on `rj/v1/action_progress/<id>`, with cancellation via a query. The Deck shows a progress bar and a cancel button.

### Multiple cameras

`Info.camera` becomes a list, with keys `rj/v1/video/<camera_id>` and a "next camera" control on the Deck (likely Y). Only the selected camera is encoded.

### Map view

Show the occupancy grid and the robot's pose. Large messages, so they'd be sent as compressed tiles on a separate key with caching.

### Native back grips / trackpads / gyro

Expose L4/R4/L5/R5, the trackpads and the gyro as distinct inputs (axes 6–11 and buttons 16–19 are reserved). This depends on SDL/hidraw access while Steam owns the controller.

### Richer GUI (Tauri)

If the Deck UI outgrows egui, a Tauri frontend can sit on top of `deck_core` ([architecture.md D7](architecture.md#design-decisions)).

### Flatpak packaging

- **Why:** Flatpak is Valve's recommended way to install desktop apps on SteamOS. Updates come through Discover or `flatpak update`. The Steam entry launches `flatpak run <app-id>`, which never changes, so the stable-path handling isn't needed. Uninstalling is clean.
- **Sketch:** a `flatpak-builder` manifest on `org.freedesktop.Platform` with the Rust SDK extension, and crates vendored with `flatpak-cargo-generator`. Add it to Steam via Discover or `steamos-add-to-steam`.
- **Sandbox permissions:** `--share=network`, `--device=dri`, `--socket=wayland` + `--socket=fallback-x11`, and gamepad access (`--device=input` on Flatpak ≥ 1.15.6, else `--device=all`).
- **Risk to check first:** gilrs uses udev to detect gamepads being connected, which is a weak spot inside the sandbox. Check that the Steam Input gamepad is found at launch *and* after Steam Input restarts. If not, gilrs needs a fallback that polls `/dev/input`.
- **Do it when:** the app is being handed to users beyond the core team.

#### Private distribution (no Flathub)

1. **Bundle file** (start here):
   ```bash
   flatpak build-bundle repo steamdeck-remote-joy.flatpak io.github.<user>.SteamDeckRemoteJoy   # dev machine
   flatpak install --user ./steamdeck-remote-joy.flatpak                                       # Deck
   ```
   Installing a newer bundle updates the app in place, and the Steam entry carries over. There are no automatic updates.
2. **Private repo:** a static-file Flatpak repo on your own server, signed with GPG, and served **only on the LAN or over Tailscale/WireGuard**.
   ```bash
   flatpak-builder --repo=/srv/flatpak-repo --gpg-sign=<key-id> build-dir io.github.<user>.SteamDeckRemoteJoy.yml
   flatpak build-update-repo --gpg-sign=<key-id> /srv/flatpak-repo
   flatpak remote-add --user --gpg-import=team.gpg teamrepo https://flatpak.lan/repo   # Deck, once
   flatpak install --user teamrepo io.github.<user>.SteamDeckRemoteJoy
   ```
   Discover then shows updates. Don't use GitHub Pages for this (Pages on a public repo are public).

Caveats:
- The runtime still comes from public Flathub, so the first install needs internet.
- Robot access-point networks (post-MVP) have no internet. Use your normal Wi-Fi, or build the bundle with `--runtime` for a fully offline install.
- Pick the app ID once and never change it.

### Release automation

CI builds the Deck tarball (Ubuntu 22.04 container) and the robot packages, checks `interface/ZENOH_VERSION` against both, and attaches artifacts to each `vX.Y.Z` release.
