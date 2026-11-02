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
    let mut args = cli_args(std::env::args().skip(1));
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

/// Command-line arguments, minus the `--ros-args ...` tail that launch_ros appends to every Node.
pub fn cli_args(args: impl Iterator<Item = String>) -> Vec<String> {
    args.take_while(|a| a != "--ros-args").collect()
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
        // Endpoint metadata: quic/<addr>?k=v;k=v#config.
        let meta = ep.split('#').next().unwrap_or("").split_once('?').map_or("", |(_, m)| m);
        let has = |opt: &str| meta.split(';').any(|p| p == opt);
        // Without mixed_rel=1, best-effort data rides QUIC streams and stalls replay stale
        // control and video (M0, O3). Zenoh would accept the endpoint without complaint.
        if !has("mixed_rel=1") {
            return Err(format!("{ep}: QUIC endpoints must include mixed_rel=1 (docs/interface.md § Connection)"));
        }
        // One QUIC stream per priority for reliable keys (stop, queries), so they don't queue
        // behind each other. The default (auto) negotiates it too, but we don't rely on that.
        if !has("multistream=1") {
            return Err(format!("{ep}: QUIC endpoints must include multistream=1 (docs/interface.md § Connection)"));
        }
    }
    // Each Deck opens one link per priority band; with max_links 1 the second is refused and
    // video shares the RealTime link's datagram queue again (O19).
    let max_links: u64 = serde_json::from_value(get("transport/unicast/max_links")?).map_err(|e| format!("transport/unicast/max_links: {e}"))?;
    if max_links < 2 {
        return Err(format!("transport/unicast/max_links is {max_links}: needs at least 2 (docs/interface.md § Connection)"));
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
    fn ignores_ros_args() {
        let a = |v: &[&str]| cli_args(v.iter().map(|s| s.to_string()));
        assert_eq!(a(&["--ros-args", "-r", "__node:=deck_router"]), Vec::<String>::new());
        assert_eq!(a(&["--check", "f.json5", "--ros-args", "-r", "x"]), vec!["--check", "f.json5"]);
    }

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
    fn requires_router_mode_mixed_rel_multistream_and_two_links() {
        let ok = |eps: &str| validate(&cfg(&format!(r#"{{mode: "router", transport: {{unicast: {{max_links: 2}}}}, listen: {{endpoints: {eps}}}}}"#)));
        assert!(ok(r#"["quic/0.0.0.0:7450?mixed_rel=1;multistream=1", "tcp/127.0.0.1:7451"]"#).is_ok());
        assert!(ok(r#"["quic/0.0.0.0:7450?multistream=1;rel=0;mixed_rel=1#bind=0.0.0.0:0"]"#).is_ok());
        let e = ok(r#"["quic/0.0.0.0:7450"]"#).unwrap_err();
        assert!(e.contains("mixed_rel=1"), "{e}");
        let e = ok(r#"["quic/0.0.0.0:7450?mixed_rel=1"]"#).unwrap_err();
        assert!(e.contains("multistream=1"), "{e}");
        assert!(ok(r#"["quic/0.0.0.0:7450?mixed_rel=10;multistream=1"]"#).is_err());
        let e = validate(&cfg(r#"{mode: "router", listen: {endpoints: ["quic/0.0.0.0:7450?mixed_rel=1;multistream=1"]}}"#)).unwrap_err();
        assert!(e.contains("max_links"), "{e}");
        assert!(validate(&cfg(r#"{mode: "client", connect: {endpoints: ["tcp/127.0.0.1:7451"]}}"#)).is_err());
    }
}
