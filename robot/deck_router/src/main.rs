//! deck_router: the robot's Zenoh router for the Steam Deck link.
//!
//! It does what `rmw_zenohd` does (opens a Zenoh session in router mode from a config file
//! and runs until stopped), but it's built from crates.io Zenoh with our own Cargo.lock. That
//! matters because the zenoh-c vendored by ROS locks quinn-proto 0.11.17, which panics in
//! `Datagrams::send` once the QUIC datagram buffer fills (quinn #2805). That happens on Wi-Fi
//! with mixed_rel=1, and it aborts the router. See docs/devlog.md § 3.4.
//!
//! Drop-in for rmw_zenohd's interface:
//!   ZENOH_ROUTER_CONFIG_URI  config file (or pass the path as the only argument)
//!   ZENOH_CONFIG_OVERRIDE    `key=json5;key=json5;...`, applied after the file (TLS paths)
//!   RUST_LOG                 log filter (default: info)
//!
//! `deck_router --check <config>` validates the config and exits.

use std::process::ExitCode;
use std::sync::mpsc;

use zenoh::{Config, Wait};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("deck_router: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let check_only = args.first().is_some_and(|a| a == "--check");
    if check_only {
        args.remove(0);
    }
    if args.iter().any(|a| a == "-h" || a == "--help") || args.len() > 1 {
        return Err("usage: deck_router [--check] [config.json5]  (default: $ZENOH_ROUTER_CONFIG_URI)".into());
    }
    let path = match args.pop() {
        Some(p) => p,
        None => std::env::var("ZENOH_ROUTER_CONFIG_URI")
            .map_err(|_| "no config: pass a path or set ZENOH_ROUTER_CONFIG_URI".to_string())?,
    };

    let config = load(&path, std::env::var("ZENOH_CONFIG_OVERRIDE").ok().as_deref())?;
    if check_only {
        println!("deck_router: {path}: ok");
        return Ok(());
    }

    zenoh::init_log_from_env_or("info");
    let session = zenoh::open(config).wait().map_err(|e| format!("open: {e}"))?;
    println!("deck_router: Zenoh {} router {} started from {path}", zenoh_version(), session.zid());

    let (tx, rx) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = tx.send(());
    })
    .map_err(|e| format!("signal handler: {e}"))?;
    let _ = rx.recv();
    println!("deck_router: stopping");
    session.close().wait().map_err(|e| format!("close: {e}"))?;
    Ok(())
}

/// Loads the config file, applies the override string, and checks the rules that the
/// Deck link depends on.
pub fn load(path: &str, overrides: Option<&str>) -> Result<Config, String> {
    let mut config = Config::from_file(path).map_err(|e| format!("{path}: {e}"))?;
    for (key, value) in parse_overrides(overrides.unwrap_or(""))? {
        config
            .insert_json5(key, value)
            .map_err(|e| format!("ZENOH_CONFIG_OVERRIDE {key}={value}: {e}"))?;
    }
    validate(&config)?;
    Ok(config)
}

/// `key=value;key=value`, the format rmw_zenoh uses. Values are JSON5. Empty items are ignored.
pub fn parse_overrides(s: &str) -> Result<Vec<(&str, &str)>, String> {
    s.split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            item.split_once('=')
                .map(|(k, v)| (k.trim(), v.trim()))
                .filter(|(k, v)| !k.is_empty() && !v.is_empty())
                .ok_or_else(|| format!("ZENOH_CONFIG_OVERRIDE: bad item {item:?} (want key=value)"))
        })
        .collect()
}

/// Refuses configs that would quietly break the Deck link.
pub fn validate(config: &Config) -> Result<(), String> {
    let get = |key: &str| -> Result<serde_json::Value, String> {
        let json = config.get_json(key).map_err(|e| format!("config {key}: {e}"))?;
        serde_json::from_str(&json).map_err(|e| format!("config {key}: {e}"))
    };
    if get("mode")? != "router" {
        return Err("mode must be \"router\"".into());
    }
    let endpoints = get("listen/endpoints")?;
    // listen/endpoints is either a list or a per-mode map ({router: [...], ...}).
    let list = match &endpoints {
        serde_json::Value::Object(m) => m.get("router").cloned().unwrap_or_default(),
        other => other.clone(),
    };
    let list: Vec<String> = serde_json::from_value(list).map_err(|e| format!("listen/endpoints: {e}"))?;
    if list.is_empty() {
        return Err("listen/endpoints is empty".into());
    }
    for ep in list.iter().filter(|e| e.starts_with("quic/")) {
        // Without mixed_rel=1, best-effort data rides QUIC streams and stalls replay stale
        // control and video (M0, O3). Zenoh would accept the endpoint without complaint.
        if !ep.split(['?', '&']).any(|p| p == "mixed_rel=1") {
            return Err(format!("{ep}: QUIC endpoints must include mixed_rel=1 (docs/interface.md § Connection)"));
        }
    }
    Ok(())
}

fn zenoh_version() -> &'static str {
    // Matches the exact pin in Cargo.toml; tests/pins.rs keeps the two in step.
    "1.10.2"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_parse() {
        let o = parse_overrides(r#" a/b="x" ; c=true;; "#).unwrap();
        assert_eq!(o, vec![("a/b", "\"x\""), ("c", "true")]);
        assert!(parse_overrides("novalue").is_err());
        assert!(parse_overrides("k=").is_err());
        assert_eq!(parse_overrides("").unwrap(), vec![]);
    }

    fn cfg(json: &str) -> Config {
        Config::from_json5(json).unwrap()
    }

    #[test]
    fn requires_router_mode_and_mixed_rel() {
        assert!(validate(&cfg(r#"{mode: "router", listen: {endpoints: ["quic/0.0.0.0:7450?mixed_rel=1", "tcp/127.0.0.1:7451"]}}"#)).is_ok());
        let e = validate(&cfg(r#"{mode: "router", listen: {endpoints: ["quic/0.0.0.0:7450"]}}"#)).unwrap_err();
        assert!(e.contains("mixed_rel=1"), "{e}");
        assert!(validate(&cfg(r#"{mode: "router", listen: {endpoints: ["quic/0.0.0.0:7450?rel=0"]}}"#)).is_err());
        assert!(validate(&cfg(r#"{mode: "router", listen: {endpoints: ["quic/0.0.0.0:7450?rel=0&mixed_rel=1"]}}"#)).is_ok());
        assert!(validate(&cfg(r#"{mode: "client", connect: {endpoints: ["tcp/127.0.0.1:7451"]}}"#)).is_err());
    }
}
