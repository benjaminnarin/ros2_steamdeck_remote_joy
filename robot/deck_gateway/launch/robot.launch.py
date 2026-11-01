"""Robot side of the Steam Deck link: router, gateway, teleop (and video, once deck_video exists).

    ros2 launch deck_gateway robot.launch.py tls_dir:=<dir with ca.pem robot.pem robot.key>

Spec: docs/robot.md § Launch. The router (deck_router) and the gateway are respawned if they
exit; the gateway starts with STOP latched, so a respawn never resumes driving by itself.
"""

import os

from ament_index_python.packages import PackageNotFoundError, get_package_share_directory
from launch import LaunchDescription
from launch.actions import DeclareLaunchArgument, LogInfo, OpaqueFunction
from launch.substitutions import LaunchConfiguration
from launch_ros.actions import Node

TLS_FILES = ('ca.pem', 'robot.pem', 'robot.key')


def _share(pkg):
    try:
        return get_package_share_directory(pkg)
    except PackageNotFoundError:
        return None


def _bool(context, name):
    return LaunchConfiguration(name).perform(context).lower() in ('1', 'true', 'yes', 'on')


def _router(context):
    if not _bool(context, 'use_router'):
        return [LogInfo(msg='deck_gateway: use_router:=false, expecting a router on the local endpoint')]
    tls_dir = os.path.expanduser(LaunchConfiguration('tls_dir').perform(context))
    missing = [f for f in TLS_FILES if not os.path.isfile(os.path.join(tls_dir, f))]
    if missing:
        raise RuntimeError(
            f'tls_dir {tls_dir} is missing {", ".join(missing)}. '
            'Create them with `ros2 run deck_gateway pair` (or point tls_dir at existing ones).')
    override = ';'.join([
        f'transport/link/tls/root_ca_certificate="{os.path.join(tls_dir, "ca.pem")}"',
        f'transport/link/tls/listen_certificate="{os.path.join(tls_dir, "robot.pem")}"',
        f'transport/link/tls/listen_private_key="{os.path.join(tls_dir, "robot.key")}"',
    ])
    return [Node(
        package='deck_router', executable='deck_router', name='deck_router', output='screen',
        respawn=True, respawn_delay=1.0,
        additional_env={
            'ZENOH_ROUTER_CONFIG_URI': LaunchConfiguration('router_config').perform(context),
            'ZENOH_CONFIG_OVERRIDE': override,
        })]


def _teleop(context):
    if not _bool(context, 'use_teleop'):
        return []
    share = get_package_share_directory('deck_gateway')
    return [Node(
        package='teleop_twist_joy', executable='teleop_node', name='teleop_twist_joy_node',
        parameters=[os.path.join(share, 'config', 'teleop_twist_joy.yaml'),
                    {'publish_stamped_twist': _bool(context, 'cmd_vel_stamped')}],
        remappings=[('cmd_vel', LaunchConfiguration('cmd_vel_topic').perform(context))])]


def _video(context):
    if not _bool(context, 'use_video'):
        return []
    if _share('deck_video') is None:
        return [LogInfo(msg='deck_gateway: deck_video is not built yet (M2); no video')]
    return [LogInfo(msg='deck_gateway: TODO start deck_video (M2)')]


def generate_launch_description():
    share = get_package_share_directory('deck_gateway')
    return LaunchDescription([
        DeclareLaunchArgument('tls_dir', default_value='~/.config/deck_gateway/tls',
                              description='Router TLS files: ca.pem, robot.pem, robot.key (from pair)'),
        DeclareLaunchArgument('router_config',
                              default_value=os.path.join(share, 'config', 'router.json5')),
        DeclareLaunchArgument('gateway_config',
                              default_value=os.path.join(share, 'config', 'gateway.yaml')),
        DeclareLaunchArgument('use_router', default_value='true'),
        DeclareLaunchArgument('use_teleop', default_value='true'),
        DeclareLaunchArgument('use_video', default_value='true'),
        DeclareLaunchArgument('image_topic', default_value='/camera/image_raw'),
        DeclareLaunchArgument('video_container', default_value=''),
        DeclareLaunchArgument('cmd_vel_topic', default_value='cmd_vel'),
        DeclareLaunchArgument('cmd_vel_stamped', default_value='true',
                              description='TwistStamped (diff_drive_controller) or Twist'),

        OpaqueFunction(function=_router),
        Node(package='deck_gateway', executable='gateway_node', name='deck_gateway',
             output='screen', respawn=True, respawn_delay=1.0,
             parameters=[LaunchConfiguration('gateway_config')]),
        OpaqueFunction(function=_teleop),
        OpaqueFunction(function=_video),
    ])
