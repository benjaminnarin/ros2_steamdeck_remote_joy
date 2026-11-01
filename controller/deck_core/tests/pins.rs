//! The Deck app is a QUIC client too: keep zenoh on the interface pin and quinn-proto past the
//! datagram panic (quinn #2805; see robot/deck_router/tests/pins.rs).

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn locked(name: &str) -> String {
    let lock = read("../Cargo.lock");
    let needle = format!("name = \"{name}\"\nversion = \"");
    let start = lock.find(&needle).unwrap_or_else(|| panic!("{name} not in Cargo.lock")) + needle.len();
    lock[start..].split('"').next().unwrap().to_string()
}

fn parts(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap()).collect()
}

#[test]
fn zenoh_matches_interface_pin() {
    let pins = read("../../interface/ZENOH_VERSION");
    let want = pins.lines().find_map(|l| l.strip_prefix("crates=")).unwrap().trim();
    assert_eq!(locked("zenoh"), want);
    assert_eq!(deck_msgs::ZENOH_VERSION, want);
    assert!(read("../Cargo.toml").contains(&format!("version = \"={want}\"")));
}

#[test]
fn quinn_proto_has_datagram_fix() {
    let v = locked("quinn-proto");
    assert!(parts(&v) >= parts("0.11.18"), "quinn-proto {v} < 0.11.18 (quinn #2805)");
}
