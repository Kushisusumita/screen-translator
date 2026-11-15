//! A glass card beside the captured area: the original, the translation, and
//! the controls to do something with it.
//!
//! The card is anchored to what was selected, so it is always on top — there is
//! nothing useful it could sit behind — and, like the other floating views, it
//! does not take the keyboard away from whatever it floats over.

use egui::{Color32, Rect, Vec2, ViewportBuilder, ViewportCommand, ViewportId};

use super::{chrome, ResultAction, ResultUi, Stage};
use crate::entities::settings::ResultView;
use crate::features::capture::native;
use crate::shared::i18n::t;
use crate::ui::theme::text;
use crate::ui::{icons, widgets, Theme};

pub const TITLE: &str = "Sakura result popup";

const WIDTH: f32 = 380.0;
/// Everything in the popup that is not the text: header, separator, button row
/// and the spacing around them. Only a starting figure — the real one is
/// measured from the first frame drawn.
const CHROME: f32 = 150.0;

/// What the popup measured of itself last frame.
///
/// Guessing its height with a constant left a gap under short results and cut
/// the buttons off long ones, because the guess had to be wrong in one
/// direction or the other. So the window follows what was actually drawn.
#[derive(Default)]
pub(super) struct Measured {
    /// Everything that is not the scrolling body.
    chrome: Option<f32>,
    /// The height the window should be: what the frame drew when the text
    /// fits, the ceiling when it does not and the body has to scroll.
    wanted: Option<f32>,
    /// Whether the body is currently in its scrolling mode.
    scrolls: Option<bool>,
    /// Which content these belong to, so a result arriving does not inherit
    /// the size of the spinner that preceded it.
    revision: u64,
}

impl ResultUi {
    pub(super) fn render_popup(&mut self, ctx: &egui::Context, theme: &Theme) {
        if self.popup.revision != self.revision {
            self.popup = Measured {
                revision: self.revision,
                ..Default::default()
            };
        }

        let height = self.popup_height(ctx);
        let pos = chrome::place_beside(
            self.anchor_points(),
            Vec2::new(WIDTH, height),
            self.work_points(),
        );

        let mut close = false;
        let mut actions = Vec::new();
        let header = self.describe();

        let builder = ViewportBuilder::default()
            // Never drawn — there is no chrome — but it is how the window is
            // found again by the platform layer.
            .with_title(TITLE)
            .with_position(pos)
            .with_inner_size([WIDTH, height])
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_transparent(native::TOOLKIT_TRANSPARENCY)
            .with_always_on_top()
            .with_active(!native::QUIET_WINDOWS)
            .with_visible(!self.starts_hidden());

        // How much room the body is allowed this frame, from what the chrome
        // actually measured last frame.
        let chrome_height = self.popup.chrome.unwrap_or(CHROME);
        let body_max = (height - chrome_height).max(48.0);
        // Whether the text needs a scroll area at all. Measured once it has been
        // drawn; until then, estimated from the text itself, so a long result
        // does not spend its first frame clipped.
        let scrolls = self.popup.scrolls.unwrap_or_else(|| {
            self.estimated_body(ctx) > (self.max_popup_height() - chrome_height)
        });
        let mut measured_body: Option<f32> = None;
        let mut shown_body: Option<f32> = None;
        let mut measured_total: Option<f32> = None;

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_result_popup"),
            builder,
            |ctx, _| {
                native::apply(
                    TITLE,
                    native::Behaviour {
                        no_activate: true,
                        instant: true,
                        rounded: Some(theme.metrics.surface_radius),
                        hidden_from_capture: false,
                        translucent: true,
                    },
                );
                let presence = self.presence(ctx);

                chrome::transparent_panel(ctx, |ui| {
                    ui.set_opacity(presence);
                    let frame = widgets::glass_frame(theme).show(ui, |ui| {
                        ui.set_width(WIDTH - 2.0);
                        ui.vertical(|ui| {
                            ui.add_space(9.0);
                            chrome::header(ui, theme, &header);

                            match &self.stage {
                                Stage::Loading { since, recognised } => {
                                    chrome::loading_body(
                                        ui,
                                        theme,
                                        since.elapsed().as_secs_f32(),
                                        recognised.as_deref(),
                                    );
                                }
                                Stage::Error(msg) => {
                                    chrome::message_body(ui, msg, theme.danger);
                                }
                                Stage::Empty(msg) => {
                                    chrome::message_body(ui, msg, theme.text_dim);
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
                                    let body = |ui: &mut egui::Ui| {
                                        if !r.original.is_empty() {
                                            ui.label(
                                                egui::RichText::new(crate::shared::logging::clip(
                                                    &r.original,
                                                    400,
                                                ))
                                                .font(text::small())
                                                .color(theme.text_dim),
                                            );
                                            ui.add_space(6.0);
                                        }
                                        ui.label(
                                            egui::RichText::new(&r.translated)
                                                .font(text::translation())
                                                .color(theme.text),
                                        );
                                    };

                                    if scrolls {
                                        let out = chrome::body_frame()
                                            .show(ui, |ui| {
                                                egui::ScrollArea::vertical()
                                                    .max_height(body_max)
                                                    .auto_shrink([false, false])
                                                    .show(ui, body)
                                            })
                                            .inner;
                                        shown_body = Some(out.inner_rect.height());
                                        measured_body = Some(out.content_size.y);
                                    } else {
                                        let drawn =
                                            chrome::body_frame().show(ui, body).response.rect;
                                        // Nothing is clipped in this mode, so
                                        // what was wanted and what was shown are
                                        // the same number.
                                        shown_body = Some(drawn.height());
                                        measured_body = Some(drawn.height());
                                    }
                                }
                            }

                            ui.add_space(8.0);
                            let footer_top = ui.cursor().min.y;
                            widgets::separator(ui, theme);
                            ui.add_space(7.0);

                            chrome::padded(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    let has_result = matches!(self.stage, Stage::Done(_));
                                    ui.add_enabled_ui(has_result, |ui| {
                                        if widgets::primary_button(ui, theme, t("Copy")).clicked() {
                                            actions.push(ResultAction::Copy);
                                        }
                                        if widgets::secondary_button(
                                            ui,
                                            theme,
                                            t("Over the original"),
                                        )
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
                                                t("Close · Esc"),
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
                                                t("Open in a separate window"),
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

                            // A faint strip under the action row.
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

                let hovered = ctx.input(|i| i.pointer.hover_pos().is_some());
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
                if chrome::copy_requested(ctx) {
                    actions.push(ResultAction::Copy);
                }
                if self.should_dismiss(ctx, hovered) {
                    close = true;
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
            if self.popup.scrolls != Some(scrolling) {
                self.popup.scrolls = Some(scrolling);
                ctx.request_repaint();
            }
            // Everything that is not the body: header, separator, buttons,
            // spacing. This is what the body's allowance is measured against.
            let chrome_height = (total - shown).max(0.0);

            if self.popup.wanted.is_none_or(|w| (w - wanted).abs() > 0.5)
                || self
                    .popup
                    .chrome
                    .is_none_or(|c| (c - chrome_height).abs() > 0.5)
            {
                self.popup.wanted = Some(wanted);
                self.popup.chrome = Some(chrome_height);
                ctx.request_repaint();
            }
        } else if let Some(total) = measured_total {
            // No scrolling body on screen — a spinner, a message. The frame's
            // own height is the whole answer.
            if self.popup.wanted.is_none_or(|w| (w - total).abs() > 0.5) {
                self.popup.wanted = Some(total);
                ctx.request_repaint();
            }
        }

        self.actions.extend(actions);
        if close {
            self.actions.push(ResultAction::Close);
        }
    }

    /// Tall enough for what is in it, and no taller.
    ///
    /// The first frame has nothing measured yet, so the text is laid out to get
    /// a starting figure; from the second frame on the window follows what was
    /// actually drawn.
    fn popup_height(&self, ctx: &egui::Context) -> f32 {
        let ceiling = self.max_popup_height();
        let floor = 96.0_f32.min(ceiling);

        if let Some(wanted) = self.popup.wanted {
            return wanted.clamp(floor, ceiling);
        }

        if matches!(self.stage, Stage::Done(_)) {
            (CHROME + self.estimated_body(ctx)).clamp(floor, ceiling)
        } else {
            150.0_f32.clamp(floor, ceiling)
        }
    }

    /// Rough height of the text, for the frame before anything has been drawn.
    fn estimated_body(&self, ctx: &egui::Context) -> f32 {
        let Stage::Done(r) = &self.stage else {
            return 0.0;
        };
        let width = WIDTH - 28.0;
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
        (self.work_points().height() - 16.0).clamp(150.0, 460.0)
    }
}
