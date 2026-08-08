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
}

const POPUP_WIDTH: f32 = 380.0;
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

    // ── 1a: glass popup at the selection ─────────────────────────────────────

    fn render_popup(&mut self, ctx: &egui::Context, theme: &Theme) {
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

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_result_popup"),
            builder,
            |ctx, _| {
                transparent_panel(ctx, theme, |ui| {
                    widgets::glass_frame(theme).show(ui, |ui| {
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
                                    padded(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(crate::shared::logging::clip(
                                                &r.original,
                                                400,
                                            ))
                                            .font(text::small())
                                            .color(theme.text_dim),
                                        );
                                    });
                                    ui.add_space(6.0);
                                    padded(ui, |ui| {
                                        egui::ScrollArea::vertical()
                                            .max_height(height - 150.0)
                                            .auto_shrink([false, true])
                                            .show(ui, |ui| {
                                                ui.label(
                                                    egui::RichText::new(&r.translated)
                                                        .font(text::translation())
                                                        .color(theme.text),
                                                );
                                            });
                                    });
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
                });

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

        self.actions.extend(actions);
        if close {
            self.actions.push(ResultAction::Close);
        }
    }

    fn popup_height(&self, ctx: &egui::Context) -> f32 {
        let Stage::Done(r) = &self.stage else {
            return 150.0;
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
        let original = measure(&r.original, text::small()).min(48.0);
        let translated = measure(&r.translated, text::translation()).min(260.0);
        (110.0 + original + translated).clamp(150.0, 460.0)
    }

    /// Below the selection, flipped above when there is no room, and always
    /// inside the desktop.
    fn popup_position(&self, size: Vec2) -> Pos2 {
        let ppp = self.geometry.points_per_pixel;
        let desktop = self.geometry.desktop;
        let (dx, dy) = (desktop.x as f32 / ppp, desktop.y as f32 / ppp);
        let (dw, dh) = (desktop.w as f32 / ppp, desktop.h as f32 / ppp);

        let ax = self.anchor.x as f32 / ppp;
        let below = self.anchor.bottom() as f32 / ppp + 8.0;
        let above = self.anchor.y as f32 / ppp - size.y - 8.0;

        let y = if below + size.y <= dy + dh {
            below
        } else if above >= dy {
            above
        } else {
            // Neither fits: sit against the bottom edge.
            (dy + dh - size.y).max(dy)
        };
        let x = ax.clamp(dx, (dx + dw - size.x).max(dx));
        Pos2::new(x, y)
    }

    // ── 1b: translation painted over the original ────────────────────────────

    fn render_inline(&mut self, ctx: &egui::Context, theme: &Theme) {
        let mut close = false;
        let mut actions = Vec::new();
        let geometry = self.geometry;
        let anchor = self.anchor;
        let show_original = self.show_original;
        let mut toggle_original = None;

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_result_inline"),
            ViewportBuilder::default()
                .with_position(geometry.window_pos_points())
                .with_inner_size(geometry.window_size_points())
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false)
                .with_always_on_top()
                .with_transparent(true),
            |ctx, _| {
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
                        let plate = patch.expand(6.0);
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

        let pos = self
            .window_pos
            .unwrap_or_else(|| self.popup_position(WINDOW_SIZE));

        let mut builder = ViewportBuilder::default()
            .with_position(pos)
            .with_inner_size(WINDOW_SIZE)
            .with_decorations(false)
            .with_resizable(true)
            .with_min_inner_size([360.0, 200.0])
            .with_transparent(true);
        if pinned {
            builder = builder.with_always_on_top();
        }

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_result_window"),
            builder,
            |ctx, _| {
                transparent_panel(ctx, theme, |ui| {
                    egui::Frame::none()
                        .fill(theme.glass)
                        .rounding(egui::Rounding::same(12.0))
                        .stroke(theme.border_stroke())
                        .show(ui, |ui| {
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
                                    let body_h = (ui.available_height() - 46.0).max(80.0);
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
                                        if widgets::secondary_button(ui, theme, "Озвучить")
                                            .clicked()
                                        {
                                            actions.push(ResultAction::Speak);
                                        }
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "История · {history_len}"
                                                ))
                                                .font(text::caption())
                                                .color(theme.text_dim),
                                            );
                                        },
                                    );
                                });
                            });
                            ui.add_space(9.0);
                        });
                });

                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
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
