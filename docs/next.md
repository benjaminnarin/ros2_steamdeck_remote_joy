# Next: where we are and what's open

The resume point between work sessions. Update it when stopping; delete entries once they're done (the devlog keeps the history).

*Last updated: 2026-11-02.*

## Where we are

- **M0, M1, M2 done.** O19 (video delaying `RobotState` on a weak link) is answered: two QUIC links per Deck (`prio=0-2`, `prio=3-7`) plus adaptive video from the Deck's `VideoFeedback` (devlog § 4.2).
- **Real robot:** the TurtleBot 1 (Create 1 base + RealSense D455) drives with video from the Deck: `ros2 launch deck_turtlebot turtlebot.launch.py` (robot.md § TurtleBot 1; source `~/create_ws` first). This laptop is the robot computer.
- **Deck app UI (2026-11-02, on the Deck as `fee865c`):** full-screen video with semi-transparent strips (status; stats behind a button), a smaller touch STOP, a Menu card (status, app and interface versions, Inputs, Close, Quit) and a development Inputs screen (every input live). The app now lays out on a 1280-point canvas: gamescope made egui use a 2.17 scale, which is why screens didn't fit (controller.md § Scale). `--screenshot DIR` captures each screen on the Deck; `--version` / the Menu show the git commit.
- **Deploys:** `deploy_deck.sh` checks SSH first (Tailscale may print a login URL to approve) and reuses one connection for 10 min.
- Recent extras: Steam Library artwork (`controller/assets/steam/`), and `camera:=webcam` in the sim (`deck_sim/webcam.py`).
- `main` is pushed and clean; see `git log` for the last commit. The TurtleBot base and RealSense are unplugged at the moment.
- **Dev setup:** pairing state lives in `~/.config/deck_gateway/` (no shims). Run the sim with
  `source install/setup.bash && ros2 launch deck_sim sim.launch.py camera:=webcam` (or `camera:=true` for the test pattern).
- **Next up: M3 Status.** A design review is written below but **not yet discussed with the user**. Go through it with them first; nothing below has been applied to the docs or code.

## M3 Status: design review (pending)

Current spec: [interface.md](interface.md) (`rj/v1/status`, `Status`/`StatusItem`), [robot.md § Status](robot.md#status), [controller.md](controller.md) (Menu screen, pinned item in the top strip). **New since the review:** the Deck panel is now the Menu card, which has little room left; status items will need their own screen (like the Inputs screen) or a scrolling section, and the pinned item goes in the top strip. Already in place: `status.proto`, the status golden file, the router's ACL and 64 KB limit, `gateway_sim.yaml` status items, and `fake_robot` publishing `/battery_state`, `/odom`, `/diagnostics`, `/lights_on` (latched). Not built: the gateway's status code (`status_sources.cpp` in robot.md doesn't exist yet) and the Deck's Status tab (`ui.rs` shows a placeholder).

Keep: a full snapshot every tick (a lost message is repaired by the next one, no deltas).

Proposed changes:

1. **Battery percentage scale.** ROS `BatteryState.percentage` is 0–1 (NaN = unknown); `fake_robot` follows that. The config example (`unit: "%"`, `warn_below: 25`) would always WARN and show "0.8 %". → The BatteryState reader converts to 0–100; NaN shows "—".
2. **Latched topics go STALE.** `/lights_on` is transient-local and published once: a volatile subscriber never gets it, and the 3 s rule marks it STALE. → Per-item `latched: true` (reliable + transient-local, no silence rule). Default subscriptions are best effort (they match both best-effort and reliable publishers). Per-item `stale_after_s` (default 3) for slow topics.
3. **Status work shares the safety thread.** The gateway uses one `rclcpp::spin` thread, which also runs the 50 Hz tick. → Status subscriptions and the publish timer get their own callback group on a second executor thread, latest values behind a mutex.
4. **`/diagnostics` merging and limits.** Each `DiagnosticArray` holds one publisher's entries. → Keep the latest per name (key `diag/<name>`) with its own silence timer; `include`/`exclude` name-prefix filters; `max_items` cap (e.g. 100); worst first. Message text only (no key/values in v1).
5. **Errors unseen while driving.** → A small dot by "Menu = panel" in the help line showing the worst level among non-pinned items. Display only.
6. **Level rules.** → Add `warn_above`/`error_above`, `warn_if`/`error_if` for bools, and text for `power_supply_status` (charging/discharging/full).
7. **Number formatting.** → Add `uint32 decimals = 10` to `StatusItem`, set per item (breaking change; fine in dev).
8. **Bad config.** → Skip the item, log an error, and publish an ERROR item in a "Config" group naming the problem. Driving never depends on status config.
9. **Small:** warn if more than one item is pinned (top bar shows the first); keep `robot_time_us`; keep the Deck's 3 s "no Status" rule.

Tests: gtest for levels, formatting, diagnostics merge/limits and staleness; a launch test with `fake_deck` subscribing to `rj/v1/status` against `fake_robot` (battery, latched lights, diagnostics); Deck unit tests for grouping, staleness and the dot; regenerate the status golden for `decimals`.

Questions for the user:
- Should the worst-level dot count the pinned item too? (Suggested: no; it shows its own colour.)
- Diagnostics by default: all entries worst first with the cap, or only non-OK? (Suggested: all.)

Then: update interface.md, robot.md, controller.md and `status.proto`, and build.

## Open todos

### Milestones
- [ ] **M3 Status**: review above, then build.
- [ ] **M4 Actions**: panel actions → `Trigger`/`SetBool` services; `fake_robot` has `/dock` and `/lights`. Keys and ACL exist.
- [ ] **M5 Packaging**: Deck tarball and robot systemd service. Blocked on **O18** (how to build with an older glibc: rootless Podman, `cargo zigbuild`, or CI).

### Video (O19 follow-ups)
- [ ] End-to-end launch test of video feedback with `fake_deck` (feedback in → `deck_video` level changes).
- [ ] Field walk test: the Deck at the edge of Wi-Fi, watching the level and RTT (`bench --seconds N`).
- [ ] Optional tuning: at most one step down per loss burst; a finer step between 15 and 20 fps.
- [ ] Optional, sim only: the laptop webcam gives 7 fps in a dim room. Turning `exposure_dynamic_framerate` off (V4L2 control 0x009a0903) raised it to 15; a fixed 15 ms exposure stayed at 15, so the rest of the limit is in the camera or `webcam.py` (maybe the Python `msg.data` copy). The user is fine with it as a source limit; the RealSense runs at 30 fps.

### Deck controls
- [ ] **R4/R5 back grips don't reach the app** (seen 2026-11-02 on the Inputs screen). In Game Mode the app only sees the Steam Virtual Gamepad (an Xbox 360 pad: 15 buttons), and `pad.rs` reads bits 0–14, so the grips only arrive if the Steam Input layout maps them to one of those buttons (the L grips do, as another button; R4/R5 aren't mapped). Options: map R4/R5 in the app's Steam Input layout (no code; they arrive as that button), or read the controller natively via hidraw to fill the reserved buttons 16–19 (future.md § Native back grips; unproven while Steam owns the controller).

### Open items ([architecture.md](architecture.md#open-items-to-resolve-in-m0-or-during-build))
- [ ] **O16**: re-measure Wi-Fi delay spikes with power saving off on the Deck and robot.
- [ ] **O4**: DSCP for QUIC (nftables fallback on UDP 7450); upstream draft U5.
- [ ] **O7**: confirm the trigger sign on hardware (marked "M1 quick check"; check whether it was done and close it).
- [ ] **O5 / O17**: on each `zenoh_cpp_vendor` release, check for zenoh-c ≥ 1.10.2 with `quinn-proto` ≥ 0.11.18, and switch back from `deck_router` to `rmw_zenohd` when it ships.

### TurtleBot
- [ ] Rename the robot from "deck_sim" to "turtlebot": the name is `robot_name` in `~/.config/deck_gateway/pairing.json`, set on first pairing (`pair --robot-name`). Check whether editing it is enough or the Deck's profile needs a fresh pairing.
- [ ] M3 status on the TurtleBot: battery is `std_msgs/Float32` `charge_ratio` (0–1), so the Float32 reader needs a `scale` (×100) as well as BatteryState; `create_driver` also publishes `/diagnostics`.
- [ ] Robot actions for M4: the driver's `dock`/`undock` are `std_msgs/Empty` topics, not services, so they need a topic-publishing action type or a small service wrapper.
- [ ] A field test on the robot: weak-link video and the adaptive rate on the real camera.

### Housekeeping
- [ ] Allow list: the old certificate `deck1-2e43e9` / `dev-3b3464` may still be allowed. `pair --revoke deck1` would also revoke the current deck1 certificate, so either re-pair after revoking or add revoke-by-CN. Check with `pair --list`.
- [ ] Docs disagree on re-pairing: deployment.md § Re-pairing says the old certificate stays allowed until `--revoke`; interface.md § Revoke says it stays locked out. Check `pair` and fix the wrong one.
- [ ] Stale text: robot.md "Still to add: `deck_video` … with M2"; the comment at the top of `deck_sim/config/gateway_sim.yaml` ("once that package exists"); controller.md crate tree note for `status.rs`.
- [ ] End of project: review the upstream drafts U1–U5 in [upstream.md](upstream.md) with the user. **Never file them ourselves.**
