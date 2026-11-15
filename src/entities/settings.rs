use serde::{Deserialize, Serialize};

use crate::entities::language::Language;
use crate::shared::i18n::t;
use crate::shared::i18n::Lang;
use crate::shared::secret::Secret;
use crate::ui::theme::ThemeMode;
use crate::ui::Platform;

// Windows virtual-key codes and hotkey modifier bits. On macOS the same numbers
// are carried through and translated at the point of registration, so settings
// files stay portable between platforms.
pub const VK_F: u32 = 0x46;
pub const VK_Q: u32 = 0x51;
pub const VK_R: u32 = 0x52;
pub const VK_W: u32 = 0x57;
pub const VK_ESCAPE: u32 = 0x1B;
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
            return t("Not set").to_string();
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
        0x1B => "Esc",
        0x21 => "PgUp",
        0x22 => "PgDn",
        0x23 => "End",
        0x24 => "Home",
        0x2D => "Ins",
        0x2E => "Del",
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

/// Whether a key may be bound with no modifier held.
///
/// A bare letter would swallow that letter everywhere on the system, which is
/// why a shortcut normally needs a modifier. The function row and the
/// navigation block type nothing, and they are exactly the keys a player has
/// free: both hands are on the mouse and WASD, and a chord is one finger too
/// many.
pub fn bindable_alone(vk: u32) -> bool {
    matches!(vk, 0x70..=0x7B | 0x21..=0x24 | 0x2D | 0x2E)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkeys {
    #[serde(default = "Hotkeys::default_region")]
    pub region: Hotkey,
    #[serde(default = "Hotkeys::default_window")]
    pub window: Hotkey,
    #[serde(default = "Hotkeys::default_fullscreen")]
    pub fullscreen: Hotkey,
    /// Re-run the last capture without drawing a new rectangle.
    #[serde(default = "Hotkeys::default_repeat")]
    pub repeat: Hotkey,
    /// Pick a region and keep translating it as its text changes.
    #[serde(default = "Hotkeys::default_watch")]
    pub watch: Hotkey,
    /// Take the translation off the screen.
    #[serde(default = "Hotkey::unbound")]
    pub hide: Hotkey,
}

impl Hotkeys {
    // Alt-based on purpose. The previous defaults were Ctrl+T, Ctrl+Shift+W
    // and Ctrl+Shift+S: a global hotkey takes its combination away from every
    // other program, so with the app running a browser could no longer open a
    // tab — and with it closed, the same habit closed the browser window.
    fn default_region() -> Hotkey {
        Hotkey::new(MOD_ALT, VK_Q)
    }
    fn default_window() -> Hotkey {
        Hotkey::new(MOD_ALT | MOD_SHIFT, VK_W)
    }
    fn default_fullscreen() -> Hotkey {
        Hotkey::new(MOD_ALT | MOD_SHIFT, VK_F)
    }
    fn default_repeat() -> Hotkey {
        Hotkey::new(MOD_ALT | MOD_SHIFT, VK_R)
    }
    fn default_watch() -> Hotkey {
        Hotkey::new(MOD_ALT | MOD_SHIFT, VK_Q)
    }

    pub fn all(&self) -> [(HotkeyAction, Hotkey); 6] {
        [
            (HotkeyAction::Region, self.region),
            (HotkeyAction::Window, self.window),
            (HotkeyAction::FullScreen, self.fullscreen),
            (HotkeyAction::Repeat, self.repeat),
            (HotkeyAction::Watch, self.watch),
            (HotkeyAction::Hide, self.hide),
        ]
    }

    pub fn slot_mut(&mut self, action: HotkeyAction) -> &mut Hotkey {
        match action {
            HotkeyAction::Region => &mut self.region,
            HotkeyAction::Window => &mut self.window,
            HotkeyAction::FullScreen => &mut self.fullscreen,
            HotkeyAction::Repeat => &mut self.repeat,
            HotkeyAction::Watch => &mut self.watch,
            HotkeyAction::Hide => &mut self.hide,
        }
    }

    /// Every action left without a shortcut — what the hotkey thread starts
    /// from before the first binding is applied.
    pub fn none() -> Self {
        Self {
            region: Hotkey::unbound(),
            window: Hotkey::unbound(),
            fullscreen: Hotkey::unbound(),
            repeat: Hotkey::unbound(),
            watch: Hotkey::unbound(),
            hide: Hotkey::unbound(),
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
            repeat: Self::default_repeat(),
            watch: Self::default_watch(),
            hide: Hotkey::unbound(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HotkeyAction {
    Region,
    Window,
    FullScreen,
    Repeat,
    Watch,
    Hide,
}

impl HotkeyAction {
    pub fn label(self) -> &'static str {
        match self {
            HotkeyAction::Region => t("Translate a region"),
            HotkeyAction::Window => t("Translate a window"),
            HotkeyAction::FullScreen => t("Translate the whole screen"),
            HotkeyAction::Repeat => t("Repeat the last capture"),
            HotkeyAction::Watch => t("Watch a region"),
            HotkeyAction::Hide => t("Hide the translation"),
        }
    }

    /// One line on what the action is for, where the name alone does not say.
    pub fn hint(self) -> &'static str {
        match self {
            HotkeyAction::Region => t("Drag a rectangle around the text"),
            HotkeyAction::Window => t("Click the window to translate"),
            HotkeyAction::FullScreen => t("Everything on the screen under the pointer"),
            HotkeyAction::Repeat => t("The same area again, without selecting it"),
            HotkeyAction::Watch => t("Keeps translating the area as its text changes"),
            HotkeyAction::Hide => t("Esc does the same while a translation is showing"),
        }
    }

    pub fn all() -> [HotkeyAction; 6] {
        [
            HotkeyAction::Region,
            HotkeyAction::Window,
            HotkeyAction::FullScreen,
            HotkeyAction::Repeat,
            HotkeyAction::Watch,
            HotkeyAction::Hide,
        ]
    }
}

// ── Capture and presentation ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    Region,
    Window,
    FullScreen,
}

impl CaptureMode {
    pub fn label(self) -> &'static str {
        match self {
            CaptureMode::Region => t("Region"),
            CaptureMode::Window => t("Window"),
            CaptureMode::FullScreen => t("Full screen"),
        }
    }

    /// Longer form for the tray menu, where the label has to say what it does.
    pub fn label_menu(self) -> &'static str {
        match self {
            CaptureMode::Region => t("Translate a region"),
            CaptureMode::Window => t("Translate a window"),
            CaptureMode::FullScreen => t("Translate the whole screen"),
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

/// How a finished translation is shown. The user picks one instead of the
/// design picking for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ResultView {
    /// Glass card anchored to the captured region.
    #[default]
    Popup,
    /// Translation painted over the original, in place.
    Inline,
    /// A line at the edge of the screen, the way a film is subtitled.
    Subtitle,
    /// Free-floating window with original and translation side by side.
    Window,
    /// Nothing on screen — clipboard only.
    None,
}

impl ResultView {
    pub fn label(self) -> &'static str {
        match self {
            ResultView::Popup => t("Popup by the selection"),
            ResultView::Inline => t("Over the original"),
            ResultView::Subtitle => t("Subtitles"),
            ResultView::Window => t("Separate window"),
            ResultView::None => t("Don't show"),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ResultView::Popup => t("A card with the original and the translation next to the captured area."),
            ResultView::Inline => {
                t("The translation replaces the text right where it was, in the same size and colours.")
            }
            ResultView::Subtitle => {
                t("A line at the edge of the screen, like film subtitles. The rest stays visible.")
            }
            ResultView::Window => t("A two-column window that can be kept on top of other windows."),
            ResultView::None => t("The result is only copied to the clipboard."),
        }
    }

    /// In the order the settings page lists them: the two that keep a game on
    /// screen first.
    pub fn all() -> [ResultView; 5] {
        [
            ResultView::Inline,
            ResultView::Subtitle,
            ResultView::Popup,
            ResultView::Window,
            ResultView::None,
        ]
    }

    /// Views that float over whatever is on screen and stay out of its way —
    /// no focus taken, nothing to close by hand.
    pub fn floats(self) -> bool {
        matches!(
            self,
            ResultView::Inline | ResultView::Subtitle | ResultView::Popup
        )
    }
}

/// When a translation leaves the screen on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AutoHide {
    /// Stays until dismissed.
    #[default]
    Never,
    /// Long enough to read it: longer for more text.
    Reading,
    Seconds5,
    Seconds10,
    Seconds20,
    Seconds30,
}

impl AutoHide {
    pub fn label(self) -> String {
        match self {
            AutoHide::Never => t("Never").to_string(),
            AutoHide::Reading => t("Once it has been read").to_string(),
            other => t("After {seconds} s").replace(
                "{seconds}",
                &other.fixed_seconds().unwrap_or_default().to_string(),
            ),
        }
    }

    pub fn all() -> [AutoHide; 6] {
        [
            AutoHide::Never,
            AutoHide::Reading,
            AutoHide::Seconds5,
            AutoHide::Seconds10,
            AutoHide::Seconds20,
            AutoHide::Seconds30,
        ]
    }

    fn fixed_seconds(self) -> Option<u64> {
        match self {
            AutoHide::Seconds5 => Some(5),
            AutoHide::Seconds10 => Some(10),
            AutoHide::Seconds20 => Some(20),
            AutoHide::Seconds30 => Some(30),
            AutoHide::Never | AutoHide::Reading => None,
        }
    }

    /// How long a translation of `chars` characters stays up, or `None` for
    /// "until dismissed".
    pub fn duration(self, chars: usize) -> Option<std::time::Duration> {
        use std::time::Duration;
        match self {
            AutoHide::Never => None,
            // A comfortable reading pace is around fifteen characters a
            // second; a couple of seconds on top to find the text first, and
            // a ceiling so a wall of text does not sit there for minutes.
            AutoHide::Reading => {
                let seconds = 2.5 + chars as f32 / 15.0;
                Some(Duration::from_secs_f32(seconds.clamp(4.0, 45.0)))
            }
            other => other.fixed_seconds().map(Duration::from_secs),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SubtitleSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl SubtitleSize {
    pub fn label(self) -> &'static str {
        match self {
            SubtitleSize::Small => t("Small"),
            SubtitleSize::Medium => t("Medium"),
            SubtitleSize::Large => t("Large"),
        }
    }

    pub fn all() -> [SubtitleSize; 3] {
        [
            SubtitleSize::Small,
            SubtitleSize::Medium,
            SubtitleSize::Large,
        ]
    }

    /// Font size in points on a 1080-line screen; scaled with the monitor.
    pub fn points(self) -> f32 {
        match self {
            SubtitleSize::Small => 17.0,
            SubtitleSize::Medium => 22.0,
            SubtitleSize::Large => 29.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SubtitlePlace {
    #[default]
    Bottom,
    Top,
}

impl SubtitlePlace {
    pub fn label(self) -> &'static str {
        match self {
            SubtitlePlace::Bottom => t("Bottom of the screen"),
            SubtitlePlace::Top => t("Top of the screen"),
        }
    }

    pub fn all() -> [SubtitlePlace; 2] {
        [SubtitlePlace::Bottom, SubtitlePlace::Top]
    }
}

/// How the floating views look and behave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Presentation {
    pub auto_hide: AutoHide,
    /// Let mouse clicks fall through the translation to whatever is under it.
    /// The translation cannot be clicked away then — Esc, the timer and the
    /// next capture are what remove it.
    pub click_through: bool,
    /// While a translation is on screen, Esc takes it away — from anywhere,
    /// without the app's window having to be focused.
    pub esc_hides: bool,
    pub subtitle_size: SubtitleSize,
    pub subtitle_place: SubtitlePlace,
    /// Opacity of the plate behind the subtitle, in percent.
    pub subtitle_opacity: u8,
    /// Show the recognised text above its translation.
    pub subtitle_original: bool,
}

impl Default for Presentation {
    fn default() -> Self {
        Self {
            auto_hide: AutoHide::Never,
            click_through: false,
            esc_hides: true,
            subtitle_size: SubtitleSize::Medium,
            subtitle_place: SubtitlePlace::Bottom,
            subtitle_opacity: 82,
            subtitle_original: false,
        }
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

    /// Whether a capture should be handed to the AI as an image rather than
    /// recognised first. Only when the AI is the engine that would be asked
    /// anyway: sending a screenshot to a model that sits third in the queue
    /// would overrule the order the user set.
    pub fn reads_screenshots(&self) -> bool {
        self.ai_config.vision && self.active().first() == Some(&EngineKind::Ai)
    }

    pub fn status(&self, kind: EngineKind) -> &'static str {
        match kind {
            EngineKind::Yandex | EngineKind::Google => t("No key needed"),
            EngineKind::DeepL => {
                if self.deepl_key.is_empty() {
                    t("Key required")
                } else {
                    t("API key")
                }
            }
            EngineKind::Ai => {
                if self.ai_config.is_usable() {
                    t("Ready")
                } else {
                    t("Key required")
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
            AiProtocol::OpenAi => t("OpenAI-compatible"),
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

impl AiPreset {
    /// What the chip says. `name` doubles as the stored identity of the preset,
    /// so it stays English and only the display goes through the tables.
    pub fn display_name(&self) -> &'static str {
        t(self.name)
    }
}

pub const AI_PRESETS: &[AiPreset] = &[
    AiPreset {
        name: "Anthropic Claude",
        protocol: AiProtocol::Anthropic,
        base_url: "https://api.anthropic.com/v1",
        model: "claude-sonnet-5-5",
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
        model: "anthropic/claude-sonnet-5.5",
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
        name: "Ollama (local)",
        protocol: AiProtocol::OpenAi,
        base_url: "http://localhost:11434/v1",
        model: "qwen2.5:7b",
        needs_key: false,
    },
    AiPreset {
        name: "LM Studio (local)",
        protocol: AiProtocol::OpenAi,
        base_url: "http://localhost:1234/v1",
        model: "local-model",
        needs_key: false,
    },
];

/// Models a preset used to set that their provider is withdrawing, with what
/// the preset sets now: endpoint, old model, its successor.
///
/// A preset's model is not only a suggestion on a chip. Every save writes the
/// whole config, so the default of the day sits in the file of anyone who ever
/// changed a setting — whether or not they have opened the AI page. Changing
/// the preset alone would leave every one of them asking for a model that no
/// longer answers.
pub const RETIRED_MODELS: &[(&str, &str, &str)] = &[
    // Withdrawn by Anthropic on 30 November 2026.
    (
        "https://api.anthropic.com/v1",
        "claude-sonnet-4-5",
        "claude-sonnet-5-5",
    ),
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
    /// Send the screenshot itself and let the model read it, instead of
    /// recognising the text first. Needs a model that accepts images.
    #[serde(default)]
    pub vision: bool,
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

    /// Moves the config off a model that is being withdrawn, onto the one
    /// that replaces it. Returns the model it moved from, if it moved.
    ///
    /// Only the exact pair a preset used to set is touched — that endpoint,
    /// that name. The same name behind someone's own gateway, or any model
    /// typed in by hand, is theirs to keep.
    pub fn replace_retired_model(&mut self) -> Option<String> {
        let endpoint = self.base_url.trim().trim_end_matches('/');
        let (_, _, successor) = RETIRED_MODELS
            .iter()
            .find(|(url, model, _)| endpoint == *url && self.model.trim() == *model)?;
        Some(std::mem::replace(&mut self.model, (*successor).to_string()))
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
            vision: false,
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
    pub result_view: ResultView,
    pub theme: ThemeMode,
    pub engines: EngineSettings,
    pub logs: LogSettings,
    pub presentation: Presentation,
    /// How often a watched region is looked at again, in milliseconds.
    pub watch_interval_ms: u32,

    pub launch_at_startup: bool,
    pub copy_to_clipboard: bool,
    /// Run without a tray icon, reachable only by hotkey. Straight from the
    /// Windows design, where Settings offers exactly this.
    pub hide_tray_icon: bool,
    /// Show the Region / Window / Full screen switcher during capture.
    pub show_mode_hud: bool,
    pub play_sound: bool,
    /// Keep the result window above everything else.
    pub pin_result_window: bool,
    /// Dismiss the result as soon as the user clicks somewhere else. On by
    /// default: a translation is read once, and one that has to be closed by
    /// hand is one more thing in the way.
    pub close_result_on_focus_loss: bool,
    /// Raise a desktop notification when a newer release appears. On by
    /// default — the app sits in the tray, so there is nowhere else a user
    /// would notice.
    pub notify_about_updates: bool,
    /// The release the user has already been told about, so the same version
    /// does not announce itself at every launch.
    #[serde(default)]
    pub notified_version: String,
    /// Keep a list of recent translations for the session. Off means nothing is
    /// recorded at all — the list is a record of everything the user pointed the
    /// tool at, and some people would rather it did not exist even in memory.
    pub keep_history: bool,
    /// Interface language. `None` follows the OS, which is what a fresh
    /// install does — someone running a Japanese system did not ask for an
    /// English app.
    #[serde(default)]
    pub ui_language: Option<Lang>,
    pub history_limit: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source_lang: Language::Auto,
            target_lang: Language::Ru,
            hotkeys: Hotkeys::default(),
            result_view: ResultView::Inline,
            theme: ThemeMode::System,
            engines: EngineSettings::default(),
            logs: LogSettings::default(),
            presentation: Presentation::default(),
            watch_interval_ms: 1000,
            launch_at_startup: false,
            copy_to_clipboard: false,
            hide_tray_icon: false,
            show_mode_hud: true,
            play_sound: false,
            pin_result_window: true,
            close_result_on_focus_loss: true,
            notify_about_updates: true,
            notified_version: String::new(),
            keep_history: true,
            ui_language: None,
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
        // The translation lands over the text it came from, and stays in front
        // until dismissed — both are what a fresh install now does.
        assert_eq!(s.result_view, ResultView::Inline);
        assert!(s.pin_result_window);
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
        assert_eq!(s.hotkeys.region.key, VK_Q);
        assert!(s.engines.yandex);
    }

    #[test]
    fn a_config_from_before_subtitles_keeps_working() {
        // Exactly what 0.2.x wrote for the parts that have since grown.
        let old = r#"
            result_view = "Inline"
            pin_result_window = true

            [hotkeys.region]
            modifiers = 16386
            key = 84
            enabled = true

            [hotkeys.repeat]
            modifiers = 16384
            key = 0
            enabled = false
        "#;
        let s: Settings = toml::from_str(old).expect("old config must parse");
        // What the user had stays as they had it…
        assert_eq!(s.result_view, ResultView::Inline);
        assert_eq!(s.hotkeys.region.key, 0x54);
        assert_eq!(s.hotkeys.region.modifiers & MOD_CONTROL, MOD_CONTROL);
        assert!(!s.hotkeys.repeat.is_bound());
        // …and what did not exist yet arrives with its defaults.
        assert!(s.hotkeys.watch.is_bound());
        assert!(!s.hotkeys.hide.is_bound());
        assert_eq!(s.presentation, Presentation::default());
        assert!(!s.engines.ai_config.vision);
    }

    #[test]
    fn the_default_shortcuts_do_not_collide_with_each_other() {
        assert!(Hotkeys::default().conflicts().is_empty());
    }

    #[test]
    fn no_default_shortcut_takes_a_ctrl_combination_from_other_programs() {
        // Ctrl+T, Ctrl+W and Ctrl+S belong to every browser and editor; a
        // global hotkey on any of them breaks those programs while the app
        // runs.
        for (action, hotkey) in Hotkeys::default().all() {
            assert_eq!(
                hotkey.modifiers & MOD_CONTROL,
                0,
                "{action:?} is bound to a Ctrl combination by default"
            );
        }
    }

    #[test]
    fn function_keys_may_be_bound_alone_and_letters_may_not() {
        assert!(bindable_alone(0x77)); // F8
        assert!(bindable_alone(0x2D)); // Insert
        assert!(!bindable_alone(VK_Q));
        assert!(!bindable_alone(0x20)); // Space
        assert!(!bindable_alone(VK_ESCAPE));
    }

    #[test]
    fn every_key_that_may_be_bound_alone_has_a_printable_name() {
        for vk in 0..=0xFF {
            if bindable_alone(vk) {
                assert_ne!(vk_name(vk), "?", "key {vk:#x} has no name");
            }
        }
    }

    #[test]
    fn reading_time_grows_with_the_text_and_stays_within_bounds() {
        let short = AutoHide::Reading.duration(10).unwrap();
        let long = AutoHide::Reading.duration(300).unwrap();
        let huge = AutoHide::Reading.duration(100_000).unwrap();
        assert!(short.as_secs_f32() >= 4.0);
        assert!(long > short);
        assert!(huge.as_secs_f32() <= 45.0);
    }

    #[test]
    fn never_means_no_deadline_and_a_fixed_time_ignores_the_length() {
        assert_eq!(AutoHide::Never.duration(50), None);
        assert_eq!(
            AutoHide::Seconds10.duration(5),
            AutoHide::Seconds10.duration(5000)
        );
        assert_eq!(AutoHide::Seconds10.duration(5).unwrap().as_secs(), 10);
    }

    #[test]
    fn the_ai_reads_screenshots_only_when_it_is_first_in_line() {
        let mut e = EngineSettings {
            ai: true,
            ..Default::default()
        };
        e.ai_config.api_key = Secret::new("k");
        e.ai_config.vision = true;
        assert!(e.reads_screenshots());

        // Moved behind another engine: the order the user set wins.
        e.order = vec![EngineKind::Yandex, EngineKind::Ai];
        assert!(!e.reads_screenshots());

        // First in line, but the option is off.
        e.order = vec![EngineKind::Ai, EngineKind::Yandex];
        e.ai_config.vision = false;
        assert!(!e.reads_screenshots());

        // First in the list, but switched off — so not first in line at all.
        e.ai_config.vision = true;
        e.ai = false;
        assert!(!e.reads_screenshots());
    }

    #[test]
    fn every_result_view_is_offered_exactly_once() {
        let all = ResultView::all();
        for view in [
            ResultView::Popup,
            ResultView::Inline,
            ResultView::Subtitle,
            ResultView::Window,
            ResultView::None,
        ] {
            assert_eq!(all.iter().filter(|v| **v == view).count(), 1, "{view:?}");
        }
    }

    #[test]
    fn settings_roundtrip_through_toml() {
        let mut s = Settings::default();
        s.engines.ai = true;
        s.engines.ai_config.api_key = Secret::new("sk-test-key");
        let text = toml::to_string_pretty(&s).expect("serialise");
        // Only when the platform has somewhere to keep the key. A Linux box
        // with no Secret Service running — a CI container, for one — has
        // nowhere, and the settings window says so rather than pretending;
        // asserting unconditionally here fails the build over a working
        // fallback.
        if crate::shared::secret::sealing_available() {
            assert!(
                !text.contains("sk-test-key"),
                "raw API key must not appear in the config file"
            );
        }
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

    /// What a config saved before the preset changed has in it.
    fn saved_with_the_old_preset() -> AiConfig {
        AiConfig {
            protocol: AiProtocol::Anthropic,
            base_url: "https://api.anthropic.com/v1".into(),
            model: "claude-sonnet-4-5".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_config_saved_with_a_withdrawn_model_moves_to_its_successor() {
        let mut c = saved_with_the_old_preset();
        assert_eq!(c.replace_retired_model().as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(c.model, "claude-sonnet-5-5");
        // And only once: there is nothing left to move.
        assert_eq!(c.replace_retired_model(), None);
    }

    #[test]
    fn a_trailing_slash_or_a_stray_space_does_not_hide_a_withdrawn_model() {
        let mut c = AiConfig {
            base_url: " https://api.anthropic.com/v1/ ".into(),
            model: " claude-sonnet-4-5".into(),
            ..saved_with_the_old_preset()
        };
        assert!(c.replace_retired_model().is_some());
        assert_eq!(c.model, "claude-sonnet-5-5");
    }

    #[test]
    fn the_same_model_name_behind_another_endpoint_is_left_alone() {
        // Someone's own gateway may go on serving it for as long as it likes.
        let mut c = AiConfig {
            base_url: "https://llm.example.internal/v1".into(),
            ..saved_with_the_old_preset()
        };
        assert_eq!(c.replace_retired_model(), None);
        assert_eq!(c.model, "claude-sonnet-4-5");
    }

    #[test]
    fn a_model_chosen_by_hand_is_left_alone() {
        let mut c = AiConfig {
            model: "claude-opus-5-5".into(),
            ..saved_with_the_old_preset()
        };
        assert_eq!(c.replace_retired_model(), None);
        assert_eq!(c.model, "claude-opus-5-5");
    }

    #[test]
    fn no_preset_offers_a_model_that_is_being_withdrawn() {
        for preset in AI_PRESETS {
            let withdrawn = RETIRED_MODELS
                .iter()
                .any(|(url, model, _)| *url == preset.base_url && *model == preset.model);
            assert!(!withdrawn, "{} still offers {}", preset.name, preset.model);
        }
    }

    #[test]
    fn a_withdrawn_model_is_replaced_by_one_that_is_not() {
        // A successor that is itself on the list would send a config round
        // in a circle, or leave it one hop short.
        for (url, _, successor) in RETIRED_MODELS {
            let chained = RETIRED_MODELS
                .iter()
                .any(|(other_url, model, _)| other_url == url && model == successor);
            assert!(!chained, "{successor} is listed as withdrawn too");
        }
    }

    #[test]
    fn a_fresh_install_starts_on_the_current_anthropic_model() {
        let c = AiConfig::default();
        assert_eq!(c.model, "claude-sonnet-5-5");
        assert_eq!(c.base_url, "https://api.anthropic.com/v1");
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
