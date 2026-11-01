//! Compiles interface/proto/remote_joy/v1/*.proto with protox (pure Rust, no protoc needed)
//! and generates prost types. The .proto files are the single source of truth.
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let proto_root = manifest.join("../../interface/proto");
    let v1 = proto_root.join("remote_joy/v1");

    let mut files: Vec<PathBuf> = std::fs::read_dir(&v1)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "proto"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!("no .proto files in {}", v1.display()).into());
    }

    println!("cargo:rerun-if-changed={}", v1.display());
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    let zenoh_version = manifest.join("../../interface/ZENOH_VERSION");
    println!("cargo:rerun-if-changed={}", zenoh_version.display());
    // key=value lines; the Deck app uses the crates.io pin (`crates=`).
    let pins = std::fs::read_to_string(&zenoh_version)?;
    let crates = pins
        .lines()
        .find_map(|l| l.strip_prefix("crates="))
        .ok_or("interface/ZENOH_VERSION has no crates= line")?;
    println!("cargo:rustc-env=REMOTE_JOY_ZENOH_VERSION={}", crates.trim());

    let fds = protox::compile(&files, [&proto_root])?;
    prost_build::Config::new().compile_fds(fds)?;
    Ok(())
}
