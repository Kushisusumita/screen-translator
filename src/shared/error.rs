use std::fmt;

use crate::shared::i18n::t;

#[derive(Debug)]
pub enum AppError {
    Io(std::io::Error),
    Reqwest(reqwest::Error),
    Json(serde_json::Error),
    Toml(toml::de::Error),
    TomlSer(toml::ser::Error),
    #[cfg(windows)]
    Windows(windows::core::Error),
    Image(image::ImageError),
    Clipboard(arboard::Error),
    Other(String),
    /// The capture contained no text. Not a failure — the user pointed at a
    /// picture, or at nothing — so it is carried separately from the errors
    /// and shown in a different voice.
    NoText,
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Io(e) => write!(f, "{}: {e}", t("IO error")),
            AppError::Reqwest(e) => write!(f, "{}: {e}", t("HTTP error")),
            AppError::Json(e) => write!(f, "{}: {e}", t("JSON error")),
            AppError::Toml(e) => write!(f, "{}: {e}", t("TOML parse error")),
            AppError::TomlSer(e) => write!(f, "{}: {e}", t("TOML serialize error")),
            #[cfg(windows)]
            AppError::Windows(e) => write!(f, "{}: {e}", t("Windows API error")),
            AppError::Image(e) => write!(f, "{}: {e}", t("Image error")),
            AppError::Clipboard(e) => write!(f, "{}: {e}", t("Clipboard error")),
            AppError::Other(s) => write!(f, "{}: {s}", t("Error")),
            AppError::NoText => write!(f, "no text in the capture"),
        }
    }
}

impl std::error::Error for AppError {}

impl AppError {
    /// What the user is told.
    ///
    /// `Display` is for the log: it carries the URL, the status line and the
    /// library's own wording, which is what you want when reading a log file and
    /// exactly what you do not want on screen. A translation failing is not an
    /// occasion to show someone a request URL with a session id in it.
    pub fn user_message(&self) -> String {
        match self {
            AppError::Reqwest(e) => {
                if e.is_timeout() {
                    t("The service did not answer in time").to_string()
                } else if e.is_connect() || e.is_request() {
                    t("No connection to the service").to_string()
                } else if let Some(status) = e.status() {
                    if status.is_server_error() {
                        t("The service is having trouble — try again in a moment").to_string()
                    } else {
                        t("The service refused the request").to_string()
                    }
                } else {
                    t("Network error").to_string()
                }
            }
            AppError::Json(_) => t("The service sent something unreadable").to_string(),
            AppError::Io(_) => t("A file could not be read or written").to_string(),
            AppError::Toml(_) | AppError::TomlSer(_) => {
                t("The settings file is damaged").to_string()
            }
            #[cfg(windows)]
            AppError::Windows(_) => t("The system refused the operation").to_string(),
            AppError::Image(_) => t("The captured image could not be processed").to_string(),
            AppError::Clipboard(_) => t("The clipboard is not available").to_string(),
            // These are written by hand at the point they are raised, in the
            // words the user should see.
            AppError::Other(s) => s.clone(),
            AppError::NoText => t("No text found in the selected area.").to_string(),
        }
    }
}

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

#[cfg(windows)]
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
