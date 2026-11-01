"""Fake robot status and services, so the Deck's status panel, actions and button hints
can be developed without hardware.

Publishes:
  /battery_state  sensor_msgs/BatteryState  1 Hz   drains (faster while moving), charges when docked
  /odom           nav_msgs/Odometry         20 Hz  integrated from cmd_vel
  /diagnostics    DiagnosticArray           1 Hz   battery, motors, lights, dock
  /lights_on      std_msgs/Bool                    latched
Services:
  /dock    std_srvs/Trigger  (a moves_robot action) docks; with turtlesim, teleports the turtle home
  /lights  std_srvs/SetBool
Listens to /joy (free buttons, see docs/interface.md):
  A -> "horn" log line, X -> toggle lights
"""

import time

import rclpy
from diagnostic_msgs.msg import DiagnosticArray, DiagnosticStatus, KeyValue
from geometry_msgs.msg import Twist, TwistStamped
from nav_msgs.msg import Odometry
from rclpy.node import Node
from rclpy.qos import DurabilityPolicy, QoSProfile
from sensor_msgs.msg import BatteryState, Joy
from std_msgs.msg import Bool
from std_srvs.srv import SetBool, Trigger

from deck_sim import model

BUTTON_A = 0
BUTTON_X = 2
DOCK_POSE = (5.544445, 5.544445, 0.0)  # turtlesim's spawn point


class FakeRobot(Node):
    def __init__(self):
        super().__init__('fake_robot')
        p = self.declare_parameter
        cmd_vel_topic = p('cmd_vel_topic', '/turtle1/cmd_vel').value
        stamped = p('cmd_vel_stamped', False).value
        self.cmd_vel_timeout = p('cmd_vel_timeout_s', 0.5).value
        self.teleport_on_dock = p('dock_teleports_turtle', True).value
        self.battery = model.Battery(
            pct=p('battery_start_pct', 100.0).value,
            idle_drain_pct_per_min=p('idle_drain_pct_per_min', 1.0).value,
            moving_drain_pct_per_min=p('moving_drain_pct_per_min', 4.0).value,
            charge_pct_per_min=p('charge_pct_per_min', 20.0).value,
        )

        self.pose = model.Pose2D()
        self.v = 0.0
        self.w = 0.0
        self.last_cmd = 0.0
        self.docked = False
        self.lights = False
        self.prev_buttons = []
        self.horns = 0
        self.last_step = time.monotonic()

        self.battery_pub = self.create_publisher(BatteryState, 'battery_state', 10)
        self.odom_pub = self.create_publisher(Odometry, 'odom', 10)
        self.diag_pub = self.create_publisher(DiagnosticArray, '/diagnostics', 10)
        latched = QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL)
        self.lights_pub = self.create_publisher(Bool, 'lights_on', latched)

        if stamped:
            self.create_subscription(TwistStamped, cmd_vel_topic, lambda m: self.on_cmd(m.twist), 10)
        else:
            self.create_subscription(Twist, cmd_vel_topic, self.on_cmd, 10)
        self.create_subscription(Joy, 'joy', self.on_joy, 10)
        self.create_service(Trigger, 'dock', self.on_dock)
        self.create_service(SetBool, 'lights', self.on_lights)

        self.teleport = None
        if self.teleport_on_dock:
            try:
                from turtlesim_msgs.srv import TeleportAbsolute
            except ImportError:  # Jazzy keeps the services in turtlesim
                from turtlesim.srv import TeleportAbsolute
            self.TeleportAbsolute = TeleportAbsolute
            self.teleport = self.create_client(TeleportAbsolute, '/turtle1/teleport_absolute')

        self.create_timer(1.0 / 20, self.step)
        self.create_timer(1.0, self.publish_slow)
        self.publish_lights()
        self.get_logger().info(f'fake robot up: cmd_vel={cmd_vel_topic} (stamped={stamped})')

    # ── inputs ──────────────────────────────────────────────
    def on_cmd(self, twist):
        self.v = twist.linear.x
        self.w = twist.angular.z
        self.last_cmd = time.monotonic()
        if self.docked and (abs(self.v) > 1e-3 or abs(self.w) > 1e-3):
            self.docked = False
            self.get_logger().info('undocked')

    def on_joy(self, msg):
        for i in model.rising_edges(self.prev_buttons, list(msg.buttons)):
            if i == BUTTON_A:
                self.horns += 1
                self.get_logger().info(f'HONK (A pressed, #{self.horns})')
            elif i == BUTTON_X:
                self.set_lights(not self.lights, 'X button')
        self.prev_buttons = list(msg.buttons)

    def on_dock(self, _req, res):
        if self.docked:
            res.success, res.message = False, 'already docked'
            return res
        self.docked = True
        self.v = self.w = 0.0
        self.pose = model.Pose2D()
        if self.teleport is not None and self.teleport.service_is_ready():
            x, y, th = DOCK_POSE
            self.teleport.call_async(self.TeleportAbsolute.Request(x=x, y=y, theta=th))
        self.get_logger().info('docked (via /dock)')
        res.success, res.message = True, 'docked, charging'
        return res

    def on_lights(self, req, res):
        self.set_lights(req.data, '/lights')
        res.success, res.message = True, f'lights {"on" if req.data else "off"}'
        return res

    def set_lights(self, on, source):
        self.lights = on
        self.get_logger().info(f'lights {"on" if on else "off"} ({source})')
        self.publish_lights()

    # ── outputs ─────────────────────────────────────────────
    def moving(self):
        return abs(self.v) > 1e-3 or abs(self.w) > 1e-3

    def step(self):
        now = time.monotonic()
        dt, self.last_step = now - self.last_step, now
        if now - self.last_cmd > self.cmd_vel_timeout:
            self.v = self.w = 0.0  # same stale-command behaviour a real base must have
        self.pose = model.integrate(self.pose, self.v, self.w, dt)
        self.battery.step(dt, self.moving(), self.docked)

        o = Odometry()
        o.header.stamp = self.get_clock().now().to_msg()
        o.header.frame_id = 'odom'
        o.child_frame_id = 'base_link'
        o.pose.pose.position.x = self.pose.x
        o.pose.pose.position.y = self.pose.y
        o.pose.pose.orientation.z, o.pose.pose.orientation.w = _yaw_quat(self.pose.theta)
        o.twist.twist.linear.x = self.v
        o.twist.twist.angular.z = self.w
        self.odom_pub.publish(o)

    def publish_slow(self):
        stamp = self.get_clock().now().to_msg()
        moving = self.moving()
        b = BatteryState()
        b.header.stamp = stamp
        b.percentage = self.battery.pct / 100.0  # ROS convention: 0..1
        b.voltage = self.battery.voltage
        b.current = self.battery.current(moving, self.docked)
        b.present = True
        b.power_supply_status = (
            BatteryState.POWER_SUPPLY_STATUS_CHARGING if self.docked
            else BatteryState.POWER_SUPPLY_STATUS_DISCHARGING)
        b.power_supply_technology = BatteryState.POWER_SUPPLY_TECHNOLOGY_LION
        self.battery_pub.publish(b)

        level = model.battery_level(self.battery.pct)
        d = DiagnosticArray()
        d.header.stamp = stamp
        d.status = [
            _status('fake_robot: battery', level,
                    ['ok', 'low', 'critical'][level] + f' ({self.battery.pct:.0f}%)',
                    voltage=f'{self.battery.voltage:.2f}'),
            _status('fake_robot: motors', model.OK, 'driving' if moving else 'idle',
                    v=f'{self.v:.2f}', w=f'{self.w:.2f}'),
            _status('fake_robot: lights', model.OK, 'on' if self.lights else 'off'),
            _status('fake_robot: dock', model.OK, 'docked' if self.docked else 'undocked'),
        ]
        self.diag_pub.publish(d)

    def publish_lights(self):
        self.lights_pub.publish(Bool(data=self.lights))


def _yaw_quat(theta):
    import math
    return math.sin(theta / 2), math.cos(theta / 2)


def _status(name, level, message, **values):
    s = DiagnosticStatus()
    s.level = bytes([level])
    s.name = name
    s.message = message
    s.hardware_id = 'deck_sim'
    s.values = [KeyValue(key=k, value=v) for k, v in values.items()]
    return s


def main():
    rclpy.init()
    node = FakeRobot()
    try:
        rclpy.spin(node)
    except KeyboardInterrupt:
        pass
    except Exception:
        if rclpy.ok():
            raise
    finally:
        node.destroy_node()
        rclpy.try_shutdown()


if __name__ == '__main__':
    main()
