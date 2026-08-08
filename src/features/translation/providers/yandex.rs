//! Yandex Translate.
//!
//! The original implementation went straight to scraping `translate.yandex.ru`
//! and, when that failed, launched a **headless Chrome per translation** and
//! polled it for up to 30 seconds. Since the page is a client-rendered SPA the
//! scrape essentially always failed, so the common path was: browser launch,
//! several seconds of polling, then fall through to Google anyway.
//!
//! The order is inverted here. The JSON endpoint the web client itself calls
//! comes first and answers in a couple of hundred milliseconds. Scraping is kept
//! as a second chance, and the browser is now opt-in and no longer leaks its
//! process when the poll times out.

use std::time::Duration;

use serde_json::Value;
use tracing::{debug, info, warn};

use super::{chunk_text, TranslateRequest, MAX_CHUNK_CHARS};
use crate::entities::language::Language;
use crate::features::translation::client::{HTTP, YANDEX_SESSION};
use crate::shared::error::AppError;
use crate::shared::logging::clip;

const API: &str = "https://translate.yandex.net/api/v1/tr.json/translate";
const WEB: &str = "https://translate.yandex.ru/";

pub async fn translate(req: &TranslateRequest, allow_headless: bool) -> Result<String, AppError> {
    let mut errors = Vec::new();

    match api_translate(req).await {
        Ok(t) if !t.trim().is_empty() => return Ok(t),
        Ok(_) => errors.push("API: пустой ответ".to_string()),
        Err(e) => {
            debug!(error = %e, "Yandex JSON API failed, falling back to page scrape");
            errors.push(format!("API: {e}"));
        }
    }

    match web_scrape(req).await {
        Ok(t) if !t.trim().is_empty() => return Ok(t),
        Ok(_) => errors.push("web: пустой ответ".to_string()),
        Err(e) => errors.push(format!("web: {e}")),
    }

    if allow_headless {
        match headless(req).await {
            Ok(t) if !t.trim().is_empty() => return Ok(t),
            Ok(_) => errors.push("headless: пустой ответ".to_string()),
            Err(e) => errors.push(format!("headless: {e}")),
        }
    }

    Err(AppError::Other(errors.join("; ")))
}

// ── Primary: the endpoint the web client uses ────────────────────────────────

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
    let id = format!("{}-0-0", YANDEX_SESSION.text_sid);
    let chunks = chunk_text(&req.text, MAX_CHUNK_CHARS);

    let mut out = String::with_capacity(req.text.len());
    for (i, chunk) in chunks.iter().enumerate() {
        let resp = HTTP
            .post(API)
            .query(&[("id", id.as_str()), ("srv", "tr-text")])
            .header("Accept", "*/*")
            .header("Referer", "https://translate.yandex.com/")
            .header("Origin", "https://translate.yandex.com")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("text", chunk.as_str()),
                ("lang", lang.as_str()),
                ("options", "4"),
            ])
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

        let piece = parse_api(&body)?;
        if i > 0 && !out.ends_with(' ') && !piece.starts_with(' ') {
            out.push(' ');
        }
        out.push_str(&piece);
    }

    Ok(out)
}

/// `{"code":200,"lang":"en-ru","text":["…"]}`
fn parse_api(body: &str) -> Result<String, AppError> {
    let json: Value = serde_json::from_str(body)
        .map_err(|e| AppError::Other(format!("нераспознанный ответ: {e}")))?;

    if let Some(code) = json.get("code").and_then(Value::as_i64) {
        if code != 200 {
            let msg = json
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("без описания");
            return Err(AppError::Other(format!("код {code}: {msg}")));
        }
    }

    let parts = json
        .get("text")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Other("в ответе нет поля text".into()))?;

    let text: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
    if text.is_empty() {
        return Err(AppError::Other("пустой массив text".into()));
    }
    Ok(text.join("\n"))
}

// ── Second chance: parse whatever the page ships ─────────────────────────────

async fn web_scrape(req: &TranslateRequest) -> Result<String, AppError> {
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

    let resp = HTTP
        .get(&url)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "ru-RU,ru;q=0.9,en;q=0.8")
        .header("Referer", WEB)
        .send()
        .await?;

    let status = resp.status();
    let html = resp.text().await?;
    debug!(
        status = status.as_u16(),
        html_len = html.len(),
        "Yandex page"
    );

    if !status.is_success() {
        return Err(AppError::Other(format!("HTTP {status}")));
    }

    for marker in GLOBAL_VARS {
        if let Some(t) = extract_from_global_var(&html, marker) {
            if !t.is_empty() {
                return Ok(t);
            }
        }
    }

    if let Some(t) = scan_all_scripts(&html) {
        if !t.is_empty() {
            return Ok(t);
        }
    }

    Err(AppError::Other("перевод не найден в HTML".into()))
}

const GLOBAL_VARS: &[&str] = &[
    "window.__data=",
    "window.__data =",
    "window.__STORE__=",
    "window.__STORE__ =",
    "window.__INITIAL_STATE__=",
    "window.__INITIAL_STATE__ =",
    "window.__serverState=",
    "window.__serverState =",
    "window.__SERVER_DATA__=",
    "window.__SERVER_DATA__ =",
    "window.__initialData=",
    "window.__initialData =",
];

fn extract_from_global_var(html: &str, marker: &str) -> Option<String> {
    let start = html.find(marker)?;
    let after = &html[start + marker.len()..];
    let json_str = extract_json_object(after)?;
    let v: Value = serde_json::from_str(&json_str).ok()?;

    const PATHS: &[&str] = &[
        "result.translations.0.text",
        "translationData.result.0",
        "translation.result",
        "data.result",
        "result.text",
        "result.texts.0",
        "translations.0.text",
        "translation.0",
    ];
    for path in PATHS {
        if let Some(t) = json_path(&v, path) {
            return Some(strip_html(&t));
        }
    }
    scan_json_for_translation(&v, 0).map(|t| strip_html(&t))
}

fn scan_all_scripts(html: &str) -> Option<String> {
    let mut pos = 0;
    while let Some(open) = html[pos..].find("<script") {
        let abs_open = pos + open;
        let tag_end = abs_open + html[abs_open..].find('>')? + 1;
        let close = tag_end + html[tag_end..].find("</script>")?;
        let content = &html[tag_end..close];
        pos = close + "</script>".len();

        if let Some(t) = scan_script_for_translation(content) {
            return Some(t);
        }
    }
    None
}

fn scan_script_for_translation(script: &str) -> Option<String> {
    let mut search = script;
    // Bounded: a 2 MB page with a `{` every other byte would otherwise turn this
    // into a million JSON parses.
    let mut budget = 64;
    while budget > 0 {
        let brace = search.find('{')?;
        budget -= 1;
        if let Some(json_str) = extract_json_object(&search[brace..]) {
            if let Ok(v) = serde_json::from_str::<Value>(&json_str) {
                if let Some(t) = scan_json_for_translation(&v, 0) {
                    return Some(strip_html(&t));
                }
            }
        }
        // `{` is ASCII, so this index is always a char boundary.
        search = &search[brace + 1..];
    }
    None
}

fn json_path(v: &Value, path: &str) -> Option<String> {
    let mut cur = v;
    for key in path.split('.') {
        cur = if let Ok(idx) = key.parse::<usize>() {
            cur.get(idx)?
        } else {
            cur.get(key)?
        };
    }
    cur.as_str().map(str::to_string)
}

fn scan_json_for_translation(v: &Value, depth: u8) -> Option<String> {
    if depth > 6 {
        return None;
    }
    let Value::Object(map) = v else {
        return None;
    };

    const KEYS: &[&str] = &[
        "translation",
        "translated",
        "translatedText",
        "result",
        "text",
        "texts",
        "output",
        "outputText",
    ];
    for key in KEYS {
        let Some(val) = map.get(*key) else { continue };
        if let Some(s) = val.as_str() {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
        if let Some(first) = val
            .as_array()
            .and_then(|a| a.first())
            .and_then(Value::as_str)
        {
            if !first.is_empty() {
                return Some(first.to_string());
            }
        }
    }
    map.values()
        .find_map(|child| scan_json_for_translation(child, depth + 1))
}

fn extract_json_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes[start..].iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        match b {
            b'\\' if in_str => escape = true,
            b'"' => in_str = !in_str,
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..start + i + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Strips tags and decodes the handful of entities that show up in translated
/// text.
///
/// The previous version used `take_while(|c| c != ';')` on the char iterator,
/// which swallows the terminator and — for a bare `&` with no `;` anywhere
/// after it — consumed the entire rest of the string.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    let mut in_tag = false;

    while let Some(c) = chars.next() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            '&' if !in_tag => {
                // Entities are short; look ahead a bounded distance for the `;`.
                let mut entity = String::new();
                let mut found = false;
                for _ in 0..10 {
                    match chars.peek() {
                        Some(';') => {
                            chars.next();
                            found = true;
                            break;
                        }
                        Some(&ch) if ch.is_ascii_alphanumeric() || ch == '#' => {
                            entity.push(ch);
                            chars.next();
                        }
                        _ => break,
                    }
                }
                match (found, entity.as_str()) {
                    (true, "amp") => out.push('&'),
                    (true, "lt") => out.push('<'),
                    (true, "gt") => out.push('>'),
                    (true, "quot") => out.push('"'),
                    (true, "apos") | (true, "#39") => out.push('\''),
                    (true, "nbsp") => out.push(' '),
                    (true, other) => {
                        out.push('&');
                        out.push_str(other);
                        out.push(';');
                    }
                    (false, other) => {
                        out.push('&');
                        out.push_str(other);
                    }
                }
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
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
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ]
    .into_iter()
    .map(std::path::PathBuf::from)
    .find(|p| p.exists())
}

async fn headless(req: &TranslateRequest) -> Result<String, AppError> {
    use chromiumoxide::{Browser, BrowserConfig};
    use futures::StreamExt as _;
    use tokio::time::{sleep, timeout};

    let exe = find_chromium_executable()
        .ok_or_else(|| AppError::Other("не найден Chrome или Edge".into()))?;

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
        .map_err(|e| AppError::Other(format!("конфигурация браузера: {e}")))?;

    let (mut browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| AppError::Other(format!("запуск браузера: {e}")))?;

    let driver = tokio::spawn(async move { while handler.next().await.is_some() {} });

    // Input area carries id="fakeArea"; the output textbox is the other one.
    const POLL_JS: &str = r#"(() => {
        const el = document.querySelector('[role="textbox"]:not([id="fakeArea"])');
        const t = el && el.innerText.trim();
        return t || null;
    })()"#;

    let outcome = timeout(Duration::from_secs(20), async {
        let page = browser
            .new_page(&url)
            .await
            .map_err(|e| AppError::Other(format!("открытие страницы: {e}")))?;
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
    // The previous version returned early on timeout and left Chrome running.
    let _ = browser.close().await;
    let _ = browser.wait().await;
    driver.abort();

    match outcome {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(AppError::Other("браузер не перевёл за 20 с".into())),
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
    fn parses_a_normal_response() {
        let body = r#"{"code":200,"lang":"en-ru","text":["Привет мир"]}"#;
        assert_eq!(parse_api(body).unwrap(), "Привет мир");
    }

    #[test]
    fn an_error_code_becomes_an_error() {
        let body = r#"{"code":400,"message":"invalid parameter: lang"}"#;
        let err = parse_api(body).unwrap_err().to_string();
        assert!(err.contains("400"), "{err}");
        assert!(err.contains("invalid parameter"), "{err}");
    }

    #[test]
    fn strip_html_decodes_known_entities() {
        assert_eq!(strip_html("a &amp; b &lt;c&gt;"), "a & b <c>");
    }

    #[test]
    fn a_bare_ampersand_does_not_eat_the_rest_of_the_string() {
        // The old take_while implementation returned "Tom " here.
        assert_eq!(strip_html("Tom & Jerry вместе"), "Tom & Jerry вместе");
    }

    #[test]
    fn an_unknown_entity_is_left_intact() {
        assert_eq!(strip_html("100&euro; всего"), "100&euro; всего");
    }

    #[test]
    fn tags_are_removed_but_their_text_kept() {
        assert_eq!(strip_html("<b>жирный</b> текст"), "жирный текст");
    }

    #[test]
    fn extract_json_object_respects_braces_inside_strings() {
        let s = r#"= {"a":"}{","b":1} trailing"#;
        assert_eq!(extract_json_object(s).unwrap(), r#"{"a":"}{","b":1}"#);
    }

    #[test]
    fn script_scan_gives_up_instead_of_looping_forever() {
        let junk = "{".repeat(5000);
        assert!(scan_script_for_translation(&junk).is_none());
    }
}
