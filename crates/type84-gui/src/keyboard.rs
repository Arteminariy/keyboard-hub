//! Keyboard drawing: the small thumbnail on the device card and the
//! interactive editor scheme on the "Отклик" tab (click, marquee drag,
//! per-key values).

use std::collections::{BTreeMap, BTreeSet};

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui,
};

use crate::geometry::{self, KeyGeom};
use crate::i18n;
use crate::model::KeyHall;
use crate::theme::{self, font, Weight};

fn key_rect(origin: Pos2, k: &KeyGeom, u: f32, gap: f32) -> Rect {
    Rect::from_min_size(
        pos2(
            origin.x + k.x * u + gap / 2.0,
            origin.y + k.y * u + gap / 2.0,
        ),
        vec2(k.w * u - gap, k.h * u - gap),
    )
}

/// Paint a keycap body.
fn cap(p: &egui::Painter, r: Rect, radius: u8, fill: Color32, edge: Color32) {
    let cr = CornerRadius::same(radius);
    p.rect(r, cr, fill, Stroke::new(1.0, edge), StrokeKind::Inside);
    if fill != theme::ACTIVE {
        // Inset bottom shade from --glass-key-shadow.
        let shade = Rect::from_min_max(
            pos2(r.left() + 2.0, r.bottom() - 2.5),
            pos2(r.right() - 2.0, r.bottom() - 1.0),
        );
        p.rect_filled(shade, CornerRadius::same(1), Color32::from_black_alpha(70));
    }
}

/// Short card legend from the KLE text: "base shifted" on one line.
fn card_legend(k: &KeyGeom) -> String {
    match k.legend.as_slice() {
        [shifted, base] => format!("{base} {shifted}"),
        [one] => one.clone(),
        _ => String::new(),
    }
}

/// Static thumbnail for the device card, `width` px wide.
pub fn card_thumbnail(ui: &mut Ui, width: f32) -> Response {
    let (wu, hu) = geometry::size();
    let pad = 8.0;
    let u = (width - 2.0 * pad) / wu;
    let size = vec2(width, hu * u + 2.0 * pad);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        CornerRadius::same(theme::R_TOOL),
        theme::PLATE,
        Stroke::new(1.0, theme::KEY_EDGE),
        StrokeKind::Inside,
    );
    let origin = rect.min + vec2(pad, pad);
    let gap = (u * 0.12).max(2.0);
    let fsize = (u * 0.30).clamp(7.0, 11.0);
    for k in geometry::keys() {
        let r = key_rect(origin, k, u, gap);
        cap(
            p,
            r,
            4,
            Color32::from_rgb(0x1d, 0x1e, 0x21),
            Color32::from_rgb(0x56, 0x57, 0x5a),
        );
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            card_legend(k),
            font(fsize, Weight::Regular),
            theme::FG,
        );
    }
    resp
}

/// What the user did on the editor scheme this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyboardAction {
    Click(u8),
    Marquee(Vec<u8>),
}

/// Inputs for the editor scheme of the "Отклик" tab.
pub struct EditorView<'a> {
    pub keys: &'a BTreeMap<u8, KeyHall>,
    pub selection: &'a BTreeSet<u8>,
    pub changed: &'a dyn Fn(u8) -> bool,
    pub enabled: bool,
}

/// Interactive Hall scheme: default legends plus per-key values.
pub fn editor(ui: &mut Ui, view: &EditorView) -> Option<KeyboardAction> {
    let face = |i: u8| {
        let (main, sub) = i18n::key_legend(i);
        KeyFace {
            main: main.to_string(),
            sub: sub.to_string(),
            ..Default::default()
        }
    };
    scheme(
        ui,
        &SchemeView {
            face: &face,
            selection: view.selection,
            changed: view.changed,
            enabled: view.enabled,
            hall: Some(view.keys),
        },
    )
}

/// How one key is drawn on a [`scheme`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyFace {
    pub main: String,
    /// Small second legend (Cyrillic letter, or the base key on the Fn layer).
    pub sub: String,
    /// Bold main legend (differs from the factory assignment).
    pub bold: bool,
    /// Key colour (lighting); the legend switches to dark on light fills.
    pub fill: Option<Color32>,
    /// Drawn with a lock glyph and dimmed (not reassignable).
    pub locked: bool,
    /// Draw `sub` small in the top-left corner instead of next to `main`.
    pub sub_corner: bool,
}

/// Inputs for a generic keyboard scheme.
pub struct SchemeView<'a> {
    pub face: &'a dyn Fn(u8) -> KeyFace,
    pub selection: &'a BTreeSet<u8>,
    pub changed: &'a dyn Fn(u8) -> bool,
    pub enabled: bool,
    /// Per-key Hall values drawn in the corners ("Отклик").
    pub hall: Option<&'a BTreeMap<u8, KeyHall>>,
}

fn luminance(c: Color32) -> f32 {
    0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32
}

fn draw_lock(p: &egui::Painter, c: Pos2, color: Color32) {
    let body = Rect::from_center_size(c + vec2(0.0, 1.5), vec2(8.0, 6.0));
    p.rect_filled(body, CornerRadius::same(1), color);
    p.circle_stroke(c + vec2(0.0, -2.0), 2.6, Stroke::new(1.3, color));
}

/// Interactive scheme filling the available width (max 1u = 68 px): click,
/// marquee drag, selection, changed dots, optional colours and values.
pub fn scheme(ui: &mut Ui, view: &SchemeView) -> Option<KeyboardAction> {
    let (wu, hu) = geometry::size();
    let u = (ui.available_width() / wu).min(68.0);
    let size = vec2(wu * u, hu * u);
    let sense = if view.enabled {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let origin = rect.min;
    let gap = (u * 0.075).max(3.0);
    let rects: Vec<(u8, Rect)> = geometry::keys()
        .iter()
        .map(|k| (k.index, key_rect(origin, k, u, gap)))
        .collect();
    let hit = |pos: Pos2| {
        rects
            .iter()
            .find(|(_, r)| r.expand(gap / 2.0).contains(pos))
            .map(|(i, _)| *i)
    };

    // Marquee state lives in egui memory, keyed by the widget id.
    let drag_id = resp.id.with("type84-marquee");
    let mut action = None;
    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            ui.data_mut(|d| d.insert_temp(drag_id, pos));
        }
    }
    let origin_pos: Option<Pos2> = ui.data(|d| d.get_temp(drag_id));
    let marquee = match (
        origin_pos,
        resp.interact_pointer_pos()
            .or(ui.ctx().pointer_latest_pos()),
    ) {
        (Some(a), Some(b)) if resp.dragged() || resp.drag_stopped() => {
            Some(Rect::from_two_pos(a, b))
        }
        _ => None,
    };
    let in_marquee: BTreeSet<u8> = marquee
        .map(|m| {
            rects
                .iter()
                .filter(|(_, r)| r.intersects(m))
                .map(|(i, _)| *i)
                .collect()
        })
        .unwrap_or_default();
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<Pos2>(drag_id));
        action = Some(KeyboardAction::Marquee(
            in_marquee.iter().copied().collect(),
        ));
    } else if resp.clicked() {
        if let Some(i) = resp.interact_pointer_pos().and_then(hit) {
            action = Some(KeyboardAction::Click(i));
        }
    }
    let hovered = if view.enabled {
        resp.hover_pos().and_then(hit)
    } else {
        None
    };

    let p = ui.painter();
    let main_size = (u * 0.27).clamp(11.0, 18.0);
    let sub_size = (main_size * 0.72).max(9.0);
    let val_size = (u * 0.165).clamp(8.0, 11.0);
    for k in geometry::keys() {
        let r = key_rect(origin, k, u, gap);
        let selected = view.selection.contains(&k.index) || in_marquee.contains(&k.index);
        let face = (view.face)(k.index);
        let (fill, edge, mut ink, mut ink2) = if selected {
            (
                theme::ACTIVE,
                theme::ACTIVE,
                theme::ON_ACTIVE,
                Color32::from_rgb(0x4a, 0x4e, 0x56),
            )
        } else if hovered == Some(k.index) {
            (theme::KEY_HOVER, theme::KEY_EDGE, theme::FG, theme::MUTED)
        } else {
            (theme::KEY_FACE, theme::KEY_EDGE, theme::FG, theme::MUTED)
        };
        let fill = match face.fill {
            Some(c) => {
                if luminance(c) > 150.0 {
                    ink = theme::ON_ACTIVE;
                    ink2 = Color32::from_rgb(0x30, 0x33, 0x3a);
                } else {
                    ink = theme::FG;
                    ink2 = theme::MUTED;
                }
                c
            }
            None => fill,
        };
        let edge = if selected && face.fill.is_some() {
            theme::ACTIVE
        } else {
            edge
        };
        cap(p, r, theme::R_KEY, fill, edge);
        if selected && face.fill.is_some() {
            // Selection ring around a coloured key.
            p.rect_stroke(
                r.expand(1.5),
                CornerRadius::same(theme::R_KEY + 1),
                Stroke::new(2.0, theme::ACTIVE),
                StrokeKind::Outside,
            );
        }
        if face.locked {
            ink = theme::mix(ink, fill, 0.45);
            ink2 = theme::mix(ink2, fill, 0.45);
            draw_lock(p, r.right_top() + vec2(-u * 0.14, u * 0.16), ink2);
        }

        // Main legend + sub-legend, centred together.
        let (main, sub) = (face.main.as_str(), face.sub.as_str());
        let mweight = if face.bold || view.hall.is_some() {
            Weight::Semibold
        } else {
            Weight::Medium
        };
        let space = 4.0;
        let max_w = r.width() - 8.0;
        let g_sub = (!sub.is_empty()).then(|| {
            let size = if face.sub_corner { val_size } else { sub_size };
            p.layout_no_wrap(sub.to_string(), font(size, Weight::Regular), ink2)
        });
        let beside = if face.sub_corner {
            0.0
        } else {
            g_sub.as_ref().map(|g| g.size().x + space).unwrap_or(0.0)
        };
        // Shrink the main legend until it fits the cap.
        let mut msize = main_size;
        let mut g_main = p.layout_no_wrap(main.to_string(), font(msize, mweight), ink);
        while g_main.size().x + beside > max_w && msize > 7.5 {
            msize -= 0.5;
            g_main = p.layout_no_wrap(main.to_string(), font(msize, mweight), ink);
        }
        let cy = r.center().y + u * 0.04;
        let main_w = g_main.size().x;
        let main_h = g_main.size().y;
        if face.sub_corner || g_main.size().x + beside > max_w {
            p.galley(
                pos2(r.center().x - main_w / 2.0, cy - main_h / 2.0 + u * 0.06),
                g_main,
                ink,
            );
            if let Some(g) = g_sub {
                let pad = vec2(u * 0.06 + 1.0, u * 0.03);
                p.galley(r.left_top() + pad, g, ink2);
            }
        } else {
            let total = main_w + beside;
            let x = r.center().x - total / 2.0;
            p.galley(pos2(x, cy - main_h / 2.0), g_main, ink);
            if let Some(g) = g_sub {
                // Align the sub-legend baseline with the main legend.
                let y = cy + main_h / 2.0 - g.size().y - (msize - sub_size) * 0.12;
                p.galley(pos2(x + main_w + space, y), g, ink2);
            }
        }

        // Per-key values: actuation top-left; RT press/release bottom.
        if let Some(h) = view.hall.and_then(|m| m.get(&k.index)) {
            let pad = vec2(u * 0.06 + 1.0, u * 0.03);
            let vf = font(val_size, Weight::Regular);
            p.text(
                r.left_top() + pad,
                Align2::LEFT_TOP,
                i18n::mm_dot(h.actuation),
                vf.clone(),
                ink2,
            );
            if h.rt {
                p.text(
                    r.left_bottom() + vec2(pad.x, -pad.y - 1.0),
                    Align2::LEFT_BOTTOM,
                    i18n::mm_dot(h.press),
                    vf.clone(),
                    ink2,
                );
                p.text(
                    r.right_bottom() + vec2(-pad.x, -pad.y - 1.0),
                    Align2::RIGHT_BOTTOM,
                    i18n::mm_dot(h.release),
                    vf,
                    ink2,
                );
            }
        }
        if (view.changed)(k.index) {
            p.circle_filled(r.right_top() + vec2(-u * 0.1, u * 0.1), 3.0, theme::ACCENT);
        }
    }
    if let Some(m) = marquee {
        p.rect(
            m,
            CornerRadius::same(2),
            Color32::from_white_alpha(18),
            Stroke::new(1.0, theme::MUTED),
            StrokeKind::Inside,
        );
    }
    action
}
