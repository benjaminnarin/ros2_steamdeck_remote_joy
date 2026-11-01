//! Bundle import against a real `pair` bundle when one is given (RJ_TEST_BUNDLE), plus the
//! tamper check on a synthetic one.

use std::io::Write;
use std::path::PathBuf;

use deck_core::profiles::{pem_cert_sha256, Store};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rj-profiles-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A self-signed-looking PEM is enough here: the import only hashes the DER.
const CA: &str = "-----BEGIN CERTIFICATE-----\nAAECAwQFBgcICQ==\n-----END CERTIFICATE-----\n";

fn bundle(dir: &std::path::Path, id: &str) -> PathBuf {
    let toml = format!(
        "[robot]\nname = \"mybot\"\nid = \"{id}\"\nendpoints = [\"quic/10.0.0.5:7450?mixed_rel=1\"]\n\n[deck]\nname = \"deck1\"\ncn = \"deck1-abcdef\"\n"
    );
    let path = dir.join("b.tar");
    let mut b = tar::Builder::new(std::fs::File::create(&path).unwrap());
    for (name, data) in [("ca.pem", CA.as_bytes()), ("client.pem", b"cert".as_slice()), ("client.key", b"key"), ("robot.toml", toml.as_bytes())] {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o600);
        h.set_cksum();
        b.append_data(&mut h, name, data).unwrap();
    }
    b.into_inner().unwrap().flush().unwrap();
    path
}

#[test]
fn add_list_active_forget() {
    let d = tmp("ok");
    let id = pem_cert_sha256(CA).unwrap();
    let store = Store::new(d.join("cfg"));
    let inst = store.add_bundle(&bundle(&d, &id)).unwrap();
    assert_eq!(inst.profile.name, "mybot");
    assert_eq!(inst.profile.deck_cn, "deck1-abcdef");
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(inst.key()), 0o600);
    assert_eq!(mode(inst.dir.clone()), 0o700);
    assert_eq!(store.active().unwrap().profile.id, id);
    assert_eq!(store.list().len(), 1);
    store.forget("mybot").unwrap();
    assert!(store.list().is_empty());
    assert!(store.active().is_err());
}

#[test]
fn tampered_bundle_is_refused() {
    let d = tmp("bad");
    let store = Store::new(d.join("cfg"));
    let err = store.add_bundle(&bundle(&d, &"0".repeat(64))).unwrap_err().to_string();
    assert!(err.contains("tampered"), "{err}");
}

#[test]
fn real_pair_bundle() {
    let Some(path) = std::env::var_os("RJ_TEST_BUNDLE") else { return };
    let d = tmp("real");
    let inst = Store::new(d.join("cfg")).add_bundle(&PathBuf::from(path)).unwrap();
    assert!(inst.profile.endpoints.iter().all(|e| e.contains("mixed_rel=1")));
}
