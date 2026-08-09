//! The translation tables, one module per language.

use super::Lang;

#[path = "de.rs"]
mod de;
#[path = "es.rs"]
mod es;
#[path = "fr.rs"]
mod fr;
#[path = "it.rs"]
mod it;
#[path = "ja.rs"]
mod ja;
#[path = "kk.rs"]
mod kk;
#[path = "ko.rs"]
mod ko;
#[path = "pl.rs"]
mod pl;
#[path = "pt.rs"]
mod pt;
#[path = "ru.rs"]
mod ru;
#[path = "tr.rs"]
mod tr;
#[path = "uk.rs"]
mod uk;
#[path = "zh.rs"]
mod zh;

/// English is the source language: its "table" is the keys themselves, so there
/// is nothing to store and nothing that can fall out of step with the code.
static EMPTY: &[(&str, &str)] = &[];

pub fn for_lang(lang: Lang) -> &'static [(&'static str, &'static str)] {
    match lang {
        Lang::En => EMPTY,
        Lang::De => de::ENTRIES,
        Lang::Es => es::ENTRIES,
        Lang::Fr => fr::ENTRIES,
        Lang::It => it::ENTRIES,
        Lang::Ja => ja::ENTRIES,
        Lang::Kk => kk::ENTRIES,
        Lang::Ko => ko::ENTRIES,
        Lang::Pl => pl::ENTRIES,
        Lang::Pt => pt::ENTRIES,
        Lang::Ru => ru::ENTRIES,
        Lang::Tr => tr::ENTRIES,
        Lang::Uk => uk::ENTRIES,
        Lang::Zh => zh::ENTRIES,
    }
}
