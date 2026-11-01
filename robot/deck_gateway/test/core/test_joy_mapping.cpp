#include <gtest/gtest.h>

#include <cmath>
#include <limits>

#include "deck_gateway/core/joy_mapping.hpp"

using namespace deck_gateway::core;

TEST(JoyMapping, SticksUnchangedTriggersNegated) {
  const float axes[6] = {0.5f, -1.0f, 0.25f, 1.0f, 0.0f, 1.0f};
  const JoyOut j = to_joy(axes, 6, 0);
  EXPECT_FLOAT_EQ(j.axes[0], 0.5f);
  EXPECT_FLOAT_EQ(j.axes[1], -1.0f);
  EXPECT_FLOAT_EQ(j.axes[2], 0.25f);
  EXPECT_FLOAT_EQ(j.axes[3], 1.0f);
  EXPECT_FLOAT_EQ(j.axes[4], 0.0f);
  EXPECT_FALSE(std::signbit(j.axes[4])) << "released trigger is +0, not -0";
  EXPECT_FLOAT_EQ(j.axes[5], -1.0f);
}

TEST(JoyMapping, ClampsAndRejectsNonFinite) {
  const float inf = std::numeric_limits<float>::infinity();
  const float axes[6] = {2.0f, -3.0f, std::nanf(""), inf, -0.5f, 7.0f};
  const JoyOut j = to_joy(axes, 6, 0);
  EXPECT_FLOAT_EQ(j.axes[0], 1.0f);
  EXPECT_FLOAT_EQ(j.axes[1], -1.0f);
  EXPECT_FLOAT_EQ(j.axes[2], 0.0f);
  EXPECT_FLOAT_EQ(j.axes[3], 0.0f);
  EXPECT_FLOAT_EQ(j.axes[4], 0.0f) << "negative trigger clamps to released";
  EXPECT_FLOAT_EQ(j.axes[5], -1.0f);
}

TEST(JoyMapping, MissingAndExtraAxes) {
  const float two[2] = {0.3f, 0.4f};
  JoyOut j = to_joy(two, 2, 0);
  EXPECT_FLOAT_EQ(j.axes[1], 0.4f);
  EXPECT_FLOAT_EQ(j.axes[5], 0.0f);
  j = to_joy(nullptr, 0, 0);
  EXPECT_EQ(j, neutral_joy());
  const float twelve[12] = {0, 0, 0, 0, 0, 0, 9, 9, 9, 9, 9, 9};
  EXPECT_EQ(to_joy(twelve, 12, 0), neutral_joy()) << "reserved axes 6-11 are dropped";
}

TEST(JoyMapping, Buttons) {
  const JoyOut j = to_joy(nullptr, 0, (1u << 0) | (1u << 9) | (1u << 20) | (1u << 21) | (1u << 31));
  EXPECT_EQ(j.buttons[0], 1);
  EXPECT_EQ(j.buttons[1], 0);
  EXPECT_EQ(j.buttons[9], 1);
  EXPECT_EQ(j.buttons[20], 1);
  int pressed = 0;
  for (int b : j.buttons) {
    pressed += b;
  }
  EXPECT_EQ(pressed, 3) << "bits 21-31 are reserved and dropped";
}
