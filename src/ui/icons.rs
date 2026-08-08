//! Vector icons drawn straight onto the painter.
//!
//! The mockup leans on glyphs like ⬚ ⛶ ⌘ ◑ that no single font ships in full —
//! on a machine missing one you get a tofu box in the middle of the HUD. These
//! are shapes instead: always present, always crisp, and they scale with the
//! rect they are handed.
//!
//! Every function fills `rect` (assumed square-ish) and takes the stroke colour.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use std::f32::consts::TAU;

fn lerp_rect(rect: Rect, fx: f32, fy: f32) -> Pos2 {
    Pos2::new(
        rect.min.x + rect.width() * fx,
        rect.min.y + rect.height() * fy,
    )
}

fn line(p: &Painter, rect: Rect, a: (f32, f32), b: (f32, f32), stroke: Stroke) {
    p.line_segment(
        [lerp_rect(rect, a.0, a.1), lerp_rect(rect, b.0, b.1)],
        stroke,
    );
}

fn stroke_for(rect: Rect, color: Color32) -> Stroke {
    Stroke::new((rect.width() * 0.09).clamp(1.0, 2.2), color)
}

/// The brand mark: five petals around a pale centre.
///
/// Geometry comes from `shared::mark`, the same source the build script
/// rasterises into the executable's icon — so the flower beside "Sakura" in the
/// window and the one in the tray are the same shape, not two drawings of it.
pub fn sakura(p: &Painter, rect: Rect, color: Color32) {
    sakura_spinning(p, rect, color, 0.0, 1.0);
}

/// The mark, turned by `turns` and scaled by `scale` — the working animation.
pub fn sakura_spinning(p: &Painter, rect: Rect, color: Color32, turns: f32, scale: f32) {
    use crate::shared::mark;

    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5 / mark::extent() * scale.clamp(0.2, 1.0);

    for disc in mark::rotated_petals(turns) {
        p.circle_filled(c + Vec2::new(disc.dx, disc.dy) * r, disc.r * r, color);
    }
    let core = mark::core();
    p.circle_filled(
        c,
        core.r * r,
        Color32::from_rgb(mark::CORE_RGB[0], mark::CORE_RGB[1], mark::CORE_RGB[2]),
    );
}

/// Region capture: a marquee with corner handles.
pub fn region(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    let inner = rect.shrink(rect.width() * 0.16);
    let dash = inner.width() / 7.0;

    // Dashed edges — drawn by hand because egui has no dash pattern.
    let mut x = inner.min.x;
    while x < inner.max.x {
        let x2 = (x + dash).min(inner.max.x);
        p.line_segment([Pos2::new(x, inner.min.y), Pos2::new(x2, inner.min.y)], s);
        p.line_segment([Pos2::new(x, inner.max.y), Pos2::new(x2, inner.max.y)], s);
        x += dash * 2.0;
    }
    let mut y = inner.min.y;
    while y < inner.max.y {
        let y2 = (y + dash).min(inner.max.y);
        p.line_segment([Pos2::new(inner.min.x, y), Pos2::new(inner.min.x, y2)], s);
        p.line_segment([Pos2::new(inner.max.x, y), Pos2::new(inner.max.x, y2)], s);
        y += dash * 2.0;
    }

    let h = rect.width() * 0.09;
    for corner in [
        inner.left_top(),
        inner.right_top(),
        inner.left_bottom(),
        inner.right_bottom(),
    ] {
        p.rect_filled(
            Rect::from_center_size(corner, Vec2::splat(h * 2.0)),
            0.0,
            color,
        );
    }
}

/// Window capture: a title bar over a body.
pub fn window(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    let body = rect.shrink(rect.width() * 0.14);
    p.rect_stroke(body, egui::Rounding::same(rect.width() * 0.1), s);
    p.line_segment(
        [
            Pos2::new(body.min.x, body.min.y + body.height() * 0.28),
            Pos2::new(body.max.x, body.min.y + body.height() * 0.28),
        ],
        s,
    );
}

/// Full screen: four corner brackets.
pub fn fullscreen(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    let r = rect.shrink(rect.width() * 0.16);
    let arm = 0.3;

    line(p, r, (0.0, arm), (0.0, 0.0), s);
    line(p, r, (0.0, 0.0), (arm, 0.0), s);
    line(p, r, (1.0 - arm, 0.0), (1.0, 0.0), s);
    line(p, r, (1.0, 0.0), (1.0, arm), s);
    line(p, r, (1.0, 1.0 - arm), (1.0, 1.0), s);
    line(p, r, (1.0, 1.0), (1.0 - arm, 1.0), s);
    line(p, r, (arm, 1.0), (0.0, 1.0), s);
    line(p, r, (0.0, 1.0), (0.0, 1.0 - arm), s);
}

/// Pin, for "keep this result on top".
pub fn pin(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    let head = Rect::from_center_size(
        lerp_rect(rect, 0.5, 0.36),
        Vec2::new(rect.width() * 0.44, rect.height() * 0.30),
    );
    p.rect_stroke(head, egui::Rounding::same(rect.width() * 0.06), s);
    line(p, rect, (0.5, 0.51), (0.5, 0.84), s);
    line(p, rect, (0.28, 0.51), (0.72, 0.51), s);
}

pub fn close(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    line(p, rect, (0.28, 0.28), (0.72, 0.72), s);
    line(p, rect, (0.72, 0.28), (0.28, 0.72), s);
}

pub fn check(p: &Painter, rect: Rect, color: Color32) {
    let s = Stroke::new((rect.width() * 0.12).clamp(1.2, 2.6), color);
    line(p, rect, (0.22, 0.52), (0.42, 0.72), s);
    line(p, rect, (0.42, 0.72), (0.78, 0.30), s);
}

/// Two arrows swapping direction — language swap and engine sections.
pub fn swap(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    line(p, rect, (0.18, 0.36), (0.82, 0.36), s);
    line(p, rect, (0.66, 0.20), (0.82, 0.36), s);
    line(p, rect, (0.82, 0.64), (0.18, 0.64), s);
    line(p, rect, (0.34, 0.80), (0.18, 0.64), s);
}

pub fn gear(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    let c = rect.center();
    let r = rect.width() * 0.26;
    p.circle_stroke(c, r, s);
    for i in 0..8 {
        let a = TAU * (i as f32) / 8.0;
        let dir = Vec2::new(a.cos(), a.sin());
        p.line_segment([c + dir * (r * 1.15), c + dir * (r * 1.62)], s);
    }
}

/// Keyboard, standing in for the shortcuts section.
pub fn keyboard(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    let body = Rect::from_min_max(lerp_rect(rect, 0.12, 0.28), lerp_rect(rect, 0.88, 0.74));
    p.rect_stroke(body, egui::Rounding::same(rect.width() * 0.08), s);
    for (i, y) in [0.42_f32, 0.58].iter().enumerate() {
        let n = if i == 0 { 4 } else { 3 };
        for k in 0..n {
            let x0 = 0.22 + (k as f32) * (0.56 / n as f32) + if i == 1 { 0.06 } else { 0.0 };
            line(p, rect, (x0, *y), (x0 + 0.08, *y), s);
        }
    }
}

/// The Binance mark: four diamonds around a fifth.
pub fn binance(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let unit = rect.width().min(rect.height()) * 0.5;
    let diamond = |offset: Vec2, half: f32| {
        let centre = c + offset;
        Shape::convex_polygon(
            vec![
                centre + Vec2::new(0.0, -half),
                centre + Vec2::new(half, 0.0),
                centre + Vec2::new(0.0, half),
                centre + Vec2::new(-half, 0.0),
            ],
            color,
            Stroke::NONE,
        )
    };
    let step = unit * 0.56;
    let small = unit * 0.26;
    p.add(diamond(Vec2::new(0.0, -step), small));
    p.add(diamond(Vec2::new(0.0, step), small));
    p.add(diamond(Vec2::new(-step, 0.0), small));
    p.add(diamond(Vec2::new(step, 0.0), small));
    p.add(diamond(Vec2::ZERO, unit * 0.34));
}

/// The Tether mark: a ₮ on a disc.
pub fn tether(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.42;
    p.circle_stroke(c, r, s);
    // The bar and stem of ₮, plus the ellipse the logo puts across the stem.
    line(p, rect, (0.30, 0.34), (0.70, 0.34), s);
    line(p, rect, (0.50, 0.34), (0.50, 0.74), s);
    let mut pts = Vec::with_capacity(25);
    for i in 0..=24 {
        let a = TAU * (i as f32) / 24.0;
        pts.push(c + Vec2::new(a.cos() * r * 0.52, a.sin() * r * 0.20));
    }
    p.add(Shape::line(pts, s));
}

/// Rocket — "launch at startup".
pub fn startup(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    // Body: a teardrop pointing up-right.
    line(p, rect, (0.30, 0.70), (0.62, 0.24), s);
    line(p, rect, (0.62, 0.24), (0.78, 0.22), s);
    line(p, rect, (0.78, 0.22), (0.76, 0.38), s);
    line(p, rect, (0.76, 0.38), (0.30, 0.70), s);
    line(p, rect, (0.30, 0.70), (0.22, 0.62), s);
    // Exhaust.
    line(p, rect, (0.26, 0.74), (0.18, 0.82), s);
    p.circle_filled(lerp_rect(rect, 0.63, 0.35), rect.width() * 0.06, color);
}

/// Bell — notification area.
pub fn bell(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    line(p, rect, (0.26, 0.66), (0.30, 0.44), s);
    line(p, rect, (0.30, 0.44), (0.50, 0.24), s);
    line(p, rect, (0.50, 0.24), (0.70, 0.44), s);
    line(p, rect, (0.70, 0.44), (0.74, 0.66), s);
    line(p, rect, (0.24, 0.66), (0.76, 0.66), s);
    line(p, rect, (0.43, 0.74), (0.57, 0.74), s);
}

/// Clipboard — copy to clipboard.
pub fn clipboard(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    let body = Rect::from_min_max(lerp_rect(rect, 0.24, 0.22), lerp_rect(rect, 0.76, 0.80));
    p.rect_stroke(body, egui::Rounding::same(rect.width() * 0.08), s);
    let clip = Rect::from_min_max(lerp_rect(rect, 0.38, 0.14), lerp_rect(rect, 0.62, 0.30));
    p.rect_stroke(clip, egui::Rounding::same(rect.width() * 0.05), s);
}

/// Magnifier — the settings search field.
pub fn search(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    p.circle_stroke(lerp_rect(rect, 0.44, 0.44), rect.width() * 0.22, s);
    line(p, rect, (0.62, 0.62), (0.80, 0.80), s);
}

/// A page with ruled lines — the log section.
pub fn journal(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    let page = Rect::from_min_max(lerp_rect(rect, 0.22, 0.14), lerp_rect(rect, 0.78, 0.86));
    p.rect_stroke(page, egui::Rounding::same(rect.width() * 0.07), s);
    for y in [0.34_f32, 0.50, 0.66] {
        line(p, rect, (0.32, y), (0.68, y), s);
    }
}

pub fn globe(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    let c = rect.center();
    let r = rect.width() * 0.36;
    p.circle_stroke(c, r, s);
    p.line_segment([c - Vec2::X * r, c + Vec2::X * r], s);
    // Two meridians, drawn as flattened ellipses.
    for k in [0.42_f32, 0.78] {
        let mut pts = Vec::with_capacity(33);
        for i in 0..=32 {
            let a = TAU * (i as f32) / 32.0;
            pts.push(c + Vec2::new(a.cos() * r * k, a.sin() * r));
        }
        p.add(Shape::line(pts, s));
    }
}

/// Half-filled disc — the appearance section, exactly the ◑ from the design.
pub fn appearance(p: &Painter, rect: Rect, color: Color32) {
    let s = stroke_for(rect, color);
    let c = rect.center();
    let r = rect.width() * 0.34;
    p.circle_stroke(c, r, s);
    let mut pts = vec![c + Vec2::new(0.0, -r)];
    for i in 0..=24 {
        let a = -TAU / 4.0 + TAU / 2.0 * (i as f32) / 24.0;
        pts.push(c + Vec2::new(a.cos() * r, a.sin() * r));
    }
    p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// Indeterminate progress arc. `phase` is seconds; the caller keeps repainting.
pub fn spinner(p: &Painter, rect: Rect, color: Color32, phase: f32) {
    let c = rect.center();
    let r = rect.width() * 0.34;
    let s = Stroke::new((rect.width() * 0.1).clamp(1.2, 2.4), color);
    let start = phase * 2.6;
    let sweep = TAU * 0.72;
    let steps = 28;
    let pts: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let a = start + sweep * (i as f32) / steps as f32;
            c + Vec2::new(a.cos() * r, a.sin() * r)
        })
        .collect();
    p.add(Shape::line(pts, s));
}

/// Traffic lights for the macOS-styled result window.
pub fn traffic_lights(p: &Painter, origin: Pos2, radius: f32, gap: f32, active: bool) -> Rect {
    let colors = if active {
        [
            Color32::from_rgb(0xF2, 0x60, 0x57),
            Color32::from_rgb(0xF5, 0xB5, 0x2E),
            Color32::from_rgb(0x54, 0xC2, 0x2C),
        ]
    } else {
        [Color32::from_gray(0xC9); 3]
    };
    for (i, color) in colors.iter().enumerate() {
        p.circle_filled(
            origin + Vec2::new((radius * 2.0 + gap) * i as f32 + radius, radius),
            radius,
            *color,
        );
    }
    Rect::from_min_size(origin, Vec2::new(radius * 6.0 + gap * 2.0, radius * 2.0))
}
