// Adaptive video rate for deck_video, kept free of ROS and Zenoh so it's unit-tested.
// Spec: docs/robot.md § deck_video (Adaptive rate), docs/interface.md (rj/v1/video_feedback).
//
// The Deck reports the newest frame it received and how long ago (VideoFeedback). deck_video
// remembers when it sent each frame, so on its own clock:
//   delay = feedback arrival - send time of last_frame_id - last_frame_age_us
// That's the frame's one-way delay plus the feedback's (small, on the RealTime link). Queued
// video shows up here long before it shows up as loss.
#pragma once

#include <algorithm>
#include <array>
#include <cstdint>
#include <deque>
#include <map>
#include <optional>

namespace deck_video {

/// What one level sends, relative to the configured rate.
struct Rate {
  double fps;
  int quality;
  double width_scale;
};

inline constexpr int kMaxLevel = 6;

/// Level 0 = the configured fps and quality. Each step cuts the bitrate by about 1.5-2x:
/// fps first (each frame stays sharp), then JPEG quality, then size.
inline Rate level_rate(int level, double base_fps, int base_quality) {
  struct Step {
    double fps_div;
    int quality_delta;
    double width_scale;
  };
  static constexpr std::array<Step, kMaxLevel + 1> kSteps{{
    {1.0, 0, 1.0},          // 30 fps, q70, 480 px at the defaults: ~4.8 Mbps
    {1.5, 0, 1.0},          // 20 fps: ~3.2 Mbps
    {2.0, -10, 1.0},        // 15 fps, q60: ~2 Mbps
    {3.0, -20, 1.0},        // 10 fps, q50: ~1.1 Mbps
    {5.0, -25, 2.0 / 3.0},  // 6 fps, q45, 320 px: ~0.4 Mbps
    {10.0, -30, 2.0 / 3.0}, // 3 fps, q40, 320 px: ~0.2 Mbps
    {30.0, -30, 0.5},       // 1 fps, q40, 240 px
  }};
  const auto & s = kSteps[static_cast<std::size_t>(std::clamp(level, 0, kMaxLevel))];
  return {std::max(1.0, base_fps / s.fps_div), std::clamp(base_quality + s.quality_delta, 20, 100),
          s.width_scale};
}

struct Feedback {
  std::uint32_t session_id = 0;
  std::uint32_t last_frame_id = 0;
  std::uint32_t last_frame_age_us = 0;
  std::uint32_t frames_received = 0;
};

struct AdaptConfig {
  std::int64_t high_delay_us = 150'000;  // above: step down
  std::int64_t low_delay_us = 75'000;    // below (and little loss) for up_hold: step up
  double high_loss = 0.10;
  double low_loss = 0.02;
  std::int64_t up_hold_us = 5'000'000;       // doubled after a step up that fails within probe_us
  std::int64_t max_up_hold_us = 40'000'000;
  std::int64_t probe_us = 10'000'000;
  std::int64_t stale_us = 1'000'000;         // feedback stopped: step down once per stale_us
  std::int64_t forget_us = 10'000'000;       // a session without feedback this long is gone
};

class RateController {
public:
  explicit RateController(AdaptConfig c = {}) : c_(c), up_hold_us_(c.up_hold_us) {}

  int level() const { return level_; }
  std::int64_t up_hold_us() const { return up_hold_us_; }
  /// The last measured delay (any session), for logs.
  std::optional<std::int64_t> last_delay_us() const { return last_delay_us_; }

  /// A frame went out at now_us.
  void on_sent(std::uint32_t frame_id, std::int64_t now_us) {
    slots_[frame_id % slots_.size()] = {frame_id, now_us};
    last_sent_id_ = frame_id;
  }

  /// Feedback from a Deck arrived at now_us.
  void on_feedback(const Feedback & f, std::int64_t now_us) {
    auto & s = sessions_[f.session_id];
    s.last_us = now_us;
    any_feedback_us_ = now_us;
    if (s.have_prev) {
      const std::uint32_t d_ids = f.last_frame_id - s.prev_id;
      const std::uint32_t d_rx = f.frames_received - s.prev_rx;
      if (d_ids < 100'000 && d_rx < 100'000) {  // not a restart on either side
        s.window.push_back({d_rx, d_ids});
        if (s.window.size() > kLossReports) {
          s.window.pop_front();
        }
      }
    }
    s.have_prev = true;
    s.prev_id = f.last_frame_id;
    s.prev_rx = f.frames_received;

    const auto delay = delay_us(f, now_us);
    if (delay) {
      last_delay_us_ = delay;
    }
    const double loss = s.loss();
    const bool bad = (delay && *delay > c_.high_delay_us) || loss > c_.high_loss;
    const bool warm = bad || (delay && *delay > c_.low_delay_us) || loss > c_.low_loss;
    if (warm) {
      last_warm_us_ = now_us;
    }
    // Act only on frames sent after the last change, and not while the queue the previous
    // level built is still draining (delay falling report to report): the frames sent just
    // after a change wait behind it, which says nothing about the new level.
    const bool after_change = static_cast<std::int32_t>(f.last_frame_id - change_frame_id_) > 0;
    const bool draining = delay && s.prev_delay_us && *delay < *s.prev_delay_us - kDrainingUs;
    if (delay) {
      s.prev_delay_us = delay;
    }
    if (bad && after_change && !draining) {
      step_down(now_us, delay && *delay > 4 * c_.high_delay_us ? 2 : 1);
    }
  }

  /// Call regularly while a Deck is watching.
  void tick(std::int64_t now_us) {
    for (auto it = sessions_.begin(); it != sessions_.end();) {
      it = now_us - it->second.last_us > c_.forget_us ? sessions_.erase(it) : std::next(it);
    }
    if (sessions_.empty()) {
      any_feedback_us_ = 0;
    }
    if (any_feedback_us_ != 0 && now_us - any_feedback_us_ > c_.stale_us) {
      // Feedback stopped while a Deck still watches: the link is probably in trouble.
      last_warm_us_ = now_us;
      if (now_us - last_change_us_ >= c_.stale_us) {
        step_down(now_us, 1);
      }
      return;
    }
    // No feedback at all (an app older than interface minor 1, or one just connecting) or
    // a good link: step back up gradually.
    if (level_ > 0 && now_us - last_warm_us_ >= up_hold_us_ && now_us - last_change_us_ >= up_hold_us_) {
      --level_;
      last_up_us_ = now_us;
      changed(now_us);
    }
    if (last_up_us_ != 0 && now_us - last_up_us_ > c_.probe_us) {
      up_hold_us_ = c_.up_hold_us;  // the last step up held
      last_up_us_ = 0;
    }
  }

  /// No Deck watching any more: forget sessions, keep the level, and wait a hold before
  /// stepping up (a Deck that's reconnecting resumes from here).
  void reset_sessions(std::int64_t now_us) {
    sessions_.clear();
    any_feedback_us_ = 0;
    last_warm_us_ = now_us;
  }

private:
  struct Session {
    std::int64_t last_us = 0;
    bool have_prev = false;
    std::uint32_t prev_id = 0;
    std::uint32_t prev_rx = 0;
    std::deque<std::pair<std::uint32_t, std::uint32_t>> window;  // (received, sent) per report
    std::optional<std::int64_t> prev_delay_us;
    /// Loss over the last kLossReports reports (~1 s). Only once the window is full: it's
    /// cleared on each change, so one loss burst (often radio, not rate) costs one step per
    /// second rather than one per report.
    double loss() const {
      if (window.size() < kLossReports) {
        return 0.0;
      }
      std::uint64_t rx = 0;
      std::uint64_t ids = 0;
      for (const auto & [r, i] : window) {
        rx += r;
        ids += i;
      }
      if (ids < 5) {  // too few frames to tell
        return 0.0;
      }
      return rx >= ids ? 0.0 : 1.0 - static_cast<double>(rx) / static_cast<double>(ids);
    }
  };

  std::optional<std::int64_t> delay_us(const Feedback & f, std::int64_t now_us) const {
    if (f.last_frame_id == 0) {
      return std::nullopt;
    }
    const auto & slot = slots_[f.last_frame_id % slots_.size()];
    if (slot.id == f.last_frame_id && slot.us > 0) {
      return now_us - slot.us - static_cast<std::int64_t>(f.last_frame_age_us);
    }
    // Not in the log: either older than it (nothing received for a long time: very late), or
    // from before a deck_video restart (ahead of what we've sent: no information).
    const auto behind = static_cast<std::int32_t>(last_sent_id_ - f.last_frame_id);
    if (behind > 0) {
      return 10 * c_.high_delay_us;
    }
    return std::nullopt;
  }

  void step_down(std::int64_t now_us, int steps) {
    if (level_ >= kMaxLevel) {
      return;
    }
    if (last_up_us_ != 0 && now_us - last_up_us_ < c_.probe_us) {
      up_hold_us_ = std::min(2 * up_hold_us_, c_.max_up_hold_us);  // that step up failed
      last_up_us_ = 0;
    }
    level_ = std::min(kMaxLevel, level_ + steps);
    changed(now_us);
  }

  void changed(std::int64_t now_us) {
    last_change_us_ = now_us;
    change_frame_id_ = last_sent_id_;
    for (auto & [id, s] : sessions_) {
      s.window.clear();
    }
  }

  static constexpr std::int64_t kDrainingUs = 5'000;
  static constexpr std::size_t kLossReports = 5;  // ~1 s at 5 Hz

  struct Slot {
    std::uint32_t id = 0;
    std::int64_t us = 0;
  };

  AdaptConfig c_;
  int level_ = 0;
  std::int64_t up_hold_us_;
  std::array<Slot, 1024> slots_{};  // send times: ~34 s at 30 fps
  std::uint32_t last_sent_id_ = 0;
  std::uint32_t change_frame_id_ = 0;
  std::int64_t last_change_us_ = 0;
  std::int64_t last_warm_us_ = 0;
  std::int64_t last_up_us_ = 0;
  std::int64_t any_feedback_us_ = 0;
  std::optional<std::int64_t> last_delay_us_;
  std::map<std::uint32_t, Session> sessions_;
};

}  // namespace deck_video
