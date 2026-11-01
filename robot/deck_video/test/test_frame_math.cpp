#include <gtest/gtest.h>

#include "deck_video/frame_math.hpp"

using deck_video::capture_steady_us;
using deck_video::due;
using deck_video::scaled_size;

TEST(FrameMath, ScaledSizeKeepsAspectAndEvenDims) {
  EXPECT_EQ(scaled_size(640, 480, 480), std::make_pair(480, 360));
  EXPECT_EQ(scaled_size(1920, 1080, 480), std::make_pair(480, 270));
  EXPECT_EQ(scaled_size(1280, 721, 480), std::make_pair(480, 270)) << "odd height rounded down to even";
  EXPECT_EQ(scaled_size(320, 240, 480), std::make_pair(320, 240)) << "never upscales";
  EXPECT_EQ(scaled_size(321, 241, 0), std::make_pair(320, 240)) << "0 = no limit, still even";
  EXPECT_EQ(scaled_size(0, 10, 480), std::make_pair(0, 0));
  EXPECT_EQ(scaled_size(4000, 2, 480), std::make_pair(480, 2)) << "at least 2";
}

TEST(FrameMath, CaptureTimeOnTheSteadyClock) {
  // Image stamped 30 ms ago (ROS time) -> 30 ms before steady "now".
  EXPECT_EQ(capture_steady_us(5'000'000, 1'000'000'000'000, 999'970'000'000), 4'970'000u);
  EXPECT_EQ(capture_steady_us(5'000'000, 1'000'000'000'000, 1'000'010'000'000), 5'000'000u)
    << "a stamp from the future reads as now";
  EXPECT_EQ(capture_steady_us(10, 1'000'000'000, 0), 0u) << "never negative";
}

TEST(FrameMath, Pacing) {
  EXPECT_TRUE(due(1'000'000, 0, 30.0)) << "first frame";
  EXPECT_FALSE(due(1'020'000, 1'000'000, 30.0)) << "20 ms < 33 ms";
  EXPECT_TRUE(due(1'030'000, 1'000'000, 30.0)) << "30 ms is within 10% of 33 ms";
  EXPECT_TRUE(due(1'000'001, 1'000'000, 0.0)) << "no limit";
}
