//! UI tests on the real backups (copied to tests/fixtures) with
//! egui_kittest. `render_screens` writes PNGs to docs/screens/ when
//! TYPE84_RENDER=1 is set (it needs a wgpu adapter, e.g. llvmpipe).

use std::path::{Path, PathBuf};

use egui::Vec2;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use type84_gui::app::{Screen, Tab};
use type84_gui::headless::compare_ground_truth;
use type84_gui::model::BoardState;
use type84_gui::worker::Presence;
use type84_gui::{theme, App};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

const CURRENT: &str = "type84-20261109-220355-fw1.17.json";
const WITH_SOCD: &str = "type84-20261109-215824-fw1.17.json";

fn harness(app: App, size: Vec2) -> Harness<'static, App> {
    Harness::builder()
        .with_size(size)
        .with_pixels_per_point(1.0)
        .with_context_setup(theme::apply)
        .build_ui_state(|ui, app: &mut App| app.show(ui), app)
}

fn backup_app(name: &str) -> App {
    let p = fixture(name);
    App::from_backup(BoardState::load_backup(&p).unwrap(), Some(p))
}

#[test]
fn backup_matches_ground_truth() {
    let st = BoardState::load_backup(&fixture(CURRENT)).unwrap();
    let (lines, ok) = compare_ground_truth(&st);
    assert!(ok, "{lines:#?}");
    assert_eq!(st.firmware, "1.17");
    assert!(st.socd.is_empty(), "current backup has no Snap Tap");
}

#[test]
fn older_backup_has_factory_socd_pair() {
    let st = BoardState::load_backup(&fixture(WITH_SOCD)).unwrap();
    assert_eq!(st.socd.len(), 1);
    let p = &st.socd[0];
    assert_eq!((p.mode, p.keys.clone()), (3, vec![49, 51]));
    assert_eq!(p.names(), ("A".to_string(), "D".to_string()));
}

#[test]
fn presets_select_and_edits_stay_local() {
    let mut h = harness(backup_app(CURRENT), Vec2::new(1400.0, 1500.0));
    h.run_steps(2);
    assert_eq!(h.state().screen, Screen::Device);
    assert_eq!(h.state().tab, Tab::Response);
    h.get_by_label("WASD").click();
    h.run_steps(2);
    assert_eq!(
        h.state().selection.iter().copied().collect::<Vec<_>>(),
        vec![34, 49, 50, 51]
    );
    // "Стрелки" adds (additive mode is on by default), × clears.
    h.get_by_label("Стрелки").click();
    h.run_steps(2);
    assert_eq!(h.state().selection.len(), 8);
    h.get_by_label("Снять выбор").click();
    h.run_steps(2);
    assert!(h.state().selection.is_empty());

    // A local edit shows the pending bar; "Сбросить" discards it.
    h.state_mut().selection = [0u8].into();
    h.run_steps(1);
    {
        let app = h.state_mut();
        let sel = vec![0u8];
        app.edits
            .as_mut()
            .unwrap()
            .edit_keys(&sel, |k| k.actuation = 100);
    }
    h.run_steps(2);
    assert!(h.query_by_label("Несохранённые изменения: 1").is_some());
    h.get_by_label("Сбросить").click();
    h.run_steps(2);
    assert!(h.query_by_label("Несохранённые изменения: 1").is_none());
}

#[test]
fn rapid_trigger_toggle_on_mixed_selection_turns_all_on() {
    let mut h = harness(backup_app(CURRENT), Vec2::new(1400.0, 1500.0));
    h.state_mut().selection = [34u8, 0].into(); // W (RT on) + Esc (RT off)
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Rapid Trigger")
        .click();
    h.run_steps(2);
    let e = h.state().edits.as_ref().unwrap();
    assert!(e.keys[&0].rt && e.keys[&34].rt);
    assert_eq!(
        (e.keys[&0].press, e.keys[&0].release),
        (10, 10),
        "RT on from 0 uses the 0.10 preset"
    );
    assert_eq!(e.diff_count(), 1);
}

#[test]
fn physical_keys_toggle_selection() {
    let mut h = harness(backup_app(CURRENT), Vec2::new(1400.0, 1500.0));
    h.state_mut().select_by_press = true;
    h.run_steps(1);
    h.key_press(egui::Key::W);
    h.run_steps(1);
    h.key_press(egui::Key::ArrowUp);
    h.run_steps(1);
    assert_eq!(
        h.state().selection.iter().copied().collect::<Vec<_>>(),
        vec![34, 90]
    );
    h.key_press(egui::Key::W);
    h.run_steps(1);
    assert_eq!(
        h.state().selection.iter().copied().collect::<Vec<_>>(),
        vec![90]
    );
}

#[test]
fn select_by_press_via_toggle_survives_space_and_enter() {
    let mut h = harness(backup_app(CURRENT), Vec2::new(1400.0, 1500.0));
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Выбор нажатием клавиш")
        .click();
    h.run_steps(2);
    assert!(h.state().select_by_press);
    for k in [
        egui::Key::Space,
        egui::Key::W,
        egui::Key::Enter,
        egui::Key::ArrowDown,
    ] {
        h.key_press(k);
        h.run_steps(2);
    }
    assert!(
        h.state().select_by_press,
        "keys must not reach the focused toggle"
    );
    assert_eq!(
        h.state().selection.iter().copied().collect::<Vec<_>>(),
        vec![34, 76, 83, 89]
    );
}

#[test]
fn socd_banner_only_when_present() {
    let mut h = harness(backup_app(WITH_SOCD), Vec2::new(1400.0, 1500.0));
    h.run_steps(2);
    assert!(h
        .query_by_label_contains("Snap Tap (SOCD) включён")
        .is_some());
    let mut h = harness(backup_app(CURRENT), Vec2::new(1400.0, 1500.0));
    h.run_steps(2);
    assert!(h
        .query_by_label_contains("Snap Tap (SOCD) включён")
        .is_none());
}

/// Hall block as the case switch leaves it: every key at `preset`, RT off,
/// flags kept (PROTOCOL.md §9.1).
fn switched_hall_block(name: &str, preset_raw: u16) -> Vec<u8> {
    let b = type84_cli::backup::Backup::load(&fixture(name)).unwrap();
    let mut raw = b.blocks.axis_rt.bytes().unwrap();
    for k in type84_proto::layout::keys() {
        let r = &mut raw[k.index as usize * 8..k.index as usize * 8 + 8];
        r[2..4].copy_from_slice(&preset_raw.to_le_bytes());
        r[4..8].fill(0);
    }
    raw
}

#[test]
fn case_switch_rewrite_raises_banner() {
    let mut h = harness(backup_app(CURRENT), Vec2::new(1400.0, 1500.0));
    h.run_steps(2);
    assert!(h
        .query_by_label("Настройки срабатывания изменены переключателем на корпусе")
        .is_none());
    let same = type84_cli::backup::Backup::load(&fixture(CURRENT))
        .unwrap()
        .blocks
        .axis_rt
        .bytes()
        .unwrap();
    h.state_mut().on_hall_block(&same);
    h.run_steps(2);
    assert!(!h.state().hall_changed, "identical table is not a change");
    h.state_mut()
        .on_hall_block(&switched_hall_block(CURRENT, 200));
    h.run_steps(2);
    assert!(h
        .query_by_label("Настройки срабатывания изменены переключателем на корпусе")
        .is_some());
    let e = h.state().edits.as_ref().unwrap();
    assert_eq!((e.keys[&34].actuation, e.keys[&34].rt), (200, false));
    assert_eq!(e.diff_count(), 0, "board values became the new base");
    h.get_by_label("Скрыть").click();
    h.run_steps(2);
    assert!(!h.state().hall_changed);
}

fn save(h: &mut Harness<'_, App>, name: &str) {
    let img = h.render().expect("wgpu render");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screens");
    std::fs::create_dir_all(&dir).unwrap();
    img.save(dir.join(name)).unwrap();
}

#[test]
fn render_screens() {
    if std::env::var_os("TYPE84_RENDER").is_none() {
        eprintln!("skipped: set TYPE84_RENDER=1 to write docs/screens/*.png");
        return;
    }
    // Device card with a connection error toast, like the hub screenshot.
    let mut app = App::detached();
    app.presence = Some(Presence {
        product: "IO Type 84 Magnetic Black".into(),
        path: "/dev/hidraw6".into(),
    });
    app.push_toast(
        type84_gui::widgets::Tone::Error,
        type84_gui::i18n::s().connect_failed,
        None,
        None,
        0.0,
    );
    let mut h = harness(app, Vec2::new(680.0, 600.0));
    h.run_steps(3);
    save(&mut h, "device-card.png");

    // "Отклик" with WASD selected (as in the hub screenshot).
    let mut app = backup_app(CURRENT);
    app.selection = [34u8, 49, 50, 51].into();
    let mut h = harness(app, Vec2::new(1400.0, 1280.0));
    h.run_steps(3);
    save(&mut h, "response-wasd.png");

    // Nothing selected: the per-key half is dimmed.
    let mut h = harness(backup_app(CURRENT), Vec2::new(1400.0, 1280.0));
    h.run_steps(3);
    save(&mut h, "response-none.png");

    // Older backup with the factory A/D Snap Tap pair, plus a pending edit.
    let mut app = backup_app(WITH_SOCD);
    app.selection = [49u8, 51, 0].into();
    let mut h = harness(app, Vec2::new(1400.0, 1400.0));
    h.run_steps(2);
    {
        let a = h.state_mut();
        let sel = vec![0u8];
        a.edits
            .as_mut()
            .unwrap()
            .edit_keys(&sel, |k| k.actuation = 100);
    }
    h.run_steps(3);
    save(&mut h, "response-socd-mixed.png");

    // Hall table rewritten by the case switch (centre = 2.00 mm).
    let mut app = backup_app(CURRENT);
    app.selection = [34u8, 49, 50, 51].into();
    let mut h = harness(app, Vec2::new(1400.0, 1360.0));
    h.run_steps(2);
    h.state_mut()
        .on_hall_block(&switched_hall_block(CURRENT, 200));
    h.run_steps(3);
    save(&mut h, "response-switch-rewrite.png");

    // "Клавиши": Fn layer, F1 picked and reassigned (pending), vendor keys locked.
    let mut app = backup_app(CURRENT);
    app.tab = Tab::Keymap;
    app.keymap.key = Some(1);
    {
        let e = app.edits.as_mut().unwrap();
        e.set_fn(1, type84_proto::fn_layer::FnAction::Consumer(0x00CD));
        e.set_fn(
            2,
            type84_proto::fn_layer::FnAction::Key {
                mods: 0,
                usage: 0x68,
            },
        );
    }
    let mut h = harness(app, Vec2::new(1400.0, 1160.0));
    h.run_steps(3);
    save(&mut h, "keymap-fn.png");

    // "Клавиши": layer 0, read-only.
    let mut app = backup_app(WITH_SOCD);
    app.tab = Tab::Keymap;
    app.keymap.fn_layer = false;
    let mut h = harness(app, Vec2::new(1400.0, 900.0));
    h.run_steps(3);
    save(&mut h, "keymap-base.png");

    // "Подсветка": single-colour effect with the colour from the record.
    let mut app = backup_app(CURRENT);
    app.tab = Tab::Lighting;
    app.board.as_mut().unwrap().colour_unknown = None;
    {
        let e = app.edits.as_mut().unwrap();
        e.light.mode = 11;
        e.light.color = [0x00, 0xA0, 0xFF];
        e.light.multi = false;
        e.light.brightness = 4;
    }
    let mut h = harness(app, Vec2::new(1400.0, 1280.0));
    h.run_steps(3);
    save(&mut h, "lighting-effect.png");

    // "Подсветка": custom effect with the per-key painter.
    let mut app = backup_app(CURRENT);
    app.tab = Tab::Lighting;
    {
        let e = app.edits.as_mut().unwrap();
        e.light.mode = 128;
        for i in [34u8, 49, 50, 51] {
            e.custom.insert(i, [0xFF, 0x6A, 0x00]);
        }
        e.custom.insert(0, [0xE5, 0x2B, 0x36]);
        for k in type84_proto::layout::keys() {
            e.custom.entry(k.index).and_modify(|c| {
                if *c == [0xFF, 0xFF, 0xFF] {
                    *c = [0x30, 0x40, 0x60];
                }
            });
        }
    }
    app.lighting.selection = [34u8, 49, 50, 51].into();
    let mut h = harness(app, Vec2::new(1400.0, 1280.0));
    h.run_steps(3);
    save(&mut h, "lighting-custom.png");

    // Confirm dialog for a combined Hall + Fn + lighting write.
    let mut app = backup_app(CURRENT);
    {
        let e = app.edits.as_mut().unwrap();
        e.edit_keys(&[34, 49, 50, 51], |k| k.actuation = 60);
        e.set_fn(1, type84_proto::fn_layer::FnAction::Consumer(0x00CD));
        e.light.brightness = 3;
        let req = type84_gui::confirm::PlanRequest::Presets {
            presets: vec![e.to_preset().unwrap()],
            origin: type84_gui::confirm::PlanOrigin::Pending,
        };
        let plan = type84_gui::confirm::build_plan(&req, &app.board.as_ref().unwrap().raw).unwrap();
        app.dialog = Some(type84_gui::dialogs::Dialog::Confirm(Box::new(plan)));
    }
    let mut h = harness(app, Vec2::new(1400.0, 1100.0));
    h.run_steps(4);
    save(&mut h, "confirm-dialog.png");

    // "Устройство" with the program settings.
    let mut app = backup_app(CURRENT);
    app.tab = Tab::Device;
    let mut h = harness(app, Vec2::new(1400.0, 700.0));
    h.run_steps(3);
    save(&mut h, "device-tab.png");

    // The app icon, for packaging.
    let icon = type84_gui::icon::rgba(256);
    let img = image::RgbaImage::from_raw(256, 256, icon).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons");
    std::fs::create_dir_all(&dir).unwrap();
    img.save(dir.join("keyboard-hub.png")).unwrap();
}
