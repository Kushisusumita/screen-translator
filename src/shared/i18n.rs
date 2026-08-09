//! Interface language.
//!
//! Every user-facing string in the app is written in English at the call site
//! and passed through [`t`]. The English text *is* the lookup key, which means
//! three things: the code reads as prose, a missing translation degrades to
//! English rather than to a blank or a `settings.page.title` placeholder, and
//! there is no English table to keep in step with the code.
//!
//! The tables live in `i18n/`, one file per language, sorted by key so a lookup
//! is a binary search. Nothing is allocated: every string is `&'static str`.
//!
//! The language is global mutable state, which is unusual for this codebase.
//! The alternative — threading a `&Lang` into every widget, icon and error
//! constructor — would touch every signature in the project to express something
//! that genuinely is one per process.

use std::sync::atomic::{AtomicU8, Ordering};

#[path = "i18n/tables.rs"]
mod tables;

/// Languages the interface is translated into.
///
/// Ordered the way the settings list shows them: English first as the source
/// language, then the rest alphabetically by their own name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    En,
    De,
    Es,
    Fr,
    It,
    Ja,
    Kk,
    Ko,
    Pl,
    Pt,
    Ru,
    Tr,
    Uk,
    Zh,
}

impl Lang {
    pub const ALL: [Lang; 14] = [
        Lang::En,
        Lang::De,
        Lang::Es,
        Lang::Fr,
        Lang::It,
        Lang::Ja,
        Lang::Kk,
        Lang::Ko,
        Lang::Pl,
        Lang::Pt,
        Lang::Ru,
        Lang::Tr,
        Lang::Uk,
        Lang::Zh,
    ];

    /// What this language calls itself. A language list that names languages in
    /// the *current* interface language is no use to someone who cannot read the
    /// current interface language — which is precisely who is looking at it.
    pub const fn endonym(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::De => "Deutsch",
            Lang::Es => "Español",
            Lang::Fr => "Français",
            Lang::It => "Italiano",
            Lang::Ja => "日本語",
            Lang::Kk => "Қазақша",
            Lang::Ko => "한국어",
            Lang::Pl => "Polski",
            Lang::Pt => "Português",
            Lang::Ru => "Русский",
            Lang::Tr => "Türkçe",
            Lang::Uk => "Українська",
            Lang::Zh => "中文",
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::De => "de",
            Lang::Es => "es",
            Lang::Fr => "fr",
            Lang::It => "it",
            Lang::Ja => "ja",
            Lang::Kk => "kk",
            Lang::Ko => "ko",
            Lang::Pl => "pl",
            Lang::Pt => "pt",
            Lang::Ru => "ru",
            Lang::Tr => "tr",
            Lang::Uk => "uk",
            Lang::Zh => "zh",
        }
    }

    /// Matches a BCP-47-ish tag from the OS: `ru`, `ru_RU`, `ru-RU.UTF-8`,
    /// `zh-Hans-CN` all land on the same place.
    pub fn from_tag(tag: &str) -> Option<Lang> {
        let primary = tag
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        Lang::ALL.into_iter().find(|l| l.code() == primary)
    }

}

/// Current language, as a `Lang` discriminant. Read on every string lookup, so
/// it is an atomic rather than a lock.
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    CURRENT.store(index_of(lang) as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    Lang::ALL[CURRENT.load(Ordering::Relaxed) as usize % Lang::ALL.len()]
}

fn index_of(lang: Lang) -> usize {
    Lang::ALL.iter().position(|&l| l == lang).unwrap_or(0)
}

/// Translates one interface string.
///
/// `key` is the English text. An unknown key returns itself, so a string added
/// to the code but not yet to the tables shows in English instead of breaking.
pub fn t(key: &'static str) -> &'static str {
    let table = tables::for_lang(current());
    match table.binary_search_by_key(&key, |(k, _)| k) {
        Ok(i) => table[i].1,
        Err(_) => key,
    }
}

/// The language the OS is set to, if the app speaks it.
///
/// Read from the environment, which is where every desktop puts it: `LANG` and
/// `LC_ALL` on macOS and Linux, and on Windows the same variables when they are
/// set, falling back to the user's UI language.
pub fn detect_system() -> Option<Lang> {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Ok(value) = std::env::var(var) {
            // `LANGUAGE` may carry a colon-separated preference list.
            for tag in value.split(':') {
                if let Some(lang) = Lang::from_tag(tag) {
                    return Some(lang);
                }
            }
        }
    }

    #[cfg(windows)]
    {
        if let Some(lang) = windows_ui_language() {
            return Some(lang);
        }
    }

    None
}

#[cfg(windows)]
fn windows_ui_language() -> Option<Lang> {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;

    let mut buffer = [0u16; 85];
    let written = unsafe { GetUserDefaultLocaleName(&mut buffer) };
    if written <= 0 {
        return None;
    }
    let tag = String::from_utf16_lossy(&buffer[..(written as usize).saturating_sub(1)]);
    Lang::from_tag(&tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locale_tag_of_any_shape_finds_its_language() {
        for tag in ["ru", "ru_RU", "ru-RU.UTF-8", "ru@euro", "RU"] {
            assert_eq!(Lang::from_tag(tag), Some(Lang::Ru), "tag {tag}");
        }
        assert_eq!(Lang::from_tag("zh-Hans-CN"), Some(Lang::Zh));
        assert_eq!(Lang::from_tag("pt-BR"), Some(Lang::Pt));
    }

    #[test]
    fn a_language_we_do_not_speak_is_not_guessed_at() {
        assert_eq!(Lang::from_tag("sv_SE"), None);
        assert_eq!(Lang::from_tag(""), None);
        assert_eq!(Lang::from_tag("C"), None);
    }

    #[test]
    fn every_table_is_sorted_and_free_of_duplicates() {
        // The lookup is a binary search; an unsorted table silently returns the
        // English fallback for whatever it fails to find.
        for lang in Lang::ALL {
            let table = tables::for_lang(lang);
            for pair in table.windows(2) {
                assert!(
                    pair[0].0 < pair[1].0,
                    "{}: {:?} is not before {:?}",
                    lang.code(),
                    pair[0].0,
                    pair[1].0
                );
            }
        }
    }

    #[test]
    fn no_translation_is_left_empty() {
        for lang in Lang::ALL {
            for (key, value) in tables::for_lang(lang) {
                assert!(!value.trim().is_empty(), "{} has no text for {key:?}", lang.code());
            }
        }
    }

    #[test]
    fn every_language_covers_the_same_keys() {
        // A key present in one table and missing from another shows up as a
        // single English string in an otherwise translated window, which reads
        // as a bug rather than as a fallback.
        let reference: Vec<&str> = tables::for_lang(Lang::Ru).iter().map(|(k, _)| *k).collect();
        assert!(!reference.is_empty(), "the Russian table is the reference");

        for lang in Lang::ALL {
            if lang == Lang::En {
                continue;
            }
            let keys: Vec<&str> = tables::for_lang(lang).iter().map(|(k, _)| *k).collect();
            assert_eq!(
                keys.len(),
                reference.len(),
                "{} has {} keys against {}",
                lang.code(),
                keys.len(),
                reference.len()
            );
            for key in &reference {
                assert!(keys.contains(key), "{} is missing {key:?}", lang.code());
            }
        }
    }

    #[test]
    fn no_translation_loses_a_placeholder() {
        // `t("…{version}…").replace("{version}", …)` is how these are filled in,
        // so a translation that drops or renames a placeholder prints a literal
        // brace at the user instead of the value.
        fn placeholders(s: &str) -> Vec<&str> {
            let mut found: Vec<&str> = s
                .match_indices('{')
                .filter_map(|(i, _)| s[i..].find('}').map(|j| &s[i..=i + j]))
                .collect();
            found.sort_unstable();
            found
        }

        for lang in Lang::ALL {
            for (key, value) in tables::for_lang(lang) {
                assert_eq!(
                    placeholders(key),
                    placeholders(value),
                    "{} changed the placeholders in {key:?}",
                    lang.code()
                );
            }
        }
    }

    #[test]
    fn an_unknown_key_falls_back_to_its_own_english() {
        set(Lang::Ru);
        assert_eq!(t("Not a key in any table"), "Not a key in any table");
        set(Lang::En);
    }

    #[test]
    fn english_is_the_identity() {
        set(Lang::En);
        assert_eq!(t("Settings"), "Settings");
    }

    #[test]
    fn every_language_translates_a_key_it_has() {
        set(Lang::Ru);
        // Whatever the tables end up containing, this must not be the English.
        if let Some((key, value)) = tables::for_lang(Lang::Ru).first() {
            assert_eq!(t(key), *value);
        }
        set(Lang::En);
    }
}
