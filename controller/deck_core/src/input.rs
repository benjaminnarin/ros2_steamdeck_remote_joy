//! Gamepad snapshot -> what the Deck sends, plus the Deck-side combos. Pure: time is passed in.
//! Spec: docs/controller.md § Controls, docs/interface.md § Axis and button indices.

use deck_msgs::v1::Button;

pub const fn bit(b: Button) -> u32 {
    1 << (b as u32)
}

pub const B: u32 = bit(Button::B);
pub const VIEW: u32 = bit(Button::View);
pub const MENU: u32 = bit(Button::Menu);

/// Raw values as gilrs reports them: sticks right = +X, up = +Y; triggers 0..1.
/// `buttons` already uses the joy bit order (bit n = Button n).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RawPad {
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
    pub lt: f32,
    pub rt: f32,
    pub buttons: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct InputConfig {
    /// Radial deadzone; the Deck's sticks rest at up to ±0.08 (M0).
    pub stick_deadzone: f32,
    /// View + Menu must be held this long to request clear_stop.
    pub clear_hold_us: u64,
    /// Sticks count as centered for the clear combo within this (after the deadzone).
    pub clear_stick_tolerance: f32,
    /// The deadman (left trigger) counts as released for the clear combo below this.
    pub deadman_release: f32,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig { stick_deadzone: 0.12, clear_hold_us: 1_000_000, clear_stick_tolerance: 0.1, deadman_release: 0.3 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// B or the touchscreen STOP: latch locally and send Stop.
    Stop(&'static str),
    /// View + Menu held long enough, sticks centered, deadman released.
    ClearRequested,
    /// Menu tapped (released without View).
    TogglePanel,
}

/// One processed input sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    /// Wire order and signs: LX, LY, RX, RY (+1 = left/up), LT, RT (0..1).
    pub axes: [f32; 6],
    pub buttons: u32,
    pub actions: Vec<Action>,
    /// 0..1 while View + Menu are held (for the UI).
    pub clear_progress: f32,
    /// Why a held combo isn't counting (for the UI), e.g. "center the sticks".
    pub clear_blocked: Option<&'static str>,
    /// The pad as read (wire order and signs), before the panel neutralizes `axes`/`buttons`:
    /// for the Menu screen's input view, never sent.
    pub pad_axes: [f32; 6],
    pub pad_buttons: u32,
}

#[derive(Debug, Default)]
pub struct InputState {
    cfg: InputConfig,
    prev_buttons: u32,
    combo_since: Option<u64>,
    combo_fired: bool,
    /// Menu is down and View hasn't been pressed during this press.
    menu_tap_possible: bool,
}

/// Radial deadzone with rescaling, so output starts at 0 just outside the deadzone.
pub fn deadzone(x: f32, y: f32, dz: f32) -> (f32, f32) {
    let (x, y) = (finite(x), finite(y));
    let m = (x * x + y * y).sqrt();
    if m <= dz {
        return (0.0, 0.0);
    }
    let scale = ((m - dz) / (1.0 - dz)).min(1.0) / m;
    ((x * scale).clamp(-1.0, 1.0), (y * scale).clamp(-1.0, 1.0))
}

fn finite(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

impl InputState {
    pub fn new(cfg: InputConfig) -> Self {
        InputState { cfg, ..Default::default() }
    }

    /// `touch_stop`: the on-screen STOP was pressed since the last call.
    /// `panel_open`: driving input goes out neutral while the panel is open.
    pub fn process(&mut self, raw: RawPad, now_us: u64, touch_stop: bool, panel_open: bool) -> Frame {
        let dz = self.cfg.stick_deadzone;
        let (lx, ly) = deadzone(raw.lx, raw.ly, dz);
        let (rx, ry) = deadzone(raw.rx, raw.ry, dz);
        let lt = finite(raw.lt).clamp(0.0, 1.0);
        let rt = finite(raw.rt).clamp(0.0, 1.0);
        // Wire: +1 = left, so negate gilrs X (right = +). Y is already up = +.
        let axes = [-lx, ly, -rx, ry, lt, rt];

        let mut f = Frame { axes, buttons: raw.buttons, pad_axes: axes, pad_buttons: raw.buttons, ..Default::default() };
        let pressed = raw.buttons & !self.prev_buttons;
        let released = self.prev_buttons & !raw.buttons;
        self.prev_buttons = raw.buttons;

        if pressed & B != 0 {
            f.actions.push(Action::Stop("button_b"));
        }
        if touch_stop {
            f.actions.push(Action::Stop("touch"));
        }

        // Menu tap vs. the View + Menu combo.
        if pressed & MENU != 0 {
            self.menu_tap_possible = raw.buttons & VIEW == 0;
        }
        if raw.buttons & VIEW != 0 {
            self.menu_tap_possible = false;
        }
        if released & MENU != 0 && self.menu_tap_possible {
            f.actions.push(Action::TogglePanel);
            self.menu_tap_possible = false;
        }

        // Clear combo: View + Menu held clear_hold, sticks centered, deadman released.
        let combo = raw.buttons & (VIEW | MENU) == VIEW | MENU;
        if combo {
            let since = *self.combo_since.get_or_insert(now_us);
            f.clear_progress = ((now_us - since) as f32 / self.cfg.clear_hold_us as f32).min(1.0);
            let tol = self.cfg.clear_stick_tolerance;
            if axes[..4].iter().any(|v| v.abs() > tol) {
                f.clear_blocked = Some("center the sticks");
            } else if lt >= self.cfg.deadman_release {
                f.clear_blocked = Some("release L2");
            }
            if f.clear_blocked.is_some() {
                self.combo_since = Some(now_us); // restart the hold once it's valid
                f.clear_progress = 0.0;
            } else if now_us - since >= self.cfg.clear_hold_us && !self.combo_fired {
                self.combo_fired = true;
                f.actions.push(Action::ClearRequested);
            }
        } else {
            self.combo_since = None;
            self.combo_fired = false;
        }

        if panel_open {
            f.axes = [0.0; 6];
            f.buttons = 0;
        }
        f
    }
}

/// The Deck's local stop latch and the race-free clear handshake (docs/interface.md § Rules):
/// stop setting Control.stop, then send clear_stop with released_seq = the first seq sent with
/// stop = false (the gateway ignores older in-flight stops). A refusal re-latches.
#[derive(Debug, Default)]
pub struct StopLatch {
    latched: bool,
    pending: Option<u32>,
}

impl StopLatch {
    pub fn latched(&self) -> bool {
        self.latched
    }

    pub fn pending_clear(&self) -> Option<u32> {
        self.pending
    }

    pub fn on_stop(&mut self) {
        self.latched = true;
        self.pending = None; // a STOP while a clear is in flight wins
    }

    /// Returns the released_seq to send, or None if a clear is already in flight.
    pub fn on_clear_request(&mut self, next_seq: u32) -> Option<u32> {
        if self.pending.is_some() {
            return None;
        }
        self.latched = false;
        self.pending = Some(next_seq);
        Some(next_seq)
    }

    /// Reply for the clear with `released_seq`. Stale replies (after a newer STOP) are ignored.
    pub fn on_clear_reply(&mut self, released_seq: u32, ok: bool) {
        if self.pending != Some(released_seq) {
            return;
        }
        self.pending = None;
        if !ok {
            self.latched = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LB: u32 = bit(Button::Lb);
    const S: u64 = 1_000_000;

    fn pad(buttons: u32) -> RawPad {
        RawPad { buttons, ..Default::default() }
    }

    #[test]
    fn deadzone_is_radial_and_rescaled() {
        assert_eq!(deadzone(0.08, -0.05, 0.12), (0.0, 0.0), "Deck resting noise");
        let (x, y) = deadzone(0.12 + 0.44, 0.0, 0.12);
        assert!((x - 0.5).abs() < 1e-5 && y == 0.0, "halfway out of the deadzone -> 0.5, got {x}");
        let (x, _) = deadzone(1.0, 0.0, 0.12);
        assert!((x - 1.0).abs() < 1e-6);
        let (x, y) = deadzone(1.0, 1.0, 0.12);
        assert!(x <= 1.0 && y <= 1.0 && (x - y).abs() < 1e-6, "diagonal clamps, keeps direction");
        assert_eq!(deadzone(f32::NAN, 0.5, 0.12).0, 0.0);
    }

    #[test]
    fn x_axes_are_negated_triggers_clamped() {
        let mut s = InputState::default();
        let f = s.process(RawPad { lx: 1.0, ry: -1.0, lt: 1.5, rt: -0.2, ..Default::default() }, 0, false, false);
        assert_eq!(f.axes, [-1.0, 0.0, 0.0, -1.0, 1.0, 0.0], "right -> -1 (wire: +1 = left)");
        let f = s.process(RawPad { ly: 1.0, rx: -1.0, ..Default::default() }, 1, false, false);
        assert_eq!(f.axes, [0.0, 1.0, 1.0, 0.0, 0.0, 0.0], "up stays +1, left -> +1");
    }

    #[test]
    fn b_press_and_touch_stop() {
        let mut s = InputState::default();
        assert_eq!(s.process(pad(B), 0, false, false).actions, vec![Action::Stop("button_b")]);
        assert!(s.process(pad(B), 10, false, false).actions.is_empty(), "edge only");
        assert_eq!(s.process(pad(0), 20, true, false).actions, vec![Action::Stop("touch")]);
    }

    #[test]
    fn menu_tap_toggles_panel_but_combo_does_not() {
        let mut s = InputState::default();
        assert!(s.process(pad(MENU), 0, false, false).actions.is_empty(), "on release, not press");
        assert_eq!(s.process(pad(0), 100, false, false).actions, vec![Action::TogglePanel]);
        // View joins while Menu is held: it's the combo, not a tap.
        s.process(pad(MENU), 200, false, false);
        s.process(pad(MENU | VIEW), 300, false, false);
        s.process(pad(MENU), 400, false, false);
        assert!(s.process(pad(0), 500, false, false).actions.is_empty());
        // Menu pressed while View is already held.
        s.process(pad(VIEW), 600, false, false);
        s.process(pad(VIEW | MENU), 700, false, false);
        assert!(s.process(pad(0), 800, false, false).actions.is_empty());
    }

    #[test]
    fn clear_combo_needs_a_full_second_once() {
        let mut s = InputState::default();
        let mut cleared = 0;
        for i in 0..=60 {
            let f = s.process(pad(VIEW | MENU), i * 20_000, false, false);
            cleared += f.actions.iter().filter(|a| **a == Action::ClearRequested).count();
            if i == 25 {
                assert!((f.clear_progress - 0.5).abs() < 0.01);
            }
        }
        assert_eq!(cleared, 1, "fires once per hold");
        s.process(pad(0), 2 * S, false, false);
        let f = s.process(pad(VIEW | MENU), 2 * S + 1, false, false);
        assert!(f.actions.is_empty());
        let f = s.process(pad(VIEW | MENU), 3 * S + 2, false, false);
        assert_eq!(f.actions, vec![Action::ClearRequested], "a new hold fires again");
    }

    #[test]
    fn clear_combo_blocked_by_sticks_or_deadman_and_restarts() {
        let mut s = InputState::default();
        let f = s.process(RawPad { ly: 0.5, buttons: VIEW | MENU, ..Default::default() }, 0, false, false);
        assert_eq!(f.clear_blocked, Some("center the sticks"));
        let f = s.process(RawPad { lt: 0.6, buttons: VIEW | MENU, ..Default::default() }, S, false, false);
        assert_eq!(f.clear_blocked, Some("release L2"));
        let f = s.process(pad(VIEW | MENU), S + 10, false, false);
        assert!(f.actions.is_empty(), "the hold restarts once valid");
        let f = s.process(pad(VIEW | MENU), 2 * S + 20, false, false);
        assert_eq!(f.actions, vec![Action::ClearRequested]);
        // Resting stick noise inside the deadzone is fine.
        let mut s = InputState::default();
        s.process(RawPad { lx: 0.08, ry: -0.07, buttons: VIEW | MENU, ..Default::default() }, 0, false, false);
        let f = s.process(RawPad { lx: 0.08, ry: -0.07, buttons: VIEW | MENU, ..Default::default() }, S, false, false);
        assert_eq!(f.actions, vec![Action::ClearRequested]);
    }

    #[test]
    fn panel_open_sends_neutral_but_still_stops() {
        let mut s = InputState::default();
        let f = s.process(RawPad { ly: 1.0, lt: 1.0, buttons: LB | B, ..Default::default() }, 0, false, true);
        assert_eq!(f.axes, [0.0; 6]);
        assert_eq!(f.buttons, 0);
        assert_eq!(f.actions, vec![Action::Stop("button_b")], "STOP works on every screen");
        assert_eq!(f.pad_buttons, LB | B, "the Menu screen still sees the pad");
        assert_eq!(f.pad_axes[1], 1.0);
        assert_eq!(f.pad_axes[4], 1.0);
    }

    #[test]
    fn stop_latch_handshake() {
        let mut l = StopLatch::default();
        l.on_stop();
        assert!(l.latched());
        assert_eq!(l.on_clear_request(41), Some(41));
        assert!(!l.latched(), "step 1: stop sending stop=true right away");
        assert_eq!(l.on_clear_request(42), None, "one clear in flight");
        l.on_clear_reply(41, false);
        assert!(l.latched(), "step 4: refused -> latch again");

        assert_eq!(l.on_clear_request(50), Some(50));
        l.on_stop(); // pressed STOP while the clear was in flight
        l.on_clear_reply(50, true);
        assert!(l.latched(), "a late OK can't undo a newer STOP");

        assert_eq!(l.on_clear_request(60), Some(60));
        l.on_clear_reply(60, true);
        assert!(!l.latched());
        assert_eq!(l.pending_clear(), None);
    }
}
