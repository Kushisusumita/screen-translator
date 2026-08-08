//! A small in-memory translation cache.
//!
//! Re-capturing the same subtitle, the same dialog box or the same paragraph a
//! second time is the normal way this tool gets used. Without a cache that is a
//! second round trip, a second bill for anyone on a paid engine, and a second
//! wait. Entries live for the process only — nothing about a user's screen is
//! written to disk.

use std::collections::VecDeque;
use std::sync::Mutex;

use once_cell::sync::Lazy;

use crate::entities::language::Language;
use crate::entities::settings::EngineKind;

const CAPACITY: usize = 96;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    text: String,
    source: Language,
    target: Language,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub text: String,
    pub engine: EngineKind,
}

struct Entry {
    key: Key,
    value: Hit,
}

/// The cache itself. Instantiable so tests get their own rather than fighting
/// over a global.
pub struct TranslationCache {
    entries: VecDeque<Entry>,
    capacity: usize,
}

impl TranslationCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    pub fn get(&mut self, text: &str, source: Language, target: Language) -> Option<Hit> {
        let key = Key {
            text: text.to_string(),
            source,
            target,
        };
        let idx = self.entries.iter().position(|e| e.key == key)?;
        // Touch: move to the front so eviction order is genuinely least-recently-used.
        let entry = self.entries.remove(idx)?;
        let hit = entry.value.clone();
        self.entries.push_front(entry);
        Some(hit)
    }

    pub fn put(&mut self, text: &str, source: Language, target: Language, value: Hit) {
        let key = Key {
            text: text.to_string(),
            source,
            target,
        };
        if let Some(idx) = self.entries.iter().position(|e| e.key == key) {
            self.entries.remove(idx);
        }
        self.entries.push_front(Entry { key, value });
        while self.entries.len() > self.capacity {
            self.entries.pop_back();
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// ── Process-wide instance ────────────────────────────────────────────────────

static CACHE: Lazy<Mutex<TranslationCache>> =
    Lazy::new(|| Mutex::new(TranslationCache::new(CAPACITY)));

/// A poisoned cache mutex must never take the app down with it — recovering the
/// inner value is safe because the guarded type is a plain container.
fn lock() -> std::sync::MutexGuard<'static, TranslationCache> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn get(text: &str, source: Language, target: Language) -> Option<Hit> {
    lock().get(text, source, target)
}

pub fn put(text: &str, source: Language, target: Language, value: Hit) {
    lock().put(text, source, target, value);
}

/// Called when the engine configuration changes: a translation produced by the
/// engine the user just switched away from is no longer the answer they want.
pub fn clear() {
    lock().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(s: &str) -> Hit {
        Hit {
            text: s.into(),
            engine: EngineKind::Google,
        }
    }

    #[test]
    fn stores_and_returns() {
        let mut c = TranslationCache::new(8);
        c.put("hello", Language::En, Language::Ru, hit("привет"));
        assert_eq!(
            c.get("hello", Language::En, Language::Ru).unwrap().text,
            "привет"
        );
    }

    #[test]
    fn a_miss_is_none() {
        let mut c = TranslationCache::new(8);
        assert!(c.get("nothing", Language::En, Language::Ru).is_none());
    }

    #[test]
    fn the_language_pair_is_part_of_the_key() {
        let mut c = TranslationCache::new(8);
        c.put("hello", Language::En, Language::Ru, hit("привет"));
        assert!(c.get("hello", Language::En, Language::De).is_none());
        assert!(c.get("hello", Language::Auto, Language::Ru).is_none());
    }

    #[test]
    fn re_putting_a_key_replaces_rather_than_duplicates() {
        let mut c = TranslationCache::new(8);
        c.put("x", Language::En, Language::Ru, hit("один"));
        c.put("x", Language::En, Language::Ru, hit("два"));
        assert_eq!(c.len(), 1);
        assert_eq!(c.get("x", Language::En, Language::Ru).unwrap().text, "два");
    }

    #[test]
    fn eviction_drops_the_least_recently_used() {
        let mut c = TranslationCache::new(4);
        for i in 0..4 {
            c.put(&format!("k{i}"), Language::En, Language::Ru, hit("v"));
        }
        // Touch the oldest so it is no longer the eviction candidate.
        assert!(c.get("k0", Language::En, Language::Ru).is_some());
        c.put("overflow", Language::En, Language::Ru, hit("v"));

        assert!(c.get("k0", Language::En, Language::Ru).is_some());
        assert!(
            c.get("k1", Language::En, Language::Ru).is_none(),
            "k1 was the least recently used and should have been evicted"
        );
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn capacity_is_never_zero() {
        let mut c = TranslationCache::new(0);
        c.put("a", Language::En, Language::Ru, hit("б"));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn clear_empties_it() {
        let mut c = TranslationCache::new(8);
        c.put("a", Language::En, Language::Ru, hit("б"));
        c.clear();
        assert_eq!(c.len(), 0);
    }

    #[test]
    fn the_shared_instance_is_reachable() {
        // Exercises the free functions without asserting on contents, which
        // other tests in this process may also be touching.
        put("shared-probe", Language::En, Language::Ru, hit("v"));
        assert!(get("shared-probe", Language::En, Language::Ru).is_some());
    }
}
