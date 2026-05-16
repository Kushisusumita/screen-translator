use egui::{Color32, ColorImage, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};

pub struct OverlayState {
    pub start: Option<Pos2>,
    pub current: Option<Pos2>,
    pub completed: Option<Rect>,
    pub cancelled: bool,
    /// Screenshot taken at hotkey press — used as background so the overlay
    /// looks like a frozen, slightly dimmed desktop instead of solid black.
    pub background_image: Option<ColorImage>,
    background_texture: Option<TextureHandle>,
}

impl OverlayState {
    pub fn new() -> Self {
        Self {
            start: None,
            current: None,
            completed: None,
            cancelled: false,
            background_image: None,
            background_texture: None,
        }
    }

    pub fn with_screenshot(img: ColorImage) -> Self {
        Self {
            background_image: Some(img),
            ..Self::new()
        }
    }
}

pub fn render_overlay(ctx: &egui::Context, state: &mut OverlayState) {
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.cancelled = true;
        return;
    }

    // Upload screenshot as texture once
    if state.background_texture.is_none() {
        if let Some(img) = state.background_image.take() {
            state.background_texture = Some(
                ctx.load_texture("overlay_bg", img, TextureOptions::LINEAR),
            );
        }
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(Color32::BLACK))
        .show(ctx, |ui| {
            let screen_rect = ui.max_rect();

            // Allocate interactive area first (avoids borrow conflict with painter)
            let response = ui.allocate_rect(screen_rect, Sense::drag());

            // Handle drag input
            if response.drag_started() {
                if let Some(pos) = response.interact_pointer_pos() {
                    state.start = Some(pos);
                    state.current = Some(pos);
                }
            }
            if response.dragged() {
                if let Some(pos) = response.interact_pointer_pos() {
                    state.current = Some(pos);
                }
            }
            if response.drag_stopped() {
                if let (Some(start), Some(current)) = (state.start, state.current) {
                    let rect = Rect::from_two_pos(start, current);
                    if rect.width() > 5.0 && rect.height() > 5.0 {
                        state.completed = Some(rect);
                    } else {
                        state.start = None;
                        state.current = None;
                    }
                }
            }

            let painter = ui.painter();

            // 1. Frozen desktop screenshot as background
            if let Some(ref tex) = state.background_texture {
                painter.image(
                    tex.id(),
                    screen_rect,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }

            // 2. Dark tint over everything
            painter.rect_filled(
                screen_rect,
                0.0,
                Color32::from_rgba_unmultiplied(0, 0, 0, 110),
            );

            // 3. Selection rectangle
            if let (Some(start), Some(current)) = (state.start, state.current) {
                let sel = Rect::from_two_pos(start, current);

                // Re-reveal original screenshot inside selection (no dark tint)
                if let Some(ref tex) = state.background_texture {
                    let sz = screen_rect.size();
                    let uv = Rect::from_min_max(
                        egui::pos2(
                            (sel.min.x - screen_rect.min.x) / sz.x,
                            (sel.min.y - screen_rect.min.y) / sz.y,
                        ),
                        egui::pos2(
                            (sel.max.x - screen_rect.min.x) / sz.x,
                            (sel.max.y - screen_rect.min.y) / sz.y,
                        ),
                    );
                    painter.image(tex.id(), sel, uv, Color32::WHITE);
                }

                // Border
                painter.rect_stroke(
                    sel,
                    0.0,
                    Stroke::new(2.0, Color32::from_rgb(64, 196, 255)),
                );

                // Corner handles
                for corner in &[
                    sel.min,
                    Pos2::new(sel.max.x, sel.min.y),
                    Pos2::new(sel.min.x, sel.max.y),
                    sel.max,
                ] {
                    painter.rect_filled(
                        Rect::from_center_size(*corner, Vec2::splat(6.0)),
                        0.0,
                        Color32::from_rgb(64, 196, 255),
                    );
                }

                // Size label
                let label = format!("{}×{}", sel.width() as i32, sel.height() as i32);
                let lx = sel.min.x.max(screen_rect.min.x + 4.0);
                let ly = (sel.min.y - 22.0).max(screen_rect.min.y + 4.0);
                painter.text(
                    Pos2::new(lx, ly),
                    egui::Align2::LEFT_TOP,
                    &label,
                    egui::FontId::proportional(14.0),
                    Color32::WHITE,
                );
            }

            // 4. Hint when nothing selected yet
            if state.start.is_none() {
                let hint = "Выделите область для перевода  |  ESC — отмена";
                let c = screen_rect.center();
                // drop shadow
                painter.text(
                    c + Vec2::new(1.0, 1.0),
                    egui::Align2::CENTER_CENTER,
                    hint,
                    egui::FontId::proportional(20.0),
                    Color32::from_black_alpha(200),
                );
                painter.text(
                    c,
                    egui::Align2::CENTER_CENTER,
                    hint,
                    egui::FontId::proportional(20.0),
                    Color32::WHITE,
                );
            }
        });
}
