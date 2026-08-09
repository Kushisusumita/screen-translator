//! Sakura widget kit — frameless.
//!
//! Nothing here draws an outline. A group is a fill a couple of tones off the
//! page; a row inside it is separated by a divider at the edge of visibility; a
//! control is a soft fill that reacts to the pointer. The one exception is a
//! surface that floats over the desktop, where a hairline is the only thing
//! keeping the panel distinct from whatever happens to be behind it.
//!
//! What stays platform-specific is identity, not chrome: the accent, the
//! navigation marker, the switch, the window controls, how a shortcut is spelled.

use egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Response, Rounding, Sense, Stroke, Ui, Vec2,
};

use super::platform::NavStyle;
use super::theme::{text, Theme};

pub type IconFn = fn(&Painter, Rect, Color32);

// ── Containers ───────────────────────────────────────────────────────────────

/// A grouped container: one rounded fill holding a run of rows, no outline.
pub fn card<R>(ui: &mut Ui, theme: &Theme, add: impl FnOnce(&mut Ui) -> R) -> R {
    // Pinned to the available width. Without this the frame sizes to its
    // content, so a row measuring `available_width()` from inside it sees the
    // parent's width instead of the group's.
    let width = ui.available_width();
    egui::Frame::none()
        .fill(theme.card)
        .rounding(theme.group_rounding())
        .inner_margin(egui::Margin::symmetric(0.0, 4.0))
        .show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
}

/// A settings list. Identical on both platforms: one container, dividers inside.
pub fn list<R>(ui: &mut Ui, theme: &Theme, add: impl FnOnce(&mut Ui) -> R) -> R {
    card(ui, theme, add)
}

/// Frosted panel used by everything that floats over the desktop. The one place
/// a hairline survives — over an arbitrary screenshot there is nothing else to
/// separate the panel from the page underneath.
pub fn glass_frame(theme: &Theme) -> egui::Frame {
    egui::Frame::none()
        .fill(theme.glass)
        .rounding(theme.surface_rounding())
        .stroke(Stroke::new(1.0, theme.border))
        .shadow(egui::epaint::Shadow {
            offset: egui::vec2(0.0, 16.0),
            blur: 44.0,
            spread: 0.0,
            color: Color32::from_black_alpha(140),
        })
        .inner_margin(egui::Margin::same(0.0))
}

/// `ЯЗЫКИ` — the small all-caps label above a group.
pub fn section_caption(ui: &mut Ui, theme: &Theme, label: &str) {
    ui.add_space(16.0);
    ui.label(
        egui::RichText::new(label.to_uppercase())
            .font(text::caption())
            .color(theme.text_dim)
            .strong(),
    );
    ui.add_space(7.0);
}

// ── Rows ─────────────────────────────────────────────────────────────────────

/// One entry in a settings list.
#[derive(Default)]
pub struct RowSpec<'a> {
    pub icon: Option<IconFn>,
    /// Colour for that icon. Defaults to the dimmed text colour, which is
    /// right for the interface's own glyphs and wrong for a brand mark: a
    /// grey Binance logo is not the Binance logo.
    pub icon_tint: Option<Color32>,
    pub title: &'a str,
    /// One line of explanation. Shown on Windows, where Settings always pairs a
    /// control with one; dropped on macOS, where it does not.
    pub subtitle: Option<&'a str>,
    /// Suppresses the divider under the final row of a group.
    pub last: bool,
    /// Tints the row — used while recording a shortcut.
    pub highlighted: bool,
}

impl<'a> RowSpec<'a> {
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            ..Default::default()
        }
    }

    pub fn icon(mut self, icon: IconFn) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn icon_tint(mut self, tint: Color32) -> Self {
        self.icon_tint = Some(tint);
        self
    }

    pub fn subtitle(mut self, subtitle: &'a str) -> Self {
        self.subtitle = Some(subtitle);
        self
    }

    pub fn last(mut self) -> Self {
        self.last = true;
        self
    }

    pub fn highlighted(mut self, on: bool) -> Self {
        self.highlighted = on;
        self
    }
}

/// Renders a list row: optional icon, title, optional subtitle, and whatever the
/// caller draws on the trailing edge.
pub fn row<R>(
    ui: &mut Ui,
    theme: &Theme,
    spec: RowSpec<'_>,
    right: impl FnOnce(&mut Ui) -> R,
) -> R {
    let m = theme.metrics;
    let show_subtitle = m.row_subtitles && spec.subtitle.is_some();
    let height = if show_subtitle {
        m.row_height + 6.0
    } else {
        m.row_height
    };
    let inset = m.row_inset;
    let (bg, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());

    if spec.highlighted {
        ui.painter()
            .rect_filled(bg, Rounding::ZERO, theme.tint(theme.accent, 20));
    }

    let mut text_left = bg.min.x + inset;
    if let Some(icon) = spec.icon {
        let size = 17.0;
        let icon_rect = Rect::from_min_size(
            egui::pos2(text_left, bg.center().y - size / 2.0),
            Vec2::splat(size),
        );
        icon(
            ui.painter(),
            icon_rect,
            spec.icon_tint.unwrap_or(theme.text_dim),
        );
        text_left = icon_rect.max.x + 12.0;
    }

    // The text is painted rather than laid out, so its width has to be measured
    // to know where the trailing controls may start. Without this a wide control
    // is free to sit on top of the subtitle.
    let painter = ui.painter();
    let mut text_right = text_left;
    if show_subtitle {
        let title = painter.layout_no_wrap(spec.title.to_owned(), text::body(), theme.text);
        let subtitle = painter.layout_no_wrap(
            spec.subtitle.unwrap_or_default().to_owned(),
            text::caption(),
            theme.text_dim,
        );
        text_right += title.size().x.max(subtitle.size().x);
        painter.galley(
            egui::pos2(text_left, bg.center().y - 9.0 - title.size().y / 2.0),
            title,
            theme.text,
        );
        painter.galley(
            egui::pos2(text_left, bg.center().y + 9.0 - subtitle.size().y / 2.0),
            subtitle,
            theme.text_dim,
        );
    } else {
        let title = painter.layout_no_wrap(spec.title.to_owned(), text::body(), theme.text);
        text_right += title.size().x;
        painter.galley(
            egui::pos2(text_left, bg.center().y - title.size().y / 2.0),
            title,
            theme.text,
        );
    }

    // Never let the label squeeze the controls out entirely, and never let the
    // controls start before the label ends.
    let trailing_left = (text_right + 12.0).min(bg.max.x - inset - 90.0);
    let mut content = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                egui::pos2(trailing_left.max(bg.min.x + inset), bg.min.y + 4.0),
                egui::pos2(bg.max.x - inset, bg.max.y - 4.0),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    content.spacing_mut().item_spacing.x = 8.0;
    let out = right(&mut content);

    if !spec.last {
        // Inset on both sides so the divider reads as internal structure rather
        // than as an edge of the container.
        ui.painter().hline(
            (bg.min.x + inset)..=(bg.max.x - inset),
            bg.max.y,
            Stroke::new(1.0, theme.separator),
        );
    }

    out
}

// ── Controls ─────────────────────────────────────────────────────────────────

/// The platform switch. Fluent greys the knob when off; Aqua keeps it white.
pub fn toggle(ui: &mut Ui, theme: &Theme, on: &mut bool) -> Response {
    let (w, h) = theme.metrics.toggle_size;
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());

    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }

    let how_on = ui.ctx().animate_bool_responsive(resp.id, *on);
    let p = ui.painter();
    let rounding = Rounding::same(h / 2.0);

    let track = if *on {
        if resp.hovered() {
            theme
                .accent
                .gamma_multiply(if theme.dark { 1.12 } else { 0.92 })
        } else {
            theme.accent
        }
    } else if resp.hovered() {
        theme.control_fill_hover()
    } else if theme.dark {
        Color32::from_rgba_unmultiplied(255, 255, 255, 40)
    } else {
        Color32::from_rgba_unmultiplied(0, 0, 0, 42)
    };
    p.rect_filled(rect, rounding, track);

    let pad = 2.6;
    let r = h / 2.0 - pad;
    let x = egui::lerp((rect.min.x + pad + r)..=(rect.max.x - pad - r), how_on);
    let knob = if *on {
        theme.on_accent
    } else if theme.metrics.toggle_dim_knob_when_off {
        theme.text_dim
    } else {
        Color32::WHITE
    };
    p.circle_filled(egui::pos2(x, rect.center().y), r, knob);

    resp
}

/// Filled action button — "Копировать".
pub fn primary_button(ui: &mut Ui, theme: &Theme, label: &str) -> Response {
    button_impl(ui, theme, label, ButtonKind::Primary)
}

/// Quiet button on a soft fill.
pub fn secondary_button(ui: &mut Ui, theme: &Theme, label: &str) -> Response {
    button_impl(ui, theme, label, ButtonKind::Secondary)
}

/// Text-only button for tertiary actions.
pub fn ghost_button(ui: &mut Ui, theme: &Theme, label: &str) -> Response {
    button_impl(ui, theme, label, ButtonKind::Ghost)
}

#[derive(Clone, Copy, PartialEq)]
enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
}

fn button_impl(ui: &mut Ui, theme: &Theme, label: &str, kind: ButtonKind) -> Response {
    let enabled = ui.is_enabled();

    // A disabled action drops the accent entirely. Keeping the fill and dimming
    // the label produced a solid blue block with nothing legible in it — egui
    // already fades everything drawn through a disabled `Ui`, so dimming again
    // here pushed the text under the threshold.
    let (fill, fg) = match kind {
        _ if !enabled => (theme.control_fill(), theme.text_dim),
        ButtonKind::Primary => (theme.accent, theme.on_accent),
        ButtonKind::Secondary => (theme.control_fill(), theme.text),
        ButtonKind::Ghost => (Color32::TRANSPARENT, theme.accent),
    };

    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), text::small(), fg);
    let pad = Vec2::new(if kind == ButtonKind::Ghost { 8.0 } else { 13.0 }, 6.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::click());

    let hot = resp.hovered() && enabled;
    let down = resp.is_pointer_button_down_on();
    let p = ui.painter();
    let bg = if !enabled {
        fill
    } else {
        match kind {
            ButtonKind::Primary => {
                if down {
                    fill.gamma_multiply(0.86)
                } else if hot {
                    fill.gamma_multiply(if theme.dark { 1.18 } else { 0.92 })
                } else {
                    fill
                }
            }
            ButtonKind::Secondary => {
                if down {
                    theme.control_fill_active()
                } else if hot {
                    theme.control_fill_hover()
                } else {
                    fill
                }
            }
            ButtonKind::Ghost => {
                if down {
                    theme.control_fill_active()
                } else if hot {
                    theme.control_fill()
                } else {
                    Color32::TRANSPARENT
                }
            }
        }
    };

    if bg.a() > 0 {
        p.rect_filled(rect, theme.control_rounding(), bg);
    }
    // `text` rather than `galley`: the colour is stated at the point of drawing,
    // so a disabled `Ui` cannot leave a filled box with an invisible label.
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        text::small(),
        fg,
    );

    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Square icon button: nothing at rest, a soft fill under the pointer.
pub fn icon_button(ui: &mut Ui, theme: &Theme, icon: IconFn, size: f32, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let hot = resp.hovered();

    let p = ui.painter();
    if resp.is_pointer_button_down_on() {
        p.rect_filled(rect, theme.control_rounding(), theme.control_fill_active());
    } else if hot {
        p.rect_filled(rect, theme.control_rounding(), theme.control_fill_hover());
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let inset = rect.height() * 0.28;
    icon(
        p,
        Rect::from_center_size(rect.center(), Vec2::splat(rect.height() - inset * 2.0)),
        if hot { theme.text } else { theme.text_dim },
    );
    resp.on_hover_text(tip)
}

/// Language pill: `EN`.
pub fn chip(ui: &mut Ui, theme: &Theme, label: &str, accent: bool) -> Response {
    let fg = if accent { theme.on_accent } else { theme.text };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), text::caption(), fg);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + Vec2::new(14.0, 8.0), Sense::hover());

    let bg = if accent {
        theme.accent
    } else {
        theme.control_fill()
    };
    let p = ui.painter();
    p.rect_filled(rect, theme.control_rounding(), bg);
    p.galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// Monospace key badge: `Alt + Shift + T`.
pub fn hotkey_badge(ui: &mut Ui, theme: &Theme, label: &str, recording: bool) -> Response {
    let fg = if recording { theme.accent } else { theme.text };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), text::mono(), fg);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + Vec2::new(20.0, 9.0), Sense::click());

    let p = ui.painter();
    let rounding = theme.control_rounding();
    if recording {
        // The one dashed outline left in the app: "waiting for a keypress" is a
        // state no fill communicates.
        p.rect_filled(rect, rounding, theme.tint(theme.accent, 22));
        dashed_rect(p, rect, rounding, Stroke::new(1.2, theme.accent));
    } else {
        let fill = if resp.hovered() {
            theme.control_fill_hover()
        } else {
            theme.control_fill()
        };
        p.rect_filled(rect, rounding, fill);
    }
    p.galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// Unbound shortcut: no box, just dimmed text.
pub fn unbound_badge(ui: &mut Ui, theme: &Theme, label: &str) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), text::mono(), theme.text_faint);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + Vec2::new(20.0, 9.0), Sense::click());
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme.text_faint,
    );
    resp
}

fn dashed_rect(p: &Painter, rect: Rect, rounding: Rounding, stroke: Stroke) {
    let dash = 4.0;
    let gap = 3.0;
    let inset = rounding.nw;
    let mut x = rect.min.x + inset;
    while x < rect.max.x - inset {
        let x2 = (x + dash).min(rect.max.x - inset);
        p.line_segment(
            [Pos2::new(x, rect.min.y), Pos2::new(x2, rect.min.y)],
            stroke,
        );
        p.line_segment(
            [Pos2::new(x, rect.max.y), Pos2::new(x2, rect.max.y)],
            stroke,
        );
        x += dash + gap;
    }
    let mut y = rect.min.y + inset;
    while y < rect.max.y - inset {
        let y2 = (y + dash).min(rect.max.y - inset);
        p.line_segment(
            [Pos2::new(rect.min.x, y), Pos2::new(rect.min.x, y2)],
            stroke,
        );
        p.line_segment(
            [Pos2::new(rect.max.x, y), Pos2::new(rect.max.x, y2)],
            stroke,
        );
        y += dash + gap;
    }
}

/// The capture-mode control at the top of the overlay. Returns the index that
/// was clicked.
///
/// The selected segment is filled with the platform accent on Windows and with
/// white on macOS, matching the two rounds of the design.
pub fn segmented(
    ui: &mut Ui,
    theme: &Theme,
    items: &[(IconFn, &str)],
    selected: usize,
) -> Option<usize> {
    let mut clicked = None;
    let use_accent = theme.metrics.nav_style == NavStyle::Indicator;
    let selected_fill = if use_accent {
        theme.accent
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 235)
    };
    let selected_fg = if use_accent {
        theme.on_accent
    } else {
        Color32::from_rgb(0x1A, 0x1A, 0x1A)
    };

    egui::Frame::none()
        .fill(OVERLAY_SURFACE)
        .rounding(theme.surface_rounding())
        .inner_margin(egui::Margin::same(4.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (i, (icon, label)) in items.iter().enumerate() {
                    let on = i == selected;
                    let fg = if on {
                        selected_fg
                    } else {
                        Color32::from_rgba_unmultiplied(255, 255, 255, 210)
                    };
                    let galley =
                        ui.painter()
                            .layout_no_wrap((*label).to_owned(), text::small(), fg);
                    let icon_w = 14.0;
                    let size = Vec2::new(galley.size().x + icon_w + 28.0, 26.0);
                    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());

                    if on {
                        ui.painter()
                            .rect_filled(rect, theme.control_rounding(), selected_fill);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(
                            rect,
                            theme.control_rounding(),
                            Color32::from_rgba_unmultiplied(255, 255, 255, 28),
                        );
                    }

                    let icon_rect = Rect::from_min_size(
                        egui::pos2(rect.min.x + 9.0, rect.center().y - icon_w / 2.0),
                        Vec2::splat(icon_w),
                    );
                    icon(ui.painter(), icon_rect, fg);
                    ui.painter().galley(
                        egui::pos2(
                            icon_rect.max.x + 5.0,
                            rect.center().y - galley.size().y / 2.0,
                        ),
                        galley,
                        fg,
                    );

                    if resp.clicked() {
                        clicked = Some(i);
                    }
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
            });
        });

    clicked
}

/// Dark chrome for controls that sit on the frozen desktop.
const OVERLAY_SURFACE: Color32 = Color32::from_rgba_premultiplied(38, 38, 42, 224);

/// A standalone action on the capture overlay, styled to match the mode strip.
pub fn overlay_button(ui: &mut Ui, theme: &Theme, icon: IconFn, label: &str) -> Response {
    let fg = Color32::from_rgba_unmultiplied(255, 255, 255, 228);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), text::small(), fg);
    let icon_w = 13.0;
    let size = Vec2::new(galley.size().x + icon_w + 32.0, 34.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());

    let p = ui.painter();
    let fill = if resp.is_pointer_button_down_on() {
        Color32::from_rgba_unmultiplied(74, 74, 80, 240)
    } else if resp.hovered() {
        Color32::from_rgba_unmultiplied(60, 60, 66, 236)
    } else {
        OVERLAY_SURFACE
    };
    p.rect_filled(rect, theme.surface_rounding(), fill);

    let icon_rect = Rect::from_min_size(
        egui::pos2(rect.min.x + 12.0, rect.center().y - icon_w / 2.0),
        Vec2::splat(icon_w),
    );
    icon(p, icon_rect, fg);
    p.galley(
        egui::pos2(
            icon_rect.max.x + 6.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        fg,
    );

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Navigation entry in the settings window.
///
/// macOS fills the whole row with the accent and puts a coloured tile behind the
/// icon. Windows 11 uses a faint neutral fill plus a short accent bar on the
/// leading edge and leaves the icon untinted.
pub fn sidebar_item(
    ui: &mut Ui,
    theme: &Theme,
    icon: IconFn,
    tile: Color32,
    label: &str,
    selected: bool,
) -> Response {
    let indicator = theme.metrics.nav_style == NavStyle::Indicator;
    let h = if indicator { 34.0 } else { 30.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());

    let p = ui.painter();
    let rounding = theme.control_rounding();

    if indicator {
        if selected {
            p.rect_filled(rect, rounding, theme.tint(theme.text, 16));
            let bar = Rect::from_min_size(
                egui::pos2(rect.min.x, rect.center().y - 8.0),
                Vec2::new(3.0, 16.0),
            );
            p.rect_filled(bar, Rounding::same(2.0), theme.accent);
        } else if resp.hovered() {
            p.rect_filled(rect, rounding, theme.hover_fill());
        }
    } else if selected {
        p.rect_filled(rect, rounding, theme.accent);
    } else if resp.hovered() {
        p.rect_filled(rect, rounding, theme.hover_fill());
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let icon_size = 18.0;
    let icon_rect = Rect::from_min_size(
        egui::pos2(rect.min.x + 10.0, rect.center().y - icon_size / 2.0),
        Vec2::splat(icon_size),
    );

    let fg = if selected && !indicator {
        theme.on_accent
    } else {
        theme.text
    };

    if indicator {
        icon(
            p,
            icon_rect,
            if selected {
                theme.accent
            } else {
                theme.text_dim
            },
        );
    } else {
        p.rect_filled(
            icon_rect,
            Rounding::same(5.0),
            if selected {
                Color32::from_rgba_unmultiplied(255, 255, 255, 64)
            } else {
                tile
            },
        );
        icon(p, icon_rect.shrink(3.5), Color32::WHITE);
    }

    p.text(
        egui::pos2(icon_rect.max.x + 9.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        text::body(),
        fg,
    );

    resp
}

/// Search field above the navigation. Returns true when the text changed.
pub fn search_field(ui: &mut Ui, theme: &Theme, query: &mut String) -> bool {
    let h = 30.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    let fill = if resp.hovered() {
        theme.control_fill_hover()
    } else {
        theme.control_fill()
    };
    ui.painter()
        .rect_filled(rect, theme.control_rounding(), fill);

    let icon_rect = Rect::from_min_size(
        egui::pos2(rect.min.x + 8.0, rect.center().y - 7.0),
        Vec2::splat(14.0),
    );
    super::icons::search(ui.painter(), icon_rect, theme.text_dim);

    let mut field = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                egui::pos2(icon_rect.max.x + 6.0, rect.min.y),
                egui::pos2(rect.max.x - 6.0, rect.max.y),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    field
        .add(
            egui::TextEdit::singleline(query)
                .frame(false)
                .desired_width(f32::INFINITY)
                .font(text::small())
                .hint_text(crate::shared::i18n::t("Search settings")),
        )
        .changed()
}

/// A selectable tile — translation engines, result presentations.
pub struct SelectCard<'a> {
    pub title: &'a str,
    pub subtitle: &'a str,
    pub selected: bool,
    /// Draws a tick after the subtitle. Only meaningful for entries that need a
    /// credential — a ticked "без ключа" reads as nonsense.
    pub ready: bool,
    /// Zero means "fill the available width".
    pub width: f32,
    pub height: f32,
}

impl<'a> SelectCard<'a> {
    pub fn new(title: &'a str, subtitle: &'a str) -> Self {
        Self {
            title,
            subtitle,
            selected: false,
            ready: false,
            width: 0.0,
            height: 52.0,
        }
    }

    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    pub fn ready(mut self, on: bool) -> Self {
        self.ready = on;
        self
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
}

/// Selection reads as a tinted fill plus an accent bar on the leading edge — the
/// same language as the navigation, and no outline anywhere.
pub fn select_card(ui: &mut Ui, theme: &Theme, card: SelectCard<'_>) -> Response {
    let width = if card.width > 0.0 {
        card.width
    } else {
        ui.available_width()
    };
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(width.max(120.0), card.height), Sense::click());
    let p = ui.painter();
    let rounding = theme.group_rounding();

    let fill = if card.selected {
        theme.tint(theme.accent, if theme.dark { 38 } else { 24 })
    } else if resp.is_pointer_button_down_on() {
        theme.control_fill_active()
    } else if resp.hovered() {
        theme.control_fill_hover()
    } else {
        theme.control_fill()
    };
    p.rect_filled(rect, rounding, fill);

    if card.selected {
        let bar = Rect::from_min_size(
            egui::pos2(rect.min.x, rect.center().y - 11.0),
            Vec2::new(3.0, 22.0),
        );
        p.rect_filled(bar, Rounding::same(2.0), theme.accent);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let left = rect.min.x + 15.0;
    p.text(
        egui::pos2(left, rect.center().y - 9.0),
        Align2::LEFT_CENTER,
        card.title,
        FontId::proportional(12.5),
        theme.text,
    );
    let subtitle = p.text(
        egui::pos2(left, rect.center().y + 9.0),
        Align2::LEFT_CENTER,
        card.subtitle,
        text::caption(),
        if card.ready {
            theme.success
        } else {
            theme.text_dim
        },
    );
    if card.ready {
        super::icons::check(
            p,
            Rect::from_min_size(
                egui::pos2(subtitle.max.x + 4.0, subtitle.center().y - 6.0),
                Vec2::splat(12.0),
            ),
            theme.success,
        );
    }

    resp
}

/// Hairline used between free-standing sections.
pub fn separator(ui: &mut Ui, theme: &Theme) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, theme.separator),
    );
}

/// Rounded status pill anchored to the bottom of the capture overlay.
pub fn hint_pill(ui: &mut Ui, theme: &Theme, label: &str) {
    let fg = Color32::from_rgba_unmultiplied(255, 255, 255, 200);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), text::caption(), fg);
    let size = galley.size() + Vec2::new(26.0, 10.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let _ = theme;
    ui.painter().rect_filled(
        rect,
        Rounding::same(14.0),
        Color32::from_rgba_unmultiplied(0, 0, 0, 115),
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
}
