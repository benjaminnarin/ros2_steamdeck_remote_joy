"""Moving test pattern on /camera/image_raw (sensor_msgs/Image, bgr8), standing in for a camera.

Each frame shows a frame counter, the ROS time it was stamped with, and a bar that sweeps
across once per second, so dropped frames, stutter and latency are visible on the Deck.
Sensor-data QoS (best effort, depth 1), like a real camera driver.
"""

import math

import cv2
import numpy as np
import rclpy
from rclpy.node import Node
from rclpy.qos import qos_profile_sensor_data
from sensor_msgs.msg import Image


def static_background(width, height):
    """The parts that never change: colour bars and grid."""
    img = np.zeros((height, width, 3), np.uint8)
    # Static colour bars (bottom third) so JPEG artefacts and colour order are easy to judge.
    bars = [(255, 255, 255), (0, 255, 255), (255, 255, 0), (0, 255, 0),
            (255, 0, 255), (0, 0, 255), (255, 0, 0), (0, 0, 0)]
    bw = width // len(bars)
    for i, c in enumerate(bars):
        img[2 * height // 3:, i * bw:(i + 1) * bw] = c
    # Grid
    for x in range(0, width, 40):
        img[: 2 * height // 3, x] = (60, 60, 60)
    for y in range(0, 2 * height // 3, 40):
        img[y, :] = (60, 60, 60)
    cx, cy, r = width - 70, 70, 50
    cv2.circle(img, (cx, cy), r, (200, 200, 200), 2)
    return img


def draw_pattern(width, height, frame_id, t, background=None):
    """BGR test frame for time t (seconds). Pure function, used by the unit test too."""
    img = (static_background(width, height) if background is None else background).copy()
    # Sweeping bar: one pass per second.
    phase = t % 1.0
    x = int(phase * (width - 20))
    img[: 2 * height // 3, x:x + 20] = (0, 200, 255)
    # Rotating clock hand: one turn per 10 s.
    cx, cy, r = width - 70, 70, 50
    a = 2 * math.pi * (t % 10.0) / 10.0
    cv2.line(img, (cx, cy), (int(cx + r * math.sin(a)), int(cy - r * math.cos(a))), (0, 0, 255), 3)
    font = cv2.FONT_HERSHEY_SIMPLEX
    cv2.putText(img, f'deck_sim  #{frame_id}', (12, 36), font, 1.0, (255, 255, 255), 2)
    cv2.putText(img, f't={t:.3f}', (12, 76), font, 1.0, (255, 255, 255), 2)
    return img


class TestCamera(Node):
    def __init__(self):
        super().__init__('test_camera')
        self.width = self.declare_parameter('width', 640).value
        self.height = self.declare_parameter('height', 480).value
        fps = self.declare_parameter('fps', 30.0).value
        self.frame_id = self.declare_parameter('frame_id', 'camera').value
        self.pub = self.create_publisher(Image, 'camera/image_raw', qos_profile_sensor_data)
        self.count = 0
        self.background = static_background(self.width, self.height)
        self.create_timer(1.0 / fps, self.tick)
        self.get_logger().info(f'test pattern {self.width}x{self.height} @ {fps} fps')

    def tick(self):
        now = self.get_clock().now()
        self.count += 1
        img = draw_pattern(self.width, self.height, self.count, now.nanoseconds / 1e9, self.background)
        msg = Image()
        msg.header.stamp = now.to_msg()
        msg.header.frame_id = self.frame_id
        msg.height, msg.width = img.shape[:2]
        msg.encoding = 'bgr8'
        msg.step = msg.width * 3
        msg.data = img.tobytes()
        self.pub.publish(msg)


def main():
    rclpy.init()
    node = TestCamera()
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
