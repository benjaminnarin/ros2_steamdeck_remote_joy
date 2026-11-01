//! Stand-in robot for the M0 spike. Runs a Zenoh router, echoes every Control
//! message straight back (so the Deck can measure RTT and stale bursts), and
//! streams synthetic JPEG frames. Prints one stats line per second.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use spike_common::*;
use zenoh::qos::{CongestionControl, Priority, Reliability};
use zenoh::Wait;

#[derive(Parser)]
#[command(about = "M0 spike: stand-in robot (router + echo + synthetic video)", version)]
struct Args {
    /// Endpoints to listen on (repeatable). `mixed_rel=1` is required for
    /// best-effort data to use QUIC datagrams (see spike/README.md).
    #[arg(long, default_values_t = vec!["quic/0.0.0.0:7450?mixed_rel=1".to_string()])]
    listen: Vec<String>,
    /// Directory with ca.pem, robot.pem, robot.key.
    #[arg(long, default_value = "certs")]
    certs: PathBuf,
    /// Disable TLS (for plain tcp/udp baselines).
    #[arg(long)]
    no_tls: bool,
    #[arg(long, default_value_t = 2000)]
    lease_ms: u64,
    /// Don't be the router: connect as a client to this local endpoint of a separate
    /// router (e.g. rmw_zenohd: tcp/127.0.0.1:7451), like deck_gateway. Implies no TLS.
    #[arg(long)]
    local: Option<String>,
    /// Use the product's rj/v1 key names (see common::Keys::v1).
    #[arg(long)]
    v1_keys: bool,
    /// Publish echo and video as reliable instead of best effort.
    #[arg(long)]
    reliable: bool,
    #[arg(long, default_value_t = 480)]
    width: u16,
    #[arg(long, default_value_t = 270)]
    height: u16,
    /// 0 disables video.
    #[arg(long, default_value_t = 30)]
    fps: u32,
    #[arg(long, default_value_t = 70)]
    quality: u8,
    /// Per-pixel noise amplitude; raises JPEG size towards real camera frames.
    #[arg(long, default_value_t = 24)]
    noise: u8,
}

/// Control arrival statistics, reset every second.
#[derive(Default)]
struct ControlStats {
    count: u64,
    max_gap_ms: f64,
    last_arrival: Option<Instant>,
    last_seq: u32,
    seq_gaps: u64,
    /// After a gap > 200 ms: how many messages arrived within 2 ms of each
    /// other, i.e. a queued (stale) burst being flushed.
    burst_after_stall: u64,
    in_recovery: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let clock = Arc::new(MonoClock::new());
    let keys = Keys::new(args.v1_keys);
    let (role, endpoints) = match &args.local {
        Some(ep) => (Role::Local, vec![ep.clone()]),
        None => (Role::Robot, args.listen.clone()),
    };
    let tls = if args.no_tls || args.local.is_some() { None } else { Some(Tls::from_dir(&args.certs, "robot")?) };
    let config = zenoh_config(&ConfigOpts { role, endpoints: &endpoints, tls: tls.as_ref(), lease_ms: args.lease_ms })?;
    let reliability = if args.reliable { Reliability::Reliable } else { Reliability::BestEffort };
    let session = z(zenoh::open(config).wait())?;
    match &args.local {
        Some(ep) => println!("robot_spike: client of local router {ep}, keys {keys:?}"),
        None => println!("robot_spike: listening on {:?} (tls: {}), keys {keys:?}", args.listen, tls.is_some()),
    }

    let echo_pub = z(session
        .declare_publisher(keys.echo)
        .congestion_control(CongestionControl::Drop)
        .priority(Priority::RealTime)
        .express(true)
        .reliability(reliability)
        .wait())?;

    let stats = Arc::new(Mutex::new(ControlStats::default()));
    let _control_sub = {
        let stats = stats.clone();
        let clock = clock.clone();
        z(session
            .declare_subscriber(keys.control)
            .callback(move |sample| {
                let Some(c) = Control::decode(&sample.payload().to_bytes()) else { return };
                let echo = Echo { robot_us: clock.now_us(), seq: c.seq, deck_us: c.deck_us };
                let _ = echo_pub.put(echo.encode()).wait();

                let now = Instant::now();
                let mut s = stats.lock().unwrap();
                s.count += 1;
                if let Some(last) = s.last_arrival {
                    let gap = now.duration_since(last).as_secs_f64() * 1000.0;
                    s.max_gap_ms = s.max_gap_ms.max(gap);
                    if gap > 200.0 {
                        s.in_recovery = true;
                        println!("  control gap {gap:.0} ms (seq {} → {})", s.last_seq, c.seq);
                    } else if s.in_recovery && gap < 2.0 {
                        s.burst_after_stall += 1;
                    } else {
                        s.in_recovery = false;
                    }
                }
                if s.last_seq != 0 && c.seq > s.last_seq + 1 {
                    s.seq_gaps += (c.seq - s.last_seq - 1) as u64;
                }
                s.last_seq = c.seq;
                s.last_arrival = Some(now);
            })
            .wait())?
    };

    let _token_watch = z(session
        .liveliness()
        .declare_subscriber(format!("{}/**", keys.deck_token))
        .callback(|sample| println!("  deck liveliness {:?}: {}", sample.kind(), sample.key_expr()))
        .wait())?;

    let frames_sent = Arc::new(AtomicU64::new(0));
    let bytes_sent = Arc::new(AtomicU64::new(0));
    if args.fps > 0 {
        let video_pub = z(session
            .declare_publisher(keys.video)
            .congestion_control(CongestionControl::Drop)
            .priority(Priority::InteractiveLow)
            .reliability(reliability)
            .wait())?;
        let (frames_sent, bytes_sent, clock) = (frames_sent.clone(), bytes_sent.clone(), clock.clone());
        let (w, h, fps, quality, noise) = (args.width, args.height, args.fps, args.quality, args.noise);
        thread::spawn(move || {
            let period = Duration::from_secs_f64(1.0 / fps as f64);
            let mut next = Instant::now();
            let mut rgb = vec![0u8; w as usize * h as usize * 3];
            let mut rng = 0x2545_f491_4f6c_dd1du64;
            for frame_id in 1u32.. {
                let capture_us = clock.now_us();
                render(&mut rgb, w as usize, h as usize, frame_id, noise, &mut rng);
                let mut jpeg = Vec::with_capacity(64 * 1024);
                let enc = jpeg_encoder::Encoder::new(&mut jpeg, quality);
                if let Err(e) = enc.encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb) {
                    eprintln!("jpeg encode failed: {e}");
                    return;
                }
                let header = FrameHeader {
                    frame_id,
                    capture_us,
                    encode_us: (clock.now_us() - capture_us) as u32,
                    width: w,
                    height: h,
                };
                let payload = header.encode_with(&jpeg);
                bytes_sent.fetch_add(payload.len() as u64, Ordering::Relaxed);
                let _ = video_pub.put(payload).wait();
                frames_sent.fetch_add(1, Ordering::Relaxed);
                next += period;
                let now = Instant::now();
                if next > now {
                    thread::sleep(next - now);
                } else {
                    next = now; // fell behind: skip ahead, never queue
                }
            }
        });
    }

    loop {
        thread::sleep(Duration::from_secs(1));
        let mut s = stats.lock().unwrap();
        let frames = frames_sent.swap(0, Ordering::Relaxed);
        let bytes = bytes_sent.swap(0, Ordering::Relaxed);
        println!(
            "control {:>3}/s  max_gap {:>6.1} ms  seq_lost {:>3}  stale_burst {:>3} | video {:>2} fps {:>5.1} KB/frame {:>5.2} Mbps",
            s.count,
            s.max_gap_ms,
            s.seq_gaps,
            s.burst_after_stall,
            frames,
            if frames > 0 { bytes as f64 / frames as f64 / 1024.0 } else { 0.0 },
            bytes as f64 * 8.0 / 1e6,
        );
        let (last_arrival, last_seq, in_recovery) = (s.last_arrival, s.last_seq, s.in_recovery);
        *s = ControlStats { last_arrival, last_seq, in_recovery, ..Default::default() };
    }
}

/// Moving gradient + bar + frame-id blocks, with noise so JPEG sizes resemble
/// a real camera rather than a flat test pattern.
fn render(rgb: &mut [u8], w: usize, h: usize, frame_id: u32, noise: u8, rng: &mut u64) {
    let bar = (frame_id as usize * 4) % w;
    for y in 0..h {
        for x in 0..w {
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            let n = if noise > 0 { (*rng % noise as u64) as u8 } else { 0 };
            let i = (y * w + x) * 3;
            let on_bar = x.abs_diff(bar) < 6;
            rgb[i] = if on_bar { 255 } else { ((x * 255 / w) as u8).saturating_add(n) };
            rgb[i + 1] = if on_bar { 255 } else { ((y * 255 / h) as u8).saturating_add(n) };
            rgb[i + 2] = if on_bar { 255 } else { (128u8).saturating_add(n) };
        }
    }
    // 32 blocks along the top encode frame_id, readable in screenshots.
    let bw = w / 32;
    for bit in 0..32 {
        let v = if frame_id >> bit & 1 == 1 { 255 } else { 0 };
        for y in 0..12.min(h) {
            for x in bit * bw..(bit + 1) * bw - 1 {
                let i = (y * w + x) * 3;
                rgb[i..i + 3].fill(v);
            }
        }
    }
}
