//! Google Translate via the public `translate_a/single` endpoint.
//!
//! Two things were wrong with the original call and both bit on real input:
//!
//! * the text went in the **query string**, so a screenshot of a paragraph blew
//!   past the URL length limit and came back `413`/`414` instead of a translation;
//! * the response was logged with `&raw[..300]`, which panics the moment the
//!   body's 300th byte lands inside a Cyrillic character — i.e. on every
//!   Russian translation long enough to truncate.
//!
//! Both are fixed here: the text is form-encoded in the body, long input is
//! chunked, and every slice goes through the char-boundary-safe helper.

use serde_json::Value;
use tracing::debug;

use super::{chunk_text, TranslateRequest, MAX_CHUNK_CHARS};
use crate::entities::language::Language;
use crate::features::translation::client::HTTP;
use crate::shared::error::AppError;
use crate::shared::logging::clip;

const ENDPOINT: &str = "https://translate.googleapis.com/translate_a/single";

pub async fn translate(req: &TranslateRequest) -> Result<String, AppError> {
    let sl = match req.source {
        Language::Auto => "auto".to_string(),
        other => other.code().to_string(),
    };
    let tl = req.target.code().to_string();

    let chunks = chunk_text(&req.text, MAX_CHUNK_CHARS);
    let mut out = String::with_capacity(req.text.len());

    for chunk in &chunks {
        let piece = translate_chunk(chunk, &sl, &tl).await?;
        out.push_str(&piece);
    }

    Ok(out)
}

async fn translate_chunk(text: &str, sl: &str, tl: &str) -> Result<String, AppError> {
    // POST, not GET: `q` in the body has no practical length limit.
    let resp = HTTP
        .post(ENDPOINT)
        .query(&[
            ("client", "gtx"),
            ("sl", sl),
            ("tl", tl),
            ("dt", "t"),
            ("ie", "UTF-8"),
            ("oe", "UTF-8"),
        ])
        .header("Accept", "*/*")
        .form(&[("q", text)])
        .send()
        .await?;

    let status = resp.status();
    let raw = resp.text().await?;
    debug!(
        status = status.as_u16(),
        body = %clip(&raw, 300),
        "Google response"
    );

    if !status.is_success() {
        return Err(AppError::Other(format!(
            "HTTP {} — {}",
            status,
            clip(raw.trim(), 200)
        )));
    }

    parse(&raw)
}

/// The response is `[[["translated","original",…],…],…]`. Segment boundaries do
/// not match sentence boundaries, so the pieces are concatenated verbatim —
/// Google already puts the spacing inside them.
fn parse(raw: &str) -> Result<String, AppError> {
    let json: Value = serde_json::from_str(raw)
        .map_err(|e| AppError::Other(format!("нераспознанный ответ Google: {e}")))?;

    let segments = json
        .get(0)
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Other("ответ Google без блока перевода".into()))?;

    let text: String = segments
        .iter()
        .filter_map(|seg| seg.get(0).and_then(Value::as_str))
        .collect();

    if text.trim().is_empty() {
        return Err(AppError::Other("Google вернул пустой перевод".into()));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concatenates_every_segment() {
        let raw = r#"[[["Привет ","Hello ",null,null,10],["мир","world",null,null,10]],null,"en"]"#;
        assert_eq!(parse(raw).unwrap(), "Привет мир");
    }

    #[test]
    fn a_null_segment_does_not_abort_the_rest() {
        let raw =
            r#"[[["Раз ","One ",null,null,0],[null,"x"],["два","two",null,null,0]],null,"en"]"#;
        assert_eq!(parse(raw).unwrap(), "Раз два");
    }

    #[test]
    fn an_all_empty_body_is_an_error_not_an_empty_success() {
        let raw = r#"[[],null,"en"]"#;
        assert!(parse(raw).is_err());
    }

    #[test]
    fn malformed_json_reports_itself() {
        assert!(parse("<html>429</html>").is_err());
    }

    #[test]
    fn a_cyrillic_body_can_be_clipped_without_panicking() {
        // The exact shape that used to panic: cut in the middle of a 2-byte char.
        let body = "к".repeat(400);
        let _ = clip(&body, 300);
    }
}
