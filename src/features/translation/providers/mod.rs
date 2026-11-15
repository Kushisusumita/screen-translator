//! Translation back ends and the chain that walks them.
//!
//! Every engine exposes the same shape: text in, text out, or an error that
//! explains itself. `translate` tries the enabled engines in the user's order
//! and returns the first success, so a missing DeepL quota quietly falls through
//! to Google instead of failing the capture.

pub mod ai;
pub mod deepl;
pub mod google;
pub mod yandex;

use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::entities::language::Language;
use crate::entities::settings::{EngineKind, EngineSettings};
use crate::shared::error::AppError;
use crate::shared::i18n::t;
use crate::shared::logging::redact;

#[derive(Debug, Clone)]
pub struct TranslateRequest {
    /// One paragraph per line. Every engine is expected to answer with the
    /// same number of lines in the same order: that is what lets each
    /// translated paragraph be put back where its original was.
    pub text: String,
    /// May be `Auto`; each provider maps that to its own convention.
    pub source: Language,
    pub target: Language,
}

#[derive(Debug, Clone)]
pub struct Translated {
    pub text: String,
    pub engine: EngineKind,
    pub elapsed: Duration,
}

/// Longest single request any of the text APIs will accept comfortably.
/// Longer input is split on sentence boundaries and reassembled.
pub const MAX_CHUNK_CHARS: usize = 3500;

/// Runs the configured engines in order and returns the first translation.
///
/// Errors are accumulated rather than discarded: if everything fails the user
/// sees why each engine failed, which is the difference between "перевод не
/// удался" and "ключ DeepL исчерпан, Google вернул 429".
pub async fn translate(
    req: &TranslateRequest,
    engines: &EngineSettings,
) -> Result<Translated, AppError> {
    let active = engines.active();

    if active.is_empty() {
        return Err(AppError::Other(no_engine_message(engines)));
    }

    let mut failures: Vec<String> = Vec::new();
    let mut user_reasons: Vec<String> = Vec::new();

    let last = active.len() - 1;
    for (position, kind) in active.into_iter().enumerate() {
        let started = Instant::now();
        // With another engine to turn to, a stalled one is given up on early;
        // the last in line gets all the time the connection allows.
        let outcome = match patience(kind).filter(|_| position < last) {
            Some(limit) => tokio::time::timeout(limit, attempt(kind, req, engines))
                .await
                .unwrap_or_else(|_| {
                    Err(AppError::Other(
                        t("The service did not answer in time").to_string(),
                    ))
                }),
            None => attempt(kind, req, engines).await,
        };
        let elapsed = started.elapsed();

        match outcome {
            Ok(text) if !text.trim().is_empty() => {
                info!(
                    engine = kind.label(),
                    ms = elapsed.as_millis() as u64,
                    chars_in = req.text.chars().count(),
                    chars_out = text.chars().count(),
                    "Translated"
                );
                debug!(engine = kind.label(), result = %redact(&text));
                return Ok(Translated {
                    text,
                    engine: kind,
                    elapsed,
                });
            }
            Ok(_) => {
                warn!(
                    engine = kind.label(),
                    "Empty translation, trying next engine"
                );
                failures.push(t("{engine}: empty response").replace("{engine}", kind.label()));
                user_reasons
                    .push(t("{engine}: empty response").replace("{engine}", kind.label()));
            }
            Err(e) => {
                warn!(engine = kind.label(), error = %e, "Engine failed");
                failures.push(format!("{}: {}", kind.label(), short_reason(&e)));
                user_reasons.push(format!("{}: {}", kind.label(), e.user_message()));
            }
        }
    }

    // Two audiences, two texts. The log gets what each engine actually said,
    // URLs and status lines included; the user gets which engines were tried
    // and why, in words that mean something without a network trace.
    warn!(reasons = %failures.join(" | "), "Every engine failed");
    Err(AppError::Other(
        t("Translation failed.\n\n{reasons}").replace("{reasons}", &user_reasons.join("\n")),
    ))
}

/// How long an engine is waited for while there is another to fall back on.
///
/// The free endpoints answer in a fraction of a second or, when something is
/// wrong, not at all — and "not at all" used to cost the full twenty seconds
/// of the connection's own timeout before the next engine was even asked.
/// Someone standing in front of a dialogue box has moved on by then. A model
/// may honestly take its time, and has a timeout of its own in the settings.
fn patience(kind: EngineKind) -> Option<Duration> {
    match kind {
        EngineKind::Yandex | EngineKind::Google => Some(Duration::from_secs(7)),
        EngineKind::DeepL => Some(Duration::from_secs(10)),
        EngineKind::Ai => None,
    }
}

async fn attempt(
    kind: EngineKind,
    req: &TranslateRequest,
    engines: &EngineSettings,
) -> Result<String, AppError> {
    match kind {
        EngineKind::Yandex => yandex::translate(req, engines.yandex_headless_fallback).await,
        EngineKind::Google => google::translate(req).await,
        EngineKind::DeepL => deepl::translate(req, &engines.deepl_key).await,
        EngineKind::Ai => ai::translate(req, &engines.ai_config).await,
    }
}

fn no_engine_message(engines: &EngineSettings) -> String {
    let enabled_but_unconfigured: Vec<&str> = EngineKind::all()
        .into_iter()
        .filter(|k| engines.is_enabled(*k) && !engines.is_configured(*k))
        .map(|k| k.label())
        .collect();

    if enabled_but_unconfigured.is_empty() {
        t("Every translation engine is turned off.\nTurn on at least one in Settings → Engine.")
            .to_string()
    } else {
        t("No translation engine is ready.\n{engines} is turned on but not configured — the API key is missing.\nSettings → Engine.")
            .replace("{engines}", &enabled_but_unconfigured.join(", "))
    }
}

/// Trims an error down to something that fits in a popup.
fn short_reason(e: &AppError) -> String {
    let s = e.to_string();
    let first_line = s.lines().next().unwrap_or(&s);
    crate::shared::logging::clip(first_line, 160).to_string()
}

// ── Shared text handling ─────────────────────────────────────────────────────

/// Repairs the damage a screenshot does to text before any engine sees it.
///
/// OCR of a rendered paragraph returns one string per visual line, so a sentence
/// arrives pre-broken and often hyphenated. Feeding that to a translator gives
/// you a translation of fragments. Rejoining first is the single biggest quality
/// win available here, and it helps every engine equally.
pub fn normalize_ocr_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut lines = raw.lines().map(str::trim).peekable();

    while let Some(line) = lines.next() {
        if line.is_empty() {
            // Blank line = paragraph break; keep exactly one.
            if !out.ends_with("\n\n") && !out.is_empty() {
                out.push('\n');
                out.push('\n');
            }
            continue;
        }

        // A trailing hyphen means the word continues on the next line.
        if let Some(stem) = line.strip_suffix('-') {
            if lines.peek().is_some_and(|n| starts_lowercase(n)) {
                out.push_str(stem);
                continue;
            }
        }

        out.push_str(line);

        match lines.peek() {
            None => {}
            Some(&"") => {}
            Some(next) => {
                // CJK has no inter-word space, so joining with one inserts a
                // gap that was never in the original.
                if ends_cjk(line) && starts_cjk(next) {
                    // nothing
                } else {
                    out.push(' ');
                }
            }
        }
    }

    collapse_spaces(out.trim())
}

fn starts_lowercase(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_lowercase())
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // kana
        | 0x3400..=0x4DBF // CJK ext A
        | 0x4E00..=0x9FFF // CJK unified
        | 0xF900..=0xFAFF // compatibility
        | 0xAC00..=0xD7AF // hangul
    )
}

fn ends_cjk(s: &str) -> bool {
    s.chars().next_back().is_some_and(is_cjk)
}

fn starts_cjk(s: &str) -> bool {
    s.chars().next().is_some_and(is_cjk)
}

fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        let is_space = c == ' ' || c == '\t';
        if is_space {
            if !prev_space {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
        prev_space = is_space;
    }
    out
}

/// A text broken into pieces that each fit one request, with enough kept to put
/// the translated pieces back together line for line.
///
/// Engines that take a list of texts get one entry per paragraph, which is the
/// only reliable way to have exactly as many paragraphs come back as went in.
/// A paragraph too long for one request is cut on sentence boundaries, and its
/// pieces are joined again afterwards — so the caller sees one line in, one
/// line out, however the text had to travel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinePlan {
    /// Every piece, in order.
    pub pieces: Vec<String>,
    /// For each piece, the index of the line it belongs to.
    owner: Vec<usize>,
    lines: usize,
}

impl LinePlan {
    pub fn new(text: &str, max_chars: usize) -> Self {
        let mut pieces = Vec::new();
        let mut owner = Vec::new();
        let mut lines = 0;

        for (index, line) in text.split('\n').enumerate() {
            lines = index + 1;
            if line.trim().is_empty() {
                // Kept as a line so the count survives, but not sent: an
                // empty string is an error to some engines.
                continue;
            }
            for piece in chunk_text(line, max_chars) {
                pieces.push(piece);
                owner.push(index);
            }
        }

        Self {
            pieces,
            owner,
            lines,
        }
    }

    /// Ranges of `pieces` that may share a request: at most `max_chars` of
    /// text and `max_items` entries each.
    pub fn batches(&self, max_chars: usize, max_items: usize) -> Vec<std::ops::Range<usize>> {
        let mut out = Vec::new();
        let mut start = 0;
        let mut size = 0;

        for (i, piece) in self.pieces.iter().enumerate() {
            let len = piece.chars().count();
            let full = i > start && (size + len > max_chars || i - start >= max_items.max(1));
            if full {
                out.push(start..i);
                start = i;
                size = 0;
            }
            size += len;
        }
        if start < self.pieces.len() {
            out.push(start..self.pieces.len());
        }
        out
    }

    /// Puts translated pieces back into lines. `translated` must line up with
    /// `pieces`; anything short of that is the caller's error to report.
    pub fn assemble(&self, translated: &[String]) -> Option<String> {
        if translated.len() != self.pieces.len() {
            return None;
        }
        let mut lines = vec![String::new(); self.lines];
        for (piece, &line) in translated.iter().zip(&self.owner) {
            let piece = piece.trim();
            let slot = &mut lines[line];
            if !slot.is_empty() && !piece.is_empty() {
                slot.push(' ');
            }
            slot.push_str(piece);
        }
        Some(lines.join("\n"))
    }
}

/// Splits long text on sentence boundaries so each piece fits one API call.
/// Never splits mid-character, and never returns an empty chunk.
pub fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    if text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;

    for sentence in split_sentences(text) {
        let len = sentence.chars().count();

        // A single sentence longer than the limit has to be cut somewhere;
        // fall back to a hard character split for that one piece.
        if len > max_chars {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
                current_len = 0;
            }
            let mut buf = String::new();
            for (i, c) in sentence.chars().enumerate() {
                buf.push(c);
                if (i + 1) % max_chars == 0 {
                    chunks.push(std::mem::take(&mut buf));
                }
            }
            if !buf.is_empty() {
                chunks.push(buf);
            }
            continue;
        }

        if current_len + len > max_chars && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_len = 0;
        }
        current.push_str(sentence);
        current_len += len;
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    chunks.retain(|c| !c.is_empty());
    if chunks.is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}

/// Yields slices that together reconstruct the input exactly, cut after
/// sentence-final punctuation or a newline.
fn split_sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes_len = text.len();

    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let boundary = matches!(c, '.' | '!' | '?' | '\n' | '。' | '！' | '？' | '؟');
        if !boundary {
            continue;
        }
        // Consume any run of trailing quotes/spaces so they stay with the
        // sentence they belong to.
        let mut end = i + c.len_utf8();
        while let Some(&(j, n)) = it.peek() {
            if matches!(n, ' ' | '"' | '»' | '\'' | ')' | '\n') {
                end = j + n.len_utf8();
                it.next();
            } else {
                break;
            }
        }
        out.push(&text[start..end]);
        start = end;
    }
    if start < bytes_len {
        out.push(&text[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyphenated_line_breaks_are_rejoined() {
        let raw = "Machine trans-\nlation used to require\na round trip.";
        assert_eq!(
            normalize_ocr_text(raw),
            "Machine translation used to require a round trip."
        );
    }

    #[test]
    fn a_trailing_hyphen_before_a_capital_is_left_alone() {
        // "Anglo-" then "Saxon" on the next line is a real hyphen, not a wrap.
        let raw = "some Anglo-\nSaxon thing";
        assert_eq!(normalize_ocr_text(raw), "some Anglo- Saxon thing");
    }

    #[test]
    fn paragraph_breaks_survive_but_do_not_multiply() {
        let raw = "First para.\n\n\n\nSecond para.";
        assert_eq!(normalize_ocr_text(raw), "First para.\n\nSecond para.");
    }

    #[test]
    fn cjk_lines_join_without_inserting_a_space() {
        let raw = "機械翻訳は\nクラウドを必要とした";
        assert_eq!(normalize_ocr_text(raw), "機械翻訳はクラウドを必要とした");
    }

    #[test]
    fn latin_lines_still_get_their_space() {
        assert_eq!(normalize_ocr_text("hello\nworld"), "hello world");
    }

    #[test]
    fn chunking_reconstructs_the_original() {
        let text = "One. Two! Three? ".repeat(400);
        let chunks = chunk_text(&text, 100);
        assert!(chunks.len() > 1);
        assert_eq!(chunks.concat(), text);
        for c in &chunks {
            assert!(
                c.chars().count() <= 100,
                "chunk over the limit: {}",
                c.len()
            );
        }
    }

    #[test]
    fn a_single_oversized_sentence_is_split_rather_than_dropped() {
        let text = "б".repeat(250); // no sentence punctuation at all
        let chunks = chunk_text(&text, 100);
        assert_eq!(chunks.concat(), text);
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn a_plan_sends_one_piece_per_line_and_gets_one_line_back() {
        let plan = LinePlan::new("Title\nA sentence. Another one.\nButton", 100);
        assert_eq!(plan.pieces, vec!["Title", "A sentence. Another one.", "Button"]);

        let back = plan
            .assemble(&["Заголовок".into(), "Фраза. Ещё одна.".into(), "Кнопка".into()])
            .expect("same number of pieces");
        assert_eq!(back, "Заголовок\nФраза. Ещё одна.\nКнопка");
    }

    #[test]
    fn a_line_too_long_for_one_request_is_split_and_then_rejoined() {
        let long = "One sentence here. ".repeat(10);
        let text = format!("Short\n{}", long.trim());
        let plan = LinePlan::new(&text, 60);
        assert!(plan.pieces.len() > 2, "the long line was not split");

        let echoed: Vec<String> = plan.pieces.iter().map(|p| p.trim().to_uppercase()).collect();
        let back = plan.assemble(&echoed).expect("same number of pieces");
        let lines: Vec<&str> = back.split('\n').collect();
        assert_eq!(lines.len(), 2, "two lines went in, two must come out");
        assert_eq!(lines[0], "SHORT");
        assert_eq!(lines[1], long.trim().to_uppercase());
    }

    #[test]
    fn an_empty_line_keeps_its_place_without_being_sent() {
        let plan = LinePlan::new("first\n\nthird", 100);
        assert_eq!(plan.pieces, vec!["first", "third"]);
        let back = plan.assemble(&["1".into(), "3".into()]).unwrap();
        assert_eq!(back, "1\n\n3");
    }

    #[test]
    fn a_reply_with_the_wrong_number_of_pieces_is_refused() {
        let plan = LinePlan::new("a\nb", 100);
        assert!(plan.assemble(&["only one".into()]).is_none());
    }

    #[test]
    fn batches_respect_both_the_size_and_the_item_limits() {
        let text = (0..10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let plan = LinePlan::new(&text, 100);

        // Six characters a line: three lines fit in twenty.
        let by_size = plan.batches(20, 100);
        assert!(by_size.iter().all(|r| r.len() <= 3), "{by_size:?}");
        assert_eq!(by_size.iter().map(|r| r.len()).sum::<usize>(), 10);

        let by_count = plan.batches(10_000, 4);
        assert_eq!(by_count.len(), 3);
        assert_eq!(by_count[0], 0..4);
        assert_eq!(by_count[2], 8..10);
    }

    #[test]
    fn a_single_oversized_piece_still_gets_a_batch_of_its_own() {
        let plan = LinePlan::new("tiny\nthis one is much longer than the limit\ntiny", 100);
        let batches = plan.batches(10, 100);
        assert_eq!(batches.len(), 3);
    }

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(chunk_text("hi", 100), vec!["hi".to_string()]);
    }

    #[test]
    fn chunking_multibyte_text_never_panics_and_never_loses_a_char() {
        let text = "Привет мир! ".repeat(300);
        let chunks = chunk_text(&text, 64);
        assert_eq!(chunks.concat(), text);
    }
}
