//! `--v1`: speak the product interface (Protobuf on rj/v1/*, controller/deck_msgs) so the spike
//! app can drive the real deck_gateway. A stand-in until the real Deck app (deck_core) exists:
//! - Control with X axes negated (wire: +1 = left), the local stop latch in `stop`
//! - B: latch locally and send Stop (reliable)
//! - View + Menu held 1 s with sticks centered: clear_stop with released_seq (docs/interface.md § Rules)
//! - RobotState: shown on screen; the owner's echo feeds the RTT stats
//! - Quit: Stop, then the session (and liveliness token) goes away

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use deck_msgs::v1::{self, robot_state};
use deck_msgs::{keys, Message};
use spike_common::Echo;
use zenoh::Wait;

const BIT_B: u32 = 1 << 1;
const BIT_VIEW: u32 = 1 << 4;
const BIT_MENU: u32 = 1 << 6;
const CLEAR_HOLD: Duration = Duration::from_secs(1);

/// What the screen shows about the gateway.
#[derive(Default, Clone)]
pub struct GwView {
    pub state: String,
    pub stop_reason: String,
    pub owner: u32,
    pub last_clear: String,
    /// 0..1 while View + Menu are held.
    pub combo_progress: f32,
}

pub struct V1 {
    pub session_id: u32,
    pub local_latch: AtomicBool,
    released_seq: AtomicU32,
    pub view: Mutex<GwView>,
    last_echo_seq: AtomicU32,
    session: zenoh::Session,
    stop_pub: zenoh::pubsub::Publisher<'static>,
    // control-loop-only state
    loop_state: Mutex<LoopState>,
}

#[derive(Default)]
struct LoopState {
    prev_buttons: u32,
    combo_since: Option<Instant>,
    combo_fired: bool,
}

pub fn state_name(s: robot_state::State) -> &'static str {
    match s {
        robot_state::State::Ok => "OK",
        robot_state::State::Stopped => "STOPPED",
        robot_state::State::NoControl => "NO_CONTROL",
        robot_state::State::Busy => "BUSY",
        robot_state::State::Rearm => "REARM",
        robot_state::State::Unspecified => "?",
    }
}

impl V1 {
    pub fn new(session: zenoh::Session, session_id: u32) -> anyhow::Result<Arc<Self>> {
        let stop_pub = session
            .declare_publisher(keys::STOP)
            .congestion_control(zenoh::qos::CongestionControl::Block)
            .priority(zenoh::qos::Priority::RealTime)
            .express(true)
            .reliability(zenoh::qos::Reliability::Reliable)
            .encoding(zenoh::bytes::Encoding::from(keys::encoding("Stop")))
            .wait()
            .map_err(|e| anyhow::anyhow!("stop publisher: {e}"))?;
        Ok(Arc::new(V1 {
            session_id,
            local_latch: AtomicBool::new(false),
            released_seq: AtomicU32::new(0),
            view: Mutex::new(GwView { state: "waiting".into(), ..Default::default() }),
            last_echo_seq: AtomicU32::new(0),
            session,
            stop_pub,
            loop_state: Mutex::new(LoopState::default()),
        }))
    }

    /// Encodes one Control. `axes` are raw gilrs values (right = +X, up = +Y, triggers 0..1).
    pub fn encode_control(&self, seq: u32, deck_us: u64, axes: [f32; 6], buttons: u32) -> Vec<u8> {
        let [lx, ly, rx, ry, lt, rt] = axes;
        v1::Control {
            session_id: self.session_id,
            seq,
            deck_time_us: deck_us,
            axes: vec![-lx, ly, -rx, ry, lt, rt],
            buttons,
            stop: self.local_latch.load(Ordering::SeqCst),
        }
        .encode_to_vec()
    }

    /// Called by the control loop for each input sample, before sending `next_seq`.
    pub fn on_input(self: &Arc<Self>, buttons: u32, axes: [f32; 6], next_seq: u32, deck_us: u64) -> Vec<String> {
        let mut events = Vec::new();
        let mut ls = self.loop_state.lock().unwrap();
        let pressed = buttons & !ls.prev_buttons;
        ls.prev_buttons = buttons;

        if pressed & BIT_B != 0 {
            self.local_latch.store(true, Ordering::SeqCst);
            self.send_stop("button_b", deck_us);
            events.push("STOP (B): latched locally, Stop sent".into());
        }

        let combo = buttons & (BIT_VIEW | BIT_MENU) == (BIT_VIEW | BIT_MENU);
        let centered = axes[..4].iter().all(|v| v.abs() <= 0.1);
        if combo && centered {
            let since = *ls.combo_since.get_or_insert_with(Instant::now);
            let held = since.elapsed();
            self.view.lock().unwrap().combo_progress = (held.as_secs_f32() / CLEAR_HOLD.as_secs_f32()).min(1.0);
            if held >= CLEAR_HOLD && !ls.combo_fired {
                ls.combo_fired = true;
                // 1. stop setting Control.stop; 2. clear_stop with the first seq sent with stop=false
                self.local_latch.store(false, Ordering::SeqCst);
                self.released_seq.store(next_seq, Ordering::SeqCst);
                let me = self.clone();
                std::thread::spawn(move || me.clear_stop(next_seq));
                events.push(format!("clear_stop requested (released_seq {next_seq})"));
            }
        } else {
            ls.combo_since = None;
            ls.combo_fired = false;
            self.view.lock().unwrap().combo_progress = 0.0;
        }
        events
    }

    fn clear_stop(&self, released_seq: u32) {
        std::thread::sleep(Duration::from_millis(30)); // let the stop=false control go first
        let req = v1::ClearStopRequest { session_id: self.session_id, released_seq };
        let result = self
            .session
            .get(keys::CLEAR_STOP)
            .payload(req.encode_to_vec())
            .encoding(zenoh::bytes::Encoding::from(keys::encoding("ClearStopRequest")))
            .timeout(Duration::from_secs(2))
            .wait();
        let mut msg = "timeout".to_string();
        let mut ok = false;
        if let Ok(replies) = result {
            while let Ok(reply) = replies.recv() {
                if let Ok(sample) = reply.result() {
                    if let Ok(r) = v1::ClearStopReply::decode(&*sample.payload().to_bytes()) {
                        ok = r.ok;
                        msg = if r.ok { "cleared".into() } else { r.reason };
                    }
                }
            }
        }
        if !ok {
            self.local_latch.store(true, Ordering::SeqCst); // 4. refused: latch again
        }
        println!("EVENT clear_stop: {msg}");
        self.view.lock().unwrap().last_clear = msg;
    }

    pub fn send_stop(&self, source: &str, deck_us: u64) {
        let m = v1::Stop { session_id: self.session_id, deck_time_us: deck_us, source: source.into() };
        let _ = self.stop_pub.put(m.encode_to_vec()).wait();
    }

    /// RobotState sample -> screen state, plus an Echo for the RTT stats when it's ours and new.
    pub fn on_robot_state(&self, bytes: &[u8]) -> Option<Echo> {
        let m = v1::RobotState::decode(bytes).ok()?;
        let mine = m.owner_session == self.session_id;
        let busy = m.owner_session != 0 && !mine;
        {
            let mut v = self.view.lock().unwrap();
            v.state = if busy { "BUSY".into() } else { state_name(m.state()).into() };
            v.stop_reason = m.stop_reason.clone();
            v.owner = m.owner_session;
        }
        if mine && m.echo_deck_time_us != 0 && self.last_echo_seq.swap(m.echo_seq, Ordering::SeqCst) != m.echo_seq {
            return Some(Echo { robot_us: m.robot_time_us, seq: m.echo_seq, deck_us: m.echo_deck_time_us });
        }
        None
    }

    /// Big status line for the screen and the log.
    pub fn status_line(&self) -> (String, [u8; 3]) {
        let v = self.view.lock().unwrap().clone();
        let local = if self.local_latch.load(Ordering::SeqCst) { "  (local STOP)" } else { "" };
        let (text, color) = match v.state.as_str() {
            "OK" => ("OK: hold L2 to drive (RB turbo)".to_string(), [60, 200, 60]),
            "STOPPED" => (format!("STOPPED: {}  (hold View+Menu 1 s to clear)", v.stop_reason), [230, 50, 50]),
            "REARM" => ("REARM: release L2, then press it again".to_string(), [240, 160, 0]),
            "NO_CONTROL" => ("NO CONTROL".to_string(), [150, 150, 150]),
            "BUSY" => (format!("BUSY: robot owned by {:08x}", v.owner), [240, 160, 0]),
            other => (other.to_string(), [150, 150, 150]),
        };
        let clear = if v.combo_progress > 0.0 && v.combo_progress < 1.0 {
            format!("  clearing {:.0}%", v.combo_progress * 100.0)
        } else if !v.last_clear.is_empty() {
            format!("  last clear: {}", v.last_clear)
        } else {
            String::new()
        };
        (format!("{text}{local}{clear}"), color)
    }
}
