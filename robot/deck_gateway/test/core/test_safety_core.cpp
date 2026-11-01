// Safety rules from docs/interface.md § Rules, with an explicit fake clock.
#include <gtest/gtest.h>

#include <algorithm>
#include <cmath>
#include <initializer_list>
#include <limits>

#include "deck_gateway/core/safety_core.hpp"

using namespace deck_gateway::core;

namespace {

constexpr std::uint32_t kDeck = 0xdec0ffee;
constexpr std::uint32_t kOther = 0x0bad0bad;
constexpr std::uint32_t LB = 1u << kButtonLB;
constexpr std::uint32_t RB = 1u << kButtonRB;
constexpr std::uint32_t A = 1u << 0;
constexpr Micros ms = 1000;

bool has_event(const std::vector<Event> & ev, EventType t) {
  return std::any_of(ev.begin(), ev.end(), [t](const Event & e) { return e.type == t; });
}

class Core : public ::testing::Test {
protected:
  Micros now = 1'000'000'000;           // robot steady clock
  std::uint64_t deck_offset = 5'000'000; // deck clock = robot clock - offset - 3 ms latency
  std::uint32_t seq[2] = {0, 0};
  SafetyCore core{Config{}, now};

  void advance(Micros d) { now += d; }

  ControlIn make(std::uint32_t session, std::uint32_t buttons = 0,
                 std::initializer_list<float> axes = {}, bool stop = false, Micros extra_delay = 0)
  {
    ControlIn c;
    c.session_id = session;
    c.seq = ++seq[session == kDeck ? 0 : 1];
    c.deck_time_us = static_cast<std::uint64_t>(now - 3 * ms - extra_delay) - deck_offset;
    std::size_t i = 0;
    for (float a : axes) {
      c.axes[i++] = a;
    }
    c.axes_count = std::max<std::size_t>(i, 6);
    c.buttons = buttons;
    c.stop = stop;
    return c;
  }

  Step send(std::uint32_t buttons = 0, std::initializer_list<float> axes = {}, bool stop = false,
            Micros extra_delay = 0)
  {
    return core.on_control(make(kDeck, buttons, axes, stop, extra_delay), now);
  }

  /// 50 Hz control stream with ticks in between, for `duration`.
  void stream(Micros duration, std::uint32_t buttons = 0, std::initializer_list<float> axes = {}) {
    for (Micros t = 0; t < duration; t += 20 * ms) {
      send(buttons, axes);
      advance(10 * ms);
      core.tick(now);
      advance(10 * ms);
      core.tick(now);
    }
  }

  /// Silence (ticks only) for `duration`.
  void silence(Micros duration) {
    for (Micros t = 0; t < duration; t += 20 * ms) {
      advance(20 * ms);
      core.tick(now);
    }
  }

  /// Connect, then clear the startup/new_owner latch.
  void connect_and_clear() {
    stream(100 * ms);
    const auto r = core.clear_stop(kDeck, seq[0] + 1, now);
    ASSERT_TRUE(r.ok) << r.reason;
    ASSERT_EQ(core.state(), State::kOk);
  }
};

}  // namespace

// ── startup and ownership ─────────────────────────────────────

TEST_F(Core, StartsLatchedWithNoOwner) {
  EXPECT_EQ(core.state(), State::kStopped);
  EXPECT_TRUE(core.stop_latched());
  EXPECT_FALSE(core.owner_connected());
  EXPECT_EQ(core.robot_state().stop_reason, "startup");
  const Step s = core.tick(now);
  ASSERT_TRUE(s.joy);
  EXPECT_EQ(*s.joy, neutral_joy());
}

TEST_F(Core, FirstSessionBecomesOwnerAndStaysLatched) {
  const Step s = send(LB, {0, 1});
  EXPECT_TRUE(has_event(s.events, EventType::kOwnerChanged));
  EXPECT_EQ(core.robot_state().stop_reason, "new_owner") << "replaces startup";
  EXPECT_EQ(core.owner_session(), kDeck);
  EXPECT_EQ(core.state(), State::kStopped);
  ASSERT_TRUE(s.joy);
  EXPECT_EQ(*s.joy, neutral_joy()) << "no driving while latched";
  EXPECT_TRUE(s.publish_state);
}

TEST_F(Core, ClearStopThenDrive) {
  connect_and_clear();
  const Step s = send(LB, {0.0f, 0.8f, -0.5f, 0, 0, 0.5f});
  ASSERT_TRUE(s.joy);
  EXPECT_FLOAT_EQ(s.joy->axes[1], 0.8f);
  EXPECT_FLOAT_EQ(s.joy->axes[2], -0.5f);
  EXPECT_FLOAT_EQ(s.joy->axes[5], -0.5f) << "trigger 0..1 -> 0..-1";
  EXPECT_EQ(s.joy->buttons[kButtonLB], 1);
  EXPECT_FALSE(core.tick(now).joy) << "no neutral while OK";
}

TEST_F(Core, OtherSessionsAreIgnoredWhileOwned) {
  connect_and_clear();
  const Step s = core.on_control(make(kOther, LB, {0, 1}), now);
  EXPECT_FALSE(s.joy);
  EXPECT_FALSE(s.publish_state);
  EXPECT_EQ(core.owner_session(), kDeck);
  EXPECT_EQ(core.counters().controls_from_others, 1u);
  EXPECT_EQ(core.robot_state().owner_session, kDeck) << "the other Deck derives BUSY from this";
}

TEST_F(Core, DuplicateAndOldSeqIgnored) {
  connect_and_clear();
  ControlIn c = make(kDeck, LB, {0, 1});
  EXPECT_TRUE(core.on_control(c, now).joy);
  EXPECT_FALSE(core.on_control(c, now).joy) << "duplicate";
  c.seq -= 1;
  EXPECT_FALSE(core.on_control(c, now).joy) << "older";
  EXPECT_EQ(core.counters().controls_out_of_order, 2u);
}

TEST_F(Core, EchoesOwnerSeqAndDeckTime) {
  const ControlIn c = make(kDeck);
  core.on_control(c, now);
  const auto rs = core.robot_state();
  EXPECT_EQ(rs.echo_seq, c.seq);
  EXPECT_EQ(rs.echo_deck_time_us, c.deck_time_us);
}

// ── watchdog, REARM, link_lost, owner timeout ─────────────────

TEST_F(Core, WatchdogTripsAt250ms) {
  connect_and_clear();
  send(LB, {0, 1});
  advance(240 * ms);
  EXPECT_FALSE(core.tick(now).joy);
  EXPECT_EQ(core.state(), State::kOk);
  advance(10 * ms);
  const Step s = core.tick(now);
  EXPECT_TRUE(has_event(s.events, EventType::kWatchdogTrip));
  EXPECT_EQ(core.state(), State::kNoControl);
  ASSERT_TRUE(s.joy);
  EXPECT_EQ(*s.joy, neutral_joy());
}

TEST_F(Core, ShortDropRearmsAndNeedsReleaseThenPress) {
  connect_and_clear();
  stream(200 * ms, LB, {0, 1});
  silence(600 * ms);
  EXPECT_EQ(core.state(), State::kNoControl);

  Step s = send(LB, {0, 1});  // deadman still held when the link returns
  EXPECT_TRUE(has_event(s.events, EventType::kRearm));
  EXPECT_EQ(core.state(), State::kRearm);
  EXPECT_FALSE(s.joy) << "neutral comes from tick while REARM";
  EXPECT_TRUE(core.tick(now).joy);
  EXPECT_FALSE(core.stop_latched()) << "a short drop needs no clear_stop";

  advance(20 * ms);
  s = send(LB, {0, 1});
  EXPECT_EQ(core.state(), State::kRearm) << "holding the deadman through the drop doesn't resume";
  advance(20 * ms);
  send(0);
  EXPECT_EQ(core.state(), State::kRearm);
  advance(20 * ms);
  s = send(RB, {0, 1});  // RB (turbo) also counts as the deadman
  EXPECT_TRUE(has_event(s.events, EventType::kRearmDone));
  EXPECT_EQ(core.state(), State::kOk);
  ASSERT_TRUE(s.joy);
  EXPECT_FLOAT_EQ(s.joy->axes[1], 1.0f);
}

TEST_F(Core, LongDropLatchesLinkLost) {
  connect_and_clear();
  stream(200 * ms, LB, {0, 1});
  silence(1200 * ms);
  const Step s = send(LB, {0, 1});
  EXPECT_EQ(core.state(), State::kStopped);
  EXPECT_EQ(core.robot_state().stop_reason, "link_lost");
  EXPECT_EQ(core.counters().link_lost, 1u);
  EXPECT_EQ(core.owner_session(), kDeck) << "same owner: < 2 s";
  ASSERT_TRUE(s.joy);
  EXPECT_EQ(*s.joy, neutral_joy());
}

TEST_F(Core, OwnerReleasedAfter2sAndReturnIsNewOwner) {
  connect_and_clear();
  stream(100 * ms);
  silence(2000 * ms);
  EXPECT_FALSE(core.owner_connected());
  EXPECT_EQ(core.state(), State::kNoControl) << "not latched, but nobody owns it";
  const Step s = send(LB);
  EXPECT_TRUE(has_event(s.events, EventType::kOwnerChanged));
  EXPECT_EQ(core.state(), State::kStopped);
  EXPECT_EQ(core.robot_state().stop_reason, "new_owner");
}

TEST_F(Core, TokenLossReleasesImmediatelyAndAnotherDeckCanTakeOver) {
  connect_and_clear();
  core.on_token_lost(kOther, now);
  EXPECT_EQ(core.owner_session(), kDeck) << "someone else's token doesn't matter";
  const Step s = core.on_token_lost(kDeck, now);
  EXPECT_TRUE(has_event(s.events, EventType::kOwnerReleased));
  EXPECT_FALSE(core.owner_connected());
  const Step o = core.on_control(make(kOther, LB, {0, 1}), now);
  EXPECT_EQ(core.owner_session(), kOther);
  EXPECT_EQ(core.state(), State::kStopped) << "a new owner always starts latched";
  EXPECT_EQ(*o.joy, neutral_joy());
}

// ── stop latch and clear_stop ─────────────────────────────────

TEST_F(Core, StopFromAnySessionLatches) {
  connect_and_clear();
  const Step s = core.on_stop(kOther, "button_b", now);
  EXPECT_TRUE(core.stop_latched());
  EXPECT_EQ(core.robot_state().stop_reason, "deck_stop");
  ASSERT_TRUE(s.joy);
  EXPECT_EQ(*s.joy, neutral_joy());
  EXPECT_FALSE(send(LB, {0, 1}).joy) << "no mapped output while latched (tick sends neutral)";
}

TEST_F(Core, ControlStopFlagLatchesAndServiceLatches) {
  connect_and_clear();
  send(0, {}, /*stop=*/true);
  EXPECT_EQ(core.robot_state().stop_reason, "deck_stop");
  stream(60 * ms);
  ASSERT_TRUE(core.clear_stop(kDeck, seq[0] + 1, now).ok);
  core.set_stop("service", now);
  EXPECT_EQ(core.robot_state().stop_reason, "service");
  core.on_stop(kDeck, "touch", now);
  EXPECT_EQ(core.robot_state().stop_reason, "service") << "the first reason is kept";
}

TEST_F(Core, ClearStopRefusals) {
  EXPECT_EQ(core.clear_stop(kDeck, 1, now).reason, "not_owner") << "no owner yet";
  stream(100 * ms, 0, {0.5f, 0});
  EXPECT_EQ(core.clear_stop(kOther, seq[0] + 1, now).reason, "not_owner");
  EXPECT_EQ(core.clear_stop(kDeck, seq[0] + 1, now).reason, "sticks_not_centered");
  stream(40 * ms, 0, {0.05f, -0.09f, 0.1f, 0});
  stream(40 * ms, LB);
  EXPECT_EQ(core.clear_stop(kDeck, seq[0] + 1, now).reason, "deadman_held");
  stream(40 * ms);
  send(0, {}, true);
  EXPECT_EQ(core.clear_stop(kDeck, seq[0], now).reason, "stop_still_set");
  silence(300 * ms);
  EXPECT_EQ(core.clear_stop(kDeck, seq[0] + 1, now).reason, "no_control");
  EXPECT_TRUE(core.stop_latched());
}

TEST_F(Core, ClearStopAcceptsStickNoiseWithinTolerance) {
  stream(100 * ms, 0, {0.08f, -0.1f, 0.0f, 0.07f});
  EXPECT_TRUE(core.clear_stop(kDeck, seq[0] + 1, now).ok);
}

TEST_F(Core, ClearStopWhenNotLatchedIsOk) {
  connect_and_clear();
  const auto r = core.clear_stop(kDeck, seq[0] + 1, now);
  EXPECT_TRUE(r.ok);
  EXPECT_EQ(r.reason, "not_latched");
}

TEST_F(Core, ReleasedSeqIgnoresInFlightStopAfterClear) {
  // The Deck latched locally with seq 10 (stop=true), sent 11 with stop=false, then clear_stop
  // with released_seq=11. The query overtakes control 10 and 11 on the way.
  stream(100 * ms);              // seq 1..5, stop=false
  ControlIn c10 = make(kDeck, 0, {}, true);
  ControlIn c11 = make(kDeck, 0, {}, false);
  const auto r = core.clear_stop(kDeck, c11.seq, now);
  ASSERT_TRUE(r.ok) << r.reason;
  core.on_control(c10, now);  // stop=true but older than released_seq: in flight before the clear
  EXPECT_FALSE(core.stop_latched());
  core.on_control(c11, now);
  EXPECT_EQ(core.state(), State::kOk);
  send(0, {}, true);             // a new press after the clear does latch
  EXPECT_TRUE(core.stop_latched());
}

TEST_F(Core, NewOwnerIsToldNewOwnerNotThePreviousOwnersReason) {
  connect_and_clear();
  stream(100 * ms, LB, {0, 1});
  silence(1200 * ms);
  send(LB);                                // kDeck's link_lost
  ASSERT_EQ(core.robot_state().stop_reason, "link_lost");
  core.on_token_lost(kDeck, now);
  core.on_control(make(kOther), now);
  EXPECT_EQ(core.robot_state().stop_reason, "new_owner");
  EXPECT_EQ(core.robot_state().owner_session, kOther);
}

TEST_F(Core, NewOwnerForgetsPreviousReleasedSeq) {
  connect_and_clear();                     // kDeck: seq 5, released_seq 6
  core.on_token_lost(kDeck, now);
  for (int i = 0; i < 3; ++i) {            // kOther: seq 1..3
    core.on_control(make(kOther), now);
    advance(20 * ms);
  }
  ASSERT_TRUE(core.clear_stop(kOther, seq[1] + 1, now).ok);  // released_seq 4
  core.on_control(make(kOther, 0, {}, true), now);           // seq 4: >= 4, but < kDeck's 6
  EXPECT_TRUE(core.stop_latched()) << "kDeck's released_seq must not hide kOther's stop";
}

// ── late-control guard ───────────────────────────────────────

TEST_F(Core, LateControlsAreNotAppliedAndDontFeedTheWatchdog) {
  connect_and_clear();
  stream(200 * ms, LB, {0, 1});            // baseline delay 3 ms
  Step s = send(LB, {0, 1}, false, 120 * ms);
  EXPECT_TRUE(s.joy) << "120 ms late is within 150 ms";
  advance(20 * ms);
  s = send(LB, {0, 1}, false, 200 * ms);
  EXPECT_FALSE(s.joy) << "200 ms late is dropped";
  EXPECT_TRUE(s.publish_state) << "but still echoed";
  EXPECT_TRUE(has_event(s.events, EventType::kLateControls));
  for (int i = 0; i < 12; ++i) {           // 240 ms of late controls
    advance(20 * ms);
    s = send(LB, {0, 1}, false, 300 * ms);
    core.tick(now);
  }
  EXPECT_EQ(core.state(), State::kNoControl) << "late controls don't feed the watchdog";
  EXPECT_EQ(core.owner_session(), kDeck) << "but they keep ownership";
  advance(20 * ms);
  send(LB, {0, 1});                        // on time again: looks like a short drop
  EXPECT_EQ(core.state(), State::kRearm);
  EXPECT_GE(core.counters().controls_late, 13u);
}

TEST(LateControlGuard, BaselineFollowsTheWindow) {
  LateControlGuard g(10'000'000, 150'000);
  const std::int64_t t0 = 1'000'000'000;
  EXPECT_FALSE(g.record_and_check(t0, 0)) << "first sample is its own baseline";
  EXPECT_FALSE(g.record_and_check(t0 + 1, 0)) << "1 us later isn't late";
  EXPECT_FALSE(g.record_and_check(t0 + 100'000, 0)) << "100 ms later than baseline";
  EXPECT_TRUE(g.record_and_check(t0 + 200'000, 0)) << "200 ms later than baseline";
  // After 10 s without a low sample, the baseline moves up to the new normal.
  bool late = true;
  for (std::int64_t t = t0 + 200'000; t < t0 + 11'000'000; t += 20'000) {
    late = g.record_and_check(t, static_cast<std::uint64_t>(t - t0 - 200'000));
  }
  EXPECT_FALSE(late);
  g.reset();
  EXPECT_EQ(g.last_excess_us(), 0);
}

// ── actions ───────────────────────────────────────────────────

TEST_F(Core, ActionGating) {
  EXPECT_EQ(core.action_refusal(kDeck, false), "not_owner");
  stream(100 * ms);
  EXPECT_EQ(core.action_refusal(kOther, false), "not_owner");
  EXPECT_EQ(core.action_refusal(kDeck, true), "stopped") << "latched";
  EXPECT_EQ(core.action_refusal(kDeck, false), "") << "non-motion actions work while STOPPED";
  ASSERT_TRUE(core.clear_stop(kDeck, seq[0] + 1, now).ok);
  EXPECT_EQ(core.action_refusal(kDeck, true), "");
  silence(300 * ms);
  EXPECT_EQ(core.action_refusal(kDeck, true), "stopped") << "NO_CONTROL";
  send(0);
  EXPECT_EQ(core.state(), State::kRearm);
  EXPECT_EQ(core.action_refusal(kDeck, true), "stopped") << "REARM";
}
