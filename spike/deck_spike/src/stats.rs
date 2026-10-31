//! Link and video statistics for the Deck spike, over a sliding 5 s window.

use std::collections::VecDeque;
use std::sync::Arc;

use spike_common::{Echo, FrameHeader};

const WINDOW_US: u64 = 5_000_000;
/// An echo slower than this means the control it answers was delivered late,
/// i.e. it sat in a queue: the "stale burst" the spike is looking for.
pub const STALE_RTT_MS: f64 = 250.0;
const LINK_LOST_US: u64 = 300_000;
const NO_VIDEO_US: u64 = 1_000_000;

#[derive(Default)]
pub struct Link {
    samples: VecDeque<(u64, f64, i64)>, // (recv_us, rtt_ms, offset_us)
    last_echo_us: Option<u64>,
    last_echo_seq: u32,
    pub echoes_total: u64,
    pub lost_total: u64,
    pub stale_total: u64,
    pub max_gap_ms: f64,
    sent_seq: u32,
    recovery: Option<Recovery>,
    events: Vec<String>,
}

struct Recovery {
    gap_ms: f64,
    started_us: u64,
    stale: u64,
    max_rtt_ms: f64,
}

impl Link {
    pub fn on_sent(&mut self, seq: u32) {
        self.sent_seq = seq;
    }

    pub fn on_echo(&mut self, e: Echo, now_us: u64) {
        let rtt_us = now_us.saturating_sub(e.deck_us);
        let rtt_ms = rtt_us as f64 / 1000.0;
        let offset = e.robot_us as i64 - (e.deck_us + rtt_us / 2) as i64;
        if let Some(last) = self.last_echo_us {
            let gap_ms = (now_us - last) as f64 / 1000.0;
            self.max_gap_ms = self.max_gap_ms.max(gap_ms);
            if gap_ms > 200.0 {
                self.recovery = Some(Recovery { gap_ms, started_us: now_us, stale: 0, max_rtt_ms: 0.0 });
            }
        }
        if self.last_echo_seq != 0 && e.seq > self.last_echo_seq + 1 {
            self.lost_total += (e.seq - self.last_echo_seq - 1) as u64;
        }
        self.last_echo_seq = self.last_echo_seq.max(e.seq);
        if rtt_ms > STALE_RTT_MS {
            self.stale_total += 1;
        }
        if let Some(r) = self.recovery.as_mut() {
            r.max_rtt_ms = r.max_rtt_ms.max(rtt_ms);
            if rtt_ms > STALE_RTT_MS {
                r.stale += 1;
            }
            if now_us - r.started_us > 1_000_000 {
                let verdict = if r.stale == 0 { "OK, no stale burst" } else { "STALE BURST" };
                self.events.push(format!(
                    "stall {:.0} ms → recovered; next 1 s: {} stale echoes, max RTT {:.0} ms ({verdict})",
                    r.gap_ms, r.stale, r.max_rtt_ms
                ));
                self.recovery = None;
            }
        }
        self.last_echo_us = Some(now_us);
        self.echoes_total += 1;
        self.samples.push_back((now_us, rtt_ms, offset));
        while self.samples.front().is_some_and(|s| now_us - s.0 > WINDOW_US) {
            self.samples.pop_front();
        }
    }

    /// Robot clock minus Deck clock, from the lowest-RTT sample in the window.
    pub fn clock_offset_us(&self) -> Option<i64> {
        self.samples.iter().min_by(|a, b| a.1.total_cmp(&b.1)).map(|s| s.2)
    }

    pub fn take_new_events(&mut self) -> Vec<String> {
        std::mem::take(&mut self.events)
    }
}

pub struct DecodedFrame {
    pub generation: u64,
    pub frame_id: u32,
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

#[derive(Default)]
pub struct Video {
    arrivals: VecDeque<(u64, usize, Option<f64>)>, // (recv_us, bytes, capture→recv ms)
    decodes: VecDeque<f64>,
    last_id: u32,
    pub missing_total: u64,
    pub received_total: u64,
    last_arrival_us: Option<u64>,
    latest: Option<Arc<DecodedFrame>>,
    generation: u64,
}

impl Video {
    pub fn on_arrival(&mut self, h: &FrameHeader, bytes: usize, now_us: u64, offset_us: Option<i64>) {
        if self.last_id != 0 && h.frame_id > self.last_id + 1 {
            self.missing_total += (h.frame_id - self.last_id - 1) as u64;
        }
        self.last_id = self.last_id.max(h.frame_id);
        self.received_total += 1;
        // capture time in Deck clock = robot capture − (robot − deck offset)
        let latency = offset_us.map(|o| (now_us as i64 - (h.capture_us as i64 - o)) as f64 / 1000.0);
        self.arrivals.push_back((now_us, bytes, latency));
        while self.arrivals.front().is_some_and(|a| now_us - a.0 > WINDOW_US) {
            self.arrivals.pop_front();
        }
        self.last_arrival_us = Some(now_us);
    }

    pub fn on_decoded(&mut self, frame_id: u32, width: usize, height: usize, rgb: Vec<u8>, decode_ms: f64) {
        self.generation += 1;
        self.latest = Some(Arc::new(DecodedFrame { generation: self.generation, frame_id, width, height, rgb }));
        self.decodes.push_back(decode_ms);
        if self.decodes.len() > 150 {
            self.decodes.pop_front();
        }
    }

    pub fn latest_if_newer(&self, shown: u64) -> Option<Arc<DecodedFrame>> {
        self.latest.as_ref().filter(|f| f.generation > shown).cloned()
    }
}

/// One snapshot of everything worth printing.
pub struct Summary {
    pub link_up: bool,
    pub video_live: bool,
    rtt_min: f64,
    rtt_avg: f64,
    rtt_p95: f64,
    rtt_max: f64,
    echo_rate: f64,
    lost_total: u64,
    stale_total: u64,
    max_gap_ms: f64,
    offset_ms: Option<f64>,
    fps: f64,
    delivered_pct: f64,
    kb_per_frame: f64,
    mbps: f64,
    latency_ms: Option<f64>,
    decode_ms: f64,
    last_frame_id: u32,
}

impl Summary {
    pub const CSV_HEADER: &'static str = "t_s,link_up,rtt_min,rtt_avg,rtt_p95,rtt_max,echo_rate,lost_total,stale_total,max_gap_ms,fps,delivered_pct,kb_per_frame,mbps,latency_ms,decode_ms";

    pub fn new(link: &Link, video: &Video, now_us: u64) -> Self {
        let mut rtts: Vec<f64> = link.samples.iter().map(|s| s.1).collect();
        rtts.sort_by(f64::total_cmp);
        let pct = |p: f64| rtts.get(((rtts.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0.0);
        let window_s = WINDOW_US as f64 / 1e6;
        let n = video.arrivals.len() as f64;
        let bytes: usize = video.arrivals.iter().map(|a| a.1).sum();
        let lats: Vec<f64> = video.arrivals.iter().filter_map(|a| a.2).collect();
        let total = video.received_total + video.missing_total;
        Self {
            link_up: link.last_echo_us.is_some_and(|t| now_us.saturating_sub(t) < LINK_LOST_US),
            video_live: video.last_arrival_us.is_some_and(|t| now_us.saturating_sub(t) < NO_VIDEO_US),
            rtt_min: rtts.first().copied().unwrap_or(0.0),
            rtt_avg: if rtts.is_empty() { 0.0 } else { rtts.iter().sum::<f64>() / rtts.len() as f64 },
            rtt_p95: pct(0.95),
            rtt_max: rtts.last().copied().unwrap_or(0.0),
            echo_rate: link.samples.len() as f64 / window_s,
            lost_total: link.lost_total,
            stale_total: link.stale_total,
            max_gap_ms: link.max_gap_ms,
            offset_ms: link.clock_offset_us().map(|o| o as f64 / 1000.0),
            fps: n / window_s,
            delivered_pct: if total > 0 { video.received_total as f64 * 100.0 / total as f64 } else { 0.0 },
            kb_per_frame: if n > 0.0 { bytes as f64 / n / 1024.0 } else { 0.0 },
            mbps: bytes as f64 * 8.0 / window_s / 1e6,
            latency_ms: if lats.is_empty() { None } else { Some(lats.iter().sum::<f64>() / lats.len() as f64) },
            decode_ms: if video.decodes.is_empty() { 0.0 } else { video.decodes.iter().sum::<f64>() / video.decodes.len() as f64 },
            last_frame_id: video.latest.as_ref().map(|f| f.frame_id).unwrap_or(0),
        }
    }

    pub fn line(&self) -> String {
        format!(
            "{} RTT {:.1}/{:.1}/{:.1}/{:.0} ms (min/avg/p95/max) echo {:.0}/s lost {} stale {} | video {:.1} fps {:.1}% {:.1} KB {:.1} Mbps lat {}",
            if self.link_up { "UP  " } else { "DOWN" },
            self.rtt_min,
            self.rtt_avg,
            self.rtt_p95,
            self.rtt_max,
            self.echo_rate,
            self.lost_total,
            self.stale_total,
            self.fps,
            self.delivered_pct,
            self.kb_per_frame,
            self.mbps,
            self.latency_ms.map(|l| format!("{l:.1} ms")).unwrap_or("-".into()),
        )
    }

    pub fn detail_lines(&self) -> Vec<String> {
        vec![
            format!("RTT min/avg/p95/max  {:.1} / {:.1} / {:.1} / {:.0} ms", self.rtt_min, self.rtt_avg, self.rtt_p95, self.rtt_max),
            format!("echo rate {:.0}/s   lost {}   stale (>{STALE_RTT_MS:.0} ms) {}", self.echo_rate, self.lost_total, self.stale_total),
            format!("max echo gap {:.0} ms   clock offset {}", self.max_gap_ms, self.offset_ms.map(|o| format!("{o:.1} ms")).unwrap_or("-".into())),
            format!("video {:.1} fps   delivered {:.1}%   frame #{}", self.fps, self.delivered_pct, self.last_frame_id),
            format!("{:.1} KB/frame   {:.1} Mbps   decode {:.1} ms", self.kb_per_frame, self.mbps, self.decode_ms),
            format!("capture→received {}", self.latency_ms.map(|l| format!("{l:.1} ms")).unwrap_or("-".into())),
        ]
    }

    pub fn csv(&self, t_s: f64) -> String {
        format!(
            "{t_s:.1},{},{:.2},{:.2},{:.2},{:.2},{:.1},{},{},{:.1},{:.1},{:.2},{:.2},{:.2},{},{:.2}",
            self.link_up as u8,
            self.rtt_min,
            self.rtt_avg,
            self.rtt_p95,
            self.rtt_max,
            self.echo_rate,
            self.lost_total,
            self.stale_total,
            self.max_gap_ms,
            self.fps,
            self.delivered_pct,
            self.kb_per_frame,
            self.mbps,
            self.latency_ms.map(|l| format!("{l:.2}")).unwrap_or_default(),
            self.decode_ms,
        )
    }
}
