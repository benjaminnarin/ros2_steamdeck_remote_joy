#include <gtest/gtest.h>

#include <map>

#include "deck_video/rate_control.hpp"

using deck_video::Feedback;
using deck_video::kMaxLevel;
using deck_video::level_rate;
using deck_video::RateController;

namespace {

constexpr std::int64_t kMs = 1000;

// Frames go out every 33 ms; a Deck reports every 200 ms with a chosen delay and loss.
struct Sim {
  RateController rc;
  std::int64_t t = 1'000 * kMs;
  std::uint32_t id = 0;
  std::map<std::uint32_t, std::int64_t> sent;
  std::uint32_t received = 0;
  std::uint32_t last_reported = 0;

  void send_until(std::int64_t until) {
    while (t < until) {
      t += 33 * kMs;
      rc.on_sent(++id, t);
      sent[id] = t;
      rc.tick(t);
    }
  }
  // Run for `ms`, reporting every 200 ms the newest frame that's `delay_ms` old, with
  // `loss` of the frames in between missing.
  void run(std::int64_t ms, std::int64_t delay_ms, double loss = 0.0, std::uint32_t session = 7) {
    const auto end = t + ms * kMs;
    while (t < end) {
      send_until(t + 200 * kMs);
      std::uint32_t newest = 0;
      for (const auto & [k, ts] : sent) {
        if (ts <= t - delay_ms * kMs) {
          newest = k;
        }
      }
      if (newest == 0) {
        continue;
      }
      if (last_reported != 0 && newest > last_reported) {
        const auto n = newest - last_reported;
        received += n - static_cast<std::uint32_t>(n * loss);
      }
      last_reported = newest;
      // The Deck received it delay_ms after it was sent; age makes the delay come out exact.
      const auto age = t - sent[newest] - delay_ms * kMs;
      rc.on_feedback(Feedback{session, newest, static_cast<std::uint32_t>(age), received}, t);
    }
  }
};

}  // namespace

TEST(RateControl, LadderStartsAtTheConfiguredRateAndOnlyGoesDown) {
  const auto r0 = level_rate(0, 30.0, 70);
  EXPECT_EQ(r0.fps, 30.0);
  EXPECT_EQ(r0.quality, 70);
  EXPECT_EQ(r0.width_scale, 1.0);
  double prev = 1e9;
  for (int l = 0; l <= kMaxLevel; ++l) {
    const auto r = level_rate(l, 30.0, 70);
    // Rough bitrate: fps x pixels x a quality factor.
    const double cost = r.fps * r.width_scale * r.width_scale * r.quality;
    EXPECT_LT(cost, prev) << "level " << l;
    prev = cost;
  }
  EXPECT_EQ(level_rate(kMaxLevel + 3, 30.0, 70).fps, level_rate(kMaxLevel, 30.0, 70).fps) << "clamped";
  EXPECT_EQ(level_rate(kMaxLevel, 2.0, 25).fps, 1.0) << "at least 1 fps";
  EXPECT_EQ(level_rate(kMaxLevel, 2.0, 25).quality, 20) << "quality floor";
}

TEST(RateControl, GoodLinkStaysAtFullRate) {
  Sim s;
  s.run(20'000, 20);
  EXPECT_EQ(s.rc.level(), 0);
  ASSERT_TRUE(s.rc.last_delay_us());
  EXPECT_EQ(*s.rc.last_delay_us(), 20 * kMs);
}

TEST(RateControl, OldAppWithoutFeedbackKeepsFullRate) {
  Sim s;
  s.send_until(s.t + 20'000 * kMs);
  EXPECT_EQ(s.rc.level(), 0);
}

TEST(RateControl, QueueingStepsDownOncePerChangeNotPerReport) {
  Sim s;
  s.run(2'000, 20);
  s.run(400, 300);  // steady 300 ms: one step, then wait for frames sent after it
  EXPECT_EQ(s.rc.level(), 1);
  s.run(2'000, 300);  // still 300 ms with frames sent after the change: the new level isn't enough
  EXPECT_GE(s.rc.level(), 2);
  const int l = s.rc.level();
  s.run(4'000, 20);  // fixed
  EXPECT_EQ(s.rc.level(), l) << "no step up before the hold";
}

TEST(RateControl, NoExtraStepWhileTheOldQueueDrains) {
  Sim s;
  s.run(2'000, 20);
  s.run(200, 900);
  const int l = s.rc.level();
  EXPECT_EQ(l, 2) << "> 4x the threshold: two steps at once";
  for (std::int64_t d = 850; d > 100; d -= 50) {  // draining by 50 ms per report
    s.run(200, d);
  }
  EXPECT_EQ(s.rc.level(), l);
}

TEST(RateControl, StepsBackUpAfterTheHoldAndBacksOffAFailedProbe) {
  Sim s;
  s.run(2'000, 20);
  s.run(400, 300);
  ASSERT_EQ(s.rc.level(), 1);
  s.run(5'400, 20);  // 5 s hold, then one step up
  EXPECT_EQ(s.rc.level(), 0);
  s.run(400, 300);  // the probe failed right away
  EXPECT_EQ(s.rc.level(), 1);
  EXPECT_EQ(s.rc.up_hold_us(), 10'000 * kMs) << "hold doubled";
  s.run(6'000, 40);  // fine, but not quiet (a standing queue, like the 3 Mbit/s cap's ~47 ms)
  EXPECT_EQ(s.rc.level(), 1) << "waits the longer hold";
  EXPECT_EQ(s.rc.up_hold_us(), 10'000 * kMs);
  s.run(5'000, 40);
  EXPECT_EQ(s.rc.level(), 0);
  s.run(11'000, 20);
  EXPECT_EQ(s.rc.up_hold_us(), 5'000 * kMs) << "the step up held: hold back to normal";
}

TEST(RateControl, AQuietLinkResetsADoubledHold) {
  Sim s;
  s.run(2'000, 20);
  s.run(400, 300);
  for (int i = 0; i < 3; ++i) {  // a long weak spell: each step up fails at once
    const int before = s.rc.level();
    while (s.rc.level() >= before) {
      s.run(200, 60);  // fine, but not quiet
    }
    s.run(400, 300);
  }
  ASSERT_GE(s.rc.level(), 1);
  ASSERT_EQ(s.rc.up_hold_us(), 40'000 * kMs) << "doubled three times";
  const int l = s.rc.level();
  s.run(5'400, 10);  // the link clears: 10 ms, nothing queueing
  EXPECT_EQ(s.rc.up_hold_us(), 5'000 * kMs) << "back to the normal hold";
  EXPECT_EQ(s.rc.level(), l - 1) << "and it steps up straight away";
  s.run(5'000 * l, 10);
  EXPECT_EQ(s.rc.level(), 0) << "then one level per normal hold";
}

TEST(RateControl, ANewSessionStartsWithTheNormalHold) {
  Sim s;
  s.run(2'000, 20);
  s.run(400, 300);
  s.run(5'400, 40);
  s.run(400, 300);  // failed probe: hold doubled
  ASSERT_EQ(s.rc.up_hold_us(), 10'000 * kMs);
  s.rc.reset_sessions(s.t);  // the Deck leaves; another connects
  EXPECT_EQ(s.rc.up_hold_us(), 5'000 * kMs);
}

TEST(RateControl, LossStepsDown) {
  Sim s;
  s.run(2'000, 20);
  s.run(1'200, 20, 0.3);
  EXPECT_GE(s.rc.level(), 1);
}

TEST(RateControl, ALossBurstCostsAtMostOneStepPerSecond) {
  Sim s;
  s.run(10'000, 20);
  s.run(2'000, 20, 0.5);  // a 2 s radio hiccup (seen on the Deck with power saving on)
  EXPECT_GE(s.rc.level(), 1);
  EXPECT_LE(s.rc.level(), 2);
}

TEST(RateControl, SmallLossIsTolerated) {
  Sim s;
  s.run(10'000, 20, 0.05);
  EXPECT_EQ(s.rc.level(), 0);
}

TEST(RateControl, FeedbackStoppingStepsDownThenAGoneDeckLetsItRecover) {
  Sim s;
  s.run(2'000, 20);
  s.send_until(s.t + 3'500 * kMs);  // no feedback: about one step per second
  EXPECT_GE(s.rc.level(), 2);
  EXPECT_LE(s.rc.level(), 3);
  s.send_until(s.t + 60'000 * kMs);  // session forgotten after 10 s, then steps back up
  EXPECT_EQ(s.rc.level(), 0);
}

TEST(RateControl, ReportOfAFrameOlderThanTheLogIsVeryLate) {
  Sim s;
  s.run(1'000, 20);
  s.send_until(s.t + 40'000 * kMs);  // > 1024 frames later
  s.rc.on_feedback(Feedback{7, 3, 0, 3}, s.t);
  ASSERT_TRUE(s.rc.last_delay_us());
  EXPECT_GT(*s.rc.last_delay_us(), 1'000 * kMs);
}

TEST(RateControl, WorstDeckWins) {
  Sim s;
  s.run(2'000, 20, 0.0, 1);
  s.run(400, 300, 0.0, 2);  // a second Deck on a worse link
  EXPECT_EQ(s.rc.level(), 1);
}

TEST(RateControl, NoDeckWatchingKeepsTheLevelThenRecovers) {
  Sim s;
  s.run(2'000, 20);
  s.run(400, 300);
  ASSERT_EQ(s.rc.level(), 1);
  s.rc.reset_sessions(s.t);
  s.send_until(s.t + 1'000 * kMs);
  EXPECT_EQ(s.rc.level(), 1) << "a reconnecting Deck resumes at the same level";
  s.send_until(s.t + 6'000 * kMs);
  EXPECT_EQ(s.rc.level(), 0) << "no feedback (an old app): back to full rate";
}
