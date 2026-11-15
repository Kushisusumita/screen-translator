//! A free-floating window with the original and the translation side by side.
//!
//! The one view that behaves like an ordinary window: it can be moved, resized
//! and pinned on top, and it takes the focus — which is what someone who opens
//! a translation "in a window" is asking for.

use egui::{Align2, Color32, Pos2, Rect, Sense, Vec2, ViewportBuilder, ViewportCommand, ViewportId};

use super::{chrome, ResultAction, ResultUi, Stage};
use crate::features::capture::native;
use crate::shared::i18n::t;
use crate::ui::platform::CaptionStyle;
use crate::ui::theme::text;
use crate::ui::{icons, widgets, Theme};

pub const TITLE: &str = "Sakura result window";

const SIZE: Vec2 = Vec2::new(560.0, 300.0);

impl ResultUi {
    /// Whether clicking away from the window should dismiss it.
    ///
    /// Pinning is a request for it to stay, so it overrides the setting; and a
    /// translation still arriving is not something to throw away because the
    /// user carried on working while waiting for it.
    fn window_lost_focus(&mut self, ctx: &egui::Context) -> bool {
        if !self.close_on_click_away || self.pinned || self.is_loading() {
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

    pub(super) fn render_window(&mut self, ctx: &egui::Context, theme: &Theme, history_len: usize) {
        let mut close = false;
        let mut actions = Vec::new();
        let pinned = self.pinned;
        let mut toggle_pin = false;

        // Shrunk to the work area before it is placed: on a small screen, or
        // one with a tall taskbar, the default would not fit and the buttons
        // along its bottom edge would be off screen.
        let work = self.work_points();
        let size = Vec2::new(
            SIZE.x.min(work.width() - 16.0),
            SIZE.y.min(work.height() - 16.0),
        );

        let pos = self
            .window_pos
            .map(|p| {
                Pos2::new(
                    p.x.clamp(work.min.x, (work.max.x - size.x).max(work.min.x)),
                    p.y.clamp(work.min.y, (work.max.y - size.y).max(work.min.y)),
                )
            })
            .unwrap_or_else(|| chrome::place_beside(self.anchor_points(), size, work));

        let builder = ViewportBuilder::default()
            .with_title(TITLE)
            .with_position(pos)
            .with_inner_size(size)
            .with_decorations(false)
            .with_resizable(true)
            .with_min_inner_size([360.0, 200.0])
            .with_transparent(native::TOOLKIT_TRANSPARENCY)
            // Both directions, explicitly. `ViewportBuilder::patch` only emits a
            // window-level command when the new builder *has* a level, so
            // leaving the field unset on unpin says "no opinion" and the window
            // stays on top for the rest of its life.
            .with_window_level(if pinned {
                egui::viewport::WindowLevel::AlwaysOnTop
            } else {
                egui::viewport::WindowLevel::Normal
            });

        let has_result = matches!(self.stage, Stage::Done(_));
        // Laid out separately so it can be placed bottom-up, before the body
        // gets to claim the space.
        let footer = |ui: &mut egui::Ui, actions: &mut Vec<ResultAction>| {
            chrome::padded(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.add_enabled_ui(has_result, |ui| {
                        if widgets::primary_button(ui, theme, t("Copy")).clicked() {
                            actions.push(ResultAction::Copy);
                        }
                        if widgets::secondary_button(ui, theme, t("Speak")).clicked() {
                            actions.push(ResultAction::Speak);
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(
                                t("History · {count}").replace("{count}", &history_len.to_string()),
                            )
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
                native::apply(
                    TITLE,
                    native::Behaviour {
                        rounded: Some(12.0),
                        translucent: true,
                        ..Default::default()
                    },
                );

                chrome::transparent_panel(ctx, |ui| {
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
                            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                                ui.add_space(9.0);
                                footer(ui, &mut actions);
                                ui.add_space(7.0);
                                widgets::separator(ui, theme);

                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    // The window has no system chrome of its
                                    // own, so the title bar and its controls
                                    // are drawn here, in the platform's idiom.
                                    let bar_h = 34.0;
                                    let (bar, bar_resp) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width(), bar_h),
                                        Sense::click_and_drag(),
                                    );
                                    if bar_resp.drag_started() {
                                        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                                    }

                                    let mut controls_right = bar.max.x - 8.0;
                                    if theme.metrics.caption == CaptionStyle::TrafficLights {
                                        let lights = icons::traffic_lights(
                                            ui.painter(),
                                            egui::pos2(bar.min.x + 12.0, bar.center().y - 5.5),
                                            5.5,
                                            7.0,
                                            true,
                                        );
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

                                    ui.painter().text(
                                        bar.center(),
                                        Align2::CENTER_CENTER,
                                        t("Sakura — translation"),
                                        text::small(),
                                        theme.text_dim,
                                    );
                                    let pin_rect = Rect::from_min_size(
                                        egui::pos2(controls_right - 26.0, bar.center().y - 11.0),
                                        Vec2::splat(22.0),
                                    );
                                    let pin_resp = ui.interact(
                                        pin_rect,
                                        egui::Id::new("win_pin"),
                                        Sense::click(),
                                    );
                                    icons::pin(
                                        ui.painter(),
                                        pin_rect.shrink(5.0),
                                        if pinned { theme.accent } else { theme.text_dim },
                                    );
                                    if pin_resp
                                        .on_hover_text(if pinned {
                                            t("Unpin")
                                        } else {
                                            t("Keep on top of other windows")
                                        })
                                        .clicked()
                                    {
                                        toggle_pin = true;
                                    }
                                    ui.painter().hline(
                                        bar.x_range(),
                                        bar.max.y,
                                        egui::Stroke::new(1.0_f32, theme.separator),
                                    );

                                    match &self.stage {
                                        Stage::Loading { since, recognised } => {
                                            ui.add_space(20.0);
                                            chrome::loading_body(
                                                ui,
                                                theme,
                                                since.elapsed().as_secs_f32(),
                                                recognised.as_deref(),
                                            );
                                            ui.add_space(20.0);
                                        }
                                        Stage::Error(msg) => {
                                            ui.add_space(16.0);
                                            chrome::message_body(ui, msg, theme.danger);
                                            ui.add_space(16.0);
                                        }
                                        Stage::Empty(msg) => {
                                            ui.add_space(16.0);
                                            chrome::message_body(ui, msg, theme.text_dim);
                                            ui.add_space(16.0);
                                        }
                                        Stage::Done(r) => {
                                            // Whatever the footer left behind. It
                                            // is laid out first, bottom-up, so
                                            // this is a real number.
                                            let body_h = ui.available_height().max(80.0);
                                            ui.allocate_ui(
                                                Vec2::new(ui.available_width(), body_h),
                                                |ui| {
                                                    ui.horizontal_top(|ui| {
                                                        let col = (ui.available_width() - 1.0) / 2.0;
                                                        chrome::column(
                                                            ui,
                                                            theme,
                                                            col,
                                                            body_h,
                                                            &t("{lang} · ORIGINAL")
                                                                .replace("{lang}", r.source.badge()),
                                                            theme.text_dim,
                                                            &r.original,
                                                            false,
                                                        );
                                                        let sep = ui.available_rect_before_wrap();
                                                        ui.painter().vline(
                                                            sep.min.x,
                                                            sep.y_range(),
                                                            egui::Stroke::new(1.0_f32, theme.separator),
                                                        );
                                                        chrome::column(
                                                            ui,
                                                            theme,
                                                            col,
                                                            body_h,
                                                            &t("{lang} · TRANSLATION")
                                                                .replace("{lang}", r.target.badge()),
                                                            theme.sakura_deep,
                                                            &r.translated,
                                                            true,
                                                        );
                                                    });
                                                },
                                            );
                                        }
                                    }
                                });
                            });
                        });
                });

                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || self.window_lost_focus(ctx) {
                    close = true;
                }
                if chrome::copy_requested(ctx) {
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
}
