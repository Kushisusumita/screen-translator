//! Recent translations, kept for the session only.
//!
//! The design shows a "История · 12" counter. It is deliberately **not**
//! persisted: the history is a record of everything the user pointed this tool
//! at, which is a far more sensitive artefact than a settings file and not
//! something to leave on disk by default.

use std::collections::VecDeque;

use crate::entities::language::Language;
use crate::entities::settings::EngineKind;

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub original: String,
    pub translated: String,
    pub source: Language,
    pub target: Language,
    pub engine: EngineKind,
}

#[derive(Debug, Default)]
pub struct History {
    entries: VecDeque<HistoryEntry>,
    limit: usize,
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            limit: limit.max(1),
        }
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(1);
        self.trim();
    }

    pub fn push(&mut self, entry: HistoryEntry) {
        // Re-translating the same thing should not fill the list with copies.
        if self
            .entries
            .front()
            .is_some_and(|e| e.original == entry.original && e.target == entry.target)
        {
            self.entries.pop_front();
        }
        self.entries.push_front(entry);
        self.trim();
    }

    fn trim(&mut self) {
        while self.entries.len() > self.limit {
            self.entries.pop_back();
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter()
    }

    pub fn latest(&self) -> Option<&HistoryEntry> {
        self.entries.front()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(original: &str) -> HistoryEntry {
        HistoryEntry {
            original: original.into(),
            translated: format!("<{original}>"),
            source: Language::En,
            target: Language::Ru,
            engine: EngineKind::Google,
        }
    }

    #[test]
    fn newest_first() {
        let mut h = History::new(10);
        h.push(entry("one"));
        h.push(entry("two"));
        assert_eq!(h.latest().unwrap().original, "two");
    }

    #[test]
    fn the_limit_evicts_the_oldest() {
        let mut h = History::new(2);
        h.push(entry("a"));
        h.push(entry("b"));
        h.push(entry("c"));
        assert_eq!(h.len(), 2);
        let kept: Vec<&str> = h.iter().map(|e| e.original.as_str()).collect();
        assert_eq!(kept, vec!["c", "b"]);
    }

    #[test]
    fn repeating_the_same_capture_does_not_duplicate_it() {
        let mut h = History::new(10);
        h.push(entry("same"));
        h.push(entry("same"));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn the_same_text_to_a_different_language_is_a_separate_entry() {
        let mut h = History::new(10);
        h.push(entry("same"));
        let mut other = entry("same");
        other.target = Language::De;
        h.push(other);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn lowering_the_limit_trims_immediately() {
        let mut h = History::new(10);
        for i in 0..5 {
            h.push(entry(&format!("e{i}")));
        }
        h.set_limit(2);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn a_zero_limit_is_treated_as_one_rather_than_panicking() {
        let mut h = History::new(0);
        h.push(entry("a"));
        assert_eq!(h.len(), 1);
    }
}
