//! The capture overlay: frozen desktop, mode switcher, selection marquee.

use std::time::Instant;

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2};

use super::frame::Frame;
use super::screenshot::{cursor_position, monitor_for, Bounds};
use super::window_pick::{hit_test, WindowInfo};
use crate::entities::settings::CaptureMode;
use crate::shared::i18n::t;
use crate::ui::theme::{text, wash};
use crate::ui::{icons, widgets, Theme};

/// Title of the overlay's window. Never drawn — the window has no chrome — but
/// it is how the platform layer picks this window out to place it over the
/// whole desktop, the menu bar and the Dock included.
pub const TITLE: &str = "Sakura capture overlay";

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

/// What the selection is for. The overlay looks the same either way; what it
/// says, and what happens when the mouse is released, differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Translate what is selected, once.
    Translate,
    /// Keep translating the selected area as its text changes.
    Watch,
}

pub struct OverlayState {
    pub mode: CaptureMode,
    pub purpose: Purpose,
    pub show_mode_hud: bool,
    pub geometry: Geometry,
    /// The desktop as it was when the shortcut was pressed. Everything the
    /// user sees and selects here is this one image, and it is the image the
    /// text is later read from.
    pub frame: Option<Frame>,
    texture: Option<TextureHandle>,
    /// Frames drawn so far. The window is created hidden and shown on the
    /// second one, once it has been told not to animate in.
    pub frames: u32,
    opened: Instant,

    drag_start: Option<Pos2>,
    drag_current: Option<Pos2>,
    /// Selection being moved wholesale while Space is held.
    move_anchor: Option<(Pos2, Rect)>,

    pub windows: Vec<WindowInfo>,
    hovered_window: Option<Rect>,
    /// Full-screen mode: the display under the pointer.
    hovered_screen: Option<Bounds>,

    /// Result in desktop physical pixels.
    pub completed: Option<Bounds>,
    pub cancelled: bool,
}

impl OverlayState {
    pub fn new(
        geometry: Geometry,
        mode: CaptureMode,
        purpose: Purpose,
        show_mode_hud: bool,
    ) -> Self {
        Self {
            // Watching is about one area; a window or a screen is not
            // something whose "text changes" in a way worth following.
            mode: if purpose == Purpose::Watch {
                CaptureMode::Region
            } else {
                mode
            },
            purpose,
            show_mode_hud: show_mode_hud && purpose == Purpose::Translate,
            geometry,
            frame: None,
            texture: None,
            frames: 0,
            opened: Instant::now(),
            drag_start: None,
            drag_current: None,
            move_anchor: None,
            windows: Vec::new(),
            hovered_window: None,
            hovered_screen: None,
            completed: None,
            cancelled: false,
        }
    }

    pub fn with_frame(mut self, frame: Option<Frame>) -> Self {
        self.frame = frame;
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

    fn clear_selection(&mut self) {
        self.drag_start = None;
        self.drag_current = None;
        self.move_anchor = None;
    }

    /// How far the dimming has faded in, 0–1. The window itself appears at
    /// once and shows exactly what was on screen, so the only thing that
    /// visibly happens is the dimming easing in over the top.
    fn dim(&self) -> f32 {
        let t = (self.opened.elapsed().as_secs_f32() / 0.14).min(1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

pub fn render(ctx: &egui::Context, theme: &Theme, state: &mut OverlayState) {
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.cancelled = true;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Tab)) && state.show_mode_hud {
        state.mode = state.mode.next();
        state.clear_selection();
    }

    if state.texture.is_none() {
        if let Some(frame) = &state.frame {
            state.texture = Some(ctx.load_texture(
                "overlay_bg",
                frame.to_color_image(),
                egui::TextureOptions::NEAREST,
            ));
        }
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(Color32::BLACK))
        .show(ctx, |ui| {
            let screen = ui.max_rect();
            let origin = screen.min;

            // On Windows the window is put on exactly the desktop's pixels, so
            // the ratio of the two sizes is the true scale of *this* window —
            // which is not necessarily the scale of the host window the
            // geometry was first built from, on a desktop with mixed scaling.
            if cfg!(windows) && screen.width() > 1.0 {
                state.geometry.points_per_pixel =
                    state.geometry.desktop.w as f32 / screen.width();
            }

            let response = ui.allocate_rect(screen, Sense::click_and_drag());

            match state.mode {
                CaptureMode::Region => handle_region_drag(&response, state, ctx, origin),
                CaptureMode::Window => handle_window_pick(&response, state, origin),
                CaptureMode::FullScreen => handle_screen_pick(&response, state, origin),
            }

            // A pointing arrow over a full-screen scrim says nothing about what
            // is about to happen; the crosshair is what every capture tool
            // uses, and it aims to the pixel.
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
    if state.dim() < 1.0 {
        ctx.request_repaint();
    }
    state.frames = state.frames.saturating_add(1);
}

fn handle_region_drag(
    response: &egui::Response,
    state: &mut OverlayState,
    ctx: &egui::Context,
    origin: Pos2,
) {
    // Right click throws the marquee away and leaves the overlay up, so the
    // next drag starts from nothing. Esc is the other thing entirely — it
    // abandons the capture — and using it to redraw a rectangle means pressing
    // the hotkey again and waiting for another screen grab.
    if response.secondary_clicked() {
        state.clear_selection();
        return;
    }

    let space_held = ctx.input(|i| i.key_down(egui::Key::Space));

    // Space turns the drag into a move of the whole marquee.
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
            let bounds = state
                .geometry
                .rect_to_bounds(rect, origin)
                .clamp_to(state.geometry.desktop);
            match bounds {
                // Ignore a stray click that produced a few-pixel rectangle.
                Some(b) if b.w >= super::screenshot::MIN_SIDE && b.h >= super::screenshot::MIN_SIDE => {
                    state.completed = Some(b);
                }
                _ => state.clear_selection(),
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

/// "The whole screen" is the display the pointer is on — not every monitor at
/// once, which on a two-screen desk is twice the text anyone asked for.
fn handle_screen_pick(response: &egui::Response, state: &mut OverlayState, origin: Pos2) {
    let pos = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos());
    let screen = match pos {
        Some(pos) => {
            let (px, py) = state.geometry.to_physical(pos, origin);
            monitor_for(Bounds {
                x: px,
                y: py,
                w: 1,
                h: 1,
            })
            .bounds
        }
        None => state.geometry.desktop,
    };
    let screen = screen
        .clamp_to(state.geometry.desktop)
        .unwrap_or(state.geometry.desktop);
    state.hovered_screen = Some(screen);
    if response.clicked() {
        state.completed = Some(screen);
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
    painter.rect_filled(screen, 0.0, theme.scrim.gamma_multiply(state.dim()));

    let highlight = match state.mode {
        CaptureMode::Region => state.selection(),
        CaptureMode::Window => state.hovered_window,
        CaptureMode::FullScreen => state
            .hovered_screen
            .map(|b| state.geometry.bounds_to_rect(b, origin)),
    };

    let Some(sel) = highlight.map(|r| r.intersect(screen)) else {
        paint_crosshair(ui, state, screen);
        return;
    };
    if sel.width() < 1.0 || sel.height() < 1.0 {
        paint_crosshair(ui, state, screen);
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

    // A dark hairline outside the light one: the frame has to read on a white
    // page and on a night scene alike.
    painter.rect_stroke(
        sel.expand(1.0),
        0.0,
        Stroke::new(1.0_f32, Color32::from_black_alpha(110)),
    );
    painter.rect_stroke(sel, 0.0, Stroke::new(1.0_f32, Color32::from_white_alpha(235)));

    if state.mode == CaptureMode::Region {
        paint_corners(painter, sel, theme.sakura_soft);
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
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| format!("{} × {}", bounds.w, bounds.h)),
        _ => format!("{} × {}", bounds.w, bounds.h),
    };

    let font = text::caption();
    let galley = painter.layout_no_wrap(label, font, Color32::WHITE);
    let pad = Vec2::new(7.0, 3.0);
    let badge_size = galley.size() + pad * 2.0;
    // Above the selection when there is room; inside its top edge when the
    // selection starts at the top of the screen.
    let above = sel.min.y - badge_size.y - 6.0;
    let badge_pos = egui::pos2(
        (sel.max.x - badge_size.x).max(screen.min.x + 4.0),
        if above >= screen.min.y + 4.0 {
            above
        } else {
            sel.min.y + 6.0
        },
    );
    let badge = Rect::from_min_size(badge_pos, badge_size);
    painter.rect_filled(badge, 4.0, wash(0, 0, 0, 150));
    painter.galley(badge.min + pad, galley, Color32::WHITE);
}

/// Brackets at the four corners of the marquee.
///
/// The selection used to carry eight square handles — the vocabulary of a
/// rectangle you can resize by its edges, which this one is not. Brackets mark
/// the corners just as clearly and promise nothing.
fn paint_corners(painter: &egui::Painter, sel: Rect, colour: Color32) {
    let arm = (sel.width().min(sel.height()) * 0.25).clamp(4.0, 14.0);
    let stroke = Stroke::new(2.5_f32, colour);
    for (corner, dx, dy) in [
        (sel.left_top(), 1.0, 1.0),
        (sel.right_top(), -1.0, 1.0),
        (sel.left_bottom(), 1.0, -1.0),
        (sel.right_bottom(), -1.0, -1.0),
    ] {
        painter.line_segment([corner, corner + Vec2::new(arm * dx, 0.0)], stroke);
        painter.line_segment([corner, corner + Vec2::new(0.0, arm * dy)], stroke);
    }
}

/// Hairlines through the pointer, from edge to edge, while nothing is selected
/// yet: they show where a drag would start, and make it possible to line the
/// first corner up with the edge of a text box before committing to it.
fn paint_crosshair(ui: &egui::Ui, state: &OverlayState, screen: Rect) {
    if state.mode != CaptureMode::Region {
        return;
    }
    let Some(pointer) = ui.ctx().pointer_latest_pos() else {
        return;
    };
    if !screen.contains(pointer) {
        return;
    }
    let stroke = Stroke::new(1.0_f32, Color32::from_white_alpha(70));
    let painter = ui.painter();
    painter.hline(screen.x_range(), pointer.y.round() + 0.5, stroke);
    painter.vline(pointer.x.round() + 0.5, screen.y_range(), stroke);
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
    painter.rect_filled(box_rect, 4.0, wash(0, 0, 0, 170));
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

    // On the display the pointer is on: with two monitors the middle of the
    // desktop is the seam between them.
    let screen = pointer_screen(ctx, state);
    egui::Area::new(egui::Id::new("capture_top_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.center().x, screen.min.y + 34.0))
        .pivot(Align2::CENTER_TOP)
        .show(ctx, |ui| {
            ui.set_opacity(state.dim());
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.horizontal(|ui| {
                if state.show_mode_hud {
                    if let Some(i) = widgets::segmented(ui, theme, &items, selected) {
                        state.mode = CaptureMode::all()[i];
                        state.clear_selection();
                    }
                    ui.label(
                        egui::RichText::new("Tab")
                            .font(text::caption())
                            .color(wash(255, 255, 255, 130)),
                    );
                }
                if state.purpose == Purpose::Watch {
                    widgets::hint_pill(ui, theme, t("Select the area to keep translating"));
                }
                if widgets::overlay_button(ui, theme, icons::close, t("Esc — cancel")).clicked() {
                    state.cancelled = true;
                }
            });
        });
}

fn paint_hint(ctx: &egui::Context, theme: &Theme, state: &OverlayState) {
    let hint = match state.mode {
        CaptureMode::Region => t(
            "Drag around the text  ·  {space} — move the selection  ·  Right click — start over  ·  Esc — cancel",
        )
        .replace("{space}", theme.platform.space_key()),
        CaptureMode::Window => {
            t("Point at a window and click  ·  Tab — mode  ·  Esc — cancel").to_string()
        }
        CaptureMode::FullScreen => {
            t("Click to translate the whole screen  ·  Esc — cancel").to_string()
        }
    };
    let screen = pointer_screen(ctx, state);
    egui::Area::new(egui::Id::new("capture_hint"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.center().x, screen.max.y - 22.0))
        .pivot(Align2::CENTER_BOTTOM)
        .show(ctx, |ui| {
            ui.set_opacity(state.dim());
            widgets::hint_pill(ui, theme, &hint);
        });
}

/// The rectangle of the overlay that lies on the display the pointer is on —
/// where the controls are put, so they are in front of the user rather than
/// straddling two monitors.
fn pointer_screen(ctx: &egui::Context, state: &OverlayState) -> Rect {
    let whole = ctx.screen_rect();
    let origin = whole.min;
    let at = ctx
        .pointer_latest_pos()
        .map(|p| state.geometry.to_physical(p, origin))
        .or_else(cursor_position);
    let Some((x, y)) = at else {
        return whole;
    };
    let monitor = monitor_for(Bounds { x, y, w: 1, h: 1 }).bounds;
    let rect = state.geometry.bounds_to_rect(monitor, origin).intersect(whole);
    if rect.width() > 200.0 && rect.height() > 120.0 {
        rect
    } else {
        whole
    }
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
