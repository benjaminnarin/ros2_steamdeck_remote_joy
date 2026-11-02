//! The Drive screen (docs/controller.md § Screens). Reads DeckCore::snapshot() every frame; the
//! only things it sets are the touch STOP and the panel. Status (M3) and actions (M4) have
//! placeholders.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Second strip with RTT, fps, video level, hints and help (the stats button).
    stats: bool,
    /// The development Inputs screen, opened from the Menu screen.
    inputs: bool,
    last_logged: String,
    /// The touch STOP fires once per press, not on every frame while held.
    stop_down: bool,
    texture: Option<egui::TextureHandle>,
    shown_generation: u64,
    /// `--screenshot DIR` (development): steps through the screens and saves each one.
    shot: Option<Shot>,
}

/// Screens saved by `--screenshot`, in order: (file name, stats strip, Menu screen, Inputs screen).
const SHOTS: [(&str, bool, bool, bool); 4] =
    [("drive", false, false, false), ("drive_stats", true, false, false), ("menu", false, true, false), ("inputs", false, true, true)];
/// Frames to wait on each screen before saving it (video and layout settle).
const SHOT_FRAMES: u32 = 90;

struct Shot {
    dir: PathBuf,
    step: usize,
    frames: u32,
    /// Set by the screenshot callback once the PNG is written.
    saved: Arc<AtomicBool>,
    requested: bool,
}

impl App {
    pub fn new(core: Arc<DeckCore>, screenshot: Option<PathBuf>) -> Self {
        let shot = screenshot.map(|dir| Shot { dir, step: 0, frames: 0, saved: Arc::new(AtomicBool::new(false)), requested: false });
        App { core, debug: false, stats: false, inputs: false, last_logged: String::new(), stop_down: false, texture: None,
              shown_generation: 0, shot }
    }

    /// `--screenshot`: set up the current step's screen, save it after SHOT_FRAMES, move on, exit.
    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.shot else { return };
        if shot.saved.swap(false, Ordering::SeqCst) {
            shot.step += 1;
            shot.frames = 0;
            shot.requested = false;
        }
        let Some(&(name, stats, menu, inputs)) = SHOTS.get(shot.step) else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        self.stats = stats;
        self.core.set_panel_open(menu);
        self.inputs = inputs;
        shot.frames += 1;
        if shot.frames >= SHOT_FRAMES && !shot.requested {
            shot.requested = true;
            let path = shot.dir.join(format!("{name}.png"));
            let saved = shot.saved.clone();
            let ppp = ctx.pixels_per_point();
            let rect = ctx.content_rect();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                match save_png(&path, &img) {
                    Ok(()) => println!("screenshot {}: {}x{} px, {:.0}x{:.0} points, pixels_per_point {ppp:.2}",
                        path.display(), img.size[0], img.size[1], rect.width(), rect.height()),
                    Err(e) => eprintln!("screenshot {}: {e}", path.display()),
                }
                saved.store(true, Ordering::SeqCst);
            })));
        }
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

/// Every screen is laid out for 1280x800 points: the Deck's panel at a scale of 1. SteamOS's
/// gamescope reports a scale of 2.17 to the app, which made everything 2.17x too big.
const DESIGN_W: f32 = 1280.0;

/// Zoom so the window is DESIGN_W points wide (on the Deck: 1 point = 1 pixel).
fn fit_design_width(ctx: &egui::Context) {
    let px = ctx.content_rect().width() * ctx.pixels_per_point();
    let target = (px / DESIGN_W).max(0.25);
    if (ctx.pixels_per_point() - target).abs() > 0.01 {
        ctx.set_zoom_factor(target / ctx.native_pixels_per_point().unwrap_or(1.0));
    }
}

/// Overlay strips over the video: semi-transparent black.
const STRIP: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 150);
const TOP_H: f32 = 40.0;
const STATS_H: f32 = 34.0;
const BANNER_H: f32 = 52.0;
/// Touch STOP, bottom right over the video.
const STOP_SIZE: egui::Vec2 = egui::vec2(170.0, 64.0);

/// (top-strip status text, colour, banner text if any, banner colour)
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
        fit_design_width(&ctx);
        self.screenshot_step(&ctx);
        let s = self.core.snapshot();
        let overlay = s.overlay.clone().unwrap_or(Overlay::Connecting);
        let (status, status_col, banner, banner_col) = describe(&overlay, &s.robot_name);
        let log_line = format!("{status} | {}", banner.clone().unwrap_or_default());
        if log_line != self.last_logged {
            println!("{:.3} UI {log_line}", self.core.now_us() as f64 / 1e6);
            self.last_logged = log_line;
        }

        // New video frame -> texture (once per decoded frame, not per draw).
        if let Some(f) = self.core.video().latest_if_newer(self.shown_generation) {
            let image = egui::ColorImage::from_rgb([f.width, f.height], &f.rgb);
            match &mut self.texture {
                Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture("video", image, egui::TextureOptions::LINEAR)),
            }
            self.shown_generation = f.generation;
            self.core.video().mark_shown(f.frame_id);
        }

        // Inputs screen (development, from the Menu screen): its own screen, nothing from the
        // Drive screen. The panel stays open, so driving input is still neutral; B still stops.
        if !s.panel_open {
            self.inputs = false;
        }
        if self.inputs {
            egui::CentralPanel::default().show(ui, |ui| self.inputs_screen(ui, &s));
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        }

        // ── drive area: the video as large as the screen allows (fit, never cropped), with
        // semi-transparent strips over it: status on top, stats when toggled, banners below. ──
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let rect = ui.max_rect();
            let p = ui.painter().clone();
            p.rect_filled(rect, 0.0, Color32::from_rgb(10, 10, 12));
            match &self.texture {
                Some(t) => {
                    let ts = t.size_vec2();
                    let scale = (rect.width() / ts.x).min(rect.height() / ts.y);
                    let img = egui::Rect::from_center_size(rect.center(), ts * scale);
                    let tint = if s.video_live { Color32::WHITE } else { Color32::from_gray(90) };
                    p.image(t.id(), img, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
                    if !s.video_live {
                        p.text(rect.center(), Align2::CENTER_CENTER, "NO VIDEO", FontId::proportional(56.0), RED);
                    } else if s.video.level > 0 {
                        // The robot lowered the video rate for a weak link (docs/controller.md § Video).
                        let text = "weak link: reduced video";
                        let at = egui::pos2(rect.left() + 12.0, rect.bottom() - 12.0);
                        let size = p.layout_no_wrap(text.into(), FontId::proportional(18.0), Color32::WHITE).size();
                        p.rect_filled(egui::Rect::from_min_size(at - egui::vec2(0.0, size.y), size).expand(5.0), 4.0, STRIP);
                        p.text(at, Align2::LEFT_BOTTOM, text, FontId::proportional(18.0), Color32::from_rgb(230, 180, 60));
                    }
                }
                None => {
                    let msg = match &s.camera {
                        Some(c) if !c.available => "this robot has no camera",
                        _ => "waiting for video…",
                    };
                    p.text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(28.0), Color32::from_gray(90));
                }
            }

            // Top strip: what matters while driving. Tap the status to toggle the debug overlay.
            let top = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), TOP_H));
            p.rect_filled(top, 0.0, STRIP);
            let y = top.center().y;
            let mut x = top.left() + 12.0;
            let mut put = |text: String, size: f32, col: Color32, gap: f32| {
                let r = p.text(egui::pos2(x, y), Align2::LEFT_CENTER, text, FontId::proportional(size), col);
                x = r.right() + gap;
                r
            };
            let st = put(status.clone(), 20.0, status_col, 10.0);
            let name = put(s.robot_name.clone(), 20.0, Color32::WHITE, 24.0);
            if ui.interact(st.union(name), ui.make_persistent_id("status_tap"), egui::Sense::click()).clicked() {
                self.debug = !self.debug;
            }
            let dot = |on: bool| if on { "●" } else { "○" };
            let col = |on: bool| if on { GREEN } else { Color32::from_gray(200) };
            let turbo = s.turbo && s.deadman;
            put(format!("DEADMAN {}", dot(s.deadman)), 18.0, col(s.deadman), 14.0);
            put(format!("TURBO {}", dot(turbo)), 18.0, col(turbo), 14.0);
            let btn = egui::Rect::from_min_size(egui::pos2(top.right() - 96.0, top.top() + 4.0), egui::vec2(88.0, TOP_H - 8.0));
            let label = if self.stats { "stats ▴" } else { "stats ▾" };
            if ui.put(btn, egui::Button::new(RichText::new(label).size(17.0)).fill(Color32::from_white_alpha(28))).clicked() {
                self.stats = !self.stats;
            }
            if let Some(b) = battery() {
                // Only when it fits between TURBO and the stats button.
                let text = format!("Deck ⏻ {b}%");
                let w = p.layout_no_wrap(text.clone(), FontId::proportional(17.0), Color32::WHITE).size().x;
                if btn.left() - 12.0 - w > x {
                    p.text(egui::pos2(btn.left() - 12.0, y), Align2::RIGHT_CENTER, text, FontId::proportional(17.0),
                        Color32::from_gray(220));
                }
            }

            // Stats strip (toggled): link numbers, then robot button hints and the help.
            let mut below = top.bottom();
            if self.stats {
                let strip = egui::Rect::from_min_size(egui::pos2(rect.left(), below), egui::vec2(rect.width(), STATS_H));
                p.rect_filled(strip, 0.0, STRIP);
                let mut parts = Vec::new();
                parts.push(s.rtt.as_ref().map(|r| format!("RTT {:.0} ms", r.avg_ms)).unwrap_or("RTT –".into()));
                parts.push(if s.video_live { format!("video {:.0} fps", s.video.fps) } else { "no video".into() });
                if s.video.level > 0 {
                    parts.push(format!("video level {}", s.video.level));
                }
                let gated = !matches!(overlay, Overlay::Ok) || s.panel_open;
                let hints: Vec<String> = s.button_hints.iter().map(|h| format!("{} {}", hint_name(h.button), h.label)).collect();
                let r = p.text(egui::pos2(rect.left() + 12.0, strip.center().y), Align2::LEFT_CENTER, parts.join("   "),
                    FontId::proportional(17.0), Color32::from_gray(230));
                let r = if hints.is_empty() { r } else {
                    p.text(egui::pos2(r.right() + 24.0, strip.center().y), Align2::LEFT_CENTER, hints.join(" · "),
                        FontId::proportional(17.0), if gated { Color32::from_gray(110) } else { Color32::from_gray(230) })
                };
                let help = "B STOP · View+Menu 1 s clear · Menu panel";
                let help_at = egui::pos2(strip.right() - 12.0, strip.center().y);
                // Only when it fits after the hints; never overlapping them.
                let w = p.layout_no_wrap(help.into(), FontId::proportional(15.0), Color32::WHITE).size().x;
                if help_at.x - w > r.right() + 24.0 {
                    p.text(help_at, Align2::RIGHT_CENTER, help, FontId::proportional(15.0), Color32::from_gray(160));
                }
                below = strip.bottom();
            }

            // Menu screen (Menu button): a card over the video with status, Inputs (dev), Close and
            // Quit. Drawn before STOP so STOP stays on top and clear of it.
            if s.panel_open {
                // Below the strips and any banner, never taller than what's left.
                let card_top = below + if banner.is_some() { BANNER_H + 16.0 } else { 12.0 };
                let card = egui::Rect::from_min_size(egui::pos2(rect.center().x - MENU_W / 2.0, card_top),
                    egui::vec2(MENU_W, (rect.bottom() - 12.0 - card_top).min(MENU_H)));
                self.menu_screen(ui, card, &s, &status, status_col, &ctx);
            }

            // Touch STOP: always there, on every screen, bottom right over the video.
            let stop_rect = egui::Rect::from_min_size(rect.right_bottom() - STOP_SIZE - egui::vec2(12.0, 12.0), STOP_SIZE);
            let resp = ui.put(stop_rect, egui::Button::new(RichText::new("STOP").size(30.0).strong().color(Color32::WHITE))
                .fill(Color32::from_rgba_unmultiplied(220, 40, 40, 200)));
            let down = resp.is_pointer_button_down_on();
            if down && !self.stop_down {
                self.core.press_touch_stop(); // on touch-down: don't wait for release
            }
            self.stop_down = down;

            let stopped = matches!(overlay, Overlay::Stopped { .. });
            if stopped {
                p.rect_stroke(rect.shrink(3.0), 0.0, Stroke::new(6.0, RED), egui::StrokeKind::Inside);
            }
            if let Some(text) = &banner {
                let band = egui::Rect::from_min_size(egui::pos2(rect.left(), below + 8.0), egui::vec2(rect.width(), BANNER_H));
                p.rect_filled(band, 0.0, banner_col.gamma_multiply(0.7));
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

            if self.debug {
                let v = &s.video;
                let ms = |o: Option<f64>| o.map(|x| format!("{x:.0} ms")).unwrap_or("-".into());
                let d = format!(
                    "capture→encoded  {:>6}     frames received {:>5.1} %\n\
                     encoded→received {:>6}     fps             {:>5.1}\n\
                     received→decoded {:>6}     frame size      {:>5.1} KB\n\
                     decoded→shown    {:>6}     control RTT     {}\n\
                     capture→shown    {:>6}     clock offset    {}\n\
                     session {:08x}  controls sent {}  pad: {}  video level {}\n\
                     axes {:+.2} {:+.2} {:+.2} {:+.2} {:.2} {:.2}  buttons {:#07x}  dropped big {} bad {}",
                    format!("{:.0} ms", v.encode_ms), v.delivered_pct,
                    ms(v.transfer_ms), v.fps,
                    format!("{:.0} ms", v.decode_ms), v.avg_kb,
                    ms(v.show_ms),
                    s.rtt.as_ref().map(|r| format!("{:.0} ms (min {:.0})", r.avg_ms, r.min_ms)).unwrap_or("-".into()),
                    ms(v.capture_to_shown_ms),
                    s.clock_offset_us.map(|o| format!("{:+.3} s", o as f64 / 1e6)).unwrap_or("-".into()),
                    s.session_id, s.controls_sent, if s.has_pad { &s.pad_name } else { "none" }, v.level,
                    s.axes[0], s.axes[1], s.axes[2], s.axes[3], s.axes[4], s.axes[5], s.buttons,
                    v.dropped_too_big, v.decode_errors,
                );
                let bg = egui::Rect::from_min_size(egui::pos2(rect.left() + 8.0, below + 70.0), egui::vec2(760.0, 170.0));
                p.rect_filled(bg, 6.0, Color32::from_black_alpha(180));
                p.text(bg.min + egui::vec2(8.0, 10.0), Align2::LEFT_TOP, d, FontId::monospace(16.0), Color32::from_gray(220));
            }

        });

        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

const MENU_W: f32 = 560.0;
/// Buttons the app reads from the Steam Virtual Gamepad: bits 0-14 (deck_app/src/pad.rs).
const READ_BUTTONS: u32 = 15;
const MENU_H: f32 = 470.0;

fn button_label(bit: u32) -> String {
    match bit {
        9 => "L1 (LB)".into(),
        10 => "R1 (RB)".into(),
        11..=14 => format!("D-pad {}", hint_name(bit).to_lowercase()),
        15 => "Quick Access".into(),
        16..=19 => format!("Grip {}", hint_name(bit)),
        20 => "Touchpad click".into(),
        _ => hint_name(bit).into(),
    }
}

const AXIS_NAMES: [&str; 6] = ["Left stick X", "Left stick Y", "Right stick X", "Right stick Y", "L2 trigger", "R2 trigger"];

impl App {
    fn menu_screen(&mut self, ui: &mut egui::Ui, card: egui::Rect, s: &deck_core::Snapshot, status: &str,
                   status_col: Color32, ctx: &egui::Context) {
        ui.painter().rect_filled(card, 12.0, Color32::from_rgba_premultiplied(8, 10, 14, 230));
        ui.scope_builder(egui::UiBuilder::new().max_rect(card.shrink(20.0)), |ui| egui::ScrollArea::vertical().show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(10.0, 6.0);
            ui.label(RichText::new("Menu").size(24.0).strong());
            ui.label(RichText::new("Driving input is neutral while this is open. STOP still works.").size(15.0).color(GREY));
            ui.add_space(4.0);
            egui::Grid::new("status").num_columns(2).spacing([18.0, 4.0]).show(ui, |ui| {
                let row = |ui: &mut egui::Ui, k: &str, v: RichText| {
                    ui.label(RichText::new(k).size(16.0).color(GREY));
                    ui.label(v.size(16.0));
                    ui.end_row();
                };
                row(ui, "connection", RichText::new(status).color(status_col));
                row(ui, "robot", RichText::new(&s.robot_name));
                row(ui, "session", RichText::new(format!("{:08x}", s.session_id)));
                row(ui, "round trip", RichText::new(s.rtt.as_ref()
                    .map(|r| format!("{:.0} ms (min {:.0})", r.avg_ms, r.min_ms)).unwrap_or("–".into())));
                let v = &s.video;
                row(ui, "video", RichText::new(if s.video_live {
                    format!("{:.0} fps · {:.0} % received · level {}", v.fps, v.delivered_pct, v.level)
                } else { "none".into() }));
                row(ui, "Deck battery", RichText::new(battery().map(|b| format!("{b} %")).unwrap_or("–".into())));
                row(ui, "robot status", RichText::new("arrives with M3").color(GREY));
                row(ui, "actions", RichText::new("arrive with M4").color(GREY));
            });
            ui.add_space(10.0);
            let w = card.width() - 40.0;
            let button = |ui: &mut egui::Ui, text: &str| {
                ui.add_sized([w, 50.0], egui::Button::new(RichText::new(text).size(20.0))).clicked()
            };
            if button(ui, "Inputs (development)") {
                self.inputs = true;
            }
            if button(ui, "Close (Menu)") {
                self.core.set_panel_open(false);
            }
            if button(ui, "Quit (stops the robot)") {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }));
    }

    /// Development: every gamepad input as read, with its index and name. Three columns so it
    /// fits 1280x800; scrolls if the display scale makes it bigger.
    fn inputs_screen(&mut self, ui: &mut egui::Ui, s: &deck_core::Snapshot) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Inputs").size(24.0).strong());
            ui.add_space(12.0);
            ui.label(RichText::new(format!(
                "controller: {}   ·   display scale {:.2}   ·   driving neutral, B = STOP",
                if s.has_pad { s.pad_name.as_str() } else { "none" }, ui.ctx().pixels_per_point()))
                .size(15.0).color(GREY));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_sized([140.0, 44.0], egui::Button::new(RichText::new("Back").size(20.0))).clicked() {
                    self.inputs = false;
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            let col_w = (ui.available_width() - 40.0) / 3.0;
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(col_w);
                    ui.label(RichText::new("Axes").size(18.0).strong());
                    egui::Grid::new("axes").num_columns(3).spacing([10.0, 8.0]).show(ui, |ui| {
                        for (i, name) in AXIS_NAMES.iter().enumerate() {
                            ui.label(RichText::new(format!("{i}")).size(15.0).color(GREY));
                            ui.label(RichText::new(*name).size(16.0));
                            axis_bar(ui, s.pad_axes[i], i >= 4, i == 0 || i == 2);
                            ui.end_row();
                        }
                    });
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("buttons {:#08x}", s.pad_buttons)).monospace().size(15.0).color(GREY));
                    ui.add_space(12.0);
                    ui.label(RichText::new(
                        "Buttons 15-20 (Quick Access, back grips L4/L5/R4/R5, touchpad click) don't exist on the \
                         Steam Virtual Gamepad. To use a back grip, map it to another button in the app's Steam \
                         Input layout; it then shows as that button.").size(14.0).color(GREY));
                });
                for range in [0..11u32, 11..21u32] {
                    ui.add_space(20.0);
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        ui.label(RichText::new("Buttons").size(18.0).strong());
                        egui::Grid::new(("buttons", range.start)).num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
                            for bit in range.clone() {
                                let held = s.pad_buttons & (1 << bit) != 0;
                                ui.label(RichText::new(format!("{bit}")).size(15.0).color(GREY));
                                if bit >= READ_BUTTONS {
                                    // The Steam Virtual Gamepad has no such button (pad.rs reads 0-14).
                                    ui.label(RichText::new(button_label(bit)).size(16.0).color(Color32::from_gray(90)));
                                    ui.label(RichText::new("not read").size(14.0).color(Color32::from_gray(90)));
                                } else {
                                    ui.label(RichText::new(button_label(bit)).size(16.0).strong());
                                    ui.label(RichText::new(if held { "● held" } else { "○" }).size(16.0)
                                        .color(if held { GREEN } else { Color32::from_gray(90) }));
                                }
                                ui.end_row();
                            }
                        });
                    });
                }
            });
        });
    }
}

/// A small live gauge: sticks -1..1 from the centre, triggers 0..1 from the left. On the wire
/// +1 is left (X axes) or up (Y axes); X fills towards the side the stick points, Y fills right
/// for up.
fn axis_bar(ui: &mut egui::Ui, v: f32, trigger: bool, x_axis: bool) {
    let (area, _) = ui.allocate_exact_size(egui::vec2(150.0 + 64.0, 16.0), egui::Sense::hover());
    let r = egui::Rect::from_min_size(area.min, egui::vec2(150.0, area.height()));
    let p = ui.painter();
    p.rect_filled(r, 3.0, Color32::from_gray(45));
    let fill = if trigger {
        egui::Rect::from_min_size(r.min, egui::vec2(r.width() * v.clamp(0.0, 1.0), r.height()))
    } else {
        let c = r.center().x;
        let dir = if x_axis { -1.0 } else { 1.0 };
        let x = c + dir * v.clamp(-1.0, 1.0) * r.width() / 2.0;
        egui::Rect::from_x_y_ranges(c.min(x)..=c.max(x), r.y_range())
    };
    p.rect_filled(fill, 3.0, if v.abs() > 0.01 { GREEN } else { Color32::from_gray(80) });
    p.text(egui::pos2(r.right() + 8.0, r.center().y), Align2::LEFT_CENTER, format!("{v:+.2}"),
        FontId::monospace(14.0), Color32::from_gray(210));
}

fn save_png(path: &Path, img: &egui::ColorImage) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, img.size[0] as u32, img.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    enc.write_header().and_then(|mut w| w.write_image_data(&rgba)).map_err(std::io::Error::other)
}
