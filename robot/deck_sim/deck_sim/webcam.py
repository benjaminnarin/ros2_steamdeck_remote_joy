"""A V4L2 webcam on /camera/image_raw (sensor_msgs/Image, bgr8), for the sim (camera:=webcam).

Like the M0 demo bridge: OpenCV with MJPEG, because typical USB webcams only reach ~15 fps in raw
mode at 640x480. A capture thread keeps only the newest frame, stamped when it was read. Frames
are scaled down to max_width before publishing: 640x480 bgr8 (900 KB) lost ~17% between processes
with Fast DDS defaults, 480x360 crossed intact (devlog 3.11). Sensor-data QoS, like test_camera.
"""

import threading

import cv2
import rclpy
from rclpy.node import Node
from rclpy.qos import qos_profile_sensor_data
from sensor_msgs.msg import Image


def fit_width(width, height, max_width):
    """Output size limited to max_width, keeping the aspect ratio, even dimensions, never up."""
    if max_width <= 0 or width <= max_width:
        w, h = width, height
    else:
        w, h = max_width, round(height * max_width / width)
    return max(2, w - w % 2), max(2, h - h % 2)


class Webcam(Node):
    def __init__(self):
        super().__init__('webcam')
        self.device = self.declare_parameter('device', '/dev/video0').value
        capture_width = self.declare_parameter('capture_width', 640).value
        capture_height = self.declare_parameter('capture_height', 480).value
        fps = self.declare_parameter('fps', 30.0).value
        self.max_width = self.declare_parameter('max_width', 480).value
        self.frame_id = self.declare_parameter('frame_id', 'camera').value
        self.pub = self.create_publisher(Image, 'camera/image_raw', qos_profile_sensor_data)

        self.cap = cv2.VideoCapture(self.device, cv2.CAP_V4L2)
        if not self.cap.isOpened():
            raise RuntimeError(f'cannot open {self.device}')
        self.cap.set(cv2.CAP_PROP_FOURCC, cv2.VideoWriter_fourcc(*'MJPG'))
        self.cap.set(cv2.CAP_PROP_FRAME_WIDTH, capture_width)
        self.cap.set(cv2.CAP_PROP_FRAME_HEIGHT, capture_height)
        self.cap.set(cv2.CAP_PROP_FPS, fps)
        self.cap.set(cv2.CAP_PROP_BUFFERSIZE, 1)
        w = int(self.cap.get(cv2.CAP_PROP_FRAME_WIDTH))
        h = int(self.cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
        fourcc = int(self.cap.get(cv2.CAP_PROP_FOURCC)).to_bytes(4, 'little').decode(errors='replace')
        self.out_size = fit_width(w, h, self.max_width)
        self.get_logger().info(
            f'webcam {self.device}: {w}x{h} {fourcc} @ {self.cap.get(cv2.CAP_PROP_FPS):.0f} fps '
            f'-> {self.out_size[0]}x{self.out_size[1]} on camera/image_raw')

        self.running = True
        self.thread = threading.Thread(target=self.capture_loop, daemon=True)
        self.thread.start()

    def capture_loop(self):
        failures = 0
        while self.running and rclpy.ok():
            ok, frame = self.cap.read()
            stamp = self.get_clock().now().to_msg()
            if not ok:
                failures += 1
                if failures in (1, 30):
                    self.get_logger().warning(f'{self.device}: read failed ({failures} in a row)')
                continue
            failures = 0
            if (frame.shape[1], frame.shape[0]) != self.out_size:
                frame = cv2.resize(frame, self.out_size, interpolation=cv2.INTER_AREA)
            msg = Image()
            msg.header.stamp = stamp
            msg.header.frame_id = self.frame_id
            msg.height, msg.width = frame.shape[:2]
            msg.encoding = 'bgr8'
            msg.step = msg.width * 3
            msg.data = frame.tobytes()
            self.pub.publish(msg)

    def destroy_node(self):
        self.running = False
        self.thread.join(timeout=1.0)
        self.cap.release()
        super().destroy_node()


def main():
    rclpy.init()
    node = Webcam()
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
