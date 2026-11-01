//! Deck <-> robot interface v1: generated Protobuf types and Zenoh key names.
//!
//! Generated from `interface/proto/remote_joy/v1/*.proto` at build time.
//! Spec: `docs/interface.md`.

/// `remote_joy.v1` messages (`Control`, `RobotState`, `Info`, ...).
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/remote_joy.v1.rs"));
}

pub mod keys;

pub use prost::Message;

/// Interface version this crate speaks (`Info.interface_version`).
pub const INTERFACE_VERSION: u32 = 1;
/// Additive revision within v1 (`Info.interface_minor`).
pub const INTERFACE_MINOR: u32 = 0;
/// Zenoh version pinned in `interface/ZENOH_VERSION`. Crates that depend on `zenoh`
/// check their pin against this (see the `zenoh_pin` test in those crates).
pub const ZENOH_VERSION: &str = env!("REMOTE_JOY_ZENOH_VERSION");

/// Bit mask for a button index, e.g. `button_bit(v1::Button::Lb)`.
pub const fn button_bit(b: v1::Button) -> u32 {
    1 << (b as u32)
}
