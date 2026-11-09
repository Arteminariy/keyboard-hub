//! `type84-gui` (Keyboard Hub): desktop configurator for the IO by Red
//! Square Type 84 Magnetic keyboard.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use type84_gui::model::BoardState;
use type84_gui::{headless, i18n, theme, App};

const USAGE: &str = "\
usage: type84-gui [--from-backup <file>] [--headless-check]

  --from-backup <file>  view a `type84 backup` JSON file offline (no HID access)
  --headless-check      read the board without a window, print the Hall state,
                        compare it with docs/GROUND_TRUTH.md and exit
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut backup: Option<PathBuf> = None;
    let mut headless_check = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--from-backup" => match args.next() {
                Some(p) => backup = Some(PathBuf::from(p)),
                None => {
                    eprint!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--headless-check" => headless_check = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    if headless_check {
        return ExitCode::from(headless::run(Duration::from_secs(30)));
    }

    let offline = match &backup {
        Some(p) => match BoardState::load_backup(p) {
            Ok(st) => Some(st),
            Err(e) => {
                eprintln!("error: {e:#}");
                return ExitCode::from(2);
            }
        },
        None => None,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(i18n::s().app_title)
            .with_app_id("keyboard-hub")
            .with_icon(std::sync::Arc::new(type84_gui::icon::window_icon()))
            .with_inner_size([1360.0, 980.0])
            .with_min_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    let r = eframe::run_native(
        i18n::s().app_title,
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let app = match offline {
                Some(st) => {
                    let mut a = App::from_backup(st, backup);
                    if let Ok(p) = type84_gui::paths::Paths::user() {
                        // Presets can be saved from a backup; writes stay off.
                        a.paths = p;
                    }
                    a
                }
                None => App::live(&cc.egui_ctx),
            };
            Ok(Box::new(app))
        }),
    );
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
