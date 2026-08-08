use std::time::Duration;

use once_cell::sync::Lazy;
use rand::Rng;
use std::time::{SystemTime, UNIX_EPOCH};

pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Ceiling for a single provider attempt. Anything slower is not useful for a
/// tool you invoke with a hotkey and wait in front of.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// One client for everything, so OCR and translation share a cookie jar and a
/// connection pool. Per-request timeouts override the default where a provider
/// legitimately needs longer (a large model, say).
///
/// The original code built this with no timeout at all: a stalled TCP connection
/// left the UI showing "переводим…" forever with no way back.
pub static HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .cookie_store(true)
        .timeout(DEFAULT_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(4)
        .build()
        .expect("Failed to build HTTP client")
});

/// Session ID for OCR requests — last segment is "tr-image" in hex.
pub fn generate_sid() -> String {
    let mut rng = rand::thread_rng();
    let a: u32 = rng.gen();
    let b: u32 = rng.gen();
    let c: u32 = rng.gen();
    format!("{a:08x}.{b:08x}.{c:08x}.74722d696d616765")
}

/// Session ID for the text translation endpoint — "tr-text" in hex.
pub fn generate_text_sid() -> String {
    let mut rng = rand::thread_rng();
    let a: u32 = rng.gen();
    let b: u32 = rng.gen();
    let c: u32 = rng.gen();
    format!("{a:08x}.{b:08x}.{c:08x}.74722d74657874")
}

/// `yu` — 19-digit Yandex user identifier.
pub fn generate_yu() -> String {
    let mut rng = rand::thread_rng();
    let n: u64 = rng.gen_range(1_000_000_000_000_000_000..9_999_999_999_999_999_999);
    n.to_string()
}

/// `yum` — Unix seconds followed by 9 random digits, matching what the web
/// client sends.
pub fn generate_yum() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let suffix: u32 = rand::thread_rng().gen_range(100_000_000..999_999_999);
    format!("{ts}{suffix}")
}

/// A Yandex session is a `(sid, yu, yum)` triple. Regenerating it on every
/// request looked like a fresh anonymous user each time, which is exactly the
/// pattern rate limiters key on. Generating it once per process and reusing it
/// keeps the cookie jar coherent and measurably reduces refusals.
pub struct YandexSession {
    pub ocr_sid: String,
    pub text_sid: String,
    pub yu: String,
    pub yum: String,
}

pub static YANDEX_SESSION: Lazy<YandexSession> = Lazy::new(|| YandexSession {
    ocr_sid: generate_sid(),
    text_sid: generate_text_sid(),
    yu: generate_yu(),
    yum: generate_yum(),
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sids_carry_the_expected_service_suffix() {
        assert!(generate_sid().ends_with(".74722d696d616765"));
        assert!(generate_text_sid().ends_with(".74722d74657874"));
    }

    #[test]
    fn yum_is_a_timestamp_followed_by_nine_digits() {
        let yum = generate_yum();
        assert_eq!(yum.len(), 19);
        assert!(yum.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn session_is_stable_within_a_process() {
        assert_eq!(YANDEX_SESSION.yu, YANDEX_SESSION.yu);
    }
}
