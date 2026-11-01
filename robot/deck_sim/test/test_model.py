import math

import pytest

from deck_sim import model


def test_battery_drains_faster_while_moving():
    idle, moving = model.Battery(pct=50.0), model.Battery(pct=50.0)
    idle.step(60.0, moving=False, docked=False)
    moving.step(60.0, moving=True, docked=False)
    assert idle.pct == pytest.approx(49.0)
    assert moving.pct == pytest.approx(46.0)


def test_battery_charges_and_clamps():
    b = model.Battery(pct=95.0)
    b.step(60.0, moving=False, docked=True)
    assert b.pct == 100.0
    b = model.Battery(pct=0.5, idle_drain_pct_per_min=60.0)
    b.step(60.0, moving=False, docked=False)
    assert b.pct == 0.0


def test_battery_voltage_range():
    assert model.Battery(pct=0.0).voltage == pytest.approx(model.V_EMPTY)
    assert model.Battery(pct=100.0).voltage == pytest.approx(model.V_FULL)


@pytest.mark.parametrize('pct,level', [(80, model.OK), (25, model.OK), (24.9, model.WARN),
                                       (10, model.WARN), (9.9, model.ERROR)])
def test_battery_level(pct, level):
    assert model.battery_level(pct) == level


def test_integrate_straight():
    p = model.integrate(model.Pose2D(0, 0, math.pi / 2), v=1.0, w=0.0, dt_s=2.0)
    assert (p.x, p.y) == pytest.approx((0.0, 2.0), abs=1e-9)


def test_integrate_full_circle_returns_home():
    p = model.Pose2D()
    for _ in range(1000):
        p = model.integrate(p, v=1.0, w=2 * math.pi / 10.0, dt_s=0.01)  # one turn in 10 s
    assert (p.x, p.y) == pytest.approx((0.0, 0.0), abs=1e-6)


def test_rising_edges():
    assert model.rising_edges([], [1, 0, 1]) == [0, 2]
    assert model.rising_edges([1, 0, 0], [1, 1, 0]) == [1]
    assert model.rising_edges([1, 1], [0, 0]) == []
