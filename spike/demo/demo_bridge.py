#!/usr/bin/env python3
"""End-to-end demo bridge: a throwaway stand-in for deck_gateway + deck_video.

Runs the Zenoh router the Deck connects to (QUIC, mTLS, mixed_rel=1), and:
  * Control  -> sensor_msgs/Joy on /joy (joy-package conventions), echo back
  * watchdog: no control for 250 ms -> neutral Joy at 50 Hz
  * webcam   -> JPEG frames on rj/spike/video
Speaks the spike's fixed-layout messages (spike/common/src/lib.rs), not Protobuf.
"""

import argparse
import struct
import threading
import time
from pathlib import Path

import cv2
import rclpy
import zenoh
from rclpy.node import Node
from sensor_msgs.msg import Joy

KEY_CONTROL = "rj/spike/control"
KEY_ECHO = "rj/spike/echo"
KEY_VIDEO = "rj/spike/video"
CONTROL = struct.Struct("<IQ6fI")  # seq, deck_us, axes[6], buttons
ECHO = struct.Struct("<QIQ")  # robot_us, seq, deck_us
FRAME = struct.Struct("<IQIHH")  # frame_id, capture_us, encode_us, w, h
WATCHDOG_S = 0.25
NUM_BUTTONS = 21


def now_us() -> int:
    return time.monotonic_ns() // 1000


def zenoh_config(args) -> zenoh.Config:
    c = zenoh.Config()
    certs = Path(args.certs).resolve()
    for key, value in [
        ("mode", '"router"'),
        ("listen/endpoints", f'["{args.listen}"]'),
        ("scouting/multicast/enabled", "false"),
        ("transport/link/tx/lease", "2000"),
        ("transport/link/tx/keep_alive", "4"),
        ("transport/link/tls/root_ca_certificate", f'"{certs / "ca.pem"}"'),
        ("transport/link/tls/listen_certificate", f'"{certs / "robot.pem"}"'),
        ("transport/link/tls/listen_private_key", f'"{certs / "robot.key"}"'),
        ("transport/link/tls/enable_mtls", "true"),
    ]:
        c.insert_json5(key, value)
    return c


def to_joy_axes(axes):
    """Spike Control carries raw gilrs values: right = +X, up = +Y, triggers 0..1.
    joy's game_controller_node: left = +X, up = +Y, triggers 0 (released) .. -1."""
    lx, ly, rx, ry, lt, rt = axes
    return [-lx, ly, -rx, ry, -lt, -rt]


class DemoBridge(Node):
    def __init__(self, args):
        super().__init__("deck_demo_bridge")
        self.joy_pub = self.create_publisher(Joy, "joy", 10)
        self.lock = threading.Lock()
        self.last_control = 0.0
        self.stale = True
        self.controls = 0

        self.session = zenoh.open(zenoh_config(args))
        self.echo_pub = self.session.declare_publisher(
            KEY_ECHO,
            congestion_control=zenoh.CongestionControl.DROP,
            priority=zenoh.Priority.REAL_TIME,
            express=True,
            reliability=zenoh.Reliability.BEST_EFFORT,
        )
        self.video_pub = self.session.declare_publisher(
            KEY_VIDEO,
            congestion_control=zenoh.CongestionControl.DROP,
            priority=zenoh.Priority.INTERACTIVE_LOW,
            reliability=zenoh.Reliability.BEST_EFFORT,
        )
        self.control_sub = self.session.declare_subscriber(KEY_CONTROL, self.on_control)
        self.create_timer(1.0 / 50, self.watchdog)
        self.create_timer(1.0, self.report)
        self.get_logger().info(f"listening on {args.listen}; /joy ready")

        if args.camera >= 0:
            threading.Thread(target=self.camera_loop, args=(args,), daemon=True).start()

    # Zenoh callback thread
    def on_control(self, sample):
        data = sample.payload.to_bytes()
        if len(data) != CONTROL.size:
            return
        seq, deck_us, *rest = CONTROL.unpack(data)
        axes, buttons = rest[:6], rest[6]
        self.echo_pub.put(ECHO.pack(now_us(), seq, deck_us))
        msg = Joy()
        msg.header.stamp = self.get_clock().now().to_msg()
        msg.header.frame_id = "steamdeck"
        msg.axes = to_joy_axes(axes)
        msg.buttons = [(buttons >> i) & 1 for i in range(NUM_BUTTONS)]
        self.joy_pub.publish(msg)
        with self.lock:
            if self.stale:
                self.get_logger().info("control link up")
            self.last_control = time.monotonic()
            self.stale = False
            self.controls += 1

    def watchdog(self):
        with self.lock:
            if time.monotonic() - self.last_control < WATCHDOG_S:
                return
            if not self.stale:
                self.get_logger().warning("no control for 250 ms -> neutral /joy")
            self.stale = True
        msg = Joy()
        msg.header.stamp = self.get_clock().now().to_msg()
        msg.header.frame_id = "steamdeck"
        msg.axes = [0.0] * 6
        msg.buttons = [0] * NUM_BUTTONS
        self.joy_pub.publish(msg)

    def report(self):
        with self.lock:
            n, self.controls = self.controls, 0
        if n:
            self.get_logger().info(f"control {n}/s")

    def camera_loop(self, args):
        cap = cv2.VideoCapture(args.camera, cv2.CAP_V4L2)
        # MJPEG lets typical USB webcams deliver 30 fps; keep only the newest frame.
        cap.set(cv2.CAP_PROP_FOURCC, cv2.VideoWriter_fourcc(*"MJPG"))
        cap.set(cv2.CAP_PROP_FRAME_WIDTH, 640)
        cap.set(cv2.CAP_PROP_FRAME_HEIGHT, 480)
        cap.set(cv2.CAP_PROP_BUFFERSIZE, 1)
        cap.set(cv2.CAP_PROP_FPS, args.fps)
        if not cap.isOpened():
            self.get_logger().error(f"cannot open camera {args.camera}")
            return
        params = [cv2.IMWRITE_JPEG_QUALITY, args.quality]
        frame_id = 0
        while rclpy.ok():
            ok, frame = cap.read()
            capture_us = now_us()
            if not ok:
                time.sleep(0.05)
                continue
            h, w = frame.shape[:2]
            if w > args.width:
                frame = cv2.resize(frame, (args.width, int(h * args.width / w)), interpolation=cv2.INTER_AREA)
                h, w = frame.shape[:2]
            ok, jpeg = cv2.imencode(".jpg", frame, params)
            if not ok:
                continue
            frame_id += 1
            header = FRAME.pack(frame_id, capture_us, now_us() - capture_us, w, h)
            try:
                self.video_pub.put(header + jpeg.tobytes())
            except zenoh.ZError:
                return  # session closed on shutdown


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--listen", default="quic/0.0.0.0:7450?mixed_rel=1")
    p.add_argument("--certs", default=str(Path(__file__).resolve().parent.parent / "certs"))
    p.add_argument("--camera", type=int, default=0, help="V4L2 index; -1 disables video")
    p.add_argument("--width", type=int, default=480)
    p.add_argument("--fps", type=int, default=30)
    p.add_argument("--quality", type=int, default=70)
    args, ros_args = p.parse_known_args()
    rclpy.init(args=ros_args)
    node = DemoBridge(args)
    try:
        rclpy.spin(node)
    except KeyboardInterrupt:
        pass
    except Exception:
        if rclpy.ok():
            raise  # a real error; after Ctrl+C/SIGTERM rclpy has already shut down
    finally:
        node.session.close()
        rclpy.try_shutdown()


if __name__ == "__main__":
    main()
