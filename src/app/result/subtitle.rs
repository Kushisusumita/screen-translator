//! The translation as a subtitle: one plate at the edge of the screen.
//!
//! For when the screen should stay the screen. Nothing is covered where the
//! text was — the game, the video, the page all remain visible — and the
//! translation appears where the eye already expects a subtitle to be.
//!
//! The plate is sized to its text, centred on the display the capture came
//! from, and steps aside if that would put it over the very text it
//! translates: a game's own subtitles sit in the same spot.

use std::sync::Arc;

use egui::{
    Color32, FontId, Galley, Pos2, Rect, Sense, Vec2, ViewportBuilder, ViewportCommand,
    ViewportId,
};

use super::{chrome, ResultAction, ResultUi, Stage};
use crate::entities::settings::SubtitlePlace;
use crate::features::capture::native;
use crate::ui::theme::wash;
use crate::ui::{icons, Theme};

pub const TITLE: &str = "Sakura subtitle";

const RADIUS: f32 = 10.0;
const PAD: Vec2 = Vec2::new(18.0, 11.0);
const TEXT: Color32 = Color32::from_rgb(250, 250, 252);
const DIM: Color32 = Color32::from_rgb(178, 178, 190);
const ERROR: Color32 = Color32::from_rgb(255, 170, 172);

#[derive(Default)]
pub(super) struct Cache {
    content: Option<Arc<Content>>,
}

/// The subtitle's text, laid out once per result.
struct Content {
    revision: u64,
    /// What decides the layout besides the text itself.
    style_key: (u8, bool, bool),
    /// Smaller, dimmer line above the main one: the original, when asked for.
    above: Option<Arc<Galley>>,
    main: Arc<Galley>,
    /// Room kept on the left for the spinner while waiting.
    lead: f32,
    size: Vec2,
}

fn centred(ctx: &egui::Context, text: &str, size: f32, wrap: f32, colour: Color32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        FontId::proportional(size),
        colour,
        wrap.max(40.0),
    );
    job.halign = egui::Align::Center;
    ctx.fonts(|f| f.layout_job(job))
}

/// Arranges the paragraphs of a capture as a subtitle.
///
/// A few paragraphs — a speaker's name, a line of dialogue, a prompt — keep a
/// line each: run together they read as one garbled sentence. A whole window's
/// worth is a different matter; one line apiece would bury the picture, so
/// past a handful they are strung together with a mark between them.
fn flowing(text: &str) -> String {
    const OWN_LINES: usize = 3;

    let paragraphs: Vec<&str> = text
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if paragraphs.len() <= OWN_LINES {
        paragraphs.join("\n")
    } else {
        paragraphs.join("  ·  ")
    }
}

impl ResultUi {
    pub(super) fn render_subtitle(&mut self, ctx: &egui::Context, theme: &Theme) {
        let content = self.subtitle_content(ctx);
        let work = self.work_points();
        let screen_height = self.monitor.bounds.h as f32 / self.ppp;
        let pos = chrome::place_subtitle(
            content.size,
            work,
            self.anchor_points(),
            self.style.subtitle_place == SubtitlePlace::Top,
            (screen_height * 0.065).max(26.0),
        );

        let builder = ViewportBuilder::default()
            .with_title(TITLE)
            .with_position(pos)
            .with_inner_size(content.size)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_transparent(native::TOOLKIT_TRANSPARENCY)
            .with_always_on_top()
            .with_active(!native::QUIET_WINDOWS)
            .with_visible(!self.starts_hidden())
            .with_mouse_passthrough(self.style.click_through);

        let mut close = false;
        let mut actions = Vec::new();
        let mut toggle_original = false;
        let has_result = matches!(self.stage, Stage::Done(_));
        let interactive = !self.style.click_through;
        let opacity = self.style.subtitle_opacity.clamp(20, 100) as f32 / 100.0;
        let waiting = match &self.stage {
            Stage::Loading { since, .. } => Some(since.elapsed().as_secs_f32()),
            _ => None,
        };

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_result_subtitle"),
            builder,
            |ctx, _| {
                native::apply(
                    TITLE,
                    native::Behaviour {
                        no_activate: true,
                        instant: true,
                        rounded: Some(RADIUS),
                        hidden_from_capture: false,
                        translucent: true,
                    },
                );
                native::reveal(ctx, self.frames);

                let presence = self.presence(ctx);

                chrome::transparent_panel(ctx, |ui| {
                    let screen = ui.max_rect();
                    let response = ui.allocate_rect(screen, Sense::click());
                    let mut painter = ui.painter().clone();
                    painter.set_opacity(presence);

                    painter.rect_filled(
                        screen,
                        RADIUS,
                        wash(10, 10, 14, (opacity * 255.0) as u8),
                    );

                    let centre_x = screen.center().x + content.lead / 2.0;
                    let mut y = screen.min.y + PAD.y;
                    if let Some(above) = &content.above {
                        painter.galley(Pos2::new(centre_x, y), above.clone(), DIM);
                        y += above.size().y + 5.0;
                    }

                    // A soft shadow under the text: on a plate turned mostly
                    // transparent the letters still have to hold against
                    // whatever the game is showing.
                    let shadow = Color32::from_black_alpha(170);
                    painter.galley_with_override_text_color(
                        Pos2::new(centre_x + 1.0, y + 1.0),
                        content.main.clone(),
                        shadow,
                    );
                    painter.galley(Pos2::new(centre_x, y), content.main.clone(), TEXT);

                    if let Some(phase) = waiting {
                        ctx.request_repaint();
                        let side = 16.0;
                        let spot = Rect::from_center_size(
                            Pos2::new(
                                centre_x - content.main.size().x / 2.0 - side / 2.0 - 7.0,
                                y + content.main.size().y / 2.0,
                            ),
                            Vec2::splat(side),
                        );
                        icons::spinner(&painter, spot, theme.sakura, phase);
                    }

                    if interactive {
                        if response.hovered() {
                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if response.clicked() {
                            close = true;
                        }
                        if response.secondary_clicked() && has_result {
                            toggle_original = true;
                        }
                    }
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

        if toggle_original {
            self.show_original = !self.show_original;
        }
        self.actions.extend(actions);
        if close {
            self.actions.push(ResultAction::Close);
        }
    }

    fn subtitle_content(&mut self, ctx: &egui::Context) -> Arc<Content> {
        let style_key = (
            self.style.subtitle_size as u8,
            self.style.subtitle_original,
            self.show_original,
        );
        if let Some(content) = &self.subtitle.content {
            if content.revision == self.revision && content.style_key == style_key {
                return Arc::clone(content);
            }
        }

        let scale = self.scale();
        let work = self.work_points();
        let type_size = self.style.subtitle_size.points() * scale;
        let small = (type_size * 0.62).max(11.0);
        // Wide enough for a sentence on one line, never the whole screen: a
        // subtitle that spans it edge to edge is a banner.
        let wrap = (work.width() * 0.7).min(1180.0 * scale) - PAD.x * 2.0;

        let (above, main, lead) = match &self.stage {
            Stage::Loading { recognised, .. } => {
                let label = chrome::loading_label(recognised.is_some());
                let above = recognised
                    .as_deref()
                    .filter(|_| self.style.subtitle_original)
                    .map(|text| centred(ctx, &flowing(text), small, wrap, DIM));
                (
                    above,
                    centred(ctx, label, type_size * 0.8, wrap, DIM),
                    // The spinner sits to the left of the label; the plate
                    // has to be wide enough to hold both.
                    46.0,
                )
            }
            Stage::Done(result) => {
                if self.show_original {
                    (
                        None,
                        centred(ctx, &flowing(&result.original), type_size, wrap, TEXT),
                        0.0,
                    )
                } else {
                    let above = (self.style.subtitle_original && !result.original.is_empty())
                        .then(|| centred(ctx, &flowing(&result.original), small, wrap, DIM));
                    (
                        above,
                        centred(ctx, &flowing(&result.translated), type_size, wrap, TEXT),
                        0.0,
                    )
                }
            }
            Stage::Error(message) => (
                None,
                centred(ctx, message, (type_size * 0.72).max(12.5), wrap, ERROR),
                0.0,
            ),
            Stage::Empty(message) => (
                None,
                centred(ctx, message, (type_size * 0.72).max(12.5), wrap, DIM),
                0.0,
            ),
        };

        let mut width = main.size().x + lead;
        let mut height = main.size().y;
        if let Some(above) = &above {
            width = width.max(above.size().x);
            height += above.size().y + 5.0;
        }
        let size = Vec2::new(
            (width + PAD.x * 2.0).ceil().max(120.0),
            // A screenful of text is not a subtitle; past this it is clipped
            // rather than allowed to cover the picture.
            (height + PAD.y * 2.0).ceil().min(work.height() * 0.45),
        );

        let content = Arc::new(Content {
            revision: self.revision,
            style_key,
            above,
            main,
            lead,
            size,
        });
        self.subtitle.content = Some(Arc::clone(&content));
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_few_paragraphs_keep_a_line_each() {
        assert_eq!(
            flowing("The Ancient Gate\nYou need the silver key.\n\nPress E"),
            "The Ancient Gate\nYou need the silver key.\nPress E"
        );
    }

    #[test]
    fn many_paragraphs_are_strung_together_instead_of_stacked() {
        assert_eq!(flowing("a\nb\nc\nd"), "a  ·  b  ·  c  ·  d");
    }

    #[test]
    fn stray_whitespace_and_blank_lines_are_dropped() {
        assert_eq!(flowing("  one  \n\n   two "), "one\ntwo");
    }
}
