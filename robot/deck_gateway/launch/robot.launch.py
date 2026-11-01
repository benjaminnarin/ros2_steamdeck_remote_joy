"""Robot side of the Steam Deck link: router, gateway, teleop (and video, once deck_video exists).

    ros2 run deck_gateway pair --deck deck1 --host <robot address>    # once per Deck
    ros2 launch deck_gateway robot.launch.py

By default everything comes from `pair`'s state in config_dir (~/.config/deck_gateway):
tls/ (certificates), router.json5 (with the paired Decks' allow list) and pairing.json (robot
name). Override with tls_dir:=, router_config:=.

Spec: docs/robot.md § Launch. The router (deck_router) and the gateway are respawned if they
exit; the gateway starts with STOP latched, so a respawn never resumes driving by itself.
"""

import base64
import hashlib
import json
import os

from ament_index_python.packages import PackageNotFoundError, get_package_share_directory
from launch import LaunchDescription
from launch.actions import DeclareLaunchArgument, LogInfo, OpaqueFunction
from launch.substitutions import LaunchConfiguration
from launch_ros.actions import ComposableNodeContainer, LoadComposableNodes, Node
from launch_ros.descriptions import ComposableNode

TLS_FILES = ('ca.pem', 'robot.pem', 'robot.key')


def _share(pkg):
    try:
        return get_package_share_directory(pkg)
    except PackageNotFoundError:
        return None


def _config_dir(context):
    return os.path.expanduser(LaunchConfiguration('config_dir').perform(context))


def _tls_dir(context):
    tls = LaunchConfiguration('tls_dir').perform(context)
    return os.path.expanduser(tls) if tls else os.path.join(_config_dir(context), 'tls')


def _robot_id(tls_dir):
    """SHA-256 of the CA certificate's DER, hex (what pair writes into each bundle)."""
    try:
        pem = open(os.path.join(tls_dir, 'ca.pem')).read()
    except OSError:
        return ''
    body = pem.split('-----BEGIN CERTIFICATE-----')[1].split('-----END CERTIFICATE-----')[0]
    return hashlib.sha256(base64.b64decode(''.join(body.split()))).hexdigest()


def _bool(context, name):
    return LaunchConfiguration(name).perform(context).lower() in ('1', 'true', 'yes', 'on')


def _router(context):
    if not _bool(context, 'use_router'):
        return [LogInfo(msg='deck_gateway: use_router:=false, expecting a router on the local endpoint')]
    tls_dir = _tls_dir(context)
    missing = [f for f in TLS_FILES if not os.path.isfile(os.path.join(tls_dir, f))]
    if missing:
        raise RuntimeError(
            f'tls_dir {tls_dir} is missing {", ".join(missing)}. '
            'Create them with `ros2 run deck_gateway pair --deck <name> --host <address>` '
            '(or point tls_dir at existing ones).')
    router_config = LaunchConfiguration('router_config').perform(context)
    if not router_config:
        generated = os.path.join(_config_dir(context), 'router.json5')
        router_config = generated if os.path.isfile(generated) else os.path.join(
            get_package_share_directory('deck_gateway'), 'config', 'router.json5')
    override = ';'.join([
        f'transport/link/tls/root_ca_certificate="{os.path.join(tls_dir, "ca.pem")}"',
        f'transport/link/tls/listen_certificate="{os.path.join(tls_dir, "robot.pem")}"',
        f'transport/link/tls/listen_private_key="{os.path.join(tls_dir, "robot.key")}"',
    ])
    return [Node(
        package='deck_router', executable='deck_router', name='deck_router', output='screen',
        respawn=True, respawn_delay=1.0,
        additional_env={
            'ZENOH_ROUTER_CONFIG_URI': router_config,
            'ZENOH_CONFIG_OVERRIDE': override,
        })]


def _gateway(context):
    overrides = {'robot_id': _robot_id(_tls_dir(context)),
                 # Info.camera (the actual frame size is in each VideoFrame)
                 'camera_available': _bool(context, 'use_video') and _share('deck_video') is not None,
                 'camera_max_width': int(LaunchConfiguration('video_max_width').perform(context)),
                 'camera_max_fps': int(float(LaunchConfiguration('video_fps').perform(context)))}
    try:
        with open(os.path.join(_config_dir(context), 'pairing.json')) as f:
            name = json.load(f).get('robot_name')
        if name:
            overrides['robot_name'] = name
    except (OSError, ValueError):
        pass
    return [Node(package='deck_gateway', executable='gateway_node', name='deck_gateway',
                 output='screen', respawn=True, respawn_delay=1.0,
                 parameters=[LaunchConfiguration('gateway_config').perform(context), overrides])]


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
        return [LogInfo(msg='deck_gateway: deck_video is not built; no video')]
    component = ComposableNode(
        package='deck_video', plugin='deck_video::VideoPublisher', name='deck_video',
        parameters=[{
            'image_topic': LaunchConfiguration('image_topic').perform(context),
            'max_width': int(LaunchConfiguration('video_max_width').perform(context)),
            'fps': float(LaunchConfiguration('video_fps').perform(context)),
            'jpeg_quality': int(LaunchConfiguration('video_quality').perform(context)),
        }])
    container = LaunchConfiguration('video_container').perform(context)
    if container:
        # Into the camera driver's container: images arrive in-process (zero copy). A camera
        # crash then also stops video, which is fine: video isn't on the safety path.
        return [LoadComposableNodes(target_container=container, composable_node_descriptions=[component])]
    return [ComposableNodeContainer(
        name='deck_video_container', namespace='', package='rclcpp_components',
        executable='component_container', output='screen', respawn=True, respawn_delay=1.0,
        composable_node_descriptions=[component])]


def generate_launch_description():
    share = get_package_share_directory('deck_gateway')
    return LaunchDescription([
        DeclareLaunchArgument('config_dir', default_value='~/.config/deck_gateway',
                              description="pair's state: tls/, router.json5, pairing.json"),
        DeclareLaunchArgument('tls_dir', default_value='',
                              description='Router TLS files ca.pem, robot.pem, robot.key '
                                          '(default: <config_dir>/tls)'),
        DeclareLaunchArgument('router_config', default_value='',
                              description='default: <config_dir>/router.json5 if pair made one, '
                                          'else the installed template'),
        DeclareLaunchArgument('gateway_config',
                              default_value=os.path.join(share, 'config', 'gateway.yaml')),
        DeclareLaunchArgument('use_router', default_value='true'),
        DeclareLaunchArgument('use_teleop', default_value='true'),
        DeclareLaunchArgument('use_video', default_value='true'),
        DeclareLaunchArgument('image_topic', default_value='/camera/image_raw'),
        DeclareLaunchArgument('video_container', default_value='',
                              description='load deck_video into this container (e.g. the camera driver\'s)'),
        DeclareLaunchArgument('video_max_width', default_value='480'),
        DeclareLaunchArgument('video_fps', default_value='30'),
        DeclareLaunchArgument('video_quality', default_value='70'),
        DeclareLaunchArgument('cmd_vel_topic', default_value='cmd_vel'),
        DeclareLaunchArgument('cmd_vel_stamped', default_value='true',
                              description='TwistStamped (diff_drive_controller) or Twist'),

        OpaqueFunction(function=_router),
        OpaqueFunction(function=_gateway),
        OpaqueFunction(function=_teleop),
        OpaqueFunction(function=_video),
    ])
