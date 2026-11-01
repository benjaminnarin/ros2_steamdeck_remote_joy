// Control (wire) -> sensor_msgs/Joy layout, as ROS 2 joy's game_controller_node publishes it.
// No ROS here: the node copies JoyOut into a sensor_msgs/Joy.
#pragma once

#include <array>
#include <cstddef>
#include <cstdint>

namespace deck_gateway::core {

inline constexpr std::size_t kJoyAxes = 6;
/// joy's 21 buttons, plus index 21: "deadman held", made by the gateway (the Deck's deadman can be
/// an analog trigger, and teleop_twist_joy's enable_button must be a button).
inline constexpr std::size_t kJoyButtons = 22;
inline constexpr std::size_t kJoyWireButtons = 21;
inline constexpr std::size_t kJoyDeadmanButton = 21;

struct JoyOut {
  std::array<float, kJoyAxes> axes{};
  std::array<std::int32_t, kJoyButtons> buttons{};
  bool operator==(const JoyOut & o) const { return axes == o.axes && buttons == o.buttons; }
};

/// All axes 0, all buttons released. Note: joy's triggers rest at 0 too (0 released .. -1 pressed).
JoyOut neutral_joy();

/// Wire axes: sticks +1 = left/up, triggers 0 (released) .. 1 (pressed); see docs/interface.md.
/// Joy: sticks unchanged, triggers 0 .. -1. Values are clamped to [-1, 1] and NaN/inf become 0.
/// Missing axes (n < 6) read as 0; axes beyond 6 (reserved) are dropped. Button bits 21-31 are
/// dropped. buttons[kJoyDeadmanButton] is left 0: the safety core sets it.
JoyOut to_joy(const float * axes, std::size_t n, std::uint32_t buttons);

/// One sanitized wire axis value (clamp to [lo, hi], NaN/inf -> 0).
float sanitize_axis(float v, float lo, float hi);

}  // namespace deck_gateway::core
