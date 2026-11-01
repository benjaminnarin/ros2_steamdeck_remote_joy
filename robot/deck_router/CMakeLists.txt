cmake_minimum_required(VERSION 3.18)
project(deck_router NONE)

find_package(ament_cmake REQUIRED)

# cargo from rustup; colcon's environment often lacks ~/.cargo/bin.
find_program(CARGO cargo HINTS "$ENV{CARGO_HOME}/bin" "$ENV{HOME}/.cargo/bin")
if(NOT CARGO)
  message(FATAL_ERROR "cargo not found: run scripts/setup_dev.sh (installs Rust via rustup)")
endif()

set(CARGO_TARGET_DIR ${CMAKE_CURRENT_BINARY_DIR}/cargo)
set(BIN ${CARGO_TARGET_DIR}/release/deck_router)
# --locked: build exactly what Cargo.lock says (the quinn-proto fix depends on it).
# Run from the source dir so rustup picks up the repo's rust-toolchain.toml.
add_custom_target(deck_router_cargo ALL
  COMMAND ${CMAKE_COMMAND} -E env "PATH=$ENV{HOME}/.cargo/bin:$ENV{PATH}"
          ${CARGO} build --release --locked --target-dir ${CARGO_TARGET_DIR}
  WORKING_DIRECTORY ${CMAKE_CURRENT_SOURCE_DIR}
  BYPRODUCTS ${BIN}
  USES_TERMINAL
  COMMENT "cargo build deck_router")

install(PROGRAMS ${BIN} DESTINATION lib/${PROJECT_NAME})

if(BUILD_TESTING)
  add_test(NAME cargo_test
    COMMAND ${CMAKE_COMMAND} -E env "PATH=$ENV{HOME}/.cargo/bin:$ENV{PATH}"
            ${CARGO} test --release --locked --target-dir ${CARGO_TARGET_DIR}
    WORKING_DIRECTORY ${CMAKE_CURRENT_SOURCE_DIR})
  # The product router config must pass deck_router's own checks.
  add_test(NAME router_config_check
    COMMAND ${BIN} --check ${CMAKE_CURRENT_SOURCE_DIR}/../deck_gateway/config/router.json5)
endif()

ament_package()
