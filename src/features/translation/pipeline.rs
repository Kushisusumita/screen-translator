//! Capture → text → translation, as one call.
//!
//! Two routes lead from pixels to a translation:
//!
//! * **read, then translate** — an OCR service returns the text *and where each
//!   paragraph sits*, the engines translate it paragraph for paragraph, and the
//!   two are zipped back together. This is the route that lets a translation be
//!   painted exactly over its original;
//! * **let the model look** — when the user has an AI engine first in line and
//!   has asked it to read screenshots, a single request does both jobs. Faster,
//!   and better on text a recogniser struggles with, but it has no geometry to
//!   offer, so it is only taken for a region the user drew around the text.
//!
//! Everything slow here — encoding included — happens off the UI thread.

use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::entities::language::Language;
use crate::entities::settings::{EngineKind, EngineSettings, Settings};
use crate::features::capture::screenshot::{encode_frame, encode_frame_for_vision};
use crate::features::capture::Frame;
use crate::features::translation::cache;
use crate::features::translation::ocr::{self, Align, Line, PxRect, TextBlock};
use crate::features::translation::providers::{self, ai, TranslateRequest};
use crate::shared::error::AppError;

/// Everything the pipeline needs, copied out of `Settings` so nothing is
/// shared with the UI thread while a request is in flight.
#[derive(Debug, Clone)]
pub struct PipelineParams {
    pub source: Language,
    pub target: Language,
    pub engines: EngineSettings,
}

impl From<&Settings> for PipelineParams {
    fn from(s: &Settings) -> Self {
        Self {
            source: s.source_lang,
            target: s.target_lang,
            engines: s.engines.clone(),
        }
    }
}

/// What the capture is of, which decides how it may be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    /// A rectangle drawn around some text.
    Region,
    /// A whole window or screen: many separate pieces of text, each of which
    /// has to land back in its own place.
    Whole,
}

/// One paragraph: where it was, what it said, what it says now.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// On the capture, in capture pixels.
    pub rect: PxRect,
    pub original: String,
    pub translated: String,
    pub glyph_height: f32,
    /// How many lines the original took.
    pub lines: usize,
    pub align: Align,
    /// The original's first line, for measuring its type off the capture.
    pub sample: Line,
    /// Distance from one line of the original to the next; zero for one line.
    pub pitch: f32,
}

#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// One paragraph per line.
    pub original: String,
    pub translated: String,
    /// The same text paragraph by paragraph, with positions. Empty when the
    /// translation could not be tied to places on the capture — a model that
    /// read the screenshot itself, or an engine that merged two lines.
    pub segments: Vec<Segment>,
    pub engine: EngineKind,
    /// What the text actually turned out to be, once something had read it.
    pub source: Language,
    pub target: Language,
    /// Wall clock for the whole capture: reading plus translation.
    pub elapsed: Duration,
    /// Just the translation call — this is the number the result header shows,
    /// because it is the one that differs between engines.
    pub engine_elapsed: Duration,
    pub from_cache: bool,
}

/// Milestones worth showing before the result is in.
#[derive(Debug, Clone)]
pub enum Progress {
    /// The text has been read; translation is under way.
    Recognised(String),
}

pub async fn run(
    frame: Frame,
    kind: CaptureKind,
    params: &PipelineParams,
    progress: impl Fn(Progress),
) -> Result<PipelineResult, AppError> {
    let started = Instant::now();
    let size = (frame.width() as u32, frame.height() as u32);

    if kind == CaptureKind::Region && params.engines.reads_screenshots() {
        match read_with_ai(&frame, params, started).await {
            Ok(result) => return Ok(result),
            // The model looked and found nothing. A recogniser will not find
            // more, and asking it would only delay saying so.
            Err(AppError::NoText) => return Err(AppError::NoText),
            Err(e) => {
                // A wrong key or a model without vision should cost a moment,
                // not the translation: the ordinary route is still there.
                warn!(error = %e, "The AI could not read the screenshot; using OCR instead");
            }
        }
    }

    let encoded = blocking(move || encode_frame(&frame)).await?;
    let recognised = ocr::recognize(&encoded, size).await?;
    if recognised.is_empty() {
        return Err(AppError::NoText);
    }
    let original = recognised.text();
    if original.trim().is_empty() {
        return Err(AppError::NoText);
    }
    progress(Progress::Recognised(original.clone()));

    let source = source_language(&original, recognised.detected, params.source);

    // Translating a language into itself wastes a round trip and often returns
    // a mangled paraphrase.
    if source == params.target {
        info!(
            lang = source.code(),
            "Text is already in the target language"
        );
        return Ok(PipelineResult {
            segments: align(&recognised.blocks, &original),
            translated: original.clone(),
            original,
            engine: EngineKind::Yandex,
            source,
            target: params.target,
            elapsed: started.elapsed(),
            engine_elapsed: Duration::ZERO,
            from_cache: false,
        });
    }

    if let Some(hit) = cache::get(&original, source, params.target) {
        info!(engine = hit.engine.label(), "Cache hit");
        return Ok(PipelineResult {
            segments: align(&recognised.blocks, &hit.text),
            original,
            translated: hit.text,
            engine: hit.engine,
            source,
            target: params.target,
            elapsed: started.elapsed(),
            engine_elapsed: Duration::ZERO,
            from_cache: true,
        });
    }

    let request = TranslateRequest {
        text: original.clone(),
        source,
        target: params.target,
    };
    let translated = providers::translate(&request, &params.engines).await?;

    cache::put(
        &original,
        source,
        params.target,
        cache::Hit {
            text: translated.text.clone(),
            engine: translated.engine,
        },
    );

    let segments = align(&recognised.blocks, &translated.text);
    if segments.is_empty() && recognised.blocks.len() > 1 {
        info!(
            blocks = recognised.blocks.len(),
            lines = translated.text.lines().count(),
            "The translation does not line up with the paragraphs; showing it as one piece"
        );
    }

    Ok(PipelineResult {
        original,
        translated: translated.text,
        segments,
        engine: translated.engine,
        source,
        target: params.target,
        elapsed: started.elapsed(),
        engine_elapsed: translated.elapsed,
        from_cache: false,
    })
}

async fn read_with_ai(
    frame: &Frame,
    params: &PipelineParams,
    started: Instant,
) -> Result<PipelineResult, AppError> {
    let frame = frame.clone();
    let jpeg = blocking(move || encode_frame_for_vision(&frame)).await?;

    let asked = Instant::now();
    let reading = ai::translate_image(
        &jpeg,
        params.source,
        params.target,
        &params.engines.ai_config,
    )
    .await?;
    let engine_elapsed = asked.elapsed();

    info!(
        ms = engine_elapsed.as_millis() as u64,
        chars_out = reading.translated.chars().count(),
        "Read and translated by the AI"
    );

    Ok(PipelineResult {
        original: reading.original,
        translated: reading.translated,
        segments: Vec::new(),
        engine: EngineKind::Ai,
        source: reading.detected.unwrap_or(params.source),
        target: params.target,
        elapsed: started.elapsed(),
        engine_elapsed,
        from_cache: false,
    })
}

/// Runs CPU-bound work on the blocking pool and flattens its two layers of
/// failure into one.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
}

/// Which language the text is to be translated from.
///
/// Three opinions are on offer, and they are trusted in this order:
///
/// 1. **The letters.** Kana is Japanese and hangul is Korean, whatever anyone
///    guessed. The recogniser reports a line of Japanese as Chinese often
///    enough — both are mostly Han characters — and translated *as* Chinese it
///    comes back as confident nonsense.
/// 2. **A source the user picked from among Chinese, Japanese and Korean**,
///    when the text is in Han characters. A menu in kanji alone — 装備, 設定 —
///    cannot be told from Chinese by looking, so someone playing a Japanese
///    game has to be able to say so and be believed.
/// 3. **The recogniser's guess**, and failing that the configured source. For
///    everything else the guess is the better of the two: the setting is
///    usually left on "detect", and one left on a language by accident is the
///    commonest reason a translation comes back identical to its input.
fn source_language(text: &str, detected: Option<Language>, configured: Language) -> Language {
    let (mut kana, mut hangul, mut han) = (0usize, 0usize, 0usize);
    for c in text.chars() {
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9D => kana += 1,
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => hangul += 1,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF => han += 1,
            _ => {}
        }
    }

    // One stray kana does not make a page Japanese: の turns up in Chinese
    // shop signs. Two do, or one in a line too short to hold a second.
    if kana >= 2 || (kana == 1 && han <= 3) {
        return Language::Ja;
    }
    if hangul >= 1 && hangul >= han {
        return Language::Ko;
    }
    if han > 0 && matches!(configured, Language::Zh | Language::Ja | Language::Ko) {
        return configured;
    }
    detected.unwrap_or(configured)
}

/// Pairs each recognised paragraph with its translation.
///
/// The engines are asked to answer line for line, and nearly always do. When
/// one does not — two lines merged, a stray blank one added — there is no
/// honest way to say which translation belongs where, and a guess would paint
/// the wrong sentence over the wrong place. So the answer is all or nothing:
/// either every paragraph gets its translation, or the list comes back empty
/// and the result is shown as one piece.
pub fn align(blocks: &[TextBlock], translated: &str) -> Vec<Segment> {
    if blocks.is_empty() || blocks.iter().any(|b| b.rect.w <= 0.0 || b.rect.h <= 0.0) {
        return Vec::new();
    }

    let exact: Vec<&str> = translated.split('\n').map(str::trim).collect();
    let lines: Vec<&str> = if exact.len() == blocks.len() {
        exact
    } else {
        // Blank lines between paragraphs are the usual way a count goes
        // wrong, and the one way it can safely be put right.
        let without_blanks: Vec<&str> = exact.into_iter().filter(|l| !l.is_empty()).collect();
        if without_blanks.len() != blocks.len() {
            return Vec::new();
        }
        without_blanks
    };

    blocks
        .iter()
        .zip(lines)
        .map(|(block, line)| Segment {
            rect: block.rect,
            original: block.text.clone(),
            // An engine that answered a paragraph with nothing has not
            // translated it; the original is better than a hole.
            translated: if line.is_empty() {
                block.text.clone()
            } else {
                line.to_string()
            },
            glyph_height: block.glyph_height,
            lines: block.lines,
            align: block.align,
            sample: block.sample.clone(),
            pitch: block.pitch,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(y: f32, text: &str) -> TextBlock {
        TextBlock {
            rect: PxRect::new(10.0, y, 200.0, 20.0),
            text: text.into(),
            glyph_height: 18.0,
            lines: 1,
            align: Align::Left,
            sample: Line {
                rect: PxRect::new(10.0, y, 200.0, 20.0),
                text: text.into(),
            },
            pitch: 0.0,
        }
    }

    #[test]
    fn each_paragraph_gets_the_line_in_its_position() {
        let blocks = [block(0.0, "Title"), block(40.0, "Body text")];
        let segments = align(&blocks, "Заголовок\nОсновной текст");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].translated, "Заголовок");
        assert_eq!(segments[0].rect.y, 0.0);
        assert_eq!(segments[1].translated, "Основной текст");
        assert_eq!(segments[1].original, "Body text");
    }

    #[test]
    fn blank_lines_between_paragraphs_are_forgiven() {
        let blocks = [block(0.0, "a"), block(40.0, "b")];
        let segments = align(&blocks, "а\n\nб\n");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].translated, "б");
    }

    #[test]
    fn a_merged_line_means_no_placement_at_all() {
        // Two paragraphs in, one line out: which half goes where is anyone's
        // guess, and a wrong guess is worse than not guessing.
        let blocks = [block(0.0, "a"), block(40.0, "b")];
        assert!(align(&blocks, "а и б вместе").is_empty());
    }

    #[test]
    fn an_extra_line_means_no_placement_at_all() {
        let blocks = [block(0.0, "a")];
        assert!(align(&blocks, "первая\nвторая").is_empty());
    }

    #[test]
    fn a_paragraph_that_came_back_empty_keeps_its_original() {
        let blocks = [block(0.0, "OK"), block(40.0, "Cancel")];
        let segments = align(&blocks, "\nОтмена");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].translated, "OK");
        assert_eq!(segments[1].translated, "Отмена");
    }

    #[test]
    fn text_without_a_position_cannot_be_placed() {
        let mut nowhere = block(0.0, "floating");
        nowhere.rect = PxRect::new(0.0, 0.0, 0.0, 0.0);
        assert!(align(&[nowhere], "плавающий").is_empty());
    }

    #[test]
    fn kana_makes_it_japanese_whatever_the_recogniser_guessed() {
        // What Yandex said about this very line: Chinese.
        let line = "峠は春まで閉ざされている。";
        assert_eq!(source_language(line, Some(Language::Zh), Language::Auto), Language::Ja);
        // And even over a source set by hand to something else entirely.
        assert_eq!(source_language(line, Some(Language::Zh), Language::En), Language::Ja);
        // Katakana counts as much as hiragana.
        assert_eq!(source_language("ゲームオーバー", None, Language::Auto), Language::Ja);
    }

    #[test]
    fn hangul_makes_it_korean() {
        assert_eq!(
            source_language("계속하려면 아무 키나 누르세요", Some(Language::Zh), Language::Auto),
            Language::Ko
        );
    }

    #[test]
    fn chinese_with_one_borrowed_kana_is_still_chinese() {
        let sign = "欢迎光临优之良品，这里有最好的零食";
        assert_eq!(source_language(sign, Some(Language::Zh), Language::Auto), Language::Zh);
        let with_no = "欢迎光临优の良品，这里有最好的零食";
        assert_eq!(source_language(with_no, Some(Language::Zh), Language::Auto), Language::Zh);
    }

    #[test]
    fn a_menu_in_kanji_alone_is_whatever_the_player_says_it_is() {
        // Nothing in the letters to go by, and the recogniser says Chinese.
        assert_eq!(source_language("装備", Some(Language::Zh), Language::Auto), Language::Zh);
        assert_eq!(source_language("装備", Some(Language::Zh), Language::Ja), Language::Ja);
    }

    #[test]
    fn for_other_scripts_the_recognisers_guess_still_beats_the_setting() {
        // A source left on English by accident, and German on the screen.
        assert_eq!(
            source_language("Einstellungen speichern", Some(Language::De), Language::En),
            Language::De
        );
        // A source of Japanese does not turn English text into Japanese.
        assert_eq!(
            source_language("Press any key", Some(Language::En), Language::Ja),
            Language::En
        );
    }

    #[test]
    fn with_no_guess_at_all_the_setting_stands() {
        assert_eq!(source_language("Hello", None, Language::Auto), Language::Auto);
        assert_eq!(source_language("Hello", None, Language::En), Language::En);
    }

    #[test]
    fn nothing_recognised_means_nothing_placed() {
        assert!(align(&[], "что-то").is_empty());
    }

    #[test]
    fn the_geometry_travels_with_the_segment() {
        let mut b = block(12.0, "Centered title");
        b.align = Align::Center;
        b.lines = 2;
        b.glyph_height = 31.0;
        b.pitch = 44.0;
        let s = &align(&[b], "Заголовок по центру")[0];
        assert_eq!(s.align, Align::Center);
        assert_eq!(s.lines, 2);
        assert_eq!(s.glyph_height, 31.0);
        assert_eq!(s.pitch, 44.0);
        assert_eq!(s.sample.text, "Centered title");
    }
}
