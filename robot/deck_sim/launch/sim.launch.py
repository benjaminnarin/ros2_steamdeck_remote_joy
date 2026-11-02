"""Simulated robot: turtlesim base, test camera and fake status/services.

    ros2 launch deck_sim sim.launch.py [turtlesim:=false] [camera:=true|webcam|false] [camera_device:=/dev/video0]
                                       [gateway:=auto|true|false]
                                       [config_dir:=<pair --dir>] [tls_dir:=<dir with ca.pem robot.pem robot.key>]

With deck_gateway built (gateway:=auto finds it), this also includes its robot.launch.py
(router, gateway, video, teleop) pointed at the sim's topics, so a Deck connects to it like a
real robot. Until then it starts teleop_twist_joy itself, so anything publishing /joy drives
the turtle (e.g. `ros2 run joy game_controller_node`).
"""

import os

from ament_index_python.packages import PackageNotFoundError, get_package_share_directory
from launch import LaunchDescription
from launch.actions import (DeclareLaunchArgument, IncludeLaunchDescription, LogInfo,
                            OpaqueFunction)
from launch.conditions import IfCondition, LaunchConfigurationEquals
from launch.launch_description_sources import PythonLaunchDescriptionSource
from launch.substitutions import LaunchConfiguration
from launch_ros.actions import Node

CMD_VEL = '/turtle1/cmd_vel'
IMAGE = '/camera/image_raw'


def _gateway_share():
    try:
        return get_package_share_directory('deck_gateway')
    except PackageNotFoundError:
        return None


def _robot_side(context):
    share = get_package_share_directory('deck_sim')
    mode = LaunchConfiguration('gateway').perform(context).lower()
    camera = LaunchConfiguration('camera').perform(context)
    if camera not in ('true', 'webcam', 'false'):
        raise RuntimeError(f'camera:={camera}: expected true (test pattern), webcam or false')
    gw = _gateway_share()
    if mode not in ('auto', 'true', 'false'):
        raise RuntimeError(f'gateway:={mode}: expected auto, true or false')
    if mode == 'true' and gw is None:
        raise RuntimeError('gateway:=true but the deck_gateway package is not built/sourced')

    if mode != 'false' and gw is not None:
        return [
            LogInfo(msg='deck_sim: starting the robot side from deck_gateway'),
            IncludeLaunchDescription(
                PythonLaunchDescriptionSource(os.path.join(gw, 'launch', 'robot.launch.py')),
                launch_arguments={
                    'cmd_vel_topic': CMD_VEL,
                    'cmd_vel_stamped': 'false',
                    'image_topic': IMAGE,
                    'use_video': 'false' if camera == 'false' else 'true',
                    'gateway_config': os.path.join(share, 'config', 'gateway_sim.yaml'),
                    'tls_dir': LaunchConfiguration('tls_dir'),
                    'config_dir': LaunchConfiguration('config_dir'),
                }.items()),
        ]

    reason = 'gateway:=false' if mode == 'false' else 'deck_gateway not found'
    return [
        LogInfo(msg=f'deck_sim: {reason}; starting teleop_twist_joy only (drive with any /joy)'),
        Node(
            package='teleop_twist_joy', executable='teleop_node', name='teleop_twist_joy_node',
            parameters=[os.path.join(share, 'config', 'teleop_sim.yaml')],
            remappings=[('cmd_vel', CMD_VEL)]),
    ]


def generate_launch_description():
    return LaunchDescription([
        DeclareLaunchArgument('turtlesim', default_value='true',
                              description='Start the turtlesim window as the base (needs a display)'),
        DeclareLaunchArgument('camera', default_value='true',
                              description='true: test pattern; webcam: a V4L2 webcam (camera_device); '
                                          'false: no camera and no video'),
        DeclareLaunchArgument('camera_device', default_value='/dev/video0',
                              description='Webcam device for camera:=webcam'),
        DeclareLaunchArgument('camera_fps', default_value='30.0'),
        # 480x360 bgr8 (506 KB) crosses processes intact; 640x480 (900 KB) lost ~17% of frames
        # between processes with Fast DDS defaults (devlog 3.11). The webcam is scaled to this width.
        DeclareLaunchArgument('camera_width', default_value='480'),
        DeclareLaunchArgument('camera_height', default_value='360'),
        DeclareLaunchArgument('gateway', default_value='auto',
                              description='auto: include deck_gateway robot.launch.py if built'),
        DeclareLaunchArgument('config_dir', default_value='~/.config/deck_gateway',
                              description="pair's state (tls/, router.json5, pairing.json)"),
        DeclareLaunchArgument('tls_dir', default_value='',
                              description='Router TLS files (default: pair\'s, in ~/.config/deck_gateway/tls)'),

        Node(package='turtlesim', executable='turtlesim_node', name='turtlesim',
             condition=IfCondition(LaunchConfiguration('turtlesim'))),
        Node(package='deck_sim', executable='fake_robot', name='fake_robot',
             parameters=[{'cmd_vel_topic': CMD_VEL, 'cmd_vel_stamped': False,
                          'dock_teleports_turtle': LaunchConfiguration('turtlesim')}]),
        Node(package='deck_sim', executable='test_camera', name='test_camera',
             parameters=[{'fps': LaunchConfiguration('camera_fps'),
                          'width': LaunchConfiguration('camera_width'),
                          'height': LaunchConfiguration('camera_height')}],
             condition=LaunchConfigurationEquals('camera', 'true')),
        Node(package='deck_sim', executable='webcam', name='webcam',
             parameters=[{'device': LaunchConfiguration('camera_device'),
                          'fps': LaunchConfiguration('camera_fps'),
                          'max_width': LaunchConfiguration('camera_width')}],
             condition=LaunchConfigurationEquals('camera', 'webcam')),
        OpaqueFunction(function=_robot_side),
    ])
