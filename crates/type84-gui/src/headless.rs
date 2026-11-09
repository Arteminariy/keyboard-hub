//! `--headless-check`: read the board through the same worker the window
//! uses, print the Hall state and compare it with docs/GROUND_TRUTH.md.

use std::time::{Duration, Instant};

use crate::model::BoardState;
use crate::worker::{Command, Event, Worker};

/// Keys set to 0.80 mm + RT 0.30/0.30 + whole travel in the reference state.
pub const GT_GAME_KEYS: [(&str, u8); 7] = [
    ("W", 34),
    ("A", 49),
    ("S", 50),
    ("D", 51),
    ("LShift", 64),
    ("LCtrl", 80),
    ("Space", 83),
];

/// Compare a state with the reference. Returns report lines and whether
/// everything matched.
pub fn compare_ground_truth(st: &BoardState) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut ok = true;
    for (name, i) in GT_GAME_KEYS {
        let k = st.keys[&i];
        let good = k.actuation == 80 && k.rt && k.press == 30 && k.release == 30 && k.whole_travel;
        ok &= good;
        lines.push(format!(
            "{name:<7} act {:.2} rt {} press {:.2} release {:.2} whole {}  {}",
            k.actuation as f32 / 100.0,
            k.rt,
            k.press as f32 / 100.0,
            k.release as f32 / 100.0,
            k.whole_travel,
            if good { "OK" } else { "MISMATCH" }
        ));
    }
    let game: Vec<u8> = GT_GAME_KEYS.iter().map(|(_, i)| *i).collect();
    let others: Vec<_> = st.keys.iter().filter(|(i, _)| !game.contains(i)).collect();
    let bad: Vec<String> = others
        .iter()
        .filter(|(_, k)| k.actuation != 150 || k.rt)
        .map(|(i, k)| {
            format!(
                "{}={:.2}{}",
                type84_proto::layout::label(**i as usize).unwrap_or("?"),
                k.actuation as f32 / 100.0,
                if k.rt { "+RT" } else { "" }
            )
        })
        .collect();
    ok &= bad.is_empty();
    lines.push(format!(
        "other {} keys: 1.50 mm, RT off  {}",
        others.len(),
        if bad.is_empty() {
            "OK".to_string()
        } else {
            format!("MISMATCH: {}", bad.join(" "))
        }
    ));
    let s = st.settings;
    let board_ok =
        s.report_rate_hz == Some(8000) && s.auto_calibration && s.stability && !s.dead_zones;
    ok &= board_ok;
    lines.push(format!(
        "board: {} Hz, auto-calibration {}, stability {}, dead zones {}  {}",
        s.report_rate_hz
            .map(|h| h.to_string())
            .unwrap_or("?".into()),
        s.auto_calibration,
        s.stability,
        s.dead_zones,
        if board_ok { "OK" } else { "MISMATCH" }
    ));
    for p in &st.socd {
        let (a, b) = p.names();
        lines.push(format!(
            "SOCD on layer 0: {a} + {b}, mode {} (keys {:?})",
            p.mode, p.keys
        ));
    }
    (lines, ok)
}

/// Run the check. Exit code 0 = read OK and matches, 1 = read OK but
/// differs, 2 = read failed.
pub fn run(timeout: Duration) -> u8 {
    let paths = match crate::paths::Paths::user() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e:#}");
            return 2;
        }
    };
    let w = Worker::spawn(None, paths);
    w.send(Command::ReadAll);
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let Some(e) = w.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        match e {
            Event::Presence(Some(p)) => println!("found {} at {}", p.product, p.path),
            Event::Presence(None) => println!("board not present"),
            Event::Reading => println!("reading full state (read-only)…"),
            Event::HallBlock(_) | Event::PollSkipped => {}
            Event::Plan(_)
            | Event::PlanFailed { .. }
            | Event::Applied { .. }
            | Event::ApplyFailed { .. }
            | Event::BackupSaved(_) => {}
            Event::Error { context, detail } => {
                eprintln!("error ({context:?}): {detail}");
                return 2;
            }
            Event::State(st) => {
                println!("{} firmware {}", st.product, st.firmware);
                let (lines, ok) = compare_ground_truth(&st);
                for l in lines {
                    println!("  {l}");
                }
                println!(
                    "RESULT: {}",
                    if ok {
                        "matches GROUND_TRUTH.md"
                    } else {
                        "differs from GROUND_TRUTH.md"
                    }
                );
                return if ok { 0 } else { 1 };
            }
        }
    }
    eprintln!("error: timed out after {timeout:?}");
    2
}
