# Reads interface/ZENOH_VERSION (key=value lines) into
#   REMOTE_JOY_ZENOH_SERIES, REMOTE_JOY_ZENOH_CRATES, REMOTE_JOY_ZENOH_ROS_MIN
function(remote_joy_read_zenoh_versions file)
  file(STRINGS "${file}" lines REGEX "^[a-z_]+=")
  foreach(line IN LISTS lines)
    string(REGEX MATCH "^([a-z_]+)=(.*)$" _ "${line}")
    string(TOUPPER "${CMAKE_MATCH_1}" key)
    string(STRIP "${CMAKE_MATCH_2}" value)
    set(REMOTE_JOY_ZENOH_${key} "${value}" PARENT_SCOPE)
  endforeach()
endfunction()

# Fails the configure step unless `version` is in the pinned series and >= ros_min.
# For packages that link ROS's zenoh-c, e.g. remote_joy_check_ros_zenoh(${zenohc_VERSION}).
function(remote_joy_check_ros_zenoh version)
  if(NOT version MATCHES "^${REMOTE_JOY_ZENOH_SERIES}\\." OR version VERSION_LESS REMOTE_JOY_ZENOH_ROS_MIN)
    message(FATAL_ERROR
      "zenoh-c ${version} from zenoh_cpp_vendor is not usable: need ${REMOTE_JOY_ZENOH_SERIES}.x >= "
      "${REMOTE_JOY_ZENOH_ROS_MIN} (interface/ZENOH_VERSION). See docs/deployment.md § Zenoh package version.")
  endif()
endfunction()
