//! Pieces the result views share: placement, panels, the small controls.

use egui::{Color32, Pos2, Rect, Sense, Vec2};

use crate::shared::i18n::t;
use crate::ui::theme::text;
use crate::ui::{icons, widgets, Theme};

/// The copy shortcut, however the platform spells it — egui reports it as a
/// Copy event, which covers Ctrl+C and ⌘C without either being named here.
pub fn copy_requested(ctx: &egui::Context) -> bool {
    ctx.input(|i| {
        i.events.iter().any(|e| matches!(e, egui::Event::Copy))
            || i.modifiers.command && i.key_pressed(egui::Key::C)
    })
}

/// A panel that paints nothing of its own, so the window shows only what the
/// view draws into it.
pub fn transparent_panel(ctx: &egui::Context, add: impl FnOnce(&mut egui::Ui)) {
    let mut v = ctx.style().visuals.clone();
    v.window_fill = Color32::TRANSPARENT;
    v.panel_fill = Color32::TRANSPARENT;
    ctx.set_visuals(v);
    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show(ctx, add);
}

/// The same horizontal inset `padded` gives a row, but as a margin rather than
/// a layout.
///
/// Anything that asks the `Ui` how much room it has — a scroll area, most
/// obviously — has to go in here instead: inside a horizontal row the answer is
/// the height of the row, not the height of the window.
pub fn body_frame() -> egui::Frame {
    egui::Frame::none().inner_margin(egui::Margin::symmetric(14.0, 0.0))
}

pub fn padded<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let mut out = None;
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width() - 14.0);
            out = Some(add(ui));
        });
    });
    out.expect("body always runs")
}

/// What the strip at the top of a result says.
pub struct HeaderInfo {
    pub source: String,
    pub target: String,
    pub right: String,
}

pub fn header(ui: &mut egui::Ui, theme: &Theme, info: &HeaderInfo) {
    padded(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            widgets::chip(ui, theme, &info.source, false);
            ui.label(
                egui::RichText::new("→")
                    .font(text::caption())
                    .color(theme.text_dim),
            );
            widgets::chip(ui, theme, &info.target, false);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(&info.right)
                        .font(text::caption())
                        .color(theme.text_dim),
                );
            });
        });
    });
}

/// Spinner and a word for what is happening. `recognised` is the text once it
/// has been read — showing it while the translation is still on its way gives
/// the wait something to look at, and confirms the right thing was captured.
pub fn loading_body(ui: &mut egui::Ui, theme: &Theme, phase: f32, recognised: Option<&str>) {
    ui.ctx().request_repaint();
    padded(ui, |ui| {
        ui.add_space(10.0);
        if let Some(original) = recognised {
            ui.label(
                egui::RichText::new(crate::shared::logging::clip(original, 400))
                    .font(text::small())
                    .color(theme.text_dim),
            );
            ui.add_space(8.0);
        }
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            icons::spinner(ui.painter(), rect, theme.sakura, phase);
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(loading_label(recognised.is_some()))
                    .font(text::body())
                    .color(theme.text_dim),
            );
        });
        ui.add_space(6.0);
    });
}

/// The two halves of the wait have different names: reading the screen, then
/// translating what was read.
pub fn loading_label(recognised: bool) -> &'static str {
    if recognised {
        t("Translating…")
    } else {
        t("Reading the text…")
    }
}

pub fn message_body(ui: &mut egui::Ui, msg: &str, colour: Color32) {
    padded(ui, |ui| {
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.label(egui::RichText::new(msg).font(text::small()).color(colour));
            });
    });
}

#[allow(clippy::too_many_arguments)]
pub fn column(
    ui: &mut egui::Ui,
    theme: &Theme,
    width: f32,
    height: f32,
    caption: &str,
    caption_color: Color32,
    body: &str,
    accent: bool,
) {
    let rect = ui.allocate_space(Vec2::new(width, height)).1;
    if accent {
        ui.painter().rect_filled(rect, 0.0, theme.card_accent);
    }
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(14.0, 12.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    inner.label(
        egui::RichText::new(caption)
            .font(text::caption())
            .color(caption_color)
            .strong(),
    );
    inner.add_space(7.0);
    egui::ScrollArea::vertical()
        .id_salt(caption.to_owned())
        .auto_shrink([false, false])
        .show(&mut inner, |ui| {
            ui.label(
                egui::RichText::new(body)
                    .font(text::body())
                    .color(if accent { theme.text } else { theme.text_dim }),
            );
        });
}

/// Dark chrome for controls that float over an arbitrary screenshot: legible
/// on a white page and on a night scene alike.
pub const TOOLBAR_FILL: Color32 = Color32::from_rgba_premultiplied(22, 22, 26, 232);
const TOOLBAR_TEXT: Color32 = Color32::from_rgba_premultiplied(235, 235, 240, 240);

/// One control of a floating toolbar: an icon, a tooltip, a soft hover.
pub fn tool_button(
    ui: &mut egui::Ui,
    icon: widgets::IconFn,
    tip: &str,
    active: bool,
    accent: Color32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    let p = ui.painter();
    if resp.is_pointer_button_down_on() {
        p.rect_filled(rect, 5.0, Color32::from_white_alpha(52));
    } else if resp.hovered() {
        p.rect_filled(rect, 5.0, Color32::from_white_alpha(30));
    }
    icon(p, rect.shrink(5.5), if active { accent } else { TOOLBAR_TEXT });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tip)
}

/// The frame a floating toolbar sits in.
pub fn toolbar_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(TOOLBAR_FILL)
        .rounding(egui::Rounding::same(7.0))
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_white_alpha(28)))
        .inner_margin(egui::Margin::same(3.0))
}

/// A small pill that says the app is working, for views with no room for more.
pub fn working_pill(painter: &egui::Painter, centre: Pos2, label: &str, phase: f32, accent: Color32) {
    let galley = painter.layout_no_wrap(label.to_owned(), text::small(), TOOLBAR_TEXT);
    let size = Vec2::new(galley.size().x + 44.0, 30.0);
    let rect = Rect::from_center_size(centre, size);
    painter.rect_filled(rect, 15.0, TOOLBAR_FILL);
    painter.rect_stroke(
        rect,
        15.0,
        egui::Stroke::new(1.0_f32, Color32::from_white_alpha(28)),
    );
    let spinner = Rect::from_center_size(
        Pos2::new(rect.min.x + 18.0, rect.center().y),
        Vec2::splat(16.0),
    );
    icons::spinner(painter, spinner, accent, phase);
    painter.galley(
        Pos2::new(spinner.max.x + 6.0, rect.center().y - galley.size().y / 2.0),
        galley,
        TOOLBAR_TEXT,
    );
}

/// How wide [`working_pill`] comes out for this label, so a view can decide
/// whether it has the room.
pub fn working_pill_width(painter: &egui::Painter, label: &str) -> f32 {
    painter
        .layout_no_wrap(label.to_owned(), text::small(), TOOLBAR_TEXT)
        .size()
        .x
        + 44.0
}

/// Places a floating window against a rectangle on screen: below it when there
/// is room, above it when there is not, and inside `work` regardless.
///
/// `work` is the *work area*, not the desktop — the last line of a translation
/// under the Dock or behind the taskbar is unreadable, which is the same as not
/// being shown.
pub fn place_beside(anchor: Rect, size: Vec2, work: Rect) -> Pos2 {
    const GAP: f32 = 8.0;

    let below = anchor.max.y + GAP;
    let above = anchor.min.y - size.y - GAP;

    let y = if below + size.y <= work.max.y {
        below
    } else if above >= work.min.y {
        above
    } else {
        // Neither side fits — the selection is most of the screen. Sit against
        // the bottom edge and let the body scroll.
        work.max.y - size.y
    };

    // This clamp is what actually guarantees the window is on screen. The
    // branches above choose where it *should* sit; the clamp handles the cases
    // where that answer is still outside — an anchor on another monitor, or a
    // work area smaller than the window itself.
    Pos2::new(
        anchor
            .min
            .x
            .clamp(work.min.x, (work.max.x - size.x).max(work.min.x)),
        y.clamp(work.min.y, (work.max.y - size.y).max(work.min.y)),
    )
}

/// Places a subtitle: centred on the screen at its bottom (or top) edge, and
/// moved clear of the text it translates when the two would overlap.
///
/// A game's own subtitles sit exactly where ours would go. Covering the
/// original with its translation is the in-place view's job; this one is meant
/// to leave the screen readable, so it steps aside — above the captured text
/// when there is room, below it otherwise.
pub fn place_subtitle(size: Vec2, work: Rect, anchor: Rect, top: bool, margin: f32) -> Pos2 {
    const GAP: f32 = 10.0;

    let x = work.center().x - size.x / 2.0;
    let mut y = if top {
        work.min.y + margin
    } else {
        work.max.y - margin - size.y
    };

    let bar = Rect::from_min_size(Pos2::new(x, y), size);
    if bar.intersects(anchor) {
        let above = anchor.min.y - GAP - size.y;
        let below = anchor.max.y + GAP;
        if above >= work.min.y {
            y = above;
        } else if below + size.y <= work.max.y {
            y = below;
        }
        // Neither: the capture fills the screen, and the edge is as good a
        // place as any.
    }

    Pos2::new(
        x.clamp(work.min.x, (work.max.x - size.x).max(work.min.x)),
        y.clamp(work.min.y, (work.max.y - size.y).max(work.min.y)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1920×1200 with a 40-point taskbar or Dock along the bottom.
    const WORK: Rect = Rect {
        min: Pos2::new(0.0, 0.0),
        max: Pos2::new(1920.0, 1160.0),
    };
    const SIZE: Vec2 = Vec2::new(380.0, 400.0);

    fn anchor(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    #[test]
    fn it_sits_under_the_selection_when_there_is_room() {
        let p = place_beside(anchor(100.0, 100.0, 300.0, 200.0), SIZE, WORK);
        assert_eq!(p, Pos2::new(100.0, 308.0));
    }

    #[test]
    fn it_flips_above_when_the_bottom_would_not_fit() {
        // Selection ends at 900; 900 + 8 + 400 = 1308, past the work area.
        let p = place_beside(anchor(100.0, 500.0, 300.0, 400.0), SIZE, WORK);
        assert_eq!(p.y, 500.0 - 400.0 - 8.0);
    }

    #[test]
    fn a_full_screen_selection_still_lands_inside_the_work_area() {
        let p = place_beside(anchor(0.0, 0.0, 1920.0, 1160.0), SIZE, WORK);
        let rect = Rect::from_min_size(p, SIZE);
        assert!(WORK.contains_rect(rect), "{rect:?} escaped {WORK:?}");
    }

    #[test]
    fn it_never_leaves_the_work_area() {
        for a in [
            anchor(1900.0, 1140.0, 400.0, 400.0),
            anchor(-500.0, -500.0, 200.0, 200.0),
            anchor(1919.0, 0.0, 1.0, 1.0),
            anchor(0.0, 1159.0, 1.0, 1.0),
        ] {
            let rect = Rect::from_min_size(place_beside(a, SIZE, WORK), SIZE);
            assert!(WORK.contains_rect(rect), "anchor {a:?} put it at {rect:?}");
        }
    }

    #[test]
    fn a_work_area_smaller_than_the_window_pins_it_to_the_corner() {
        let tiny = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(200.0, 200.0));
        let p = place_beside(anchor(10.0, 10.0, 50.0, 50.0), SIZE, tiny);
        assert_eq!(p, tiny.min);
    }

    /// The work area does not have to start at the origin: a second monitor
    /// above or to the left of the primary one gives it negative coordinates.
    #[test]
    fn it_respects_a_work_area_that_starts_off_origin() {
        let left = Rect::from_min_max(Pos2::new(-1920.0, -100.0), Pos2::new(0.0, 980.0));
        let rect = Rect::from_min_size(
            place_beside(anchor(-1800.0, 800.0, 200.0, 200.0), SIZE, left),
            SIZE,
        );
        assert!(left.contains_rect(rect), "{rect:?} escaped {left:?}");
    }

    const BAR: Vec2 = Vec2::new(800.0, 60.0);

    #[test]
    fn a_subtitle_sits_centred_at_the_bottom() {
        let p = place_subtitle(BAR, WORK, anchor(100.0, 100.0, 300.0, 80.0), false, 70.0);
        assert_eq!(p.x, (1920.0 - 800.0) / 2.0);
        assert_eq!(p.y, 1160.0 - 70.0 - 60.0);
    }

    #[test]
    fn a_subtitle_can_be_asked_to_the_top_instead() {
        let p = place_subtitle(BAR, WORK, anchor(100.0, 600.0, 300.0, 80.0), true, 70.0);
        assert_eq!(p.y, 70.0);
    }

    #[test]
    fn a_subtitle_steps_above_the_text_it_would_have_covered() {
        // The game's own subtitle, right where ours would go.
        let captured = anchor(560.0, 1010.0, 800.0, 70.0);
        let p = place_subtitle(BAR, WORK, captured, false, 70.0);
        let bar = Rect::from_min_size(p, BAR);
        assert!(!bar.intersects(captured), "{bar:?} still covers {captured:?}");
        assert!(bar.max.y <= captured.min.y, "expected it above the capture");
    }

    #[test]
    fn a_subtitle_goes_below_when_there_is_no_room_above() {
        let captured = anchor(560.0, 20.0, 800.0, 70.0);
        let p = place_subtitle(BAR, WORK, captured, true, 30.0);
        let bar = Rect::from_min_size(p, BAR);
        assert!(!bar.intersects(captured));
        assert!(bar.min.y >= captured.max.y);
    }

    #[test]
    fn a_subtitle_for_a_full_screen_capture_stays_at_the_edge() {
        let p = place_subtitle(BAR, WORK, WORK, false, 70.0);
        assert_eq!(p.y, 1160.0 - 70.0 - 60.0);
    }

    #[test]
    fn a_subtitle_wider_than_the_screen_is_still_placed_on_it() {
        let wide = Vec2::new(4000.0, 60.0);
        let p = place_subtitle(wide, WORK, anchor(0.0, 0.0, 10.0, 10.0), false, 70.0);
        assert_eq!(p.x, WORK.min.x);
    }
}
