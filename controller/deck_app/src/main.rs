//! steamdeck-remote-joy: drive a ROS 2 robot from a Steam Deck. docs/controller.md § CLI.

mod pad;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use deck_core::events::{self, EventLog};
use deck_core::profiles::{self, Store};
use deck_core::DeckCore;

/// "0.1.0 (9988d30 2026-11-02)": the package version and the git commit it was built from
/// (`-dirty` when built with uncommitted changes); see build.rs.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("DECK_BUILD"), ")");

#[derive(Parser)]
#[command(name = "steamdeck-remote-joy", version = VERSION, about = "Drive a ROS 2 robot from a Steam Deck")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Profiles directory (default ~/.config/steamdeck-remote-joy).
    #[arg(long, global = true)]
    config_dir: Option<PathBuf>,
    /// Event log (default ~/.local/state/steamdeck-remote-joy/events.log).
    #[arg(long, global = true)]
    log_file: Option<PathBuf>,
    /// Window instead of fullscreen (development).
    #[arg(long)]
    windowed: bool,
    /// Development: show each screen (Drive, Drive with stats, Menu, Inputs) for 1.5 s, save it
    /// as <DIR>/<screen>.png at the window's real size and scale, print the scale, then exit.
    #[arg(long, value_name = "DIR")]
    screenshot: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Manage paired robots.
    Robot {
        #[command(subcommand)]
        cmd: RobotCmd,
    },
    /// Connect, print Info and the robot's state, then exit (sends no Control).
    Probe {
        #[arg(long, default_value_t = 3.0)]
        seconds: f64,
    },
    /// Measure the link headless: neutral Control at 50 Hz (sticks centred, no deadman, so the
    /// robot stays STOPPED; it takes ownership if no one has it), video received but not shown.
    /// Prints one line per second, then a summary.
    Bench {
        #[arg(long, default_value_t = 20.0)]
        seconds: f64,
    },
    /// Show the local event log.
    Log {
        #[arg(long, default_value_t = 30)]
        tail: usize,
    },
}

#[derive(Subcommand)]
enum RobotCmd {
    /// Install a bundle from `ros2 run deck_gateway pair` and make it the active robot.
    Add {
        #[arg(long)]
        bundle: PathBuf,
    },
    List,
    /// Remove a robot by name or id prefix.
    Forget { name: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = Store::new(cli.config_dir.clone().unwrap_or_else(profiles::default_root));
    let log = Arc::new(EventLog::new(cli.log_file.clone().unwrap_or_else(events::default_path)));
    match cli.cmd {
        Some(Cmd::Robot { cmd: RobotCmd::Add { bundle } }) => {
            let i = store.add_bundle(&bundle)?;
            println!("added {} (id {}…), active; endpoints {:?}", i.profile.name, &i.profile.id[..16], i.profile.endpoints);
            println!("certificate {} for Deck {:?}. You can delete {} now.", i.profile.deck_cn, i.profile.deck_name, bundle.display());
        }
        Some(Cmd::Robot { cmd: RobotCmd::List }) => {
            let active = store.active().ok().map(|a| a.profile.id);
            for i in store.list() {
                let mark = if Some(&i.profile.id) == active.as_ref() { "*" } else { " " };
                println!("{mark} {:<16} {}…  {:?}  cert {}", i.profile.name, &i.profile.id[..16], i.profile.endpoints, i.profile.deck_cn);
            }
        }
        Some(Cmd::Robot { cmd: RobotCmd::Forget { name } }) => {
            let p = store.forget(&name)?;
            println!("forgot {} ({}…)", p.name, &p.id[..16]);
        }
        Some(Cmd::Probe { seconds }) => {
            let i = store.active()?;
            println!("probing {} at {:?}", i.profile.name, i.profile.endpoints);
            let r = deck_core::probe(&i, seconds)?;
            match &r.info {
                Some(info) => println!(
                    "Info: {} interface {}.{} (reply after {:.0} ms), robot id {}",
                    info.robot_name,
                    info.interface_version,
                    info.interface_minor,
                    r.info_ms,
                    if r.id_ok { "matches the profile" } else { "DOES NOT MATCH the profile" }
                ),
                None => println!("Info: no reply"),
            }
            println!("RobotState: {} in {seconds:.1} s ({:.1} Hz)", r.states, r.states as f64 / seconds);
            if let Some(s) = r.last_state {
                println!("  last: state {:?} owner {:08x} stop_reason {:?}", s.state(), s.owner_session, s.stop_reason);
            }
            if !(r.info.is_some() && r.id_ok) {
                std::process::exit(1);
            }
        }
        Some(Cmd::Bench { seconds }) => bench(&store, log, seconds)?,
        Some(Cmd::Log { tail }) => {
            for l in log.tail(tail) {
                println!("{l}");
            }
        }
        None => run_gui(&store, log, cli.windowed, cli.screenshot)?,
    }
    Ok(())
}

/// A gamepad that's connected and untouched.
struct NeutralPad;
impl deck_core::PadSource for NeutralPad {
    fn poll(&mut self) -> Option<(deck_core::input::RawPad, String)> {
        Some((Default::default(), "bench".into()))
    }
}

fn bench(store: &Store, log: Arc<EventLog>, seconds: f64) -> Result<()> {
    let profile = store.active()?;
    let core = DeckCore::start(profile, log, Box::new(|| Box::new(NeutralPad) as Box<dyn deck_core::PadSource>));
    let start = std::time::Instant::now();
    let mut rtts: Vec<f64> = Vec::new();
    let mut levels: Vec<u32> = Vec::new();
    let mut last_print = 0u64;
    while start.elapsed().as_secs_f64() < seconds {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let s = core.snapshot();
        // Ignore the first second: connecting.
        if start.elapsed().as_secs_f64() >= 1.0 {
            if let Some(r) = &s.rtt {
                rtts.push(r.last_ms);
            }
            levels.push(s.video.level);
        }
        let sec = start.elapsed().as_secs();
        if sec > last_print {
            last_print = sec;
            let v = &s.video;
            println!(
                "{sec:>3} s  rtt {:>7}  video {:>4.1} fps {:>5.1} % {:>5.1} KB  encoded→received {:>7}  level {}",
                s.rtt.as_ref().map(|r| format!("{:.0} ms", r.last_ms)).unwrap_or("-".into()),
                v.fps,
                v.delivered_pct,
                v.avg_kb,
                v.transfer_ms.map(|x| format!("{x:.0} ms")).unwrap_or("-".into()),
                v.level,
            );
        }
    }
    core.shutdown();
    rtts.sort_by(|a, b| a.total_cmp(b));
    let pct = |p: f64| rtts.get(((rtts.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(f64::NAN);
    let max_level = levels.iter().max().copied().unwrap_or(0);
    let final_level = levels.last().copied().unwrap_or(0);
    println!(
        "summary: rtt median {:.1} p95 {:.1} max {:.1} ms ({} samples), video level max {max_level} final {final_level}",
        pct(0.5),
        pct(0.95),
        pct(1.0),
        rtts.len()
    );
    Ok(())
}

fn run_gui(store: &Store, log: Arc<EventLog>, windowed: bool, screenshot: Option<PathBuf>) -> Result<()> {
    let profile = store.active()?;
    let core = DeckCore::start(profile, log, Box::new(|| Box::new(pad::GilrsPad::new()) as Box<dyn deck_core::PadSource>));
    println!("steamdeck-remote-joy: session {:08x}", core.session_id());
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Steam Deck Remote Joy")
        .with_inner_size([1280.0, 800.0])
        .with_fullscreen(!windowed);
    let c = core.clone();
    let result = eframe::run_native(
        "steamdeck-remote-joy",
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(move |_cc| Ok(Box::new(ui::App::new(c, screenshot)))),
    );
    core.shutdown(); // Stop, withdraw the token, close the session
    result.map_err(|e| anyhow::anyhow!("eframe: {e}"))
}
