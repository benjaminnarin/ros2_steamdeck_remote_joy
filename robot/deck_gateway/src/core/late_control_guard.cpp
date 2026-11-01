#include "deck_gateway/core/late_control_guard.hpp"

namespace deck_gateway::core {

LateControlGuard::LateControlGuard(std::int64_t window_us, std::int64_t max_delay_us)
: window_us_(window_us), max_delay_us_(max_delay_us) {}

bool LateControlGuard::record_and_check(std::int64_t arrival_us, std::uint64_t deck_time_us) {
  // Two's-complement wrap keeps the difference meaningful even if the clocks are far apart.
  const auto d = static_cast<std::int64_t>(
    static_cast<std::uint64_t>(arrival_us) - deck_time_us);
  while (!samples_.empty() && samples_.back().second >= d) {
    samples_.pop_back();
  }
  samples_.emplace_back(arrival_us, d);
  while (samples_.front().first < arrival_us - window_us_) {
    samples_.pop_front();  // never empties: the sample just added is inside the window
  }
  last_excess_us_ = d - samples_.front().second;
  return last_excess_us_ > max_delay_us_;
}

void LateControlGuard::reset() {
  samples_.clear();
  last_excess_us_ = 0;
}

}  // namespace deck_gateway::core
