#include "deck_gateway/core/joy_mapping.hpp"

#include <algorithm>
#include <cmath>

namespace deck_gateway::core {

namespace {
constexpr std::size_t kLeftTrigger = 4;
constexpr std::size_t kRightTrigger = 5;
}  // namespace

JoyOut neutral_joy() { return JoyOut{}; }

float sanitize_axis(float v, float lo, float hi) {
  if (!std::isfinite(v)) {
    return 0.0f;
  }
  return std::clamp(v, lo, hi);
}

JoyOut to_joy(const float * axes, std::size_t n, std::uint32_t buttons) {
  JoyOut j;
  for (std::size_t i = 0; i < kJoyAxes; ++i) {
    const float raw = (axes != nullptr && i < n) ? axes[i] : 0.0f;
    if (i == kLeftTrigger || i == kRightTrigger) {
      j.axes[i] = -sanitize_axis(raw, 0.0f, 1.0f);  // 0 released .. 1 pressed  ->  0 .. -1
      if (j.axes[i] == 0.0f) {
        j.axes[i] = 0.0f;  // no -0.0 in the output
      }
    } else {
      j.axes[i] = sanitize_axis(raw, -1.0f, 1.0f);
    }
  }
  for (std::size_t b = 0; b < kJoyButtons; ++b) {
    j.buttons[b] = static_cast<std::int32_t>((buttons >> b) & 1u);
  }
  return j;
}

}  // namespace deck_gateway::core
