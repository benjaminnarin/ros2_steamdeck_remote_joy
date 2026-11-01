"""launch_testing: the real robot.launch.py (deck_router + gateway_node + teleop_twist_joy) driven
by fake_deck over the router's local endpoint. Checks the ROS side: /joy, cmd_vel, the latched
topics, set_stop, /diagnostics, and clean exits.

Self-contained: pairs into a temp dir, moves the router to ports 17450/17451, and runs in its own
ROS domain (set in CMakeLists.txt), so it can run next to a live robot or deck_sim.
"""

import base64
import hashlib
import os
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

import launch
import launch_testing
import launch_testing.actions
import launch_testing.asserts
import pytest
import rclpy
from ament_index_python.packages import get_package_prefix, get_package_share_directory
from diagnostic_msgs.msg import DiagnosticArray
from geometry_msgs.msg import Twist
from launch.actions import IncludeLaunchDescription
from launch.launch_description_sources import PythonLaunchDescriptionSource
from rclpy.qos import DurabilityPolicy, QoSProfile, ReliabilityPolicy
from sensor_msgs.msg import Joy
from std_msgs.msg import Bool
from std_srvs.srv import Trigger

QUIC_PORT, LOCAL_PORT = 17450, 17451
ENDPOINT = f'tcp/127.0.0.1:{LOCAL_PORT}'
BIN = Path(get_package_prefix('deck_gateway')) / 'lib' / 'deck_gateway'
DEADMAN = 21


def make_state():
    tmp = Path(tempfile.mkdtemp(prefix='deck_gateway_launch_test_'))
    subprocess.run([str(BIN / 'pair'), '--dir', str(tmp), '--deck', 'tester', '--host', '127.0.0.1',
                    '--robot-name', 'launch_test', '--out', str(tmp / 'tester.bundle.tar'),
                    '--no-restart'], check=True, capture_output=True)
    router = (tmp / 'router.json5').read_text()
    router = router.replace(':7450?', f':{QUIC_PORT}?').replace(':7451"', f':{LOCAL_PORT}"')
    assert f':{QUIC_PORT}?' in router and f':{LOCAL_PORT}"' in router
    (tmp / 'router.json5').write_text(router)
    (tmp / 'gateway.yaml').write_text(
        'deck_gateway:\n  ros__parameters:\n'
        f'    zenoh_endpoint: {ENDPOINT}\n    deadman: LT\n')
    return tmp


@pytest.mark.launch_test
def generate_test_description():
    tmp = make_state()
    robot_launch = os.path.join(get_package_share_directory('deck_gateway'), 'launch',
                                'robot.launch.py')
    return launch.LaunchDescription([
        IncludeLaunchDescription(
            PythonLaunchDescriptionSource(robot_launch),
            launch_arguments={
                'config_dir': str(tmp),
                'gateway_config': str(tmp / 'gateway.yaml'),
                'cmd_vel_topic': '/test_cmd_vel',
                'cmd_vel_stamped': 'false',
                'use_video': 'false',
            }.items()),
        launch_testing.actions.ReadyToTest(),
    ]), {'state_dir': tmp}


def is_neutral(j):
    return all(a == 0.0 for a in j.axes) and not any(j.buttons)


class TestGateway(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        rclpy.init()
        cls.node = rclpy.create_node('deck_gateway_launch_test')
        cls.lock = threading.Lock()
        cls.joy, cls.cmd, cls.diag = [], [], []
        cls.latched, cls.owner = [], []
        latched = QoSProfile(depth=1, reliability=ReliabilityPolicy.RELIABLE,
                             durability=DurabilityPolicy.TRANSIENT_LOCAL)
        n = cls.node
        n.create_subscription(Joy, '/joy', lambda m: cls._add(cls.joy, m), 100)
        n.create_subscription(Twist, '/test_cmd_vel', lambda m: cls._add(cls.cmd, m), 100)
        n.create_subscription(DiagnosticArray, '/diagnostics', lambda m: cls._add(cls.diag, m), 10)
        n.create_subscription(Bool, '/steamdeck/stop_latched',
                              lambda m: cls._add(cls.latched, m.data), latched)
        n.create_subscription(Bool, '/steamdeck/owner_connected',
                              lambda m: cls._add(cls.owner, m.data), latched)
        cls.set_stop = n.create_client(Trigger, '/steamdeck/set_stop')
        cls.wait_for_gateway()

    @classmethod
    def tearDownClass(cls):
        cls.node.destroy_node()
        rclpy.shutdown()

    @classmethod
    def _add(cls, lst, m):
        with cls.lock:
            lst.append((time.monotonic(), m))

    @classmethod
    def spin(cls, seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            rclpy.spin_once(cls.node, timeout_sec=0.02)

    @classmethod
    def wait_for_gateway(cls):
        end = time.monotonic() + 30
        while time.monotonic() < end:
            cls.spin(0.2)
            out = cls.deck('info', check=False)
            if 'robot_name=launch_test' in out:
                return
        raise AssertionError('gateway did not answer rj/v1/info within 30 s')

    @classmethod
    def deck(cls, *args, check=True, background=False):
        cmd = [str(BIN / 'fake_deck'), '--endpoint', ENDPOINT, *map(str, args)]
        p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        if background:
            return p
        return cls.finish(p, check)

    @classmethod
    def finish(cls, p, check=True):
        while p.poll() is None:
            cls.spin(0.05)
        out = p.stdout.read()
        if check and p.returncode != 0:
            raise AssertionError(f'fake_deck failed ({p.returncode}):\n{out}')
        return out

    def since(self, lst, t0):
        with self.lock:
            return [m for t, m in lst if t >= t0]

    @staticmethod
    def field(out, prefix, key):
        for line in out.splitlines():
            if line.startswith(prefix):
                for part in line.split():
                    if part.startswith(key + '='):
                        return part.split('=', 1)[1]
        return None

    # ── tests (run in name order; each fake_deck run is a new session) ──

    def test_01_startup_is_latched_and_neutral_at_50hz(self):
        t0 = time.monotonic()
        self.spin(2.0)
        joys = self.since(self.joy, t0)
        self.assertGreater(len(joys), 80, 'neutral /joy at 50 Hz')
        self.assertTrue(all(is_neutral(j) for j in joys))
        self.assertTrue(all(len(j.buttons) == 22 and len(j.axes) == 6 for j in joys))
        self.assertTrue(self.latched and self.latched[-1][1], 'stop_latched (transient local)')
        self.assertTrue(self.owner and not self.owner[-1][1], 'owner_connected false')

    def test_02_info_has_robot_name_and_id(self, state_dir):  # state_dir: from generate_test_description
        out = self.deck('info')
        self.assertIn('robot_name=launch_test', out)
        pem = (state_dir / 'tls' / 'ca.pem').read_text()
        der = base64.b64decode(''.join(pem.split('-----')[2].split()))
        self.assertEqual(self.field(out, 'INFO', 'robot_id'), hashlib.sha256(der).hexdigest())

    def test_03_drive_reaches_joy_and_cmd_vel_then_stops(self):
        t0 = time.monotonic()
        out = self.deck('drive', 1.5, 1.0, 0.0)
        self.assertIn('CLEAR ok=1', out)
        self.assertIn('OK', self.field(out, 'SUMMARY', 'states'))
        joys = self.since(self.joy, t0)
        driving = [j for j in joys if j.axes[1] == 1.0 and j.buttons[DEADMAN] == 1]
        self.assertGreater(len(driving), 40, 'mapped input with the deadman (button 21) held')
        self.assertTrue(all(j.axes[4] == -1.0 for j in driving), 'trigger 1 -> joy -1')
        cmds = self.since(self.cmd, t0)
        self.assertTrue(any(c.linear.x > 0.4 for c in cmds), 'teleop_twist_joy drove forward')
        self.spin(0.5)
        self.assertTrue(is_neutral(self.joy[-1][1]))
        self.assertEqual(self.cmd[-1][1].linear.x, 0.0, 'teleop sent a zero twist on release')
        self.assertFalse(self.owner[-1][1], 'token withdrawn when fake_deck quit')

    def test_04_short_gap_rearms(self):
        out = self.deck('gap', 0.5, 500, 0.5)
        states = self.field(out, 'SUMMARY', 'states')
        self.assertIn('NO_CONTROL>REARM>OK', states)

    def test_05_long_gap_latches_link_lost(self):
        t0 = time.monotonic()
        out = self.deck('gap', 0.5, 1500, 0.3)
        self.assertEqual(self.field(out, 'SUMMARY', 'last_reason'), 'link_lost')
        late = [j for t, j in self.joy if t > t0 + 2.2]  # after the gap, during "after"
        self.assertTrue(late and all(not j.buttons[DEADMAN] for j in late), 'stays neutral')

    def test_06_clear_stop_refusals(self):
        self.assertIn('reason=sticks_not_centered', self.deck('clear', 0.5, 0))
        self.assertIn('reason=deadman_held', self.deck('clear', 0.0, 1))
        self.assertIn('CLEAR ok=1', self.deck('clear', 0.05, 0))

    def test_07_second_deck_sees_busy(self):
        a = self.deck('hold', 3, background=True)
        self.spin(1.0)
        out = self.deck('watch', 0.8)
        self.finish(a)
        self.assertIn('state=BUSY', out)

    def test_08_stop_from_another_deck(self):
        a = self.deck('drive', 3, 1.0, 0.0, background=True)
        self.spin(1.5)
        self.assertIn('STOP sent', self.deck('stop'))
        t_stop = time.monotonic()
        out = self.finish(a)
        self.assertEqual(self.field(out, 'SUMMARY', 'last_reason'), 'deck_stop')
        after = [j for t, j in self.joy if t > t_stop + 0.1]
        self.assertTrue(after and all(not j.buttons[DEADMAN] for j in after))

    def test_09_set_stop_service(self):
        self.assertTrue(self.set_stop.wait_for_service(timeout_sec=5.0))
        a = self.deck('drive', 3, 1.0, 0.0, background=True)
        self.spin(1.5)
        fut = self.set_stop.call_async(Trigger.Request())
        while not fut.done():
            self.spin(0.05)
        self.assertTrue(fut.result().success)
        out = self.finish(a)
        self.assertEqual(self.field(out, 'SUMMARY', 'last_reason'), 'service')
        self.spin(0.3)
        self.assertTrue(self.latched[-1][1])

    def test_10_diagnostics(self):
        t0 = time.monotonic()
        self.spin(2.5)
        statuses = [s for m in self.since(self.diag, t0) for s in m.status
                    if s.name == 'deck_gateway: link']
        self.assertTrue(statuses, '/diagnostics has deck_gateway: link')
        keys = {kv.key: kv.value for kv in statuses[-1].values}
        for k in ('state', 'stop_reason', 'control_rate_hz', 'watchdog_trips', 'rearms',
                  'link_lost', 'owner_changes', 'stops_latched', 'bad_messages'):
            self.assertIn(k, keys)
        self.assertGreaterEqual(int(keys['rearms']), 1)
        self.assertGreaterEqual(int(keys['link_lost']), 1)


@launch_testing.post_shutdown_test()
class TestShutdown(unittest.TestCase):
    def test_exit_codes(self, proc_info):
        # SIGINT on shutdown: ROS nodes exit 0; deck_router exits 0 via its signal handler.
        launch_testing.asserts.assertExitCodes(proc_info, allowable_exit_codes=[0, -2, -15])
