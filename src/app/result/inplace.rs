//! The translation, painted where the original was.
//!
//! The window is exactly the captured rectangle and shows exactly the captured
//! pixels, so at the moment it appears nothing on screen changes. Then each
//! paragraph is covered by a plate in the colour of its own background, and the
//! translation is set on it in the colour and size of the text it replaces. A
//! gold title stays a gold title; a dialogue box still looks like the game's.
//!
//! This used to be a frozen copy of the *whole desktop* with a box of 12-point
//! text over the selection: the game vanished behind a screenshot and stayed
//! there until dismissed. Now the window covers only what was captured, never
//! takes the focus, and everything around it carries on.
//!
//! Most of this file is about one problem: a translation is rarely the length
//! of its original, so every paragraph has to be given somewhere to grow —
//! away from the edge it is anchored to, over more of its own background and
//! nothing else, and no further than the next paragraph. Only when that is not
//! enough does the type get smaller.
//!
//! When the pieces cannot be told apart — a model read the screenshot itself,
//! or an engine merged two lines — the whole area becomes one plate instead.

use std::sync::Arc;

use egui::{
    Align2, Color32, FontId, Galley, Pos2, Rect, Sense, Stroke, TextureOptions, Vec2,
    ViewportBuilder, ViewportCommand, ViewportId,
};

use super::fit::{self, Grow};
use super::{chrome, ResultAction, ResultUi, Stage};
use crate::entities::settings::ResultView;
use crate::features::capture::frame::{readable_on, Clearance, Rgb, TextColours};
use crate::features::capture::{native, Bounds};
use crate::features::translation::ocr::Align;
use crate::features::translation::PipelineResult;
use crate::shared::i18n::t;
use crate::ui::{icons, Theme};

pub const TITLE: &str = "Sakura translation";

/// The least a plate reaches past its text, sideways and vertically.
const PAD: Vec2 = Vec2::new(3.0, 2.0);
/// Kept between a plate and a neighbouring paragraph.
const GAP: f32 = 3.0;
/// Below this the type stops being readable, whatever the original's size.
const SMALLEST_TYPE: f32 = 10.0;
/// A capture narrower than this cannot hold paragraphs side by side; it is
/// treated as one piece and widened.
const NARROW: f32 = 150.0;
/// How far type may shrink before a paragraph is allowed to spread instead.
const COMFORTABLE: f32 = 0.84;
/// How far it may shrink at all.
const TIGHTEST: f32 = 0.66;
/// Lines set closer than this share of the font's own spacing start to touch.
const CLOSEST_LINES: f32 = 0.88;
/// The size the app's font is measured at when working out what size would
/// reproduce the original. Large, so that a pixel of rounding is nothing.
const REFERENCE: f32 = 64.0;
/// What a size taken from a recogniser's box has to be multiplied by: the box
/// runs from ascender to descender, the font's own measure a little further.
const BOX_TO_SIZE: f32 = 1.3;

/// The plate under text that sat on no surface at all.
const CAPTION_FILL: Rgb = Rgb(16, 16, 20);
const CAPTION_TEXT: Rgb = Rgb(246, 246, 246);
const MESSAGE_FILL: Rgb = Rgb(24, 24, 28);
const ERROR_TEXT: Rgb = Rgb(255, 164, 168);
const QUIET_TEXT: Rgb = Rgb(214, 214, 220);

#[derive(Default)]
pub(super) struct Cache {
    layout: Option<Arc<Layout>>,
}

/// Everything worked out once per result rather than once per frame.
struct Layout {
    revision: u64,
    show_original: bool,
    /// The window, in desktop pixels. Usually the captured rectangle; taller
    /// or wider when the translation needed more room than the original had.
    window: Bounds,
    plates: Vec<Plate>,
    /// Colour for the part of the window the capture does not reach.
    filler: Color32,
}

struct Plate {
    /// In points, from the window's top-left.
    rect: Rect,
    rounding: f32,
    /// The surface under the text, at the plate's four corners.
    corners: [Color32; 4],
    /// The same surface as one colour, for the soft edge.
    fill: Color32,
    text: Arc<Galley>,
    /// Where the text hangs from: its top-left, top-centre or top-right,
    /// according to how it is aligned.
    text_pos: Pos2,
}

/// How far a plate reaches past the box a paragraph was recognised in.
///
/// A recogniser's boxes hug the letters, and by its own idea of a letter: the
/// box ends a pixel or two under the baseline, while the tail of a "g" or a
/// "у" hangs well below it. So the margin grows with the type, and it is
/// deepest underneath — a plate that stops at the box leaves a row of
/// descenders showing under the translation.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Margin {
    side: f32,
    above: f32,
    below: f32,
}

impl Margin {
    fn for_glyphs(height: f32) -> Self {
        Self {
            side: PAD.x.max(height * 0.14),
            above: PAD.y.max(height * 0.12),
            below: (PAD.y + 1.0).max(height * 0.30),
        }
    }

    /// The same margin with room added all round: a caption is a thing of
    /// its own and wants air, where a plate that continues a surface wants
    /// to be as small as it can.
    fn roomier(self, by: f32) -> Self {
        Self {
            side: self.side + by * 1.6,
            above: self.above + by,
            below: self.below + by,
        }
    }

    fn around(self, rect: Rect) -> Rect {
        Rect::from_min_max(
            Pos2::new(rect.min.x - self.side, rect.min.y - self.above),
            Pos2::new(rect.max.x + self.side, rect.max.y + self.below),
        )
    }
}

fn colour(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// What a plate is shaded with, corner by corner.
///
/// A surface that is one colour near enough is made exactly one colour: the
/// few levels of noise between its corners would only show as a faint slope
/// where there was none.
///
/// On a gradient the colours were read at the corners of the *paragraph*
/// (`sampled_at`), and the plate is usually larger. A gradient carries on at
/// the rate it was going, so the colours are carried on with it — stretching
/// them over the larger plate instead would flatten the gradient, and the
/// plate would show at both ends.
fn plate_colours(colours: TextColours, sampled_at: Rect, plate: Rect) -> [Color32; 4] {
    if colours.is_flat() {
        return [colour(colours.background); 4];
    }
    let [top_left, top_right, bottom_left, bottom_right] = colours.corners;
    let at = |x: f32, y: f32| {
        // Only so far: past a paragraph's width again, a guess is a guess.
        let u = ((x - sampled_at.min.x) / sampled_at.width().max(1.0)).clamp(-1.0, 2.0);
        let v = ((y - sampled_at.min.y) / sampled_at.height().max(1.0)).clamp(-1.0, 2.0);
        let channel = |of: fn(Rgb) -> u8| {
            let top = of(top_left) as f32 + (of(top_right) as f32 - of(top_left) as f32) * u;
            let bottom =
                of(bottom_left) as f32 + (of(bottom_right) as f32 - of(bottom_left) as f32) * u;
            (top + (bottom - top) * v).round().clamp(0.0, 255.0) as u8
        };
        Color32::from_rgb(channel(|c| c.0), channel(|c| c.1), channel(|c| c.2))
    };
    [
        at(plate.min.x, plate.min.y),
        at(plate.max.x, plate.min.y),
        at(plate.min.x, plate.max.y),
        at(plate.max.x, plate.max.y),
    ]
}

/// How far outside a paragraph its surface colours are read; see
/// `Frame::text_colours`.
const SAMPLED_OUTSIDE: f32 = 3.0;

fn egui_align(grow: Grow) -> egui::Align {
    match grow {
        Grow::Right => egui::Align::Min,
        Grow::Both => egui::Align::Center,
        Grow::Left => egui::Align::Max,
    }
}

/// Text laid out at a given size, wrapped to a width. `pitch` is the distance
/// from one line to the next; `None` leaves that to the font.
fn galley(
    ctx: &egui::Context,
    text: &str,
    size: f32,
    wrap: f32,
    colour: Color32,
    align: egui::Align,
    pitch: Option<f32>,
) -> Arc<Galley> {
    let font = FontId::proportional(size);
    let line_height = pitch.map(|pitch| pitch.max(row_height(ctx, size) * CLOSEST_LINES));
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat {
            font_id: font,
            color: colour,
            line_height,
            ..Default::default()
        },
    );
    job.wrap.max_width = wrap.max(8.0);
    job.halign = align;
    ctx.fonts(|f| f.layout_job(job))
}

fn row_height(ctx: &egui::Context, size: f32) -> f32 {
    ctx.fonts(|f| f.row_height(&FontId::proportional(size)))
}

/// How tall the text of a galley is: down to the bottom of its last line, and
/// not the slack a wide pitch leaves underneath it.
fn text_height(ctx: &egui::Context, galley: &Galley, size: f32) -> f32 {
    match galley.rows.last() {
        Some(last) => last.rect.min.y - galley.rect.min.y + row_height(ctx, size),
        None => 0.0,
    }
}

/// Where the first and the last line of a galley stand, down from its top.
fn baselines(galley: &Galley) -> Option<(f32, f32)> {
    let first = galley
        .rows
        .iter()
        .find_map(|row| row.glyphs.first().map(|glyph| glyph.pos.y))?;
    let last = galley
        .rows
        .iter()
        .rev()
        .find_map(|row| row.glyphs.first().map(|glyph| glyph.pos.y))?;
    Some((first - galley.rect.min.y, last - galley.rect.min.y))
}

/// What the app's own font makes of a piece of text at [`REFERENCE`] size:
/// where its ink starts and ends and where its baseline runs, all measured
/// down from the top of the line.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Profile {
    top: f32,
    bottom: f32,
    baseline: f32,
}

impl Profile {
    fn of(ctx: &egui::Context, text: &str) -> Option<Self> {
        let laid_out = ctx.fonts(|f| {
            f.layout_no_wrap(
                text.to_owned(),
                FontId::proportional(REFERENCE),
                Color32::WHITE,
            )
        });
        let row = laid_out.rows.first()?;
        let mut top = f32::INFINITY;
        let mut bottom = f32::NEG_INFINITY;
        let mut baseline = None;
        for glyph in &row.glyphs {
            // Spaces have a place in the line and nothing drawn in it.
            if glyph.uv_rect.is_nothing() {
                continue;
            }
            let glyph_top = glyph.pos.y + glyph.uv_rect.offset.y;
            top = top.min(glyph_top);
            bottom = bottom.max(glyph_top + glyph.uv_rect.size.y);
            baseline.get_or_insert(glyph.pos.y);
        }
        let baseline = baseline?;
        (bottom - top >= 1.0).then_some(Self {
            top,
            bottom,
            baseline,
        })
    }

    fn ink(self) -> f32 {
        self.bottom - self.top
    }

    /// The size at which this text would be `ink` tall, and how far below the
    /// middle of its ink its baseline would then run.
    fn scaled_to(self, ink: f32) -> (f32, f32) {
        let size = REFERENCE * ink / self.ink();
        let below_middle = (self.baseline - (self.top + self.bottom) / 2.0) * size / REFERENCE;
        (size, below_middle)
    }
}

/// Whether a word can be broken anywhere. Scripts written without spaces are
/// wrapped between any two characters; for the rest, a break inside a word is
/// a misprint.
fn breaks_anywhere(word: &str) -> bool {
    word.chars().any(|c| c as u32 >= 0x2E80)
}

/// Width of the widest word that must stay in one piece, at this size.
fn widest_word(ctx: &egui::Context, text: &str, size: f32) -> f32 {
    // Only the few longest are worth measuring: width follows length closely
    // enough that the widest is always among them.
    let mut words: Vec<&str> = text
        .split_whitespace()
        .filter(|word| !breaks_anywhere(word))
        .collect();
    words.sort_by_key(|word| std::cmp::Reverse(word.chars().count()));
    words
        .into_iter()
        .take(3)
        .map(|word| {
            ctx.fonts(|f| {
                f.layout_no_wrap(word.to_owned(), FontId::proportional(size), Color32::WHITE)
                    .size()
                    .x
            })
        })
        .fold(0.0, f32::max)
}

/// How much room the text takes at a size and a width — or "infinitely much"
/// when the width is too narrow for its longest word, or when it comes to more
/// lines than `most_rows`. Either way that size then counts as not fitting,
/// rather than as a paragraph with a word split in two.
fn measure(
    ctx: &egui::Context,
    text: &str,
    size: f32,
    width: f32,
    pitch: Option<f32>,
    most_rows: usize,
) -> Vec2 {
    if widest_word(ctx, text, size) > width {
        return Vec2::new(width, f32::INFINITY);
    }
    let laid_out = galley(ctx, text, size, width, Color32::WHITE, egui::Align::Min, pitch);
    if laid_out.rows.len() > most_rows {
        return Vec2::new(width, f32::INFINITY);
    }
    Vec2::new(laid_out.size().x, text_height(ctx, &laid_out, size))
}

/// Sampled colours are trusted only as far as they stay legible: the original
/// may have been pale grey on white because it was large, and the translation
/// set smaller in the same grey would not be read by anyone.
fn legible(colours: TextColours) -> TextColours {
    if colours.text.contrast(colours.background) >= 3.2 {
        colours
    } else {
        TextColours {
            text: readable_on(colours.background),
            ..colours
        }
    }
}

/// What a paragraph is dressed in: the plate's colours and the type's.
///
/// Normally the surface it was read from. But text laid straight over a scene
/// has no surface to continue, and a patch of averaged scenery under the
/// translation reads as a smudge — so it gets a plain dark caption instead,
/// which reads as a caption. The type keeps its colour if it shows on that.
fn dressed(sampled: TextColours) -> TextColours {
    if sampled.busy {
        let text = if sampled.text.contrast(CAPTION_FILL) >= 4.5 {
            sampled.text
        } else {
            CAPTION_TEXT
        };
        return TextColours::flat(CAPTION_FILL, text);
    }
    legible(sampled)
}

/// Whether a single line sits in the middle of its own surface — a label on a
/// button, a title over a panel. Judged by how much of that surface is free on
/// either side: about the same means centred.
fn sits_centred(clear: Clearance) -> bool {
    let (left, right) = (clear.left, clear.right);
    left >= 4.0 && right >= 4.0 && (left - right).abs() <= (left.max(right) * 0.18).max(6.0)
}

/// Narrows the room a paragraph has among its neighbours to the part of it
/// that is the paragraph's own surface.
fn within_surface(room: Rect, block: Rect, clear: Clearance, grow: Grow) -> Rect {
    let left = match grow {
        Grow::Right => block.min.x,
        _ => room.min.x.max(block.min.x - clear.left),
    };
    let right = match grow {
        Grow::Left => block.max.x,
        _ => room.max.x.min(block.max.x + clear.right),
    };
    let (left, right) = if grow == Grow::Both {
        // Kept even on both sides, or it would stop being centred.
        let centre = block.center().x;
        let half = (centre - left)
            .min(right - centre)
            .max(block.width() / 2.0);
        (centre - half, centre + half)
    } else {
        (left.min(block.min.x), right.max(block.max.x))
    };
    let bottom = room.max.y.min(block.max.y + clear.below).max(block.max.y);
    Rect::from_min_max(Pos2::new(left, block.min.y), Pos2::new(right, bottom))
}

/// The outcome of finding a size and a width for one paragraph.
struct Placement {
    size: f32,
    wrap: f32,
    pitch: Option<f32>,
}

/// One way of trying to set a paragraph: the range of type sizes to search,
/// smallest then largest; the width to wrap to and the height to stay within;
/// the distance between lines, if not the font's own; and the most lines it
/// may come to.
struct Attempt(f32, f32, f32, f32, Option<f32>, usize);

/// What matters about the original when fitting its translation.
struct Original {
    block: Rect,
    lines: usize,
    /// Distance between its lines, when it had more than one.
    pitch: Option<f32>,
}

/// Finds the type size and the line width for a paragraph.
///
/// One line stays one line for as long as it can: it runs on along its own
/// surface, then gives up some size, and wraps only when neither is enough —
/// a label that turns into two lines no longer looks like a label.
///
/// A paragraph keeps its shape instead: as many lines as it had, the same
/// distance apart, in the narrowest column that will hold them — its own
/// width if that is enough, wider if the translation is longer. Only when the
/// surface is not wide enough for that does it take more lines, close them
/// up, and finally shrink.
///
/// And when nothing fits, it takes whatever room its neighbours leave — a
/// plate running off the end of a button is ugly, but less so than a
/// translation nobody can read.
fn place(
    ctx: &egui::Context,
    text: &str,
    preferred: f32,
    original: &Original,
    surface: Rect,
    neighbours: Rect,
) -> Placement {
    const ANY: usize = usize::MAX;
    let tightest = (preferred * TIGHTEST).clamp(SMALLEST_TYPE, preferred);
    let comfortable = (preferred * COMFORTABLE).clamp(tightest, preferred);
    let wide = surface.width();
    let tall = surface.height().max(1.0);

    let mut attempts: Vec<Attempt> = Vec::new();
    if original.lines <= 1 {
        attempts.push(Attempt(comfortable, preferred, wide, f32::MAX, None, 1));
        attempts.push(Attempt(tightest, comfortable, wide, f32::MAX, None, 1));
        attempts.push(Attempt(comfortable, preferred, wide, tall, None, ANY));
        attempts.push(Attempt(tightest, comfortable, wide, tall, None, ANY));
    } else {
        let own = (original.block.width() * 1.05).min(wide);
        for size in [preferred, comfortable] {
            let holds = |width: f32| {
                measure(ctx, text, size, width, original.pitch, original.lines)
                    .y
                    .is_finite()
            };
            if let Some(width) = fit::narrowest(own, wide, holds) {
                return Placement {
                    size,
                    wrap: width,
                    pitch: original.pitch,
                };
            }
        }
        if original.pitch.is_some() {
            attempts.push(Attempt(comfortable, preferred, own, tall, original.pitch, ANY));
        }
        attempts.push(Attempt(comfortable, preferred, own, tall, None, ANY));
        if original.pitch.is_some() {
            attempts.push(Attempt(comfortable, preferred, wide, tall, original.pitch, ANY));
        }
        attempts.push(Attempt(comfortable, preferred, wide, tall, None, ANY));
        attempts.push(Attempt(tightest, comfortable, wide, tall, None, ANY));
    }

    for Attempt(smallest, largest, width, height, pitch, most_rows) in attempts {
        let fitted = fit::fit(smallest, largest, width, height, |size, width| {
            measure(ctx, text, size, width, pitch, most_rows)
        });
        if fitted.fits {
            return Placement {
                size: fitted.size,
                wrap: width,
                pitch,
            };
        }
    }

    let width = neighbours.width().max(wide);
    let fitted = fit::fit(
        tightest,
        preferred,
        width,
        neighbours.height().max(1.0),
        |size, width| measure(ctx, text, size, width, None, ANY),
    );
    Placement {
        size: fitted.size,
        wrap: width,
        pitch: None,
    }
}

impl ResultUi {
    pub(super) fn render_inplace(&mut self, ctx: &egui::Context, theme: &Theme) {
        let layout = self.inplace_layout(ctx);
        let ppp = self.ppp;
        let window = layout.window;

        let builder = ViewportBuilder::default()
            .with_title(TITLE)
            .with_position(Pos2::new(window.x as f32 / ppp, window.y as f32 / ppp))
            .with_inner_size([window.w as f32 / ppp, window.h as f32 / ppp])
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            // Where the window can be told never to activate, it is not
            // activated on creation either: that alone would pull the
            // keyboard away from a game.
            .with_active(!native::QUIET_WINDOWS)
            .with_visible(!self.starts_hidden())
            .with_mouse_passthrough(self.style.click_through);

        // The capture keeps its place on screen even when the window had to
        // move or grow around it.
        let image = Rect::from_min_size(
            Pos2::new(
                (self.anchor.x - window.x) as f32 / ppp,
                (self.anchor.y - window.y) as f32 / ppp,
            ),
            Vec2::new(self.anchor.w as f32 / ppp, self.anchor.h as f32 / ppp),
        );

        let mut close = false;
        let mut actions = Vec::new();
        let mut toggle_original = false;
        let has_result = matches!(self.stage, Stage::Done(_));
        let interactive = !self.style.click_through;
        let accent = theme.sakura_soft;

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_result_inplace"),
            builder,
            |ctx, _| {
                native::apply(
                    TITLE,
                    native::Behaviour {
                        no_activate: true,
                        instant: true,
                        rounded: None,
                        // A watched area is grabbed again and again from under
                        // this very window.
                        hidden_from_capture: self.watching,
                        translucent: false,
                    },
                );
                native::place(TITLE, window);

                if self.texture.is_none() {
                    if let Some(shot) = &self.shot {
                        self.texture = Some(ctx.load_texture(
                            "inplace_shot",
                            shot.to_color_image(),
                            TextureOptions::NEAREST,
                        ));
                    }
                }

                let mut screen = Rect::NOTHING;
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(layout.filler))
                    .show(ctx, |ui| {
                        screen = ui.max_rect();
                        let origin = screen.min.to_vec2();
                        let response = ui.allocate_rect(screen, Sense::click());
                        let painter = ui.painter();

                        if let Some(texture) = &self.texture {
                            painter.image(
                                texture.id(),
                                image.translate(origin),
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }

                        match &self.stage {
                            Stage::Loading { since, recognised } => paint_waiting(
                                painter,
                                screen,
                                since.elapsed().as_secs_f32(),
                                recognised.is_some(),
                                theme,
                            ),
                            _ => {
                                // The plates ease in over the original, so the
                                // text appears to turn into its translation
                                // rather than being stamped over.
                                let appear = (self.since.elapsed().as_secs_f32() / 0.18).min(1.0);
                                if appear < 1.0 {
                                    ctx.request_repaint();
                                }
                                let mut fading = painter.clone();
                                fading.set_opacity(appear);
                                for plate in &layout.plates {
                                    paint_plate(&fading, plate, origin);
                                }
                                // Text after every plate, so one paragraph's
                                // soft edge never dims its neighbour's letters.
                                for plate in &layout.plates {
                                    fading.galley(
                                        plate.text_pos + origin,
                                        plate.text.clone(),
                                        Color32::WHITE,
                                    );
                                }
                            }
                        }

                        if self.watching {
                            // A thin frame, so it is clear which area is live.
                            painter.rect_stroke(
                                screen.shrink(0.5),
                                0.0,
                                Stroke::new(1.0_f32, theme.tint(accent, 150)),
                            );
                        }

                        if interactive {
                            if response.clicked() {
                                close = true;
                            }
                            if response.secondary_clicked() && has_result {
                                toggle_original = true;
                            }
                        }
                    });

                let hovered = ctx.input(|i| i.pointer.hover_pos().is_some());

                // Room for the controls only on a result large enough that they
                // do not cover what was asked for.
                if hovered && interactive && screen.width() >= 150.0 && screen.height() >= 44.0 {
                    egui::Area::new(egui::Id::new("inplace_toolbar"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(Pos2::new(screen.max.x - 5.0, screen.min.y + 5.0))
                        .pivot(Align2::RIGHT_TOP)
                        .show(ctx, |ui| {
                            chrome::toolbar_frame().show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 1.0;
                                    if has_result {
                                        let tip = if self.show_original {
                                            t("Show the translation")
                                        } else {
                                            t("Show the original")
                                        };
                                        if chrome::tool_button(
                                            ui,
                                            icons::swap,
                                            tip,
                                            self.show_original,
                                            accent,
                                        )
                                        .clicked()
                                        {
                                            toggle_original = true;
                                        }
                                        if chrome::tool_button(
                                            ui,
                                            icons::clipboard,
                                            t("Copy"),
                                            false,
                                            accent,
                                        )
                                        .clicked()
                                        {
                                            actions.push(ResultAction::Copy);
                                        }
                                    }
                                    let watch_tip = if self.watching {
                                        t("Stop watching this area")
                                    } else {
                                        t("Keep translating this area")
                                    };
                                    if chrome::tool_button(
                                        ui,
                                        icons::eye,
                                        watch_tip,
                                        self.watching,
                                        accent,
                                    )
                                    .clicked()
                                    {
                                        actions.push(ResultAction::ToggleWatch);
                                    }
                                    if has_result
                                        && chrome::tool_button(
                                            ui,
                                            icons::pin,
                                            t("Open in a separate window"),
                                            false,
                                            accent,
                                        )
                                        .clicked()
                                    {
                                        actions.push(ResultAction::SwitchTo(ResultView::Window));
                                    }
                                    if chrome::tool_button(
                                        ui,
                                        icons::close,
                                        t("Close · Esc"),
                                        false,
                                        accent,
                                    )
                                    .clicked()
                                    {
                                        close = true;
                                    }
                                });
                            });
                        });
                }

                // Only reaches this window where it can hold the focus; where
                // it cannot, Esc arrives as a global shortcut instead.
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

    fn inplace_layout(&mut self, ctx: &egui::Context) -> Arc<Layout> {
        if let Some(layout) = &self.inplace.layout {
            if layout.revision == self.revision && layout.show_original == self.show_original {
                return Arc::clone(layout);
            }
        }
        let layout = Arc::new(self.build_layout(ctx));
        self.inplace.layout = Some(Arc::clone(&layout));
        layout
    }

    fn build_layout(&self, ctx: &egui::Context) -> Layout {
        let size = Vec2::new(
            self.anchor.w as f32 / self.ppp,
            self.anchor.h as f32 / self.ppp,
        );
        let untouched = Layout {
            revision: self.revision,
            show_original: self.show_original,
            window: self.anchor,
            plates: Vec::new(),
            filler: Color32::BLACK,
        };

        match &self.stage {
            Stage::Loading { .. } => untouched,
            // The capture exactly as it was: that *is* the original.
            Stage::Done(_) if self.show_original => untouched,
            Stage::Done(result) if !result.segments.is_empty() && size.x >= NARROW => {
                self.layout_paragraphs(ctx, result, size)
            }
            Stage::Done(result) => {
                let colours =
                    dressed(self.sample(0.0, 0.0, self.anchor.w as f32, self.anchor.h as f32));
                self.layout_single(
                    ctx,
                    &result.translated,
                    colours,
                    17.0 * self.scale(),
                    260.0 * self.scale(),
                    size,
                )
            }
            Stage::Error(message) => self.layout_single(
                ctx,
                message,
                TextColours::flat(MESSAGE_FILL, ERROR_TEXT),
                12.5 * self.scale(),
                320.0 * self.scale(),
                size,
            ),
            Stage::Empty(message) => self.layout_single(
                ctx,
                message,
                TextColours::flat(MESSAGE_FILL, QUIET_TEXT),
                12.5 * self.scale(),
                320.0 * self.scale(),
                size,
            ),
        }
    }

    fn sample(&self, x: f32, y: f32, w: f32, h: f32) -> TextColours {
        match &self.shot {
            Some(shot) => shot.text_colours(x, y, w, h),
            None => TextColours::flat(Rgb(24, 24, 28), Rgb(240, 240, 244)),
        }
    }

    /// How far the surface under a paragraph runs past it, in points.
    fn clearance(&self, x: f32, y: f32, w: f32, h: f32) -> Clearance {
        let px = match &self.shot {
            Some(shot) => shot.clearance(x, y, w, h),
            None => Clearance {
                left: 0.0,
                right: 0.0,
                below: 0.0,
            },
        };
        Clearance {
            left: px.left / self.ppp,
            right: px.right / self.ppp,
            below: px.below / self.ppp,
        }
    }

    /// The size of the original's type and where its first line stands,
    /// measured off the capture itself.
    ///
    /// The recogniser says which letters the line is made of; the app's own
    /// font says how tall those letters are at a known size; the capture says
    /// how tall they really were. The ratio is the size that reproduces them.
    fn measured_type(
        &self,
        ctx: &egui::Context,
        sample: &crate::features::translation::ocr::Line,
        colours: &TextColours,
    ) -> Option<(f32, f32)> {
        let shot = self.shot.as_ref()?;
        let r = sample.rect;
        let (top, bottom) = shot.ink_rows(r.x, r.y, r.w, r.h, colours)?;
        let profile = Profile::of(ctx, &sample.text)?;
        let (size, below_middle) = profile.scaled_to((bottom - top) / self.ppp);
        // Hung from the middle of the ink, where being a little wrong about
        // the size moves the baseline least.
        let baseline = (top + bottom) / 2.0 / self.ppp + below_middle;
        Some((size, baseline))
    }

    /// Every paragraph on a plate of its own, where it was.
    fn layout_paragraphs(&self, ctx: &egui::Context, result: &PipelineResult, size: Vec2) -> Layout {
        let ppp = self.ppp;
        let bounds = Rect::from_min_size(Pos2::ZERO, size).shrink(1.0);
        let blocks: Vec<Rect> = result
            .segments
            .iter()
            .map(|s| {
                Rect::from_min_size(
                    Pos2::new(s.rect.x / ppp, s.rect.y / ppp),
                    Vec2::new(s.rect.w / ppp, s.rect.h / ppp),
                )
            })
            .collect();

        /// What the picture says about one paragraph.
        struct Reading {
            /// The type size as first worked out, before paragraphs of one
            /// size were given one size.
            estimate: f32,
            colours: TextColours,
            grow: Grow,
            /// Room among its neighbours.
            neighbours: Rect,
            /// The part of that room which is the paragraph's own surface.
            surface: Rect,
            margin: Margin,
            /// Where its first line stood, when that could be measured.
            baseline: Option<f32>,
            /// On a caption of its own rather than on the surface it was on.
            caption: bool,
        }

        // First pass: read the picture around each paragraph.
        let mut readings = Vec::with_capacity(blocks.len());
        let mut estimates = Vec::with_capacity(blocks.len());
        let mut all_measured = true;
        for (i, segment) in result.segments.iter().enumerate() {
            let r = segment.rect;
            let sampled = self.sample(r.x, r.y, r.w, r.h);
            let clear = self.clearance(r.x, r.y, r.w, r.h);

            let grow = match segment.align {
                Align::Center => Grow::Both,
                Align::Right => Grow::Left,
                // One line with as much of its surface on the left as on the
                // right is a label in the middle of something.
                Align::Left if segment.lines <= 1 && sits_centred(clear) => Grow::Both,
                Align::Left => Grow::Right,
            };
            let neighbours = fit::room_for(i, &blocks, bounds, GAP, grow);
            // A caption has no surface to stay on: any room will do.
            let surface = if sampled.busy {
                neighbours
            } else {
                within_surface(neighbours, blocks[i], clear, grow)
            };

            let glyph = segment.glyph_height / ppp;
            let measured = self.measured_type(ctx, &segment.sample, &sampled);
            all_measured &= measured.is_some();
            let estimate = measured.map_or(glyph * BOX_TO_SIZE, |(size, _)| size);
            let estimate = estimate.clamp(SMALLEST_TYPE + 1.0, 96.0);
            estimates.push(estimate);
            let margin = Margin::for_glyphs(glyph);
            readings.push(Reading {
                estimate,
                colours: dressed(sampled),
                grow,
                neighbours,
                surface,
                margin: if sampled.busy {
                    margin.roomier(3.0)
                } else {
                    margin
                },
                baseline: measured.map(|(_, baseline)| baseline),
                caption: sampled.busy,
            });
        }

        // Second pass: one size for everything that was set in one size. A
        // size measured off the pixels is good to a pixel or so; one taken
        // from a recogniser's box needs far more forgiving.
        let preferred = fit::harmonise(&estimates, if all_measured { 1.12 } else { 1.3 });

        // How tall a capital stands, as a share of the type size: what turns
        // "this size" into "this much of the picture is letters".
        let cap = Profile::of(ctx, "H").map_or(0.52, |p| (p.baseline - p.top) / REFERENCE);

        // Third pass: set the type and work out what each plate has to cover.
        let mut rects = Vec::with_capacity(blocks.len());
        let mut texts = Vec::with_capacity(blocks.len());
        for (((segment, block), reading), preferred) in result
            .segments
            .iter()
            .zip(&blocks)
            .zip(&readings)
            .zip(&preferred)
        {
            let original = Original {
                block: *block,
                lines: segment.lines,
                pitch: (segment.pitch > 0.0).then(|| segment.pitch / ppp),
            };
            let placement = place(
                ctx,
                &segment.translated,
                *preferred,
                &original,
                reading.surface,
                reading.neighbours,
            );
            let text = galley(
                ctx,
                &segment.translated,
                placement.size,
                placement.wrap,
                colour(reading.colours.text),
                egui_align(reading.grow),
                placement.pitch,
            );
            let width = text.size().x;
            let one_line = segment.lines <= 1 && text.rows.len() <= 1;
            let capital = cap * placement.size;
            let (first, last) = baselines(&text).unwrap_or((capital, capital));

            let top = match reading.baseline {
                // On the line the original stood on. Type that had to shrink
                // is raised to the middle of that line rather than left
                // sitting on its floor.
                Some(baseline) => {
                    let lift = if one_line {
                        cap * (*preferred - placement.size).max(0.0) / 2.0
                    } else {
                        0.0
                    };
                    baseline - lift - first
                }
                None if one_line => block.center().y + capital / 2.0 - first,
                None => block.min.y,
            };
            tracing::debug!(
                block = ?block,
                grow = ?reading.grow,
                surface = ?reading.surface,
                measured = reading.baseline.is_some(),
                estimate = reading.estimate,
                preferred,
                size = placement.size,
                wrap = placement.wrap,
                pitch = ?placement.pitch,
                rows = text.rows.len(),
                "Placed"
            );

            let (left, right, anchor_x) = match reading.grow {
                Grow::Both => {
                    let centre = block.center().x;
                    let half = (width / 2.0).max(block.width() / 2.0);
                    (centre - half, centre + half, centre)
                }
                Grow::Right => (block.min.x, block.max.x.max(block.min.x + width), block.min.x),
                Grow::Left => (block.min.x.min(block.max.x - width), block.max.x, block.max.x),
            };

            // What has to be covered: the original, and the letters of the
            // translation — not the whole of the line they are set on, which
            // reaches well above a capital. The margin is added afterwards,
            // so that two plates whose margins merely touch are not treated
            // as text running into text.
            let letters_top = top + first - capital * 1.3;
            let letters_bottom = top + last + capital * 0.4;
            let rect = Rect::from_min_max(
                Pos2::new(left, block.min.y.min(letters_top)),
                Pos2::new(right, block.max.y.max(letters_bottom)),
            );

            rects.push(rect);
            texts.push((text, Pos2::new(anchor_x, top), rect.min.y));
        }

        let lowest = fit::stack(&mut rects, 1.0);

        let plates = rects
            .iter()
            .zip(texts)
            .zip(readings.iter().zip(&blocks))
            .map(|((rect, (text, pos, original_top)), (reading, block))| {
                let plate = reading.margin.around(*rect);
                Plate {
                    rect: plate,
                    rounding: if reading.caption { 6.0 } else { 2.0 },
                    corners: plate_colours(
                        reading.colours,
                        block.expand(SAMPLED_OUTSIDE / ppp),
                        plate,
                    ),
                    fill: colour(reading.colours.background),
                    text,
                    // Carried along with its plate if stacking moved it.
                    text_pos: Pos2::new(pos.x, pos.y + (rect.min.y - original_top)),
                }
            })
            .collect();

        // Translations that needed more lines than there was space for push
        // the window past the capture's bottom edge.
        let height = size.y.max(lowest + PAD.y + 2.0);
        let room_below = (self.monitor.bounds.bottom() - self.anchor.y).max(self.anchor.h);
        let window = Bounds {
            h: ((height * ppp).ceil() as i32).min(room_below),
            ..self.anchor
        };
        let filler = if window.h > self.anchor.h {
            // Continue whatever the capture ends on.
            let strip = self.sample(0.0, self.anchor.h as f32 - 5.0, self.anchor.w as f32, 3.0);
            colour(strip.background)
        } else {
            Color32::BLACK
        };

        Layout {
            revision: self.revision,
            show_original: self.show_original,
            window,
            plates,
            filler,
        }
    }

    /// The whole area as one plate: for a result with no paragraph positions,
    /// and for messages.
    fn layout_single(
        &self,
        ctx: &egui::Context,
        text: &str,
        colours: TextColours,
        preferred: f32,
        min_width: f32,
        size: Vec2,
    ) -> Layout {
        const PLATE_PAD: Vec2 = Vec2::new(10.0, 7.0);
        let ppp = self.ppp;
        let screen = self.to_points(self.monitor.bounds);
        let text_colour = colour(colours.text);

        // A selection around one word is too narrow to hold a sentence about
        // it; the plate widens rather than wrapping every second word.
        let width = size.x.max(min_width.min(screen.width()));
        let wrap = width - PLATE_PAD.x * 2.0;
        let fitted = fit::fit(
            SMALLEST_TYPE + 0.5,
            preferred,
            wrap,
            (size.y - PLATE_PAD.y * 2.0).max(1.0),
            |size, width| measure(ctx, text, size, width, None, usize::MAX),
        );
        let laid_out = galley(
            ctx,
            text,
            fitted.size,
            wrap,
            text_colour,
            egui::Align::Min,
            None,
        );
        let text_height = laid_out.size().y;
        let height = size.y.max(text_height + PLATE_PAD.y * 2.0);

        // Kept on the display the capture came from: growing off its edge
        // would put the last lines where nobody can read them.
        let m = self.monitor.bounds;
        let w = ((width * ppp).ceil() as i32).min(m.w);
        let h = ((height * ppp).ceil() as i32).min(m.h);
        let window = Bounds {
            x: self.anchor.x.min(m.right() - w).max(m.x),
            y: self.anchor.y.min(m.bottom() - h).max(m.y),
            w,
            h,
        };

        Layout {
            revision: self.revision,
            show_original: self.show_original,
            window,
            plates: vec![Plate {
                rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height)),
                rounding: 0.0,
                corners: plate_colours(
                    colours,
                    Rect::from_min_size(Pos2::ZERO, size),
                    Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height)),
                ),
                fill: colour(colours.background),
                text: laid_out,
                // Centred vertically when there is room to spare, the way a
                // line of dialogue sits in its box.
                text_pos: Pos2::new(
                    PLATE_PAD.x,
                    ((height - text_height) / 2.0).max(PLATE_PAD.y),
                ),
            }],
            filler: colour(colours.background),
        }
    }
}

/// One plate, with an edge that fades out over a few pixels.
///
/// On a flat panel the plate is exactly the panel's colour and its edge cannot
/// be seen either way. On a gradient no single colour matches all the way
/// round, so the plate is shaded from corner to corner the way the surface
/// under it is — soft edge included, or the edge would be the one thing on
/// the plate that is a single colour, and show as a frame.
fn paint_plate(painter: &egui::Painter, plate: &Plate, origin: Vec2) {
    const EDGE: [(f32, f32); 3] = [(3.0, 0.22), (2.0, 0.42), (1.0, 0.68)];
    let rect = plate.rect.translate(origin);

    if plate.corners.iter().all(|corner| *corner == plate.fill) {
        for (grow, alpha) in EDGE {
            painter.rect_filled(
                rect.expand(grow),
                plate.rounding + grow,
                plate.fill.gamma_multiply(alpha),
            );
        }
        painter.rect_filled(rect, plate.rounding, plate.fill);
        return;
    }

    for (grow, alpha) in EDGE {
        shade(
            painter,
            rect.expand(grow),
            plate.corners.map(|corner| corner.gamma_multiply(alpha)),
        );
    }
    shade(painter, rect, plate.corners);
}

/// A rectangle blended between four corner colours: top-left, top-right,
/// bottom-left, bottom-right.
fn shade(painter: &egui::Painter, rect: Rect, corners: [Color32; 4]) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), corners[0]);
    mesh.colored_vertex(rect.right_top(), corners[1]);
    mesh.colored_vertex(rect.left_bottom(), corners[2]);
    mesh.colored_vertex(rect.right_bottom(), corners[3]);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// What the captured area shows while the translation is on its way: the
/// pixels as they were, a soft pulsing frame, and a word on what is happening.
fn paint_waiting(
    painter: &egui::Painter,
    screen: Rect,
    phase: f32,
    recognised: bool,
    theme: &Theme,
) {
    painter.ctx().request_repaint();

    let pulse = 0.55 + 0.45 * (phase * 3.2).sin().abs();
    painter.rect_stroke(
        screen.shrink(1.0),
        0.0,
        Stroke::new(2.0_f32, theme.tint(theme.sakura, (pulse * 255.0) as u8)),
    );

    let label = chrome::loading_label(recognised);
    let pill = chrome::working_pill_width(painter, label);
    if screen.width() >= pill + 12.0 && screen.height() >= 38.0 {
        chrome::working_pill(painter, screen.center(), label, phase, theme.sakura);
    } else if screen.height() >= 16.0 {
        // No room for words: a spinner in the corner says the same.
        let side = (screen.height() - 4.0).min(20.0);
        let spot = Rect::from_center_size(
            Pos2::new(screen.max.x - side / 2.0 - 4.0, screen.center().y),
            Vec2::splat(side),
        );
        painter.circle_filled(spot.center(), side / 2.0 + 1.0, chrome::TOOLBAR_FILL);
        icons::spinner(painter, spot, theme.sakura, phase);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    fn clear(left: f32, right: f32, below: f32) -> Clearance {
        Clearance { left, right, below }
    }

    #[test]
    fn a_plate_reaches_furthest_underneath_where_descenders_hang() {
        let m = Margin::for_glyphs(20.0);
        assert!(m.below > m.above, "{m:?}");
        // The tail of a "g" set in 20-pixel glyphs runs four pixels or so
        // under the recogniser's box.
        assert!(m.below >= 5.0, "{m:?}");

        let block = rect(40.0, 100.0, 200.0, 20.0);
        let plate = m.around(block);
        assert!(plate.contains_rect(block));
        assert_eq!(plate.max.y, 120.0 + m.below);
        assert_eq!(plate.min.y, 100.0 - m.above);
    }

    #[test]
    fn tiny_type_still_gets_a_margin() {
        let m = Margin::for_glyphs(4.0);
        assert!(m.side >= PAD.x && m.above >= PAD.y && m.below > PAD.y);
    }

    #[test]
    fn readable_sampled_colours_are_kept() {
        let gold_on_navy = TextColours::flat(Rgb(24, 28, 44), Rgb(255, 200, 80));
        assert_eq!(legible(gold_on_navy), gold_on_navy);
    }

    #[test]
    fn washed_out_sampled_colours_are_replaced_by_something_legible() {
        let pale = TextColours::flat(Rgb(250, 250, 250), Rgb(205, 205, 205));
        let fixed = legible(pale);
        assert_eq!(fixed.background, pale.background, "the surface must not change");
        assert_eq!(fixed.corners, pale.corners, "the surface must not change");
        assert!(fixed.text.contrast(fixed.background) >= 4.5);
    }

    #[test]
    fn text_over_a_scene_is_given_a_caption_of_its_own() {
        let over_scenery = TextColours {
            busy: true,
            ..TextColours::flat(Rgb(96, 120, 70), Rgb(255, 230, 120))
        };
        let worn = dressed(over_scenery);
        assert_eq!(worn.background, CAPTION_FILL);
        assert!(worn.is_flat() && !worn.busy);
        // Yellow subtitles stay yellow: it shows on the caption.
        assert_eq!(worn.text, Rgb(255, 230, 120));
    }

    #[test]
    fn dark_type_over_a_scene_is_not_kept_on_a_dark_caption() {
        let over_scenery = TextColours {
            busy: true,
            ..TextColours::flat(Rgb(200, 190, 170), Rgb(30, 30, 30))
        };
        let worn = dressed(over_scenery);
        assert!(worn.text.contrast(worn.background) >= 4.5, "{worn:?}");
    }

    #[test]
    fn text_on_a_surface_keeps_the_surface() {
        let on_panel = TextColours::flat(Rgb(24, 28, 44), Rgb(255, 200, 80));
        assert_eq!(dressed(on_panel), on_panel);
    }

    #[test]
    fn a_gradient_is_carried_on_past_where_it_was_sampled() {
        // Black on the left edge of the paragraph, white on the right, and a
        // plate half as wide again on either side.
        let colours = TextColours {
            corners: [Rgb(100, 100, 100), Rgb(200, 200, 200), Rgb(100, 100, 100), Rgb(200, 200, 200)],
            ..TextColours::flat(Rgb(150, 150, 150), Rgb(255, 255, 255))
        };
        let sampled_at = rect(100.0, 0.0, 100.0, 20.0);
        let plate = rect(50.0, 0.0, 200.0, 20.0);
        let [tl, tr, _, _] = plate_colours(colours, sampled_at, plate);
        assert_eq!(tl, Color32::from_rgb(50, 50, 50));
        assert_eq!(tr, Color32::from_rgb(250, 250, 250));
    }

    #[test]
    fn a_plate_on_a_flat_surface_is_exactly_one_colour() {
        // Corners a few levels apart: noise, not a gradient.
        let colours = TextColours {
            corners: [Rgb(24, 28, 44), Rgb(25, 28, 45), Rgb(24, 27, 44), Rgb(24, 28, 43)],
            ..TextColours::flat(Rgb(24, 28, 44), Rgb(255, 255, 255))
        };
        let corners = plate_colours(colours, rect(0.0, 0.0, 10.0, 10.0), rect(0.0, 0.0, 90.0, 10.0));
        assert!(corners.iter().all(|c| *c == Color32::from_rgb(24, 28, 44)));
    }

    #[test]
    fn a_label_with_equal_room_either_side_is_centred() {
        // The middle of a button.
        assert!(sits_centred(clear(44.0, 46.0, 10.0)));
        // A row label: the page edge on one side, the rest of the row on the
        // other.
        assert!(!sits_centred(clear(80.0, 600.0, 10.0)));
        // Hard against an edge is not "in the middle of" anything.
        assert!(!sits_centred(clear(0.0, 2.0, 10.0)));
    }

    #[test]
    fn a_label_stays_on_its_button() {
        // Plenty of room among the neighbours, but only 44 points of button
        // either side of the text.
        let block = rect(124.0, 420.0, 132.0, 24.0);
        let neighbours = rect(0.0, 420.0, 360.0, 40.0);
        let surface = within_surface(neighbours, block, clear(44.0, 44.0, 14.0), Grow::Both);
        assert_eq!(surface.min.x, 80.0);
        assert_eq!(surface.max.x, 300.0);
        assert_eq!(surface.max.y, 458.0);
    }

    #[test]
    fn left_aligned_text_never_gains_room_on_the_left() {
        let block = rect(80.0, 100.0, 100.0, 20.0);
        let neighbours = rect(80.0, 100.0, 700.0, 300.0);
        let surface = within_surface(neighbours, block, clear(80.0, 500.0, 200.0), Grow::Right);
        assert_eq!(surface.min.x, 80.0, "its first letter must stay put");
        assert_eq!(surface.max.x, 680.0);
    }

    #[test]
    fn right_aligned_text_never_gains_room_on_the_right() {
        let block = rect(790.0, 100.0, 130.0, 20.0);
        let neighbours = rect(194.0, 100.0, 726.0, 300.0);
        let surface = within_surface(neighbours, block, clear(400.0, 60.0, 20.0), Grow::Left);
        assert_eq!(surface.max.x, 920.0, "its last letter must stay put");
        assert_eq!(surface.min.x, 390.0);
    }

    #[test]
    fn a_paragraph_with_no_clear_surface_keeps_at_least_its_own_box() {
        let block = rect(100.0, 100.0, 200.0, 20.0);
        let neighbours = rect(100.0, 100.0, 500.0, 200.0);
        let surface = within_surface(neighbours, block, clear(0.0, 0.0, 0.0), Grow::Right);
        assert_eq!(surface, block);
    }

    #[test]
    fn only_scripts_written_without_spaces_may_break_inside_a_word() {
        assert!(breaks_anywhere("機械翻訳"));
        assert!(breaks_anywhere("한국어"));
        assert!(!breaks_anywhere("Нетрудоспособный"));
        assert!(!breaks_anywhere("Donaudampfschiff"));
    }
}
