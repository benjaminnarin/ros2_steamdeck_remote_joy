"""TurtleBot 1 (iRobot Create 1 base + RealSense D455) driven from the Steam Deck.

    source ~/create_ws/install/setup.bash && source install/setup.bash
    ros2 launch deck_turtlebot turtlebot.launch.py [base:=true] [camera:=true] [depth:=false]
                                                    [config_dir:=<pair --dir>]

Starts:
  - create_driver (create_bringup create_1.launch, the Lyrical port in ~/create_ws): cmd_vel
    (geometry_msgs/Twist), odom, battery/*, /diagnostics, on /dev/ttyUSB0 (create_bringup's
    config/default.yaml). It stops the wheels when cmd_vel has
    been silent for latch_cmd_duration (0.2 s), the base's stale-command timeout (O13).
  - the RealSense D455 as a component in `camera_container`, colour 640x480 at 30 fps on
    /camera/color/image_raw (depth:=true adds aligned depth). deck_video loads into the same
    container, so frames reach it in-process instead of over DDS.
  - deck_gateway's robot.launch.py: deck_router, gateway_node, teleop_twist_joy (Create speeds,
    config/teleop_turtlebot.yaml) and deck_video.
"""

import os

from ament_index_python.packages import get_package_share_directory
from launch import LaunchDescription
from launch.actions import DeclareLaunchArgument, IncludeLaunchDescription, OpaqueFunction
from launch.launch_description_sources import AnyLaunchDescriptionSource, PythonLaunchDescriptionSource
from launch.substitutions import LaunchConfiguration
from launch_ros.actions import ComposableNodeContainer
from launch_ros.descriptions import ComposableNode

CONTAINER = 'camera_container'
IMAGE = '/camera/color/image_raw'


def _on(context, name):
    return LaunchConfiguration(name).perform(context).lower() in ('1', 'true', 'yes')


def _setup(context):
    share = get_package_share_directory('deck_turtlebot')
    camera = _on(context, 'camera')
    depth = _on(context, 'depth')
    out = []

    if _on(context, 'base'):
        out.append(IncludeLaunchDescription(
            AnyLaunchDescriptionSource(os.path.join(
                get_package_share_directory('create_bringup'), 'launch', 'create_1.launch'))))

    if camera:
        out.append(ComposableNodeContainer(
            name=CONTAINER, namespace='', package='rclcpp_components',
            executable='component_container', output='screen',
            respawn=True, respawn_delay=2.0,
            composable_node_descriptions=[ComposableNode(
                package='realsense2_camera', plugin='realsense2_camera::RealSenseNodeFactory',
                name='camera', namespace='',
                parameters=[{
                    'enable_color': True,
                    'rgb_camera.color_profile': '640,480,30',
                    'enable_depth': depth,
                    'align_depth.enable': depth,
                    'enable_infra1': False,
                    'enable_infra2': False,
                    'enable_gyro': False,
                    'enable_accel': False,
                }])]))

    out.append(IncludeLaunchDescription(
        PythonLaunchDescriptionSource(os.path.join(
            get_package_share_directory('deck_gateway'), 'launch', 'robot.launch.py')),
        launch_arguments={
            'cmd_vel_topic': '/cmd_vel',
            'cmd_vel_stamped': 'false',
            'image_topic': IMAGE,
            'use_video': 'true' if camera else 'false',
            'video_container': CONTAINER if camera else '',
            'gateway_config': os.path.join(share, 'config', 'gateway_turtlebot.yaml'),
            'teleop_config': os.path.join(share, 'config', 'teleop_turtlebot.yaml'),
            'config_dir': LaunchConfiguration('config_dir'),
        }.items()))
    return out


def generate_launch_description():
    return LaunchDescription([
        DeclareLaunchArgument('base', default_value='true',
                              description='Start the Create 1 driver (false: no base, e.g. bench tests)'),
        DeclareLaunchArgument('camera', default_value='true',
                              description='Start the RealSense and video'),
        DeclareLaunchArgument('depth', default_value='false',
                              description='Also stream depth aligned to colour (not sent to the Deck)'),
        DeclareLaunchArgument('config_dir', default_value='~/.config/deck_gateway',
                              description='Pairing state from `pair` (TLS files, router.json5)'),
        OpaqueFunction(function=_setup),
    ])
