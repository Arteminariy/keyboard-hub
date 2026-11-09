//! Write flows end to end against a simulated board (`type84_hid::mock`):
//! the real worker, the real preview code and the real guarded
//! `WriteSession::apply_plan` with pre-write backup and readback. Nothing
//! here can reach a real device, and every file goes to a temp dir.

use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::Vec2;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use type84_cli::apply::{self, Applied};
use type84_cli::backup::Backup;
use type84_gui::app::{Busy, Tab};
use type84_gui::backend::{ApplyFail, Backend};
use type84_gui::dialogs::{Dialog, SaveState};
use type84_gui::paths::Paths;
use type84_gui::worker::{Presence, Worker};
use type84_gui::{theme, App};
use type84_hid::mock::SimBoard;
use type84_hid::{Interface, ReadOnlyTransport, WriteSession, WriteStep};
use type84_proto::transfer::FrameIo;

const CURRENT: &str = "type84-20261109-220355-fw1.17.json";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn sim_from(name: &str) -> SimBoard {
    let b = Backup::load(&fixture(name)).unwrap();
    let bl = &b.blocks;
    let mut images = HashMap::new();
    for blk in [
        &bl.device_info,
        &bl.keymap_layer0,
        &bl.keymap_layer1,
        &bl.settings,
        &bl.led_effect,
        &bl.custom_led,
        &bl.macro_table,
        &bl.axis_rt,
        &bl.dks,
        &bl.default_fn_keymap,
    ] {
        images.insert(blk.opcode, blk.bytes().unwrap());
    }
    SimBoard::new(images)
}

fn iface() -> Interface {
    Interface {
        path: CString::new("sim").unwrap(),
        usage_page: 0xFF68,
        usage: 0x61,
        interface_number: 2,
        product: Some("IO Type 84 Magnetic Black".into()),
        manufacturer: None,
        serial: None,
        release: 0x0117,
    }
}

struct SimBackend(SimBoard);

impl Backend for SimBackend {
    fn scan(&mut self) -> Result<Option<Presence>, String> {
        Ok(Some(Presence {
            product: "IO Type 84 Magnetic Black".into(),
            path: "sim".into(),
        }))
    }
    fn open_read(&mut self) -> anyhow::Result<(Interface, Box<dyn FrameIo + '_>)> {
        Ok((iface(), Box::new(ReadOnlyTransport::new(self.0.open()))))
    }
    fn apply(
        &mut self,
        steps: &[WriteStep],
        backup_dir: &Path,
        state_dir: &Path,
    ) -> Result<Applied, ApplyFail> {
        let opt_in = type84_cli::plan::opt_in_for(steps)
            .ok_or_else(|| ApplyFail::before_write("nothing to write"))?;
        let mut s = WriteSession::new(self.0.open(), opt_in);
        apply::run_steps(&mut s, &iface(), steps, backup_dir, state_dir, false)
            .map_err(|e| ApplyFail::from_apply(&e))
    }
}

struct Rig {
    h: Harness<'static, App>,
    sim: SimBoard,
    paths: Paths,
    _dir: TempDir,
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rig(name: &str, test: &str) -> Rig {
    let dir = std::env::temp_dir().join(format!("keyboard-hub-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let paths = Paths::under(&dir);
    let sim = sim_from(name);
    let s2 = sim.clone();
    let worker = Worker::spawn_with(
        Box::new(move || Box::new(SimBackend(s2))),
        paths.clone(),
        None,
    );
    let mut app = App::with_worker(worker, paths.clone());
    app.open_device();
    let h = Harness::builder()
        .with_size(Vec2::new(1400.0, 1500.0))
        .with_pixels_per_point(1.0)
        .with_context_setup(theme::apply)
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    let mut r = Rig {
        h,
        sim,
        paths,
        _dir: TempDir(dir),
    };
    r.until("board read", |a| a.board.is_some() && !a.reading);
    r
}

impl Rig {
    fn until(&mut self, what: &str, f: impl Fn(&App) -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while !f(self.h.state()) {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            self.h.run_steps(1);
            std::thread::sleep(Duration::from_millis(10));
        }
        self.h.run_steps(2);
    }

    fn idle(&mut self) {
        self.until("idle", |a| a.busy.is_none() && !a.reading);
    }

    fn toast(&self, prefix: &str) -> Option<String> {
        self.h
            .state()
            .toasts
            .iter()
            .find(|t| t.text.starts_with(prefix))
            .map(|t| format!("{}\n{}", t.text, t.detail.clone().unwrap_or_default()))
    }

    fn confirm_open(&mut self) {
        self.until("confirm dialog", |a| {
            matches!(a.dialog, Some(Dialog::Confirm(_)))
        });
    }

    fn click_write(&mut self) {
        self.h.get_by_label("Записать").click();
        self.h.run_steps(2);
        self.until("write finished", |a| a.busy.is_none() && !a.reading);
    }
}

fn rec(block: &[u8], i: usize, len: usize) -> Vec<u8> {
    block[i * len..(i + 1) * len].to_vec()
}

#[test]
fn hall_edit_confirm_write_and_verify() {
    let mut r = rig(CURRENT, "hall");
    let before = r.sim.image(0x17);
    r.h.state_mut()
        .edits
        .as_mut()
        .unwrap()
        .edit_keys(&[0], |k| k.actuation = 100);
    r.h.run_steps(2);
    assert!(r.h.query_by_label("Несохранённые изменения: 1").is_some());
    r.h.get_by_label("Применить").click();
    r.h.run_steps(1);
    r.confirm_open();
    assert_eq!(
        r.sim.write_packets(),
        0,
        "nothing is written before «Записать»"
    );
    assert!(r.h.query_by_label("Записать в клавиатуру?").is_some());
    assert!(r.h.query_by_label("Esc").is_some());
    assert!(r.h.query_by_label("1,50 мм · RT выкл.").is_some());
    assert!(r.h.query_by_label("1,00 мм · RT выкл.").is_some());
    r.click_write();
    let ok = r.toast("Записано и проверено").expect("success toast");
    assert!(ok.contains("Отклик"), "{ok}");
    let after = r.sim.image(0x17);
    assert_eq!(after[2..4], 100u16.to_le_bytes(), "Esc actuation written");
    assert_eq!(after[8..], before[8..], "only Esc changed");
    assert_eq!(
        r.sim.write_opcodes().iter().filter(|&&o| o == 0x27).count(),
        19
    );
    // The pre-write backup is in the temp dir and named in the toast.
    let backup = std::fs::read_dir(&r.paths.auto_backup_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(ok.contains(&backup.file_name().unwrap().to_string_lossy().to_string()));
    Backup::load(&backup).unwrap();
    // Re-read after the write: no pending edits, no case-switch banner.
    assert_eq!(r.h.state().pending(), 0);
    assert!(!r.h.state().hall_changed);
}

#[test]
fn cancel_writes_nothing() {
    let mut r = rig(CURRENT, "cancel");
    r.h.state_mut()
        .edits
        .as_mut()
        .unwrap()
        .edit_keys(&[0], |k| k.actuation = 100);
    r.h.run_steps(2);
    r.h.get_by_label("Применить").click();
    r.confirm_open();
    r.h.get_by_label("Отмена").click();
    r.h.run_steps(3);
    assert!(r.h.state().dialog.is_none());
    assert_eq!(r.sim.write_packets(), 0);
    assert_eq!(r.h.state().pending(), 1, "edits stay pending");
}

#[test]
fn fn_layer_pick_and_locked_keys() {
    let mut r = rig(CURRENT, "fn");
    r.h.state_mut().tab = Tab::Keymap;
    r.h.state_mut().keymap.key = Some(106); // Fn+Delete = vendor reset
    r.h.run_steps(3);
    assert!(r
        .h
        .query_by_label_contains("закреплена за прошивкой")
        .is_some());
    assert!(
        r.h.query_by_label("F13").is_none(),
        "no picker on a locked key"
    );
    r.h.state_mut().keymap.key = Some(85);
    r.h.run_steps(3);
    assert!(r.h.query_by_label_contains("Это клавиша Fn").is_some());

    r.h.state_mut().keymap.key = Some(1); // Fn+F1, unassigned
    r.h.run_steps(3);
    r.h.get_by_label("F13").click();
    r.h.run_steps(2);
    assert_eq!(r.h.state().pending(), 1);
    r.h.get_by_label("Применить").click();
    r.confirm_open();
    assert!(r.h.query_by_label("Fn+F1").is_some());
    assert!(r.h.query_by_label("клавиша F13").is_some());
    r.click_write();
    assert!(r.toast("Записано и проверено").unwrap().contains("Fn-слой"));
    let fnl = r.sim.image(0x16);
    assert_eq!(rec(&fnl, 1, 4), vec![2, 0, 0x68, 0]);
    assert_eq!(
        rec(&fnl, 106, 4),
        vec![0x0D, 0, 0, 1],
        "vendor function kept"
    );
    assert_eq!(rec(&fnl, 85, 4), vec![2, 0, 0xAF, 0], "Fn key kept");
}

#[test]
fn lighting_paint_custom_and_record() {
    let mut r = rig(CURRENT, "light");
    r.h.state_mut().tab = Tab::Lighting;
    {
        let a = r.h.state_mut();
        a.edits.as_mut().unwrap().light.mode = 128;
        a.lighting.selection = [34u8, 49].into();
        a.lighting.brush = [0xFF, 0x6A, 0x00];
    }
    r.h.run_steps(3);
    r.h.get_by_label("Покрасить выбранные").click();
    r.h.run_steps(2);
    assert_eq!(r.h.state().pending(), 3, "effect + two keys");
    r.h.get_by_label("Применить").click();
    r.confirm_open();
    assert!(r.h.query_all_by_label("Пользовательский").count() >= 1);
    assert!(
        r.h.query_by_label("Расходящаяся рябь").is_some(),
        "effect row: before"
    );
    r.click_write();
    let ok = r.toast("Записано и проверено").unwrap();
    assert!(ok.contains("проверьте его на глаз"), "{ok}");
    assert_eq!(r.sim.image(0x13)[0], 20, "custom reads back as 20");
    let c = r.sim.image(0x14);
    assert_eq!(rec(&c, 34, 4)[1..], [0xFF, 0x6A, 0x00]);
    assert_eq!(rec(&c, 49, 4)[1..], [0xFF, 0x6A, 0x00]);
    // The colour record went to the temp state dir, not the user's.
    assert!(r.paths.state_dir.join("lighting-last.json").exists());
    // Lighting tab of the re-read state shows the painted keys, no pending.
    assert_eq!(r.h.state().pending(), 0);
}

#[test]
fn colour_unknown_note_and_known_after_write() {
    let mut r = rig(CURRENT, "colour");
    r.h.state_mut().tab = Tab::Lighting;
    r.h.run_steps(3);
    assert!(r
        .h
        .query_by_label_contains("сведений о последней записи нет")
        .is_some());
    {
        let e = r.h.state_mut().edits.as_mut().unwrap();
        e.light.color = [0x22, 0xD0, 0x7C];
        e.light.multi = false;
    }
    r.h.run_steps(2);
    r.h.get_by_label("Применить").click();
    r.confirm_open();
    r.click_write();
    r.h.run_steps(3);
    // The record is fresh now: the colour is known and shown.
    let e = r.h.state().edits.as_ref().unwrap();
    assert_eq!(e.light.color, [0x22, 0xD0, 0x7C]);
    assert!(!e.light.multi);
    assert!(r
        .h
        .query_by_label_contains("Показан цвет из последней записи")
        .is_some());
}

#[test]
fn board_changed_after_preview_is_refused_with_backup_path() {
    let mut r = rig(CURRENT, "basechanged");
    r.h.state_mut()
        .edits
        .as_mut()
        .unwrap()
        .edit_keys(&[0], |k| k.actuation = 100);
    r.h.run_steps(2);
    r.h.get_by_label("Применить").click();
    r.confirm_open();
    // The case switch rewrites the table while the dialog is open.
    let mut hall = r.sim.image(0x17);
    hall[34 * 8 + 2] = 200;
    r.sim.set_image(0x17, hall);
    r.click_write();
    let t = r.toast("Запись не выполнена").expect("refusal toast");
    assert!(t.contains("Резервная копия до записи"), "{t}");
    assert!(
        t.contains(&r.paths.auto_backup_dir.display().to_string()) || t.contains("prewrite"),
        "{t}"
    );
    assert_eq!(r.sim.write_packets(), 0);
}

#[test]
fn external_writer_lock_blocks_poll_and_write() {
    let mut r = rig(CURRENT, "lock");
    let lock = apply::write_lock(&r.paths.state_dir).unwrap();
    r.h.state_mut().request_hall_poll();
    r.until("poll skipped", |a| a.poll_skips == 1);
    r.h.state_mut()
        .edits
        .as_mut()
        .unwrap()
        .edit_keys(&[0], |k| k.actuation = 100);
    r.h.run_steps(2);
    r.h.get_by_label("Применить").click();
    r.h.run_steps(2);
    assert_eq!(r.h.state().busy, Some(Busy::Preview));
    r.idle();
    let t = r
        .toast("Не удалось подготовить запись")
        .expect("lock refusal");
    assert!(t.contains("another type84 process"), "{t}");
    assert_eq!(r.sim.write_packets(), 0);
    drop(lock);
    let skips = r.h.state().poll_skips;
    r.h.state_mut().request_hall_poll();
    r.h.run_steps(5);
    std::thread::sleep(Duration::from_millis(200));
    r.h.run_steps(5);
    assert_eq!(
        r.h.state().poll_skips,
        skips,
        "polls resume once the lock is free"
    );
}

#[test]
fn restore_from_backup_previews_and_writes() {
    let mut r = rig(CURRENT, "restore");
    // A backup whose Hall table differs from the board (Esc at 0.70 mm).
    let mut backup = Backup::load(&fixture(CURRENT)).unwrap();
    let mut hall = backup.blocks.axis_rt.bytes().unwrap();
    hall[2..4].copy_from_slice(&70u16.to_le_bytes());
    backup.blocks.axis_rt.data = hex::encode(&hall);
    let file = r.paths.backup_dir.join("old.json");
    std::fs::create_dir_all(&r.paths.backup_dir).unwrap();
    std::fs::write(&file, serde_json::to_string(&backup).unwrap()).unwrap();
    r.h.state_mut().open_restore();
    r.h.run_steps(2);
    r.h.state_mut().restore_load(file.clone());
    r.h.run_steps(2);
    assert!(r.h.query_by_label_contains("old.json").is_some());
    r.h.get_by_label("Показать изменения").click();
    r.confirm_open();
    let rows = match &r.h.state().dialog {
        Some(Dialog::Confirm(p)) => p.model.rows.len(),
        _ => unreachable!(),
    };
    assert!(rows > 0);
    assert!(r.h.query_by_label("0,70 мм · RT выкл.").is_some());
    r.click_write();
    assert_eq!(r.sim.image(0x17), hall);
}

#[test]
fn banner_restore_reapplies_table_from_before_the_switch() {
    let mut r = rig(CURRENT, "banner");
    let original = r.sim.image(0x17);
    let mut switched = original.clone();
    for k in type84_proto::layout::keys() {
        let o = k.index as usize * 8;
        switched[o + 2..o + 4].copy_from_slice(&200u16.to_le_bytes());
        switched[o + 4..o + 8].fill(0);
    }
    r.sim.set_image(0x17, switched);
    r.h.state_mut().request_hall_poll();
    r.until("banner", |a| a.hall_changed);
    r.h.get_by_label("Восстановить").click();
    r.h.run_steps(3);
    assert!(r
        .h
        .query_by_label_contains("Как было до переключения")
        .is_some());
    r.h.get_by_label("Показать изменения").click();
    r.confirm_open();
    r.click_write();
    assert_eq!(r.sim.image(0x17), original);
    assert!(!r.h.state().hall_changed);
}

#[test]
fn save_list_apply_and_delete_presets() {
    let mut r = rig(CURRENT, "presets");
    r.h.state_mut().dialog = Some(Dialog::SavePreset(SaveState {
        name: "mine".into(),
        hall: true,
        fn_layer: true,
        lighting: false,
        overwrite: false,
        error: None,
    }));
    r.h.run_steps(2);
    r.h.get_by_label("Сохранить").click();
    r.h.run_steps(3);
    let file = r.paths.presets_dir.join("mine.json");
    assert!(file.exists(), "{:?}", r.h.state().toasts);
    type84_cli::preset::Preset::load(&file).unwrap();
    // Invalid (non-ASCII) names are refused inline.
    r.h.state_mut().dialog = Some(Dialog::SavePreset(SaveState {
        name: "мой".into(),
        hall: true,
        fn_layer: false,
        lighting: false,
        overwrite: false,
        error: None,
    }));
    r.h.run_steps(2);
    assert!(r
        .h
        .query_by_label_contains("Только латинские буквы")
        .is_some());
    r.h.state_mut().dialog = None;

    r.h.state_mut().open_presets();
    r.h.run_steps(2);
    assert!(r.h.query_by_label("mine").is_some());
    assert!(r
        .h
        .query_by_label_contains("совпадает с клавиатурой")
        .is_some());
    // The board already holds it: applying writes nothing.
    r.h.get_by_label("Применить").click();
    r.h.run_steps(1);
    r.idle();
    assert!(r.toast("Записывать нечего").is_some());
    assert_eq!(r.sim.write_packets(), 0);

    r.h.state_mut().open_presets();
    r.h.run_steps(2);
    r.h.get_by_label("Удалить").click();
    r.h.run_steps(2);
    r.h.get_by_label("Удалить?").click();
    r.h.run_steps(2);
    assert!(!file.exists());
}

#[test]
fn backup_viewer_disables_writes() {
    let p = fixture(CURRENT);
    let mut app = App::from_backup(
        type84_gui::model::BoardState::load_backup(&p).unwrap(),
        Some(p),
    );
    app.edits
        .as_mut()
        .unwrap()
        .edit_keys(&[0], |k| k.actuation = 100);
    let mut h = Harness::builder()
        .with_size(Vec2::new(1400.0, 1500.0))
        .with_pixels_per_point(1.0)
        .with_context_setup(theme::apply)
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.run_steps(3);
    assert!(!h.state().can_write());
    h.get_by_label("Применить").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_none() && h.state().busy.is_none());
}

#[test]
fn background_read_feeds_tray_model() {
    let dir = std::env::temp_dir().join(format!("keyboard-hub-tray-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _cleanup = TempDir(dir.clone());
    let paths = Paths::under(&dir);
    // A preset matching the board's Hall table, and one that does not.
    let st = type84_gui::model::BoardState::load_backup(&fixture(CURRENT)).unwrap();
    let mine = apply::preset_from_board(
        "mine",
        &[type84_cli::preset::Section::Hall],
        &st.raw,
        "t".into(),
    )
    .unwrap();
    mine.save(&paths.presets_dir.join("mine.json"), false)
        .unwrap();
    let mut other = mine.clone();
    other.name = "other".into();
    other.hall.as_mut().unwrap().keys[0].actuation_mm = 0.5;
    other
        .save(&paths.presets_dir.join("other.json"), false)
        .unwrap();

    let sim = sim_from(CURRENT);
    let worker = Worker::spawn_with(
        Box::new(move || Box::new(SimBackend(sim))),
        paths.clone(),
        None,
    );
    let app = App::with_worker(worker, paths);
    let mut h = Harness::builder()
        .with_size(Vec2::new(900.0, 700.0))
        .with_pixels_per_point(1.0)
        .with_context_setup(theme::apply)
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    let end = Instant::now() + Duration::from_secs(20);
    while h.state().board.is_none() {
        assert!(Instant::now() < end, "no background read");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
    h.run_steps(2);
    assert_eq!(
        h.state().screen,
        type84_gui::app::Screen::Devices,
        "stays on the card"
    );
    let m = h.state().tray_model_now();
    assert_eq!(m.presets, vec!["mine".to_string(), "other".to_string()]);
    assert_eq!(m.current[0], ("Отклик".to_string(), "mine".to_string()));
    assert_eq!(m.current[1].1, "—");
}
