cmake_minimum_required(VERSION 3.16)
project(deck_video CXX)

if(NOT CMAKE_CXX_STANDARD)
  set(CMAKE_CXX_STANDARD 17)
  set(CMAKE_CXX_STANDARD_REQUIRED ON)
endif()
if(CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")
  add_compile_options(-Wall -Wextra -Wpedantic -Wshadow -Wconversion -Werror=return-type)
endif()

find_package(ament_cmake REQUIRED)
find_package(rclcpp REQUIRED)
find_package(rclcpp_components REQUIRED)
find_package(sensor_msgs REQUIRED)
find_package(cv_bridge REQUIRED)
find_package(OpenCV REQUIRED COMPONENTS core imgproc imgcodecs)
find_package(remote_joy_interface REQUIRED)
find_package(zenoh_cpp_vendor REQUIRED)
remote_joy_check_ros_zenoh(${zenohc_VERSION})

add_library(video_publisher SHARED src/video_publisher.cpp)
target_include_directories(video_publisher PUBLIC
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>)
target_link_libraries(video_publisher
  rclcpp::rclcpp rclcpp_components::component ${sensor_msgs_TARGETS}
  cv_bridge::cv_bridge ${OpenCV_LIBS}
  remote_joy_interface::remote_joy_interface zenohcxx::zenohc)
rclcpp_components_register_node(video_publisher
  PLUGIN "deck_video::VideoPublisher"
  EXECUTABLE video_publisher_node)

install(TARGETS video_publisher
  ARCHIVE DESTINATION lib LIBRARY DESTINATION lib RUNTIME DESTINATION bin)

if(BUILD_TESTING)
  find_package(ament_cmake_gtest REQUIRED)
  ament_add_gtest(test_frame_math test/test_frame_math.cpp)
  target_include_directories(test_frame_math PRIVATE include)
  ament_add_gtest(test_rate_control test/test_rate_control.cpp)
  target_include_directories(test_rate_control PRIVATE include)
endif()

ament_package()
