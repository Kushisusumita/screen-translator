use serde::{Deserialize, Serialize};
use std::fmt;

/// A translation language, plus `Auto` for "let the OCR decide".
///
/// `Auto` is only meaningful as a *source*. Guarding that in the type would mean
/// two enums and a conversion at every call site; instead `code()` returns
/// `"auto"` and each provider maps it to whatever that API calls automatic
/// detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Language {
    Auto,
    #[default]
    En,
    Ru,
    Uk,
    De,
    Fr,
    Es,
    It,
    Pt,
    Pl,
    Nl,
    Tr,
    Cs,
    Sv,
    El,
    Ro,
    Hu,
    Fi,
    Da,
    Bg,
    Sr,
    Kk,
    He,
    Ar,
    Fa,
    Hi,
    Th,
    Vi,
    Id,
    Zh,
    Ja,
    Ko,
}

impl Language {
    /// ISO 639-1, or `auto`.
    pub fn code(self) -> &'static str {
        use Language::*;
        match self {
            Auto => "auto",
            En => "en",
            Ru => "ru",
            Uk => "uk",
            De => "de",
            Fr => "fr",
            Es => "es",
            It => "it",
            Pt => "pt",
            Pl => "pl",
            Nl => "nl",
            Tr => "tr",
            Cs => "cs",
            Sv => "sv",
            El => "el",
            Ro => "ro",
            Hu => "hu",
            Fi => "fi",
            Da => "da",
            Bg => "bg",
            Sr => "sr",
            Kk => "kk",
            He => "he",
            Ar => "ar",
            Fa => "fa",
            Hi => "hi",
            Th => "th",
            Vi => "vi",
            Id => "id",
            Zh => "zh",
            Ja => "ja",
            Ko => "ko",
        }
    }

    /// Two-letter badge for the result popup: `EN`, `RU`, `АВТО`.
    pub fn badge(self) -> &'static str {
        match self {
            Language::Auto => "АВТО",
            other => match other.code() {
                "en" => "EN",
                "ru" => "RU",
                "uk" => "UK",
                "de" => "DE",
                "fr" => "FR",
                "es" => "ES",
                "it" => "IT",
                "pt" => "PT",
                "pl" => "PL",
                "nl" => "NL",
                "tr" => "TR",
                "cs" => "CS",
                "sv" => "SV",
                "el" => "EL",
                "ro" => "RO",
                "hu" => "HU",
                "fi" => "FI",
                "da" => "DA",
                "bg" => "BG",
                "sr" => "SR",
                "kk" => "KK",
                "he" => "HE",
                "ar" => "AR",
                "fa" => "FA",
                "hi" => "HI",
                "th" => "TH",
                "vi" => "VI",
                "id" => "ID",
                "zh" => "ZH",
                "ja" => "JA",
                "ko" => "KO",
                _ => "??",
            },
        }
    }

    /// English name, used when talking to an LLM about the target language.
    pub fn english_name(self) -> &'static str {
        use Language::*;
        match self {
            Auto => "the detected language",
            En => "English",
            Ru => "Russian",
            Uk => "Ukrainian",
            De => "German",
            Fr => "French",
            Es => "Spanish",
            It => "Italian",
            Pt => "Portuguese",
            Pl => "Polish",
            Nl => "Dutch",
            Tr => "Turkish",
            Cs => "Czech",
            Sv => "Swedish",
            El => "Greek",
            Ro => "Romanian",
            Hu => "Hungarian",
            Fi => "Finnish",
            Da => "Danish",
            Bg => "Bulgarian",
            Sr => "Serbian",
            Kk => "Kazakh",
            He => "Hebrew",
            Ar => "Arabic",
            Fa => "Persian",
            Hi => "Hindi",
            Th => "Thai",
            Vi => "Vietnamese",
            Id => "Indonesian",
            Zh => "Chinese",
            Ja => "Japanese",
            Ko => "Korean",
        }
    }

    /// The interface is Russian, so the pickers show Russian names.
    pub fn name_ru(self) -> &'static str {
        use Language::*;
        match self {
            Auto => "Определять автоматически",
            En => "Английский",
            Ru => "Русский",
            Uk => "Украинский",
            De => "Немецкий",
            Fr => "Французский",
            Es => "Испанский",
            It => "Итальянский",
            Pt => "Португальский",
            Pl => "Польский",
            Nl => "Нидерландский",
            Tr => "Турецкий",
            Cs => "Чешский",
            Sv => "Шведский",
            El => "Греческий",
            Ro => "Румынский",
            Hu => "Венгерский",
            Fi => "Финский",
            Da => "Датский",
            Bg => "Болгарский",
            Sr => "Сербский",
            Kk => "Казахский",
            He => "Иврит",
            Ar => "Арабский",
            Fa => "Персидский",
            Hi => "Хинди",
            Th => "Тайский",
            Vi => "Вьетнамский",
            Id => "Индонезийский",
            Zh => "Китайский",
            Ja => "Японский",
            Ko => "Корейский",
        }
    }

    /// Shorter form for a narrow picker, where "Определять автоматически" does
    /// not fit.
    pub fn short_ru(self) -> &'static str {
        match self {
            Language::Auto => "Авто",
            other => other.name_ru(),
        }
    }

    /// Everything, `Auto` first.
    pub fn all() -> &'static [Language] {
        use Language::*;
        &[
            Auto, En, Ru, Uk, De, Fr, Es, It, Pt, Pl, Nl, Tr, Cs, Sv, El, Ro, Hu, Fi, Da, Bg, Sr,
            Kk, He, Ar, Fa, Hi, Th, Vi, Id, Zh, Ja, Ko,
        ]
    }

    /// Everything except `Auto` — a target language has to be concrete.
    pub fn targets() -> impl Iterator<Item = Language> {
        Language::all()
            .iter()
            .copied()
            .filter(|l| *l != Language::Auto)
    }

    /// Maps a detector's answer back onto the enum. Accepts the `zh-CN` style
    /// tags OCR services sometimes return.
    pub fn from_code(code: &str) -> Option<Language> {
        let base = code
            .split(['-', '_'])
            .next()
            .unwrap_or(code)
            .to_ascii_lowercase();
        Language::all()
            .iter()
            .copied()
            .find(|l| l.code() == base && *l != Language::Auto)
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name_ru())
    }
}

#[cfg(test)]
mod tests {
    use super::Language;

    #[test]
    fn codes_are_unique() {
        let mut codes: Vec<&str> = Language::all().iter().map(|l| l.code()).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(before, codes.len(), "duplicate language code");
    }

    #[test]
    fn from_code_handles_regional_tags() {
        assert_eq!(Language::from_code("zh-CN"), Some(Language::Zh));
        assert_eq!(Language::from_code("pt_BR"), Some(Language::Pt));
        assert_eq!(Language::from_code("EN"), Some(Language::En));
    }

    #[test]
    fn from_code_never_returns_auto() {
        assert_eq!(Language::from_code("auto"), None);
        assert_eq!(Language::from_code("*"), None);
    }

    #[test]
    fn every_language_has_a_badge() {
        for l in Language::all() {
            assert_ne!(l.badge(), "??", "{l:?} has no badge");
        }
    }

    #[test]
    fn the_short_form_only_differs_for_auto() {
        assert_eq!(Language::Auto.short_ru(), "Авто");
        assert_eq!(Language::Ru.short_ru(), Language::Ru.name_ru());
    }

    #[test]
    fn targets_exclude_auto() {
        assert!(!Language::targets().any(|l| l == Language::Auto));
    }
}
