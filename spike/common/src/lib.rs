//! Shared bits for the M0 spike: key names, tiny fixed-layout messages and
//! Zenoh configuration. The product uses Protobuf (docs/interface.md); the
//! spike only needs to measure the transport, so plain little-endian structs
//! keep it dependency-free.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, Result};

pub const KEY_CONTROL: &str = "rj/spike/control";
pub const KEY_ECHO: &str = "rj/spike/echo";
pub const KEY_VIDEO: &str = "rj/spike/video";
pub const KEY_DECK_TOKEN: &str = "rj/spike/deck";

/// Zenoh errors are boxed trait objects; this turns them into anyhow errors.
pub fn z<T>(r: zenoh::Result<T>) -> Result<T> {
    r.map_err(|e| anyhow!("zenoh: {e}"))
}

/// Microseconds on a process-local monotonic clock.
pub struct MonoClock(Instant);

impl MonoClock {
    pub fn new() -> Self {
        Self(Instant::now())
    }
    pub fn now_us(&self) -> u64 {
        self.0.elapsed().as_micros() as u64
    }
}

impl Default for MonoClock {
    fn default() -> Self {
        Self::new()
    }
}

// ── messages ────────────────────────────────────────────────────────────────

/// Deck → robot, 50 Hz. 40 bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Control {
    pub seq: u32,
    pub deck_us: u64,
    pub axes: [f32; 6],
    pub buttons: u32,
}

impl Control {
    pub const LEN: usize = 4 + 8 + 6 * 4 + 4;

    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(Self::LEN);
        b.extend_from_slice(&self.seq.to_le_bytes());
        b.extend_from_slice(&self.deck_us.to_le_bytes());
        for a in self.axes {
            b.extend_from_slice(&a.to_le_bytes());
        }
        b.extend_from_slice(&self.buttons.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != Self::LEN {
            return None;
        }
        let mut axes = [0f32; 6];
        for (i, a) in axes.iter_mut().enumerate() {
            *a = f32::from_le_bytes(b[12 + i * 4..16 + i * 4].try_into().ok()?);
        }
        Some(Self {
            seq: u32::from_le_bytes(b[0..4].try_into().ok()?),
            deck_us: u64::from_le_bytes(b[4..12].try_into().ok()?),
            axes,
            buttons: u32::from_le_bytes(b[36..40].try_into().ok()?),
        })
    }
}

/// Robot → Deck, one per received Control. 20 bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Echo {
    pub robot_us: u64,
    pub seq: u32,
    pub deck_us: u64,
}

impl Echo {
    pub const LEN: usize = 8 + 4 + 8;

    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(Self::LEN);
        b.extend_from_slice(&self.robot_us.to_le_bytes());
        b.extend_from_slice(&self.seq.to_le_bytes());
        b.extend_from_slice(&self.deck_us.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != Self::LEN {
            return None;
        }
        Some(Self {
            robot_us: u64::from_le_bytes(b[0..8].try_into().ok()?),
            seq: u32::from_le_bytes(b[8..12].try_into().ok()?),
            deck_us: u64::from_le_bytes(b[12..20].try_into().ok()?),
        })
    }
}

/// Robot → Deck video frame: 20-byte header followed by one complete JPEG.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameHeader {
    pub frame_id: u32,
    pub capture_us: u64,
    pub encode_us: u32,
    pub width: u16,
    pub height: u16,
}

impl FrameHeader {
    pub const LEN: usize = 4 + 8 + 4 + 2 + 2;

    pub fn encode_with(&self, jpeg: &[u8]) -> Vec<u8> {
        let mut b = Vec::with_capacity(Self::LEN + jpeg.len());
        b.extend_from_slice(&self.frame_id.to_le_bytes());
        b.extend_from_slice(&self.capture_us.to_le_bytes());
        b.extend_from_slice(&self.encode_us.to_le_bytes());
        b.extend_from_slice(&self.width.to_le_bytes());
        b.extend_from_slice(&self.height.to_le_bytes());
        b.extend_from_slice(jpeg);
        b
    }

    /// Returns the header and the JPEG bytes.
    pub fn decode(b: &[u8]) -> Option<(Self, &[u8])> {
        if b.len() < Self::LEN {
            return None;
        }
        let h = Self {
            frame_id: u32::from_le_bytes(b[0..4].try_into().ok()?),
            capture_us: u64::from_le_bytes(b[4..12].try_into().ok()?),
            encode_us: u32::from_le_bytes(b[12..16].try_into().ok()?),
            width: u16::from_le_bytes(b[16..18].try_into().ok()?),
            height: u16::from_le_bytes(b[18..20].try_into().ok()?),
        };
        Some((h, &b[Self::LEN..]))
    }
}

// ── configuration ───────────────────────────────────────────────────────────

/// Certificates produced by `scripts/gen_certs.sh`.
#[derive(Clone, Debug)]
pub struct Tls {
    pub ca: PathBuf,
    pub cert: PathBuf,
    pub key: PathBuf,
}

impl Tls {
    /// `role` is "robot" or "deck", matching the file names gen_certs.sh writes.
    pub fn from_dir(dir: &Path, role: &str) -> Result<Self> {
        let tls = Self {
            ca: dir.join("ca.pem"),
            cert: dir.join(format!("{role}.pem")),
            key: dir.join(format!("{role}.key")),
        };
        for p in [&tls.ca, &tls.cert, &tls.key] {
            if !p.exists() {
                return Err(anyhow!("missing {} (run scripts/gen_certs.sh)", p.display()));
            }
        }
        Ok(tls)
    }
}

pub enum Role {
    /// Listens; the stand-in for the robot's router.
    Robot,
    /// Connects only to the robot.
    Deck,
}

pub struct ConfigOpts<'a> {
    pub role: Role,
    pub endpoints: &'a [String],
    pub tls: Option<&'a Tls>,
    pub lease_ms: u64,
}

pub fn zenoh_config(o: &ConfigOpts) -> Result<zenoh::Config> {
    let mut c = zenoh::Config::default();
    let set = |c: &mut zenoh::Config, k: &str, v: String| {
        c.insert_json5(k, &v).map_err(|e| anyhow!("config {k}={v}: {e}"))
    };
    let list = serde_list(o.endpoints);
    match o.role {
        Role::Robot => {
            set(&mut c, "mode", "\"router\"".into())?;
            set(&mut c, "listen/endpoints", list)?;
        }
        Role::Deck => {
            set(&mut c, "mode", "\"client\"".into())?;
            set(&mut c, "connect/endpoints", list)?;
            // Default retry starts at 1 s and doubles to 4 s; reconnect faster.
            set(
                &mut c,
                "connect/retry",
                "{period_init_ms: 200, period_max_ms: 1000, period_increase_factor: 1.5}".into(),
            )?;
            // The client must keep retrying after the robot drops, not exit.
            set(&mut c, "connect/exit_on_failure", "false".into())?;
            set(&mut c, "connect/timeout_ms", "-1".into())?;
        }
    }
    set(&mut c, "scouting/multicast/enabled", "false".into())?;
    set(&mut c, "transport/link/tx/lease", o.lease_ms.to_string())?;
    set(&mut c, "transport/link/tx/keep_alive", "4".into())?;
    if let Some(t) = o.tls {
        let p = |p: &Path| format!("{:?}", p.display().to_string());
        set(&mut c, "transport/link/tls/root_ca_certificate", p(&t.ca))?;
        set(&mut c, "transport/link/tls/enable_mtls", "true".into())?;
        match o.role {
            Role::Robot => {
                set(&mut c, "transport/link/tls/listen_certificate", p(&t.cert))?;
                set(&mut c, "transport/link/tls/listen_private_key", p(&t.key))?;
            }
            Role::Deck => {
                set(&mut c, "transport/link/tls/connect_certificate", p(&t.cert))?;
                set(&mut c, "transport/link/tls/connect_private_key", p(&t.key))?;
            }
        }
    }
    Ok(c)
}

fn serde_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("{s:?}")).collect();
    format!("[{}]", quoted.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_roundtrip() {
        let c = Control { seq: 7, deck_us: 123, axes: [0.5, -1.0, 0.0, 1.0, 0.25, 0.0], buttons: 0b101 };
        let d = Control::decode(&c.encode()).unwrap();
        assert_eq!((d.seq, d.deck_us, d.axes, d.buttons), (c.seq, c.deck_us, c.axes, c.buttons));
    }

    #[test]
    fn echo_and_frame_roundtrip() {
        let e = Echo { robot_us: 9, seq: 3, deck_us: 4 };
        let d = Echo::decode(&e.encode()).unwrap();
        assert_eq!((d.robot_us, d.seq, d.deck_us), (9, 3, 4));
        let h = FrameHeader { frame_id: 1, capture_us: 2, encode_us: 3, width: 480, height: 270 };
        let bytes = h.encode_with(&[1, 2, 3]);
        let (d, jpeg) = FrameHeader::decode(&bytes).unwrap();
        assert_eq!((d.frame_id, d.width, jpeg), (1, 480, &[1u8, 2, 3][..]));
    }

    #[test]
    fn config_builds() {
        let eps = vec!["quic/127.0.0.1:7450".to_string()];
        for role in [Role::Robot, Role::Deck] {
            zenoh_config(&ConfigOpts { role, endpoints: &eps, tls: None, lease_ms: 2000 }).unwrap();
        }
    }
}
