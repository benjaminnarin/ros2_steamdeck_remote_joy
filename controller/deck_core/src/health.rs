//! RobotState stream -> link state, RTT, clock offset, ownership. Pure: time is passed in.
//! Spec: docs/controller.md § Screens (overlay states), docs/interface.md § Clock offset.

use std::collections::VecDeque;

use deck_msgs::v1::robot_state::State;
use deck_msgs::v1::RobotState;

/// While we own the robot, RobotState arrives with every Control (50 Hz): a 300 ms silence is a
/// lost link. Otherwise it's only the 5 Hz heartbeat, so allow longer before saying LINK LOST.
pub const LINK_TIMEOUT_OWNER_US: u64 = 300_000;
pub const LINK_TIMEOUT_OTHER_US: u64 = 600_000;
const RTT_WINDOW_US: u64 = 5_000_000;

#[derive(Clone, Debug, PartialEq)]
pub enum Robot {
    /// The gateway's state as it applies to this Deck.
    Ok,
    Stopped(String),
    NoControl,
    Rearm,
    /// Another session owns the robot.
    Busy(u32),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rtt {
    pub last_ms: f64,
    pub min_ms: f64,
    pub avg_ms: f64,
    pub samples: usize,
}

#[derive(Debug)]
pub struct Health {
    session_id: u32,
    last_rx_us: Option<u64>,
    last: Option<RobotState>,
    last_echo_seq: u32,
    /// (received_us, rtt_us, offset_us)
    rtt: VecDeque<(u64, u64, i64)>,
}

impl Health {
    pub fn new(session_id: u32) -> Self {
        Health { session_id, last_rx_us: None, last: None, last_echo_seq: 0, rtt: VecDeque::new() }
    }

    pub fn on_robot_state(&mut self, m: RobotState, now_us: u64) {
        let mine = m.owner_session == self.session_id;
        if mine && m.echo_deck_time_us != 0 && m.echo_seq != self.last_echo_seq {
            self.last_echo_seq = m.echo_seq;
            let rtt = now_us.saturating_sub(m.echo_deck_time_us);
            // robot clock - deck clock, assuming a symmetric path
            let offset = m.robot_time_us as i64 - (m.echo_deck_time_us + rtt / 2) as i64;
            self.rtt.push_back((now_us, rtt, offset));
        }
        while self.rtt.front().is_some_and(|s| s.0 + RTT_WINDOW_US < now_us) {
            self.rtt.pop_front();
        }
        self.last_rx_us = Some(now_us);
        self.last = Some(m);
    }

    pub fn owner(&self) -> bool {
        self.last.as_ref().is_some_and(|m| m.owner_session == self.session_id)
    }

    /// Has RobotState arrived recently enough to call the link up?
    pub fn link_up(&self, now_us: u64) -> bool {
        let timeout = if self.owner() { LINK_TIMEOUT_OWNER_US } else { LINK_TIMEOUT_OTHER_US };
        self.last_rx_us.is_some_and(|t| now_us.saturating_sub(t) <= timeout)
    }

    pub fn ever_received(&self) -> bool {
        self.last_rx_us.is_some()
    }

    pub fn robot(&self) -> Option<Robot> {
        let m = self.last.as_ref()?;
        if m.owner_session != 0 && m.owner_session != self.session_id {
            return Some(Robot::Busy(m.owner_session));
        }
        Some(match m.state() {
            State::Ok => Robot::Ok,
            State::Stopped => Robot::Stopped(m.stop_reason.clone()),
            State::Rearm => Robot::Rearm,
            State::Busy => Robot::Busy(m.owner_session),
            State::NoControl | State::Unspecified => Robot::NoControl,
        })
    }

    pub fn rtt(&self) -> Option<Rtt> {
        let last = self.rtt.back()?;
        let ms = |us: u64| us as f64 / 1000.0;
        let min = self.rtt.iter().map(|s| s.1).min().unwrap_or(0);
        let avg = self.rtt.iter().map(|s| s.1).sum::<u64>() as f64 / self.rtt.len() as f64;
        Some(Rtt { last_ms: ms(last.1), min_ms: ms(min), avg_ms: avg / 1000.0, samples: self.rtt.len() })
    }

    /// robot_clock - deck_clock from the lowest-RTT sample in the window (for video latency).
    pub fn clock_offset_us(&self) -> Option<i64> {
        self.rtt.iter().min_by_key(|s| s.1).map(|s| s.2)
    }
}

/// What the Drive screen shows, in priority order (docs/controller.md § Screens).
#[derive(Clone, Debug, PartialEq)]
pub enum Overlay {
    NoController,
    Connecting,
    WrongRobot(String),
    Version(u32),
    LinkLost,
    Busy(u32),
    /// `local`: the Deck's own latch (e.g. B just pressed, not yet confirmed by the robot).
    Stopped { reason: String, local: bool },
    Rearm,
    NoControl,
    Ok,
}

pub struct OverlayInputs<'a> {
    pub has_pad: bool,
    pub session_up: bool,
    pub fatal: Option<&'a Overlay>,
    pub local_latch: bool,
    pub now_us: u64,
}

pub fn overlay(h: &Health, i: OverlayInputs) -> Overlay {
    if let Some(f) = i.fatal {
        return f.clone();
    }
    if !i.has_pad {
        return Overlay::NoController;
    }
    if !i.session_up || !h.ever_received() {
        return Overlay::Connecting;
    }
    if !h.link_up(i.now_us) {
        return Overlay::LinkLost;
    }
    match h.robot() {
        Some(Robot::Busy(o)) => Overlay::Busy(o),
        Some(Robot::Stopped(r)) => Overlay::Stopped { reason: r, local: false },
        _ if i.local_latch => Overlay::Stopped { reason: "local".into(), local: true },
        Some(Robot::Rearm) => Overlay::Rearm,
        Some(Robot::Ok) => Overlay::Ok,
        Some(Robot::NoControl) | None => Overlay::NoControl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 0x1234;
    const MS: u64 = 1000;

    fn rs(owner: u32, state: State, echo_seq: u32, echo_deck_us: u64, robot_us: u64) -> RobotState {
        RobotState {
            robot_time_us: robot_us,
            state: state as i32,
            owner_session: owner,
            echo_seq,
            echo_deck_time_us: echo_deck_us,
            stop_reason: if state == State::Stopped { "new_owner".into() } else { String::new() },
        }
    }

    fn inputs(now: u64) -> OverlayInputs<'static> {
        OverlayInputs { has_pad: true, session_up: true, fatal: None, local_latch: false, now_us: now }
    }

    #[test]
    fn rtt_and_offset_from_our_echoes_only() {
        let mut h = Health::new(ME);
        // Deck sent at 1000 ms; robot clock = deck + 50 s; reply arrives 6 ms later.
        h.on_robot_state(rs(ME, State::Ok, 1, 1_000 * MS, 51_003 * MS), 1_006 * MS);
        h.on_robot_state(rs(ME, State::Ok, 1, 1_000 * MS, 51_100 * MS), 1_100 * MS); // heartbeat, same echo
        h.on_robot_state(rs(ME, State::Ok, 2, 1_200 * MS, 51_202 * MS), 1_204 * MS);
        let r = h.rtt().unwrap();
        assert_eq!(r.samples, 2, "repeated echo_seq isn't a new sample");
        assert_eq!(r.min_ms, 4.0);
        assert_eq!(r.last_ms, 4.0);
        assert_eq!(r.avg_ms, 5.0);
        assert_eq!(h.clock_offset_us(), Some(50_000 * MS as i64));
        h.on_robot_state(rs(0x9999, State::Ok, 77, 5, 0), 1_300 * MS);
        assert_eq!(h.rtt().unwrap().samples, 2, "another owner's echo isn't ours");
    }

    #[test]
    fn link_lost_timeouts_depend_on_ownership() {
        let mut h = Health::new(ME);
        h.on_robot_state(rs(ME, State::Ok, 1, 1, 0), 1_000 * MS);
        assert!(h.link_up(1_300 * MS));
        assert!(!h.link_up(1_301 * MS));
        h.on_robot_state(rs(0, State::NoControl, 0, 0, 0), 2_000 * MS);
        assert!(h.link_up(2_600 * MS), "heartbeat only: 5 Hz");
        assert!(!h.link_up(2_601 * MS));
    }

    #[test]
    fn overlay_priorities() {
        let mut h = Health::new(ME);
        let now = 1_000 * MS;
        assert_eq!(overlay(&h, OverlayInputs { has_pad: false, ..inputs(now) }), Overlay::NoController);
        assert_eq!(overlay(&h, inputs(now)), Overlay::Connecting);
        h.on_robot_state(rs(ME, State::Stopped, 1, 1, 0), now);
        assert_eq!(overlay(&h, inputs(now)), Overlay::Stopped { reason: "new_owner".into(), local: false });
        h.on_robot_state(rs(ME, State::Ok, 2, 1, 0), now);
        assert_eq!(overlay(&h, inputs(now)), Overlay::Ok);
        assert_eq!(
            overlay(&h, OverlayInputs { local_latch: true, ..inputs(now) }),
            Overlay::Stopped { reason: "local".into(), local: true },
            "B shows STOPPED immediately, before the robot confirms"
        );
        h.on_robot_state(rs(ME, State::Rearm, 3, 1, 0), now);
        assert_eq!(overlay(&h, inputs(now)), Overlay::Rearm);
        h.on_robot_state(rs(0xbeef, State::Ok, 9, 1, 0), now);
        assert_eq!(overlay(&h, inputs(now)), Overlay::Busy(0xbeef), "derived: another owner");
        assert_eq!(overlay(&h, inputs(now + 700 * MS)), Overlay::LinkLost);
        let wrong = Overlay::WrongRobot("other".into());
        assert_eq!(overlay(&h, OverlayInputs { fatal: Some(&wrong), ..inputs(now) }), wrong);
    }
}
