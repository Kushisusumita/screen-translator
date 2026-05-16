use once_cell::sync::Lazy;
use rand::Rng;
use std::time::{SystemTime, UNIX_EPOCH};

pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:150.0) Gecko/20100101 Firefox/150.0";

/// One shared HTTP client so OCR and Translate share the same cookie jar.
pub static HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .cookie_store(true)
        .build()
        .expect("Failed to build HTTP client")
});

/// Session ID for OCR requests — last segment is "tr-image" in hex.
pub fn generate_sid() -> String {
    let mut rng = rand::thread_rng();
    let a: u32 = rng.gen();
    let b: u32 = rng.gen();
    let c: u32 = rng.gen();
    format!("{:08x}.{:08x}.{:08x}.74722d696d616765", a, b, c)
}

/// `yu` — 19-digit Yandex user identifier (persistent across sessions ideally,
/// but a random 19-digit number is accepted by the API).
pub fn generate_yu() -> String {
    let mut rng = rand::thread_rng();
    // u64 max is 18_446_744_073_709_551_615 (20 digits), so 19 fits comfortably.
    let n: u64 = rng.gen_range(1_000_000_000_000_000_000..9_999_999_999_999_999_999);
    n.to_string()
}

/// `yum` — 19-digit value: Unix timestamp in seconds (10 digits) + 9 random digits.
/// Matches the pattern Yandex uses: e.g. `1778922981` + `810218011`.
pub fn generate_yum() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let suffix: u32 = rand::thread_rng().gen_range(100_000_000..999_999_999);
    format!("{}{}", ts, suffix)
}
