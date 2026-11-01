// Small pure helpers for deck_video, kept apart so they're unit-tested without ROS or Zenoh.
#pragma once

#include <algorithm>
#include <cstdint>
#include <utility>

namespace deck_video {

/// Output size for a w x h image limited to max_width, keeping the aspect ratio. Both dimensions
/// are even (JPEG chroma subsampling) and at least 2. Never upscales.
inline std::pair<int, int> scaled_size(int w, int h, int max_width) {
  if (w <= 0 || h <= 0) {
    return {0, 0};
  }
  int ow = w;
  int oh = h;
  if (max_width > 0 && w > max_width) {
    ow = max_width;
    oh = static_cast<int>((static_cast<long long>(h) * max_width + w / 2) / w);
  }
  ow = std::max(2, ow - (ow % 2));
  oh = std::max(2, oh - (oh % 2));
  return {ow, oh};
}

/// Converts an image's ROS header stamp to the steady (monotonic) clock the gateway uses for
/// robot_time_us: steady_now - (ros_now - stamp). Clamped so a future stamp reads as "now".
inline std::uint64_t capture_steady_us(std::int64_t steady_now_us, std::int64_t ros_now_ns,
                                       std::int64_t stamp_ns) {
  std::int64_t age_us = (ros_now_ns - stamp_ns) / 1000;
  if (age_us < 0) {
    age_us = 0;
  }
  const std::int64_t t = steady_now_us - age_us;
  return t > 0 ? static_cast<std::uint64_t>(t) : 0;
}

/// Frame pacing: true if a frame may be sent at `now_us`, given the last send and the fps limit.
/// Allows 10% early so timer jitter doesn't halve the rate.
inline bool due(std::int64_t now_us, std::int64_t last_sent_us, double max_fps) {
  if (max_fps <= 0.0 || last_sent_us <= 0) {
    return true;
  }
  const auto period_us = static_cast<std::int64_t>(1e6 / max_fps);
  return now_us - last_sent_us >= period_us * 9 / 10;
}

}  // namespace deck_video
