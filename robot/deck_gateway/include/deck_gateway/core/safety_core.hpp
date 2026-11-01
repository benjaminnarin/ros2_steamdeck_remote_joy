// deck_gateway's safety logic: ownership, watchdog, late-control guard, REARM, stop latch,
// clear_stop and action gating. Rules: docs/interface.md § Rules.
//
// No ROS, no Zenoh, no clock of its own: every call takes `now_us` from a steady clock, so tests
// drive time explicitly. Not thread-safe: the node calls it from one place (or under a mutex).
#pragma once

#include <array>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

#include "deck_gateway/core/joy_mapping.hpp"
#include "deck_gateway/core/late_control_guard.hpp"

namespace deck_gateway::core {

using Micros = std::int64_t;

inline constexpr std::uint32_t kButtonLB = 9;
inline constexpr std::uint32_t kButtonRB = 10;

struct Config {
  Micros watchdog_us = 250'000;          // no valid control for this long -> NO_CONTROL
  Micros resume_grace_us = 1'000'000;    // shorter drops resume via REARM, longer latch link_lost
  Micros owner_timeout_us = 2'000'000;   // no control from the owner for this long -> released
  Micros max_control_delay_us = 150'000; // late-control guard threshold
  Micros delay_window_us = 10'000'000;   // late-control guard baseline window
  float clear_stop_stick_tolerance = 0.1f;
  /// Buttons that make teleop_twist_joy drive: LB (enable) and RB (turbo, which also enables).
  std::uint32_t drive_buttons = (1u << kButtonLB) | (1u << kButtonRB);
};

/// Values match remote_joy.v1.RobotState.State. BUSY isn't here: RobotState is broadcast and
/// always describes the owner's state; a Deck shows BUSY itself when owner_session is another's.
enum class State : int { kOk = 1, kStopped = 2, kNoControl = 3, kRearm = 5 };

const char * to_string(State s);

/// A Control as received (already decoded from Protobuf by the node).
struct ControlIn {
  std::uint32_t session_id = 0;
  std::uint32_t seq = 0;
  std::uint64_t deck_time_us = 0;
  std::array<float, 12> axes{};
  std::size_t axes_count = 0;
  std::uint32_t buttons = 0;
  bool stop = false;
};

enum class EventType {
  kOwnerChanged,   // detail: "first" or "replaced"
  kOwnerReleased,  // detail: "timeout" or "token_lost"
  kStopLatched,    // detail: reason
  kStopCleared,
  kWatchdogTrip,
  kRearm,          // short drop: waiting for deadman release + press
  kRearmDone,
  kLateControls,   // start of a run of late controls; detail: how late (ms)
};

const char * to_string(EventType t);

struct Event {
  EventType type;
  std::uint32_t session = 0;
  std::string detail;
};

/// What a call wants the node to do.
struct Step {
  /// Publish this on /joy now (mapped input while OK, neutral right after a stop or on tick).
  std::optional<JoyOut> joy;
  /// An in-sequence Control from the owner: publish RobotState now (the echo for RTT).
  bool publish_state = false;
  /// For logging (INFO with session ID) and diagnostics.
  std::vector<Event> events;
};

struct RobotStateOut {
  State state = State::kStopped;
  std::uint32_t owner_session = 0;
  std::uint32_t echo_seq = 0;
  std::uint64_t echo_deck_time_us = 0;
  std::string stop_reason;
};

struct ClearStopResult {
  bool ok = false;
  std::string reason;  // refusal: "not_owner", "no_control", "sticks_not_centered", "deadman_held", "stop_still_set"
  std::vector<Event> events;
};

struct Counters {
  std::uint64_t controls_applied = 0;
  std::uint64_t controls_late = 0;
  std::uint64_t controls_out_of_order = 0;  // seq <= last seq (duplicates, reordering)
  std::uint64_t controls_from_others = 0;   // non-owner sessions (the Deck shows them BUSY)
  std::uint64_t watchdog_trips = 0;
  std::uint64_t rearms = 0;
  std::uint64_t link_lost = 0;
  std::uint64_t owner_changes = 0;
  std::uint64_t stops_latched = 0;
  std::uint64_t stops_cleared = 0;
};

class SafetyCore {
public:
  /// Starts with no owner and the stop latch set ("startup").
  SafetyCore(const Config & config, Micros now_us);

  Step on_control(const ControlIn & control, Micros now_us);
  /// rj/v1/stop from any paired session (an observing Deck can stop the robot too).
  Step on_stop(std::uint32_t session_id, std::string_view source, Micros now_us);
  /// The liveliness token rj/v1/deck/<session> disappeared (deliberate disconnect or lease loss).
  Step on_token_lost(std::uint32_t session_id, Micros now_us);
  /// steamdeck/set_stop service or other robot software.
  Step set_stop(std::string_view reason, Micros now_us);
  ClearStopResult clear_stop(std::uint32_t session_id, std::uint32_t released_seq, Micros now_us);
  /// Call at a steady 50 Hz: watchdog, owner timeout, and neutral /joy while not OK.
  Step tick(Micros now_us);

  /// Empty if allowed, else the refusal: "not_owner", or "stopped" for moves_robot actions while
  /// STOPPED, REARM or NO_CONTROL.
  std::string action_refusal(std::uint32_t session_id, bool moves_robot) const;

  State state() const;
  RobotStateOut robot_state() const;
  bool stop_latched() const { return latched_; }
  bool owner_connected() const { return owner_.has_value(); }
  std::optional<std::uint32_t> owner_session() const;
  const Counters & counters() const { return counters_; }
  const Config & config() const { return config_; }

private:
  struct Owner {
    std::uint32_t session = 0;
    Micros since_us = 0;
    bool has_seq = false;
    std::uint32_t last_seq = 0;
    Micros last_rx_us = 0;       // any in-sequence control, late or not: keeps ownership
    bool has_applied = false;
    Micros last_applied_us = 0;  // controls that passed the guard: feed the watchdog
    ControlIn last_applied;
    std::uint64_t echo_deck_time_us = 0;
  };

  void become_owner(std::uint32_t session, Micros now_us, Step & step);
  void release_owner(const char * why, Step & step);
  void latch(std::string_view reason, std::uint32_t session, Step & step);
  bool sticks_centered(const ControlIn & c) const;

  Config config_;
  std::optional<Owner> owner_;
  bool latched_ = true;
  std::string stop_reason_ = "startup";
  /// After a clear: Control.stop re-latches only for seq >= this (older ones were in flight).
  std::optional<std::uint32_t> released_seq_;
  State mode_ = State::kNoControl;  // OK, NO_CONTROL or REARM; the latch overrides it
  bool rearm_released_seen_ = false;
  bool in_late_run_ = false;
  LateControlGuard late_guard_;
  Counters counters_;
};

}  // namespace deck_gateway::core
