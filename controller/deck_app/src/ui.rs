//! The Drive screen (docs/controller.md § Screens). Reads DeckCore::snapshot() every frame; the
//! only things it sets are the touch STOP and the panel. Video (M2), status (M3) and actions (M4)
//! have placeholders.

use std::sync::Arc;
use std::time::Duration;

use deck_core::health::Overlay;
use deck_core::DeckCore;
use eframe::egui::{self, Align2, Color32, FontId, RichText, Stroke};

const RED: Color32 = Color32::from_rgb(220, 40, 40);
const AMBER: Color32 = Color32::from_rgb(240, 160, 0);
const GREEN: Color32 = Color32::from_rgb(60, 200, 90);
const GREY: Color32 = Color32::from_rgb(140, 140, 140);

pub struct App {
    core: Arc<DeckCore>,
    debug: bool,
    last_logged: String,
    /// The touch STOP fires once per press, not on every frame while held.
    stop_down: bool,
}

impl App {
    pub fn new(core: Arc<DeckCore>) -> Self {
        App { core, debug: false, last_logged: String::new(), stop_down: false }
    }
}

fn battery() -> Option<u8> {
    for b in ["BAT1", "BAT0"] {
        if let Ok(s) = std::fs::read_to_string(format!("/sys/class/power_supply/{b}/capacity")) {
            return s.trim().parse().ok();
        }
    }
    None
}

fn hint_name(bit: u32) -> &'static str {
    const N: [&str; 21] = [
        "A", "B", "X", "Y", "View", "Steam", "Menu", "L3", "R3", "LB", "RB", "Up", "Down", "Left", "Right", "QAM", "P1",
        "P2", "P3", "P4", "Pad",
    ];
    N.get(bit as usize).copied().unwrap_or("?")
}

/// (top-bar text, colour, banner text if any, banner colour)
fn describe(o: &Overlay, robot: &str) -> (String, Color32, Option<String>, Color32) {
    match o {
        Overlay::NoController => ("NO CONTROLLER".into(), GREY, Some("No controller: launch from Steam (Game Mode)".into()), GREY),
        Overlay::Connecting => ("● CONNECTING".into(), AMBER, None, AMBER),
        Overlay::WrongRobot(m) => ("● WRONG ROBOT".into(), RED, Some(format!("Not the paired robot: {m}")), RED),
        Overlay::Version(v) => ("● UPDATE THE APP".into(), RED, Some(format!("The robot speaks interface v{v}; update the app")), RED),
        Overlay::LinkLost => ("● LINK LOST".into(), RED, Some("LINK LOST: reconnecting…".into()), RED),
        Overlay::Busy(o) => ("● BUSY".into(), AMBER, Some(format!("Another Deck is driving {robot} ({o:08x})")), AMBER),
        Overlay::Stopped { reason, local } => {
            let why = if *local { "STOP pressed".to_string() } else { reason.replace('_', " ") };
            ("● CONNECTED".into(), GREEN, Some(format!("STOP (teleop): {why}   ·   hold View + Menu 1 s to clear STOP on {robot}")), RED)
        }
        Overlay::Rearm => ("● CONNECTED".into(), GREEN, Some("REARM: release L2, then press it again to resume".into()), AMBER),
        Overlay::NoControl => ("● CONNECTED".into(), GREEN, Some("No control: hold L2 to drive".into()), GREY),
        Overlay::Ok => ("● CONNECTED".into(), GREEN, None, GREEN),
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let s = self.core.snapshot();
        let overlay = s.overlay.clone().unwrap_or(Overlay::Connecting);
        let (status, status_col, banner, banner_col) = describe(&overlay, &s.robot_name);
        let log_line = format!("{status} | {}", banner.clone().unwrap_or_default());
        if log_line != self.last_logged {
            println!("{:.3} UI {log_line}", self.core.now_us() as f64 / 1e6);
            self.last_logged = log_line;
        }

        // ── top bar ──
        egui::Panel::top("top").exact_size(44.0).show(ui, |ui| {
            let r = ui.horizontal_centered(|ui| {
                ui.label(RichText::new(&status).color(status_col).size(22.0).strong());
                ui.add_space(12.0);
                ui.label(RichText::new(&s.robot_name).size(22.0).strong());
                if let Some(rtt) = &s.rtt {
                    ui.add_space(12.0);
                    ui.label(RichText::new(format!("RTT {:.0} ms", rtt.avg_ms)).size(18.0));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(b) = battery() {
                        ui.label(RichText::new(format!("⏻ {b}%")).size(18.0));
                    }
                });
            });
            if r.response.interact(egui::Sense::click()).clicked() {
                self.debug = !self.debug;
            }
        });

        // ── panel (Menu) ──
        if s.panel_open {
            egui::Panel::right("panel").exact_size(380.0).show(ui, |ui| {
                ui.heading("Robot");
                ui.label(format!("{}   session {:08x}", s.robot_name, s.session_id));
                ui.separator();
                ui.heading("Status");
                ui.label(RichText::new("Robot status arrives with M3").color(GREY));
                ui.separator();
                ui.heading("Actions");
                ui.label(RichText::new("Robot actions arrive with M4").color(GREY));
                ui.separator();
                ui.label("Driving input is neutral while this panel is open.");
                ui.add_space(20.0);
                if ui.add_sized([340.0, 56.0], egui::Button::new(RichText::new("Close panel").size(22.0))).clicked() {
                    self.core.set_panel_open(false);
                }
                ui.add_space(10.0);
                if ui.add_sized([340.0, 56.0], egui::Button::new(RichText::new("Quit (stops the robot)").size(22.0))).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        }

        // ── drive area ──
        egui::CentralPanel::default().show(ui, |ui| {
            let rect = ui.max_rect();
            let p = ui.painter();
            p.rect_filled(rect, 0.0, Color32::from_rgb(18, 18, 22));
            p.text(rect.center(), Align2::CENTER_CENTER, "camera arrives with M2", FontId::proportional(28.0), Color32::from_gray(70));

            let stopped = matches!(overlay, Overlay::Stopped { .. });
            if stopped {
                p.rect_stroke(rect.shrink(3.0), 0.0, Stroke::new(6.0, RED), egui::StrokeKind::Inside);
            }
            if let Some(text) = &banner {
                let band = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 12.0), egui::vec2(rect.width(), 56.0));
                p.rect_filled(band, 0.0, banner_col.gamma_multiply(0.85));
                p.text(band.center(), Align2::CENTER_CENTER, text, FontId::proportional(24.0), Color32::WHITE);
            }

            // Clear combo progress.
            if s.clear_progress > 0.0 || s.clear_blocked.is_some() {
                let bar = egui::Rect::from_center_size(rect.center() + egui::vec2(0.0, 60.0), egui::vec2(420.0, 36.0));
                p.rect_filled(bar, 6.0, Color32::from_gray(40));
                let mut fill = bar;
                fill.set_width(bar.width() * s.clear_progress);
                p.rect_filled(fill, 6.0, GREEN);
                let label = s.clear_blocked.map(|b| format!("clear STOP: {b}")).unwrap_or_else(|| "clearing STOP…".into());
                p.text(bar.center(), Align2::CENTER_CENTER, label, FontId::proportional(20.0), Color32::WHITE);
            }

            // Notices (clear results etc.), newest last.
            for (i, (_, n)) in s.notices.iter().rev().take(3).enumerate() {
                p.text(
                    rect.center() + egui::vec2(0.0, 120.0 + i as f32 * 30.0),
                    Align2::CENTER_CENTER,
                    n,
                    FontId::proportional(22.0),
                    Color32::from_gray(230),
                );
            }

            // Bottom-left: deadman / turbo, hints, help.
            let gated = !matches!(overlay, Overlay::Ok) || s.panel_open;
            let dot = |on: bool| if on { "●" } else { "○" };
            let base = rect.left_bottom() + egui::vec2(16.0, -16.0);
            p.text(base + egui::vec2(0.0, -64.0), Align2::LEFT_BOTTOM,
                format!("DEADMAN (L2) {}   TURBO (RB) {}", dot(s.deadman), dot(s.turbo && s.deadman)),
                FontId::proportional(22.0), if s.deadman { GREEN } else { Color32::from_gray(200) });
            let hints: Vec<String> = s.button_hints.iter().map(|h| format!("{} {}", hint_name(h.button), h.label)).collect();
            if !hints.is_empty() {
                p.text(base + egui::vec2(0.0, -34.0), Align2::LEFT_BOTTOM, hints.join(" · "), FontId::proportional(20.0),
                    if gated { Color32::from_gray(90) } else { Color32::from_gray(220) });
            }
            p.text(base, Align2::LEFT_BOTTOM, "B = STOP · View+Menu 1 s = clear · Menu = panel",
                FontId::proportional(18.0), Color32::from_gray(150));

            if self.debug {
                let d = format!(
                    "session {:08x}   controls sent {}\nRTT {}\npad: {}\naxes {:+.2} {:+.2} {:+.2} {:+.2} {:.2} {:.2}  buttons {:#07x}",
                    s.session_id,
                    s.controls_sent,
                    s.rtt.as_ref().map(|r| format!("last {:.1} / min {:.1} / avg {:.1} ms ({} samples)", r.last_ms, r.min_ms, r.avg_ms, r.samples)).unwrap_or("-".into()),
                    if s.has_pad { &s.pad_name } else { "none" },
                    s.axes[0], s.axes[1], s.axes[2], s.axes[3], s.axes[4], s.axes[5], s.buttons,
                );
                p.text(rect.left_top() + egui::vec2(16.0, 90.0), Align2::LEFT_TOP, d, FontId::monospace(16.0), Color32::from_gray(220));
            }

            // Touch STOP: always there, on every screen.
            let stop_rect = egui::Rect::from_min_size(rect.right_bottom() - egui::vec2(236.0, 136.0), egui::vec2(220.0, 120.0));
            let resp = ui.put(stop_rect, egui::Button::new(RichText::new("STOP").size(44.0).strong().color(Color32::WHITE)).fill(RED));
            let down = resp.is_pointer_button_down_on();
            if down && !self.stop_down {
                self.core.press_touch_stop(); // on touch-down: don't wait for release
            }
            self.stop_down = down;
        });

        ctx.request_repaint_after(Duration::from_millis(16));
    }
}
