//! M0 spike app for the Steam Deck.
//!
//! - O1: shows every gamepad gilrs sees, with live axes/buttons.
//! - O2: runs fullscreen under gamescope, with a touch test button.
//! - O3/O15: sends Control at 50 Hz over Zenoh/TLS, measures RTT from the
//!   robot's echo, and reports stalls and any stale burst after recovery.
//! - Video: receives synthetic JPEG frames, decodes off the UI thread, and
//!   reports fps, delivery and capture→received latency.
//!
//! `--headless` skips the window and prints one stats line per second.

mod stats;
mod v1;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use eframe::egui;
use spike_common::*;
use stats::{Link, Summary, Video};
use v1::V1;
use zenoh::qos::{CongestionControl, Priority, Reliability};
use zenoh::Wait;

#[derive(Parser, Clone)]
#[command(about = "M0 spike: Steam Deck side", version)]
struct Args {
    /// Robot address (used when --connect isn't given).
    #[arg(long, default_value = "192.168.1.131")]
    robot: String,
    #[arg(long, default_value_t = 7450)]
    port: u16,
    /// Explicit endpoints (repeatable), e.g. "quic/192.168.1.131:7450?rel=0".
    #[arg(long)]
    connect: Vec<String>,
    /// Raw Zenoh config override (repeatable), KEY=JSON5, applied last, e.g.
    /// 'transport/unicast/max_links=2'.
    #[arg(long, value_name = "KEY=JSON5")]
    zcfg: Vec<String>,
    /// File of --zcfg lines (KEY=JSON5 per line; blank lines and # comments skipped).
    #[arg(long)]
    zcfg_file: Option<PathBuf>,
    /// Directory with ca.pem, deck.pem, deck.key.
    #[arg(long, default_value = "certs")]
    certs: PathBuf,
    /// Disable TLS (for plain tcp/udp baselines).
    #[arg(long)]
    no_tls: bool,
    #[arg(long, default_value_t = 2000)]
    lease_ms: u64,
    /// Publish Control as reliable instead of best effort.
    #[arg(long)]
    reliable: bool,
    #[arg(long, default_value_t = 50)]
    rate_hz: u32,
    /// No window: print stats once per second.
    #[arg(long)]
    headless: bool,
    /// Headless only: stop after this many seconds (0 = run forever).
    #[arg(long, default_value_t = 0)]
    duration_s: u64,
    /// Headless only: also append per-second stats to this CSV file.
    #[arg(long)]
    csv: Option<PathBuf>,
    /// Window instead of fullscreen.
    #[arg(long)]
    windowed: bool,
    /// Use the product's rj/v1 key names (see common::Keys::v1), e.g. against rmw_zenohd
    /// with the product router.json5 (O17).
    #[arg(long)]
    v1_keys: bool,
    /// Speak the product interface (Protobuf on rj/v1/*) to drive the real deck_gateway:
    /// B = STOP, View+Menu 1 s = clear STOP. Implies --v1-keys.
    #[arg(long)]
    v1: bool,
    /// Don't subscribe to video (with --v1, deck_video then sends nothing: O19 baseline).
    #[arg(long)]
    no_video: bool,
}

/// What the control thread last read from the gamepads.
#[derive(Default, Clone)]
struct Input {
    status: String,
    pads: Vec<String>,
    axes: [f32; 6],
    buttons: u32,
    events: u64,
    last_event: String,
}

#[derive(Default)]
struct Shared {
    input: Mutex<Input>,
    link: Mutex<Link>,
    video: Mutex<Video>,
    /// Latest undecoded video payload; the decode thread takes it.
    pending_frame: Mutex<Option<Vec<u8>>>,
    frame_ready: Condvar,
    sent: AtomicU64,
    quit: AtomicBool,
    /// Events from the --v1 logic (STOP, clear), shown with the link events.
    extra_events: Mutex<Vec<String>>,
}

const BUTTONS: [(gilrs::Button, &str); 15] = [
    (gilrs::Button::South, "A"),
    (gilrs::Button::East, "B"),
    (gilrs::Button::West, "X"),
    (gilrs::Button::North, "Y"),
    (gilrs::Button::Select, "View"),
    (gilrs::Button::Mode, "Steam"),
    (gilrs::Button::Start, "Menu"),
    (gilrs::Button::LeftThumb, "L3"),
    (gilrs::Button::RightThumb, "R3"),
    (gilrs::Button::LeftTrigger, "LB"),
    (gilrs::Button::RightTrigger, "RB"),
    (gilrs::Button::DPadUp, "Up"),
    (gilrs::Button::DPadDown, "Down"),
    (gilrs::Button::DPadLeft, "Left"),
    (gilrs::Button::DPadRight, "Right"),
];
const AXIS_NAMES: [&str; 6] = ["LX", "LY", "RX", "RY", "LT", "RT"];

fn main() -> Result<()> {
    let args = Args::parse();
    let clock = Arc::new(MonoClock::new());
    let shared = Arc::new(Shared::default());
    let keys = Keys::new(args.v1_keys || args.v1);

    let endpoints = if args.connect.is_empty() {
        if args.no_tls {
            vec![format!("udp/{}:{}", args.robot, args.port)]
        } else {
            // mixed_rel=1: best effort over QUIC datagrams; without it a stall replays stale data.
            vec![format!("quic/{}:{}?mixed_rel=1", args.robot, args.port)]
        }
    } else {
        args.connect.clone()
    };
    let tls = if args.no_tls { None } else { Some(Tls::from_dir(&args.certs, "deck")?) };
    let mut config = zenoh_config(&ConfigOpts {
        role: Role::Deck,
        endpoints: &endpoints,
        tls: tls.as_ref(),
        lease_ms: args.lease_ms,
    })?;
    let mut zcfg = args.zcfg.clone();
    if let Some(f) = &args.zcfg_file {
        let text = std::fs::read_to_string(f)?;
        zcfg.extend(text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from));
    }
    for kv in &zcfg {
        let (k, v) = kv.split_once('=').ok_or_else(|| anyhow::anyhow!("--zcfg wants KEY=JSON5: {kv}"))?;
        config.insert_json5(k, v).map_err(|e| anyhow::anyhow!("--zcfg {k}={v}: {e}"))?;
        println!("deck_spike: config {k} = {v}");
    }
    println!("deck_spike: connecting to {endpoints:?} (tls: {}), keys {keys:?}", tls.is_some());
    let session = z(zenoh::open(config).wait())?;

    let session_id = (clock.now_us() as u32) ^ std::process::id();
    let _token = z(session.liveliness().declare_token(format!("{}/{session_id:08x}", keys.deck_token)).wait())?;
    let v1 = if args.v1 { Some(V1::new(session.clone(), session_id)?) } else { None };
    if args.v1 {
        println!("deck_spike: --v1 (Protobuf), session {session_id:08x}: B = STOP, View+Menu 1 s = clear");
    }

    let _echo_sub = {
        let (shared, clock, v1) = (shared.clone(), clock.clone(), v1.clone());
        z(session
            .declare_subscriber(keys.echo)
            .callback(move |sample| {
                let bytes = sample.payload().to_bytes();
                let echo = match &v1 {
                    Some(v) => v.on_robot_state(&bytes),
                    None => Echo::decode(&bytes),
                };
                if let Some(e) = echo {
                    shared.link.lock().unwrap().on_echo(e, clock.now_us());
                }
            })
            .wait())?
    };

    let _video_sub = if args.no_video { None } else { Some({
        let (shared, clock) = (shared.clone(), clock.clone());
        z(session
            .declare_subscriber(keys.video)
            .callback(move |sample| {
                let bytes = sample.payload().to_bytes().into_owned();
                if let Some((h, _)) = FrameHeader::decode(&bytes) {
                    let offset = shared.link.lock().unwrap().clock_offset_us();
                    shared.video.lock().unwrap().on_arrival(&h, bytes.len(), clock.now_us(), offset);
                    *shared.pending_frame.lock().unwrap() = Some(bytes); // newest wins
                    shared.frame_ready.notify_one();
                }
            })
            .wait())?
    }) };

    spawn_decoder(shared.clone());

    let control_pub = z(session
        .declare_publisher(keys.control)
        .congestion_control(CongestionControl::Drop)
        .priority(Priority::RealTime)
        .express(true)
        .reliability(if args.reliable { Reliability::Reliable } else { Reliability::BestEffort })
        .wait())?;
    {
        let (shared, clock, rate, v1) = (shared.clone(), clock.clone(), args.rate_hz, v1.clone());
        thread::spawn(move || control_loop(shared, clock, control_pub, rate, v1));
    }

    let result = if args.headless {
        run_headless(&args, &shared, &clock, v1.as_deref())
    } else {
        run_gui(&args, shared.clone(), clock.clone(), endpoints, v1.clone())
    };
    if let Some(v) = &v1 {
        // Quitting stops the robot right away (docs/interface.md § Rules, Deck).
        shared.quit.store(true, Ordering::Relaxed);
        v.send_stop("quit", clock.now_us());
        thread::sleep(Duration::from_millis(200));
    }
    result
}

/// Owns gilrs (created on this thread), sends Control every tick and
/// immediately on any button change.
fn control_loop(
    shared: Arc<Shared>,
    clock: Arc<MonoClock>,
    publisher: zenoh::pubsub::Publisher<'static>,
    rate_hz: u32,
    v1: Option<Arc<V1>>,
) {
    let mut gilrs = match gilrs::Gilrs::new() {
        Ok(g) => Some(g),
        Err(e) => {
            shared.input.lock().unwrap().status = format!("gilrs init failed: {e}");
            None
        }
    };
    let period = Duration::from_secs_f64(1.0 / rate_hz.max(1) as f64);
    let mut next = Instant::now();
    let mut seq = 0u32;
    while !shared.quit.load(Ordering::Relaxed) {
        let mut changed = false;
        let mut input = shared.input.lock().unwrap().clone();
        if let Some(g) = gilrs.as_mut() {
            while let Some(ev) = g.next_event() {
                input.events += 1;
                input.last_event = format!("{:?}", ev.event);
                if !matches!(ev.event, gilrs::EventType::AxisChanged(..) | gilrs::EventType::ButtonChanged(..)) {
                    println!("{:.3} GILRS {}", clock.now_us() as f64 / 1e6, input.last_event);
                }
                changed |= matches!(ev.event, gilrs::EventType::ButtonPressed(..) | gilrs::EventType::ButtonReleased(..));
            }
            input.pads = g
                .gamepads()
                .map(|(id, p)| {
                    format!("#{id} {:?} (os: {:?}) mapping: {:?} power: {:?}", p.name(), p.os_name(), p.mapping_source(), p.power_info())
                })
                .collect();
            input.status = if input.pads.is_empty() { "no gamepad".into() } else { "ok".into() };
            if let Some((_, p)) = g.gamepads().next() {
                use gilrs::{Axis, Button};
                let trig = |b| p.button_data(b).map(|d| d.value()).unwrap_or(0.0);
                input.axes = [
                    p.value(Axis::LeftStickX),
                    p.value(Axis::LeftStickY),
                    p.value(Axis::RightStickX),
                    p.value(Axis::RightStickY),
                    trig(Button::LeftTrigger2),
                    trig(Button::RightTrigger2),
                ];
                input.buttons = BUTTONS
                    .iter()
                    .enumerate()
                    .filter(|(_, (b, _))| p.is_pressed(*b))
                    .fold(0, |acc, (i, _)| acc | 1 << i);
            } else {
                input.axes = [0.0; 6];
                input.buttons = 0;
            }
        }
        let (axes, buttons) = (input.axes, input.buttons);
        *shared.input.lock().unwrap() = input;
        if let Some(v) = &v1 {
            let events = v.on_input(buttons, axes, seq.wrapping_add(1), clock.now_us());
            for e in &events {
                println!("{:.3} EVENT {e}", clock.now_us() as f64 / 1e6);
            }
            shared.extra_events.lock().unwrap().extend(events);
        }

        let now = Instant::now();
        if changed || now >= next {
            seq = seq.wrapping_add(1);
            let deck_us = clock.now_us();
            let payload = match &v1 {
                Some(v) => v.encode_control(seq, deck_us, axes, buttons),
                None => Control { seq, deck_us, axes, buttons }.encode().to_vec(),
            };
            let _ = publisher.put(payload).wait();
            shared.sent.fetch_add(1, Ordering::Relaxed);
            shared.link.lock().unwrap().on_sent(seq);
            if now >= next {
                next += period;
                if next < now {
                    next = now + period;
                }
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
}

/// Decodes the newest pending JPEG; older ones are dropped by the subscriber.
fn spawn_decoder(shared: Arc<Shared>) {
    thread::spawn(move || loop {
        let bytes = {
            let mut pending = shared.pending_frame.lock().unwrap();
            while pending.is_none() {
                pending = shared.frame_ready.wait(pending).unwrap();
            }
            pending.take().unwrap()
        };
        let Some((h, jpeg)) = FrameHeader::decode(&bytes) else { continue };
        let started = Instant::now();
        let mut decoder = zune_jpeg::JpegDecoder::new(jpeg);
        match decoder.decode() {
            Ok(rgb) => {
                let (w, hgt) = decoder.dimensions().unwrap_or((h.width as usize, h.height as usize));
                let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
                shared.video.lock().unwrap().on_decoded(h.frame_id, w, hgt, rgb, decode_ms);
            }
            Err(e) => eprintln!("jpeg decode failed: {e}"),
        }
    });
}

fn summary(shared: &Shared, now_us: u64) -> Summary {
    let link = shared.link.lock().unwrap();
    let video = shared.video.lock().unwrap();
    Summary::new(&link, &video, now_us)
}

fn run_headless(args: &Args, shared: &Shared, clock: &MonoClock, v1: Option<&V1>) -> Result<()> {
    use std::io::Write;
    let mut csv = match &args.csv {
        Some(p) => {
            let new = !p.exists();
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p)?;
            if new {
                writeln!(f, "{}", Summary::CSV_HEADER)?;
            }
            Some(f)
        }
        None => None,
    };
    let started = Instant::now();
    let mut pads_seen = Vec::new();
    loop {
        thread::sleep(Duration::from_secs(1));
        let s = summary(shared, clock.now_us());
        let input = shared.input.lock().unwrap().clone();
        if input.pads != pads_seen {
            for p in &input.pads {
                println!("  GAMEPAD {p}");
            }
            pads_seen = input.pads.clone();
        }
        println!("{}  | pad: {}", s.line(), input.status);
        if let Some(v) = v1 {
            println!("  ROBOT {}", v.status_line().0);
        }
        shared.extra_events.lock().unwrap().clear(); // already printed by the control loop
        for e in shared.link.lock().unwrap().take_new_events() {
            println!("  EVENT {e}");
        }
        if let Some(f) = csv.as_mut() {
            writeln!(f, "{}", s.csv(started.elapsed().as_secs_f64()))?;
        }
        if args.duration_s > 0 && started.elapsed().as_secs() >= args.duration_s {
            shared.quit.store(true, Ordering::Relaxed);
            return Ok(());
        }
    }
}

fn run_gui(args: &Args, shared: Arc<Shared>, clock: Arc<MonoClock>, endpoints: Vec<String>, v1: Option<Arc<V1>>) -> Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("RJ spike")
        .with_inner_size([1280.0, 800.0])
        .with_fullscreen(!args.windowed);
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "RJ spike",
        options,
        Box::new(move |_cc| Ok(Box::new(SpikeApp::new(shared, clock, endpoints, v1)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}

struct SpikeApp {
    shared: Arc<Shared>,
    clock: Arc<MonoClock>,
    endpoints: Vec<String>,
    texture: Option<egui::TextureHandle>,
    shown_frame: u64,
    touches: u64,
    events: Vec<String>,
    last_log: Instant,
    logged_pads: Vec<String>,
    logged_input: ([f32; 6], u32),
    v1: Option<Arc<V1>>,
    logged_status: String,
}

impl SpikeApp {
    fn new(shared: Arc<Shared>, clock: Arc<MonoClock>, endpoints: Vec<String>, v1: Option<Arc<V1>>) -> Self {
        Self {
            shared,
            clock,
            endpoints,
            texture: None,
            shown_frame: 0,
            touches: 0,
            events: Vec::new(),
            last_log: Instant::now(),
            logged_pads: Vec::new(),
            logged_input: ([0.0; 6], 0),
            v1,
            logged_status: String::new(),
        }
    }
}

impl eframe::App for SpikeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let s = summary(&self.shared, self.clock.now_us());
        let input = self.shared.input.lock().unwrap().clone();
        let mut new_events = self.shared.link.lock().unwrap().take_new_events();
        new_events.extend(std::mem::take(&mut *self.shared.extra_events.lock().unwrap()));
        let t = self.clock.now_us() as f64 / 1e6;
        for e in &new_events {
            println!("{t:.3} EVENT {e}");
        }
        self.events.extend(new_events);
        // Mirror what's on screen into the log so tests can be read back over SSH.
        if input.pads != self.logged_pads {
            for p in &input.pads {
                println!("{t:.3} GAMEPAD {p}");
            }
            self.logged_pads = input.pads.clone();
        }
        let moved = input.axes.iter().zip(self.logged_input.0).any(|(a, b)| (a - b).abs() > 0.2);
        if moved || input.buttons != self.logged_input.1 {
            let ax: Vec<String> = input.axes.iter().zip(AXIS_NAMES).map(|(v, n)| format!("{n}={v:+.2}")).collect();
            println!("{t:.3} INPUT {} buttons={:#07x}", ax.join(" "), input.buttons);
            self.logged_input = (input.axes, input.buttons);
        }
        if self.last_log.elapsed() >= Duration::from_secs(1) {
            println!("{t:.3} STATS {} | taps {} | screen {:?}", s.line(), self.touches, ctx.content_rect().size());
            self.last_log = Instant::now();
        }
        let keep = self.events.len().saturating_sub(10);
        self.events.drain(..keep);

        if let Some(f) = self.shared.video.lock().unwrap().latest_if_newer(self.shown_frame) {
            let image = egui::ColorImage::from_rgb([f.width, f.height], &f.rgb);
            match &mut self.texture {
                Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture("video", image, egui::TextureOptions::LINEAR)),
            }
            self.shown_frame = f.generation;
        }

        egui::Panel::top("link").show(ui, |ui| {
            let (txt, col) = if s.link_up {
                ("● CONNECTED", egui::Color32::GREEN)
            } else {
                ("● LINK LOST", egui::Color32::RED)
            };
            ui.horizontal(|ui| {
                ui.colored_label(col, egui::RichText::new(txt).size(22.0).strong());
                ui.label(egui::RichText::new(s.line()).monospace().size(15.0));
            });
            if let Some(v) = &self.v1 {
                let (line, [r, g, b]) = v.status_line();
                ui.colored_label(egui::Color32::from_rgb(r, g, b), egui::RichText::new(&line).size(30.0).strong());
                if line != self.logged_status {
                    println!("{t:.3} ROBOT {line}");
                    self.logged_status = line;
                }
            }
        });

        egui::Panel::right("info").exact_size(420.0).show(ui, |ui| {
            ui.heading("Gamepad (O1)");
            ui.label(format!("status: {}   events: {}", input.status, input.events));
            for p in &input.pads {
                ui.label(egui::RichText::new(p).small());
            }
            ui.label(egui::RichText::new(format!("last: {}", input.last_event)).small().monospace());
            for (i, name) in AXIS_NAMES.iter().enumerate() {
                let v = input.axes[i];
                ui.horizontal(|ui| {
                    ui.monospace(format!("{name} {v:+.3}"));
                    let frac = if i >= 4 { v } else { (v + 1.0) / 2.0 };
                    ui.add(egui::ProgressBar::new(frac.clamp(0.0, 1.0)).desired_width(220.0));
                });
            }
            let pressed: Vec<&str> =
                BUTTONS.iter().enumerate().filter(|(i, _)| input.buttons >> i & 1 == 1).map(|(_, (_, n))| *n).collect();
            ui.monospace(format!("pressed: {}", pressed.join(" ")));

            ui.separator();
            ui.heading("Touch (O2)");
            if ui.add_sized([380.0, 70.0], egui::Button::new(egui::RichText::new("TAP ME").size(28.0))).clicked() {
                self.touches += 1;
            }
            ui.label(format!("taps: {}   screen: {:?}", self.touches, ctx.content_rect().size()));

            ui.separator();
            ui.heading("Link / video (O3, O15)");
            for l in s.detail_lines() {
                ui.monospace(l);
            }
            ui.label(egui::RichText::new(self.endpoints.join(", ")).small());
            ui.separator();
            ui.label("Events:");
            for e in self.events.iter().rev() {
                ui.monospace(e);
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                self.shared.quit.store(true, Ordering::Relaxed);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });

        egui::CentralPanel::default().show(ui, |ui| match &self.texture {
            Some(t) => {
                let avail = ui.available_size();
                let ts = t.size_vec2();
                let scale = (avail.x / ts.x).min(avail.y / ts.y);
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new((t.id(), ts * scale)));
                });
                if !s.video_live {
                    ui.painter().text(
                        ui.max_rect().center(),
                        egui::Align2::CENTER_CENTER,
                        "NO VIDEO",
                        egui::FontId::proportional(48.0),
                        egui::Color32::RED,
                    );
                }
            }
            None => {
                ui.centered_and_justified(|ui| ui.heading("waiting for video…"));
            }
        });

        ctx.request_repaint();
    }
}
