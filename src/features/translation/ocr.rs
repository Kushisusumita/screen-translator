//! Text recognition, and where on the capture each piece of text was found.
//!
//! The recogniser answers with more than a string: every paragraph comes back
//! with its rectangle, its lines and the height of its glyphs. That geometry is
//! what lets a translation be drawn *where the original was* — same place, same
//! size — instead of being poured into a box somewhere near it. So it is kept
//! all the way through rather than flattened into text at the door.
//!
//! Coordinates leave this module in **capture pixels**: the image is enlarged
//! before it is sent (small text recognises badly), and everything that comes
//! back is divided by that factor again, so nothing downstream has to know an
//! upscale ever happened.

use serde_json::Value;
use tracing::{debug, info, warn};

use crate::entities::language::Language;
use crate::features::capture::screenshot::Encoded;
use crate::features::translation::client::{HTTP, YANDEX_SESSION};
use crate::features::translation::providers::normalize_ocr_text;
use crate::shared::error::AppError;
use crate::shared::i18n::t;
use crate::shared::logging::{clip, redact};

/// A rectangle on the capture, in capture pixels from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PxRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl PxRect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.w / 2.0
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.h / 2.0
    }

    pub fn union(&self, other: PxRect) -> PxRect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        PxRect {
            x,
            y,
            w: self.right().max(other.right()) - x,
            h: self.bottom().max(other.bottom()) - y,
        }
    }
}

/// Which edge a block of text is anchored to.
///
/// Matters the moment a translation is longer than its original: it has to
/// grow somewhere, and growing away from the anchored edge is what keeps a
/// column of values flush and a centred title centred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// One visual line: where it is and what it says, exactly as it was read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Line {
    pub rect: PxRect,
    pub text: String,
}

/// One paragraph as the recogniser saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBlock {
    pub rect: PxRect,
    /// The paragraph on one line: wrapped lines rejoined, hyphenation undone.
    pub text: String,
    /// Height of a typical glyph run, ascender to descender, as the recogniser
    /// boxed it. Good to a fifth or so; see [`Self::sample`] for better.
    pub glyph_height: f32,
    /// How many visual lines the paragraph occupied.
    pub lines: usize,
    pub align: Align,
    /// Its first line, kept apart from the rest. Knowing which letters a line
    /// is made of *and* where exactly it sits is what lets the type size be
    /// measured off the picture instead of guessed from a box.
    pub sample: Line,
    /// Distance from one line to the next, or zero for a single line.
    pub pitch: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Recognised {
    pub blocks: Vec<TextBlock>,
    pub detected: Option<Language>,
}

impl Recognised {
    /// Everything that was read, one paragraph per line.
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// Sends an encoded capture to Yandex OCR. `size` is the capture's own size in
/// pixels, before any enlargement.
pub async fn recognize(image: &Encoded, size: (u32, u32)) -> Result<Recognised, AppError> {
    let s = &*YANDEX_SESSION;
    let url = format!(
        "https://translate.yandex.net/ocr/v1.1/recognize\
         ?srv=tr-image&sid={sid}&lang=*&rotate=auto&yu={yu}&yum={yum}",
        sid = s.ocr_sid,
        yu = s.yu,
        yum = s.yum,
    );

    let part = reqwest::multipart::Part::bytes(image.jpeg.clone())
        .file_name("blob")
        .mime_str("image/jpeg")?;
    let form = reqwest::multipart::Form::new().part("file", part);

    let resp = HTTP
        .post(&url)
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://translate.yandex.com/")
        .header("Origin", "https://translate.yandex.com")
        .multipart(form)
        .send()
        .await?;

    let status = resp.status();
    let body = resp.text().await?;
    debug!(status = status.as_u16(), body = %clip(&body, 600), "OCR response");

    if !status.is_success() {
        // The status and the body are worth having in the log and worth
        // nothing on screen: the user cannot act on either, and the reply
        // can carry session identifiers.
        warn!(status = status.as_u16(), body = %clip(body.trim(), 200), "OCR rejected the image");
        return Err(AppError::Other(
            t("Text recognition is unavailable right now").to_string(),
        ));
    }

    let json: Value = serde_json::from_str(&body).map_err(|e| {
        warn!(error = %e, "OCR response was not JSON");
        AppError::Other(t("Text recognition is unavailable right now").to_string())
    })?;

    let result = parse(&json, image.scale.max(1) as f32, size.0 as f32);

    if result.is_empty() {
        warn!(detected = ?result.detected, "OCR returned no text");
    } else {
        info!(
            blocks = result.blocks.len(),
            chars = result.text().chars().count(),
            detected = result.detected.map(|l| l.code()).unwrap_or("?"),
            "OCR complete"
        );
        debug!(text = %redact(&result.text()));
    }

    Ok(result)
}

fn number(v: &Value, key: &str) -> Option<f32> {
    v.get(key).and_then(Value::as_f64).map(|n| n as f32)
}

fn rect_of(v: &Value, scale: f32) -> Option<PxRect> {
    let rect = PxRect::new(
        number(v, "x")? / scale,
        number(v, "y")? / scale,
        number(v, "w")? / scale,
        number(v, "h")? / scale,
    );
    (rect.w > 0.0 && rect.h > 0.0).then_some(rect)
}

fn median(mut values: Vec<f32>) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(values[values.len() / 2])
}

/// Reads the structured reply. `scale` is how much the image was enlarged
/// before sending; `capture_width` is the width of the capture itself.
fn parse(json: &Value, scale: f32, capture_width: f32) -> Recognised {
    let detected = json
        .pointer("/data/detected_lang")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && *s != "*")
        .and_then(Language::from_code);

    let mut blocks = Vec::new();

    if let Some(raw_blocks) = json.pointer("/data/blocks").and_then(Value::as_array) {
        for block in raw_blocks {
            let Some(boxes) = block.get("boxes").and_then(Value::as_array) else {
                continue;
            };

            // One box per visual line. Kept on separate lines so the
            // normaliser can rejoin wrapped sentences and hyphenated words.
            let mut lines: Vec<&str> = Vec::new();
            let mut line_rects: Vec<PxRect> = Vec::new();
            let mut glyph_heights: Vec<f32> = Vec::new();
            let mut sample: Option<Line> = None;

            for line in boxes {
                let Some(text) = line
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                else {
                    continue;
                };
                lines.push(text);

                let line_rect = rect_of(line, scale);
                let mut rect = line_rect;

                // A line's own box hugs the x-height; its words carry the full
                // glyph box, which is both the better size estimate and the
                // better thing to cover when painting over the original.
                if let Some(words) = line.get("words").and_then(Value::as_array) {
                    for word in words {
                        if let Some(word_rect) = rect_of(word, scale) {
                            glyph_heights.push(word_rect.h);
                            rect = Some(rect.map_or(word_rect, |r| r.union(word_rect)));
                        }
                    }
                }
                if let Some(rect) = rect {
                    line_rects.push(rect);
                    sample.get_or_insert_with(|| Line {
                        rect,
                        text: text.to_owned(),
                    });
                }
                if glyph_heights.is_empty() {
                    if let Some(r) = line_rect {
                        // No words to measure: the line box is roughly the
                        // x-height, and a full glyph run is about half as
                        // tall again.
                        glyph_heights.push(r.h * 1.45);
                    }
                }
            }

            if lines.is_empty() {
                continue;
            }

            let rect = line_rects
                .iter()
                .copied()
                .reduce(|a, b| a.union(b))
                .or_else(|| rect_of(block, scale));
            let Some(rect) = rect else {
                // Text with no position cannot be placed; it is still text.
                blocks.push(TextBlock {
                    rect: PxRect::default(),
                    text: normalize_ocr_text(&lines.join("\n")),
                    glyph_height: 0.0,
                    lines: lines.len(),
                    align: Align::Left,
                    sample: Line::default(),
                    pitch: 0.0,
                });
                continue;
            };

            let text = normalize_ocr_text(&lines.join("\n"));
            if text.is_empty() {
                continue;
            }

            // From the middle of the first line to the middle of the last,
            // shared out evenly: steadier than any one pair of neighbours.
            let pitch = match (line_rects.first(), line_rects.last()) {
                (Some(first), Some(last)) if line_rects.len() >= 2 => {
                    (last.center_y() - first.center_y()) / (line_rects.len() - 1) as f32
                }
                _ => 0.0,
            };

            blocks.push(TextBlock {
                rect,
                text,
                glyph_height: median(glyph_heights).unwrap_or(rect.h / lines.len() as f32),
                lines: lines.len(),
                align: alignment(&line_rects, rect, capture_width),
                sample: sample.unwrap_or_default(),
                pitch: pitch.max(0.0),
            });
        }
    }

    align_columns(&mut blocks);
    for block in &blocks {
        debug!(
            x = block.rect.x.round(),
            y = block.rect.y.round(),
            w = block.rect.w.round(),
            h = block.rect.h.round(),
            glyph = block.glyph_height,
            lines = block.lines,
            align = ?block.align,
            text = %redact(&block.text),
            "Block"
        );
    }
    Recognised { blocks, detected }
}

/// Finds columns that are flush on the right.
///
/// One line on its own says nothing about which edge it hangs from. A column
/// does: `2560 x 1440`, `Ultra`, `High` under one another end at the same
/// pixel and start at different ones. Those are right-aligned values, and a
/// translation that grew to the right would walk off the panel.
///
/// Every other line casts a vote. One that ends where this one ends but
/// starts somewhere else says "right"; one that starts where this one starts
/// but ends somewhere else says "left". Lines that match on both sides are
/// merely the same width — `Ultra` and `High` — and say nothing either way.
fn align_columns(blocks: &mut [TextBlock]) {
    let edges: Vec<(f32, f32, f32)> = blocks
        .iter()
        .map(|b| (b.rect.x, b.rect.right(), b.glyph_height))
        .collect();

    for (i, block) in blocks.iter_mut().enumerate() {
        if block.align != Align::Left || block.lines != 1 || block.rect.w <= 0.0 {
            continue;
        }
        let (left, right, glyph) = edges[i];
        let tolerance = (glyph * 0.35).max(3.0);
        let (mut for_right, mut for_left) = (0, 0);
        for (j, (other_left, other_right, _)) in edges.iter().enumerate() {
            if j == i {
                continue;
            }
            let same_left = (left - other_left).abs() <= tolerance;
            let same_right = (right - other_right).abs() <= tolerance;
            match (same_left, same_right) {
                (false, true) => for_right += 1,
                (true, false) => for_left += 1,
                _ => {}
            }
        }
        if for_right > for_left {
            block.align = Align::Right;
        }
    }
}

/// Which edge a block hangs from, as far as its own lines can say.
///
/// Several lines give it away themselves: ragged on the left with their
/// middles lined up is centred; ragged on the left and flush on the right is
/// right-aligned; flush on the left is the ordinary case. When the lines are
/// all one width there is nothing to compare, and neither is there for a
/// single line — then the block is called centred only if it sits in the
/// middle of the capture with clear margin on both sides: a title, a
/// subtitle.
fn alignment(lines: &[PxRect], block: PxRect, capture_width: f32) -> Align {
    if lines.len() >= 2 {
        let spread = |values: Vec<f32>| {
            let lo = values.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            hi - lo
        };
        let lefts = spread(lines.iter().map(|l| l.x).collect());
        let rights = spread(lines.iter().map(|l| l.right()).collect());
        let centres = spread(lines.iter().map(|l| l.center_x()).collect());
        let tolerance = (block.w * 0.04).max(3.0);

        if lefts > tolerance {
            if centres < lefts * 0.5 && centres <= rights {
                return Align::Center;
            }
            if rights <= tolerance {
                return Align::Right;
            }
            return Align::Left;
        }
        if rights > tolerance {
            return Align::Left;
        }
        // Flush on both sides: lines of one width. Fall through.
    }

    if capture_width > 0.0 {
        let off_centre = (block.center_x() - capture_width / 2.0).abs();
        let margin = block.x.min(capture_width - block.right());
        if off_centre <= capture_width * 0.04 && margin >= capture_width * 0.12 {
            return Align::Center;
        }
    }
    Align::Left
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_plain(v: &Value) -> Recognised {
        parse(v, 1.0, 1000.0)
    }

    #[test]
    fn the_lines_of_a_block_are_rejoined_into_one_paragraph() {
        let v = json!({"data":{"detected_lang":"en","blocks":[
            {"x":0,"y":0,"w":100,"h":40,"boxes":[
                {"x":0,"y":0,"w":100,"h":15,"text":"Machine trans-"},
                {"x":0,"y":20,"w":90,"h":15,"text":"lation is here"}
            ]}
        ]}});
        let r = parse_plain(&v);
        assert_eq!(r.blocks.len(), 1);
        assert_eq!(r.blocks[0].text, "Machine translation is here");
        assert_eq!(r.blocks[0].lines, 2);
        assert_eq!(r.detected, Some(Language::En));
    }

    #[test]
    fn separate_blocks_stay_separate_and_read_one_per_line() {
        let v = json!({"data":{"blocks":[
            {"x":0,"y":0,"w":50,"h":10,"boxes":[{"x":0,"y":0,"w":50,"h":10,"text":"Заголовок"}]},
            {"x":0,"y":30,"w":50,"h":10,"boxes":[{"x":0,"y":30,"w":50,"h":10,"text":"Текст"}]}
        ]}});
        let r = parse_plain(&v);
        assert_eq!(r.blocks.len(), 2);
        assert_eq!(r.text(), "Заголовок\nТекст");
    }

    #[test]
    fn coordinates_come_back_in_capture_pixels_whatever_the_upscale() {
        // The image went out at ×2; the capture it was made from is half that.
        let v = json!({"data":{"blocks":[
            {"x":40,"y":20,"w":200,"h":60,"boxes":[
                {"x":40,"y":20,"w":200,"h":30,"text":"hello",
                 "words":[{"x":40,"y":16,"w":200,"h":40,"text":"hello"}]}
            ]}
        ]}});
        let r = parse(&v, 2.0, 500.0);
        let b = &r.blocks[0];
        assert_eq!(b.rect, PxRect::new(20.0, 8.0, 100.0, 20.0));
        assert_eq!(b.glyph_height, 20.0);
    }

    #[test]
    fn the_block_covers_the_full_glyph_boxes_not_just_the_x_height() {
        // The line box is 15 tall; the words reach above and below it.
        let v = json!({"data":{"blocks":[
            {"x":42,"y":121,"w":462,"h":15,"boxes":[
                {"x":42,"y":121,"w":462,"h":15,"text":"You need",
                 "words":[
                    {"x":42,"y":118,"w":42,"h":20,"text":"You"},
                    {"x":91,"y":118,"w":50,"h":20,"text":"need"}
                 ]}
            ]}
        ]}});
        let b = &parse_plain(&v).blocks[0];
        assert_eq!(b.rect.y, 118.0);
        assert_eq!(b.rect.bottom(), 138.0);
        assert_eq!(b.glyph_height, 20.0);
    }

    #[test]
    fn a_line_without_word_boxes_still_gets_a_plausible_glyph_height() {
        let v = json!({"data":{"blocks":[
            {"x":0,"y":0,"w":100,"h":20,"boxes":[{"x":0,"y":0,"w":100,"h":20,"text":"abc"}]}
        ]}});
        let b = &parse_plain(&v).blocks[0];
        assert!(b.glyph_height > 20.0 && b.glyph_height < 40.0);
    }

    #[test]
    fn lines_with_matching_middles_and_ragged_edges_are_centred() {
        let v = json!({"data":{"blocks":[
            {"x":100,"y":0,"w":400,"h":60,"boxes":[
                {"x":100,"y":0,"w":400,"h":20,"text":"a long first line of text"},
                {"x":200,"y":30,"w":200,"h":20,"text":"short one"}
            ]}
        ]}});
        assert_eq!(parse_plain(&v).blocks[0].align, Align::Center);
    }

    #[test]
    fn lines_sharing_a_left_edge_are_left_aligned() {
        let v = json!({"data":{"blocks":[
            {"x":100,"y":0,"w":400,"h":60,"boxes":[
                {"x":100,"y":0,"w":400,"h":20,"text":"a long first line of text"},
                {"x":100,"y":30,"w":200,"h":20,"text":"short one"}
            ]}
        ]}});
        assert_eq!(parse_plain(&v).blocks[0].align, Align::Left);
    }

    #[test]
    fn lines_ragged_on_the_left_and_flush_on_the_right_are_right_aligned() {
        let v = json!({"data":{"blocks":[
            {"x":100,"y":0,"w":400,"h":60,"boxes":[
                {"x":100,"y":0,"w":400,"h":20,"text":"a long first line of text"},
                {"x":300,"y":30,"w":200,"h":20,"text":"short one"}
            ]}
        ]}});
        assert_eq!(parse_plain(&v).blocks[0].align, Align::Right);
    }

    #[test]
    fn two_lines_of_one_width_in_the_middle_of_the_capture_are_centred() {
        // A two-line subtitle: nothing ragged to go by, but it is plainly in
        // the middle of the picture.
        let v = json!({"data":{"blocks":[
            {"x":300,"y":200,"w":400,"h":60,"boxes":[
                {"x":300,"y":200,"w":400,"h":20,"text":"I told you not to come back."},
                {"x":304,"y":230,"w":392,"h":20,"text":"The council will not forgive."}
            ]}
        ]}});
        assert_eq!(parse(&v, 1.0, 1000.0).blocks[0].align, Align::Center);
    }

    #[test]
    fn two_lines_of_one_width_at_the_left_edge_stay_left_aligned() {
        let v = json!({"data":{"blocks":[
            {"x":20,"y":200,"w":400,"h":60,"boxes":[
                {"x":20,"y":200,"w":400,"h":20,"text":"A paragraph that fills its box"},
                {"x":20,"y":230,"w":398,"h":20,"text":"and carries on to a second line"}
            ]}
        ]}});
        assert_eq!(parse(&v, 1.0, 1000.0).blocks[0].align, Align::Left);
    }

    #[test]
    fn a_column_that_ends_at_one_edge_is_right_aligned() {
        // Labels on the left, values on the right, the way a settings page is.
        let v = json!({"data":{"blocks":[
            {"x":80,"y":110,"w":110,"h":20,"boxes":[{"x":80,"y":110,"w":110,"h":20,"text":"Resolution"}]},
            {"x":790,"y":110,"w":130,"h":20,"boxes":[{"x":790,"y":110,"w":130,"h":20,"text":"2560 x 1440"}]},
            {"x":80,"y":158,"w":150,"h":20,"boxes":[{"x":80,"y":158,"w":150,"h":20,"text":"Texture quality"}]},
            {"x":870,"y":158,"w":50,"h":20,"boxes":[{"x":870,"y":158,"w":50,"h":20,"text":"Ultra"}]}
        ]}});
        let r = parse(&v, 1.0, 1000.0);
        let align = |text: &str| r.blocks.iter().find(|b| b.text == text).unwrap().align;
        assert_eq!(align("2560 x 1440"), Align::Right);
        assert_eq!(align("Ultra"), Align::Right);
        // The labels share a left edge and end wherever their words end.
        assert_eq!(align("Resolution"), Align::Left);
        assert_eq!(align("Texture quality"), Align::Left);
    }

    #[test]
    fn two_values_of_one_width_do_not_hide_the_column_they_are_in() {
        // "Ultra" and "High" happen to be as wide as each other. They still
        // end where the rest of the column ends and start where it does not.
        let v = json!({"data":{"blocks":[
            {"x":816,"y":116,"w":99,"h":16,"boxes":[{"x":816,"y":116,"w":99,"h":16,"text":"2560 x 1440"}]},
            {"x":873,"y":163,"w":46,"h":18,"boxes":[{"x":873,"y":163,"w":46,"h":18,"text":"Ultra"}]},
            {"x":872,"y":209,"w":45,"h":22,"boxes":[{"x":872,"y":209,"w":45,"h":22,"text":"High"}]},
            {"x":841,"y":260,"w":76,"h":16,"boxes":[{"x":841,"y":260,"w":76,"h":16,"text":"Disabled"}]}
        ]}});
        let r = parse(&v, 1.0, 1000.0);
        assert!(r.blocks.iter().all(|b| b.align == Align::Right), "{:?}",
            r.blocks.iter().map(|b| (&b.text, b.align)).collect::<Vec<_>>());
    }

    #[test]
    fn the_first_line_is_kept_as_a_sample_with_its_own_box() {
        let v = json!({"data":{"blocks":[
            {"x":41,"y":121,"w":463,"h":94,"boxes":[
                {"x":41,"y":121,"w":463,"h":15,"text":"You need the silver key",
                 "words":[{"x":41,"y":118,"w":463,"h":20,"text":"You"}]},
                {"x":41,"y":161,"w":440,"h":14,"text":"to open this door.",
                 "words":[{"x":41,"y":159,"w":440,"h":20,"text":"to"}]}
            ]}
        ]}});
        let b = &parse_plain(&v).blocks[0];
        assert_eq!(b.sample.text, "You need the silver key");
        assert_eq!(b.sample.rect, PxRect::new(41.0, 118.0, 463.0, 20.0));
        // Middle of the first line to the middle of the second.
        assert_eq!(b.pitch, 41.0);
    }

    #[test]
    fn a_single_line_has_no_pitch() {
        let v = json!({"data":{"blocks":[
            {"x":0,"y":0,"w":100,"h":20,"boxes":[{"x":0,"y":0,"w":100,"h":20,"text":"abc"}]}
        ]}});
        let b = &parse_plain(&v).blocks[0];
        assert_eq!(b.pitch, 0.0);
        assert_eq!(b.sample.text, "abc");
    }

    #[test]
    fn a_column_of_equal_widths_is_not_mistaken_for_right_aligned() {
        let v = json!({"data":{"blocks":[
            {"x":80,"y":0,"w":100,"h":20,"boxes":[{"x":80,"y":0,"w":100,"h":20,"text":"aaaa"}]},
            {"x":80,"y":40,"w":100,"h":20,"boxes":[{"x":80,"y":40,"w":100,"h":20,"text":"bbbb"}]}
        ]}});
        let r = parse(&v, 1.0, 1000.0);
        assert!(r.blocks.iter().all(|b| b.align == Align::Left));
    }

    #[test]
    fn a_lone_line_in_the_middle_of_the_capture_is_centred() {
        let v = json!({"data":{"blocks":[
            {"x":400,"y":0,"w":200,"h":20,"boxes":[{"x":400,"y":0,"w":200,"h":20,"text":"Title"}]}
        ]}});
        assert_eq!(parse(&v, 1.0, 1000.0).blocks[0].align, Align::Center);
    }

    #[test]
    fn a_lone_line_at_the_left_edge_is_not_called_centred() {
        let v = json!({"data":{"blocks":[
            {"x":10,"y":0,"w":980,"h":20,"boxes":[{"x":10,"y":0,"w":980,"h":20,"text":"Wide"}]}
        ]}});
        assert_eq!(parse(&v, 1.0, 1000.0).blocks[0].align, Align::Left);
    }

    #[test]
    fn a_regional_detection_tag_maps_onto_the_enum() {
        let v = json!({"data":{"detected_lang":"zh-CN","blocks":[]}});
        assert_eq!(parse_plain(&v).detected, Some(Language::Zh));
    }

    #[test]
    fn the_wildcard_detection_answer_means_unknown() {
        let v = json!({"data":{"detected_lang":"*","blocks":[]}});
        assert_eq!(parse_plain(&v).detected, None);
    }

    #[test]
    fn empty_boxes_are_dropped_without_leaving_blank_lines() {
        let v = json!({"data":{"blocks":[
            {"x":0,"y":0,"w":10,"h":10,"boxes":[
                {"x":0,"y":0,"w":10,"h":10,"text":"  "},
                {"x":0,"y":0,"w":10,"h":10,"text":"real"},
                {"x":0,"y":0,"w":10,"h":10,"text":""}
            ]}
        ]}});
        let r = parse_plain(&v);
        assert_eq!(r.text(), "real");
        assert_eq!(r.blocks[0].lines, 1);
    }

    #[test]
    fn text_without_any_geometry_is_kept_rather_than_lost() {
        let v = json!({"data":{"blocks":[{"boxes":[{"text":"no boxes here"}]}]}});
        let r = parse_plain(&v);
        assert_eq!(r.text(), "no boxes here");
        assert_eq!(r.blocks[0].rect.w, 0.0);
    }

    #[test]
    fn a_response_with_no_data_section_yields_nothing_not_a_panic() {
        assert!(parse_plain(&json!({"error":"nope"})).is_empty());
    }

    #[test]
    fn rect_union_covers_both() {
        let a = PxRect::new(10.0, 10.0, 20.0, 5.0);
        let b = PxRect::new(5.0, 12.0, 10.0, 20.0);
        assert_eq!(a.union(b), PxRect::new(5.0, 10.0, 25.0, 22.0));
    }
}
