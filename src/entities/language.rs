use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    En,
    Ru,
    De,
    Fr,
    Es,
    Zh,
    Ja,
    Ko,
    Ar,
    Pt,
    It,
    Tr,
}

impl Language {
    pub fn code(&self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Ru => "ru",
            Language::De => "de",
            Language::Fr => "fr",
            Language::Es => "es",
            Language::Zh => "zh",
            Language::Ja => "ja",
            Language::Ko => "ko",
            Language::Ar => "ar",
            Language::Pt => "pt",
            Language::It => "it",
            Language::Tr => "tr",
        }
    }

    pub fn all() -> &'static [Language] {
        &[
            Language::En,
            Language::Ru,
            Language::De,
            Language::Fr,
            Language::Es,
            Language::Zh,
            Language::Ja,
            Language::Ko,
            Language::Ar,
            Language::Pt,
            Language::It,
            Language::Tr,
        ]
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Language::En => "English",
            Language::Ru => "Russian",
            Language::De => "German",
            Language::Fr => "French",
            Language::Es => "Spanish",
            Language::Zh => "Chinese",
            Language::Ja => "Japanese",
            Language::Ko => "Korean",
            Language::Ar => "Arabic",
            Language::Pt => "Portuguese",
            Language::It => "Italian",
            Language::Tr => "Turkish",
        };
        write!(f, "{}", s)
    }
}

impl Default for Language {
    fn default() -> Self {
        Language::En
    }
}
