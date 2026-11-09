//! Custom widgets drawn to resemble the vendor hub (UI_SPEC §2 "Control
//! anatomy"): toggle, range slider, value box, buttons, notices, panels.

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, Frame, Margin, Pos2, Rect, Response, Role, Sense,
    Shape, Stroke, StrokeKind, Ui, Vec2, WidgetInfo,
};

use crate::i18n;
use crate::theme::{self, font, Weight};

/// Tri-state for toggles over a mixed key selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tri {
    Off,
    On,
    Mixed,
}

impl Tri {
    pub fn from_bools(mut it: impl Iterator<Item = bool>) -> Tri {
        let Some(first) = it.next() else {
            return Tri::Off;
        };
        if it.all(|b| b == first) {
            if first {
                Tri::On
            } else {
                Tri::Off
            }
        } else {
            Tri::Mixed
        }
    }
}

/// Toggle switch, 50x28: grey knob with a dash when off, white knob with a
/// check when on, knob centred when mixed. Returns `changed()` on click;
/// the caller flips its value.
pub fn toggle(ui: &mut Ui, state: Tri, label: &str) -> Response {
    let size = vec2(
        2.0 * theme::THUMB + 2.0 * theme::CONTROL_INSET,
        theme::TRACK,
    );
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        resp.mark_changed();
    }
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, enabled, state == Tri::On, label));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    let fill = if resp.hovered() && enabled {
        theme::CONTROL_HOVER
    } else {
        theme::CONTROL
    };
    p.rect(
        rect,
        CornerRadius::same(theme::R_CONTROL),
        fill,
        Stroke::new(1.0, theme::CONTROL_BORDER),
        StrokeKind::Inside,
    );
    let inner = rect.shrink(theme::CONTROL_INSET);
    let knob_w = theme::THUMB;
    let x = match state {
        Tri::Off => inner.left(),
        Tri::Mixed => inner.center().x - knob_w / 2.0,
        Tri::On => inner.right() - knob_w,
    };
    let knob = Rect::from_min_size(pos2(x, inner.top()), vec2(knob_w, inner.height()));
    let knob_fill = match state {
        Tri::On => theme::ACTIVE,
        Tri::Off => Color32::from_rgb(0x8e, 0x8f, 0x91),
        Tri::Mixed => Color32::from_rgb(0xb8, 0xb9, 0xbb),
    };
    p.rect_filled(knob, CornerRadius::same(theme::R_CONTROL - 3), knob_fill);
    let c = knob.center();
    let ink = Stroke::new(1.6, theme::ON_ACTIVE);
    match state {
        Tri::On => {
            p.add(Shape::line(
                vec![
                    c + vec2(-4.0, 0.0),
                    c + vec2(-1.2, 2.8),
                    c + vec2(4.0, -3.0),
                ],
                ink,
            ));
        }
        Tri::Off | Tri::Mixed => {
            p.line_segment([c - vec2(3.5, 0.0), c + vec2(3.5, 0.0)], ink);
        }
    }
    resp
}

/// Range slider: 28px track with a 1px border, no progress fill, faint
/// ticks at the ends and every whole millimetre, 22px white square thumb.
/// `value` is in hundredths of a mm. Returns the response; `changed()` is
/// set while dragging.
pub fn range_slider(
    ui: &mut Ui,
    value: &mut u16,
    min: u16,
    max: u16,
    width: f32,
    label: &str,
) -> Response {
    let (rect, mut resp) =
        ui.allocate_exact_size(vec2(width, theme::TRACK), Sense::click_and_drag());
    let enabled = ui.is_enabled();
    let inner = rect.shrink(theme::CONTROL_INSET);
    let x0 = inner.left() + theme::THUMB / 2.0;
    let x1 = inner.right() - theme::THUMB / 2.0;
    let to_x = |v: u16| x0 + (x1 - x0) * (v.saturating_sub(min)) as f32 / (max - min) as f32;
    if enabled {
        if let Some(pos) = resp.interact_pointer_pos() {
            if resp.dragged() || resp.clicked() {
                let t = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0);
                let v = (min as f32 + t * (max - min) as f32).round() as u16;
                if v != *value {
                    *value = v;
                    resp.mark_changed();
                }
            }
        }
    }
    let v = *value;
    resp.widget_info(|| WidgetInfo::slider(enabled, v as f64 / 100.0, label));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    let track_fill = theme::VALUE_BOX;
    p.rect(
        rect,
        CornerRadius::same(theme::R_CONTROL),
        track_fill,
        Stroke::new(1.0, theme::CONTROL_BORDER),
        StrokeKind::Inside,
    );
    let tick = Stroke::new(1.0, Color32::from_rgb(0x5a, 0x5b, 0x5e));
    let mut ticks = vec![min, max];
    ticks.extend((1..=3).map(|m| m * 100).filter(|t| *t > min && *t < max));
    for t in ticks {
        let x = to_x(t).round() + 0.5;
        p.line_segment(
            [
                pos2(x, rect.center().y - 4.0),
                pos2(x, rect.center().y + 4.0),
            ],
            tick,
        );
    }
    let cx = to_x(v.clamp(min, max));
    let thumb = Rect::from_center_size(pos2(cx, rect.center().y), Vec2::splat(theme::THUMB));
    let thumb_fill = if enabled {
        theme::ACTIVE
    } else {
        Color32::from_rgb(0x8e, 0x8f, 0x91)
    };
    p.rect_filled(thumb, CornerRadius::same(theme::R_CONTROL - 3), thumb_fill);
    resp
}

/// Numeric box (68px, right-aligned, decimal comma) followed by the unit.
/// Click to type, drag to scrub. `value` in hundredths of a mm.
pub fn value_box(ui: &mut Ui, value: &mut u16, min: u16, max: u16, label: egui::Id) -> Response {
    let mut v = *value as f64 / 100.0;
    let r = ui
        .scope(|ui| {
            let vis = &mut ui.style_mut().visuals.widgets;
            for w in [&mut vis.inactive, &mut vis.hovered, &mut vis.active] {
                w.bg_fill = theme::VALUE_BOX;
                w.weak_bg_fill = theme::VALUE_BOX;
            }
            vis.inactive.bg_stroke = Stroke::new(1.0, theme::CONTROL_BORDER);
            ui.style_mut().override_font_id = Some(font(theme::FS_BODY, Weight::Regular));
            ui.add_sized(
                vec2(68.0, 30.0),
                egui::DragValue::new(&mut v)
                    .range(min as f64 / 100.0..=max as f64 / 100.0)
                    .clamp_existing_to_range(false)
                    .speed(0.01)
                    .custom_formatter(|n, _| i18n::mm_comma((n * 100.0).round().max(0.0) as u16))
                    .custom_parser(|s| s.trim().replace(',', ".").parse::<f64>().ok()),
            )
        })
        .inner
        .labelled_by(label);
    let nv = crate::model::to_hundredths(v, min, max);
    let mut r = r;
    // Only user input counts; a stored value outside the range (e.g. 0 when
    // RT is off) must not turn into an edit just by being displayed.
    if r.changed() && nv != *value {
        *value = nv;
        r.mark_changed();
    }
    r
}

/// Small unit label after a value box.
pub fn unit(ui: &mut Ui) {
    ui.label(
        egui::RichText::new(i18n::s().mm)
            .font(font(theme::FS_HINT, Weight::Regular))
            .color(theme::HINT),
    );
}

/// Primary (white) button.
pub fn primary_button(ui: &mut Ui, text: &str, min_w: f32) -> Response {
    // Grow atoms on both sides centre the label inside the min width.
    let label = egui::RichText::new(text)
        .color(theme::ON_ACTIVE)
        .font(font(theme::FS_BODY, Weight::Medium));
    let b = egui::Button::new((egui::Atom::grow(), label, egui::Atom::grow()))
        .fill(theme::ACTIVE)
        .stroke(Stroke::new(1.0, theme::ACTIVE))
        .corner_radius(CornerRadius::same(theme::R_CONTROL))
        .min_size(vec2(min_w, theme::BUTTON_H));
    ui.add(b)
}

/// Regular button (panel2 fill, 1px border).
pub fn button(ui: &mut Ui, text: &str) -> Response {
    let b = egui::Button::new(egui::RichText::new(text).font(font(theme::FS_HINT, Weight::Medium)))
        .fill(theme::CONTROL)
        .stroke(Stroke::new(1.0, theme::CONTROL_BORDER))
        .corner_radius(CornerRadius::same(theme::R_CONTROL))
        .min_size(vec2(0.0, theme::BUTTON_H));
    ui.add(b)
}

/// Frameless text button (group chips, ghost header actions).
pub fn ghost_button(ui: &mut Ui, text: &str) -> Response {
    let b = egui::Button::new(egui::RichText::new(text).font(font(theme::FS_HINT, Weight::Medium)))
        .frame_when_inactive(false)
        .corner_radius(CornerRadius::same(theme::R_CONTROL))
        .min_size(vec2(0.0, theme::BUTTON_H));
    ui.add(b)
}

/// Navigation tab: white pill when active.
pub fn tab_button(ui: &mut Ui, text: &str, active: bool) -> Response {
    let rt = egui::RichText::new(text).font(font(theme::FS_BODY, Weight::Medium));
    let b = if active {
        egui::Button::new(rt.color(theme::ON_ACTIVE))
            .fill(theme::ACTIVE)
            .stroke(Stroke::NONE)
    } else {
        egui::Button::new(rt).frame_when_inactive(false)
    };
    ui.add(
        b.corner_radius(CornerRadius::same(theme::R_CONTROL))
            .min_size(vec2(0.0, theme::BUTTON_H)),
    )
}

/// Which painter-drawn icon an [`icon_button`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Close,
    AddToSelection,
}

/// Square 36x36 icon button; `active` draws the white pressed state.
pub fn icon_button(ui: &mut Ui, icon: Icon, active: bool, label: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(theme::BUTTON_H), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(Role::Button, enabled, active, label));
    let p = ui.painter();
    let (fill, ink) = if active {
        (theme::ACTIVE, theme::ON_ACTIVE)
    } else if resp.hovered() {
        (theme::CONTROL_HOVER, theme::FG)
    } else {
        (Color32::TRANSPARENT, theme::FG)
    };
    p.rect_filled(rect, CornerRadius::same(theme::R_CONTROL), fill);
    let c = rect.center();
    let s = Stroke::new(1.5, ink);
    match icon {
        Icon::Close => {
            let d = 5.5;
            p.line_segment([c + vec2(-d, -d), c + vec2(d, d)], s);
            p.line_segment([c + vec2(-d, d), c + vec2(d, -d)], s);
        }
        Icon::AddToSelection => {
            let r = Rect::from_center_size(c, Vec2::splat(14.0));
            p.rect_stroke(
                r,
                CornerRadius::same(3),
                Stroke::new(1.3, ink),
                StrokeKind::Middle,
            );
            p.line_segment([c - vec2(3.5, 0.0), c + vec2(3.5, 0.0)], s);
            p.line_segment([c - vec2(0.0, 3.5), c + vec2(0.0, 3.5)], s);
        }
    }
    resp.on_hover_text(label)
}

/// Small "↺" reset glyph button next to a label.
pub fn reset_button(ui: &mut Ui, tooltip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, tooltip));
    let color = if resp.hovered() {
        theme::FG
    } else {
        theme::MUTED
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "↺",
        font(theme::FS_HINT, Weight::Medium),
        color,
    );
    resp.on_hover_text(tooltip)
}

/// Section title (14px, 650).
pub fn title(ui: &mut Ui, text: &str) -> Response {
    ui.label(
        egui::RichText::new(text)
            .font(font(theme::FS_BODY, Weight::Semibold))
            .color(theme::FG),
    )
}

/// Hint text (13px, muted), wrapping.
pub fn hint(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(font(theme::FS_HINT, Weight::Regular))
                .color(theme::HINT),
        )
        .wrap(),
    )
}

/// Rounded panel with the hub's 1px light border.
pub fn panel_frame() -> Frame {
    Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, theme::PANEL_BORDER))
        .corner_radius(CornerRadius::same(theme::R_PANEL))
        .inner_margin(Margin::same(theme::PANEL_INSET as i8))
}

/// Notice tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Error,
    Warn,
    Info,
    Success,
}

/// Inline notice (UI_SPEC §2): tone 28% border, tone 9% fill, icon then
/// text, 14px.
pub fn notice(ui: &mut Ui, tone: Tone, text: &str, detail: Option<&str>) -> Response {
    let tc = match tone {
        Tone::Error => theme::DANGER_TEXT,
        Tone::Warn => theme::WARN,
        Tone::Info => theme::MUTED,
        Tone::Success => theme::GOOD,
    };
    let fill = theme::mix(theme::CONTROL, tc, 0.09);
    let border = theme::mix(theme::BG, tc, 0.45);
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(CornerRadius::same(theme::R_TOOL))
        .inner_margin(Margin::symmetric(14, 11))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                draw_alert_icon(ui, r.center(), tc, tone);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let weight = if matches!(tone, Tone::Warn | Tone::Success) {
                        Weight::Semibold
                    } else {
                        Weight::Regular
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .font(font(theme::FS_BODY, weight))
                                .color(if tone == Tone::Error {
                                    theme::mix(theme::FG, tc, 0.35)
                                } else if matches!(tone, Tone::Warn | Tone::Success) {
                                    theme::FG
                                } else {
                                    tc
                                }),
                        )
                        .wrap(),
                    );
                    if let Some(d) = detail {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(d)
                                    .font(font(theme::FS_HINT, Weight::Regular))
                                    .color(theme::HINT),
                            )
                            .wrap(),
                        );
                    }
                });
            });
        })
        .response
}

/// Circled "!" (error/info) or triangle "!" (warning).
fn draw_alert_icon(ui: &Ui, c: Pos2, color: Color32, tone: Tone) {
    let p = ui.painter();
    let s = Stroke::new(1.5, color);
    if tone == Tone::Success {
        p.circle_stroke(c, 8.0, s);
        p.add(Shape::line(
            vec![
                c + vec2(-3.8, 0.2),
                c + vec2(-1.0, 3.0),
                c + vec2(4.0, -2.8),
            ],
            s,
        ));
    } else if tone == Tone::Warn {
        let pts = vec![c + vec2(0.0, -8.0), c + vec2(8.5, 7.0), c + vec2(-8.5, 7.0)];
        p.add(Shape::closed_line(pts, s));
        p.line_segment([c + vec2(0.0, -2.5), c + vec2(0.0, 2.0)], s);
        p.circle_filled(c + vec2(0.0, 4.6), 0.9, color);
    } else {
        p.circle_stroke(c, 8.0, s);
        p.line_segment([c + vec2(0.0, -4.0), c + vec2(0.0, 1.0)], s);
        p.circle_filled(c + vec2(0.0, 3.8), 0.9, color);
    }
}

/// Segmented control (toggle buttons): white pressed segment. Returns the
/// clicked index.
pub fn segmented(ui: &mut Ui, options: &[&str], selected: usize) -> Option<usize> {
    let mut out = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, o) in options.iter().enumerate() {
            let on = i == selected;
            let rt = egui::RichText::new(*o).font(font(theme::FS_HINT, Weight::Medium));
            let b = if on {
                egui::Button::new(rt.color(theme::ON_ACTIVE))
                    .fill(theme::ACTIVE)
                    .stroke(Stroke::new(1.0, theme::ACTIVE))
            } else {
                egui::Button::new(rt)
                    .fill(theme::CONTROL)
                    .stroke(Stroke::new(1.0, theme::CONTROL_BORDER))
            };
            let r = ui.add(
                b.corner_radius(CornerRadius::same(theme::R_CONTROL))
                    .min_size(vec2(0.0, 32.0)),
            );
            if r.clicked() {
                out = Some(i);
            }
        }
    });
    out
}

/// Step slider 0..=max with one tick per step (brightness, speed).
pub fn step_slider(ui: &mut Ui, value: &mut u8, max: u8, width: f32, label: &str) -> Response {
    let (rect, mut resp) =
        ui.allocate_exact_size(vec2(width, theme::TRACK), Sense::click_and_drag());
    let enabled = ui.is_enabled();
    let inner = rect.shrink(theme::CONTROL_INSET);
    let x0 = inner.left() + theme::THUMB / 2.0;
    let x1 = inner.right() - theme::THUMB / 2.0;
    let to_x = |v: u8| x0 + (x1 - x0) * v as f32 / max.max(1) as f32;
    if enabled {
        if let Some(pos) = resp.interact_pointer_pos() {
            if resp.dragged() || resp.clicked() {
                let t = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0);
                let v = (t * max as f32).round() as u8;
                if v != *value {
                    *value = v;
                    resp.mark_changed();
                }
            }
        }
    }
    let v = *value;
    resp.widget_info(|| WidgetInfo::slider(enabled, v as f64, label));
    let p = ui.painter();
    p.rect(
        rect,
        CornerRadius::same(theme::R_CONTROL),
        theme::VALUE_BOX,
        Stroke::new(1.0, theme::CONTROL_BORDER),
        StrokeKind::Inside,
    );
    let tick = Stroke::new(1.0, Color32::from_rgb(0x5a, 0x5b, 0x5e));
    for t in 0..=max {
        let x = to_x(t).round() + 0.5;
        p.line_segment(
            [
                pos2(x, rect.center().y - 4.0),
                pos2(x, rect.center().y + 4.0),
            ],
            tick,
        );
    }
    let thumb = Rect::from_center_size(
        pos2(to_x(v.min(max)), rect.center().y),
        Vec2::splat(theme::THUMB),
    );
    let fill = if enabled {
        theme::ACTIVE
    } else {
        Color32::from_rgb(0x8e, 0x8f, 0x91)
    };
    p.rect_filled(thumb, CornerRadius::same(theme::R_CONTROL - 3), fill);
    resp
}

/// A small colour square.
pub fn swatch(ui: &mut Ui, c: [u8; 3], size: f32) -> Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    ui.painter().rect(
        r,
        CornerRadius::same(4),
        Color32::from_rgb(c[0], c[1], c[2]),
        Stroke::new(1.0, theme::CONTROL_BORDER),
        StrokeKind::Inside,
    );
    resp
}

/// Modal dialog frame in the hub style.
pub fn modal_frame() -> Frame {
    Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, theme::PANEL_BORDER))
        .corner_radius(CornerRadius::same(theme::R_PANEL))
        .inner_margin(Margin::same(24))
}

fn rgb_to_hsv(c: [u8; 3]) -> (f32, f32, f32) {
    let [r, g, b] = c.map(|x| x as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

/// HSV (all 0..1) to RGB.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r, g, b].map(|t| ((t + m) * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// Colour picker in the hub's layout: saturation/brightness field above a
/// hue strip («Насыщенность и яркость», «Оттенок»). Returns true when the
/// colour changed.
pub fn colour_picker(ui: &mut Ui, rgb: &mut [u8; 3], id_salt: &str, width: f32) -> bool {
    let id = ui.scope_id().with(("colour-picker", id_salt));
    let (mut h, mut s, mut v) = rgb_to_hsv(*rgb);
    // Keep the hue across grey/black colours, where RGB loses it.
    if let Some((c, hue)) = ui.data(|d| d.get_temp::<([u8; 3], f32)>(id)) {
        if c == *rgb {
            h = hue;
        }
    }
    let mut changed = false;
    let p_rect = |ui: &mut Ui, size: Vec2, label: &str| {
        let (r, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let enabled = ui.is_enabled();
        resp.widget_info(|| WidgetInfo::labeled(Role::Slider, enabled, label));
        (r, resp)
    };
    // Saturation / value field.
    let (r, resp) = p_rect(ui, vec2(width, 150.0), i18n::s().light_color);
    if let Some(pos) = resp
        .interact_pointer_pos()
        .filter(|_| resp.dragged() || resp.clicked())
    {
        s = ((pos.x - r.left()) / r.width()).clamp(0.0, 1.0);
        v = 1.0 - ((pos.y - r.top()) / r.height()).clamp(0.0, 1.0);
        changed = true;
    }
    let hue_c = hsv_to_rgb(h, 1.0, 1.0);
    let p = ui.painter();
    {
        use egui::epaint::{Mesh, Vertex, WHITE_UV};
        let c32 = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
        // A grid keeps the gradient close to real HSV.
        let n = 8;
        let mut mesh = Mesh::default();
        for j in 0..=n {
            for i in 0..=n {
                let (fs, fv) = (i as f32 / n as f32, 1.0 - j as f32 / n as f32);
                mesh.vertices.push(Vertex {
                    pos: pos2(r.left() + fs * r.width(), r.top() + (1.0 - fv) * r.height()),
                    uv: WHITE_UV,
                    color: c32(hsv_to_rgb(h, fs, fv)),
                });
            }
        }
        for j in 0..n {
            for i in 0..n {
                let a = (j * (n + 1) + i) as u32;
                let b = a + 1;
                let c = a + (n + 1) as u32;
                let d = c + 1;
                mesh.add_triangle(a, b, c);
                mesh.add_triangle(b, d, c);
            }
        }
        p.add(Shape::mesh(mesh));
        p.rect_stroke(
            r,
            CornerRadius::same(theme::R_CONTROL),
            Stroke::new(1.0, theme::CONTROL_BORDER),
            StrokeKind::Outside,
        );
        let m = pos2(r.left() + s * r.width(), r.top() + (1.0 - v) * r.height());
        p.circle_stroke(m, 7.0, Stroke::new(2.0, Color32::WHITE));
        p.circle_stroke(m, 8.5, Stroke::new(1.0, Color32::from_black_alpha(160)));

        ui.add_space(theme::SP_CONTROL);
        // Hue strip.
        let (hr, hresp) = p_rect(ui, vec2(width, 22.0), "Оттенок");
        if let Some(pos) = hresp
            .interact_pointer_pos()
            .filter(|_| hresp.dragged() || hresp.clicked())
        {
            h = ((pos.x - hr.left()) / hr.width()).clamp(0.0, 0.9999);
            changed = true;
        }
        let p = ui.painter();
        let mut mesh = Mesh::default();
        let steps = 24;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let c = c32(hsv_to_rgb(t, 1.0, 1.0));
            let x = hr.left() + t * hr.width();
            mesh.vertices.push(Vertex {
                pos: pos2(x, hr.top()),
                uv: WHITE_UV,
                color: c,
            });
            mesh.vertices.push(Vertex {
                pos: pos2(x, hr.bottom()),
                uv: WHITE_UV,
                color: c,
            });
        }
        for i in 0..steps as u32 {
            let a = i * 2;
            mesh.add_triangle(a, a + 1, a + 2);
            mesh.add_triangle(a + 1, a + 3, a + 2);
        }
        p.add(Shape::mesh(mesh));
        p.rect_stroke(
            hr,
            CornerRadius::same(4),
            Stroke::new(1.0, theme::CONTROL_BORDER),
            StrokeKind::Outside,
        );
        let x = hr.left() + h * hr.width();
        let thumb = Rect::from_center_size(pos2(x, hr.center().y), vec2(8.0, hr.height() + 6.0));
        p.rect(
            thumb,
            CornerRadius::same(3),
            c32(hue_c),
            Stroke::new(2.0, Color32::WHITE),
            StrokeKind::Middle,
        );
    }
    if changed {
        *rgb = hsv_to_rgb(h, s, v);
        let c = *rgb;
        ui.data_mut(|d| d.insert_temp(id, (c, h)));
    }
    changed
}
