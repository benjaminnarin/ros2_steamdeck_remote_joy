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

# ── the node and the fake Deck ──
find_package(rclcpp REQUIRED)
find_package(sensor_msgs REQUIRED)
find_package(std_msgs REQUIRED)
find_package(std_srvs REQUIRED)
find_package(diagnostic_updater REQUIRED)
find_package(remote_joy_interface REQUIRED)
find_package(zenoh_cpp_vendor REQUIRED)
# ROS's zenoh-c must be in the pinned series (interface/ZENOH_VERSION): Lyrical 0.10.6 is 1.8.0.
remote_joy_check_ros_zenoh(${zenohc_VERSION})
message(STATUS "deck_gateway: zenoh-c ${zenohc_VERSION} (series ${REMOTE_JOY_ZENOH_SERIES})")

add_executable(gateway_node src/gateway_node.cpp)
target_link_libraries(gateway_node
  deck_gateway_core
  remote_joy_interface::remote_joy_interface
  zenohcxx::zenohc
  rclcpp::rclcpp
  ${sensor_msgs_TARGETS} ${std_msgs_TARGETS} ${std_srvs_TARGETS}
  diagnostic_updater::diagnostic_updater)

add_executable(fake_deck src/fake_deck.cpp)
target_link_libraries(fake_deck remote_joy_interface::remote_joy_interface zenohcxx::zenohc)

install(TARGETS gateway_node fake_deck DESTINATION lib/${PROJECT_NAME})
install(PROGRAMS scripts/pair DESTINATION lib/${PROJECT_NAME})
install(DIRECTORY config launch DESTINATION share/${PROJECT_NAME})

if(BUILD_TESTING)
  find_package(ament_cmake_gtest REQUIRED)
  ament_add_gtest(test_core
    test/core/test_safety_core.cpp
    test/core/test_joy_mapping.cpp)
  target_link_libraries(test_core deck_gateway_core)

  find_package(ament_cmake_pytest REQUIRED)
  ament_add_pytest_test(test_pair test/test_pair.py TIMEOUT 120)

  # The real robot.launch.py driven by fake_deck. Own ports (in the test) and ROS domain, so it
  # can run next to a live robot or deck_sim.
  find_package(launch_testing_ament_cmake REQUIRED)
  add_launch_test(test/launch/test_gateway.launch.py
    TARGET test_gateway_launch
    TIMEOUT 240
    ENV ROS_DOMAIN_ID=73 ROS_AUTOMATIC_DISCOVERY_RANGE=LOCALHOST)
endif()

ament_package()
