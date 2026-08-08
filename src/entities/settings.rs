use serde::{Deserialize, Serialize};

use crate::entities::language::Language;
use crate::shared::secret::Secret;
use crate::ui::theme::ThemeMode;
use crate::ui::Platform;

// Windows virtual-key codes and hotkey modifier bits. On macOS the same numbers
// are carried through and translated at the point of registration, so settings
// files stay portable between platforms.
pub const VK_T: u32 = 0x54;
pub const VK_W: u32 = 0x57;
pub const VK_S: u32 = 0x53;
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
pub const MOD_NOREPEAT: u32 = 0x4000;

fn default_true() -> bool {
    true
}

// ── Hotkeys ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key: u32,
    /// `false` means the action exists but has no shortcut bound.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Hotkey {
    pub const fn new(modifiers: u32, key: u32) -> Self {
        Self {
            modifiers: modifiers | MOD_NOREPEAT,
            key,
            enabled: true,
        }
    }

    pub const fn unbound() -> Self {
        Self {
            modifiers: MOD_NOREPEAT,
            key: 0,
            enabled: false,
        }
    }

    pub fn is_bound(&self) -> bool {
        self.enabled && self.key != 0
    }

    /// `Alt+Shift+T` on Windows, `⌥⇧T` on macOS.
    pub fn display(&self) -> String {
        if !self.is_bound() {
            return "не задано".to_string();
        }
        Platform::current().format_hotkey(self.modifiers, vk_name(self.key))
    }
}

/// Printable name of a virtual-key code.
pub fn vk_name(vk: u32) -> &'static str {
    match vk {
        0x30..=0x39 => match vk {
            0x30 => "0",
            0x31 => "1",
            0x32 => "2",
            0x33 => "3",
            0x34 => "4",
            0x35 => "5",
            0x36 => "6",
            0x37 => "7",
            0x38 => "8",
            _ => "9",
        },
        0x41..=0x5A => LETTERS[(vk - 0x41) as usize],
        0x70..=0x7B => FKEYS[(vk - 0x70) as usize],
        0x20 => "Space",
        0x0D => "Enter",
        0xC0 => "`",
        0xBD => "-",
        0xBB => "=",
        0xDB => "[",
        0xDD => "]",
        0xBA => ";",
        0xDE => "'",
        0xBC => ",",
        0xBE => ".",
        0xBF => "/",
        _ => "?",
    }
}

const LETTERS: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];
const FKEYS: [&str; 12] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkeys {
    #[serde(default = "Hotkeys::default_region")]
    pub region: Hotkey,
    #[serde(default = "Hotkeys::default_window")]
    pub window: Hotkey,
    #[serde(default = "Hotkeys::default_fullscreen")]
    pub fullscreen: Hotkey,
    /// Re-run the last capture without drawing a new rectangle.
    #[serde(default = "Hotkey::unbound")]
    pub repeat: Hotkey,
}

impl Hotkeys {
    fn default_region() -> Hotkey {
        Hotkey::new(MOD_CONTROL, VK_T)
    }
    fn default_window() -> Hotkey {
        Hotkey::new(MOD_CONTROL | MOD_SHIFT, VK_W)
    }
    fn default_fullscreen() -> Hotkey {
        Hotkey::new(MOD_CONTROL | MOD_SHIFT, VK_S)
    }

    pub fn all(&self) -> [(HotkeyAction, Hotkey); 4] {
        [
            (HotkeyAction::Region, self.region),
            (HotkeyAction::Window, self.window),
            (HotkeyAction::FullScreen, self.fullscreen),
            (HotkeyAction::Repeat, self.repeat),
        ]
    }

    pub fn slot_mut(&mut self, action: HotkeyAction) -> &mut Hotkey {
        match action {
            HotkeyAction::Region => &mut self.region,
            HotkeyAction::Window => &mut self.window,
            HotkeyAction::FullScreen => &mut self.fullscreen,
            HotkeyAction::Repeat => &mut self.repeat,
        }
    }

    /// Two actions bound to the same combination means only one of them will
    /// ever fire, so the settings UI flags it.
    pub fn conflicts(&self) -> Vec<(HotkeyAction, HotkeyAction)> {
        let all = self.all();
        let mut out = Vec::new();
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                let (a, ha) = all[i];
                let (b, hb) = all[j];
                if ha.is_bound()
                    && hb.is_bound()
                    && ha.modifiers == hb.modifiers
                    && ha.key == hb.key
                {
                    out.push((a, b));
                }
            }
        }
        out
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            region: Self::default_region(),
            window: Self::default_window(),
            fullscreen: Self::default_fullscreen(),
            repeat: Hotkey::unbound(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HotkeyAction {
    Region,
    Window,
    FullScreen,
    Repeat,
}

impl HotkeyAction {
    pub fn label(self) -> &'static str {
        match self {
            HotkeyAction::Region => "Перевести область",
            HotkeyAction::Window => "Перевести окно",
            HotkeyAction::FullScreen => "Перевести весь экран",
            HotkeyAction::Repeat => "Повторить последний захват",
        }
    }

    pub fn all() -> [HotkeyAction; 4] {
        [
            HotkeyAction::Region,
            HotkeyAction::Window,
            HotkeyAction::FullScreen,
            HotkeyAction::Repeat,
        ]
    }
}

// ── Capture and presentation ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CaptureMode {
    #[default]
    Region,
    Window,
    FullScreen,
}

impl CaptureMode {
    pub fn label(self) -> &'static str {
        match self {
            CaptureMode::Region => "Область",
            CaptureMode::Window => "Окно",
            CaptureMode::FullScreen => "Весь экран",
        }
    }

    /// Longer form for the tray menu, where the label has to say what it does.
    pub fn label_menu(self) -> &'static str {
        match self {
            CaptureMode::Region => "Перевести область",
            CaptureMode::Window => "Перевести окно",
            CaptureMode::FullScreen => "Перевести весь экран",
        }
    }

    pub fn all() -> [CaptureMode; 3] {
        [
            CaptureMode::Region,
            CaptureMode::Window,
            CaptureMode::FullScreen,
        ]
    }

    pub fn next(self) -> Self {
        match self {
            CaptureMode::Region => CaptureMode::Window,
            CaptureMode::Window => CaptureMode::FullScreen,
            CaptureMode::FullScreen => CaptureMode::Region,
        }
    }
}

/// How a finished translation is shown. All three come from the design; the
/// user picks one instead of the design picking for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ResultView {
    /// Glass card anchored to the captured region.
    #[default]
    Popup,
    /// Translation painted over the original, in place.
    Inline,
    /// Free-floating window with original and translation side by side.
    Window,
    /// Nothing on screen — clipboard only.
    None,
}

impl ResultView {
    pub fn label(self) -> &'static str {
        match self {
            ResultView::Popup => "Popup у выделения",
            ResultView::Inline => "Поверх оригинала",
            ResultView::Window => "Отдельное окно",
            ResultView::None => "Не показывать",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ResultView::Popup => "Карточка с оригиналом и переводом рядом с захваченной областью.",
            ResultView::Inline => {
                "Перевод рисуется на месте оригинала — удобно для длинных абзацев."
            }
            ResultView::Window => "Окно с двумя колонками, можно закрепить поверх всех окон.",
            ResultView::None => "Результат только копируется в буфер обмена.",
        }
    }

    pub fn all() -> [ResultView; 4] {
        [
            ResultView::Popup,
            ResultView::Inline,
            ResultView::Window,
            ResultView::None,
        ]
    }
}

// ── Translation engines ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EngineKind {
    Yandex,
    Google,
    DeepL,
    Ai,
}

impl EngineKind {
    pub fn label(self) -> &'static str {
        match self {
            EngineKind::Yandex => "Yandex",
            EngineKind::Google => "Google",
            EngineKind::DeepL => "DeepL",
            EngineKind::Ai => "AI",
        }
    }

    pub fn all() -> [EngineKind; 4] {
        [
            EngineKind::Yandex,
            EngineKind::Google,
            EngineKind::DeepL,
            EngineKind::Ai,
        ]
    }

    /// Whether this engine is useless without a credential.
    pub fn needs_key(self) -> bool {
        matches!(self, EngineKind::DeepL | EngineKind::Ai)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineSettings {
    /// Tried in this order; the first success wins.
    #[serde(default = "EngineSettings::default_order")]
    pub order: Vec<EngineKind>,
    #[serde(default = "default_true")]
    pub yandex: bool,
    #[serde(default = "default_true")]
    pub google: bool,
    #[serde(default)]
    pub deepl: bool,
    #[serde(default)]
    pub ai: bool,
    #[serde(default)]
    pub deepl_key: Secret,
    #[serde(default)]
    pub ai_config: AiConfig,
    /// Last-resort Yandex path that drives a real headless browser. Off by
    /// default: it costs a browser launch and several seconds per translation.
    #[serde(default)]
    pub yandex_headless_fallback: bool,
}

impl EngineSettings {
    fn default_order() -> Vec<EngineKind> {
        vec![
            EngineKind::Ai,
            EngineKind::DeepL,
            EngineKind::Yandex,
            EngineKind::Google,
        ]
    }

    pub fn is_enabled(&self, kind: EngineKind) -> bool {
        match kind {
            EngineKind::Yandex => self.yandex,
            EngineKind::Google => self.google,
            EngineKind::DeepL => self.deepl,
            EngineKind::Ai => self.ai,
        }
    }

    pub fn set_enabled(&mut self, kind: EngineKind, on: bool) {
        match kind {
            EngineKind::Yandex => self.yandex = on,
            EngineKind::Google => self.google = on,
            EngineKind::DeepL => self.deepl = on,
            EngineKind::Ai => self.ai = on,
        }
    }

    /// Enabled engines in priority order, skipping any that are missing the
    /// credentials they need — an engine that cannot possibly answer should not
    /// sit in front of one that can.
    pub fn active(&self) -> Vec<EngineKind> {
        let mut order = self.order.clone();
        for kind in EngineKind::all() {
            if !order.contains(&kind) {
                order.push(kind);
            }
        }
        order
            .into_iter()
            .filter(|k| self.is_enabled(*k) && self.is_configured(*k))
            .collect()
    }

    pub fn is_configured(&self, kind: EngineKind) -> bool {
        match kind {
            EngineKind::Yandex | EngineKind::Google => true,
            EngineKind::DeepL => !self.deepl_key.is_empty(),
            EngineKind::Ai => self.ai_config.is_usable(),
        }
    }

    pub fn status(&self, kind: EngineKind) -> &'static str {
        match kind {
            EngineKind::Yandex | EngineKind::Google => "без ключа",
            EngineKind::DeepL => {
                if self.deepl_key.is_empty() {
                    "нужен ключ"
                } else {
                    "API-ключ"
                }
            }
            EngineKind::Ai => {
                if self.ai_config.is_usable() {
                    "готов"
                } else {
                    "нужен ключ"
                }
            }
        }
    }
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            order: Self::default_order(),
            yandex: true,
            google: true,
            deepl: false,
            ai: false,
            deepl_key: Secret::default(),
            ai_config: AiConfig::default(),
            yandex_headless_fallback: false,
        }
    }
}

// ── Bring-your-own AI ────────────────────────────────────────────────────────

/// Wire format. Nearly every hosted model speaks one of these three, so a base
/// URL plus a protocol covers "any AI with a token" without a provider list
/// that goes stale every month.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AiProtocol {
    /// `POST {base}/chat/completions`, `Authorization: Bearer`.
    #[default]
    OpenAi,
    /// `POST {base}/messages`, `x-api-key` + `anthropic-version`.
    Anthropic,
    /// `POST {base}/models/{model}:generateContent?key=`.
    Gemini,
}

impl AiProtocol {
    pub fn label(self) -> &'static str {
        match self {
            AiProtocol::OpenAi => "OpenAI-совместимый",
            AiProtocol::Anthropic => "Anthropic Messages",
            AiProtocol::Gemini => "Google Gemini",
        }
    }

    pub fn all() -> [AiProtocol; 3] {
        [
            AiProtocol::OpenAi,
            AiProtocol::Anthropic,
            AiProtocol::Gemini,
        ]
    }
}

/// A one-click starting point. The user can edit every field afterwards, which
/// is what makes "any AI" true rather than "any AI on our list".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiPreset {
    pub name: &'static str,
    pub protocol: AiProtocol,
    pub base_url: &'static str,
    pub model: &'static str,
    pub needs_key: bool,
}

pub const AI_PRESETS: &[AiPreset] = &[
    AiPreset {
        name: "Anthropic Claude",
        protocol: AiProtocol::Anthropic,
        base_url: "https://api.anthropic.com/v1",
        model: "claude-sonnet-4-5",
        needs_key: true,
    },
    AiPreset {
        name: "OpenAI",
        protocol: AiProtocol::OpenAi,
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o-mini",
        needs_key: true,
    },
    AiPreset {
        name: "Google Gemini",
        protocol: AiProtocol::Gemini,
        base_url: "https://generativelanguage.googleapis.com/v1beta",
        model: "gemini-2.0-flash",
        needs_key: true,
    },
    AiPreset {
        name: "OpenRouter",
        protocol: AiProtocol::OpenAi,
        base_url: "https://openrouter.ai/api/v1",
        model: "anthropic/claude-sonnet-4.5",
        needs_key: true,
    },
    AiPreset {
        name: "DeepSeek",
        protocol: AiProtocol::OpenAi,
        base_url: "https://api.deepseek.com/v1",
        model: "deepseek-chat",
        needs_key: true,
    },
    AiPreset {
        name: "Groq",
        protocol: AiProtocol::OpenAi,
        base_url: "https://api.groq.com/openai/v1",
        model: "llama-3.3-70b-versatile",
        needs_key: true,
    },
    AiPreset {
        name: "Mistral",
        protocol: AiProtocol::OpenAi,
        base_url: "https://api.mistral.ai/v1",
        model: "mistral-small-latest",
        needs_key: true,
    },
    AiPreset {
        name: "Ollama (локально)",
        protocol: AiProtocol::OpenAi,
        base_url: "http://localhost:11434/v1",
        model: "qwen2.5:7b",
        needs_key: false,
    },
    AiPreset {
        name: "LM Studio (локально)",
        protocol: AiProtocol::OpenAi,
        base_url: "http://localhost:1234/v1",
        model: "local-model",
        needs_key: false,
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default)]
    pub protocol: AiProtocol,
    #[serde(default = "AiConfig::default_base_url")]
    pub base_url: String,
    #[serde(default = "AiConfig::default_model")]
    pub model: String,
    #[serde(default)]
    pub api_key: Secret,
    #[serde(default = "AiConfig::default_temperature")]
    pub temperature: f32,
    /// Empty means the built-in prompt. Lets the user ask for a tone, a
    /// glossary, or "keep code identifiers untouched".
    #[serde(default)]
    pub extra_instructions: String,
    #[serde(default = "AiConfig::default_timeout")]
    pub timeout_secs: u64,
    /// Preset the user last picked, remembered only so the UI can highlight it.
    #[serde(default)]
    pub preset_name: String,
}

impl AiConfig {
    fn default_base_url() -> String {
        AI_PRESETS[0].base_url.to_string()
    }
    fn default_model() -> String {
        AI_PRESETS[0].model.to_string()
    }
    fn default_temperature() -> f32 {
        0.2
    }
    fn default_timeout() -> u64 {
        45
    }

    pub fn apply_preset(&mut self, preset: &AiPreset) {
        self.protocol = preset.protocol;
        self.base_url = preset.base_url.to_string();
        self.model = preset.model.to_string();
        self.preset_name = preset.name.to_string();
    }

    /// A local model behind `localhost` legitimately needs no key, so "usable"
    /// is not simply "has a key".
    pub fn is_usable(&self) -> bool {
        if self.base_url.trim().is_empty() || self.model.trim().is_empty() {
            return false;
        }
        !self.requires_key_here() || !self.api_key.is_empty()
    }

    pub fn requires_key_here(&self) -> bool {
        !is_loopback(&self.base_url)
    }
}

fn is_loopback(url: &str) -> bool {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    matches!(
        host,
        "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]" | "::1"
    )
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            protocol: AiProtocol::default(),
            base_url: Self::default_base_url(),
            model: Self::default_model(),
            api_key: Secret::default(),
            temperature: Self::default_temperature(),
            extra_instructions: String::new(),
            timeout_secs: Self::default_timeout(),
            preset_name: AI_PRESETS[0].name.to_string(),
        }
    }
}

// ── Logging ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSettings {
    #[serde(default = "LogSettings::default_retention")]
    pub retention_days: u16,
    #[serde(default = "LogSettings::default_max_mb")]
    pub max_mb_per_day: u64,
    /// Writes recognised text and translations to the log verbatim. Off by
    /// default because that is the contents of the user's screen.
    #[serde(default)]
    pub verbose: bool,
}

impl LogSettings {
    fn default_retention() -> u16 {
        3
    }
    fn default_max_mb() -> u64 {
        8
    }
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            retention_days: Self::default_retention(),
            max_mb_per_day: Self::default_max_mb(),
            verbose: false,
        }
    }
}

// ── Root ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub source_lang: Language,
    pub target_lang: Language,
    pub hotkeys: Hotkeys,
    pub capture_mode: CaptureMode,
    pub result_view: ResultView,
    pub theme: ThemeMode,
    pub engines: EngineSettings,
    pub logs: LogSettings,

    pub launch_at_startup: bool,
    pub copy_to_clipboard: bool,
    /// Run without a tray icon, reachable only by hotkey. Straight from the
    /// Windows design, where Settings offers exactly this.
    pub hide_tray_icon: bool,
    /// Show the Область/Окно/Весь экран switcher during capture.
    pub show_mode_hud: bool,
    pub play_sound: bool,
    /// Keep the result window above everything else.
    pub pin_result_window: bool,
    /// Dismiss the result as soon as the user clicks away from it. On by
    /// default: a translation is read once, and a window that has to be closed
    /// by hand is one more thing in the way.
    pub close_result_on_focus_loss: bool,
    /// Raise a desktop notification when a newer release appears. On by
    /// default — the app sits in the tray, so there is nowhere else a user
    /// would notice.
    pub notify_about_updates: bool,
    /// The release the user has already been told about, so the same version
    /// does not announce itself at every launch.
    #[serde(default)]
    pub notified_version: String,
    pub history_limit: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source_lang: Language::Auto,
            target_lang: Language::Ru,
            hotkeys: Hotkeys::default(),
            capture_mode: CaptureMode::Region,
            result_view: ResultView::Popup,
            theme: ThemeMode::System,
            engines: EngineSettings::default(),
            logs: LogSettings::default(),
            launch_at_startup: false,
            copy_to_clipboard: false,
            hide_tray_icon: false,
            show_mode_hud: true,
            play_sound: false,
            pin_result_window: false,
            close_result_on_focus_loss: true,
            notify_about_updates: true,
            notified_version: String::new(),
            history_limit: 50,
        }
    }
}

impl Settings {
    /// True when a capture would produce nothing the user can see or paste.
    pub fn produces_no_output(&self) -> bool {
        self.result_view == ResultView::None && !self.copy_to_clipboard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_config_file_loads_as_defaults() {
        let s: Settings = toml::from_str("").expect("empty config must parse");
        assert_eq!(s.target_lang, Language::Ru);
        assert_eq!(s.result_view, ResultView::Popup);
    }

    #[test]
    fn a_config_from_the_previous_version_still_loads() {
        // Only the fields v0.1.0 wrote. Everything new must fall back.
        let old = r#"
            source_lang = "En"
            target_lang = "Ru"
            launch_at_startup = false
            copy_to_clipboard = true
        "#;
        let s: Settings = toml::from_str(old).expect("old config must parse");
        assert!(s.copy_to_clipboard);
        assert_eq!(s.hotkeys.region.key, VK_T);
        assert!(s.engines.yandex);
    }

    #[test]
    fn settings_roundtrip_through_toml() {
        let mut s = Settings::default();
        s.engines.ai = true;
        s.engines.ai_config.api_key = Secret::new("sk-test-key");
        let text = toml::to_string_pretty(&s).expect("serialise");
        assert!(
            !text.contains("sk-test-key"),
            "raw API key must not appear in the config file"
        );
        let back: Settings = toml::from_str(&text).expect("deserialise");
        assert!(back.engines.ai);
    }

    #[test]
    fn active_engines_skip_ones_missing_a_key() {
        // Both enabled, neither given a key.
        let e = EngineSettings {
            deepl: true,
            ai: true,
            ..Default::default()
        };
        let active = e.active();
        assert!(!active.contains(&EngineKind::DeepL));
        assert!(!active.contains(&EngineKind::Ai));
        assert_eq!(active, vec![EngineKind::Yandex, EngineKind::Google]);
    }

    #[test]
    fn active_engines_respect_the_configured_order() {
        let e = EngineSettings {
            order: vec![EngineKind::Google, EngineKind::Yandex],
            ..Default::default()
        };
        assert_eq!(e.active(), vec![EngineKind::Google, EngineKind::Yandex]);
    }

    #[test]
    fn an_engine_missing_from_order_is_still_reachable() {
        // Yandex dropped from the list entirely.
        let e = EngineSettings {
            order: vec![EngineKind::Google],
            ..Default::default()
        };
        assert!(e.active().contains(&EngineKind::Yandex));
    }

    #[test]
    fn a_local_model_needs_no_api_key() {
        let mut c = AiConfig {
            base_url: "http://localhost:11434/v1".into(),
            model: "qwen2.5:7b".into(),
            ..Default::default()
        };
        assert!(c.is_usable());

        c.base_url = "https://api.openai.com/v1".into();
        assert!(!c.is_usable(), "a hosted endpoint must require a key");
    }

    #[test]
    fn duplicate_bindings_are_reported() {
        let mut h = Hotkeys::default();
        h.window = h.region;
        let conflicts = h.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert!(matches!(
            conflicts[0],
            (HotkeyAction::Region, HotkeyAction::Window)
        ));
    }

    #[test]
    fn unbound_hotkeys_never_conflict() {
        let h = Hotkeys {
            region: Hotkey::unbound(),
            window: Hotkey::unbound(),
            ..Default::default()
        };
        assert!(h.conflicts().is_empty());
    }
}
