"""ROS-free models behind fake_robot, kept separate so they can be unit tested."""

import math
from dataclasses import dataclass

# Battery voltage for a 3S Li-ion pack, linear in charge (good enough for a fake).
V_EMPTY = 10.5
V_FULL = 12.6


@dataclass
class Battery:
    """Charge in percent (0..100). Drains while idle, faster while moving, charges when docked."""

    pct: float = 100.0
    idle_drain_pct_per_min: float = 1.0
    moving_drain_pct_per_min: float = 4.0
    charge_pct_per_min: float = 20.0

    def step(self, dt_s: float, moving: bool, docked: bool) -> None:
        if docked:
            rate = self.charge_pct_per_min
        elif moving:
            rate = -self.moving_drain_pct_per_min
        else:
            rate = -self.idle_drain_pct_per_min
        self.pct = min(100.0, max(0.0, self.pct + rate * dt_s / 60.0))

    @property
    def voltage(self) -> float:
        return V_EMPTY + (V_FULL - V_EMPTY) * self.pct / 100.0

    def current(self, moving: bool, docked: bool) -> float:
        """Amps, positive = charging (sensor_msgs/BatteryState convention)."""
        if docked:
            return 2.0
        return -3.5 if moving else -0.8


# diagnostic_msgs/DiagnosticStatus levels
OK, WARN, ERROR = 0, 1, 2


def battery_level(pct: float, warn_below: float = 25.0, error_below: float = 10.0) -> int:
    if pct < error_below:
        return ERROR
    if pct < warn_below:
        return WARN
    return OK


@dataclass
class Pose2D:
    x: float = 0.0
    y: float = 0.0
    theta: float = 0.0


def integrate(pose: Pose2D, v: float, w: float, dt_s: float) -> Pose2D:
    """Unicycle model, exact for constant v and w over dt."""
    if abs(w) < 1e-9:
        return Pose2D(
            pose.x + v * dt_s * math.cos(pose.theta),
            pose.y + v * dt_s * math.sin(pose.theta),
            pose.theta,
        )
    th1 = pose.theta + w * dt_s
    r = v / w
    return Pose2D(
        pose.x + r * (math.sin(th1) - math.sin(pose.theta)),
        pose.y - r * (math.cos(th1) - math.cos(pose.theta)),
        math.atan2(math.sin(th1), math.cos(th1)),
    )


def rising_edges(prev: list, now: list) -> list:
    """Indices of buttons that went 0 -> 1."""
    return [i for i, b in enumerate(now) if b and (i >= len(prev) or not prev[i])]
