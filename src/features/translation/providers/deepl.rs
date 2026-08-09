//! DeepL, for users who have a key and want the best non-LLM quality available.

use serde_json::{json, Value};
use tracing::debug;

use super::{chunk_text, TranslateRequest, MAX_CHUNK_CHARS};
use crate::entities::language::Language;
use crate::features::translation::client::HTTP;
use crate::shared::error::AppError;
use crate::shared::i18n::t;
use crate::shared::logging::clip;
use crate::shared::secret::Secret;

const FREE: &str = "https://api-free.deepl.com/v2/translate";
const PRO: &str = "https://api.deepl.com/v2/translate";

/// Free-tier keys carry a `:fx` suffix and only work against the free host;
/// sending them to the pro host returns 403 with no useful explanation.
fn endpoint_for(key: &str) -> &'static str {
    if key.trim().ends_with(":fx") {
        FREE
    } else {
        PRO
    }
}

/// DeepL wants upper-case tags, and for a few languages a regional variant.
fn target_code(lang: Language) -> Option<&'static str> {
    Some(match lang {
        Language::En => "EN-US",
        Language::Pt => "PT-PT",
        Language::Zh => "ZH",
        Language::Ru => "RU",
        Language::Uk => "UK",
        Language::De => "DE",
        Language::Fr => "FR",
        Language::Es => "ES",
        Language::It => "IT",
        Language::Pl => "PL",
        Language::Nl => "NL",
        Language::Tr => "TR",
        Language::Cs => "CS",
        Language::Sv => "SV",
        Language::El => "EL",
        Language::Ro => "RO",
        Language::Hu => "HU",
        Language::Fi => "FI",
        Language::Da => "DA",
        Language::Bg => "BG",
        Language::Id => "ID",
        Language::Ja => "JA",
        Language::Ko => "KO",
        Language::Ar => "AR",
        // Everything else is outside DeepL's set; the chain moves on.
        _ => return None,
    })
}

fn source_code(lang: Language) -> Option<&'static str> {
    match lang {
        Language::Auto => None,
        other => target_code(other).map(|c| match c {
            // Source side takes the bare tag, not the regional variant.
            "EN-US" => "EN",
            "PT-PT" => "PT",
            other => other,
        }),
    }
}

pub async fn translate(req: &TranslateRequest, key: &Secret) -> Result<String, AppError> {
    if key.is_empty() {
        return Err(AppError::Other(t("No API key set").to_string()));
    }
    let target = target_code(req.target).ok_or_else(|| {
        AppError::Other(
            t("DeepL does not support {language}")
                .replace("{language}", &req.target.to_string()),
        )
    })?;

    let url = endpoint_for(key.expose());
    let chunks = chunk_text(&req.text, MAX_CHUNK_CHARS);

    let mut out = String::with_capacity(req.text.len());
    for (i, chunk) in chunks.iter().enumerate() {
        let mut body = json!({
            "text": [chunk],
            "target_lang": target,
        });
        if let Some(src) = source_code(req.source) {
            body["source_lang"] = json!(src);
        }

        let resp = HTTP
            .post(url)
            .header("Authorization", format!("DeepL-Auth-Key {}", key.expose()))
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let raw = resp.text().await?;
        debug!(status = status.as_u16(), "DeepL response");

        if !status.is_success() {
            return Err(AppError::Other(explain(status.as_u16(), &raw)));
        }

        let piece = parse(&raw)?;
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&piece);
    }

    Ok(out)
}

fn parse(raw: &str) -> Result<String, AppError> {
    let json: Value = serde_json::from_str(raw).map_err(|e| {
        AppError::Other(
            t("Unrecognized response from DeepL: {error}").replace("{error}", &e.to_string()),
        )
    })?;
    let parts: Vec<&str> = json
        .get("translations")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| t.get("text").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();

    if parts.is_empty() {
        return Err(AppError::Other(
            t("DeepL returned an empty translation").to_string(),
        ));
    }
    Ok(parts.join(" "))
}

/// DeepL's status codes have specific, actionable meanings — worth spelling out
/// rather than showing the user a bare number.
fn explain(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| clip(body.trim(), 160).to_string());

    match status {
        403 => t("Key rejected — check the key and the account type (a free key ends in :fx)")
            .to_string(),
        429 => t("Too many requests, try again later").to_string(),
        456 => t("The DeepL account is out of characters").to_string(),
        _ => format!("HTTP {status} — {detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_keys_go_to_the_free_host() {
        assert_eq!(endpoint_for("abc-123:fx"), FREE);
        assert_eq!(endpoint_for("abc-123"), PRO);
    }

    #[test]
    fn english_and_portuguese_targets_use_a_regional_variant() {
        assert_eq!(target_code(Language::En), Some("EN-US"));
        assert_eq!(target_code(Language::Pt), Some("PT-PT"));
    }

    #[test]
    fn the_source_side_drops_the_region() {
        assert_eq!(source_code(Language::En), Some("EN"));
        assert_eq!(source_code(Language::Pt), Some("PT"));
        assert_eq!(source_code(Language::Auto), None);
    }

    #[test]
    fn an_unsupported_target_is_reported_not_silently_wrong() {
        assert_eq!(target_code(Language::Hi), None);
    }

    #[test]
    fn parses_translations() {
        let raw = r#"{"translations":[{"detected_source_language":"EN","text":"Привет"}]}"#;
        assert_eq!(parse(raw).unwrap(), "Привет");
    }

    #[test]
    fn quota_exhaustion_says_so() {
        assert!(explain(456, "{}").contains("out of characters"));
    }
}
