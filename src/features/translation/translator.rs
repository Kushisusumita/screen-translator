use crate::features::translation::client::HTTP;
use crate::shared::error::AppError;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

static SESSION_WARMED: AtomicBool = AtomicBool::new(false);

async fn warmup_session() {
    if SESSION_WARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    let result = HTTP
        .get("https://translate.yandex.ru/")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "ru-RU,ru;q=0.9,en;q=0.8")
        .send()
        .await;
    match result {
        Ok(r) => info!("Session warmup: {}", r.status()),
        Err(e) => warn!("Session warmup failed: {}", e),
    }
}

/// Translate `text` from `src` to `tgt`.
/// Chain: Yandex web scrape → Google Translate.
pub async fn translate(
    text: &str,
    src: &str,
    tgt: &str,
    use_yandex: bool,
    use_google: bool,
) -> Result<String, AppError> {
    if !use_yandex && !use_google {
        return Err(AppError::Other(
            "All translation backends are disabled.\nEnable at least one in Settings → Translation Backends.".to_string(),
        ));
    }

    warmup_session().await;

    if use_yandex {
        // 1. Static HTML scrape — works if Yandex ever adds SSR.
        match yandex_web_scrape(text, src, tgt).await {
            Ok(t) if !t.is_empty() => return Ok(t),
            Ok(_) => warn!("Yandex web scrape returned empty, trying headless"),
            Err(e) => warn!("Yandex web scrape: {} — trying headless", e),
        }

        // 2. Headless browser — browser found via Windows Registry (disk-agnostic).
        match yandex_headless(text, src, tgt).await {
            Ok(t) if !t.is_empty() => return Ok(t),
            Ok(_) => warn!("Yandex headless returned empty"),
            Err(e) => warn!("Yandex headless: {}", e),
        }
    }

    if use_google {
        match google_translate(text, src, tgt).await {
            Ok(t) if !t.is_empty() => return Ok(t),
            Ok(_) => warn!("Google translate returned empty"),
            Err(e) => warn!("Google translate: {}", e),
        }
    }

    let msg = match (use_yandex, use_google) {
        (true, true) => {
            "Translation failed.\nPlease check your internet connection.\n\
             Both Yandex and Google are enabled but neither responded."
        }
        (true, false) => {
            "Yandex translation failed.\n\
             Try enabling Google Translate as a fallback in\nSettings → Translation Backends."
        }
        (false, true) => {
            "Google translation failed.\n\
             Try enabling Yandex Translate as a fallback in\nSettings → Translation Backends."
        }
        (false, false) => unreachable!(),
    };
    Err(AppError::Other(msg.to_string()))
}

/// Fetch the Yandex Translate page and extract the translation from the page HTML.
async fn yandex_web_scrape(text: &str, src: &str, tgt: &str) -> Result<String, AppError> {
    let encoded = urlencoding::encode(text);
    let url = format!(
        "https://translate.yandex.ru/?source_lang={}&target_lang={}&text={}",
        src, tgt, encoded
    );

    let resp = HTTP
        .get(&url)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "ru-RU,ru;q=0.9,en;q=0.8")
        .header("Referer", "https://translate.yandex.ru/")
        .send()
        .await?;

    let status = resp.status();
    let html = resp.text().await?;
    info!("Yandex web scrape status={} html_len={}", status, html.len());

    if !status.is_success() {
        return Err(AppError::Other(format!("Yandex web: HTTP {}", status)));
    }

    // Try every known Yandex SPA global variable name.
    let global_vars = [
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

    for marker in &global_vars {
        if let Some(t) = extract_from_global_var(&html, marker) {
            if !t.is_empty() {
                return Ok(t);
            }
        }
    }

    // Try meta description fallback.
    if let Some(t) = extract_from_meta(
        &html,
        &[r#"property="og:description" content=""#, r#"name="description" content=""#],
    ) {
        if !t.is_empty() {
            return Ok(t);
        }
    }

    // Scan every <script> tag for JSON that looks like a translation response.
    if let Some(t) = scan_all_scripts(&html) {
        return Ok(t);
    }

    // Log which global assignments exist so we can add the right extractor next time.
    let found_globals = collect_global_var_names(&html);
    if found_globals.is_empty() {
        debug!("Yandex page: no global window.* assignments found in scripts");
    } else {
        debug!("Yandex page globals found: {}", found_globals.join(", "));
    }

    Err(AppError::Other(
        "Could not extract translation from Yandex web page".into(),
    ))
}

// ── Extraction helpers ────────────────────────────────────────────────────────

fn extract_from_global_var(html: &str, marker: &str) -> Option<String> {
    let start = html.find(marker)?;
    let after = &html[start + marker.len()..];
    let json_str = extract_json_object(after)?;
    let v: Value = serde_json::from_str(&json_str).ok()?;

    let paths = [
        "result.translations.0.text",
        "translationData.result.0",
        "translation.result",
        "data.result",
        "result.text",
        "result.texts.0",
        "translations.0.text",
        "translation.0",
    ];
    for path in &paths {
        if let Some(t) = json_path(&v, path) {
            return Some(strip_html(&t));
        }
    }

    scan_json_for_translation(&v, 0).map(|t| strip_html(&t))
}

/// Scan all `<script>` blocks for a JSON object containing translation data.
fn scan_all_scripts(html: &str) -> Option<String> {
    let mut pos = 0;
    while let Some(open) = html[pos..].find("<script") {
        let abs_open = pos + open;
        // Skip to end of opening tag.
        let tag_end = html[abs_open..].find('>')? + abs_open + 1;
        // Find </script>.
        let close = html[tag_end..].find("</script>")? + tag_end;
        let content = &html[tag_end..close];
        pos = close + 9;

        if let Some(t) = scan_script_for_translation(content) {
            return Some(t);
        }
    }
    None
}

fn scan_script_for_translation(script: &str) -> Option<String> {
    let mut search = script;
    while let Some(brace) = search.find('{') {
        if let Some(json_str) = extract_json_object(&search[brace..]) {
            if let Ok(v) = serde_json::from_str::<Value>(&json_str) {
                if let Some(t) = scan_json_for_translation(&v, 0) {
                    return Some(strip_html(&t));
                }
            }
            search = &search[brace + 1..];
        } else {
            break;
        }
    }
    None
}

/// Collect names of `window.X =` assignments for diagnostics.
fn collect_global_var_names(html: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut pos = 0;
    while let Some(w) = html[pos..].find("window.") {
        let abs = pos + w;
        let rest = &html[abs + 7..];
        if let Some(eq) = rest.find('=') {
            let name = rest[..eq].trim();
            if name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.is_empty() {
                names.push(name.to_string());
            }
        }
        pos = abs + 7;
    }
    names.sort();
    names.dedup();
    names
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
    cur.as_str().map(|s| s.to_string())
}

fn scan_json_for_translation(v: &Value, depth: u8) -> Option<String> {
    if depth > 6 {
        return None;
    }
    if let Value::Object(map) = v {
        for key in &[
            "translation", "translated", "translatedText", "result",
            "text", "texts", "output", "outputText",
        ] {
            if let Some(val) = map.get(*key) {
                if let Some(s) = val.as_str() {
                    if !s.is_empty() {
                        return Some(s.to_string());
                    }
                }
                if let Some(first) = val
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                {
                    if !first.is_empty() {
                        return Some(first.to_string());
                    }
                }
            }
        }
        for (_, child) in map {
            if let Some(t) = scan_json_for_translation(child, depth + 1) {
                return Some(t);
            }
        }
    }
    None
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

fn extract_from_meta(html: &str, markers: &[&str]) -> Option<String> {
    for marker in markers {
        if let Some(pos) = html.find(marker) {
            let rest = &html[pos + marker.len()..];
            if let Some(end) = rest.find('"') {
                let content = &rest[..end];
                if !content.is_empty() {
                    return Some(content.to_string());
                }
            }
        }
    }
    None
}

// ── Yandex headless (Chromium CDP) ───────────────────────────────────────────

/// Find a Chromium-based browser via the Windows Registry.
/// Chrome, Edge and other Chromium browsers register their path under
/// `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\<exe>`.
/// This is disk-agnostic — works regardless of install drive or folder.
fn find_chromium_executable() -> Option<std::path::PathBuf> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    // Edge is installed on every Windows 10/11 machine.
    // Chrome is checked first because it tends to start slightly faster.
    let candidates = ["chrome.exe", "msedge.exe", "chromium.exe"];

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for exe in &candidates {
        let key_path = format!(
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{}",
            exe
        );
        if let Ok(key) = hklm.open_subkey_with_flags(&key_path, KEY_READ) {
            // Default value ("") holds the full path to the executable.
            if let Ok(path) = key.get_value::<String, _>("") {
                let p = std::path::PathBuf::from(&path);
                if p.exists() {
                    info!("Found browser via registry: {}", p.display());
                    return Some(p);
                }
            }
        }
    }

    warn!("No Chromium-based browser found in registry (chrome.exe / msedge.exe)");
    None
}

/// Launch a headless Chromium browser (Chrome or Edge), navigate to
/// translate.yandex.ru, and poll until the SPA renders the translated text.
async fn yandex_headless(text: &str, src: &str, tgt: &str) -> Result<String, AppError> {
    use chromiumoxide::{Browser, BrowserConfig};
    use futures::StreamExt as _;
    use tokio::time::{sleep, timeout, Duration};

    let exe = find_chromium_executable().ok_or_else(|| {
        AppError::Other(
            "headless: no Chromium browser found (Chrome or Edge required)".into(),
        )
    })?;
    info!("Yandex headless: browser = {}", exe.display());

    let url = format!(
        "https://translate.yandex.ru/?source_lang={}&target_lang={}&text={}",
        src,
        tgt,
        urlencoding::encode(text)
    );

    let config = BrowserConfig::builder()
        .chrome_executable(&exe)
        .arg("--headless=new")
        .arg("--no-sandbox")
        .arg("--disable-gpu")
        .arg("--disable-dev-shm-usage")
        .build()
        .map_err(|e| AppError::Other(format!("Browser config: {}", e)))?;

    let (mut browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| AppError::Other(format!("Browser launch: {}", e)))?;

    // CDP handler must be polled from a separate task.
    let _driver = tokio::spawn(async move {
        while let Some(_) = handler.next().await {}
    });

    let page = browser
        .new_page(&url)
        .await
        .map_err(|e| AppError::Other(format!("Browser page: {}", e)))?;

    // Yandex Translate is a SPA — poll via JS until the output textbox appears.
    // Input area has id="fakeArea"; output area has role="textbox" without that id.
    const POLL_JS: &str = r#"(() => {
        const el = document.querySelector('[role="textbox"]:not([id="fakeArea"])');
        const t = el && el.innerText.trim();
        return t || null;
    })()"#;

    let result = timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(val) = page.evaluate(POLL_JS).await {
                if let Ok(Some(t)) = val.into_value::<Option<String>>() {
                    if !t.is_empty() {
                        break t;
                    }
                }
            }
            sleep(Duration::from_millis(400)).await;
        }
    })
    .await
    .map_err(|_| AppError::Other("Timeout: Yandex did not translate within 30s".into()))?;

    let _ = browser.close().await;
    info!("Yandex headless result len={}", result.len());
    Ok(result)
}

// ── Google Translate ──────────────────────────────────────────────────────────

async fn google_translate(text: &str, src: &str, tgt: &str) -> Result<String, AppError> {
    let resp = HTTP
        .get("https://translate.googleapis.com/translate_a/single")
        .query(&[("client", "gtx"), ("sl", src), ("tl", tgt), ("dt", "t"), ("q", text)])
        .header("Accept", "*/*")
        .send()
        .await?;

    let status = resp.status();
    let raw = resp.text().await?;
    info!("Google translate status={} body={}", status, &raw[..raw.len().min(300)]);

    if !status.is_success() {
        return Err(AppError::Other(format!("Google translate {}: {}", status, raw)));
    }

    let json: Value = serde_json::from_str(&raw)?;
    let translated = json[0]
        .as_array()
        .ok_or_else(|| AppError::Other("Unexpected Google response".into()))?
        .iter()
        .filter_map(|seg| seg[0].as_str())
        .collect::<Vec<_>>()
        .join("");

    if translated.is_empty() {
        return Err(AppError::Other("Google returned empty translation".into()));
    }
    Ok(translated)
}

// ── HTML strip ────────────────────────────────────────────────────────────────

fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut chars = html.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            '&' if !in_tag => {
                let entity: String = chars.by_ref().take_while(|&ch| ch != ';').collect();
                let ch = match entity.as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    _ => {
                        out.push('&');
                        out.push_str(&entity);
                        out.push(';');
                        None
                    }
                };
                if let Some(ch) = ch {
                    out.push(ch);
                }
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}
