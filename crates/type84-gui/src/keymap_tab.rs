//! The "Клавиши" tab: layer 0 (read-only) and the Fn layer on the KLE
//! scheme, with a picker for the Fn-layer action of one key. Vendor
//! functions and the Fn key are locked (same rule as the write path).
//! Edits are pending until «Применить».

use egui::{vec2, Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui};
use type84_proto::fn_layer::FnAction;

use crate::actions::{self, Lock};
use crate::app::App;
use crate::i18n::{self, fill, s};
use crate::keyboard::{self, KeyFace, KeyboardAction, SchemeView};
use crate::theme::{self, font, Weight};
use crate::widgets::{self, Tone};

impl App {
    pub(crate) fn keymap_tab(&mut self, ui: &mut Ui) {
        let Some(edits) = &self.edits else { return };
        let Some(board) = &self.board else { return };
        let fn_layer = self.keymap.fn_layer;

        // Layer switch.
        ui.horizontal(|ui| {
            widgets::title(ui, s().keymap_layers);
            ui.add_space(theme::SP_CONTROL);
            let sel = if fn_layer { 1 } else { 0 };
            if let Some(i) =
                widgets::segmented(ui, &[s().keymap_layer_base, s().keymap_layer_fn], sel)
            {
                self.keymap.fn_layer = i == 1;
                self.keymap.key = None;
            }
        });
        ui.add_space(theme::SP_RELATED);

        let layer0 = board.layer0.clone();
        let selection: std::collections::BTreeSet<u8> = self.keymap.key.into_iter().collect();
        let face = |i: u8| -> KeyFace {
            let base_rec = rec(&layer0, i);
            if !fn_layer {
                let short = actions::record_short(base_rec);
                let (legend, sub) = i18n::key_legend(i);
                // The familiar legend when layer 0 holds the plain key.
                let plain = legend.split(' ').next() == Some(short.as_str());
                return KeyFace {
                    main: if plain {
                        legend.to_string()
                    } else if short.is_empty() {
                        "—".into()
                    } else {
                        short
                    },
                    sub: if plain {
                        sub.to_string()
                    } else {
                        legend.to_string()
                    },
                    ..Default::default()
                };
            }
            let r = edits.fn_record(i);
            let lock = actions::fn_lock(i, edits.fn_base_record(i));
            let short = actions::record_short(r);
            let (legend, _) = i18n::key_legend(i);
            // A key that repeats its own base key reads like layer 0.
            let plain = legend.split(' ').next() == Some(short.as_str());
            KeyFace {
                main: if plain { legend.to_string() } else { short },
                sub: if plain {
                    String::new()
                } else {
                    legend.to_string()
                },
                bold: r != edits.fn_factory_record(i),
                fill: None,
                locked: lock.is_some(),
                sub_corner: true,
            }
        };
        let changed = |i: u8| fn_layer && edits.fn_record(i) != edits.fn_base_record(i);
        let action = Frame::new()
            .fill(theme::KEY_PLATE)
            .stroke(Stroke::new(1.0, theme::PANEL_BORDER))
            .corner_radius(CornerRadius::same(theme::R_PANEL))
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    keyboard::scheme(
                        ui,
                        &SchemeView {
                            face: &face,
                            selection: &selection,
                            changed: &changed,
                            enabled: fn_layer,
                            hall: None,
                        },
                    )
                })
                .inner
            })
            .inner;
        if let Some(KeyboardAction::Click(i)) = action {
            self.keymap.key = if self.keymap.key == Some(i) {
                None
            } else {
                Some(i)
            };
        }
        ui.add_space(theme::SP_RELATED);
        if !fn_layer {
            widgets::notice(ui, Tone::Info, s().keymap_base_readonly, None);
            return;
        }
        widgets::hint(ui, s().keymap_legend);
        ui.add_space(theme::SP_RELATED);
        self.fn_picker(ui);
        ui.add_space(theme::SP_SECTION);
    }

    fn fn_picker(&mut self, ui: &mut Ui) {
        let Some(edits) = &mut self.edits else { return };
        widgets::panel_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let Some(key) = self.keymap.key else {
                widgets::title(ui, s().tab_keymap);
                widgets::hint(ui, s().keymap_pick_key);
                return;
            };
            let label = i18n::key_legend(key).0;
            widgets::title(ui, &fill(s().keymap_key_title, &[("key", label)]));
            let now = edits.fn_record(key);
            let factory = edits.fn_factory_record(key);
            widgets::hint(
                ui,
                &format!(
                    "{} · {}",
                    fill(s().keymap_now, &[("action", &actions::record_label(now))]),
                    fill(
                        s().keymap_factory,
                        &[("action", &actions::record_label(factory))]
                    )
                ),
            );
            ui.add_space(theme::SP_RELATED);
            match actions::fn_lock(key, edits.fn_base_record(key)) {
                Some(Lock::FnKey) => {
                    widgets::notice(ui, Tone::Info, s().keymap_locked_fn, None);
                    return;
                }
                Some(Lock::Firmware) => {
                    widgets::notice(ui, Tone::Info, s().keymap_locked_fw, None);
                    return;
                }
                None => {}
            }
            let groups = actions::groups();
            let titles: Vec<&str> = groups.iter().map(|g| g.title).collect();
            let gsel = self.keymap.group.min(groups.len() - 1);
            ui.horizontal(|ui| {
                if let Some(i) = widgets::segmented(ui, &titles, gsel) {
                    self.keymap.group = i;
                }
                ui.add_space(theme::SP_GROUP);
                let (r, _) = ui.allocate_exact_size(vec2(1.0, 26.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 0.0, theme::DIVIDER);
                ui.add_space(theme::SP_CONTROL);
                if pick_button(ui, s().keymap_none, now == [0; 4]).clicked() {
                    edits.set_fn(key, FnAction::None);
                }
                let fac_ok = type84_proto::fn_layer::replaceable(factory);
                if ui
                    .add_enabled_ui(fac_ok, |ui| {
                        pick_button(ui, s().keymap_factory_btn, now == factory)
                    })
                    .inner
                    .clicked()
                {
                    edits.set_fn(key, FnAction::Factory);
                }
            });
            ui.add_space(theme::SP_RELATED);
            let gsel = self.keymap.group.min(groups.len() - 1);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for a in &groups[gsel].items {
                    let rec = a.record(factory);
                    if pick_button(ui, &actions::action_short(*a), rec == now)
                        .on_hover_text(actions::record_label(rec))
                        .clicked()
                    {
                        edits.set_fn(key, *a);
                    }
                }
            });
            ui.add_space(theme::SP_RELATED);
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                widgets::hint(ui, s().keymap_mods_note);
            });
        });
    }
}

fn rec(block: &[u8], i: u8) -> [u8; 4] {
    let o = i as usize * 4;
    [block[o], block[o + 1], block[o + 2], block[o + 3]]
}

/// Picker button; white when it is the key's current action.
fn pick_button(ui: &mut Ui, text: &str, on: bool) -> egui::Response {
    let rt = RichText::new(text).font(font(theme::FS_HINT, Weight::Medium));
    let b = if on {
        egui::Button::new(rt.color(theme::ON_ACTIVE))
            .fill(theme::ACTIVE)
            .stroke(Stroke::new(1.0, theme::ACTIVE))
    } else {
        egui::Button::new(rt)
            .fill(theme::CONTROL)
            .stroke(Stroke::new(1.0, theme::CONTROL_BORDER))
    };
    ui.add(
        b.corner_radius(CornerRadius::same(theme::R_CONTROL))
            .min_size(vec2(56.0, 32.0)),
    )
}
