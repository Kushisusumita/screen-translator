//! The capture overlay: frozen desktop, mode switcher, selection marquee.

use egui::{Align2, Color32, ColorImage, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2};

use super::screenshot::Bounds;
use super::window_pick::{hit_test, WindowInfo};
use crate::entities::settings::CaptureMode;
use crate::ui::theme::text;
use crate::ui::{icons, widgets, Theme};

/// Converts between egui's local points and desktop physical pixels.
///
/// This is the whole DPI story in one place. The overlay window is positioned at
/// the virtual desktop's top-left and sized to cover it, so a point at the
/// overlay's own origin is the desktop's origin — which is negative when a
/// monitor sits left of or above the primary one.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub desktop: Bounds,
    /// egui points → physical pixels.
    pub points_per_pixel: f32,
}

impl Geometry {
    pub fn new(desktop: Bounds, points_per_pixel: f32) -> Self {
        Self {
            desktop,
            points_per_pixel: if points_per_pixel > 0.0 {
                points_per_pixel
            } else {
                1.0
            },
        }
    }

    /// Overlay-local point → desktop physical pixel.
    pub fn to_physical(self, p: Pos2, origin: Pos2) -> (i32, i32) {
        let x = self.desktop.x as f32 + (p.x - origin.x) * self.points_per_pixel;
        let y = self.desktop.y as f32 + (p.y - origin.y) * self.points_per_pixel;
        (x.round() as i32, y.round() as i32)
    }

    /// Desktop physical pixel → overlay-local point.
    pub fn to_local(self, x: i32, y: i32, origin: Pos2) -> Pos2 {
        Pos2::new(
            origin.x + (x - self.desktop.x) as f32 / self.points_per_pixel,
            origin.y + (y - self.desktop.y) as f32 / self.points_per_pixel,
        )
    }

    pub fn rect_to_bounds(self, r: Rect, origin: Pos2) -> Bounds {
        let (x0, y0) = self.to_physical(r.min, origin);
        let (x1, y1) = self.to_physical(r.max, origin);
        Bounds {
            x: x0.min(x1),
            y: y0.min(y1),
            w: (x1 - x0).abs(),
            h: (y1 - y0).abs(),
        }
    }

    pub fn bounds_to_rect(self, b: Bounds, origin: Pos2) -> Rect {
        Rect::from_min_max(
            self.to_local(b.x, b.y, origin),
            self.to_local(b.right(), b.bottom(), origin),
        )
    }

    /// Size of the overlay window in points.
    pub fn window_size_points(self) -> Vec2 {
        Vec2::new(
            self.desktop.w as f32 / self.points_per_pixel,
            self.desktop.h as f32 / self.points_per_pixel,
        )
    }

    /// Position of the overlay window in points.
    pub fn window_pos_points(self) -> Pos2 {
        Pos2::new(
            self.desktop.x as f32 / self.points_per_pixel,
            self.desktop.y as f32 / self.points_per_pixel,
        )
    }
}

pub struct OverlayState {
    pub mode: CaptureMode,
    pub show_mode_hud: bool,
    pub geometry: Geometry,
    pub background: Option<ColorImage>,
    texture: Option<TextureHandle>,

    drag_start: Option<Pos2>,
    drag_current: Option<Pos2>,
    /// Selection being moved wholesale while Space is held.
    move_anchor: Option<(Pos2, Rect)>,

    pub windows: Vec<WindowInfo>,
    hovered_window: Option<Rect>,

    /// Result in desktop physical pixels.
    pub completed: Option<Bounds>,
    pub cancelled: bool,
}

impl OverlayState {
    pub fn new(geometry: Geometry, mode: CaptureMode, show_mode_hud: bool) -> Self {
        Self {
            mode,
            show_mode_hud,
            geometry,
            background: None,
            texture: None,
            drag_start: None,
            drag_current: None,
            move_anchor: None,
            windows: Vec::new(),
            hovered_window: None,
            completed: None,
            cancelled: false,
        }
    }

    pub fn with_background(mut self, img: ColorImage) -> Self {
        self.background = Some(img);
        self
    }

    pub fn with_windows(mut self, windows: Vec<WindowInfo>) -> Self {
        self.windows = windows;
        self
    }

    fn selection(&self) -> Option<Rect> {
        match (self.drag_start, self.drag_current) {
            (Some(a), Some(b)) => Some(Rect::from_two_pos(a, b)),
            _ => None,
        }
    }
}

pub fn render(ctx: &egui::Context, theme: &Theme, state: &mut OverlayState) {
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.cancelled = true;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Tab)) && state.show_mode_hud {
        state.mode = state.mode.next();
        state.drag_start = None;
        state.drag_current = None;
    }

    if state.texture.is_none() {
        if let Some(img) = state.background.take() {
            state.texture =
                Some(ctx.load_texture("overlay_bg", img, egui::TextureOptions::NEAREST));
        }
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(Color32::BLACK))
        .show(ctx, |ui| {
            let screen = ui.max_rect();
            let origin = screen.min;
            let response = ui.allocate_rect(screen, Sense::click_and_drag());

            match state.mode {
                CaptureMode::Region => handle_region_drag(&response, state, ctx),
                CaptureMode::Window => handle_window_pick(&response, state, origin),
                CaptureMode::FullScreen => {
                    if response.clicked() {
                        state.completed = Some(state.geometry.desktop);
                    }
                }
            }

            // A pointing arrow over a full-screen scrim says nothing about what
            // is about to happen; the crosshair is what every capture tool on
            // both platforms uses, and it aims to the pixel.
            ctx.set_cursor_icon(match state.mode {
                CaptureMode::Region => egui::CursorIcon::Crosshair,
                CaptureMode::Window | CaptureMode::FullScreen => egui::CursorIcon::PointingHand,
            });

            paint(ui, theme, state, screen, origin);
            paint_cursor_readout(ui, state, screen, origin);
        });

    if state.completed.is_none() {
        paint_top_bar(ctx, theme, state);
        paint_hint(ctx, theme, state);
    }
}

fn handle_region_drag(response: &egui::Response, state: &mut OverlayState, ctx: &egui::Context) {
    // Right click throws the marquee away and leaves the overlay up, so the
    // next drag starts from nothing. Esc is the other thing entirely — it
    // abandons the capture — and using it to redraw a rectangle means pressing
    // the hotkey again and waiting for another screen grab.
    if response.secondary_clicked() {
        state.drag_start = None;
        state.drag_current = None;
        state.move_anchor = None;
        return;
    }

    let space_held = ctx.input(|i| i.key_down(egui::Key::Space));

    // Space turns the drag into a move of the whole marquee — the design's
    // "Space — переместить выделение".
    if space_held {
        if let (Some(pos), Some(sel)) = (response.interact_pointer_pos(), state.selection()) {
            if state.move_anchor.is_none() && response.dragged() {
                state.move_anchor = Some((pos, sel));
            }
            if let Some((anchor, original)) = state.move_anchor {
                let delta = pos - anchor;
                let moved = original.translate(delta);
                state.drag_start = Some(moved.min);
                state.drag_current = Some(moved.max);
            }
            return;
        }
    } else {
        state.move_anchor = None;
    }

    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            state.drag_start = Some(pos);
            state.drag_current = Some(pos);
        }
    }
    if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            state.drag_current = Some(pos);
        }
    }
    if response.drag_stopped() {
        if let Some(rect) = state.selection() {
            let bounds = state.geometry.rect_to_bounds(rect, Pos2::ZERO);
            // Ignore a stray click that produced a few-pixel rectangle.
            if bounds.w >= super::screenshot::MIN_SIDE && bounds.h >= super::screenshot::MIN_SIDE {
                state.completed = Some(bounds);
            } else {
                state.drag_start = None;
                state.drag_current = None;
            }
        }
    }
}

fn handle_window_pick(response: &egui::Response, state: &mut OverlayState, origin: Pos2) {
    let Some(pos) = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos())
    else {
        state.hovered_window = None;
        return;
    };
    let (px, py) = state.geometry.to_physical(pos, origin);
    match hit_test(&state.windows, px, py) {
        Some(win) => {
            state.hovered_window = Some(state.geometry.bounds_to_rect(win.bounds, origin));
            if response.clicked() {
                state.completed = Some(win.bounds);
            }
        }
        None => {
            // Clicking bare desktop is a miss, not a cancel — cancelling is Esc
            // or the button, and silently aborting on a stray click is the kind
            // of thing that makes a capture tool feel unreliable.
            state.hovered_window = None;
        }
    }
}

fn paint(ui: &mut egui::Ui, theme: &Theme, state: &OverlayState, screen: Rect, origin: Pos2) {
    let painter = ui.painter();

    if let Some(tex) = &state.texture {
        painter.image(
            tex.id(),
            screen,
            Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    painter.rect_filled(screen, 0.0, theme.scrim);

    let highlight = match state.mode {
        CaptureMode::Region => state.selection(),
        CaptureMode::Window => state.hovered_window,
        CaptureMode::FullScreen => Some(screen),
    };

    let Some(sel) = highlight else { return };
    let sel = sel.intersect(screen);
    if sel.width() < 1.0 || sel.height() < 1.0 {
        return;
    }

    // Undim the chosen area by repainting the frozen pixels over the scrim.
    if let Some(tex) = &state.texture {
        let size = screen.size();
        let uv = Rect::from_min_max(
            egui::pos2(
                (sel.min.x - screen.min.x) / size.x,
                (sel.min.y - screen.min.y) / size.y,
            ),
            egui::pos2(
                (sel.max.x - screen.min.x) / size.x,
                (sel.max.y - screen.min.y) / size.y,
            ),
        );
        painter.image(tex.id(), sel, uv, Color32::WHITE);
    }

    painter.rect_stroke(sel, 0.0, Stroke::new(1.5, Color32::WHITE));

    // Eight handles: four corners plus the edge midpoints, exactly as drawn in
    // the design.
    if state.mode == CaptureMode::Region {
        let h = 7.0;
        for p in [
            sel.left_top(),
            sel.right_top(),
            sel.left_bottom(),
            sel.right_bottom(),
            egui::pos2(sel.center().x, sel.min.y),
            egui::pos2(sel.center().x, sel.max.y),
            egui::pos2(sel.min.x, sel.center().y),
            egui::pos2(sel.max.x, sel.center().y),
        ] {
            painter.rect_filled(
                Rect::from_center_size(p, Vec2::splat(h)),
                1.0,
                Color32::WHITE,
            );
        }
    }

    // Size badge, in real captured pixels rather than layout points — the two
    // differ on any scaled display, and the number the user reads should be the
    // number they get.
    let bounds = state.geometry.rect_to_bounds(sel, origin);
    let label = match state.mode {
        CaptureMode::Window => state
            .windows
            .iter()
            .find(|w| w.bounds == bounds)
            .map(|w| crate::shared::logging::clip(&w.title, 60).to_string())
            .unwrap_or_else(|| format!("{} × {}", bounds.w, bounds.h)),
        _ => format!("{} × {}", bounds.w, bounds.h),
    };

    let font = text::caption();
    let galley = painter.layout_no_wrap(label, font, Color32::WHITE);
    let pad = Vec2::new(7.0, 3.0);
    let badge_size = galley.size() + pad * 2.0;
    let badge_pos = egui::pos2(
        (sel.max.x - badge_size.x).max(screen.min.x + 4.0),
        (sel.min.y - badge_size.y - 6.0).max(screen.min.y + 4.0),
    );
    let badge = Rect::from_min_size(badge_pos, badge_size);
    painter.rect_filled(badge, 4.0, Color32::from_rgba_unmultiplied(0, 0, 0, 140));
    painter.galley(badge.min + pad, galley, Color32::WHITE);
}

/// The coordinate box that follows the crosshair, the way macOS's own capture
/// shows one.
///
/// Two lines, X over Y, in desktop physical pixels — the same unit as the size
/// badge and as the image that comes back, so the number the user reads is the
/// number they get. While a selection is being dragged the numbers track the
/// corner under the cursor, which is what makes it possible to line an edge up
/// with something exactly.
fn paint_cursor_readout(ui: &egui::Ui, state: &OverlayState, screen: Rect, origin: Pos2) {
    // Only the region mode aims at a point. Window mode aims at a window, and
    // full screen has nothing to aim at.
    if state.mode != CaptureMode::Region {
        return;
    }
    let Some(pointer) = ui.ctx().pointer_latest_pos() else {
        return;
    };
    if !screen.contains(pointer) {
        return;
    }

    let (x, y) = state.geometry.to_physical(pointer, origin);
    let painter = ui.painter();
    // Monospaced so the box does not twitch as the digits change.
    let galley = painter.layout_no_wrap(format!("X {x}\nY {y}"), text::mono(), Color32::WHITE);

    let pad = Vec2::new(7.0, 4.0);
    let box_rect = readout_rect(pointer, galley.size() + pad * 2.0, screen);
    painter.rect_filled(box_rect, 4.0, Color32::from_rgba_unmultiplied(0, 0, 0, 170));
    painter.galley(box_rect.min + pad, galley, Color32::WHITE);
}

/// Places the readout below-right of the crosshair, flipping to the other side
/// of it rather than sliding along the edge — a box that creeps over the cursor
/// near a corner would cover the very pixels being aimed at.
fn readout_rect(pointer: Pos2, size: Vec2, screen: Rect) -> Rect {
    /// Clear of the crosshair's own arms.
    const GAP: f32 = 16.0;
    const MARGIN: f32 = 4.0;

    let x = if pointer.x + GAP + size.x + MARGIN <= screen.max.x {
        pointer.x + GAP
    } else {
        pointer.x - GAP - size.x
    };
    let y = if pointer.y + GAP + size.y + MARGIN <= screen.max.y {
        pointer.y + GAP
    } else {
        pointer.y - GAP - size.y
    };

    // The lower bound wins if the box is wider than the space it has to live
    // in. `clamp` panics when handed min > max, and a desktop narrower than the
    // readout is unlikely but not impossible — a tiny secondary display, or a
    // large interface scale.
    let left = screen.min.x + MARGIN;
    let top = screen.min.y + MARGIN;
    Rect::from_min_size(
        Pos2::new(
            x.clamp(left, (screen.max.x - size.x - MARGIN).max(left)),
            y.clamp(top, (screen.max.y - size.y - MARGIN).max(top)),
        ),
        size,
    )
}

/// The strip at the top of the overlay: capture modes, plus a Cancel button that
/// does what Esc does.
///
/// Esc alone is not enough of an affordance. The hint pill mentions it, but a
/// user mid-drag wants something to click, and a key that silently does nothing
/// when the overlay has lost focus is worse than no key at all.
fn paint_top_bar(ctx: &egui::Context, theme: &Theme, state: &mut OverlayState) {
    let items: Vec<(widgets::IconFn, &str)> = vec![
        (
            icons::region as widgets::IconFn,
            CaptureMode::Region.label(),
        ),
        (
            icons::window as widgets::IconFn,
            CaptureMode::Window.label(),
        ),
        (
            icons::fullscreen as widgets::IconFn,
            CaptureMode::FullScreen.label(),
        ),
    ];
    let selected = CaptureMode::all()
        .iter()
        .position(|m| *m == state.mode)
        .unwrap_or(0);

    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("capture_top_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.center().x, screen.min.y + 34.0))
        .pivot(Align2::CENTER_TOP)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.horizontal(|ui| {
                if state.show_mode_hud {
                    if let Some(i) = widgets::segmented(ui, theme, &items, selected) {
                        state.mode = CaptureMode::all()[i];
                        state.drag_start = None;
                        state.drag_current = None;
                    }
                    ui.label(
                        egui::RichText::new("Tab")
                            .font(text::caption())
                            .color(Color32::from_rgba_unmultiplied(255, 255, 255, 130)),
                    );
                }
                if widgets::overlay_button(ui, theme, icons::close, "Esc — отмена").clicked()
                {
                    state.cancelled = true;
                }
            });
        });
}

fn paint_hint(ctx: &egui::Context, theme: &Theme, state: &OverlayState) {
    // Each platform names the space bar and the copy shortcut its own way, and
    // the design spells both out differently in the two rounds.
    let hint = match state.mode {
        CaptureMode::Region => format!(
            "Esc — отмена  ·  ПКМ — сбросить выделение  ·  {space} — переместить выделение  ·  {copy} — скопировать перевод",
            space = theme.platform.space_key(),
            copy = theme.platform.copy_shortcut(),
        ),
        CaptureMode::Window => {
            "Наведите на окно и кликните  ·  Tab — режим  ·  Esc — отмена".to_string()
        }
        CaptureMode::FullScreen => {
            "Кликните, чтобы перевести весь экран  ·  Esc — отмена".to_string()
        }
    };
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("capture_hint"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.center().x, screen.max.y - 22.0))
        .pivot(Align2::CENTER_BOTTOM)
        .show(ctx, |ui| {
            widgets::hint_pill(ui, theme, &hint);
        });
}

#[cfg(test)]
mod readout_tests {
    use super::*;

    const SCREEN: Rect = Rect {
        min: Pos2::new(0.0, 0.0),
        max: Pos2::new(1920.0, 1080.0),
    };
    const SIZE: Vec2 = Vec2::new(60.0, 34.0);

    #[test]
    fn it_sits_below_and_right_of_the_crosshair() {
        let r = readout_rect(Pos2::new(500.0, 400.0), SIZE, SCREEN);
        assert!(r.min.x > 500.0 && r.min.y > 400.0);
    }

    #[test]
    fn near_the_right_edge_it_flips_to_the_other_side() {
        let r = readout_rect(Pos2::new(1910.0, 400.0), SIZE, SCREEN);
        assert!(r.max.x < 1910.0, "should not cover the pointer: {r:?}");
        assert!(r.min.x >= SCREEN.min.x);
    }

    #[test]
    fn near_the_bottom_edge_it_flips_upwards() {
        let r = readout_rect(Pos2::new(500.0, 1075.0), SIZE, SCREEN);
        assert!(r.max.y < 1075.0, "should not cover the pointer: {r:?}");
    }

    #[test]
    fn it_never_leaves_the_overlay() {
        for p in [
            Pos2::new(0.0, 0.0),
            Pos2::new(1920.0, 1080.0),
            Pos2::new(1920.0, 0.0),
            Pos2::new(0.0, 1080.0),
        ] {
            let r = readout_rect(p, SIZE, SCREEN);
            assert!(SCREEN.contains_rect(r), "escaped the screen at {p:?}: {r:?}");
        }
    }

    #[test]
    fn a_screen_smaller_than_the_box_still_produces_a_sane_rect() {
        let tiny = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(40.0, 20.0));
        let r = readout_rect(Pos2::new(20.0, 10.0), SIZE, tiny);
        assert!(r.min.x.is_finite() && r.min.y.is_finite());
        assert!(r.min.x >= tiny.min.x && r.min.y >= tiny.min.y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 150 % display: one egui point is 1.5 physical pixels.
    fn scaled() -> Geometry {
        Geometry::new(
            Bounds {
                x: 0,
                y: 0,
                w: 2880,
                h: 1620,
            },
            1.5,
        )
    }

    /// A second monitor to the left, so desktop x goes negative.
    fn dual() -> Geometry {
        Geometry::new(
            Bounds {
                x: -1920,
                y: -200,
                w: 3840,
                h: 1280,
            },
            1.0,
        )
    }

    #[test]
    fn scaling_is_applied_when_converting_a_selection() {
        // The exact bug: a 400×100 point marquee on a 150 % display is a
        // 600×150 pixel region, not 400×100.
        let g = scaled();
        let rect = Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(400.0, 100.0));
        let b = g.rect_to_bounds(rect, Pos2::ZERO);
        assert_eq!((b.x, b.y, b.w, b.h), (150, 75, 600, 150));
    }

    #[test]
    fn the_desktop_origin_offsets_the_result() {
        let g = dual();
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(100.0, 40.0));
        let b = g.rect_to_bounds(rect, Pos2::ZERO);
        assert_eq!((b.x, b.y), (-1910, -180));
    }

    #[test]
    fn point_and_pixel_conversion_round_trips() {
        for g in [scaled(), dual()] {
            for (x, y) in [(0, 0), (123, 456), (-500, -100)] {
                let local = g.to_local(x, y, Pos2::ZERO);
                assert_eq!(g.to_physical(local, Pos2::ZERO), (x, y));
            }
        }
    }

    #[test]
    fn the_overlay_window_covers_the_whole_virtual_desktop_in_points() {
        let g = scaled();
        assert_eq!(g.window_size_points(), Vec2::new(1920.0, 1080.0));

        let g = dual();
        assert_eq!(g.window_pos_points(), Pos2::new(-1920.0, -200.0));
        assert_eq!(g.window_size_points(), Vec2::new(3840.0, 1280.0));
    }

    #[test]
    fn a_zero_scale_factor_does_not_produce_infinities() {
        let g = Geometry::new(
            Bounds {
                x: 0,
                y: 0,
                w: 100,
                h: 100,
            },
            0.0,
        );
        assert_eq!(g.points_per_pixel, 1.0);
        assert!(g.window_size_points().x.is_finite());
    }

    #[test]
    fn a_rectangle_dragged_up_and_left_still_has_positive_size() {
        let g = Geometry::new(
            Bounds {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            },
            1.0,
        );
        // from_two_pos normalises, but rect_to_bounds must not assume that.
        let rect = Rect::from_two_pos(Pos2::new(300.0, 300.0), Pos2::new(100.0, 100.0));
        let b = g.rect_to_bounds(rect, Pos2::ZERO);
        assert_eq!((b.x, b.y, b.w, b.h), (100, 100, 200, 200));
    }
}
