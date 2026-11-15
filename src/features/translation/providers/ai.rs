//! Bring-your-own AI.
//!
//! "Any AI with a token" in practice means three wire formats. Rather than ship
//! a provider list that goes stale, the user supplies a base URL, a model name
//! and a key, and picks which protocol the endpoint speaks. Presets in the
//! settings fill those three fields in for the common services; anything
//! OpenAI-compatible — including a local Ollama or LM Studio — works with no
//! code change.
//!
//! An LLM is the only engine here that can be *told* what it is looking at, and
//! the only one that can look for itself. So it is used two ways:
//!
//! * **as a translator**, given the text an OCR service read. The prompt says
//!   the input is OCR of a screenshot, so the model repairs recognition damage
//!   instead of faithfully translating it;
//! * **as the reader too**, given the screenshot itself. One round trip instead
//!   of two, no dependence on the OCR service, and a model that can see a
//!   stylised game font reads it better than a recogniser trained on documents.
//!
//! One thing all three protocols share is that the set of accepted request
//! fields moves with every model generation: a sampling temperature is
//! mandatory folklore on one model and a hard `400` on the next. Nothing here
//! can know which model a user typed in, so each call is made in its full form
//! first and, if the endpoint rejects the request as malformed, once more with
//! only the fields every model accepts. What worked is remembered for the
//! session, so the detour is paid once.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine as _;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use super::TranslateRequest;
use crate::entities::language::Language;
use crate::entities::settings::{AiConfig, AiProtocol};
use crate::features::translation::client::HTTP;
use crate::shared::error::AppError;
use crate::shared::i18n::t;
use crate::shared::logging::clip;

/// What the model made of a screenshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// The text as the model read it, one paragraph per line.
    pub original: String,
    pub translated: String,
    pub detected: Option<Language>,
}

pub async fn translate(req: &TranslateRequest, cfg: &AiConfig) -> Result<String, AppError> {
    ensure_usable(cfg)?;

    let system = system_prompt(req.source, req.target, &cfg.extra_instructions);
    let raw = call(cfg, &system, &req.text, None, output_budget(&req.text)).await?;

    let cleaned = clean_output(&raw);
    if cleaned.trim().is_empty() {
        return Err(AppError::Other(
            t("The model returned an empty response").into(),
        ));
    }
    Ok(cleaned)
}

/// Reads and translates a screenshot in one request.
pub async fn translate_image(
    jpeg: &[u8],
    source: Language,
    target: Language,
    cfg: &AiConfig,
) -> Result<Reading, AppError> {
    ensure_usable(cfg)?;

    let system = vision_prompt(source, target, &cfg.extra_instructions);
    let image = base64::engine::general_purpose::STANDARD.encode(jpeg);
    // The reply carries the text twice — as read and as translated — and a
    // screenshot gives no character count to budget from.
    let raw = call(
        cfg,
        &system,
        "Read and translate the text in this screenshot.",
        Some(&image),
        8192,
    )
    .await?;

    let reading = parse_reading(&raw);
    if reading.translated.trim().is_empty() {
        return Err(AppError::NoText);
    }
    Ok(reading)
}

fn ensure_usable(cfg: &AiConfig) -> Result<(), AppError> {
    if cfg.is_usable() {
        Ok(())
    } else {
        Err(AppError::Other(
            t("Not configured: a URL, a model and a key are required").into(),
        ))
    }
}

// ── Prompts ──────────────────────────────────────────────────────────────────

fn language_clause(source: Language) -> String {
    match source {
        Language::Auto => "the language you detect".to_string(),
        s => s.english_name().to_string(),
    }
}

/// Rules that hold whichever way the text reached the model. Every clause
/// exists because the alternative showed up in output: models like to explain
/// themselves, wrap results in quotes, translate code identifiers, and
/// helpfully "fix" numbers.
///
/// Two of them are about fidelity, and say why. Left to itself a model
/// translates the way a cautious editor would: a curse becomes "darn", slang
/// becomes standard speech, a moan or a stutter is dropped as noise, a
/// stage direction in asterisks is tidied away. Each of those is a small lie
/// about the story — the reader comes away with a milder scene and a blander
/// character than the author wrote. A translator's job is the opposite, and
/// the prompt says so in as many words rather than hoping.
fn shared_rules(target: Language) -> String {
    format!(
        "- The text comes from a screen: a game, an application, a web page or a video. \
Translate it the way a professional localiser would — natural, idiomatic {to}, with the \
register the original uses. Dialogue stays dialogue; an interface label stays short.\n\
- Be faithful to what the author wrote, not to what would be polite. The reader chose this \
text and needs the story, the characters and the tone exactly as they were intended; a \
softened translation misleads them. So keep the full force of the original: profanity \
stays profanity of the same strength, an insult stays an insult, slang becomes the slang \
a native speaker of {to} would really use, and crude, sexual, violent or otherwise adult \
content is translated as it is written — not toned down, not euphemised, not summarised, \
not skipped and not remarked on.\n\
- Carry over everything that shows how a line is delivered: interjections, moans, gasps, \
cries, laughter and other vocal sounds (spelled the way {to} writes them), stutters, \
drawn-out letters, ellipses and emphasis, and any description of an emotion or an action \
given in asterisks or brackets.\n\
- Keep names of characters, places, items and brands as they are unless they have an \
established {to} form.\n\
- Leave code identifiers, file paths, URLs, email addresses, version numbers and key names \
(Esc, Enter, WASD) exactly as they are. Keep all numbers and units unchanged.\n\
- If a fragment is already in {to}, repeat it unchanged rather than paraphrasing it.\n\
- Do not ask questions and do not add remarks of your own. If part of the text is \
unintelligible, give your best reading of it.",
        to = target.english_name(),
    )
}

fn append_user_instructions(prompt: &mut String, extra: &str) {
    let extra = extra.trim();
    if !extra.is_empty() {
        prompt.push_str("\n\nAdditional instructions from the user:\n");
        prompt.push_str(extra);
    }
}

/// For text an OCR service has already read.
fn system_prompt(source: Language, target: Language, extra: &str) -> String {
    let mut p = format!(
        "You are a translation engine inside a screen-translation utility. \
Translate the user's text from {from} into {to}.\n\n\
The text was extracted by OCR from a screenshot, so expect recognition errors, \
confused look-alike characters, doubled or missing punctuation and stray symbols. \
Silently repair those artefacts, then translate the repaired text.\n\n\
The input holds one paragraph per line. Rules:\n\
- Reply with the translation and nothing else: no preamble, no notes, no explanation, \
no surrounding quotation marks, no markdown.\n\
- Keep the line structure exactly: the reply has the same number of lines as the input, \
in the same order, each line translating the line in the same position. Never merge two \
lines, never split one, never drop one.\n\
{shared}",
        from = language_clause(source),
        to = target.english_name(),
        shared = shared_rules(target),
    );
    append_user_instructions(&mut p, extra);
    p
}

/// For a screenshot the model reads itself.
fn vision_prompt(source: Language, target: Language, extra: &str) -> String {
    let mut p = format!(
        "You are the reading and translation engine inside a screen-translation utility. \
The user sends a screenshot. Read every piece of text in it that is in {from}, in natural \
reading order, and translate it into {to}.\n\n\
Reply in exactly this form and with nothing else:\n\
<language>ISO 639-1 code of the language the text is written in</language>\n\
<original>\n\
the text as written, one paragraph per line\n\
</original>\n\
<translation>\n\
the translation, one paragraph per line, in the same order\n\
</translation>\n\n\
Rules:\n\
- A paragraph is a block of text that belongs together: a line of dialogue, a caption, \
a button, a list entry. Join a paragraph that wraps over several lines into one line.\n\
- Ignore decoration that is not text. If the screenshot holds no readable text at all, \
leave both sections empty.\n\
{shared}",
        from = language_clause(source),
        to = target.english_name(),
        shared = shared_rules(target),
    );
    append_user_instructions(&mut p, extra);
    p
}

fn base(cfg: &AiConfig) -> &str {
    cfg.base_url.trim().trim_end_matches('/')
}

/// Room for the answer, in tokens.
///
/// Generous on purpose. Current models think before they answer and the
/// thinking is paid for out of this same allowance, so a cap sized to the
/// translation alone cuts the translation off half way. A translation is
/// roughly input-sized; several times that still stops a runaway generation.
fn output_budget(text: &str) -> u32 {
    (text.chars().count() as u32)
        .saturating_mul(3)
        .saturating_add(2048)
        .clamp(4096, 16_000)
}

// ── Request shapes ───────────────────────────────────────────────────────────

/// How much of the optional request surface to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Everything that helps: a sampling temperature where the protocol has
    /// one, a low reasoning effort where it has that.
    Full,
    /// Only the fields no model of the protocol rejects.
    Minimal,
}

/// Which shape an endpoint accepted, so the detour through a `400` is taken
/// once per session rather than once per translation.
static ACCEPTED: Lazy<Mutex<HashMap<String, Shape>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn shape_key(cfg: &AiConfig) -> String {
    format!("{:?}|{}|{}", cfg.protocol, base(cfg), cfg.model.trim())
}

fn remembered_shape(cfg: &AiConfig) -> Shape {
    ACCEPTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&shape_key(cfg))
        .copied()
        .unwrap_or(Shape::Full)
}

fn remember_shape(cfg: &AiConfig, shape: Shape) {
    ACCEPTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(shape_key(cfg), shape);
}

async fn call(
    cfg: &AiConfig,
    system: &str,
    text: &str,
    image: Option<&str>,
    max_tokens: u32,
) -> Result<String, AppError> {
    let timeout = Duration::from_secs(cfg.timeout_secs.clamp(5, 300));
    let first = remembered_shape(cfg);

    let (status, raw) = send(cfg, system, text, image, max_tokens, first, timeout).await?;
    let (status, raw, used) = if status == 400 && first == Shape::Full {
        // "Bad request" is how every provider says "this model does not take
        // that field". Worth exactly one more try without the optional ones.
        debug!(body = %clip(raw.trim(), 300), "Request rejected; retrying with the minimal shape");
        let (status, raw) = send(
            cfg,
            system,
            text,
            image,
            max_tokens,
            Shape::Minimal,
            timeout,
        )
        .await?;
        (status, raw, Shape::Minimal)
    } else {
        (status, raw, first)
    };

    if !(200..300).contains(&status) {
        return Err(AppError::Other(explain(status, &raw)));
    }
    if used != first {
        info!(model = %cfg.model, "This model only accepts the minimal request shape");
    }
    remember_shape(cfg, used);

    let v: Value = serde_json::from_str(&raw).map_err(|e| {
        AppError::Other(t("Unreadable response: {error}").replace("{error}", &e.to_string()))
    })?;
    match cfg.protocol {
        AiProtocol::OpenAi => parse_openai(&v),
        AiProtocol::Anthropic => parse_anthropic(&v),
        AiProtocol::Gemini => parse_gemini(&v),
    }
}

async fn send(
    cfg: &AiConfig,
    system: &str,
    text: &str,
    image: Option<&str>,
    max_tokens: u32,
    shape: Shape,
    timeout: Duration,
) -> Result<(u16, String), AppError> {
    let rq = match cfg.protocol {
        AiProtocol::OpenAi => {
            let url = format!("{}/chat/completions", base(cfg));
            let body = openai_body(cfg, system, text, image, shape);
            let mut rq = HTTP.post(&url).timeout(timeout).json(&body);
            if !cfg.api_key.is_empty() {
                rq = rq.bearer_auth(cfg.api_key.expose());
            }
            rq
        }
        AiProtocol::Anthropic => {
            let url = format!("{}/messages", base(cfg));
            let body = anthropic_body(cfg, system, text, image, max_tokens, shape);
            HTTP.post(&url)
                .timeout(timeout)
                .header("x-api-key", cfg.api_key.expose())
                .header("anthropic-version", "2023-06-01")
                .json(&body)
        }
        AiProtocol::Gemini => {
            let url = format!("{}/models/{}:generateContent", base(cfg), cfg.model.trim());
            let body = gemini_body(cfg, system, text, image, max_tokens, shape);
            // Gemini takes the key in a header; passing it as `?key=` would put
            // a credential in every URL that gets logged along the way.
            HTTP.post(&url)
                .timeout(timeout)
                .header("x-goog-api-key", cfg.api_key.expose())
                .json(&body)
        }
    };

    let resp = rq.send().await?;
    let status = resp.status().as_u16();
    let raw = resp.text().await?;
    debug!(status, len = raw.len(), ?shape, "AI response");
    Ok((status, raw))
}

// ── OpenAI-compatible ────────────────────────────────────────────────────────

fn openai_body(
    cfg: &AiConfig,
    system: &str,
    text: &str,
    image: Option<&str>,
    shape: Shape,
) -> Value {
    let user: Value = match image {
        Some(data) => json!([
            { "type": "text", "text": text },
            { "type": "image_url",
              "image_url": { "url": format!("data:image/jpeg;base64,{data}") } },
        ]),
        None => json!(text),
    };

    let mut body = json!({
        "model": cfg.model.trim(),
        "stream": false,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    // Reasoning models accept only their default temperature and say so with
    // a 400; everything else takes this one.
    if shape == Shape::Full {
        body["temperature"] = json!(cfg.temperature);
    }
    body
}

fn parse_openai(v: &Value) -> Result<String, AppError> {
    // `content` is normally a string, but some gateways return the newer
    // array-of-parts shape.
    let msg = v.pointer("/choices/0/message/content").ok_or_else(|| {
        AppError::Other(t("The response has no choices[0].message.content").into())
    })?;

    if let Some(s) = msg.as_str() {
        return Ok(s.to_string());
    }
    if let Some(parts) = msg.as_array() {
        let joined: String = parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect();
        if !joined.is_empty() {
            return Ok(joined);
        }
    }
    Err(AppError::Other(t("The content field is empty").into()))
}

// ── Anthropic Messages ───────────────────────────────────────────────────────

fn anthropic_body(
    cfg: &AiConfig,
    system: &str,
    text: &str,
    image: Option<&str>,
    max_tokens: u32,
    shape: Shape,
) -> Value {
    let content: Value = match image {
        Some(data) => json!([
            { "type": "image",
              "source": { "type": "base64", "media_type": "image/jpeg", "data": data } },
            { "type": "text", "text": text },
        ]),
        None => json!(text),
    };

    let mut body = json!({
        "model": cfg.model.trim(),
        "max_tokens": max_tokens,
        "system": system,
        "messages": [ { "role": "user", "content": content } ],
    });

    match shape {
        // No sampling temperature on purpose: current Claude models reject a
        // non-default one outright. What a translation wants from them is
        // speed, and that is what a low effort buys — they think before
        // answering by default, which a line of dialogue does not need.
        Shape::Full => body["output_config"] = json!({ "effort": "low" }),
        // Models from before `effort` existed. Their output ceiling is lower
        // too, and asking for more than a model can produce is also a 400.
        Shape::Minimal => body["max_tokens"] = json!(max_tokens.min(8192)),
    }
    body
}

fn parse_anthropic(v: &Value) -> Result<String, AppError> {
    // A safety classifier can decline a request with a perfectly ordinary 200.
    // Without this check that reads as "the model returned nothing".
    if v.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        return Err(AppError::Other(
            t("The model declined to translate this text").into(),
        ));
    }

    let blocks = v
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Other(t("The response has no content block").into()))?;

    // Skip thinking/tool blocks and take the text ones.
    let text: String = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect();

    if text.is_empty() {
        return Err(AppError::Other(
            t("The response has no text blocks").into(),
        ));
    }
    if v.get("stop_reason").and_then(Value::as_str) == Some("max_tokens") {
        warn!("The model ran out of output tokens; the translation may be cut short");
    }
    Ok(text)
}

// ── Google Gemini ────────────────────────────────────────────────────────────

fn gemini_body(
    cfg: &AiConfig,
    system: &str,
    text: &str,
    image: Option<&str>,
    max_tokens: u32,
    shape: Shape,
) -> Value {
    let mut parts = Vec::new();
    if let Some(data) = image {
        parts.push(json!({ "inlineData": { "mimeType": "image/jpeg", "data": data } }));
    }
    parts.push(json!({ "text": text }));

    let mut body = json!({
        "systemInstruction": { "parts": [ { "text": system } ] },
        "contents": [ { "role": "user", "parts": parts } ],
    });
    if shape == Shape::Full {
        body["generationConfig"] = json!({
            "temperature": cfg.temperature,
            "maxOutputTokens": max_tokens,
        });
    }
    body
}

fn parse_gemini(v: &Value) -> Result<String, AppError> {
    if let Some(reason) = v
        .pointer("/candidates/0/finishReason")
        .and_then(Value::as_str)
    {
        if reason == "SAFETY" || reason == "PROHIBITED_CONTENT" {
            return Err(AppError::Other(
                t("Gemini blocked the text with its safety filter").into(),
            ));
        }
    }

    let parts = v
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AppError::Other(t("The response has no candidates[0].content.parts").into())
        })?;

    // A thinking model returns its reasoning as parts flagged `thought`; they
    // are not part of the answer.
    let text: String = parts
        .iter()
        .filter(|p| p.get("thought").and_then(Value::as_bool) != Some(true))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();

    if text.is_empty() {
        return Err(AppError::Other(t("Empty response from the model").into()));
    }
    Ok(text)
}

// ── Shared ───────────────────────────────────────────────────────────────────

/// Turns a provider error body into one line a user can act on. All three
/// protocols bury the message in a different place.
fn explain(status: u16, body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.pointer("/error/0/message"))
                .or_else(|| v.pointer("/0/error/message"))
                .or_else(|| v.pointer("/message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| clip(body.trim(), 200).to_string());

    let status_text = status.to_string();
    match status {
        401 | 403 => t("Key rejected ({status}): {message}")
            .replace("{status}", &status_text)
            .replace("{message}", &msg),
        404 => t("Model or URL not found ({status}): {message}")
            .replace("{status}", &status_text)
            .replace("{message}", &msg),
        429 => t("Rate limit exceeded: {message}").replace("{message}", &msg),
        500..=599 => t("The provider failed ({status}): {message}")
            .replace("{status}", &status_text)
            .replace("{message}", &msg),
        _ => format!("HTTP {status}: {msg}"),
    }
}

/// Strips the wrappers models add despite being told not to.
fn clean_output(raw: &str) -> String {
    let mut s = raw.trim();

    // ```\n…\n``` or ```text\n…\n```
    if s.starts_with("```") {
        if let Some(rest) = s.split_once('\n').map(|(_, r)| r) {
            if let Some(end) = rest.rfind("```") {
                s = rest[..end].trim();
            }
        }
    }

    // A single pair of matching quotes wrapping the whole answer.
    for (open, close) in [('"', '"'), ('«', '»'), ('“', '”'), ('\'', '\'')] {
        if s.starts_with(open) && s.ends_with(close) && s.chars().count() > 1 {
            let inner = &s[open.len_utf8()..s.len() - close.len_utf8()];
            // Only unwrap when the quotes really are the outermost pair.
            if !inner.contains(close) {
                s = inner.trim();
            }
            break;
        }
    }

    // "Перевод:" / "Translation:" on its own opening line.
    for prefix in ["Перевод:", "Translation:", "перевод:", "translation:"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim_start();
            break;
        }
    }

    s.to_string()
}

/// The text between `<tag>` and `</tag>`, tolerant of a missing closing tag —
/// a reply cut short by the output limit still has a usable beginning.
fn section<'a>(raw: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = raw.find(&open)? + open.len();
    let rest = &raw[start..];
    let end = rest.find(&close).unwrap_or(rest.len());
    Some(rest[..end].trim())
}

/// Drops blank lines and stray whitespace, so "one paragraph per line" holds
/// even when the model spaced its paragraphs out.
fn tidy_lines(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_reading(raw: &str) -> Reading {
    let detected = section(raw, "language").and_then(|code| Language::from_code(code.trim()));

    match section(raw, "translation") {
        Some(translation) => Reading {
            original: tidy_lines(section(raw, "original").unwrap_or_default()),
            translated: tidy_lines(translation),
            detected,
        },
        // The model ignored the format and simply answered. Its answer is
        // still the translation; there is just no original to show beside it.
        None => Reading {
            original: String::new(),
            translated: tidy_lines(&clean_output(raw)),
            detected,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::secret::Secret;

    fn config(protocol: AiProtocol) -> AiConfig {
        AiConfig {
            protocol,
            base_url: "https://example.test/v1".into(),
            model: "some-model".into(),
            api_key: Secret::new("k"),
            temperature: 0.2,
            ..Default::default()
        }
    }

    #[test]
    fn openai_string_content_is_read() {
        let v: Value =
            serde_json::from_str(r#"{"choices":[{"message":{"content":"Привет"}}]}"#).unwrap();
        assert_eq!(parse_openai(&v).unwrap(), "Привет");
    }

    #[test]
    fn openai_array_content_is_also_read() {
        let v: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"content":[{"type":"text","text":"При"},{"type":"text","text":"вет"}]}}]}"#,
        )
        .unwrap();
        assert_eq!(parse_openai(&v).unwrap(), "Привет");
    }

    #[test]
    fn anthropic_skips_non_text_blocks() {
        let v: Value = serde_json::from_str(
            r#"{"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Привет"}]}"#,
        )
        .unwrap();
        assert_eq!(parse_anthropic(&v).unwrap(), "Привет");
    }

    #[test]
    fn an_anthropic_refusal_is_an_error_that_says_so() {
        // HTTP 200, no text, and a stop reason that explains why.
        let v: Value = serde_json::from_str(
            r#"{"content":[],"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"}}"#,
        )
        .unwrap();
        // Not "the response has no text blocks", which is what an empty
        // content array would otherwise be reported as.
        let err = parse_anthropic(&v).unwrap_err();
        assert!(matches!(err, AppError::Other(_)), "{err}");
        let empty: Value = serde_json::from_str(r#"{"content":[]}"#).unwrap();
        assert_ne!(
            err.to_string(),
            parse_anthropic(&empty).unwrap_err().to_string()
        );
    }

    #[test]
    fn gemini_safety_block_is_explained() {
        let v: Value =
            serde_json::from_str(r#"{"candidates":[{"finishReason":"SAFETY"}]}"#).unwrap();
        let err = parse_gemini(&v).unwrap_err().to_string();
        // The text is translated at runtime, so assert on what survives every
        // language: the provider name.
        assert!(err.contains("Gemini"), "{err}");
    }

    #[test]
    fn gemini_joins_parts() {
        let v: Value = serde_json::from_str(
            r#"{"candidates":[{"content":{"parts":[{"text":"При"},{"text":"вет"}]}}]}"#,
        )
        .unwrap();
        assert_eq!(parse_gemini(&v).unwrap(), "Привет");
    }

    #[test]
    fn gemini_leaves_its_own_reasoning_out_of_the_answer() {
        let v: Value = serde_json::from_str(
            r#"{"candidates":[{"content":{"parts":[{"text":"let me think","thought":true},{"text":"Привет"}]}}]}"#,
        )
        .unwrap();
        assert_eq!(parse_gemini(&v).unwrap(), "Привет");
    }

    #[test]
    fn the_full_anthropic_request_asks_for_low_effort_and_no_temperature() {
        let body = anthropic_body(
            &config(AiProtocol::Anthropic),
            "sys",
            "hello",
            None,
            4096,
            Shape::Full,
        );
        assert_eq!(body["output_config"]["effort"], "low");
        // Current Claude models answer a sampling temperature with a 400.
        assert!(body.get("temperature").is_none());
        assert_eq!(body["messages"][0]["content"], "hello");
    }

    #[test]
    fn the_minimal_anthropic_request_carries_only_what_every_model_takes() {
        let body = anthropic_body(
            &config(AiProtocol::Anthropic),
            "sys",
            "hello",
            None,
            16_000,
            Shape::Minimal,
        );
        let mut keys: Vec<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["max_tokens", "messages", "model", "system"]);
        assert_eq!(body["max_tokens"], 8192);
    }

    #[test]
    fn an_anthropic_screenshot_goes_as_an_image_block_before_the_text() {
        let body = anthropic_body(
            &config(AiProtocol::Anthropic),
            "sys",
            "read this",
            Some("QUJD"),
            4096,
            Shape::Full,
        );
        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["type"], "base64");
        assert_eq!(content[0]["source"]["media_type"], "image/jpeg");
        assert_eq!(content[0]["source"]["data"], "QUJD");
        assert_eq!(content[1]["text"], "read this");
    }

    #[test]
    fn an_openai_screenshot_goes_as_a_data_url() {
        let body = openai_body(
            &config(AiProtocol::OpenAi),
            "sys",
            "read this",
            Some("QUJD"),
            Shape::Full,
        );
        let content = &body["messages"][1]["content"];
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/jpeg;base64,QUJD"
        );
        assert_eq!(body["temperature"], 0.2_f32 as f64);
    }

    #[test]
    fn the_minimal_openai_request_drops_the_temperature() {
        let body = openai_body(
            &config(AiProtocol::OpenAi),
            "sys",
            "hello",
            None,
            Shape::Minimal,
        );
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn a_gemini_screenshot_goes_as_inline_data() {
        let body = gemini_body(
            &config(AiProtocol::Gemini),
            "sys",
            "read this",
            Some("QUJD"),
            4096,
            Shape::Full,
        );
        let parts = &body["contents"][0]["parts"];
        assert_eq!(parts[0]["inlineData"]["mimeType"], "image/jpeg");
        assert_eq!(parts[0]["inlineData"]["data"], "QUJD");
        assert_eq!(parts[1]["text"], "read this");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 4096);
    }

    #[test]
    fn code_fences_are_removed() {
        assert_eq!(clean_output("```\nПривет мир\n```"), "Привет мир");
        assert_eq!(clean_output("```text\nПривет\n```"), "Привет");
    }

    #[test]
    fn wrapping_quotes_are_removed_but_inner_ones_kept() {
        assert_eq!(clean_output("\"Привет\""), "Привет");
        assert_eq!(
            clean_output("Он сказал \"привет\" и ушёл"),
            "Он сказал \"привет\" и ушёл"
        );
    }

    #[test]
    fn a_quote_at_each_end_of_a_dialogue_is_not_unwrapped() {
        // Two separate quoted phrases — unwrapping would corrupt the text.
        let s = "\"да\" или \"нет\"";
        assert_eq!(clean_output(s), s);
    }

    #[test]
    fn a_label_prefix_is_dropped() {
        assert_eq!(clean_output("Перевод: Привет"), "Привет");
    }

    #[test]
    fn the_output_budget_leaves_room_to_think() {
        // A model that reasons first spends from the same allowance.
        assert_eq!(output_budget(""), 4096);
        assert_eq!(output_budget(&"a".repeat(100_000)), 16_000);
        assert!(output_budget(&"a".repeat(3000)) > 4096);
    }

    #[test]
    fn the_prompt_names_both_languages() {
        let p = system_prompt(Language::En, Language::Ru, "");
        assert!(p.contains("English"));
        assert!(p.contains("Russian"));
    }

    #[test]
    fn the_prompt_insists_on_the_line_structure() {
        // The in-place view puts each translated paragraph back where its
        // original was, which only works if the count survives the trip.
        let p = system_prompt(Language::En, Language::Ru, "");
        assert!(p.contains("same number of lines"));
    }

    #[test]
    fn auto_source_does_not_claim_a_language() {
        let p = system_prompt(Language::Auto, Language::Ru, "");
        assert!(p.contains("the language you detect"));
    }

    #[test]
    fn both_prompts_ask_for_a_faithful_translation_rather_than_a_polite_one() {
        for p in [
            system_prompt(Language::Ja, Language::Ru, ""),
            vision_prompt(Language::Ja, Language::Ru, ""),
        ] {
            // The strength of the language, and the sounds and stage directions
            // a cautious model drops as noise.
            assert!(p.contains("profanity stays profanity"), "{p}");
            assert!(p.contains("not toned down"), "{p}");
            assert!(p.contains("moans"), "{p}");
            assert!(p.contains("asterisks or brackets"), "{p}");
            // In the language the reader asked for, by name.
            assert!(p.contains("a native speaker of Russian"), "{p}");
        }
    }

    #[test]
    fn user_instructions_are_appended_to_both_prompts() {
        for p in [
            system_prompt(Language::En, Language::Ru, "Обращайся на «ты»"),
            vision_prompt(Language::En, Language::Ru, "Обращайся на «ты»"),
        ] {
            assert!(p.contains("Обращайся на «ты»"));
        }
    }

    #[test]
    fn error_bodies_are_unwrapped_from_all_three_shapes() {
        assert!(explain(401, r#"{"error":{"message":"bad key"}}"#).contains("bad key"));
        assert!(explain(429, r#"{"message":"slow down"}"#).contains("slow down"));
        assert!(explain(500, "gateway exploded").contains("gateway exploded"));
        // Gemini wraps its error in an array on some paths.
        assert!(explain(400, r#"[{"error":{"message":"bad model"}}]"#).contains("bad model"));
    }

    #[test]
    fn a_well_formed_reading_is_split_into_its_parts() {
        let raw = "<language>en</language>\n<original>\nThe Ancient Gate\nPress E\n</original>\n\
                   <translation>\nДревние врата\nНажмите E\n</translation>";
        let r = parse_reading(raw);
        assert_eq!(r.original, "The Ancient Gate\nPress E");
        assert_eq!(r.translated, "Древние врата\nНажмите E");
        assert_eq!(r.detected, Some(Language::En));
    }

    #[test]
    fn blank_lines_between_paragraphs_do_not_change_the_line_count() {
        let raw = "<original>\na\n\nb\n</original><translation>\n\nа\n\n\nб\n</translation>";
        let r = parse_reading(raw);
        assert_eq!(r.original, "a\nb");
        assert_eq!(r.translated, "а\nб");
    }

    #[test]
    fn a_reading_cut_off_before_the_closing_tag_is_still_used() {
        let raw = "<original>\nHello\n</original>\n<translation>\nПривет, как де";
        assert_eq!(parse_reading(raw).translated, "Привет, как де");
    }

    #[test]
    fn a_reply_that_ignored_the_format_is_taken_as_the_translation() {
        let r = parse_reading("Привет, мир");
        assert_eq!(r.translated, "Привет, мир");
        assert_eq!(r.original, "");
        assert_eq!(r.detected, None);
    }

    #[test]
    fn empty_sections_mean_there_was_nothing_to_read() {
        let r = parse_reading("<language></language><original></original><translation></translation>");
        assert!(r.translated.is_empty());
    }

    #[test]
    fn the_accepted_shape_is_remembered_per_endpoint_and_model() {
        let a = AiConfig {
            model: "shape-test-a".into(),
            ..config(AiProtocol::Anthropic)
        };
        let b = AiConfig {
            model: "shape-test-b".into(),
            ..config(AiProtocol::Anthropic)
        };
        assert_eq!(remembered_shape(&a), Shape::Full);
        remember_shape(&a, Shape::Minimal);
        assert_eq!(remembered_shape(&a), Shape::Minimal);
        assert_eq!(remembered_shape(&b), Shape::Full, "one model's answer leaked to another");
    }
}
