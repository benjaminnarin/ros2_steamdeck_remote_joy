cmake_minimum_required(VERSION 3.16)
project(deck_gateway CXX)

if(NOT CMAKE_CXX_STANDARD)
  set(CMAKE_CXX_STANDARD 17)
  set(CMAKE_CXX_STANDARD_REQUIRED ON)
endif()
if(CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")
  add_compile_options(-Wall -Wextra -Wpedantic -Wshadow -Wconversion -Werror=return-type)
endif()

find_package(ament_cmake REQUIRED)

# ── deck_gateway_core: the safety logic. No ROS, no Zenoh, no Protobuf. ──
add_library(deck_gateway_core STATIC
  src/core/joy_mapping.cpp
  src/core/late_control_guard.cpp
  src/core/safety_core.cpp)
target_include_directories(deck_gateway_core PUBLIC
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>)
set_target_properties(deck_gateway_core PROPERTIES POSITION_INDEPENDENT_CODE ON)

install(DIRECTORY config DESTINATION share/${PROJECT_NAME})

if(BUILD_TESTING)
  find_package(ament_cmake_gtest REQUIRED)
  ament_add_gtest(test_core
    test/core/test_safety_core.cpp
    test/core/test_joy_mapping.cpp)
  target_link_libraries(test_core deck_gateway_core)
endif()

ament_package()
