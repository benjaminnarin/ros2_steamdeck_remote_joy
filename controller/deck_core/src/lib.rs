//! deck_core: everything on the Deck that matters for safety and networking, with no UI.
//! The UI (deck_app) reads `DeckCore::snapshot()` and sets a few flags (touch STOP, panel open).
//! Spec: docs/controller.md.

pub mod events;
pub mod health;
pub mod input;
pub mod profiles;
pub mod video;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use deck_msgs::v1::{self, ButtonHint, CameraInfo, ClearStopReply, ClearStopRequest, Control, Info, RobotState, Stop, VideoFrame};
use deck_msgs::{keys, Message};
use serde_json::json;
use zenoh::qos::{CongestionControl, Priority, Reliability};
use zenoh::Wait;

use events::EventLog;
use health::{Health, Overlay, OverlayInputs};
use input::{Action, InputConfig, InputState, RawPad, StopLatch};
use profiles::Installed;

pub const SUPPORTED_INTERFACE: u32 = deck_msgs::INTERFACE_VERSION;
const CONTROL_PERIOD: Duration = Duration::from_millis(20);
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

static START: OnceLock<Instant> = OnceLock::new();

/// The Deck clock: monotonic microseconds since the process started. Used for
/// Control.deck_time_us, RTT, and video timing, so they all line up.
pub fn mono_us() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_micros() as u64 + 1 // never 0: 0 = "no echo"
}

#[derive(Clone)]
pub struct Clock;
impl Clock {
    pub fn new() -> Self {
        let _ = mono_us();
        Clock
    }
    pub fn now_us(&self) -> u64 {
        mono_us()
    }
}
impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

/// What the UI draws. Cloned out under a short lock.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub overlay: Option<Overlay>,
    pub robot_name: String,
    pub session_id: u32,
    pub rtt: Option<health::Rtt>,
    pub has_pad: bool,
    pub pad_name: String,
    /// Wire axes and buttons last sent (or that would be sent).
    pub axes: [f32; 6],
    pub buttons: u32,
    pub deadman: bool,
    pub turbo: bool,
    pub local_latch: bool,
    pub clear_progress: f32,
    pub clear_blocked: Option<&'static str>,
    pub panel_open: bool,
    pub button_hints: Vec<ButtonHint>,
    /// Short-lived messages for the UI (e.g. "clear refused: sticks not centered").
    pub notices: Vec<(u64, String)>,
    pub controls_sent: u64,
    pub camera: Option<CameraInfo>,
    pub video_live: bool,
    pub video_ever: bool,
    pub video: video::Stats,
    pub clock_offset_us: Option<i64>,
}

struct Links {
    session: zenoh::Session,
    control: zenoh::pubsub::Publisher<'static>,
    stop: zenoh::pubsub::Publisher<'static>,
    _token: zenoh::liveliness::LivelinessToken,
    _state_sub: zenoh::pubsub::Subscriber<()>,
    _video_sub: zenoh::pubsub::Subscriber<()>,
}

#[derive(Default)]
struct State {
    health: Option<Health>,
    session_up: bool,
    fatal: Option<Overlay>,
    info_ok: bool,
    robot_name: String,
    button_hints: Vec<ButtonHint>,
    camera: Option<CameraInfo>,
    latch: StopLatch,
    pad: Option<String>,
    last_frame: input::Frame,
    notices: Vec<(u64, String)>,
    controls_sent: u64,
}

pub struct DeckCore {
    profile: Installed,
    session_id: u32,
    clock: Clock,
    log: Arc<EventLog>,
    state: Mutex<State>,
    links: OnceLock<Links>,
    video: Arc<video::Video>,
    touch_stop: AtomicBool,
    panel_open: AtomicBool,
    quit: AtomicBool,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

/// Something that yields gamepad snapshots: gilrs on the Deck, a script in tests. Built on the
/// control thread by a `PadFactory` (gilrs isn't Send).
pub trait PadSource {
    /// None = no gamepad.
    fn poll(&mut self) -> Option<(RawPad, String)>;
    /// True if input changed since the last poll (sends a Control right away).
    fn changed(&mut self) -> bool {
        false
    }
}

pub type PadFactory = Box<dyn FnOnce() -> Box<dyn PadSource> + Send>;

pub fn session_id(clock: &Clock) -> u32 {
    let seed = clock.now_us()
        ^ std::process::id() as u64
        ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let mixed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    ((mixed >> 32) as u32) | 1
}

pub fn zenoh_config(profile: &Installed) -> Result<zenoh::Config> {
    let mut c = zenoh::Config::default();
    let mut set = |k: &str, v: String| c.insert_json5(k, &v).map_err(|e| anyhow!("config {k}={v}: {e}"));
    for ep in &profile.profile.endpoints {
        if ep.starts_with("quic/") && !ep.contains("mixed_rel=1") {
            return Err(anyhow!("{ep}: QUIC endpoints need ?mixed_rel=1 (docs/interface.md § Connection)"));
        }
    }
    set("mode", "\"client\"".into())?;
    set("connect/endpoints", serde_json::to_string(&profile.profile.endpoints)?)?;
    set("connect/retry", "{period_init_ms: 200, period_max_ms: 1000, period_increase_factor: 1.5}".into())?;
    set("connect/exit_on_failure", "false".into())?;
    set("connect/timeout_ms", "-1".into())?;
    set("scouting/multicast/enabled", "false".into())?;
    set("transport/link/tx/lease", "2000".into())?;
    set("transport/link/tx/keep_alive", "4".into())?;
    let p = |p: std::path::PathBuf| serde_json::to_string(&p.display().to_string()).unwrap();
    set("transport/link/tls/root_ca_certificate", p(profile.ca()))?;
    set("transport/link/tls/enable_mtls", "true".into())?;
    set("transport/link/tls/connect_certificate", p(profile.cert()))?;
    set("transport/link/tls/connect_private_key", p(profile.key()))?;
    Ok(c)
}

impl DeckCore {
    pub fn start(profile: Installed, log: Arc<EventLog>, pad: PadFactory) -> Arc<Self> {
        let clock = Clock::new();
        let session_id = session_id(&clock);
        log.set_context("robot_id", json!(profile.profile.id[..16]));
        log.set_context("session", json!(format!("{session_id:08x}")));
        let core = Arc::new(DeckCore {
            session_id,
            clock,
            log,
            state: Mutex::new(State { health: Some(Health::new(session_id)), robot_name: profile.profile.name.clone(), ..Default::default() }),
            profile,
            links: OnceLock::new(),
            video: video::Video::new(),
            touch_stop: AtomicBool::new(false),
            panel_open: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            threads: Mutex::new(Vec::new()),
        });
        core.log.log("start", core.clock.now_us(), json!({"robot": core.profile.profile.name, "endpoints": core.profile.profile.endpoints}));
        let c = core.clone();
        let session_thread = thread::Builder::new().name("session".into()).spawn(move || c.session_loop()).unwrap();
        let c = core.clone();
        let control_thread = thread::Builder::new().name("control".into()).spawn(move || c.control_loop(pad)).unwrap();
        core.threads.lock().unwrap().extend([session_thread, control_thread]);
        core
    }

    pub fn session_id(&self) -> u32 {
        self.session_id
    }

    /// Decoded video frames for the UI (kept out of Snapshot so frames aren't cloned per draw).
    pub fn video(&self) -> &video::Video {
        &self.video
    }

    pub fn now_us(&self) -> u64 {
        self.clock.now_us()
    }

    // ── UI inputs ────────────────────────────────────────────
    pub fn press_touch_stop(&self) {
        self.touch_stop.store(true, Ordering::SeqCst);
    }
    pub fn set_panel_open(&self, open: bool) {
        self.panel_open.store(open, Ordering::SeqCst);
    }

    pub fn snapshot(&self) -> Snapshot {
        let now = self.clock.now_us();
        let mut st = self.state.lock().unwrap();
        st.notices.retain(|(t, _)| now.saturating_sub(*t) < 5_000_000);
        let h = st.health.as_ref().unwrap();
        let overlay = health::overlay(
            h,
            OverlayInputs {
                has_pad: st.pad.is_some(),
                session_up: st.session_up && st.info_ok,
                fatal: st.fatal.as_ref(),
                local_latch: st.latch.latched(),
                now_us: now,
            },
        );
        let f = &st.last_frame;
        Snapshot {
            overlay: Some(overlay),
            robot_name: st.robot_name.clone(),
            session_id: self.session_id,
            rtt: h.rtt(),
            has_pad: st.pad.is_some(),
            pad_name: st.pad.clone().unwrap_or_default(),
            axes: f.axes,
            buttons: f.buttons,
            deadman: f.axes[4] > 0.5,
            turbo: f.buttons & input::bit(v1::Button::Rb) != 0,
            local_latch: st.latch.latched(),
            clear_progress: f.clear_progress,
            clear_blocked: f.clear_blocked,
            panel_open: self.panel_open.load(Ordering::SeqCst),
            button_hints: st.button_hints.clone(),
            notices: st.notices.clone(),
            controls_sent: st.controls_sent,
            camera: st.camera,
            video_live: self.video.live(now),
            video_ever: self.video.ever(),
            video: self.video.stats(now),
            clock_offset_us: h.clock_offset_us(),
        }
    }

    fn notice(&self, msg: impl Into<String>) {
        let now = self.clock.now_us();
        let mut st = self.state.lock().unwrap();
        st.notices.push((now, msg.into()));
        if st.notices.len() > 5 {
            st.notices.remove(0);
        }
    }

    // ── session ──────────────────────────────────────────────
    fn session_loop(self: Arc<Self>) {
        let links = match self.open() {
            Ok(l) => l,
            Err(e) => {
                self.log.log("connect_failed", self.clock.now_us(), json!({"error": e.to_string()}));
                self.state.lock().unwrap().fatal = Some(Overlay::WrongRobot(format!("can't start Zenoh: {e}")));
                return;
            }
        };
        let _ = self.links.set(links);
        self.state.lock().unwrap().session_up = true;
        self.log.log("session_open", self.clock.now_us(), json!({}));
        // Info: retry until the robot answers, then check it's the paired robot.
        while !self.quit.load(Ordering::SeqCst) {
            match self.query_info() {
                Ok(info) => {
                    self.check_info(info);
                    return;
                }
                Err(_) => thread::sleep(Duration::from_millis(500)),
            }
        }
    }

    fn open(self: &Arc<Self>) -> Result<Links> {
        let session = zenoh::open(zenoh_config(&self.profile)?).wait().map_err(|e| anyhow!("{e}"))?;
        let token = session
            .liveliness()
            .declare_token(keys::deck_token(self.session_id))
            .wait()
            .map_err(|e| anyhow!("token: {e}"))?;
        let me = self.clone();
        let state_sub = session
            .declare_subscriber(keys::ROBOT_STATE)
            .callback(move |sample| {
                if let Ok(m) = RobotState::decode(&*sample.payload().to_bytes()) {
                    me.on_robot_state(m);
                }
            })
            .wait()
            .map_err(|e| anyhow!("robot_state: {e}"))?;
        let me = self.clone();
        let video_sub = session
            .declare_subscriber(keys::VIDEO)
            .callback(move |sample| {
                let now = me.clock.now_us();
                if let Ok(f) = VideoFrame::decode(&*sample.payload().to_bytes()) {
                    let offset = me.state.lock().unwrap().health.as_ref().and_then(|h| h.clock_offset_us());
                    me.video.push(f, now, offset);
                }
            })
            .wait()
            .map_err(|e| anyhow!("video: {e}"))?;
        let control = session
            .declare_publisher(keys::CONTROL)
            .congestion_control(CongestionControl::Drop)
            .priority(Priority::RealTime)
            .express(true)
            .reliability(Reliability::BestEffort)
            .encoding(zenoh::bytes::Encoding::from(keys::encoding("Control")))
            .wait()
            .map_err(|e| anyhow!("control: {e}"))?;
        let stop = session
            .declare_publisher(keys::STOP)
            .congestion_control(CongestionControl::Block)
            .priority(Priority::RealTime)
            .express(true)
            .reliability(Reliability::Reliable)
            .encoding(zenoh::bytes::Encoding::from(keys::encoding("Stop")))
            .wait()
            .map_err(|e| anyhow!("stop: {e}"))?;
        Ok(Links { session, control, stop, _token: token, _state_sub: state_sub, _video_sub: video_sub })
    }

    fn on_robot_state(&self, m: RobotState) {
        let now = self.clock.now_us();
        let (before, after) = {
            let mut st = self.state.lock().unwrap();
            let h = st.health.as_mut().unwrap();
            let before = (h.link_up(now), h.robot());
            h.on_robot_state(m, now);
            (before, (true, h.robot()))
        };
        if !before.0 && after.0 {
            self.log.log("link_up", now, json!({}));
        }
        if before.1 != after.1 {
            let state = match &after.1 {
                Some(health::Robot::Stopped(r)) => format!("Stopped({r})"),
                Some(health::Robot::Busy(o)) => format!("Busy({o:08x})"),
                Some(r) => format!("{r:?}"),
                None => "none".into(),
            };
            self.log.log("robot_state", now, json!({"state": state}));
        }
    }

    fn query_info(&self) -> Result<Info> {
        let links = self.links.get().ok_or_else(|| anyhow!("not connected"))?;
        let replies = links.session.get(keys::INFO).timeout(QUERY_TIMEOUT).wait().map_err(|e| anyhow!("{e}"))?;
        while let Ok(reply) = replies.recv() {
            if let Ok(sample) = reply.result() {
                if let Ok(info) = Info::decode(&*sample.payload().to_bytes()) {
                    return Ok(info);
                }
            }
        }
        Err(anyhow!("no Info reply"))
    }

    fn check_info(&self, info: Info) {
        let now = self.clock.now_us();
        let got = profiles::hex(&info.robot_id);
        let mut st = self.state.lock().unwrap();
        if got != self.profile.profile.id {
            let msg = if got.is_empty() { "robot sent no robot_id".to_string() } else { format!("robot id {}…", &got[..16.min(got.len())]) };
            st.fatal = Some(Overlay::WrongRobot(format!("expected {} ({}…), {msg}", self.profile.profile.name, &self.profile.profile.id[..16])));
            drop(st);
            self.log.log("wrong_robot", now, json!({"got": got}));
            return;
        }
        if info.interface_version != SUPPORTED_INTERFACE {
            st.fatal = Some(Overlay::Version(info.interface_version));
            drop(st);
            self.log.log("version_mismatch", now, json!({"robot": info.interface_version, "app": SUPPORTED_INTERFACE}));
            return;
        }
        st.robot_name = info.robot_name.clone();
        st.button_hints = info.button_hints.clone();
        st.camera = info.camera;
        st.info_ok = true;
        drop(st);
        self.log.log("connected", now, json!({"robot_name": info.robot_name, "interface_minor": info.interface_minor}));
    }

    // ── control ──────────────────────────────────────────────
    fn control_loop(self: Arc<Self>, pad: PadFactory) {
        let mut pad = pad();
        let mut input = InputState::new(InputConfig::default());
        let mut seq: u32 = 0;
        let mut next = Instant::now();
        while !self.quit.load(Ordering::SeqCst) {
            let polled = pad.poll();
            let changed = pad.changed();
            let now_us = self.clock.now_us();
            let touch = self.touch_stop.swap(false, Ordering::SeqCst);
            let panel = self.panel_open.load(Ordering::SeqCst);
            let (raw, name) = polled.clone().unzip();
            let frame = input.process(raw.unwrap_or_default(), now_us, touch, panel);

            for a in &frame.actions {
                match a {
                    Action::Stop(source) => self.stop(source, now_us),
                    Action::ClearRequested => self.request_clear(seq.wrapping_add(1), now_us),
                    Action::TogglePanel => {
                        let open = !self.panel_open.load(Ordering::SeqCst);
                        self.panel_open.store(open, Ordering::SeqCst);
                    }
                }
            }

            let ready = {
                let mut st = self.state.lock().unwrap();
                st.pad = name;
                st.last_frame = frame.clone();
                st.fatal.is_none() && st.info_ok
            };
            let now = Instant::now();
            // No gamepad: send nothing (docs/controller.md § Controls). Wrong robot: never send.
            if ready && polled.is_some() && (changed || now >= next || !frame.actions.is_empty()) {
                if let Some(links) = self.links.get() {
                    seq = seq.wrapping_add(1);
                    let latched = self.state.lock().unwrap().latch.latched();
                    let c = Control {
                        session_id: self.session_id,
                        seq,
                        deck_time_us: self.clock.now_us(),
                        axes: frame.axes.to_vec(),
                        buttons: frame.buttons,
                        stop: latched,
                    };
                    let _ = links.control.put(c.encode_to_vec()).wait();
                    self.state.lock().unwrap().controls_sent += 1;
                }
            }
            if now >= next {
                next += CONTROL_PERIOD;
                if next < now {
                    next = now + CONTROL_PERIOD;
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn stop(&self, source: &str, now_us: u64) {
        self.state.lock().unwrap().latch.on_stop();
        if let Some(links) = self.links.get() {
            let m = Stop { session_id: self.session_id, deck_time_us: now_us, source: source.into() };
            let _ = links.stop.put(m.encode_to_vec()).wait();
        }
        self.log.log("stop", now_us, json!({"source": source}));
    }

    fn request_clear(self: &Arc<Self>, released_seq: u32, now_us: u64) {
        let Some(rs) = self.state.lock().unwrap().latch.on_clear_request(released_seq) else { return };
        self.log.log("clear_requested", now_us, json!({"released_seq": rs}));
        let me = self.clone();
        thread::spawn(move || {
            // The next Control (stop = false, seq = released_seq) goes out within ~20 ms.
            thread::sleep(Duration::from_millis(30));
            let (ok, reason) = me.send_clear(rs).unwrap_or_else(|e| (false, e.to_string()));
            me.state.lock().unwrap().latch.on_clear_reply(rs, ok);
            me.log.log("clear_reply", me.clock.now_us(), json!({"ok": ok, "reason": reason}));
            me.notice(if ok { "STOP cleared".to_string() } else { format!("clear refused: {}", reason.replace('_', " ")) });
        });
    }

    fn send_clear(&self, released_seq: u32) -> Result<(bool, String)> {
        let links = self.links.get().ok_or_else(|| anyhow!("not connected"))?;
        let req = ClearStopRequest { session_id: self.session_id, released_seq };
        let replies = links
            .session
            .get(keys::CLEAR_STOP)
            .payload(req.encode_to_vec())
            .encoding(zenoh::bytes::Encoding::from(keys::encoding("ClearStopRequest")))
            .timeout(QUERY_TIMEOUT)
            .wait()
            .map_err(|e| anyhow!("{e}"))?;
        while let Ok(reply) = replies.recv() {
            if let Ok(sample) = reply.result() {
                if let Ok(r) = ClearStopReply::decode(&*sample.payload().to_bytes()) {
                    return Ok((r.ok, r.reason));
                }
            }
        }
        Ok((false, "timeout".into()))
    }

    /// Quit: Stop (reliable), withdraw the liveliness token, close the session.
    pub fn shutdown(&self) {
        self.quit.store(true, Ordering::SeqCst);
        for t in self.threads.lock().unwrap().drain(..) {
            let _ = t.join();
        }
        let now = self.clock.now_us();
        if let Some(links) = self.links.get() {
            let m = Stop { session_id: self.session_id, deck_time_us: now, source: "quit".into() };
            let _ = links.stop.put(m.encode_to_vec()).wait();
            thread::sleep(Duration::from_millis(100));
            let _ = links.session.close().wait();
        }
        self.log.log("quit", now, json!({}));
    }
}

/// `probe`: connect, read Info, watch RobotState for `secs` without sending Control.
pub struct ProbeResult {
    pub info: Option<Info>,
    pub info_ms: f64,
    pub states: usize,
    pub last_state: Option<RobotState>,
    pub id_ok: bool,
}

pub fn probe(profile: &Installed, secs: f64) -> Result<ProbeResult> {
    let session = zenoh::open(zenoh_config(profile)?).wait().map_err(|e| anyhow!("{e}"))?;
    let last: Arc<Mutex<(usize, Option<RobotState>)>> = Arc::default();
    let l = last.clone();
    let _sub = session
        .declare_subscriber(keys::ROBOT_STATE)
        .callback(move |s| {
            if let Ok(m) = RobotState::decode(&*s.payload().to_bytes()) {
                let mut g = l.lock().unwrap();
                g.0 += 1;
                g.1 = Some(m);
            }
        })
        .wait()
        .map_err(|e| anyhow!("{e}"))?;
    let t0 = Instant::now();
    let mut info = None;
    while info.is_none() && t0.elapsed() < Duration::from_secs_f64(secs.max(3.0)) {
        let replies = session.get(keys::INFO).timeout(QUERY_TIMEOUT).wait().map_err(|e| anyhow!("{e}"))?;
        while let Ok(r) = replies.recv() {
            if let Ok(s) = r.result() {
                info = Info::decode(&*s.payload().to_bytes()).ok();
            }
        }
    }
    let info_ms = t0.elapsed().as_secs_f64() * 1000.0;
    thread::sleep(Duration::from_secs_f64(secs));
    let (states, last_state) = last.lock().unwrap().clone();
    let id_ok = info.as_ref().is_some_and(|i| profiles::hex(&i.robot_id) == profile.profile.id);
    let _ = session.close().wait();
    Ok(ProbeResult { info, info_ms, states, last_state, id_ok })
}
