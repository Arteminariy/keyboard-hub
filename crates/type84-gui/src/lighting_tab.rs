//! The "Подсветка" tab: effect, colour, colour mode, brightness, speed and
//! direction, plus a per-key painter for the custom effect. The board never
//! reports the effect colour; the shown colour comes from the CLI's
//! last-write record when it is fresh. Edits are pending until «Применить».

use egui::{vec2, Align, Color32, CornerRadius, Frame, Layout, Margin, Stroke, Ui};
use type84_proto::led;

use crate::app::App;
use crate::i18n::{fill, s};
use crate::keyboard::{self, KeyFace, KeyboardAction, SchemeView};
use crate::theme;
use crate::widgets::{self, Tone};

/// «Готовые цвета».
pub const PALETTE: [[u8; 3]; 10] = [
    [0xE5, 0x2B, 0x36],
    [0xFF, 0x6A, 0x00],
    [0xFF, 0xD4, 0x00],
    [0x22, 0xD0, 0x7C],
    [0x00, 0xC8, 0xC8],
    [0x00, 0xA0, 0xFF],
    [0x2B, 0x5B, 0xFF],
    [0x9B, 0x3B, 0xFF],
    [0xFF, 0x3B, 0xB0],
    [0xFF, 0xFF, 0xFF],
];

fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Effects with a user colour (the others choose colours themselves).
fn has_colour(mode: u8) -> bool {
    !matches!(mode, 0 | 8) && !led::is_custom_mode(mode)
}

/// Colour picker: saturation/value square, hue strip and the palette.
/// Returns true when the colour changed.
fn colour_picker(ui: &mut Ui, c: &mut [u8; 3], salt: &str) -> bool {
    let w = ui.available_width().min(360.0);
    let mut changed = widgets::colour_picker(ui, c, salt, w);
    ui.add_space(theme::SP_CONTROL);
    ui.label(
        egui::RichText::new(s().light_palette)
            .font(theme::font(theme::FS_SMALL, theme::Weight::Medium))
            .color(theme::HINT),
    );
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for p in PALETTE {
            if widgets::swatch(ui, p, 22.0)
                .on_hover_text(led::format_hex_color(p))
                .clicked()
            {
                *c = p;
                changed = true;
            }
        }
    });
    changed
}

impl App {
    pub(crate) fn lighting_tab(&mut self, ui: &mut Ui) {
        let Some(edits) = &self.edits else { return };
        let colour_unknown = self.board.as_ref().and_then(|b| b.colour_unknown.clone());
        let light = edits.light;
        let custom = light.is_custom();
        let colours = edits.custom.clone();
        let changed_keys: std::collections::BTreeSet<u8> =
            edits.custom_changed().into_iter().collect();

        // Scheme with the colours the board would show.
        let face = |i: u8| {
            let (main, sub) = crate::i18n::key_legend(i);
            let fill = if light.mode == 0 {
                None
            } else if custom {
                colours.get(&i).map(|c| c32(*c))
            } else if !light.multi && has_colour(light.mode) {
                Some(theme::mix(theme::KEY_FACE, c32(light.color), 0.55))
            } else {
                None
            };
            KeyFace {
                main: main.to_string(),
                sub: sub.to_string(),
                fill,
                ..Default::default()
            }
        };
        let changed = |i: u8| changed_keys.contains(&i);
        let empty = std::collections::BTreeSet::new();
        let sel = if custom {
            &self.lighting.selection
        } else {
            &empty
        };
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
                            selection: sel,
                            changed: &changed,
                            enabled: custom,
                            hall: None,
                        },
                    )
                })
                .inner
            })
            .inner;
        match action {
            Some(KeyboardAction::Click(i)) if !self.lighting.selection.remove(&i) => {
                self.lighting.selection.insert(i);
            }
            Some(KeyboardAction::Click(_)) => {}
            Some(KeyboardAction::Marquee(keys)) => self.lighting.selection.extend(keys),
            None => {}
        }
        ui.add_space(theme::SP_SECTION);

        widgets::panel_frame().show(ui, |ui| {
            let total = ui.available_width();
            ui.set_width(total);
            let gap = theme::SP_COLUMN;
            let left_w = ((total - gap) * 0.45).floor();
            let right_w = total - left_w - gap;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.allocate_ui_with_layout(vec2(left_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(left_w);
                    ui.spacing_mut().item_spacing = vec2(theme::SP_CONTROL, theme::SP_CONTROL);
                    self.effect_column(ui, left_w);
                });
                ui.add_space(gap);
                ui.allocate_ui_with_layout(
                    vec2(right_w, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(right_w);
                        ui.spacing_mut().item_spacing = vec2(theme::SP_CONTROL, theme::SP_CONTROL);
                        if custom {
                            self.painter_column(ui);
                        } else {
                            self.colour_column(ui, colour_unknown.as_deref());
                        }
                    },
                );
            });
        });
        ui.add_space(theme::SP_SECTION);
    }

    fn effect_column(&mut self, ui: &mut Ui, w: f32) {
        let Some(edits) = &mut self.edits else { return };
        let l = &mut edits.light;
        widgets::title(ui, s().light_title);
        ui.add_space(theme::SP_CONTROL);
        let label_id = widgets::title(ui, s().light_effect).id;
        let before = l.mode;
        egui::ComboBox::from_id_salt("light-effect")
            .width(w.min(320.0))
            .selected_text(led::effect_label_ru(l.mode))
            .show_ui(ui, |ui| {
                for m in led::BOARD_EFFECTS {
                    ui.selectable_value(&mut l.mode, m, led::effect_label_ru(m));
                }
            })
            .response
            .labelled_by(label_id);
        if l.mode != before && !l.is_custom() {
            // Per-key colours only show in the custom effect: drop painting.
            edits.custom = edits.custom_base.clone();
        }
        let l = &mut edits.light;
        ui.add_space(theme::SP_RELATED);
        let off = l.mode == 0;
        ui.add_enabled_ui(!off, |ui| {
            widgets::title(
                ui,
                &format!("{} · {} из 5", s().light_brightness, l.brightness),
            );
            widgets::step_slider(ui, &mut l.brightness, 5, w, s().light_brightness);
            ui.add_space(theme::SP_RELATED);
            widgets::title(ui, &format!("{} · {} из 5", s().light_speed, l.speed));
            widgets::step_slider(ui, &mut l.speed, 5, w, s().light_speed);
            ui.add_space(theme::SP_RELATED);
            widgets::title(ui, s().light_direction);
            if let Some(i) = widgets::segmented(
                ui,
                &[s().light_forward, s().light_reverse],
                l.reverse as usize,
            ) {
                l.reverse = i == 1;
            }
        });
    }

    fn colour_column(&mut self, ui: &mut Ui, unknown: Option<&str>) {
        let Some(edits) = &mut self.edits else { return };
        let l = &mut edits.light;
        let usable = has_colour(l.mode);
        widgets::title(ui, s().light_color_mode);
        ui.add_enabled_ui(usable, |ui| {
            if let Some(i) =
                widgets::segmented(ui, &[s().light_single, s().light_multi], l.multi as usize)
            {
                l.multi = i == 1;
            }
        });
        ui.add_space(theme::SP_RELATED);
        ui.horizontal(|ui| {
            widgets::title(ui, s().light_color);
            widgets::swatch(ui, l.color, 18.0);
            widgets::hint(ui, &led::format_hex_color(l.color));
        });
        if !usable {
            widgets::hint(ui, s().light_auto_color);
            return;
        }
        if colour_picker(ui, &mut l.color, "effect") {
            // As in the hub: picking a colour selects single-colour mode.
            l.multi = false;
        }
        ui.add_space(theme::SP_CONTROL);
        match unknown {
            None => widgets::notice(ui, Tone::Info, s().light_colour_known, None),
            Some(r) => widgets::notice(
                ui,
                Tone::Warn,
                &fill(s().light_colour_unknown, &[("reason", r)]),
                None,
            ),
        };
    }

    fn painter_column(&mut self, ui: &mut Ui) {
        widgets::title(ui, s().light_brush);
        let mut brush = self.lighting.brush;
        if colour_picker(ui, &mut brush, "brush") {
            self.lighting.brush = brush;
        }
        ui.add_space(theme::SP_RELATED);
        let n = self.lighting.selection.len();
        widgets::hint(
            ui,
            &if n == 0 {
                s().light_custom_hint.to_string()
            } else {
                fill(s().selected_count, &[("n", &n.to_string())])
            },
        );
        let mut paint: Option<[u8; 3]> = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            ui.add_enabled_ui(n > 0, |ui| {
                if widgets::button(ui, s().light_paint).clicked() {
                    paint = Some(brush);
                }
                if widgets::button(ui, s().light_black).clicked() {
                    paint = Some([0, 0, 0]);
                }
            });
            if widgets::ghost_button(ui, s().light_select_all).clicked() {
                self.lighting.selection = type84_proto::layout::keys()
                    .iter()
                    .map(|k| k.index)
                    .collect();
            }
            if widgets::ghost_button(ui, s().light_clear).clicked() {
                self.lighting.selection.clear();
            }
        });
        if let (Some(c), Some(edits)) = (paint, &mut self.edits) {
            self.lighting.selection.iter().for_each(|i| {
                edits.custom.insert(*i, c);
            });
            edits.light.mode = 128;
        }
    }
}
