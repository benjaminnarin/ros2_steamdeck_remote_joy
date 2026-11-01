cmake_minimum_required(VERSION 3.16)
project(remote_joy_interface CXX)

if(NOT CMAKE_CXX_STANDARD)
  set(CMAKE_CXX_STANDARD 17)
  set(CMAKE_CXX_STANDARD_REQUIRED ON)
endif()

find_package(ament_cmake REQUIRED)
find_package(Protobuf REQUIRED)

include(cmake/zenoh_versions.cmake)
remote_joy_read_zenoh_versions(${CMAKE_CURRENT_SOURCE_DIR}/ZENOH_VERSION)
message(STATUS "remote_joy_interface: Zenoh series ${REMOTE_JOY_ZENOH_SERIES} "
  "(crates ${REMOTE_JOY_ZENOH_CRATES}, ROS zenoh-c >= ${REMOTE_JOY_ZENOH_ROS_MIN})")

set(PROTO_ROOT ${CMAKE_CURRENT_SOURCE_DIR}/proto)
file(GLOB PROTO_FILES CONFIGURE_DEPENDS ${PROTO_ROOT}/remote_joy/v1/*.proto)
set(GEN_DIR ${CMAKE_CURRENT_BINARY_DIR}/generated)
file(MAKE_DIRECTORY ${GEN_DIR})

# Generated C++ library: #include <remote_joy/v1/control.pb.h>, plus the hand-written
# <remote_joy_interface/keys.hpp>.
add_library(${PROJECT_NAME} SHARED ${PROTO_FILES})
protobuf_generate(
  TARGET ${PROJECT_NAME}
  LANGUAGE cpp
  IMPORT_DIRS ${PROTO_ROOT}
  PROTOC_OUT_DIR ${GEN_DIR})
target_include_directories(${PROJECT_NAME} PUBLIC
  $<BUILD_INTERFACE:${GEN_DIR}>
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
  $<INSTALL_INTERFACE:include/${PROJECT_NAME}>)
target_link_libraries(${PROJECT_NAME} PUBLIC protobuf::libprotobuf)
set_target_properties(${PROJECT_NAME} PROPERTIES POSITION_INDEPENDENT_CODE ON)

install(DIRECTORY include/ DESTINATION include/${PROJECT_NAME})
install(DIRECTORY ${GEN_DIR}/ DESTINATION include/${PROJECT_NAME} FILES_MATCHING PATTERN "*.pb.h")
install(TARGETS ${PROJECT_NAME} EXPORT export_${PROJECT_NAME}
  ARCHIVE DESTINATION lib LIBRARY DESTINATION lib RUNTIME DESTINATION bin)
# The .proto files and the Zenoh pin are shared with the Deck's Rust build and other tools.
install(DIRECTORY proto/ DESTINATION share/${PROJECT_NAME}/proto)
install(FILES ZENOH_VERSION DESTINATION share/${PROJECT_NAME})
install(FILES cmake/zenoh_versions.cmake DESTINATION share/${PROJECT_NAME}/cmake)

if(BUILD_TESTING)
  find_package(ament_cmake_gtest REQUIRED)
  ament_add_gtest(test_golden test/test_golden.cpp)
  target_link_libraries(test_golden ${PROJECT_NAME})
  target_compile_definitions(test_golden PRIVATE
    GOLDEN_DIR="${CMAKE_CURRENT_SOURCE_DIR}/test/golden")
endif()

ament_export_targets(export_${PROJECT_NAME} HAS_LIBRARY_TARGET)
ament_export_dependencies(Protobuf)
ament_package(CONFIG_EXTRAS cmake/${PROJECT_NAME}-extras.cmake.in)
