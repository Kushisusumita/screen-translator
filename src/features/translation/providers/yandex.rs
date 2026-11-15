//! Yandex Translate.
//!
//! This used to call the endpoint the web page itself uses, with a session id
//! made up on the spot. Yandex now ties that endpoint to a session issued by
//! the page — which sits behind a captcha for anything that is not a browser —
//! so every call came back `405 Session is invalid`. The page scrape that was
//! kept as a second chance fetched the captcha instead of the translator. The
//! upshot was that "Yandex" in the engine list quietly did nothing: each
//! translation paid for two failed round trips and was then done by Google.
//!
//! The mobile endpoint asks for no page session, only a client id, and it takes
//! a *list* of texts — one entry per paragraph in, one translation per entry
//! out. That is exactly the shape the in-place view needs, so paragraphs go out
//! as separate entries rather than as one string with line breaks to be hoped
//! for on the way back.
//!
//! The headless browser remains as an opt-in last resort.

use std::time::Duration;

use serde_json::Value;
use tracing::{debug, info, warn};

use super::{LinePlan, TranslateRequest, MAX_CHUNK_CHARS};
use crate::entities::language::Language;
use crate::features::translation::client::{HTTP, YANDEX_SESSION};
use crate::shared::error::AppError;
use crate::shared::i18n::t;
use crate::shared::logging::clip;

const API: &str = "https://translate.yandex.net/api/v1/tr.json/translate";
const WEB: &str = "https://translate.yandex.ru/";
/// Entries per request. The endpoint takes more; a whole screen of text is
/// better spread over a few quick calls than risked on one large one.
const MAX_TEXTS_PER_REQUEST: usize = 40;

pub async fn translate(req: &TranslateRequest, allow_headless: bool) -> Result<String, AppError> {
    let api_error = match api_translate(req).await {
        Ok(text) if !text.trim().is_empty() => return Ok(text),
        Ok(_) => AppError::Other(t("API: empty response").to_string()),
        Err(e) => e,
    };

    if !allow_headless {
        return Err(api_error);
    }

    debug!(error = %api_error, "Yandex API failed, trying the browser");
    match headless(req).await {
        Ok(text) if !text.trim().is_empty() => Ok(text),
        Ok(_) => Err(AppError::Other(
            t("headless: empty response").to_string(),
        )),
        Err(e) => {
            warn!(api = %api_error, browser = %e, "Both Yandex paths failed");
            Err(e)
        }
    }
}

// ── The mobile endpoint ──────────────────────────────────────────────────────

fn lang_pair(req: &TranslateRequest) -> String {
    match req.source {
        // Omitting the source half asks Yandex to detect it.
        Language::Auto => req.target.code().to_string(),
        src if src == req.target => req.target.code().to_string(),
        src => format!("{}-{}", src.code(), req.target.code()),
    }
}

async fn api_translate(req: &TranslateRequest) -> Result<String, AppError> {
    let lang = lang_pair(req);
    let plan = LinePlan::new(&req.text, MAX_CHUNK_CHARS);
    let mut translated: Vec<String> = Vec::with_capacity(plan.pieces.len());

    for batch in plan.batches(MAX_CHUNK_CHARS, MAX_TEXTS_PER_REQUEST) {
        let mut form: Vec<(&str, &str)> = plan.pieces[batch.clone()]
            .iter()
            .map(|piece| ("text", piece.as_str()))
            .collect();
        form.push(("lang", lang.as_str()));

        let resp = HTTP
            .post(API)
            .query(&[
                ("ucid", YANDEX_SESSION.ucid.as_str()),
                ("srv", "android"),
                ("format", "text"),
            ])
            .header("Accept", "*/*")
            .form(&form)
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;

        if !status.is_success() {
            return Err(AppError::Other(format!(
                "HTTP {} — {}",
                status,
                clip(body.trim(), 160)
            )));
        }

        let texts = parse_api(&body)?;
        if texts.len() != batch.len() {
            return Err(AppError::Other(
                t("The service answered with a different number of lines").to_string(),
            ));
        }
        translated.extend(texts);
    }

    plan.assemble(&translated).ok_or_else(|| {
        AppError::Other(t("The service answered with a different number of lines").to_string())
    })
}

/// `{"code":200,"lang":"en-ru","text":["…","…"]}` — one entry per text sent.
fn parse_api(body: &str) -> Result<Vec<String>, AppError> {
    let json: Value = serde_json::from_str(body).map_err(|e| {
        AppError::Other(t("Unrecognized response: {error}").replace("{error}", &e.to_string()))
    })?;

    if let Some(code) = json.get("code").and_then(Value::as_i64) {
        if code != 200 {
            let msg = json
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or(t("no details"));
            return Err(AppError::Other(
                t("Code {code}: {message}")
                    .replace("{code}", &code.to_string())
                    .replace("{message}", msg),
            ));
        }
    }

    let parts = json
        .get("text")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Other(t("The response has no text field").to_string()))?;

    let texts: Vec<String> = parts
        .iter()
        .map(|p| p.as_str().unwrap_or_default().to_string())
        .collect();
    if texts.is_empty() {
        return Err(AppError::Other(t("The text array is empty").to_string()));
    }
    Ok(texts)
}

// ── Last resort: drive a real browser ────────────────────────────────────────

#[cfg(windows)]
fn find_chromium_executable() -> Option<std::path::PathBuf> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for exe in ["chrome.exe", "msedge.exe", "chromium.exe"] {
        let key_path = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
        if let Ok(key) = hklm.open_subkey_with_flags(&key_path, KEY_READ) {
            if let Ok(path) = key.get_value::<String, _>("") {
                let p = std::path::PathBuf::from(&path);
                if p.exists() {
                    info!(browser = %p.display(), "Found browser via registry");
                    return Some(p);
                }
            }
        }
    }
    warn!("No Chromium-based browser found in registry");
    None
}

#[cfg(not(windows))]
fn find_chromium_executable() -> Option<std::path::PathBuf> {
    let found = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ]
    .into_iter()
    .map(std::path::PathBuf::from)
    .find(|p| p.exists());
    if let Some(p) = &found {
        info!(browser = %p.display(), "Found browser");
    }
    found
}

async fn headless(req: &TranslateRequest) -> Result<String, AppError> {
    use chromiumoxide::{Browser, BrowserConfig};
    use futures::StreamExt as _;
    use tokio::time::{sleep, timeout};

    let exe = find_chromium_executable()
        .ok_or_else(|| AppError::Other(t("Chrome or Edge not found").to_string()))?;

    let src = match req.source {
        Language::Auto => "auto".to_string(),
        s => s.code().to_string(),
    };
    let url = format!(
        "{WEB}?source_lang={}&target_lang={}&text={}",
        src,
        req.target.code(),
        urlencoding::encode(&req.text)
    );

    let config = BrowserConfig::builder()
        .chrome_executable(&exe)
        .arg("--headless=new")
        .arg("--no-sandbox")
        .arg("--disable-gpu")
        .arg("--disable-dev-shm-usage")
        .build()
        .map_err(|e| {
            AppError::Other(
                t("Browser configuration: {error}").replace("{error}", &e.to_string()),
            )
        })?;

    let (mut browser, mut handler) = Browser::launch(config).await.map_err(|e| {
        AppError::Other(t("Browser launch: {error}").replace("{error}", &e.to_string()))
    })?;

    let driver = tokio::spawn(async move { while handler.next().await.is_some() {} });

    // Input area carries id="fakeArea"; the output textbox is the other one.
    const POLL_JS: &str = r#"(() => {
        const el = document.querySelector('[role="textbox"]:not([id="fakeArea"])');
        const t = el && el.innerText.trim();
        return t || null;
    })()"#;

    let outcome = timeout(Duration::from_secs(20), async {
        let page = browser.new_page(&url).await.map_err(|e| {
            AppError::Other(t("Opening the page: {error}").replace("{error}", &e.to_string()))
        })?;
        loop {
            if let Ok(val) = page.evaluate(POLL_JS).await {
                if let Ok(Some(t)) = val.into_value::<Option<String>>() {
                    if !t.is_empty() {
                        return Ok::<String, AppError>(t);
                    }
                }
            }
            sleep(Duration::from_millis(300)).await;
        }
    })
    .await;

    // Whatever happened above, the browser process must not survive this call.
    let _ = browser.close().await;
    let _ = browser.wait().await;
    driver.abort();

    match outcome {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(AppError::Other(
            t("The browser did not translate within 20 s").to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::language::Language;

    fn req(src: Language, tgt: Language) -> TranslateRequest {
        TranslateRequest {
            text: "hi".into(),
            source: src,
            target: tgt,
        }
    }

    #[test]
    fn auto_source_sends_only_the_target() {
        assert_eq!(lang_pair(&req(Language::Auto, Language::Ru)), "ru");
    }

    #[test]
    fn an_explicit_pair_is_hyphenated() {
        assert_eq!(lang_pair(&req(Language::En, Language::Ru)), "en-ru");
    }

    #[test]
    fn identical_source_and_target_degrade_to_detection() {
        // "ru-ru" is rejected by the API; asking it to detect is the useful read.
        assert_eq!(lang_pair(&req(Language::Ru, Language::Ru)), "ru");
    }

    #[test]
    fn every_entry_of_the_reply_is_kept_in_order() {
        let body = r#"{"code":200,"lang":"en-ru","text":["Привет","мир"]}"#;
        assert_eq!(parse_api(body).unwrap(), vec!["Привет", "мир"]);
    }

    #[test]
    fn an_error_code_becomes_an_error() {
        let body = r#"{"code":405,"message":"Session is invalid"}"#;
        let err = parse_api(body).unwrap_err().to_string();
        assert!(err.contains("405"), "{err}");
        assert!(err.contains("Session is invalid"), "{err}");
    }

    #[test]
    fn a_reply_without_translations_is_an_error_not_an_empty_success() {
        assert!(parse_api(r#"{"code":200,"text":[]}"#).is_err());
        assert!(parse_api(r#"{"code":200}"#).is_err());
        assert!(parse_api("<html>captcha</html>").is_err());
    }
}
