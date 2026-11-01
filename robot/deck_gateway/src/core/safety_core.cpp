#include "deck_gateway/core/safety_core.hpp"

#include <cctype>
#include <cmath>

namespace deck_gateway::core {

const char * to_string(State s) {
  switch (s) {
    case State::kOk: return "OK";
    case State::kStopped: return "STOPPED";
    case State::kNoControl: return "NO_CONTROL";
    case State::kRearm: return "REARM";
  }
  return "?";
}

const char * to_string(EventType t) {
  switch (t) {
    case EventType::kOwnerChanged: return "owner_changed";
    case EventType::kOwnerReleased: return "owner_released";
    case EventType::kStopLatched: return "stop_latched";
    case EventType::kStopCleared: return "stop_cleared";
    case EventType::kWatchdogTrip: return "watchdog_trip";
    case EventType::kRearm: return "rearm";
    case EventType::kRearmDone: return "rearm_done";
    case EventType::kLateControls: return "late_controls";
  }
  return "?";
}

std::optional<Deadman> parse_deadman(std::string_view name) {
  std::string n;
  for (char ch : name) {
    n += static_cast<char>(std::toupper(static_cast<unsigned char>(ch)));
  }
  if (n == "LT" || n == "L2") {
    return Deadman::axis(kAxisLT);
  }
  if (n == "RT" || n == "R2") {
    return Deadman::axis(kAxisRT);
  }
  if (n == "LB" || n == "L1") {
    return Deadman::button(kButtonLB);
  }
  if (n == "RB" || n == "R1") {
    return Deadman::button(kButtonRB);
  }
  return std::nullopt;
}

bool SafetyCore::deadman_held(const ControlIn & c, bool was_held) const {
  const Deadman & d = config_.deadman;
  if (d.kind == Deadman::Kind::kButton) {
    return ((c.buttons >> d.index) & 1u) != 0;
  }
  const float v = d.index < c.axes_count ? sanitize_axis(c.axes[d.index], 0.0f, 1.0f) : 0.0f;
  return was_held ? v > d.release : v > d.press;
}

JoyOut SafetyCore::make_joy(const ControlIn & c, bool deadman) const {
  const std::uint32_t buttons = deadman ? c.buttons : (c.buttons & ~config_.needs_deadman_buttons);
  JoyOut j = to_joy(c.axes.data(), c.axes_count, buttons);
  j.buttons[kJoyDeadmanButton] = deadman ? 1 : 0;
  return j;
}

SafetyCore::SafetyCore(const Config & config, Micros /*now_us*/)
: config_(config), late_guard_(config.delay_window_us, config.max_control_delay_us) {}

std::optional<std::uint32_t> SafetyCore::owner_session() const {
  if (!owner_) {
    return std::nullopt;
  }
  return owner_->session;
}

State SafetyCore::state() const {
  if (latched_) {
    return State::kStopped;
  }
  if (!owner_) {
    return State::kNoControl;
  }
  return mode_;
}

RobotStateOut SafetyCore::robot_state() const {
  RobotStateOut r;
  r.state = state();
  r.stop_reason = latched_ ? stop_reason_ : "";
  if (owner_) {
    r.owner_session = owner_->session;
    r.echo_seq = owner_->has_seq ? owner_->last_seq : 0;
    r.echo_deck_time_us = owner_->echo_deck_time_us;
  }
  return r;
}

void SafetyCore::latch(std::string_view reason, std::uint32_t session, Step & step) {
  step.joy = neutral_joy();  // cut output now rather than on the next tick
  if (latched_) {
    return;  // keep the first reason
  }
  latched_ = true;
  stop_reason_ = std::string(reason);
  ++counters_.stops_latched;
  step.events.push_back({EventType::kStopLatched, session, stop_reason_});
}

void SafetyCore::become_owner(std::uint32_t session, Micros now_us, Step & step) {
  const bool replaced = counters_.owner_changes > 0;
  owner_ = Owner{};
  owner_->session = session;
  owner_->since_us = now_us;
  owner_->last_rx_us = now_us;
  mode_ = State::kNoControl;
  deadman_ = false;
  rearm_released_seen_ = false;
  in_late_run_ = false;
  released_seq_.reset();  // the previous owner's sequence numbers mean nothing now
  late_guard_.reset();
  ++counters_.owner_changes;
  step.events.push_back({EventType::kOwnerChanged, session, replaced ? "replaced" : "first"});
  // Any ownership change latches STOP, including the same Deck with a new session. The reason
  // is always new_owner: a leftover reason (e.g. the previous owner's link_lost) would mislead.
  if (latched_) {
    stop_reason_ = "new_owner";
    step.joy = neutral_joy();
    step.events.push_back({EventType::kStopLatched, session, stop_reason_});
  } else {
    latch("new_owner", session, step);
  }
}

void SafetyCore::release_owner(const char * why, Step & step) {
  step.events.push_back({EventType::kOwnerReleased, owner_->session, why});
  owner_.reset();
  mode_ = State::kNoControl;
  in_late_run_ = false;
  late_guard_.reset();
  step.joy = neutral_joy();
}

bool SafetyCore::sticks_centered(const ControlIn & c) const {
  for (std::size_t i = 0; i < 4; ++i) {
    const float v = i < c.axes_count ? c.axes[i] : 0.0f;
    if (!std::isfinite(v) || std::fabs(v) > config_.clear_stop_stick_tolerance) {
      return false;
    }
  }
  return true;
}

Step SafetyCore::on_control(const ControlIn & c, Micros now_us) {
  Step step;
  if (!owner_) {
    become_owner(c.session_id, now_us, step);
  } else if (owner_->session != c.session_id) {
    ++counters_.controls_from_others;
    return step;
  }
  Owner & o = *owner_;
  if (o.has_seq && c.seq <= o.last_seq) {
    ++counters_.controls_out_of_order;
    return step;
  }
  o.has_seq = true;
  o.last_seq = c.seq;
  o.last_rx_us = now_us;
  o.echo_deck_time_us = c.deck_time_us;
  step.publish_state = true;

  if (late_guard_.record_and_check(now_us, c.deck_time_us)) {
    ++counters_.controls_late;
    if (!in_late_run_) {
      in_late_run_ = true;
      step.events.push_back({EventType::kLateControls, c.session_id,
        std::to_string(late_guard_.last_excess_us() / 1000) + " ms late"});
    }
    return step;  // not applied, doesn't feed the watchdog
  }
  in_late_run_ = false;

  const bool returning = o.has_applied && mode_ == State::kNoControl;
  const Micros gap = o.has_applied ? now_us - o.last_applied_us : 0;
  o.has_applied = true;
  o.last_applied_us = now_us;
  o.last_applied = c;
  // After a gap, start the hysteresis fresh: a trigger resting just under `press` isn't "held".
  deadman_ = deadman_held(c, returning ? false : deadman_);
  ++counters_.controls_applied;

  if (c.stop && (!released_seq_ || c.seq >= *released_seq_)) {
    latch("deck_stop", c.session_id, step);
  }

  if (returning) {
    if (gap < config_.resume_grace_us) {
      mode_ = State::kRearm;
      rearm_released_seen_ = false;
      ++counters_.rearms;
      step.events.push_back({EventType::kRearm, c.session_id,
        "after " + std::to_string(gap / 1000) + " ms"});
    } else {
      mode_ = State::kOk;  // the latch holds it stopped until clear_stop
      ++counters_.link_lost;
      latch("link_lost", c.session_id, step);
    }
  } else if (mode_ == State::kNoControl) {
    mode_ = State::kOk;  // first applied control of this owner (who starts latched)
  }

  if (mode_ == State::kRearm) {
    if (!deadman_) {
      rearm_released_seen_ = true;
    } else if (rearm_released_seen_) {
      mode_ = State::kOk;
      step.events.push_back({EventType::kRearmDone, c.session_id, ""});
    }
  }

  if (!latched_ && mode_ == State::kOk) {
    step.joy = make_joy(c, deadman_);
  }
  return step;
}

Step SafetyCore::on_stop(std::uint32_t session_id, std::string_view source, Micros /*now_us*/) {
  Step step;
  latch("deck_stop", session_id, step);
  if (!step.events.empty()) {
    step.events.back().detail = "deck_stop (" + std::string(source) + ")";
  }
  return step;
}

Step SafetyCore::on_token_lost(std::uint32_t session_id, Micros /*now_us*/) {
  Step step;
  if (owner_ && owner_->session == session_id) {
    release_owner("token_lost", step);
  }
  return step;
}

Step SafetyCore::set_stop(std::string_view reason, Micros /*now_us*/) {
  Step step;
  latch(reason, 0, step);
  return step;
}

ClearStopResult SafetyCore::clear_stop(
  std::uint32_t session_id, std::uint32_t released_seq, Micros /*now_us*/)
{
  ClearStopResult r;
  if (!owner_ || owner_->session != session_id) {
    r.reason = "not_owner";
    return r;
  }
  if (!latched_) {
    r.ok = true;
    r.reason = "not_latched";
    return r;
  }
  const Owner & o = *owner_;
  if (!o.has_applied || mode_ == State::kNoControl) {
    r.reason = "no_control";
    return r;
  }
  const ControlIn & c = o.last_applied;
  if (c.stop && c.seq >= released_seq) {
    r.reason = "stop_still_set";
    return r;
  }
  if (!sticks_centered(c)) {
    r.reason = "sticks_not_centered";
    return r;
  }
  if (deadman_) {
    r.reason = "deadman_held";
    return r;
  }
  latched_ = false;
  stop_reason_.clear();
  released_seq_ = released_seq;
  ++counters_.stops_cleared;
  r.ok = true;
  r.events.push_back({EventType::kStopCleared, session_id, ""});
  return r;
}

Step SafetyCore::tick(Micros now_us) {
  Step step;
  if (owner_) {
    const Owner & o = *owner_;
    const Micros last_applied = o.has_applied ? o.last_applied_us : o.since_us;
    if (mode_ != State::kNoControl && now_us - last_applied >= config_.watchdog_us) {
      mode_ = State::kNoControl;
      ++counters_.watchdog_trips;
      step.events.push_back({EventType::kWatchdogTrip, o.session,
        std::to_string((now_us - last_applied) / 1000) + " ms"});
    }
    if (now_us - o.last_rx_us >= config_.owner_timeout_us) {
      release_owner("timeout", step);
    }
  }
  if (state() != State::kOk) {
    step.joy = neutral_joy();
  }
  return step;
}

std::string SafetyCore::action_refusal(std::uint32_t session_id, bool moves_robot) const {
  if (!owner_ || owner_->session != session_id) {
    return "not_owner";
  }
  if (moves_robot && state() != State::kOk) {
    return "stopped";
  }
  return "";
}

}  // namespace deck_gateway::core
