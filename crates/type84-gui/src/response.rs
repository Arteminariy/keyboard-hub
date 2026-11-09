//! The "Отклик" tab: key selection bar, keyboard scheme, per-key Hall /
//! Rapid Trigger panel and keyboard-wide settings (UI_SPEC §4). Edits only
//! change the local pending state; «Применить» writes them. Keyboard-wide
//! settings are read-only (SET_GAME_MODE 0x21 is not writable).

use egui::{vec2, Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui};

use crate::app::{mm, ordered, App};
use crate::geometry::{self, PRESET_ARROWS, PRESET_WASD};
use crate::i18n::{fill, s, socd_mode};
use crate::keyboard::{self, EditorView, KeyboardAction};
use crate::model::{
    KeyHall, ACTUATION_MAX, ACTUATION_MIN, ACTUATION_PRESET, DEAD_ZONE_MAX, REPORT_RATES, SENS_MAX,
    SENS_MIN, SENS_PRESET,
};
use crate::theme::{self, font, Weight};
use crate::widgets::{self, Icon, Tone, Tri};

/// Values shown while nothing is selected (the descriptor presets).
const PRESET_KEY: KeyHall = KeyHall {
    actuation: ACTUATION_PRESET,
    rt: false,
    press: SENS_PRESET,
    release: SENS_PRESET,
    separate: false,
    whole_travel: false,
};

/// Per-field "differs across the selection" flags.
#[derive(Default, Clone, Copy)]
struct Mixed {
    actuation: bool,
    rt: bool,
    separate: bool,
    press: bool,
    release: bool,
    whole_travel: bool,
}

impl Mixed {
    fn any(&self) -> bool {
        self.actuation
            || self.rt
            || self.separate
            || self.press
            || self.release
            || self.whole_travel
    }
}

fn mixed_of(keys: &[KeyHall]) -> Mixed {
    let Some(f) = keys.first() else {
        return Mixed::default();
    };
    let d = |g: &dyn Fn(&KeyHall) -> u32| keys.iter().any(|k| g(k) != g(f));
    Mixed {
        actuation: d(&|k| k.actuation as u32),
        rt: d(&|k| k.rt as u32),
        separate: d(&|k| k.separate as u32),
        press: d(&|k| k.press as u32),
        release: d(&|k| k.release as u32),
        whole_travel: d(&|k| k.whole_travel as u32),
    }
}

/// A row with a label (+ optional hint) on the left and a fixed-width
/// control on the right.
fn setting_row<R>(
    ui: &mut Ui,
    label: &str,
    hint: Option<&str>,
    control_w: f32,
    control: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.horizontal_top(|ui| {
        let text_w = (ui.available_width() - control_w - theme::SP_RELATED).max(80.0);
        ui.allocate_ui_with_layout(vec2(text_w, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.set_width(text_w);
            widgets::title(ui, label);
            if let Some(h) = hint {
                widgets::hint(ui, h);
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Min), control)
            .inner
    })
    .inner
}

/// Label on the left, value box + unit on the right.
fn value_row(
    ui: &mut Ui,
    label: &str,
    extra: Option<&str>,
    muted: bool,
    value: &mut u16,
    (min, max): (u16, u16),
    reset: Option<(u16, &mut bool)>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.set_min_height(32.0);
        let color = if muted { theme::MUTED } else { theme::FG };
        let weight = if muted {
            Weight::Medium
        } else {
            Weight::Semibold
        };
        let label_id = ui
            .label(
                RichText::new(label)
                    .font(font(theme::FS_BODY, weight))
                    .color(color),
            )
            .id;
        if let Some((preset, clicked)) = reset {
            *clicked =
                widgets::reset_button(ui, &fill(s().reset_to, &[("value", &mm(preset))])).clicked();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            widgets::unit(ui);
            changed = widgets::value_box(ui, value, min, max, label_id).changed();
        });
    });
    // The mixed marker goes on its own line so the row never overflows.
    if let Some(e) = extra {
        ui.add_space(-6.0);
        ui.label(
            RichText::new(e)
                .font(font(theme::FS_SMALL, Weight::Regular))
                .color(theme::WARN),
        );
    }
    changed
}

impl App {
    pub(crate) fn response_tab(&mut self, ui: &mut Ui) {
        self.hall_changed_banner(ui);
        self.socd_banner(ui);
        self.pick_bar(ui);
        ui.add_space(theme::SP_RELATED);
        self.keyboard_panel(ui);
        ui.add_space(theme::SP_SECTION);
        self.hall_panel(ui);
        ui.add_space(theme::SP_RELATED);
        widgets::notice(ui, Tone::Info, s().hw_switches_note, None);
        ui.add_space(theme::SP_SECTION);
    }

    fn hall_changed_banner(&mut self, ui: &mut Ui) {
        if !self.hall_changed {
            return;
        }
        ui.horizontal_top(|ui| {
            ui.set_max_width(ui.available_width());
            let w = (ui.available_width() - 320.0).max(300.0);
            ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(w);
                widgets::notice(
                    ui,
                    Tone::Warn,
                    s().hall_changed,
                    Some(s().hall_changed_detail),
                );
            });
            ui.add_space(theme::SP_RELATED);
            ui.vertical(|ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let can = self.can_write();
                    if ui
                        .add_enabled_ui(can, |ui| widgets::button(ui, s().restore_from_backup))
                        .inner
                        .on_disabled_hover_text(s().restore_disabled)
                        .clicked()
                    {
                        self.open_hall_restore();
                    }
                    if widgets::ghost_button(ui, s().dismiss).clicked() {
                        self.hall_changed = false;
                    }
                });
            });
        });
        ui.add_space(theme::SP_RELATED);
    }

    fn socd_banner(&mut self, ui: &mut Ui) {
        let Some(board) = &self.board else { return };
        if board.socd.is_empty() {
            return;
        }
        let pairs: Vec<String> = board
            .socd
            .iter()
            .map(|p| {
                let (a, b) = p.names();
                fill(
                    s().socd_pair,
                    &[("k1", &a), ("k2", &b), ("mode", &socd_mode(p.mode))],
                )
            })
            .collect();
        let detail = format!(
            "{} {}",
            fill(s().socd_detail, &[("pairs", &pairs.join(", "))]),
            s().socd_switch_hint
        );
        widgets::notice(ui, Tone::Warn, s().socd_banner, Some(&detail));
        ui.add_space(theme::SP_RELATED);
    }

    fn select_group(&mut self, keys: impl IntoIterator<Item = u8>) {
        if !self.add_mode {
            self.selection.clear();
        }
        self.selection.extend(keys);
    }

    fn pick_bar(&mut self, ui: &mut Ui) {
        Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::PANEL_BORDER))
            .corner_radius(CornerRadius::same(theme::R_TOOL))
            .inner_margin(Margin {
                left: 12,
                right: 6,
                top: 5,
                bottom: 5,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    widgets::title(ui, s().pick_keys);
                    let n = self.selection.len();
                    let count = if n == 0 {
                        s().none_selected.to_string()
                    } else {
                        fill(s().selected_count, &[("n", &n.to_string())])
                    };
                    ui.add_space(theme::SP_CONTROL);
                    ui.label(
                        RichText::new(count)
                            .font(font(theme::FS_HINT, Weight::Regular))
                            .color(theme::HINT),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.add_enabled_ui(false, |ui| widgets::button(ui, s().test_keys))
                            .inner
                            .on_disabled_hover_text(s().test_keys_disabled);
                        ui.add_space(4.0);
                        let state = if self.select_by_press {
                            Tri::On
                        } else {
                            Tri::Off
                        };
                        if widgets::toggle(ui, state, s().select_by_press)
                            .on_hover_text(s().select_by_press_hint)
                            .clicked()
                        {
                            self.select_by_press = !self.select_by_press;
                        }
                        ui.label(
                            RichText::new(s().select_by_press)
                                .font(font(theme::FS_BODY, Weight::Regular)),
                        );
                        ui.add_space(4.0);
                        let (r, _) = ui.allocate_exact_size(vec2(1.0, 26.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 0.0, theme::DIVIDER);
                        ui.add_space(4.0);
                        if widgets::icon_button(
                            ui,
                            Icon::AddToSelection,
                            self.add_mode,
                            s().add_to_selection,
                        )
                        .clicked()
                        {
                            self.add_mode = !self.add_mode;
                        }
                        if widgets::icon_button(ui, Icon::Close, false, s().clear_selection)
                            .clicked()
                        {
                            self.selection.clear();
                        }
                        if widgets::ghost_button(ui, s().preset_all).clicked() {
                            self.select_group(geometry::keys().iter().map(|k| k.index));
                        }
                        if widgets::ghost_button(ui, s().preset_arrows).clicked() {
                            self.select_group(PRESET_ARROWS);
                        }
                        if widgets::ghost_button(ui, s().preset_wasd).clicked() {
                            self.select_group(PRESET_WASD);
                        }
                    });
                });
            });
    }

    fn keyboard_panel(&mut self, ui: &mut Ui) {
        let Some(edits) = &self.edits else { return };
        let action = Frame::new()
            .fill(theme::KEY_PLATE)
            .stroke(Stroke::new(1.0, theme::PANEL_BORDER))
            .corner_radius(CornerRadius::same(theme::R_PANEL))
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    let changed = |i: u8| edits.key_changed(i);
                    let view = EditorView {
                        keys: &edits.keys,
                        selection: &self.selection,
                        changed: &changed,
                        enabled: true,
                    };
                    keyboard::editor(ui, &view)
                })
                .inner
            })
            .inner;
        match action {
            Some(KeyboardAction::Click(i)) => {
                if self.add_mode {
                    if !self.selection.remove(&i) {
                        self.selection.insert(i);
                    }
                } else if self.selection.len() == 1 && self.selection.contains(&i) {
                    self.selection.clear();
                } else {
                    self.selection = [i].into();
                }
            }
            Some(KeyboardAction::Marquee(keys)) => {
                if !self.add_mode {
                    self.selection.clear();
                }
                self.selection.extend(keys);
            }
            None => {}
        }
    }

    fn hall_panel(&mut self, ui: &mut Ui) {
        widgets::panel_frame().show(ui, |ui| {
            let total = ui.available_width();
            ui.set_width(total);
            let gap = theme::SP_COLUMN;
            let left_w = ((total - gap) * 0.62).floor();
            let right_w = total - left_w - gap;
            let mut divider_x = 0.0;
            let r = ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.allocate_ui_with_layout(vec2(left_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(left_w);
                    ui.spacing_mut().item_spacing = vec2(theme::SP_CONTROL, theme::SP_CONTROL);
                    self.selected_keys_half(ui);
                });
                divider_x = ui.cursor().left() + gap / 2.0;
                ui.add_space(gap);
                ui.allocate_ui_with_layout(
                    vec2(right_w, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(right_w);
                        ui.spacing_mut().item_spacing = vec2(theme::SP_CONTROL, theme::SP_CONTROL);
                        self.whole_board_half(ui);
                    },
                );
            });
            let rr = r.response.rect;
            ui.painter().line_segment(
                [
                    egui::pos2(divider_x, rr.top()),
                    egui::pos2(divider_x, rr.bottom()),
                ],
                Stroke::new(1.0, theme::DIVIDER),
            );
        });
    }

    fn selected_keys_half(&mut self, ui: &mut Ui) {
        let sel = ordered(&self.selection);
        let Some(edits) = &mut self.edits else { return };
        let keys: Vec<KeyHall> = sel
            .iter()
            .filter_map(|i| edits.keys.get(i).copied())
            .collect();
        let none = keys.is_empty();
        let mixed = mixed_of(&keys);
        let mut k = keys.first().copied().unwrap_or(PRESET_KEY);

        ui.horizontal(|ui| {
            widgets::title(ui, s().selected_keys);
        });
        if none {
            widgets::hint(ui, s().pick_first);
        } else if mixed.any() {
            widgets::hint(ui, s().mixed);
        }
        ui.add_space(theme::SP_CONTROL);

        let w = ui.available_width();
        let col_w = ((w - theme::SP_COLUMN) / 2.0).floor();
        let before = k;
        let mut reset_press = false;
        let mut reset_release = false;
        let mut press_changed = false;
        let mut release_changed = false;
        let mut act_changed = false;
        let mut rt_clicked = false;
        let mut sep_clicked = false;
        let mut whole_clicked = false;
        let ms = s().mixed_short;

        ui.add_enabled_ui(!none, |ui| {
            if none {
                ui.multiply_opacity(0.45);
            }
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                // Column A: actuation point.
                ui.allocate_ui_with_layout(vec2(col_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(col_w);
                    ui.spacing_mut().item_spacing.y = theme::SP_CONTROL;
                    act_changed |= value_row(
                        ui,
                        s().actuation_point,
                        mixed.actuation.then_some(ms),
                        false,
                        &mut k.actuation,
                        (ACTUATION_MIN, ACTUATION_MAX),
                        None,
                    );
                    act_changed |= widgets::range_slider(
                        ui,
                        &mut k.actuation,
                        ACTUATION_MIN,
                        ACTUATION_MAX,
                        col_w,
                        s().actuation_point,
                    )
                    .changed();
                    widgets::hint(ui, s().actuation_hint);
                });
                ui.add_space(theme::SP_COLUMN);
                // Column B: Rapid Trigger.
                ui.allocate_ui_with_layout(vec2(col_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(col_w);
                    ui.spacing_mut().item_spacing.y = theme::SP_CONTROL;
                    let rt = if mixed.rt {
                        Tri::Mixed
                    } else if k.rt {
                        Tri::On
                    } else {
                        Tri::Off
                    };
                    rt_clicked = setting_row(ui, s().rapid_trigger, None, 50.0, |ui| {
                        widgets::toggle(ui, rt, s().rapid_trigger).clicked()
                    });
                    ui.add_space(theme::SP_CONTROL);
                    let rt_on = k.rt || mixed.rt;
                    ui.add_enabled_ui(rt_on, |ui| {
                        if !rt_on && !none {
                            ui.multiply_opacity(0.55);
                        }
                        let sep = if mixed.separate {
                            Tri::Mixed
                        } else if k.separate {
                            Tri::On
                        } else {
                            Tri::Off
                        };
                        sep_clicked = setting_row(ui, s().separate, None, 50.0, |ui| {
                            widgets::toggle(ui, sep, s().separate).clicked()
                        });
                        ui.add_space(theme::SP_RELATED);
                        press_changed |= value_row(
                            ui,
                            s().press_sensitivity,
                            mixed.press.then_some(ms),
                            true,
                            &mut k.press,
                            (SENS_MIN, SENS_MAX),
                            Some((SENS_PRESET, &mut reset_press)),
                        );
                        press_changed |= widgets::range_slider(
                            ui,
                            &mut k.press,
                            SENS_MIN,
                            SENS_MAX,
                            col_w,
                            s().press_sensitivity,
                        )
                        .changed();
                        widgets::hint(ui, s().press_hint);
                        ui.add_space(theme::SP_RELATED);
                        let sep_on = k.separate || mixed.separate;
                        ui.add_enabled_ui(sep_on, |ui| {
                            release_changed |= value_row(
                                ui,
                                s().release_sensitivity,
                                mixed.release.then_some(ms),
                                true,
                                &mut k.release,
                                (SENS_MIN, SENS_MAX),
                                Some((SENS_PRESET, &mut reset_release)),
                            );
                            release_changed |= widgets::range_slider(
                                ui,
                                &mut k.release,
                                SENS_MIN,
                                SENS_MAX,
                                col_w,
                                s().release_sensitivity,
                            )
                            .changed();
                            widgets::hint(ui, s().release_hint);
                        });
                        ui.add_space(theme::SP_RELATED);
                        let wt = if mixed.whole_travel {
                            Tri::Mixed
                        } else if k.whole_travel {
                            Tri::On
                        } else {
                            Tri::Off
                        };
                        whole_clicked = setting_row(
                            ui,
                            s().whole_travel,
                            Some(s().whole_travel_hint),
                            50.0,
                            |ui| widgets::toggle(ui, wt, s().whole_travel).clicked(),
                        );
                    });
                });
            });
        });

        if none {
            return;
        }
        // Apply what changed to every selected key.
        if act_changed {
            let v = k.actuation;
            edits.edit_keys(&sel, |x| x.actuation = v);
        }
        if rt_clicked {
            let on = mixed.rt || !before.rt;
            edits.edit_keys(&sel, |x| {
                x.rt = on;
                if on && x.press == 0 && x.release == 0 {
                    // The firmware needs both sensitivities > 0 for RT.
                    x.press = SENS_PRESET;
                    x.release = SENS_PRESET;
                }
            });
        }
        if sep_clicked {
            let on = mixed.separate || !before.separate;
            edits.edit_keys(&sel, |x| {
                x.separate = on;
                if !on {
                    x.release = x.press;
                }
            });
        }
        if reset_press {
            k.press = SENS_PRESET;
            press_changed = true;
        }
        if reset_release {
            k.release = SENS_PRESET;
            release_changed = true;
        }
        if press_changed {
            let v = k.press;
            edits.edit_keys(&sel, |x| {
                x.press = v;
                if !x.separate {
                    x.release = v;
                }
            });
        }
        if release_changed {
            let v = k.release;
            edits.edit_keys(&sel, |x| {
                if x.separate {
                    x.release = v;
                }
            });
        }
        if whole_clicked {
            let on = mixed.whole_travel || !before.whole_travel;
            edits.edit_keys(&sel, |x| x.whole_travel = on);
        }
    }

    fn whole_board_half(&mut self, ui: &mut Ui) {
        let Some(edits) = &mut self.edits else { return };
        // Shown as read; SET_GAME_MODE (0x21) is not on the write allowlist.
        edits.settings = edits.base_settings;
        let st = &mut edits.settings;
        widgets::title(ui, s().whole_board).on_hover_text(s().settings_readonly);
        ui.add_space(theme::SP_CONTROL + 4.0);
        let r = ui.add_enabled_ui(false, |ui| self::board_columns(ui, st, edits.base_settings));
        r.response.on_disabled_hover_text(s().settings_readonly);
        widgets::hint(ui, s().settings_readonly);
    }
}

fn board_columns(
    ui: &mut Ui,
    st: &mut crate::model::BoardSettings,
    base: crate::model::BoardSettings,
) {
    {
        let w = ui.available_width();
        let col_w = ((w - theme::SP_COLUMN) / 2.0).floor();
        let tri = |b: bool| if b { Tri::On } else { Tri::Off };
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            // Column C: polling rate, auto-calibration, stability.
            ui.allocate_ui_with_layout(vec2(col_w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(col_w);
                ui.spacing_mut().item_spacing.y = theme::SP_RELATED;
                let combo_w = (col_w * 0.42).clamp(100.0, 140.0);
                setting_row(ui, s().polling_rate, None, combo_w, |ui| {
                    let label = |hz: Option<u32>| match hz {
                        Some(h) => fill(s().hz, &[("n", &h.to_string())]),
                        None => "—".to_string(),
                    };
                    let current = base.report_rate_hz;
                    egui::ComboBox::from_id_salt("report-rate")
                        .width(combo_w)
                        .selected_text(label(st.report_rate_hz))
                        .show_ui(ui, |ui| {
                            for hz in REPORT_RATES {
                                let mut text = label(Some(hz));
                                if current == Some(hz) {
                                    text = fill(s().hz_current, &[("label", &text)]);
                                }
                                ui.selectable_value(&mut st.report_rate_hz, Some(hz), text);
                            }
                        })
                        .response
                        .on_hover_text(s().polling_description);
                });
                if setting_row(
                    ui,
                    s().auto_calibration,
                    Some(s().auto_calibration_hint),
                    50.0,
                    |ui| {
                        widgets::toggle(ui, tri(st.auto_calibration), s().auto_calibration)
                            .clicked()
                    },
                ) {
                    st.auto_calibration = !st.auto_calibration;
                }
                if setting_row(ui, s().stability, Some(s().stability_hint), 50.0, |ui| {
                    widgets::toggle(ui, tri(st.stability), s().stability).clicked()
                }) {
                    st.stability = !st.stability;
                }
            });
            ui.add_space(theme::SP_COLUMN);
            // Column D: dead zones.
            ui.allocate_ui_with_layout(vec2(col_w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(col_w);
                ui.spacing_mut().item_spacing.y = theme::SP_RELATED;
                if setting_row(ui, s().dead_zones, Some(s().dead_zones_hint), 50.0, |ui| {
                    widgets::toggle(ui, tri(st.dead_zones), s().dead_zones).clicked()
                }) {
                    st.dead_zones = !st.dead_zones;
                }
                if st.dead_zones {
                    value_row(
                        ui,
                        s().dead_zone_top,
                        None,
                        true,
                        &mut st.top_dead_zone,
                        (0, DEAD_ZONE_MAX),
                        None,
                    );
                    value_row(
                        ui,
                        s().dead_zone_bottom,
                        None,
                        true,
                        &mut st.bottom_dead_zone,
                        (0, DEAD_ZONE_MAX),
                        None,
                    );
                }
            });
        });
    }
}
