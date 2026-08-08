//! The three ways a finished translation is presented.
//!
//! All three come straight from the design: a glass popup anchored to the
//! captured region, the translation painted in place over the original, and a
//! free-floating two-column window. They share one state object so switching
//! between them mid-result — which the popup's own buttons do — keeps the text,
//! the timing and the engine label.

use std::time::Instant;

use egui::{
    Align2, Color32, ColorImage, Pos2, Rect, Sense, TextureHandle, Vec2, ViewportBuilder,
    ViewportCommand, ViewportId,
};

use crate::entities::settings::ResultView;
use crate::features::capture::{Bounds, Geometry};
use crate::features::translation::PipelineResult;
use crate::ui::platform::CaptionStyle;
use crate::ui::theme::text;
use crate::ui::{icons, widgets, Theme};

#[derive(Debug)]
pub enum Stage {
    Loading { since: Instant },
    Done(Box<PipelineResult>),
    Error(String),
}

/// Something the user asked for that the app, not the view, has to carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultAction {
    Copy,
    Speak,
    SwitchTo(ResultView),
    Close,
}

pub struct ResultUi {
    pub stage: Stage,
    pub view: ResultView,
    pub anchor: Bounds,
    pub geometry: Geometry,
    pub pinned: bool,
    /// Inline view: show the untranslated text instead, for a quick compare.
    pub show_original: bool,
    pub actions: Vec<ResultAction>,
    background: Option<ColorImage>,
    texture: Option<TextureHandle>,
    /// Remembered so the floating window reopens where the user left it.
    window_pos: Option<Pos2>,
    /// Dismiss as soon as the user clicks away. Mirrors the setting; kept here
    /// so the view does not need the whole `Settings`.
    pub close_on_focus_loss: bool,
    /// A window is not focused on the frame it is created, and closing on that
    /// would mean the result never appears at all. Only a *loss* counts.
    had_focus: bool,
    /// What the popup actually measured last frame: everything that is not the
    /// scrolling body, and how tall the body wanted to be. Guessing these with
    /// a constant left a gap under short results and cut the buttons off long
    /// ones, because the guess had to be wrong in one direction or the other.
    popup_chrome: Option<f32>,
    /// The height the window should be: what the frame actually drew when the
    /// text fits, the ceiling when it does not and the body has to scroll.
    popup_wanted: Option<f32>,
    /// Whether the body is currently in its scrolling mode.
    popup_scrolls: Option<bool>,
    /// Identifies the content the measurements above belong to, so a result
    /// arriving does not inherit the size of the spinner that preceded it.
    popup_measured_for: u64,
}

const POPUP_WIDTH: f32 = 380.0;
/// Only ever used to find these windows again through the platform's own API,
/// so their corners can be rounded; nothing draws them.
#[cfg(any(target_os = "macos", windows))]
const POPUP_TITLE: &str = "Sakura result popup";
#[cfg(any(target_os = "macos", windows))]
const WINDOW_TITLE: &str = "Sakura result window";
/// Everything in the popup that is not the text: header, separator, button row
/// and the spacing around them. The body gets whatever is left.
const POPUP_CHROME: f32 = 150.0;
const WINDOW_SIZE: Vec2 = Vec2::new(560.0, 300.0);

impl ResultUi {
    pub fn loading(view: ResultView, anchor: Bounds, geometry: Geometry) -> Self {
        Self {
            stage: Stage::Loading {
                since: Instant::now(),
            },
            // "Не показывать" still needs somewhere to report a failure, so the
            // popup stands in for it.
            view: if view == ResultView::None {
                ResultView::Popup
            } else {
                view
            },
            anchor,
            geometry,
            pinned: false,
            show_original: false,
            actions: Vec::new(),
            background: None,
            texture: None,
            window_pos: None,
            close_on_focus_loss: true,
            had_focus: false,
            popup_chrome: None,
            popup_wanted: None,
            popup_scrolls: None,
            popup_measured_for: 0,
        }
    }

    pub fn with_background(mut self, img: Option<ColorImage>) -> Self {
        self.background = img;
        self
    }

    pub fn result(&self) -> Option<&PipelineResult> {
        match &self.stage {
            Stage::Done(r) => Some(r.as_ref()),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.stage, Stage::Loading { .. })
    }

    /// The inline view needs the frozen desktop; without it, fall back to the
    /// popup rather than drawing a translation onto a black rectangle.
    fn effective_view(&self) -> ResultView {
        match self.view {
            ResultView::Inline if self.background.is_none() && self.texture.is_none() => {
                ResultView::Popup
            }
            ResultView::None => ResultView::Popup,
            other => other,
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, theme: &Theme, history_len: usize) {
        match self.effective_view() {
            ResultView::Popup | ResultView::None => self.render_popup(ctx, theme),
            ResultView::Inline => self.render_inline(ctx, theme),
            ResultView::Window => self.render_window(ctx, theme, history_len),
        }
    }

    /// Whether clicking away from this window should dismiss it.
    ///
    /// Called from inside the viewport, where `viewport().focused` is that
    /// window's own focus rather than the host's. Pinning the floating window
    /// is a request for it to stay, so it overrides the setting.
    fn focus_lost(&mut self, ctx: &egui::Context, pinned: bool) -> bool {
        if !self.close_on_focus_loss || pinned {
            return false;
        }
        // A translation still arriving is not something to throw away because
        // the user carried on working while waiting for it.
        if self.is_loading() {
            return false;
        }

        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        if focused {
            self.had_focus = true;
            false
        } else {
            self.had_focus
        }
    }

    // ── 1a: glass popup at the selection ─────────────────────────────────────

    fn render_popup(&mut self, ctx: &egui::Context, theme: &Theme) {
        let key = self.stage_key();
        if self.popup_measured_for != key {
            self.popup_measured_for = key;
            self.popup_chrome = None;
            self.popup_wanted = None;
            self.popup_scrolls = None;
        }

        let height = self.popup_height(ctx);
        let pos = self.popup_position(Vec2::new(POPUP_WIDTH, height));

        let mut close = false;
        let mut actions = Vec::new();
        let stage_snapshot = self.describe();

        // The popup is anchored to what the user just selected, so it is always
        // on top — there is nothing useful it could sit behind.
        let builder = ViewportBuilder::default()
            .with_position(pos)
            .with_inner_size([POPUP_WIDTH, height])
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_transparent(true)
            .with_always_on_top();

        // The title is never drawn — there is no chrome — but it is how the
        // window is found again so its corners can be rounded.
        #[cfg(any(target_os = "macos", windows))]
        let builder = builder.with_title(POPUP_TITLE);

        // How much room the body is allowed this frame, from what the chrome
        // actually measured last frame.
        let chrome = self.popup_chrome.unwrap_or(POPUP_CHROME);
        let body_max = (height - chrome).max(48.0);
        // Whether the text needs a scroll area at all. Measured once it has been
        // drawn; until then, estimated from the text itself, so a long result
        // does not spend its first frame clipped.
        let scrolls = self
            .popup_scrolls
            .unwrap_or_else(|| self.estimated_body(ctx) > (self.max_popup_height() - chrome));
        let mut measured_body: Option<f32> = None;
        let mut shown_body: Option<f32> = None;
        let mut measured_total: Option<f32> = None;

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_result_popup"),
            builder,
            |ctx, _| {
                #[cfg(target_os = "macos")]
                crate::features::capture::mac_window::round_corners(
                    POPUP_TITLE,
                    theme.metrics.surface_radius as f64,
                );
                #[cfg(windows)]
                crate::features::capture::win_window::round_corners(POPUP_TITLE);

                transparent_panel(ctx, theme, |ui| {
                    let frame = widgets::glass_frame(theme).show(ui, |ui| {
                        ui.set_width(POPUP_WIDTH - 2.0);
                        ui.vertical(|ui| {
                            ui.add_space(9.0);
                            header(ui, theme, &stage_snapshot);

                            match &self.stage {
                                Stage::Loading { since } => {
                                    loading_body(ui, theme, since.elapsed().as_secs_f32());
                                }
                                Stage::Error(msg) => {
                                    error_body(ui, theme, msg);
                                }
                                Stage::Done(r) => {
                                    ui.add_space(6.0);

                                    // Two modes, because a scroll area always
                                    // takes the height it is offered — it never
                                    // shrinks to its text — so wrapping a short
                                    // result in one leaves a gap between the
                                    // text and the buttons that no window size
                                    // can absorb. Short text is drawn plainly
                                    // and the window is sized to it; only text
                                    // that genuinely overflows gets a scroll
                                    // area, at the ceiling height.
                                    //
                                    // `body_frame` rather than `padded`: the
                                    // latter lays out a horizontal row, and a
                                    // scroll area inside one reads the row's
                                    // height — 24 points — as all the space it
                                    // has.
                                    let text = |ui: &mut egui::Ui| {
                                        ui.label(
                                            egui::RichText::new(
                                                crate::shared::logging::clip(&r.original, 400),
                                            )
                                            .font(text::small())
                                            .color(theme.text_dim),
                                        );
                                        ui.add_space(6.0);
                                        ui.label(
                                            egui::RichText::new(&r.translated)
                                                .font(text::translation())
                                                .color(theme.text),
                                        );
                                    };

                                    if scrolls {
                                        let out = body_frame()
                                            .show(ui, |ui| {
                                                egui::ScrollArea::vertical()
                                                    .max_height(body_max)
                                                    .auto_shrink([false, false])
                                                    .show(ui, text)
                                            })
                                            .inner;
                                        shown_body = Some(out.inner_rect.height());
                                        measured_body = Some(out.content_size.y);
                                    } else {
                                        let drawn = body_frame().show(ui, text).response.rect;
                                        // Nothing is clipped in this mode, so
                                        // what was wanted and what was shown are
                                        // the same number.
                                        shown_body = Some(drawn.height());
                                        measured_body = Some(drawn.height());
                                    }
                                }
                            }

                            ui.add_space(8.0);
                            // Fluent puts the action row on a faint strip with a
                            // rule above it; on macOS the rule alone is enough.
                            let footer_top = ui.cursor().min.y;
                            widgets::separator(ui, theme);
                            ui.add_space(7.0);

                            padded(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    let has_result = matches!(self.stage, Stage::Done(_));
                                    ui.add_enabled_ui(has_result, |ui| {
                                        if widgets::primary_button(ui, theme, "Копировать")
                                            .clicked()
                                        {
                                            actions.push(ResultAction::Copy);
                                        }
                                        if widgets::secondary_button(ui, theme, "Поверх оригинала")
                                            .clicked()
                                        {
                                            actions
                                                .push(ResultAction::SwitchTo(ResultView::Inline));
                                        }
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if widgets::icon_button(
                                                ui,
                                                theme,
                                                icons::close,
                                                24.0,
                                                "Закрыть · Esc",
                                            )
                                            .clicked()
                                            {
                                                close = true;
                                            }
                                            if widgets::icon_button(
                                                ui,
                                                theme,
                                                icons::pin,
                                                24.0,
                                                "Открыть в отдельном окне",
                                            )
                                            .clicked()
                                            {
                                                actions.push(ResultAction::SwitchTo(
                                                    ResultView::Window,
                                                ));
                                            }
                                        },
                                    );
                                });
                            });
                            ui.add_space(9.0);

                            let footer = Rect::from_min_max(
                                egui::pos2(ui.min_rect().min.x, footer_top),
                                ui.min_rect().max,
                            );
                            ui.painter().rect_filled(
                                footer,
                                egui::Rounding {
                                    nw: 0.0,
                                    ne: 0.0,
                                    sw: theme.metrics.surface_radius,
                                    se: theme.metrics.surface_radius,
                                },
                                theme.footer,
                            );
                        });
                    });
                    measured_total = Some(frame.response.rect.height());
                });

                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || self.focus_lost(ctx, false) {
                    close = true;
                }
                if copy_requested(ctx) {
                    actions.push(ResultAction::Copy);
                }
                if close {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            },
        );

        // Sized from what was drawn, not from what was predicted. When the text
        // fits, the frame's own height *is* the answer — measuring the pieces
        // and adding them back up reintroduces the rounding the old guess
        // suffered from. When it does not fit, the window goes to the ceiling
        // and the body scrolls inside it.
        if let (Some(total), Some(shown), Some(content)) =
            (measured_total, shown_body, measured_body)
        {
            // In plain mode `content` is what was drawn, so this asks whether
            // that would have fitted; in scrolling mode it asks whether it
            // still overflows.
            let allowance = self.max_popup_height() - (total - shown).max(0.0);
            let scrolling = content > allowance + 0.5;
            let wanted = if scrolling {
                self.max_popup_height()
            } else {
                total
            };
            if self.popup_scrolls != Some(scrolling) {
                self.popup_scrolls = Some(scrolling);
                ctx.request_repaint();
            }
            // Everything that is not the body: header, separator, buttons,
            // spacing. This is what the body's allowance is measured against.
            let chrome = (total - shown).max(0.0);

            if self.popup_wanted.is_none_or(|w| (w - wanted).abs() > 0.5)
                || self.popup_chrome.is_none_or(|c| (c - chrome).abs() > 0.5)
            {
                self.popup_wanted = Some(wanted);
                self.popup_chrome = Some(chrome);
                ctx.request_repaint();
            }
        }

        self.actions.extend(actions);
        if close {
            self.actions.push(ResultAction::Close);
        }
    }

    /// A cheap identity for what the popup is currently showing.
    fn stage_key(&self) -> u64 {
        match &self.stage {
            Stage::Loading { .. } => 1,
            Stage::Error(msg) => 2 ^ (msg.len() as u64) << 8,
            Stage::Done(r) => {
                3 ^ (r.original.len() as u64) << 8 ^ (r.translated.len() as u64) << 32
            }
        }
    }

    /// Tall enough for what is in it, and no taller.
    ///
    /// The first frame has nothing measured yet, so the text is laid out to get
    /// a starting figure; from the second frame on the window follows what was
    /// actually drawn. Text laid out on its own is never quite what a `Ui`
    /// produces — spacing, wrapping inside a narrower column — and the gap
    /// showed up as either empty space under the buttons or buttons cut off at
    /// the bottom edge.
    fn popup_height(&self, ctx: &egui::Context) -> f32 {
        let ceiling = self.max_popup_height();
        let floor = 120.0_f32.min(ceiling);

        if let Some(wanted) = self.popup_wanted {
            return wanted.clamp(floor, ceiling);
        }

        if matches!(self.stage, Stage::Done(_)) {
            (POPUP_CHROME + self.estimated_body(ctx)).clamp(floor, ceiling)
        } else {
            150.0_f32.clamp(floor, ceiling)
        }
    }

    /// Rough height of the text, for the frame before anything has been drawn.
    fn estimated_body(&self, ctx: &egui::Context) -> f32 {
        let Stage::Done(r) = &self.stage else {
            return 0.0;
        };
        let width = POPUP_WIDTH - 28.0;
        let measure = |s: &str, font: egui::FontId| {
            ctx.fonts(|f| {
                f.layout(
                    crate::shared::logging::clip(s, 2000).to_string(),
                    font,
                    Color32::WHITE,
                    width,
                )
                .size()
                .y
            })
        };
        measure(&r.original, text::small()) + measure(&r.translated, text::translation())
    }

    /// What is left of the work area once the popup keeps a margin from both
    /// edges. Never smaller than something that can still show a line of text
    /// and the buttons.
    fn max_popup_height(&self) -> f32 {
        let (_, _, _, wh) = self.work_area_points();
        (wh - 16.0).clamp(150.0, 460.0)
    }

    /// The area a window may occupy, in the same points the viewport builder
    /// speaks — the desktop minus the taskbar, the menu bar and the Dock.
    fn work_area_points(&self) -> (f32, f32, f32, f32) {
        let ppp = self.geometry.points_per_pixel;
        let area = crate::features::capture::work_area();
        (
            area.x as f32 / ppp,
            area.y as f32 / ppp,
            area.w as f32 / ppp,
            area.h as f32 / ppp,
        )
    }

    /// Below the selection, flipped above when there is no room, and always
    /// inside the work area — the desktop would put its last line under the
    /// Dock or the taskbar.
    fn popup_position(&self, size: Vec2) -> Pos2 {
        let ppp = self.geometry.points_per_pixel;
        let anchor = Rect::from_min_max(
            Pos2::new(self.anchor.x as f32 / ppp, self.anchor.y as f32 / ppp),
            Pos2::new(
                self.anchor.right() as f32 / ppp,
                self.anchor.bottom() as f32 / ppp,
            ),
        );
        let (x, y, w, h) = self.work_area_points();
        place_beside(anchor, size, Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h)))
    }

    // ── 1b: translation painted over the original ────────────────────────────

    fn render_inline(&mut self, ctx: &egui::Context, theme: &Theme) {
        let mut close = false;
        let mut actions = Vec::new();
        let geometry = self.geometry;
        let anchor = self.anchor;
        let show_original = self.show_original;
        let mut toggle_original = None;

        let builder = ViewportBuilder::default()
            .with_position(geometry.window_pos_points())
            .with_inner_size(geometry.window_size_points())
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_transparent(true);

        // This view covers the desktop exactly like the capture overlay, so it
        // needs the same push past the menu bar and the Dock — without it the
        // frozen backdrop sits below both and no longer lines up with what is
        // actually on screen.
        #[cfg(target_os = "macos")]
        let builder = builder
            .with_title(crate::features::capture::mac_window::INLINE_TITLE)
            .with_active(false);

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_result_inline"),
            builder,
            |ctx, _| {
                #[cfg(target_os = "macos")]
                crate::features::capture::mac_window::present_overlay(
                    crate::features::capture::mac_window::INLINE_TITLE,
                );

                if self.texture.is_none() {
                    if let Some(img) = self.background.take() {
                        self.texture =
                            Some(ctx.load_texture("inline_bg", img, egui::TextureOptions::NEAREST));
                    }
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(Color32::BLACK))
                    .show(ctx, |ui| {
                        let screen = ui.max_rect();
                        let origin = screen.min;
                        let backdrop = ui.allocate_rect(screen, Sense::click());

                        if let Some(tex) = &self.texture {
                            ui.painter().image(
                                tex.id(),
                                screen,
                                Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                        // Lighter than the capture scrim: the point here is to
                        // read the page, not to aim at it.
                        ui.painter().rect_filled(
                            screen,
                            0.0,
                            Color32::from_rgba_unmultiplied(10, 12, 22, 82),
                        );

                        let patch = geometry.bounds_to_rect(anchor, origin).intersect(screen);
                        let body = match &self.stage {
                            Stage::Done(r) => {
                                if show_original {
                                    r.original.clone()
                                } else {
                                    r.translated.clone()
                                }
                            }
                            Stage::Loading { .. } => "Перевожу…".to_string(),
                            Stage::Error(e) => e.clone(),
                        };

                        // Opaque plate so the original text underneath does not
                        // show through the replacement.
                        //
                        // Grown downwards when the translation is longer than the
                        // text it replaces — which it usually is, going into
                        // Russian — rather than left at the size of the original
                        // with everything past the second line hidden behind a
                        // scrollbar nobody looks for. It stops at the bottom of
                        // the screen, and only then does the body scroll.
                        let plate = {
                            let base = patch.expand(6.0);
                            let width = (base.width() - 16.0).max(40.0);
                            let text_h = ctx.fonts(|f| {
                                f.layout(body.clone(), text::body(), theme.text, width)
                                    .size()
                                    .y
                            });
                            let wanted = text_h + 12.0;
                            let height = base
                                .height()
                                .max(wanted)
                                .min((screen.max.y - base.min.y - 8.0).max(base.height()));
                            Rect::from_min_size(base.min, Vec2::new(base.width(), height))
                        };
                        ui.painter().rect_filled(
                            plate,
                            3.0,
                            if theme.dark {
                                Color32::from_rgb(0x1B, 0x1D, 0x23)
                            } else {
                                Color32::from_rgb(0xF6, 0xF5, 0xF3)
                            },
                        );
                        ui.painter().rect_stroke(
                            plate,
                            3.0,
                            egui::Stroke::new(1.5, theme.sakura_border()),
                        );

                        let mut plate_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(plate.shrink2(Vec2::new(8.0, 6.0)))
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(&mut plate_ui, |ui| {
                                ui.label(
                                    egui::RichText::new(&body)
                                        .font(text::body())
                                        .color(theme.text),
                                );
                            });

                        if backdrop.clicked() {
                            close = true;
                        }
                    });

                // Mini toolbar under the patch.
                let screen = ctx.screen_rect();
                let patch = geometry.bounds_to_rect(anchor, screen.min);
                egui::Area::new(egui::Id::new("inline_toolbar"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(
                        patch.center().x,
                        (patch.max.y + 12.0).min(screen.max.y - 44.0),
                    ))
                    .pivot(Align2::CENTER_TOP)
                    .show(ctx, |ui| {
                        egui::Frame::none()
                            .fill(Color32::from_rgba_unmultiplied(28, 28, 32, 200))
                            .rounding(egui::Rounding::same(9.0))
                            .stroke(egui::Stroke::new(
                                1.0,
                                Color32::from_rgba_unmultiplied(255, 255, 255, 36),
                            ))
                            .inner_margin(egui::Margin::same(3.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    let (src, tgt) = match &self.stage {
                                        Stage::Done(r) => (r.source.badge(), r.target.badge()),
                                        _ => ("—", "—"),
                                    };
                                    if pill(ui, tgt, !show_original).clicked() {
                                        toggle_original = Some(false);
                                    }
                                    if pill(ui, src, show_original).clicked() {
                                        toggle_original = Some(true);
                                    }
                                    ui.add_space(4.0);
                                    if flat(ui, "Копировать").clicked() {
                                        actions.push(ResultAction::Copy);
                                    }
                                    if flat(ui, "В окно ↗").clicked() {
                                        actions.push(ResultAction::SwitchTo(ResultView::Window));
                                    }
                                    if flat(ui, "✕").clicked() {
                                        close = true;
                                    }
                                });
                            });
                    });

                // The inline view is the whole desktop; "clicking away" from it
                // is not a thing, and its own click-anywhere already closes it.
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
                if copy_requested(ctx) {
                    actions.push(ResultAction::Copy);
                }
                if close {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            },
        );

        if let Some(v) = toggle_original {
            self.show_original = v;
        }
        self.actions.extend(actions);
        if close {
            self.actions.push(ResultAction::Close);
        }
    }

    // ── 1c: floating two-column window ───────────────────────────────────────

    fn render_window(&mut self, ctx: &egui::Context, theme: &Theme, history_len: usize) {
        let mut close = false;
        let mut actions = Vec::new();
        let pinned = self.pinned;
        let mut toggle_pin = false;

        // Shrunk to the work area before it is placed: on a small screen, or
        // one with a tall taskbar, the default would not fit and the buttons
        // along its bottom edge would be off screen.
        let (wx, wy, ww, wh) = self.work_area_points();
        let size = Vec2::new(WINDOW_SIZE.x.min(ww - 16.0), WINDOW_SIZE.y.min(wh - 16.0));

        let pos = self
            .window_pos
            .map(|p| {
                Pos2::new(
                    p.x.clamp(wx, (wx + ww - size.x).max(wx)),
                    p.y.clamp(wy, (wy + wh - size.y).max(wy)),
                )
            })
            .unwrap_or_else(|| self.popup_position(size));

        let mut builder = ViewportBuilder::default()
            .with_position(pos)
            .with_inner_size(size)
            .with_decorations(false)
            .with_resizable(true)
            .with_min_inner_size([360.0, 200.0])
            .with_transparent(true);
        if pinned {
            builder = builder.with_always_on_top();
        }
        #[cfg(any(target_os = "macos", windows))]
        let builder = builder.with_title(WINDOW_TITLE);

        let has_result = matches!(self.stage, Stage::Done(_));
        // Laid out separately so it can be placed bottom-up, before the body
        // gets to claim the space.
        let footer = |ui: &mut egui::Ui, actions: &mut Vec<ResultAction>| {
            padded(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.add_enabled_ui(has_result, |ui| {
                        if widgets::primary_button(ui, theme, "Копировать").clicked() {
                            actions.push(ResultAction::Copy);
                        }
                        if widgets::secondary_button(ui, theme, "Озвучить").clicked() {
                            actions.push(ResultAction::Speak);
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("История · {history_len}"))
                                .font(text::caption())
                                .color(theme.text_dim),
                        );
                    });
                });
            });
        };

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_result_window"),
            builder,
            |ctx, _| {
                #[cfg(target_os = "macos")]
                crate::features::capture::mac_window::round_corners(WINDOW_TITLE, 12.0);
                #[cfg(windows)]
                crate::features::capture::win_window::round_corners(WINDOW_TITLE);

                transparent_panel(ctx, theme, |ui| {
                    egui::Frame::none()
                        .fill(theme.glass)
                        .rounding(egui::Rounding::same(12.0))
                        .stroke(theme.border_stroke())
                        .show(ui, |ui| {
                            // The frame fills the window, and the footer is laid
                            // out first from the bottom: whatever it needs is
                            // taken off the top of what the body may use, so the
                            // buttons cannot be pushed past the bottom edge no
                            // matter how the user has resized the window.
                            ui.set_min_size(ui.available_size());
                            ui.with_layout(
                                egui::Layout::bottom_up(egui::Align::Min),
                                |ui| {
                            ui.add_space(9.0);
                            footer(ui, &mut actions);
                            ui.add_space(7.0);
                            widgets::separator(ui, theme);

                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            // Title bar, with the macOS traffic lights the design
                            // draws — on Windows they read as a close affordance
                            // just as well, and the window has no chrome of its own.
                            let bar_h = 34.0;
                            let (bar, bar_resp) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), bar_h),
                                Sense::click_and_drag(),
                            );
                            if bar_resp.drag_started() {
                                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                            }
                            // macOS puts the window controls top left as three
                            // dots; Windows puts a close button top right. Both
                            // are drawn here because the window has no system
                            // chrome of its own to inherit them from.
                            let mut title_left = bar.min.x;
                            let mut controls_right = bar.max.x - 8.0;
                            if theme.metrics.caption == CaptionStyle::TrafficLights {
                                let lights = icons::traffic_lights(
                                    ui.painter(),
                                    egui::pos2(bar.min.x + 12.0, bar.center().y - 5.5),
                                    5.5,
                                    7.0,
                                    true,
                                );
                                title_left = lights.max.x;
                                if ui
                                    .interact(
                                        Rect::from_min_size(lights.min, Vec2::splat(11.0)),
                                        egui::Id::new("win_close"),
                                        Sense::click(),
                                    )
                                    .clicked()
                                {
                                    close = true;
                                }
                            } else {
                                let close_rect = Rect::from_min_size(
                                    egui::pos2(bar.max.x - 30.0, bar.center().y - 11.0),
                                    Vec2::splat(22.0),
                                );
                                let close_resp = ui.interact(
                                    close_rect,
                                    egui::Id::new("win_close"),
                                    Sense::click(),
                                );
                                if close_resp.hovered() {
                                    ui.painter().rect_filled(
                                        close_rect,
                                        theme.control_rounding(),
                                        theme.tint(theme.danger, 190),
                                    );
                                }
                                icons::close(
                                    ui.painter(),
                                    close_rect.shrink(6.0),
                                    if close_resp.hovered() {
                                        Color32::WHITE
                                    } else {
                                        theme.text_dim
                                    },
                                );
                                if close_resp.clicked() {
                                    close = true;
                                }
                                controls_right = close_rect.min.x - 4.0;
                            }
                            let _ = title_left;
                            ui.painter().text(
                                bar.center(),
                                Align2::CENTER_CENTER,
                                "Sakura — перевод",
                                text::small(),
                                theme.text_dim,
                            );
                            let pin_rect = Rect::from_min_size(
                                egui::pos2(controls_right - 26.0, bar.center().y - 11.0),
                                Vec2::splat(22.0),
                            );
                            let pin_resp =
                                ui.interact(pin_rect, egui::Id::new("win_pin"), Sense::click());
                            icons::pin(
                                ui.painter(),
                                pin_rect.shrink(5.0),
                                if pinned { theme.accent } else { theme.text_dim },
                            );
                            if pin_resp
                                .on_hover_text(if pinned {
                                    "Открепить"
                                } else {
                                    "Поверх всех окон"
                                })
                                .clicked()
                            {
                                toggle_pin = true;
                            }
                            ui.painter().hline(
                                bar.x_range(),
                                bar.max.y,
                                egui::Stroke::new(1.0, theme.separator),
                            );

                            match &self.stage {
                                Stage::Loading { since } => {
                                    ui.add_space(24.0);
                                    loading_body(ui, theme, since.elapsed().as_secs_f32());
                                    ui.add_space(24.0);
                                }
                                Stage::Error(msg) => {
                                    ui.add_space(16.0);
                                    error_body(ui, theme, msg);
                                    ui.add_space(16.0);
                                }
                                Stage::Done(r) => {
                                    // Whatever the footer left behind. It is laid
                                    // out first, bottom-up, so this is a real
                                    // number rather than the 46-point guess that
                                    // used to push the buttons off the window.
                                    let body_h = ui.available_height().max(80.0);
                                    ui.allocate_ui(Vec2::new(ui.available_width(), body_h), |ui| {
                                        ui.horizontal_top(|ui| {
                                            let col = (ui.available_width() - 1.0) / 2.0;
                                            column(
                                                ui,
                                                theme,
                                                col,
                                                body_h,
                                                &format!("{} · ОРИГИНАЛ", r.source.badge()),
                                                theme.text_dim,
                                                &r.original,
                                                false,
                                            );
                                            let sep = ui.available_rect_before_wrap();
                                            ui.painter().vline(
                                                sep.min.x,
                                                sep.y_range(),
                                                egui::Stroke::new(1.0, theme.separator),
                                            );
                                            column(
                                                ui,
                                                theme,
                                                col,
                                                body_h,
                                                &format!("{} · ПЕРЕВОД", r.target.badge()),
                                                theme.sakura_deep,
                                                &r.translated,
                                                true,
                                            );
                                        });
                                    });
                                }
                            }

                            });
                                },
                            );
                        });
                });

                // Pinned means "keep this in front of everything", so clicking
                // away is exactly what the user does with it — it must not
                // close then.
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || self.focus_lost(ctx, pinned) {
                    close = true;
                }
                if copy_requested(ctx) {
                    actions.push(ResultAction::Copy);
                }
                if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
                    self.window_pos = Some(outer.min);
                }
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                if close {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            },
        );

        if toggle_pin {
            self.pinned = !self.pinned;
        }
        self.actions.extend(actions);
        if close {
            self.actions.push(ResultAction::Close);
        }
    }

    fn describe(&self) -> HeaderInfo {
        match &self.stage {
            Stage::Done(r) => HeaderInfo {
                source: r.source.badge().to_string(),
                target: r.target.badge().to_string(),
                right: if r.from_cache {
                    format!("{} · из кэша", r.engine.label())
                } else {
                    format!(
                        "{} · {:.1} c",
                        r.engine.label(),
                        r.engine_elapsed.as_secs_f32()
                    )
                },
            },
            Stage::Loading { .. } => HeaderInfo {
                source: "—".into(),
                target: "—".into(),
                right: String::new(),
            },
            Stage::Error(_) => HeaderInfo {
                source: "—".into(),
                target: "—".into(),
                right: "ошибка".into(),
            },
        }
    }
}

struct HeaderInfo {
    source: String,
    target: String,
    right: String,
}

/// The overlay hint promises that the copy shortcut works while a result is on
/// screen, so it has to actually work — egui reports it as a Copy event, which
/// covers Ctrl+C and ⌘C without either being spelled out here.
fn copy_requested(ctx: &egui::Context) -> bool {
    ctx.input(|i| {
        i.events.iter().any(|e| matches!(e, egui::Event::Copy))
            || i.modifiers.command && i.key_pressed(egui::Key::C)
    })
}

fn transparent_panel(ctx: &egui::Context, theme: &Theme, add: impl FnOnce(&mut egui::Ui)) {
    let mut v = ctx.style().visuals.clone();
    v.window_fill = Color32::TRANSPARENT;
    v.panel_fill = Color32::TRANSPARENT;
    ctx.set_visuals(v);
    let _ = theme;
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
fn body_frame() -> egui::Frame {
    egui::Frame::none().inner_margin(egui::Margin::symmetric(14.0, 0.0))
}

fn padded<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
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

fn header(ui: &mut egui::Ui, theme: &Theme, info: &HeaderInfo) {
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

fn loading_body(ui: &mut egui::Ui, theme: &Theme, phase: f32) {
    ui.ctx().request_repaint();
    padded(ui, |ui| {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            icons::spinner(ui.painter(), rect, theme.sakura, phase);
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Перевожу…")
                    .font(text::body())
                    .color(theme.text_dim),
            );
        });
        ui.add_space(6.0);
    });
}

fn error_body(ui: &mut egui::Ui, theme: &Theme, msg: &str) {
    padded(ui, |ui| {
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(msg)
                        .font(text::small())
                        .color(theme.danger),
                );
            });
    });
}

#[allow(clippy::too_many_arguments)]
fn column(
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

fn pill(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let fg = if active {
        Color32::WHITE
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 160)
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), text::small(), fg);
    let (rect, resp) =
        ui.allocate_exact_size(galley.size() + Vec2::new(20.0, 10.0), Sense::click());
    if active {
        ui.painter().rect_filled(
            rect,
            egui::Rounding::same(6.0),
            Color32::from_rgba_unmultiplied(255, 255, 255, 40),
        );
    } else if resp.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::Rounding::same(6.0),
            Color32::from_rgba_unmultiplied(255, 255, 255, 20),
        );
    }
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

fn flat(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let fg = Color32::from_rgba_unmultiplied(255, 255, 255, 216);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), text::small(), fg);
    let (rect, resp) =
        ui.allocate_exact_size(galley.size() + Vec2::new(20.0, 10.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::Rounding::same(6.0),
            Color32::from_rgba_unmultiplied(255, 255, 255, 24),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// Places a floating window against a rectangle on screen: below it when there
/// is room, above it when there is not, and inside `work` regardless.
///
/// `work` is the *work area*, not the desktop — the last line of a translation
/// under the Dock or behind the taskbar is unreadable, which is the same as not
/// being shown.
fn place_beside(anchor: Rect, size: Vec2, work: Rect) -> Pos2 {
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
        anchor.min.x.clamp(work.min.x, (work.max.x - size.x).max(work.min.x)),
        y.clamp(work.min.y, (work.max.y - size.y).max(work.min.y)),
    )
}

#[cfg(test)]
mod placement_tests {
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
}
