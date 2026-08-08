//! Carrying settings over from the pre-Sakura format.
//!
//! The 0.1.0 config was flat: a single hotkey, two engine booleans, and a
//! `tooltip_mode`. The new one groups those into `hotkeys`, `engines` and
//! `result_view`. Since the root struct is `#[serde(default)]`, an old file
//! parses *successfully* into all-defaults — which would silently discard a
//! customised hotkey or a Google-only setup without anyone noticing.
//!
//! So the old keys are read explicitly and mapped across.

use serde::Deserialize;
use tracing::info;

use crate::entities::settings::{Hotkey, ResultView, Settings};

/// Only the fields that moved or were renamed. Everything whose name and type
/// survived (`source_lang`, `copy_to_clipboard`, …) is picked up by the normal
/// parse and needs nothing here.
#[derive(Debug, Default, Deserialize)]
pub struct LegacySettings {
    pub hotkey_modifiers: Option<u32>,
    pub hotkey_key: Option<u32>,
    pub use_yandex: Option<bool>,
    pub use_google: Option<bool>,
    pub tooltip_mode: Option<String>,
    pub show_translation: Option<bool>,
}

impl LegacySettings {
    pub fn is_present(&self) -> bool {
        self.hotkey_key.is_some()
            || self.hotkey_modifiers.is_some()
            || self.use_yandex.is_some()
            || self.use_google.is_some()
            || self.tooltip_mode.is_some()
            || self.show_translation.is_some()
    }
}

/// Applies old values on top of an already-parsed `Settings`. Returns true when
/// anything was carried over.
pub fn apply(settings: &mut Settings, legacy: &LegacySettings) -> bool {
    if !legacy.is_present() {
        return false;
    }

    if let (Some(modifiers), Some(key)) = (legacy.hotkey_modifiers, legacy.hotkey_key) {
        if key != 0 {
            settings.hotkeys.region = Hotkey {
                modifiers,
                key,
                enabled: true,
            };
        }
    }

    if let Some(on) = legacy.use_yandex {
        settings.engines.yandex = on;
    }
    if let Some(on) = legacy.use_google {
        settings.engines.google = on;
    }

    // The old pair of switches collapses into one choice. `show_translation`
    // was the master toggle, so it decides first.
    settings.result_view = match legacy.show_translation {
        Some(false) => ResultView::None,
        // Both old modes put the translation on screen near or over the
        // capture; the popup is the closest thing in the new set to what these
        // users already had. A fresh install starts on the inline view instead.
        _ if legacy.tooltip_mode.is_some() => ResultView::Popup,
        _ => settings.result_view,
    };

    info!(
        hotkey = %settings.hotkeys.region.display(),
        yandex = settings.engines.yandex,
        google = settings.engines.google,
        result_view = settings.result_view.label(),
        "Migrated settings from the previous version"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::language::Language;
    use crate::entities::settings::{MOD_ALT, MOD_NOREPEAT, MOD_SHIFT};

    /// Exactly what version 0.1.0 wrote.
    const OLD_CONFIG: &str = r#"
source_lang = "En"
target_lang = "Ru"
hotkey_modifiers = 16386
hotkey_key = 84
hotkey_display = "Ctrl+T"
launch_at_startup = false
copy_to_clipboard = false
use_yandex = true
use_google = true
tooltip_mode = "Native"
show_translation = true
"#;

    fn load(text: &str) -> Settings {
        let mut s: Settings = toml::from_str(text).expect("new parser accepts the old file");
        let legacy: LegacySettings = toml::from_str(text).expect("legacy view parses");
        apply(&mut s, &legacy);
        s
    }

    #[test]
    fn a_real_v0_config_keeps_its_hotkey() {
        let s = load(OLD_CONFIG);
        assert_eq!(s.hotkeys.region.key, 0x54);
        assert_eq!(s.hotkeys.region.modifiers, 16386);
        assert!(s.hotkeys.region.is_bound());
    }

    #[test]
    fn fields_that_did_not_move_are_still_read() {
        let s = load(OLD_CONFIG);
        assert_eq!(s.source_lang, Language::En);
        assert_eq!(s.target_lang, Language::Ru);
    }

    #[test]
    fn a_custom_hotkey_survives() {
        let text = format!(
            "hotkey_modifiers = {}\nhotkey_key = 0x57\n",
            MOD_ALT | MOD_SHIFT | MOD_NOREPEAT
        );
        let s = load(&text);
        assert_eq!(s.hotkeys.region.key, 0x57);
        assert_eq!(s.hotkeys.region.modifiers & MOD_ALT, MOD_ALT);
        assert_eq!(s.hotkeys.region.modifiers & MOD_SHIFT, MOD_SHIFT);
        // How it is spelled is the platform's business — `Alt+Shift+W` on
        // Windows, `⌥⇧W` on macOS — so the migration is checked against the
        // same source of truth rather than against one platform's wording.
        assert_eq!(
            s.hotkeys.region.display(),
            crate::ui::Platform::current().format_hotkey(MOD_ALT | MOD_SHIFT, "W")
        );
    }

    #[test]
    fn a_google_only_setup_is_not_silently_re_enabled_for_yandex() {
        let s = load("use_yandex = false\nuse_google = true\n");
        assert!(!s.engines.yandex);
        assert!(s.engines.google);
    }

    #[test]
    fn clipboard_only_users_keep_clipboard_only() {
        let s = load("show_translation = false\ncopy_to_clipboard = true\n");
        assert_eq!(s.result_view, ResultView::None);
        assert!(s.copy_to_clipboard);
    }

    #[test]
    fn both_old_display_modes_land_on_the_popup() {
        for mode in ["Overlay", "Native"] {
            let s = load(&format!(
                "tooltip_mode = \"{mode}\"\nshow_translation = true\n"
            ));
            assert_eq!(s.result_view, ResultView::Popup, "mode {mode}");
        }
    }

    #[test]
    fn a_new_format_file_is_left_alone() {
        let mut s = Settings {
            result_view: ResultView::Window,
            ..Default::default()
        };
        let before = s.clone();

        let legacy = LegacySettings::default();
        assert!(!apply(&mut s, &legacy));
        assert_eq!(s.result_view, before.result_view);
        assert_eq!(s.hotkeys.region.key, before.hotkeys.region.key);
    }

    #[test]
    fn a_zero_key_is_not_treated_as_a_binding() {
        let s = load("hotkey_modifiers = 2\nhotkey_key = 0\n");
        // Falls back to the default rather than binding "modifier plus nothing".
        assert_eq!(s.hotkeys.region.key, crate::entities::settings::VK_T);
    }
}
