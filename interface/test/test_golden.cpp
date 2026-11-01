// Wire-format pinning: the C++ encoder must match protoc's encoding of each golden case.
#include <google/protobuf/text_format.h>
#include <google/protobuf/util/message_differencer.h>
#include <gtest/gtest.h>

#include <fstream>
#include <memory>
#include <sstream>
#include <string>

#include "remote_joy/v1/action.pb.h"
#include "remote_joy/v1/control.pb.h"
#include "remote_joy/v1/info.pb.h"
#include "remote_joy/v1/status.pb.h"
#include "remote_joy/v1/video.pb.h"
#include "remote_joy_interface/keys.hpp"

namespace v1 = remote_joy::v1;

namespace {

std::string read_file(const std::string & name) {
  std::ifstream in(std::string(GOLDEN_DIR) + "/" + name, std::ios::binary);
  EXPECT_TRUE(in.good()) << "missing golden file " << name;
  std::ostringstream ss;
  ss << in.rdbuf();
  return ss.str();
}

template <typename Msg>
void check_golden(const std::string & name) {
  SCOPED_TRACE(name);
  Msg from_text;
  ASSERT_TRUE(google::protobuf::TextFormat::ParseFromString(read_file(name + ".txtpb"), &from_text));
  const std::string golden = read_file(name + ".bin");
  ASSERT_FALSE(golden.empty());

  EXPECT_EQ(from_text.SerializeAsString(), golden) << "C++ encoding differs from protoc";

  Msg from_bin;
  ASSERT_TRUE(from_bin.ParseFromString(golden));
  EXPECT_TRUE(google::protobuf::util::MessageDifferencer::Equals(from_text, from_bin));
}

}  // namespace

TEST(Golden, Control) { check_golden<v1::Control>("control"); }
TEST(Golden, RobotState) { check_golden<v1::RobotState>("robot_state"); }
TEST(Golden, ClearStopRequest) { check_golden<v1::ClearStopRequest>("clear_stop_request"); }
TEST(Golden, Info) { check_golden<v1::Info>("info"); }
TEST(Golden, Status) { check_golden<v1::Status>("status"); }
TEST(Golden, VideoFrame) { check_golden<v1::VideoFrame>("video_frame"); }

TEST(Golden, ControlFieldValues) {
  v1::Control c;
  ASSERT_TRUE(c.ParseFromString(read_file("control.bin")));
  EXPECT_EQ(c.session_id(), 0xdec0ffeeu);
  EXPECT_EQ(c.seq(), 42u);
  ASSERT_EQ(c.axes_size(), 6);
  EXPECT_FLOAT_EQ(c.axes(v1::AXIS_LEFT_Y), 1.0f);
  EXPECT_FLOAT_EQ(c.axes(v1::AXIS_RIGHT_Y), -0.25f);
  EXPECT_TRUE(c.buttons() & (1u << v1::BUTTON_LB));
  EXPECT_TRUE(c.buttons() & (1u << v1::BUTTON_RB));
  EXPECT_FALSE(c.buttons() & (1u << v1::BUTTON_B));
}

TEST(Indices, MatchJoyOrder) {
  // docs/interface.md § Axis and button indices
  EXPECT_EQ(v1::AXIS_RIGHT_TRIGGER, 5);
  EXPECT_EQ(v1::BUTTON_B, 1);
  EXPECT_EQ(v1::BUTTON_MENU, 6);
  EXPECT_EQ(v1::BUTTON_LB, 9);
  EXPECT_EQ(v1::BUTTON_RB, 10);
  EXPECT_EQ(v1::BUTTON_DPAD_RIGHT, 14);
  EXPECT_EQ(v1::BUTTON_TOUCHPAD, 20);
}

TEST(Keys, Strings) {
  EXPECT_EQ(v1::keys::kControl, "rj/v1/control");
  EXPECT_EQ(v1::keys::action("dock"), "rj/v1/action/dock");
  EXPECT_EQ(v1::encoding("Control"), "application/protobuf;remote_joy.v1.Control");
  EXPECT_EQ(v1::encoding(v1::Control::descriptor()->name()),
            "application/protobuf;" + v1::Control::descriptor()->full_name());
}
