use std::fmt;

#[derive(Debug)]
pub enum AppError {
    Io(std::io::Error),
    Reqwest(reqwest::Error),
    Json(serde_json::Error),
    Toml(toml::de::Error),
    TomlSer(toml::ser::Error),
    Windows(windows::core::Error),
    Image(image::ImageError),
    Clipboard(arboard::Error),
    Other(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Io(e) => write!(f, "IO error: {}", e),
            AppError::Reqwest(e) => write!(f, "HTTP error: {}", e),
            AppError::Json(e) => write!(f, "JSON error: {}", e),
            AppError::Toml(e) => write!(f, "TOML parse error: {}", e),
            AppError::TomlSer(e) => write!(f, "TOML serialize error: {}", e),
            AppError::Windows(e) => write!(f, "Windows API error: {}", e),
            AppError::Image(e) => write!(f, "Image error: {}", e),
            AppError::Clipboard(e) => write!(f, "Clipboard error: {}", e),
            AppError::Other(s) => write!(f, "Error: {}", s),
        }
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e)
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError::Reqwest(e)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Json(e)
    }
}

impl From<toml::de::Error> for AppError {
    fn from(e: toml::de::Error) -> Self {
        AppError::Toml(e)
    }
}

impl From<toml::ser::Error> for AppError {
    fn from(e: toml::ser::Error) -> Self {
        AppError::TomlSer(e)
    }
}

impl From<windows::core::Error> for AppError {
    fn from(e: windows::core::Error) -> Self {
        AppError::Windows(e)
    }
}

impl From<image::ImageError> for AppError {
    fn from(e: image::ImageError) -> Self {
        AppError::Image(e)
    }
}

impl From<arboard::Error> for AppError {
    fn from(e: arboard::Error) -> Self {
        AppError::Clipboard(e)
    }
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        AppError::Other(s)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        AppError::Other(s.to_string())
    }
}
