//! Wire-format pinning against protoc's encodings in interface/test/golden/ (see its README).
//! Each case decodes the protoc-produced bytes, checks the fields, and re-encodes to the same bytes.

use deck_msgs::v1::{self, robot_state, status_item, video_frame, Axis, Button};
use deck_msgs::{button_bit, keys, Message};
use std::path::PathBuf;

fn golden(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../interface/test/golden").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e} (run interface/test/golden/regen.sh)", p.display()))
}

fn roundtrip<M: Message + Default + PartialEq + std::fmt::Debug>(name: &str) -> M {
    let bytes = golden(&format!("{name}.bin"));
    let m = M::decode(bytes.as_slice()).expect("decode");
    assert_eq!(m.encode_to_vec(), bytes, "{name}: Rust encoding differs from protoc");
    m
}

#[test]
fn control() {
    let c: v1::Control = roundtrip("control");
    assert_eq!(c.session_id, 0xdec0_ffee);
    assert_eq!(c.seq, 42);
    assert_eq!(c.deck_time_us, 123_456_789_012);
    assert_eq!(c.axes, vec![0.5, 1.0, 0.0, -0.25, 0.0, 1.0]);
    assert_eq!(c.axes[Axis::LeftY as usize], 1.0);
    assert_eq!(c.buttons, button_bit(Button::Lb) | button_bit(Button::Rb));
    assert!(!c.stop);
}

#[test]
fn robot_state() {
    let s: v1::RobotState = roundtrip("robot_state");
    assert_eq!(s.state(), robot_state::State::Rearm);
    assert_eq!(s.owner_session, 0xdec0_ffee);
    assert_eq!(s.echo_seq, 42);
    assert_eq!(s.echo_deck_time_us, 123_456_789_012);
    assert_eq!(s.stop_reason, "link_lost");
}

#[test]
fn clear_stop_request() {
    let r: v1::ClearStopRequest = roundtrip("clear_stop_request");
    assert_eq!((r.session_id, r.released_seq), (0xdec0_ffee, 7));
}

#[test]
fn info() {
    let i: v1::Info = roundtrip("info");
    assert_eq!(i.robot_name, "mybot");
    assert_eq!((i.interface_version, i.interface_minor), (1, 0));
    assert_eq!(i.actions.len(), 2);
    assert!(i.actions[0].confirm && i.actions[0].moves_robot);
    assert!(!i.actions[1].moves_robot);
    let cam = i.camera.expect("camera");
    assert_eq!((cam.available, cam.width, cam.height, cam.max_fps), (true, 480, 270, 30));
    assert_eq!(i.robot_id, vec![1, 2, 3, 0xfe]);
    assert_eq!(i.button_hints[1].button, Button::X as u32);
    assert_eq!(i.button_hints[1].label, "Lights");
}

#[test]
fn status() {
    let s: v1::Status = roundtrip("status");
    assert_eq!(s.items.len(), 3);
    assert_eq!(s.items[0].value, Some(status_item::Value::Number(72.5)));
    assert!(s.items[0].pinned);
    assert_eq!(s.items[1].value, Some(status_item::Value::Text("Overcurrent".into())));
    assert_eq!(s.items[1].level(), status_item::Level::Error);
    // A oneof set to its default value is still present on the wire.
    assert_eq!(s.items[2].value, Some(status_item::Value::Flag(false)));
    assert_eq!(s.items[2].level(), status_item::Level::Stale);
}

#[test]
fn video_frame() {
    let f: v1::VideoFrame = roundtrip("video_frame");
    assert_eq!(f.codec(), video_frame::Codec::Jpeg);
    assert_eq!((f.width, f.height, f.encode_done_offset_us), (480, 270, 4500));
    assert_eq!(f.data, vec![0xff, 0xd8, 0xff, 0xd9]);
    assert_eq!(f.level, 3);
}

#[test]
fn video_feedback() {
    let f: v1::VideoFeedback = roundtrip("video_feedback");
    assert_eq!(f.session_id, 0xdec0_ffee);
    assert_eq!((f.last_frame_id, f.last_frame_age_us, f.frames_received), (4242, 35_000, 4100));
}

#[test]
fn indices_match_joy_order() {
    assert_eq!(Axis::RightTrigger as i32, 5);
    assert_eq!(Button::B as i32, 1);
    assert_eq!(Button::Menu as i32, 6);
    assert_eq!(Button::Lb as i32, 9);
    assert_eq!(Button::Touchpad as i32, 20);
}

#[test]
fn key_strings() {
    assert_eq!(keys::CONTROL, "rj/v1/control");
    assert_eq!(keys::action("dock"), "rj/v1/action/dock");
    assert_eq!(keys::deck_token(0xdec0ffee), "rj/v1/deck/dec0ffee");
    assert_eq!(keys::encoding("Control"), "application/protobuf;remote_joy.v1.Control");
    assert_eq!(deck_msgs::ZENOH_VERSION, "1.10.2");
}

/// The C++ header and this crate must list the same keys.
#[test]
fn keys_match_cpp_header() {
    let hpp = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../interface/include/remote_joy_interface/keys.hpp"),
    )
    .unwrap();
    for k in [
        keys::CONTROL,
        keys::STOP,
        keys::ROBOT_STATE,
        keys::STATUS,
        keys::VIDEO,
        keys::VIDEO_FEEDBACK,
        keys::INFO,
        keys::CLEAR_STOP,
        keys::ACTION_PREFIX,
        keys::DECK_PREFIX,
    ] {
        assert!(hpp.contains(&format!("\"{k}\"")), "{k} missing from keys.hpp");
    }
}
