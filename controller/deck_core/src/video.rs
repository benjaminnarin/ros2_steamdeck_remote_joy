//! rj/v1/video -> newest-only slot -> decode thread -> latest RGB frame + stats.
//! docs/controller.md § Threads, § Debug overlay.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use deck_msgs::v1::VideoFrame;

/// The Deck drops bigger frames (docs/interface.md § Limits).
pub const MAX_FRAME_BYTES: usize = 512 * 1024;
/// NO VIDEO after this long without a decoded frame.
pub const NO_VIDEO_US: u64 = 1_000_000;
const WINDOW_US: u64 = 5_000_000;

#[derive(Clone)]
pub struct Decoded {
    /// Increments per decoded frame: the UI re-uploads its texture when it changes.
    pub generation: u64,
    pub width: usize,
    pub height: usize,
    pub rgb: Arc<Vec<u8>>,
    pub frame_id: u32,
}

/// One frame's timeline. Robot times are converted to the Deck clock with the clock offset.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    pub frame_id: u32,
    pub bytes: usize,
    /// capture -> encoded, robot side (from the frame).
    pub encode_us: u32,
    /// encoded -> received, needs the clock offset.
    pub transfer_us: Option<i64>,
    pub received_us: u64,
    pub decoded_us: u64,
    pub shown_us: Option<u64>,
    pub capture_deck_us: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub fps: f64,
    /// Frames received / frames sent (from frame_id gaps), %.
    pub delivered_pct: f64,
    pub avg_kb: f64,
    pub encode_ms: f64,
    pub transfer_ms: Option<f64>,
    pub decode_ms: f64,
    pub show_ms: Option<f64>,
    pub capture_to_shown_ms: Option<f64>,
    pub frames: usize,
    /// The robot's adaptive rate level of the newest frame (0 = full rate).
    pub level: u32,
    pub dropped_too_big: u64,
    pub decode_errors: u64,
}

/// Pure statistics over a sliding window of Timings.
#[derive(Default)]
pub struct StatsWindow {
    t: VecDeque<Timing>,
}

impl StatsWindow {
    pub fn push(&mut self, t: Timing) {
        self.t.push_back(t);
    }

    pub fn mark_shown(&mut self, frame_id: u32, now_us: u64) {
        if let Some(t) = self.t.iter_mut().rev().find(|t| t.frame_id == frame_id) {
            t.shown_us.get_or_insert(now_us);
        }
    }

    pub fn stats(&mut self, now_us: u64) -> Stats {
        while self.t.front().is_some_and(|t| t.received_us + WINDOW_US < now_us) {
            self.t.pop_front();
        }
        let n = self.t.len();
        if n == 0 {
            return Stats::default();
        }
        let avg = |f: &dyn Fn(&Timing) -> Option<f64>| {
            let v: Vec<f64> = self.t.iter().filter_map(f).collect();
            (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
        };
        let first = self.t.front().unwrap();
        let last = self.t.back().unwrap();
        let span_us = last.received_us.saturating_sub(first.received_us);
        let ids = last.frame_id.wrapping_sub(first.frame_id) as f64 + 1.0;
        Stats {
            fps: if span_us > 0 { (n - 1) as f64 * 1e6 / span_us as f64 } else { 0.0 },
            delivered_pct: (n as f64 / ids * 100.0).min(100.0),
            avg_kb: avg(&|t| Some(t.bytes as f64 / 1024.0)).unwrap_or(0.0),
            encode_ms: avg(&|t| Some(t.encode_us as f64 / 1000.0)).unwrap_or(0.0),
            transfer_ms: avg(&|t| t.transfer_us.map(|v| v as f64 / 1000.0)),
            decode_ms: avg(&|t| Some(t.decoded_us.saturating_sub(t.received_us) as f64 / 1000.0)).unwrap_or(0.0),
            show_ms: avg(&|t| t.shown_us.map(|s| s.saturating_sub(t.decoded_us) as f64 / 1000.0)),
            capture_to_shown_ms: avg(&|t| match (t.shown_us, t.capture_deck_us) {
                (Some(s), Some(c)) => Some((s as i64 - c) as f64 / 1000.0),
                _ => None,
            }),
            frames: n,
            ..Default::default()
        }
    }
}

/// What the Deck tells deck_video (rj/v1/video_feedback), counted as frames arrive (before the
/// decode slot, so frames the decoder skips aren't reported as lost).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Received {
    pub last_frame_id: u32,
    pub last_rx_us: u64,
    pub count: u32,
}

#[derive(Default)]
struct Inner {
    received: Option<Received>,
    level: u32,
    pending: Option<(VideoFrame, u64, Option<i64>)>,
    latest: Option<Decoded>,
    last_decoded_us: Option<u64>,
    generation: u64,
    window: StatsWindow,
    dropped_too_big: u64,
    decode_errors: u64,
}

/// Shared between the Zenoh callback (push), the decode thread and the UI.
#[derive(Default)]
pub struct Video {
    inner: Mutex<Inner>,
    ready: Condvar,
}

impl Video {
    pub fn new() -> Arc<Self> {
        let v = Arc::new(Video::default());
        let w = Arc::downgrade(&v);
        thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || loop {
                let Some(v) = w.upgrade() else { return };
                v.decode_one();
            })
            .unwrap();
        v
    }

    /// From the Zenoh callback: keep only the newest undecoded frame. `offset_us` is
    /// robot_clock - deck_clock, if known.
    pub fn push(&self, f: VideoFrame, received_us: u64, offset_us: Option<i64>) {
        let mut g = self.inner.lock().unwrap();
        let count = g.received.map_or(0, |r| r.count).wrapping_add(1);
        g.received = Some(Received { last_frame_id: f.frame_id, last_rx_us: received_us, count });
        g.level = f.level;
        if f.data.len() > MAX_FRAME_BYTES {
            g.dropped_too_big += 1;
            return;
        }
        g.pending = Some((f, received_us, offset_us)); // the older one, if any, is dropped
        self.ready.notify_one();
    }

    fn decode_one(&self) {
        let (f, received_us, offset) = {
            let g = self.inner.lock().unwrap();
            // Wake at least every 500 ms so the thread notices when Video is dropped.
            let (mut g, _) = self.ready.wait_timeout_while(g, std::time::Duration::from_millis(500), |g| g.pending.is_none()).unwrap();
            match g.pending.take() {
                Some(p) => p,
                None => return,
            }
        };
        let mut dec = zune_jpeg::JpegDecoder::new(&f.data);
        let result = dec.decode();
        let decoded_us = crate::mono_us();
        let mut g = self.inner.lock().unwrap();
        match result {
            Ok(rgb) => {
                let (w, h) = dec.dimensions().unwrap_or((f.width as usize, f.height as usize));
                if rgb.len() != w * h * 3 {
                    g.decode_errors += 1;
                    return;
                }
                g.generation += 1;
                let generation = g.generation;
                g.latest = Some(Decoded { generation, width: w, height: h, rgb: Arc::new(rgb), frame_id: f.frame_id });
                g.last_decoded_us = Some(decoded_us);
                let capture_deck = offset.map(|o| f.capture_time_us as i64 - o);
                g.window.push(Timing {
                    frame_id: f.frame_id,
                    bytes: f.data.len(),
                    encode_us: f.encode_done_offset_us,
                    transfer_us: capture_deck.map(|c| received_us as i64 - (c + f.encode_done_offset_us as i64)),
                    received_us,
                    decoded_us,
                    shown_us: None,
                    capture_deck_us: capture_deck,
                });
            }
            Err(_) => g.decode_errors += 1,
        }
    }

    /// The newest decoded frame, if it's newer than `generation`.
    pub fn latest_if_newer(&self, generation: u64) -> Option<Decoded> {
        let g = self.inner.lock().unwrap();
        g.latest.as_ref().filter(|d| d.generation > generation).cloned()
    }

    /// The UI uploaded frame `frame_id` to the screen.
    pub fn mark_shown(&self, frame_id: u32) {
        self.inner.lock().unwrap().window.mark_shown(frame_id, crate::mono_us());
    }

    pub fn live(&self, now_us: u64) -> bool {
        self.inner.lock().unwrap().last_decoded_us.is_some_and(|t| now_us.saturating_sub(t) < NO_VIDEO_US)
    }

    pub fn ever(&self) -> bool {
        self.inner.lock().unwrap().latest.is_some()
    }

    /// For rj/v1/video_feedback: None until a frame has arrived.
    pub fn received(&self) -> Option<Received> {
        self.inner.lock().unwrap().received
    }

    pub fn stats(&self, now_us: u64) -> Stats {
        let mut g = self.inner.lock().unwrap();
        let mut s = g.window.stats(now_us);
        s.level = g.level;
        s.dropped_too_big = g.dropped_too_big;
        s.decode_errors = g.decode_errors;
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1000;

    fn t(id: u32, rx_ms: u64) -> Timing {
        Timing {
            frame_id: id,
            bytes: 18 * 1024,
            encode_us: 9_000,
            transfer_us: Some(4_000),
            received_us: rx_ms * MS,
            decoded_us: rx_ms * MS + 3 * MS,
            shown_us: None,
            capture_deck_us: Some((rx_ms * MS) as i64 - 13_000),
        }
    }

    #[test]
    fn fps_delivery_and_stages() {
        let mut w = StatsWindow::default();
        // 30 fps, but frame 5 is lost.
        for id in 1..=31u32 {
            if id != 5 {
                w.push(t(id, 1_000 + (id as u64 - 1) * 33));
            }
        }
        w.mark_shown(31, (1_000 + 30 * 33 + 3 + 7) * MS);
        let s = w.stats((1_000 + 30 * 33 + 20) * MS);
        assert_eq!(s.frames, 30);
        assert!((s.delivered_pct - 30.0 / 31.0 * 100.0).abs() < 0.01);
        // 30 frames received over 990 ms = 29 intervals.
        assert!((s.fps - 29.0 * 1000.0 / 990.0).abs() < 0.01, "fps {}", s.fps);
        assert_eq!(s.avg_kb, 18.0);
        assert_eq!(s.encode_ms, 9.0);
        assert_eq!(s.transfer_ms, Some(4.0));
        assert_eq!(s.decode_ms, 3.0);
        assert_eq!(s.show_ms, Some(7.0));
        assert_eq!(s.capture_to_shown_ms, Some(13.0 + 3.0 + 7.0));
    }

    #[test]
    fn window_expires() {
        let mut w = StatsWindow::default();
        w.push(t(1, 1_000));
        assert_eq!(w.stats(1_000 * MS).frames, 1);
        assert_eq!(w.stats(7_000 * MS), Stats::default());
    }

    #[test]
    fn newest_wins_and_big_frames_drop() {
        let v = Video::default(); // no decode thread: inspect the slot directly
        let frame = |id, n| VideoFrame { frame_id: id, data: vec![0; n], ..Default::default() };
        v.push(frame(1, 10), 1, None);
        v.push(frame(2, 10), 2, None);
        v.push(frame(3, MAX_FRAME_BYTES + 1), 3, None);
        let g = v.inner.lock().unwrap();
        assert_eq!(g.pending.as_ref().unwrap().0.frame_id, 2, "older pending frame replaced; too-big one dropped");
        assert_eq!(g.dropped_too_big, 1);
    }

    #[test]
    fn received_counts_every_arrival() {
        let v = Video::default();
        assert_eq!(v.received(), None);
        let frame = |id, n| VideoFrame { frame_id: id, data: vec![0; n], level: 2, ..Default::default() };
        v.push(frame(1, 10), 100, None);
        v.push(frame(3, MAX_FRAME_BYTES + 1), 200, None); // arrived, though too big to decode
        assert_eq!(v.received(), Some(Received { last_frame_id: 3, last_rx_us: 200, count: 2 }));
        assert_eq!(v.stats(300).level, 2);
    }
}
