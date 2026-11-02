// Zenoh key expressions and encodings for the Deck <-> robot interface v1.
// Must match controller/deck_msgs/src/keys.rs and docs/interface.md § Key space.
#pragma once

#include <cstdint>
#include <string>
#include <string_view>

namespace remote_joy::v1 {

inline constexpr std::uint32_t kInterfaceVersion = 1;
inline constexpr std::uint32_t kInterfaceMinor = 1;  // 1: VideoFeedback, VideoFrame.level

namespace keys {
inline constexpr std::string_view kPrefix = "rj/v1";
inline constexpr std::string_view kControl = "rj/v1/control";
inline constexpr std::string_view kStop = "rj/v1/stop";
inline constexpr std::string_view kRobotState = "rj/v1/robot_state";
inline constexpr std::string_view kStatus = "rj/v1/status";
inline constexpr std::string_view kVideo = "rj/v1/video";
inline constexpr std::string_view kVideoFeedback = "rj/v1/video_feedback";
inline constexpr std::string_view kInfo = "rj/v1/info";
inline constexpr std::string_view kClearStop = "rj/v1/clear_stop";
inline constexpr std::string_view kActionPrefix = "rj/v1/action";  // + "/<id>"
inline constexpr std::string_view kActionAll = "rj/v1/action/*";
inline constexpr std::string_view kDeckPrefix = "rj/v1/deck";  // + "/<session_id as 8 hex digits>"
inline constexpr std::string_view kDeckAll = "rj/v1/deck/*";

inline std::string action(std::string_view id) {
  return std::string(kActionPrefix) + "/" + std::string(id);
}
}  // namespace keys

// Zenoh payload encoding: "application/protobuf;remote_joy.v1.<Message>".
inline std::string encoding(std::string_view message) {
  return "application/protobuf;remote_joy.v1." + std::string(message);
}

}  // namespace remote_joy::v1
