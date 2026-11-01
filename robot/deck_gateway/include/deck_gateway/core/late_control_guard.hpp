// Late-control guard (docs/interface.md § Rules): Wi-Fi sometimes delivers packets 300-500 ms
// late without losing them. Without synchronized clocks, compare each control's
// d = robot_arrival - deck_time with the lowest d seen in the last `window`. Only changes in d
// matter, because the two clocks are unrelated.
#pragma once

#include <cstdint>
#include <deque>
#include <utility>

namespace deck_gateway::core {

class LateControlGuard {
public:
  /// window_us: how long a low d stays the baseline (spec: 10 s).
  /// max_delay_us: how much later than the baseline counts as late (spec: 150 ms).
  LateControlGuard(std::int64_t window_us, std::int64_t max_delay_us);

  /// Records the sample, then returns true if it's more than max_delay later than the baseline.
  /// Samples must come in non-decreasing arrival order.
  bool record_and_check(std::int64_t arrival_us, std::uint64_t deck_time_us);

  /// How much later than the baseline the last recorded sample was (0 if none).
  std::int64_t last_excess_us() const { return last_excess_us_; }

  /// Forget the baseline (new owner, new Deck clock).
  void reset();

private:
  std::int64_t window_us_;
  std::int64_t max_delay_us_;
  std::int64_t last_excess_us_ = 0;
  // Monotonic deque of (arrival, d) with d strictly increasing: front is the window minimum.
  std::deque<std::pair<std::int64_t, std::int64_t>> samples_;
};

}  // namespace deck_gateway::core
