//! Sakura design tokens.
//!
//! One brand, two dialects. The sakura pink is constant; everything structural —
//! the accent, the surfaces, the greys — comes from the platform, because the
//! design specifies Aqua and Fluent as two complete rounds rather than one look
//! with different corner radii.
//!
//! Notably the accent is *not* shared: macOS uses the Aqua blue on a white
//! foreground, while Windows uses Fluent `#0F6CBD` in light mode and the much
//! lighter `#4CC2FF` in dark — which needs a dark navy foreground, not white, or
//! the label on a primary button is unreadable.
//!
//! Surfaces are frameless. Depth comes from a step in fill — `group` sits a
//! couple of tones off `window` — and from `separator`, a divider faint enough
//! to read as a hint of structure rather than a line.

use egui::{Color32, Context, FontData, FontDefinitions, FontFamily, Rounding, Stroke, Visuals};
use tracing::debug;

use crate::shared::i18n::t;
use super::platform::{Metrics, Platform};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => t("Match the system"),
            ThemeMode::Light => t("Light"),
            ThemeMode::Dark => t("Dark"),
        }
    }

    pub fn resolve(self) -> bool {
        match self {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => system_prefers_dark(),
        }
    }
}

/// Reads the OS appearance setting. Falls back to light, which is what both
/// platforms ship with.
fn system_prefers_dark() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) =
            hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        {
            if let Ok(light) = key.get_value::<u32, _>("AppsUseLightTheme") {
                return light == 0;
            }
        }
        false
    }
    #[cfg(target_os = "macos")]
    {
        // `defaults read -g AppleInterfaceStyle` prints "Dark" and exits non-zero
        // when the system is in light mode.
        std::process::Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "Dark")
            .unwrap_or(false)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        false
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub platform: Platform,
    pub metrics: Metrics,

    // Surfaces, back to front.
    /// Settings window background.
    pub window: Color32,
    /// Navigation pane. Equal to `window` on Windows, where Settings uses one
    /// Mica surface throughout.
    pub chrome: Color32,
    /// The grouped container that settings rows sit in: a couple of tones off
    /// `window`, no outline.
    pub card: Color32,
    /// Translation column — a whisper of sakura so the eye lands on it first.
    pub card_accent: Color32,
    /// Glass popups floating over the desktop.
    pub glass: Color32,
    /// Faint strip under a popup's action row.
    pub footer: Color32,
    /// Dimming laid over the frozen desktop during capture.
    pub scrim: Color32,

    /// Divider between rows inside a group. Deliberately at the edge of
    /// visibility — it should suggest a boundary, not draw one.
    pub separator: Color32,
    /// Reserved for surfaces that float over unknown content, where an edge is
    /// the only thing separating the panel from whatever is behind it.
    pub border: Color32,

    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    /// Foreground for anything painted on `accent`.
    pub on_accent: Color32,

    /// Brand mark and anything that says "this is the translation".
    pub sakura: Color32,
    pub sakura_deep: Color32,
    pub sakura_soft: Color32,
    /// Primary actions and selection.
    pub accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
}

impl Theme {
    pub fn resolve(mode: ThemeMode) -> Self {
        Self::for_platform(Platform::current(), mode.resolve())
    }

    pub fn for_platform(platform: Platform, dark: bool) -> Self {
        match (platform, dark) {
            (Platform::Windows, false) => Self::windows_light(),
            (Platform::Windows, true) => Self::windows_dark(),
            (Platform::MacOs, false) => Self::mac_light(),
            (Platform::MacOs, true) => Self::mac_dark(),
        }
    }

    fn windows_light() -> Self {
        let platform = Platform::Windows;
        Self {
            dark: false,
            platform,
            metrics: platform.metrics(),

            window: Color32::from_rgb(0xF3, 0xF3, 0xF3),
            chrome: Color32::from_rgb(0xF3, 0xF3, 0xF3),
            card: Color32::WHITE,
            card_accent: Color32::from_rgb(0xFD, 0xF4, 0xF7),
            glass: Color32::from_rgba_unmultiplied(0xF9, 0xF9, 0xF9, 242),
            footer: Color32::from_rgba_unmultiplied(0, 0, 0, 6),
            scrim: Color32::from_rgba_unmultiplied(6, 12, 24, 140),

            separator: Color32::from_rgba_unmultiplied(0, 0, 0, 16),
            border: Color32::from_rgba_unmultiplied(0, 0, 0, 14),

            text: Color32::from_rgb(0x1B, 0x1B, 0x1B),
            text_dim: Color32::from_rgb(0x6E, 0x6E, 0x70),
            text_faint: Color32::from_rgb(0x8C, 0x8C, 0x90),
            on_accent: Color32::WHITE,

            sakura: Color32::from_rgb(0xE8, 0x7C, 0x9E),
            sakura_deep: Color32::from_rgb(0xC2, 0x53, 0x7C),
            sakura_soft: Color32::from_rgb(0xF0, 0xA7, 0xBD),
            accent: Color32::from_rgb(0x0F, 0x6C, 0xBD),
            success: Color32::from_rgb(0x0F, 0x7B, 0x0F),
            warning: Color32::from_rgb(0x9D, 0x5D, 0x00),
            danger: Color32::from_rgb(0xC4, 0x2B, 0x1C),
        }
    }

    fn windows_dark() -> Self {
        let platform = Platform::Windows;
        Self {
            dark: true,
            platform,
            metrics: platform.metrics(),

            window: Color32::from_rgb(0x20, 0x20, 0x24),
            chrome: Color32::from_rgb(0x20, 0x20, 0x24),
            card: Color32::from_rgb(0x27, 0x27, 0x2C),
            card_accent: Color32::from_rgb(0x2E, 0x27, 0x2C),
            glass: Color32::from_rgba_unmultiplied(0x20, 0x20, 0x24, 234),
            footer: Color32::from_rgba_unmultiplied(255, 255, 255, 8),
            scrim: Color32::from_rgba_unmultiplied(6, 12, 24, 145),

            // The 0.08 white the brief asks for.
            separator: Color32::from_rgba_unmultiplied(255, 255, 255, 20),
            border: Color32::from_rgba_unmultiplied(255, 255, 255, 16),

            text: Color32::from_rgb(0xF2, 0xF2, 0xF4),
            text_dim: Color32::from_rgb(0xA6, 0xA6, 0xAE),
            text_faint: Color32::from_rgb(0x80, 0x80, 0x88),
            // Fluent's dark accent is a pale blue; white text on it is illegible.
            on_accent: Color32::from_rgb(0x00, 0x33, 0x54),

            sakura: Color32::from_rgb(0xF0, 0xA7, 0xBD),
            sakura_deep: Color32::from_rgb(0xE8, 0x7C, 0x9E),
            sakura_soft: Color32::from_rgb(0xF7, 0xC9, 0xD8),
            accent: Color32::from_rgb(0x4C, 0xC2, 0xFF),
            success: Color32::from_rgb(0x6C, 0xCB, 0x5F),
            warning: Color32::from_rgb(0xFF, 0xB4, 0x3C),
            danger: Color32::from_rgb(0xFF, 0x99, 0xA4),
        }
    }

    fn mac_light() -> Self {
        let platform = Platform::MacOs;
        Self {
            dark: false,
            platform,
            metrics: platform.metrics(),

            window: Color32::from_rgb(0xF2, 0xF2, 0xF4),
            chrome: Color32::from_rgb(0xEC, 0xEC, 0xEF),
            card: Color32::WHITE,
            card_accent: Color32::from_rgb(0xFD, 0xF4, 0xF7),
            glass: Color32::from_rgba_unmultiplied(0xF6, 0xF6, 0xF8, 238),
            footer: Color32::from_rgba_unmultiplied(0, 0, 0, 5),
            scrim: Color32::from_rgba_unmultiplied(10, 12, 22, 133),

            separator: Color32::from_rgba_unmultiplied(0, 0, 0, 14),
            border: Color32::from_rgba_unmultiplied(0, 0, 0, 12),

            text: Color32::from_rgb(0x1A, 0x1A, 0x1A),
            text_dim: Color32::from_rgb(0x6E, 0x6E, 0x72),
            text_faint: Color32::from_rgb(0x8E, 0x8E, 0x93),
            on_accent: Color32::WHITE,

            sakura: Color32::from_rgb(0xE8, 0x7C, 0x9E),
            sakura_deep: Color32::from_rgb(0xC2, 0x53, 0x7C),
            sakura_soft: Color32::from_rgb(0xF0, 0xA7, 0xBD),
            // The design's Aqua blue is #2F7CF6, which gives white button labels
            // only 3.9:1 — under the 4.5:1 needed at this text size. Apple's own
            // system blue is no better and ships anyway, but a translation popup
            // is read at a glance over arbitrary backgrounds, so the same hue is
            // taken two steps deeper to clear the threshold at 4.6:1.
            accent: Color32::from_rgb(0x2A, 0x72, 0xE0),
            success: Color32::from_rgb(0x24, 0x8A, 0x3D),
            warning: Color32::from_rgb(0xB0, 0x6E, 0x00),
            danger: Color32::from_rgb(0xD7, 0x36, 0x2B),
        }
    }

    fn mac_dark() -> Self {
        let platform = Platform::MacOs;
        Self {
            dark: true,
            platform,
            metrics: platform.metrics(),

            window: Color32::from_rgb(0x1C, 0x1E, 0x24),
            chrome: Color32::from_rgb(0x21, 0x23, 0x2A),
            card: Color32::from_rgb(0x24, 0x26, 0x2E),
            card_accent: Color32::from_rgb(0x30, 0x28, 0x2F),
            glass: Color32::from_rgba_unmultiplied(0x20, 0x22, 0x2A, 232),
            footer: Color32::from_rgba_unmultiplied(255, 255, 255, 8),
            scrim: Color32::from_rgba_unmultiplied(6, 7, 12, 145),

            separator: Color32::from_rgba_unmultiplied(255, 255, 255, 20),
            border: Color32::from_rgba_unmultiplied(255, 255, 255, 16),

            text: Color32::from_rgb(0xF0, 0xF0, 0xF2),
            text_dim: Color32::from_rgb(0xA6, 0xA8, 0xB0),
            text_faint: Color32::from_rgb(0x7C, 0x7E, 0x88),
            on_accent: Color32::WHITE,

            sakura: Color32::from_rgb(0xF0, 0xA7, 0xBD),
            sakura_deep: Color32::from_rgb(0xE8, 0x7C, 0x9E),
            sakura_soft: Color32::from_rgb(0xF7, 0xC9, 0xD8),
            // The design keeps a white label on blue in dark mode too, so the
            // blue is deepened here rather than flipping the label to navy the
            // way the Fluent dark round does. Same value as the light theme.
            accent: Color32::from_rgb(0x2A, 0x72, 0xE0),
            success: Color32::from_rgb(0x3D, 0xD1, 0x66),
            warning: Color32::from_rgb(0xFF, 0xB0, 0x2E),
            danger: Color32::from_rgb(0xFF, 0x6B, 0x62),
        }
    }

    pub fn surface_rounding(&self) -> Rounding {
        Rounding::same(self.metrics.surface_radius)
    }

    /// The grouped settings container.
    pub fn group_rounding(&self) -> Rounding {
        Rounding::same(self.metrics.group_radius)
    }

    pub fn control_rounding(&self) -> Rounding {
        Rounding::same(self.metrics.control_radius)
    }

    pub fn border_stroke(&self) -> Stroke {
        Stroke::new(1.0, self.border)
    }

    /// Same hue, dialled down — for hover fills and tinted notices.
    pub fn tint(&self, color: Color32, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
    }

    /// Outline for the inline translation patch. The deeper pink reads as an
    /// edge on a light page; on a dark one it needs the softer tint to stay
    /// visible without glowing.
    pub fn sakura_border(&self) -> Color32 {
        if self.dark {
            self.sakura_soft
        } else {
            self.sakura
        }
    }

    /// Resting fill of a frameless control — a dropdown, a button, a key badge.
    /// Enough separation from the surface underneath to read as a target.
    pub fn control_fill(&self) -> Color32 {
        if self.dark {
            Color32::from_rgba_unmultiplied(255, 255, 255, 26)
        } else {
            Color32::from_rgba_unmultiplied(0, 0, 0, 16)
        }
    }

    pub fn control_fill_hover(&self) -> Color32 {
        if self.dark {
            Color32::from_rgba_unmultiplied(255, 255, 255, 42)
        } else {
            Color32::from_rgba_unmultiplied(0, 0, 0, 26)
        }
    }

    pub fn control_fill_active(&self) -> Color32 {
        if self.dark {
            Color32::from_rgba_unmultiplied(255, 255, 255, 58)
        } else {
            Color32::from_rgba_unmultiplied(0, 0, 0, 38)
        }
    }

    /// Fill for something you type into.
    ///
    /// A translucent wash is enough for a button, but not for a text field: with
    /// no outline to mark it, an input has to sit *below* its container to look
    /// editable at all. Recessed rather than raised, and opaque so it does not
    /// change with whatever it happens to be layered on.
    pub fn field_fill(&self) -> Color32 {
        if self.dark {
            Color32::from_rgb(0x18, 0x18, 0x1C)
        } else {
            Color32::from_rgb(0xEC, 0xEC, 0xEF)
        }
    }

    pub fn hover_fill(&self) -> Color32 {
        if self.dark {
            Color32::from_rgba_unmultiplied(255, 255, 255, 18)
        } else {
            Color32::from_rgba_unmultiplied(0, 0, 0, 12)
        }
    }

    /// Pushes the palette into egui so stock widgets match the custom ones.
    pub fn apply(&self, ctx: &Context) {
        let mut v = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };

        v.override_text_color = Some(self.text);
        v.panel_fill = self.window;
        v.window_fill = self.window;
        v.extreme_bg_color = self.field_fill();
        v.faint_bg_color = self.hover_fill();
        v.hyperlink_color = self.accent;
        v.window_rounding = self.surface_rounding();
        v.menu_rounding = Rounding::same(self.metrics.menu_radius);
        v.window_stroke = self.border_stroke();
        v.selection.bg_fill = self.tint(self.accent, 90);
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.window_shadow = egui::epaint::Shadow {
            offset: egui::vec2(0.0, 12.0),
            blur: 32.0,
            spread: 0.0,
            color: Color32::from_black_alpha(self.metrics.shadow_alpha),
        };
        v.popup_shadow = egui::epaint::Shadow {
            offset: egui::vec2(0.0, 8.0),
            blur: 24.0,
            spread: 0.0,
            color: Color32::from_black_alpha(self.metrics.shadow_alpha),
        };

        // Frameless throughout: a control is a soft fill that reacts to the
        // pointer, never an outline. Dropdowns and text fields inherit this, so
        // there is no ring of hairlines down the settings page.
        let r = self.control_rounding();
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.rounding = r;
            w.bg_stroke = Stroke::NONE;
            w.expansion = 0.0;
        }
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text_dim);
        v.widgets.inactive.weak_bg_fill = self.control_fill();
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.hovered.weak_bg_fill = self.control_fill_hover();
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.active.weak_bg_fill = self.control_fill_active();
        v.widgets.active.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.open.weak_bg_fill = self.control_fill_active();
        v.widgets.open.fg_stroke = Stroke::new(1.0, self.text);

        ctx.set_visuals(v);

        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 24.0;
        style.spacing.window_margin = egui::Margin::same(self.metrics.window_padding);
        style.visuals.clip_rect_margin = 0.0;
        ctx.set_style(style);
    }
}

// ── Fonts ────────────────────────────────────────────────────────────────────

/// Builds the font stack: native UI face first so the app looks like it belongs
/// on the platform, then egui's bundled faces, then CJK, then emoji.
///
/// Order matters — egui walks the family list until a face has the glyph, so a
/// missing Cyrillic or kana character falls through instead of rendering tofu.
pub fn build_fonts() -> FontDefinitions {
    let platform = Platform::current();
    let mut fonts = FontDefinitions::default();

    if let Some((name, data)) = load_first(platform.ui_font_candidates()) {
        debug!(font = %name, "UI font loaded");
        fonts.font_data.insert("ui".to_owned(), data);
    }
    if let Some((name, data)) = load_first(platform.mono_font_candidates()) {
        debug!(font = %name, "Monospace font loaded");
        fonts.font_data.insert("mono".to_owned(), data);
    }
    // Every face we can find, not the first one: no single CJK font covers
    // Japanese, Chinese *and* Korean. Loading only the first meant a Korean
    // interface rendered as a window full of tofu, because the Japanese face
    // that happened to be first has no hangul in it.
    let cjk: Vec<String> = load_all_weighted(platform.cjk_font_candidates())
        .into_iter()
        .enumerate()
        .map(|(i, (name, data))| {
            let key = format!("cjk{i}");
            debug!(font = %name, "CJK fallback loaded");
            fonts.font_data.insert(key.clone(), data);
            key
        })
        .collect();

    let has_ui = fonts.font_data.contains_key("ui");
    let has_mono = fonts.font_data.contains_key("mono");

    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    if has_ui {
        prop.insert(0, "ui".to_owned());
    }
    // A CJK interface language wants these faces *first*: the Latin UI font
    // has no kana, hanzi or hangul, so leaving it in front means a miss on
    // every glyph, and the few characters it does cover come out in a
    // different face from the rest of the line.
    if crate::shared::i18n::current().needs_cjk() {
        for (i, key) in cjk.iter().enumerate() {
            prop.insert(i, key.clone());
        }
    } else {
        prop.extend(cjk.iter().cloned());
    }

    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    if has_mono {
        mono.insert(0, "mono".to_owned());
    }
    mono.extend(cjk.iter().cloned());

    fonts
}

fn load_first(paths: &[&str]) -> Option<(String, FontData)> {
    for path in paths {
        if let Ok(bytes) = std::fs::read(path) {
            return Some(((*path).to_string(), FontData::from_owned(bytes)));
        }
    }
    None
}

/// Every candidate that exists on this machine, in the order given.
///
/// The scripts are split across several files on every platform, so this
/// takes all of them and lets egui walk the list per glyph.
fn load_all_weighted(paths: &[(&str, f32)]) -> Vec<(String, FontData)> {
    let mut loaded = Vec::new();
    for (path, y_offset) in paths {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fd = FontData::from_owned(bytes);
            fd.tweak.y_offset_factor = *y_offset;
            loaded.push(((*path).to_string(), fd));
        }
    }
    loaded
}

/// Type ramp. Kept in one place so a size change lands everywhere at once.
pub mod text {
    use egui::FontId;

    pub fn body() -> FontId {
        FontId::proportional(12.5)
    }
    /// Part of the type scale, kept so the ramp stays complete even when no
    /// screen currently reaches for this step.
    #[allow(dead_code)]
    pub fn body_strong() -> FontId {
        FontId::proportional(13.5)
    }
    pub fn small() -> FontId {
        FontId::proportional(11.5)
    }
    pub fn caption() -> FontId {
        FontId::proportional(10.5)
    }
    pub fn mono() -> FontId {
        FontId::monospace(11.5)
    }
    pub fn translation() -> FontId {
        FontId::proportional(13.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative luminance, for a WCAG-style contrast check.
    fn luminance(c: Color32) -> f32 {
        let f = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        let (hi, lo) = if x > y { (x, y) } else { (y, x) };
        (hi + 0.05) / (lo + 0.05)
    }

    fn all_themes() -> Vec<(&'static str, Theme)> {
        vec![
            (
                "windows light",
                Theme::for_platform(Platform::Windows, false),
            ),
            ("windows dark", Theme::for_platform(Platform::Windows, true)),
            ("mac light", Theme::for_platform(Platform::MacOs, false)),
            ("mac dark", Theme::for_platform(Platform::MacOs, true)),
        ]
    }

    #[test]
    fn a_label_on_a_primary_button_is_readable() {
        // Fluent's dark accent is pale enough that white text on it fails, which
        // is exactly why `on_accent` is chosen per theme rather than shared.
        for (name, t) in all_themes() {
            let ratio = contrast(t.on_accent, t.accent);
            assert!(ratio >= 4.5, "{name}: accent contrast is only {ratio:.2}:1");
        }
    }

    #[test]
    fn body_text_is_readable_on_every_surface() {
        for (name, t) in all_themes() {
            for (surface_name, surface) in [("window", t.window), ("card", t.card)] {
                let ratio = contrast(t.text, surface);
                assert!(
                    ratio >= 7.0,
                    "{name}/{surface_name}: text contrast is only {ratio:.2}:1"
                );
            }
        }
    }

    #[test]
    fn dimmed_text_still_clears_the_minimum() {
        for (name, t) in all_themes() {
            let ratio = contrast(t.text_dim, t.card);
            assert!(
                ratio >= 4.0,
                "{name}: dimmed text contrast is only {ratio:.2}:1"
            );
        }
    }

    #[test]
    fn warning_and_danger_text_are_readable_on_a_card() {
        for (name, t) in all_themes() {
            for (label, color) in [("warning", t.warning), ("danger", t.danger)] {
                let ratio = contrast(color, t.card);
                assert!(
                    ratio >= 4.0,
                    "{name}/{label}: contrast is only {ratio:.2}:1"
                );
            }
        }
    }

    #[test]
    fn the_two_platforms_use_different_accents() {
        assert_ne!(
            Theme::for_platform(Platform::Windows, false).accent,
            Theme::for_platform(Platform::MacOs, false).accent
        );
    }

    #[test]
    fn the_brand_pink_is_the_same_on_both_platforms() {
        assert_eq!(
            Theme::for_platform(Platform::Windows, false).sakura,
            Theme::for_platform(Platform::MacOs, false).sakura
        );
    }
}
