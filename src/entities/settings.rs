use serde::{Deserialize, Serialize};
use crate::entities::language::Language;

// Windows virtual key codes
pub const VK_T: u32 = 0x54;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_NOREPEAT: u32 = 0x4000;

fn default_true() -> bool { true }

/// How the translation result is displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TooltipMode {
    /// Full-screen overlay — frozen desktop screenshot + semi-transparent tint,
    /// text top-left. Click anywhere to dismiss.
    #[default]
    Overlay,
    /// Compact native Win32 hint that appears just below the selected region,
    /// without covering the whole screen.
    Native,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub source_lang: Language,
    pub target_lang: Language,
    pub hotkey_modifiers: u32,
    pub hotkey_key: u32,
    pub hotkey_display: String,
    pub launch_at_startup: bool,
    pub copy_to_clipboard: bool,
    #[serde(default = "default_true")]
    pub use_yandex: bool,
    #[serde(default = "default_true")]
    pub use_google: bool,
    #[serde(default)]
    pub tooltip_mode: TooltipMode,
    /// Whether to show the translation result on screen (overlay or native hint).
    /// When false the result is only written to the clipboard (if that option is on).
    #[serde(default = "default_true")]
    pub show_translation: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source_lang: Language::En,
            target_lang: Language::Ru,
            hotkey_modifiers: MOD_CONTROL | MOD_NOREPEAT,
            hotkey_key: VK_T,
            hotkey_display: "Ctrl+T".to_string(),
            launch_at_startup: false,
            copy_to_clipboard: false,
            use_yandex: true,
            use_google: true,
            tooltip_mode: TooltipMode::Overlay,
            show_translation: true,
        }
    }
}
