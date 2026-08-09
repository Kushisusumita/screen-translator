//! Bring-your-own AI.
//!
//! "Any AI with a token" in practice means three wire formats. Rather than ship
//! a provider list that goes stale, the user supplies a base URL, a model name
//! and a key, and picks which protocol the endpoint speaks. Presets in the
//! settings fill those three fields in for the common services; anything
//! OpenAI-compatible — including a local Ollama or LM Studio — works with no
//! code change.
//!
//! An LLM is also the only engine here that can be *told* what it is looking at,
//! which is where the quality comes from: the prompt says the input is OCR of a
//! screenshot, so the model repairs recognition damage instead of faithfully
//! translating it.

use std::time::Duration;

use serde_json::{json, Value};
use tracing::debug;

use super::TranslateRequest;
use crate::entities::language::Language;
use crate::entities::settings::{AiConfig, AiProtocol};
use crate::features::translation::client::HTTP;
use crate::shared::error::AppError;
use crate::shared::i18n::t;
use crate::shared::logging::clip;

pub async fn translate(req: &TranslateRequest, cfg: &AiConfig) -> Result<String, AppError> {
    if !cfg.is_usable() {
        return Err(AppError::Other(
            t("Not configured: a URL, a model and a key are required").into(),
        ));
    }

    let system = system_prompt(req.source, req.target, &cfg.extra_instructions);
    let timeout = Duration::from_secs(cfg.timeout_secs.clamp(5, 300));

    let raw = match cfg.protocol {
        AiProtocol::OpenAi => call_openai(cfg, &system, &req.text, timeout).await?,
        AiProtocol::Anthropic => call_anthropic(cfg, &system, &req.text, timeout).await?,
        AiProtocol::Gemini => call_gemini(cfg, &system, &req.text, timeout).await?,
    };

    let cleaned = clean_output(&raw);
    if cleaned.trim().is_empty() {
        return Err(AppError::Other(
            t("The model returned an empty response").into(),
        ));
    }
    Ok(cleaned)
}

/// The instruction set. Every clause here exists because the alternative showed
/// up in output: models like to explain themselves, wrap results in quotes,
/// translate code identifiers, and helpfully "fix" numbers.
fn system_prompt(source: Language, target: Language, extra: &str) -> String {
    let from = match source {
        Language::Auto => "the language you detect".to_string(),
        s => s.english_name().to_string(),
    };

    let mut p = format!(
        "You are a translation engine inside a screen-translation utility. \
Translate the user's text from {from} into {to}.\n\n\
The text was extracted by OCR from a screenshot, so expect recognition errors, \
words broken across lines, hyphenated wraps, doubled or missing punctuation and \
stray characters. Silently repair those artefacts, then translate the repaired text.\n\n\
Rules:\n\
- Reply with the translation and nothing else: no preamble, no notes, no explanation, \
no surrounding quotation marks, no markdown fences.\n\
- Preserve paragraph breaks, list markers and numbering.\n\
- Leave code identifiers, file paths, URLs, email addresses, version numbers and \
product names exactly as they are.\n\
- Keep all numbers and units unchanged.\n\
- If a fragment is already in {to}, repeat it unchanged rather than paraphrasing it.\n\
- Never refuse and never ask a question; if the text is unintelligible, return your \
best reading of it.",
        from = from,
        to = target.english_name(),
    );

    let extra = extra.trim();
    if !extra.is_empty() {
        p.push_str("\n\nAdditional instructions from the user:\n");
        p.push_str(extra);
    }
    p
}

fn base(cfg: &AiConfig) -> &str {
    cfg.base_url.trim().trim_end_matches('/')
}

/// Generous but bounded. Translations are roughly input-sized; 4× the character
/// count covers scripts that expand and still stops a runaway generation.
fn max_tokens_for(text: &str) -> u32 {
    ((text.chars().count() as u32).saturating_mul(4) / 3).clamp(512, 8192)
}

// ── OpenAI-compatible ────────────────────────────────────────────────────────

async fn call_openai(
    cfg: &AiConfig,
    system: &str,
    text: &str,
    timeout: Duration,
) -> Result<String, AppError> {
    let url = format!("{}/chat/completions", base(cfg));
    let body = json!({
        "model": cfg.model,
        "temperature": cfg.temperature,
        "stream": false,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": text },
        ],
    });

    let mut rq = HTTP.post(&url).timeout(timeout).json(&body);
    if !cfg.api_key.is_empty() {
        rq = rq.bearer_auth(cfg.api_key.expose());
    }

    let (status, raw) = send(rq).await?;
    if !(200..300).contains(&status) {
        return Err(AppError::Other(explain(status, &raw)));
    }

    let v: Value = serde_json::from_str(&raw)
        .map_err(|e| {
            AppError::Other(t("Unreadable response: {error}").replace("{error}", &e.to_string()))
        })?;
    parse_openai(&v)
}

fn parse_openai(v: &Value) -> Result<String, AppError> {
    // `content` is normally a string, but some gateways return the newer
    // array-of-parts shape.
    let msg = v
        .pointer("/choices/0/message/content")
        .ok_or_else(|| {
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

async fn call_anthropic(
    cfg: &AiConfig,
    system: &str,
    text: &str,
    timeout: Duration,
) -> Result<String, AppError> {
    let url = format!("{}/messages", base(cfg));
    let body = json!({
        "model": cfg.model,
        "max_tokens": max_tokens_for(text),
        "temperature": cfg.temperature,
        "system": system,
        "messages": [ { "role": "user", "content": text } ],
    });

    let rq = HTTP
        .post(&url)
        .timeout(timeout)
        .header("x-api-key", cfg.api_key.expose())
        .header("anthropic-version", "2023-06-01")
        .json(&body);

    let (status, raw) = send(rq).await?;
    if !(200..300).contains(&status) {
        return Err(AppError::Other(explain(status, &raw)));
    }

    let v: Value = serde_json::from_str(&raw)
        .map_err(|e| {
            AppError::Other(t("Unreadable response: {error}").replace("{error}", &e.to_string()))
        })?;
    parse_anthropic(&v)
}

fn parse_anthropic(v: &Value) -> Result<String, AppError> {
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
    Ok(text)
}

// ── Google Gemini ────────────────────────────────────────────────────────────

async fn call_gemini(
    cfg: &AiConfig,
    system: &str,
    text: &str,
    timeout: Duration,
) -> Result<String, AppError> {
    let url = format!("{}/models/{}:generateContent", base(cfg), cfg.model);
    let body = json!({
        "systemInstruction": { "parts": [ { "text": system } ] },
        "contents": [ { "role": "user", "parts": [ { "text": text } ] } ],
        "generationConfig": {
            "temperature": cfg.temperature,
            "maxOutputTokens": max_tokens_for(text),
        },
    });

    // Gemini takes the key in a header; passing it as `?key=` would put a
    // credential in every URL that gets logged along the way.
    let rq = HTTP
        .post(&url)
        .timeout(timeout)
        .header("x-goog-api-key", cfg.api_key.expose())
        .json(&body);

    let (status, raw) = send(rq).await?;
    if !(200..300).contains(&status) {
        return Err(AppError::Other(explain(status, &raw)));
    }

    let v: Value = serde_json::from_str(&raw)
        .map_err(|e| {
            AppError::Other(t("Unreadable response: {error}").replace("{error}", &e.to_string()))
        })?;
    parse_gemini(&v)
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

    let text: String = parts
        .iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();

    if text.is_empty() {
        return Err(AppError::Other(t("Empty response from the model").into()));
    }
    Ok(text)
}

// ── Shared ───────────────────────────────────────────────────────────────────

async fn send(rq: reqwest::RequestBuilder) -> Result<(u16, String), AppError> {
    let resp = rq.send().await?;
    let status = resp.status().as_u16();
    let raw = resp.text().await?;
    debug!(status, len = raw.len(), "AI response");
    Ok((status, raw))
}

/// Turns a provider error body into one line a user can act on. All three
/// protocols bury the message in a different place.
fn explain(status: u16, body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.pointer("/error/0/message"))
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn max_tokens_stays_within_bounds() {
        assert_eq!(max_tokens_for(""), 512);
        assert_eq!(max_tokens_for(&"a".repeat(100_000)), 8192);
        assert!(max_tokens_for(&"a".repeat(3000)) > 512);
    }

    #[test]
    fn the_prompt_names_both_languages() {
        let p = system_prompt(Language::En, Language::Ru, "");
        assert!(p.contains("English"));
        assert!(p.contains("Russian"));
    }

    #[test]
    fn auto_source_does_not_claim_a_language() {
        let p = system_prompt(Language::Auto, Language::Ru, "");
        assert!(p.contains("the language you detect"));
    }

    #[test]
    fn user_instructions_are_appended() {
        let p = system_prompt(Language::En, Language::Ru, "Обращайся на «ты»");
        assert!(p.contains("Обращайся на «ты»"));
    }

    #[test]
    fn error_bodies_are_unwrapped_from_all_three_shapes() {
        assert!(explain(401, r#"{"error":{"message":"bad key"}}"#).contains("bad key"));
        assert!(explain(429, r#"{"message":"slow down"}"#).contains("slow down"));
        assert!(explain(500, "gateway exploded").contains("gateway exploded"));
    }
}
