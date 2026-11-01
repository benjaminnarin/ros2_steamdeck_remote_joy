//! Local event log: one JSON line per event, rotated at 5 MB with 3 files kept.
//! docs/controller.md § Event log.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const KEEP: usize = 3;

pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
    base.join("steamdeck-remote-joy").join("events.log")
}

pub struct EventLog {
    path: PathBuf,
    lock: Mutex<()>,
    /// Added to every line: robot id, session id.
    context: Mutex<Value>,
}

impl EventLog {
    pub fn new(path: PathBuf) -> Self {
        EventLog { path, lock: Mutex::new(()), context: Mutex::new(json!({})) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn set_context(&self, key: &str, value: Value) {
        if let Value::Object(m) = &mut *self.context.lock().unwrap() {
            m.insert(key.into(), value);
        }
    }

    /// `event` e.g. "stop", `fields` e.g. {"source": "button_b"}. Never fails the caller.
    pub fn log(&self, event: &str, mono_us: u64, fields: Value) {
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let mut line = json!({"t": wall, "mono_us": mono_us, "event": event});
        let m = line.as_object_mut().unwrap();
        if let Value::Object(ctx) = &*self.context.lock().unwrap() {
            for (k, v) in ctx {
                m.insert(k.clone(), v.clone());
            }
        }
        if let Value::Object(f) = fields {
            m.extend(f);
        }
        let _guard = self.lock.lock().unwrap();
        let _ = self.append(&line.to_string());
    }

    fn append(&self, line: &str) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        if fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0) + line.len() as u64 > MAX_BYTES {
            for i in (1..KEEP).rev() {
                let _ = fs::rename(self.rotated(i), self.rotated(i + 1));
            }
            let _ = fs::rename(&self.path, self.rotated(1));
            let _ = fs::remove_file(self.rotated(KEEP));
        }
        let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(f, "{line}")
    }

    fn rotated(&self, i: usize) -> PathBuf {
        PathBuf::from(format!("{}.{i}", self.path.display()))
    }

    /// Last `n` lines (current file only).
    pub fn tail(&self, n: usize) -> Vec<String> {
        let s = fs::read_to_string(&self.path).unwrap_or_default();
        let lines: Vec<&str> = s.lines().collect();
        lines[lines.len().saturating_sub(n)..].iter().map(|l| l.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_json_lines_with_context() {
        let dir = std::env::temp_dir().join(format!("rj-events-{}", std::process::id()));
        let log = EventLog::new(dir.join("events.log"));
        log.set_context("session", json!("0000abcd"));
        log.log("stop", 42, json!({"source": "button_b"}));
        let lines = log.tail(10);
        assert_eq!(lines.len(), 1);
        let v: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v["event"], "stop");
        assert_eq!(v["source"], "button_b");
        assert_eq!(v["session"], "0000abcd");
        assert_eq!(v["mono_us"], 42);
        let _ = fs::remove_dir_all(dir);
    }
}
