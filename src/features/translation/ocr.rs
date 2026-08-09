//! Yandex OCR.
//!
//! Two changes over the original beyond bug fixes. The response is now read
//! **structurally** — boxes are lines and blocks are paragraphs, so the layout
//! of the original text survives into the translator instead of every line being
//! glued together with spaces. And the detected language is mapped back onto the
//! `Language` enum rather than passed through as a raw string, so a detector
//! answer of `zh-CN` no longer produces a translation request for a language
//! code that no engine recognises.

use serde_json::Value;
use tracing::{debug, info, warn};

use crate::entities::language::Language;
use crate::features::translation::client::{HTTP, YANDEX_SESSION};
use crate::shared::error::AppError;
use crate::shared::i18n::t;
use crate::shared::logging::{clip, redact};

pub struct OcrResult {
    /// Lines grouped into paragraphs by blank lines, ready for `normalize_ocr_text`.
    pub text: String,
    pub detected: Option<Language>,
}

pub async fn recognize(jpeg: &[u8]) -> Result<OcrResult, AppError> {
    let s = &*YANDEX_SESSION;
    let url = format!(
        "https://translate.yandex.net/ocr/v1.1/recognize\
         ?srv=tr-image&sid={sid}&lang=*&rotate=auto&yu={yu}&yum={yum}",
        sid = s.ocr_sid,
        yu = s.yu,
        yum = s.yum,
    );

    let part = reqwest::multipart::Part::bytes(jpeg.to_vec())
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

    // `&body[..600]` panicked here whenever byte 600 landed inside a Cyrillic or
    // CJK character — which is most of the time, since that is what gets OCRed.
    debug!(status = status.as_u16(), body = %clip(&body, 600), "OCR response");

    if !status.is_success() {
        return Err(AppError::Other(format!(
            "OCR HTTP {} — {}",
            status,
            clip(body.trim(), 200)
        )));
    }

    let json: Value = serde_json::from_str(&body).map_err(|e| {
        AppError::Other(t("Unreadable OCR response: {error}").replace("{error}", &e.to_string()))
    })?;

    let result = parse(&json);

    if result.text.trim().is_empty() {
        warn!(detected = ?result.detected, "OCR returned no text");
    } else {
        info!(
            lines = result.text.lines().count(),
            chars = result.text.chars().count(),
            detected = result.detected.map(|l| l.code()).unwrap_or("?"),
            "OCR complete"
        );
        debug!(text = %redact(&result.text));
    }

    Ok(result)
}

fn parse(json: &Value) -> OcrResult {
    let detected = json
        .pointer("/data/detected_lang")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && *s != "*")
        .and_then(Language::from_code);

    let mut paragraphs: Vec<String> = Vec::new();

    if let Some(blocks) = json.pointer("/data/blocks").and_then(Value::as_array) {
        for block in blocks {
            let Some(boxes) = block.get("boxes").and_then(Value::as_array) else {
                continue;
            };
            // One box per visual line; keeping them on separate lines lets the
            // normaliser rejoin wrapped sentences and hyphenated words correctly.
            let lines: Vec<&str> = boxes
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            if !lines.is_empty() {
                paragraphs.push(lines.join("\n"));
            }
        }
    }

    OcrResult {
        text: paragraphs.join("\n\n"),
        detected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lines_within_a_block_stay_on_separate_lines() {
        let v = json!({"data":{"detected_lang":"en","blocks":[
            {"boxes":[{"text":"Machine trans-"},{"text":"lation is here"}]}
        ]}});
        let r = parse(&v);
        assert_eq!(r.text, "Machine trans-\nlation is here");
        assert_eq!(r.detected, Some(Language::En));
    }

    #[test]
    fn separate_blocks_become_separate_paragraphs() {
        let v = json!({"data":{"blocks":[
            {"boxes":[{"text":"Заголовок"}]},
            {"boxes":[{"text":"Текст"}]}
        ]}});
        assert_eq!(parse(&v).text, "Заголовок\n\nТекст");
    }

    #[test]
    fn a_regional_detection_tag_maps_onto_the_enum() {
        let v = json!({"data":{"detected_lang":"zh-CN","blocks":[]}});
        assert_eq!(parse(&v).detected, Some(Language::Zh));
    }

    #[test]
    fn the_wildcard_detection_answer_means_unknown() {
        let v = json!({"data":{"detected_lang":"*","blocks":[]}});
        assert_eq!(parse(&v).detected, None);
    }

    #[test]
    fn empty_boxes_are_dropped_without_leaving_blank_lines() {
        let v = json!({"data":{"blocks":[
            {"boxes":[{"text":"  "},{"text":"real"},{"text":""}]}
        ]}});
        assert_eq!(parse(&v).text, "real");
    }

    #[test]
    fn a_response_with_no_data_section_yields_empty_text_not_a_panic() {
        assert_eq!(parse(&json!({"error":"nope"})).text, "");
    }
}
