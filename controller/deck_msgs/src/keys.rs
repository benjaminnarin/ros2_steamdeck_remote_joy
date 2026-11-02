//! Zenoh key expressions and encodings. Must match
//! `interface/include/remote_joy_interface/keys.hpp` and docs/interface.md § Key space.

pub const PREFIX: &str = "rj/v1";
pub const CONTROL: &str = "rj/v1/control";
pub const STOP: &str = "rj/v1/stop";
pub const ROBOT_STATE: &str = "rj/v1/robot_state";
pub const STATUS: &str = "rj/v1/status";
pub const VIDEO: &str = "rj/v1/video";
pub const VIDEO_FEEDBACK: &str = "rj/v1/video_feedback";
pub const INFO: &str = "rj/v1/info";
pub const CLEAR_STOP: &str = "rj/v1/clear_stop";
pub const ACTION_PREFIX: &str = "rj/v1/action";
pub const DECK_PREFIX: &str = "rj/v1/deck";

/// `rj/v1/action/<id>`
pub fn action(id: &str) -> String {
    format!("{ACTION_PREFIX}/{id}")
}

/// `rj/v1/deck/<session_id as 8 hex digits>`: the Deck's liveliness token.
pub fn deck_token(session_id: u32) -> String {
    format!("{DECK_PREFIX}/{session_id:08x}")
}

/// Zenoh payload encoding: `application/protobuf;remote_joy.v1.<Message>`.
pub fn encoding(message: &str) -> String {
    format!("application/protobuf;remote_joy.v1.{message}")
}
