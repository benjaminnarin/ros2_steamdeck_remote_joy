//! gilrs -> RawPad. Steam Input presents one "Steam Virtual Gamepad" with an SDL mapping (M0, O1).

use deck_core::input::RawPad;
use deck_core::PadSource;

/// Index = joy button bit (docs/interface.md § Axis and button indices).
const BUTTONS: [gilrs::Button; 15] = [
    gilrs::Button::South,        // 0 A
    gilrs::Button::East,         // 1 B
    gilrs::Button::West,         // 2 X
    gilrs::Button::North,        // 3 Y
    gilrs::Button::Select,       // 4 View
    gilrs::Button::Mode,         // 5 Steam
    gilrs::Button::Start,        // 6 Menu
    gilrs::Button::LeftThumb,    // 7 L3
    gilrs::Button::RightThumb,   // 8 R3
    gilrs::Button::LeftTrigger,  // 9 LB
    gilrs::Button::RightTrigger, // 10 RB
    gilrs::Button::DPadUp,       // 11
    gilrs::Button::DPadDown,     // 12
    gilrs::Button::DPadLeft,     // 13
    gilrs::Button::DPadRight,    // 14
];

pub struct GilrsPad {
    gilrs: Option<gilrs::Gilrs>,
    changed: bool,
}

impl GilrsPad {
    pub fn new() -> Self {
        let gilrs = match gilrs::Gilrs::new() {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("gilrs: {e}");
                None
            }
        };
        GilrsPad { gilrs, changed: false }
    }
}

impl PadSource for GilrsPad {
    fn poll(&mut self) -> Option<(RawPad, String)> {
        let g = self.gilrs.as_mut()?;
        while let Some(ev) = g.next_event() {
            if matches!(ev.event, gilrs::EventType::ButtonPressed(..) | gilrs::EventType::ButtonReleased(..)) {
                self.changed = true;
            }
        }
        let (_, p) = g.gamepads().next()?;
        use gilrs::{Axis, Button};
        let trig = |b| p.button_data(b).map(|d| d.value()).unwrap_or(0.0);
        let buttons = BUTTONS.iter().enumerate().filter(|(_, b)| p.is_pressed(**b)).fold(0u32, |acc, (i, _)| acc | 1 << i);
        Some((
            RawPad {
                lx: p.value(Axis::LeftStickX),
                ly: p.value(Axis::LeftStickY),
                rx: p.value(Axis::RightStickX),
                ry: p.value(Axis::RightStickY),
                lt: trig(Button::LeftTrigger2),
                rt: trig(Button::RightTrigger2),
                buttons,
            },
            p.name().to_string(),
        ))
    }

    fn changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
}
