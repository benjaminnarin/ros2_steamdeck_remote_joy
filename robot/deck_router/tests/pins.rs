//! Keeps deck_router's dependencies on the versions the Deck link was tested with.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(p: PathBuf) -> String {
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn pin(key: &str) -> String {
    read(root().join("../../interface/ZENOH_VERSION"))
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")).map(|v| v.trim().to_string()))
        .unwrap_or_else(|| panic!("{key}= missing from interface/ZENOH_VERSION"))
}

fn locked(name: &str) -> String {
    let lock = read(root().join("Cargo.lock"));
    let needle = format!("name = \"{name}\"\nversion = \"");
    let start = lock.find(&needle).unwrap_or_else(|| panic!("{name} not in Cargo.lock")) + needle.len();
    lock[start..].split('"').next().unwrap().to_string()
}

fn parts(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap()).collect()
}

#[test]
fn zenoh_matches_interface_pin() {
    let want = pin("crates");
    assert_eq!(locked("zenoh"), want, "Cargo.lock zenoh vs interface/ZENOH_VERSION crates=");
    assert!(read(root().join("Cargo.toml")).contains(&format!("zenoh = \"={want}\"")));
    assert!(read(root().join("src/main.rs")).contains(&format!("\"{want}\"")), "zenoh_version() in main.rs");
    assert!(want.starts_with(&format!("{}.", pin("series"))));
}

/// quinn-proto 0.11.17 panics in Datagrams::send when evicting outgoing datagrams
/// (quinn #2805, fixed in 0.11.18), which kills the router under Wi-Fi load with mixed_rel=1.
#[test]
fn quinn_proto_has_datagram_fix() {
    let v = locked("quinn-proto");
    assert!(parts(&v) >= parts("0.11.18"), "quinn-proto {v} < 0.11.18 (quinn #2805)");
}
