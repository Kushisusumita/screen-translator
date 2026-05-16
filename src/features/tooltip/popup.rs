use egui::{Color32, FontId, Rect};

/// Renders a full-screen overlay identical in style to the capture overlay —
/// frozen desktop screenshot + semi-transparent dark tint — with the translation
/// text at the top-left corner. Sets *close_requested = true when the backdrop is clicked.
pub fn render_tooltip(
    ctx: &egui::Context,
    text: &str,
    close_requested: &mut bool,
    bg_texture: Option<&egui::TextureHandle>,
) {
    let screen = ctx.screen_rect();

    // Backdrop — full screen, detects clicks to dismiss.
    let backdrop = egui::Area::new(egui::Id::new("tt_backdrop"))
        .fixed_pos(screen.min)
        .order(egui::Order::Background)
        .show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(screen.size(), egui::Sense::click());

            // 1. Frozen desktop screenshot (same as the capture overlay background).
            if let Some(tex) = bg_texture {
                ui.painter().image(
                    tex.id(),
                    rect,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }

            // 2. Semi-transparent dark tint — same opacity as the capture overlay.
            ui.painter().rect_filled(
                rect,
                0.0,
                Color32::from_rgba_unmultiplied(0, 0, 0, 110),
            );

            resp
        });

    if backdrop.inner.clicked() {
        *close_requested = true;
    }

    // Translation text — top-left corner, word-wrap on X, scrollable on Y.
    egui::Area::new(egui::Id::new("tt_text"))
        .fixed_pos(egui::pos2(24.0, 24.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let max_w = (screen.width() - 48.0).max(200.0);
            egui::ScrollArea::vertical()
                .max_height(screen.height() - 48.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_max_width(max_w);
                    ui.label(
                        egui::RichText::new(text)
                            .font(FontId::proportional(18.0))
                            .color(Color32::WHITE),
                    );
                });
        });
}
