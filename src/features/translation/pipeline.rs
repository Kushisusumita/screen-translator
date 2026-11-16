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
//! The reading itself has two homes as well. Where the machine has a
//! recogniser of its own for the language, the capture is read there: in
//! hundredths of a second, and without the picture going anywhere. The online
//! recogniser is what is turned to when there is none, or when the one here
//! found nothing.
//!
//! Everything slow here — encoding included — happens off the UI thread.

use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::entities::language::Language;
use crate::entities::settings::{EngineKind, EngineSettings, Recogniser, Settings};
use crate::features::capture::screenshot::{encode_frame, encode_frame_for_vision};
use crate::features::capture::Frame;
use crate::features::translation::cache;
use crate::features::translation::ocr::{self, Align, Line, PxRect, Recognised, TextBlock};
use crate::features::translation::providers::{self, ai, TranslateRequest};
use crate::shared::error::AppError;
use crate::shared::i18n::t;

/// Everything the pipeline needs, copied out of `Settings` so nothing is
/// shared with the UI thread while a request is in flight.
#[derive(Debug, Clone)]
pub struct PipelineParams {
    pub source: Language,
    pub target: Language,
    pub engines: EngineSettings,
    pub recogniser: Recogniser,
}

impl From<&Settings> for PipelineParams {
    fn from(s: &Settings) -> Self {
        Self {
            source: s.source_lang,
            target: s.target_lang,
            engines: s.engines.clone(),
            recogniser: s.recogniser,
        }
    }
}

impl PipelineParams {
    /// The language a capture would be read as on this machine, if it would
    /// be read here at all.
    pub fn read_here_as(&self) -> Option<Language> {
        match self.recogniser {
            Recogniser::Online => None,
            Recogniser::Auto | Recogniser::Local => {
                ocr::local::language_for(self.source, self.target)
            }
        }
    }

    /// Whether a capture of this kind is handed to a model as a picture
    /// instead of being read first.
    pub fn shows_the_model(&self, kind: CaptureKind) -> bool {
        // Not when the picture was to stay on this machine.
        kind == CaptureKind::Region
            && self.recogniser != Recogniser::Local
            && self.engines.reads_screenshots()
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

/// How the text of a capture came to be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// By the recogniser on this machine, as opposed to the online one.
    pub here: bool,
    /// There was text.
    pub found: bool,
    pub elapsed: Duration,
}

/// Milestones worth showing before the result is in.
#[derive(Debug, Clone)]
pub enum Progress {
    /// The capture has been read, whatever was or was not in it.
    Read(Reading),
    /// The text has been read; translation is under way.
    Recognised(String),
}

/// The terms on which a watched area is read once more.
///
/// Reading on this machine costs so little that an area can be read again
/// every fraction of a second, a scene in motion included. Translating
/// cannot be: every new text is a request to a service that belongs to
/// someone else. So a reading made on these terms goes no further than the
/// text unless the text is worth a request — new, and done changing.
#[derive(Debug, Clone, Default)]
pub struct Again {
    /// What is translated and on screen already, where that still stands.
    pub shown: Option<String>,
    /// What the area said the last time it was read.
    pub heard: Option<String>,
    /// The picture has come to rest, so what it says now is what it says.
    /// Without that, a text has to be read twice running before it counts:
    /// a line being typed out letter by letter is a new text at every look.
    pub settled: bool,
    /// A translation may be asked for. Not when too many have been lately.
    pub may_translate: bool,
    /// The online recogniser may be turned to if the one here finds nothing.
    pub online: bool,
}

impl Again {
    /// Whether what the area says now is worth translating: not what is
    /// translated already, and not something still on its way to being
    /// something else.
    fn is_news(&self, text: &str) -> bool {
        let same = |known: &Option<String>| known.as_deref() == Some(text);
        !same(&self.shown) && (self.settled || same(&self.heard))
    }
}

/// What came of a capture.
#[derive(Debug, Clone)]
pub enum Outcome {
    Translated(Box<PipelineResult>),
    /// It was read and left at that, as [`Again`] asked: there was nothing
    /// in it to translate yet. The text is what it said.
    Held(String),
}

pub async fn run(
    frame: Frame,
    kind: CaptureKind,
    params: &PipelineParams,
    again: Option<&Again>,
    progress: impl Fn(Progress),
) -> Result<Outcome, AppError> {
    translate(frame, kind, params, again, progress)
        .await
        .map(|outcome| match outcome {
            Ok(result) => Outcome::Translated(Box::new(result)),
            Err(text) => Outcome::Held(text),
        })
}

/// The pipeline itself. Inside the outer result, `Err` is not a failure but
/// a text that was read and held back.
async fn translate(
    frame: Frame,
    kind: CaptureKind,
    params: &PipelineParams,
    again: Option<&Again>,
    progress: impl Fn(Progress),
) -> Result<Result<PipelineResult, String>, AppError> {
    let started = Instant::now();
    let may_go_online = again.map_or(true, |again| again.online);

    if params.shows_the_model(kind) && may_go_online {
        match read_with_ai(&frame, params, started).await {
            Ok(result) => return Ok(Ok(result)),
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

    let (recognised, here) = read(frame, params, may_go_online).await?;
    let original = recognised.text();
    let found = !recognised.is_empty() && !original.trim().is_empty();
    progress(Progress::Read(Reading {
        here,
        found,
        elapsed: started.elapsed(),
    }));
    if !found {
        return Err(AppError::NoText);
    }

    // A reading made cheaply, to see whether anything has changed: it stops
    // here unless something has, and has finished changing.
    let held = again.filter(|again| !again.online);
    if held.is_some_and(|again| !again.is_news(&original)) {
        return Ok(Err(original));
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
        return Ok(Ok(PipelineResult {
            segments: align(&recognised.blocks, &original),
            translated: original.clone(),
            original,
            engine: EngineKind::Yandex,
            source,
            target: params.target,
            elapsed: started.elapsed(),
            engine_elapsed: Duration::ZERO,
            from_cache: false,
        }));
    }

    if let Some(hit) = cache::get(&original, source, params.target) {
        info!(engine = hit.engine.label(), "Cache hit");
        let segments = align(&recognised.blocks, &hit.text);
        return Ok(Ok(PipelineResult {
            translated: whole(&segments, &original, hit.text),
            segments,
            original,
            engine: hit.engine,
            source,
            target: params.target,
            elapsed: started.elapsed(),
            engine_elapsed: Duration::ZERO,
            from_cache: true,
        }));
    }

    // From here on it costs a request.
    if held.is_some_and(|again| !again.may_translate) {
        return Ok(Err(original));
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

    Ok(Ok(PipelineResult {
        translated: whole(&segments, &original, translated.text),
        original,
        segments,
        engine: translated.engine,
        source,
        target: params.target,
        elapsed: started.elapsed(),
        engine_elapsed: translated.elapsed,
        from_cache: false,
    }))
}

/// How long the recogniser on this machine is waited for.
const HERE_PATIENCE: Duration = Duration::from_secs(8);

/// Reads the text of a capture, and says whether it was read on this
/// machine.
///
/// Here first, where there is a recogniser for the language. What it finds
/// is taken as it is. When it finds nothing the online recogniser is asked
/// as well, if it may be: nothing found here can mean there is nothing, and
/// can mean lettering this recogniser was not made for. Whoever watches an
/// area decides which — it knows whether this recogniser has ever read that
/// area — and says so through `may_go_online`.
async fn read(
    frame: Frame,
    params: &PipelineParams,
    may_go_online: bool,
) -> Result<(Recognised, bool), AppError> {
    let size = (frame.width() as u32, frame.height() as u32);
    let here_only = params.recogniser == Recogniser::Local;

    match params.read_here_as() {
        Some(language) => {
            let shot = frame.clone();
            let reading = blocking(move || ocr::local::read(&shot, language));
            // It answers in a fraction of a second or something is wrong
            // with it, and a capture must not wait on that for ever.
            let answered = tokio::time::timeout(HERE_PATIENCE, reading)
                .await
                .unwrap_or_else(|_| {
                    Err(AppError::Other(
                        t("Text recognition is unavailable right now").to_string(),
                    ))
                });
            match answered {
                Ok(mut found) => {
                    if !found.is_empty() || here_only || !may_go_online {
                        // A language the user named is the language it was
                        // read in. One that was only assumed is left for
                        // the translator to make out: German read by the
                        // English recogniser is still German.
                        if params.source != Language::Auto && !found.is_empty() {
                            found.detected = Some(language);
                        }
                        return Ok((found, true));
                    }
                    debug!("Nothing was read on this computer; asking the online recogniser");
                }
                Err(e) if here_only => return Err(e),
                Err(e) => {
                    warn!(error = %e, "Could not read on this computer; asking the online recogniser");
                }
            }
        }
        None if here_only => return Err(ocr::local::missing(params.source)),
        None => {}
    }
    if !may_go_online {
        return Ok((Recognised::default(), true));
    }

    let encoding = Instant::now();
    let encoded = blocking(move || encode_frame(&frame)).await?;
    debug!(
        ms = encoding.elapsed().as_millis() as u64,
        bytes = encoded.jpeg.len(),
        scale = encoded.scale,
        "Picture prepared for recognition"
    );
    let found = ocr::recognize(&encoded, size).await?;
    Ok((found, false))
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

/// Gives a translation the capitals of what it translates.
///
/// A menu set in capitals — `CONTINUE`, `SETTINGS` — comes back from an engine
/// in lower case as often as not, and a button that used to shout now mumbles
/// next to one that still shouts. The engines are not wrong, exactly: they
/// translate words, and capitals on a button are typography. So that part is
/// put back here: text that was all capitals stays all capitals, and text
/// that began with one begins with one.
fn match_case(original: &str, translated: &str) -> String {
    let cased = || original.chars().filter(|c| c.is_uppercase() || c.is_lowercase());
    // Two letters in capitals are as likely an abbreviation as a style.
    if cased().count() >= 3 && cased().all(char::is_uppercase) {
        return translated.to_uppercase();
    }

    let begins_with_capital = original
        .chars()
        .find(|c| c.is_alphabetic())
        .is_some_and(char::is_uppercase);
    if !begins_with_capital {
        return translated.to_string();
    }
    let mut out = String::with_capacity(translated.len());
    let mut seen_letter = false;
    for c in translated.chars() {
        if !seen_letter && c.is_alphabetic() {
            seen_letter = true;
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The translation as one text, for the views that show it whole.
///
/// When every paragraph has its own line, those lines are the text: they have
/// been through [`match_case`] and the engine's answer has not. Otherwise the
/// answer is all there is, and it is given the same treatment in one piece.
fn whole(segments: &[Segment], original: &str, answer: String) -> String {
    if segments.is_empty() {
        return match_case(original, &answer);
    }
    segments
        .iter()
        .map(|segment| segment.translated.as_str())
        .collect::<Vec<_>>()
        .join("\n")
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
                match_case(&block.text, line)
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
    fn a_menu_in_capitals_stays_in_capitals() {
        assert_eq!(match_case("CONTINUE", "продолжить"), "ПРОДОЛЖИТЬ");
        assert_eq!(match_case("NEW GAME", "Новая игра"), "НОВАЯ ИГРА");
        assert_eq!(
            match_case("PRESS E TO INTERACT", "нажмите E для взаимодействия"),
            "НАЖМИТЕ E ДЛЯ ВЗАИМОДЕЙСТВИЯ"
        );
    }

    #[test]
    fn an_abbreviation_does_not_make_the_translation_shout() {
        assert_eq!(match_case("OK", "хорошо"), "Хорошо");
        assert_eq!(match_case("HP: 100", "ОЗ: 100"), "ОЗ: 100");
    }

    #[test]
    fn text_that_began_with_a_capital_begins_with_one() {
        assert_eq!(match_case("Settings", "настройки"), "Настройки");
        assert_eq!(match_case("\"Stay here,\" she said.", "«оставайся здесь», — сказала она."),
            "«Оставайся здесь», — сказала она.");
        // And only the first letter is touched.
        assert_eq!(match_case("Open the Gate", "открой ворота"), "Открой ворота");
    }

    #[test]
    fn lower_case_and_caseless_text_is_left_as_the_engine_gave_it() {
        assert_eq!(match_case("continue", "продолжить"), "продолжить");
        assert_eq!(match_case("設定", "настройки"), "настройки");
        assert_eq!(match_case("3 items", "3 предмета"), "3 предмета");
    }

    #[test]
    fn each_paragraph_takes_its_own_capitals_and_the_whole_text_follows() {
        let blocks = [block(0.0, "CONTINUE"), block(40.0, "Settings"), block(80.0, "exit")];
        let segments = align(&blocks, "продолжить\nнастройки\nвыход");
        let lines: Vec<&str> = segments.iter().map(|s| s.translated.as_str()).collect();
        assert_eq!(lines, ["ПРОДОЛЖИТЬ", "Настройки", "выход"]);
        assert_eq!(
            whole(&segments, "CONTINUE\nSettings\nexit", "продолжить\nнастройки\nвыход".into()),
            "ПРОДОЛЖИТЬ\nНастройки\nвыход"
        );
    }

    #[test]
    fn a_translation_with_no_places_is_still_given_its_capitals() {
        assert_eq!(whole(&[], "GAME OVER", "игра окончена".into()), "ИГРА ОКОНЧЕНА");
    }

    #[test]
    fn a_picture_that_was_to_stay_on_this_machine_is_not_shown_to_a_model() {
        let mut engines = EngineSettings {
            ai: true,
            ..Default::default()
        };
        engines.ai_config.api_key = crate::shared::secret::Secret::new("k");
        engines.ai_config.vision = true;
        let mut params = PipelineParams {
            source: Language::Auto,
            target: Language::Ru,
            engines,
            recogniser: Recogniser::Auto,
        };
        // A region the user drew, and a model asked to read screenshots.
        assert!(params.shows_the_model(CaptureKind::Region));
        // A whole window is read first whatever happens, for the places.
        assert!(!params.shows_the_model(CaptureKind::Whole));

        params.recogniser = Recogniser::Local;
        assert!(!params.shows_the_model(CaptureKind::Region));

        params.recogniser = Recogniser::Online;
        assert!(params.shows_the_model(CaptureKind::Region));
        // And told to read online, nothing is read here.
        assert_eq!(params.read_here_as(), None);
    }

    #[test]
    fn what_is_on_screen_already_is_not_news() {
        let again = Again {
            shown: Some("The pass is closed.".into()),
            settled: true,
            ..Default::default()
        };
        assert!(!again.is_news("The pass is closed."));
        assert!(again.is_news("You will have to wait."));
    }

    #[test]
    fn over_a_moving_picture_a_text_is_news_once_it_has_been_read_twice() {
        let mut again = Again {
            shown: Some("The pass is closed.".into()),
            ..Default::default()
        };
        // A line being typed out: never the same twice.
        assert!(!again.is_news("You w"));
        again.heard = Some("You w".into());
        assert!(!again.is_news("You will have"));
        // And then it is.
        again.heard = Some("You will have to wait.".into());
        assert!(again.is_news("You will have to wait."));
        // Read twice or not, what is already up stays what it is.
        again.heard = again.shown.clone();
        assert!(!again.is_news("The pass is closed."));
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
