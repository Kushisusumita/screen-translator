//! Platform look-and-feel.
//!
//! The design carries two rounds — Fluent for Windows 11, Aqua for macOS — and
//! each platform keeps its own: accent, navigation, switch, window controls, how
//! a shortcut is spelled, what the pages are called.
//!
//! What is *not* platform-specific is the surface treatment. Both rounds of the
//! mockup outline every row in its own bordered card; the app instead uses one
//! frameless group per section with faint dividers inside it. That is a
//! deliberate departure, applied identically on both platforms.

use crate::shared::i18n::t;
use crate::entities::settings::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
}

/// How the selected navigation entry is marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavStyle {
    /// macOS: the whole row fills with the accent colour.
    Filled,
    /// Windows 11: a faint fill plus a short accent bar on the leading edge.
    Indicator,
}

/// Window controls drawn by the app, since these windows have no system chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionStyle {
    /// macOS: three dots, top left.
    TrafficLights,
    /// Windows: minimise and close glyphs, top right.
    Buttons,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Windows
        }
    }

    pub const fn metrics(self) -> Metrics {
        match self {
            Platform::Windows => Metrics {
                surface_radius: 8.0,
                group_radius: 8.0,
                control_radius: 5.0,
                menu_radius: 8.0,
                row_height: 48.0,
                row_inset: 16.0,
                window_padding: 22.0,
                nav_width: 186.0,
                page_title_size: 19.0,
                toggle_size: (38.0, 19.0),
                toggle_dim_knob_when_off: true,
                nav_style: NavStyle::Indicator,
                caption: CaptionStyle::Buttons,
                row_subtitles: true,
                nav_search: true,
                shadow_alpha: 128,
            },
            Platform::MacOs => Metrics {
                surface_radius: 10.0,
                group_radius: 10.0,
                control_radius: 6.0,
                menu_radius: 8.0,
                row_height: 40.0,
                row_inset: 14.0,
                window_padding: 20.0,
                nav_width: 172.0,
                page_title_size: 15.0,
                toggle_size: (38.0, 22.0),
                toggle_dim_knob_when_off: false,
                nav_style: NavStyle::Filled,
                caption: CaptionStyle::TrafficLights,
                row_subtitles: false,
                nav_search: false,
                shadow_alpha: 90,
            },
        }
    }

    /// How a hotkey reads to a user of this platform: `⌥⇧T` versus `Alt+Shift+T`.
    ///
    /// `spaced` widens the Windows form to `Alt + Shift + T` for the key badge in
    /// the settings window, which is how Windows itself renders it there. Menus
    /// keep the compact form.
    pub fn format_hotkey_with(self, modifiers: u32, key_name: &str, spaced: bool) -> String {
        match self {
            Platform::MacOs => {
                let mut s = String::new();
                if modifiers & MOD_CONTROL != 0 {
                    s.push('⌃');
                }
                if modifiers & MOD_ALT != 0 {
                    s.push('⌥');
                }
                if modifiers & MOD_SHIFT != 0 {
                    s.push('⇧');
                }
                if modifiers & MOD_WIN != 0 {
                    s.push('⌘');
                }
                s.push_str(key_name);
                s
            }
            Platform::Windows => {
                let mut parts = Vec::new();
                if modifiers & MOD_CONTROL != 0 {
                    parts.push("Ctrl");
                }
                if modifiers & MOD_ALT != 0 {
                    parts.push("Alt");
                }
                if modifiers & MOD_SHIFT != 0 {
                    parts.push("Shift");
                }
                if modifiers & MOD_WIN != 0 {
                    parts.push("Win");
                }
                parts.push(key_name);
                parts.join(if spaced { " + " } else { "+" })
            }
        }
    }

    pub fn format_hotkey(self, modifiers: u32, key_name: &str) -> String {
        self.format_hotkey_with(modifiers, key_name, false)
    }

    /// The copy shortcut, as this platform's users know it.
    pub const fn copy_shortcut(self) -> &'static str {
        match self {
            Platform::Windows => "Ctrl+C",
            Platform::MacOs => "⌘C",
        }
    }

    /// The space bar, named the way the rest of the OS names it.
    pub fn space_key(self) -> &'static str {
        t("Space")
    }

    /// Candidate UI font files, best first. Missing files are skipped.
    ///
    /// Linux borrows the Fluent look — there is no third round of the design —
    /// but not the font paths, which would all be `C:\Windows\Fonts`. Falling
    /// through to egui's built-in face would cost the app every CJK glyph, and
    /// CJK is most of what it is asked to translate.
    pub const fn ui_font_candidates(self) -> &'static [&'static str] {
        #[cfg(all(unix, not(target_os = "macos")))]
        return &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/gnu-free/FreeSans.otf",
        ];

        #[allow(unreachable_code)]
        match self {
            Platform::Windows => &[
                r"C:\Windows\Fonts\SegUIVar.ttf",
                r"C:\Windows\Fonts\segoeui.ttf",
                r"C:\Windows\Fonts\tahoma.ttf",
            ],
            Platform::MacOs => &[
                "/System/Library/Fonts/SFNS.ttf",
                "/System/Library/Fonts/SFNSText.ttf",
                "/System/Library/Fonts/Helvetica.ttc",
            ],
        }
    }

    /// Monospace face for key badges and size labels.
    pub const fn mono_font_candidates(self) -> &'static [&'static str] {
        #[cfg(all(unix, not(target_os = "macos")))]
        return &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        ];

        #[allow(unreachable_code)]
        match self {
            Platform::Windows => &[
                r"C:\Windows\Fonts\consola.ttf",
                r"C:\Windows\Fonts\cour.ttf",
            ],
            Platform::MacOs => &[
                "/System/Library/Fonts/SFNSMono.ttf",
                "/System/Library/Fonts/Menlo.ttc",
            ],
        }
    }

    /// Candidate CJK fallbacks. The second element compensates for these fonts
    /// reporting a taller ascent than the Latin face, which otherwise makes
    /// kana float above the baseline inside a mixed run.
    pub const fn cjk_font_candidates(self) -> &'static [(&'static str, f32)] {
        #[cfg(all(unix, not(target_os = "macos")))]
        return &[
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0.10),
            ("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc", 0.10),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0.10),
            ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0.10),
        ];

        #[allow(unreachable_code)]
        match self {
            Platform::Windows => &[
                (r"C:\Windows\Fonts\meiryo.ttc", 0.15),
                (r"C:\Windows\Fonts\YuGothR.ttc", 0.10),
                (r"C:\Windows\Fonts\YuGothM.ttc", 0.10),
                (r"C:\Windows\Fonts\msgothic.ttc", 0.10),
                // Simplified Chinese and Korean, for the interface language as
                // much as for translated text: neither is covered by the
                // Japanese faces above.
                (r"C:\Windows\Fonts\msyh.ttc", 0.10),
                (r"C:\Windows\Fonts\malgun.ttf", 0.10),
            ],
            Platform::MacOs => &[
                ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0.06),
                ("/System/Library/Fonts/PingFang.ttc", 0.06),
                ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0.06),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    /// Floating popups and windows.
    pub surface_radius: f32,
    /// The grouped settings container.
    pub group_radius: f32,
    /// Buttons, chips, text fields.
    pub control_radius: f32,
    pub menu_radius: f32,
    /// Height of a settings row.
    pub row_height: f32,
    /// Horizontal padding inside the grouped container.
    pub row_inset: f32,
    pub window_padding: f32,
    pub nav_width: f32,
    pub page_title_size: f32,
    pub toggle_size: (f32, f32),
    /// Fluent greys the knob out when the switch is off; Aqua keeps it white.
    pub toggle_dim_knob_when_off: bool,
    pub nav_style: NavStyle,
    pub caption: CaptionStyle,
    /// Windows 11 Settings pairs every row with a one-line explanation.
    pub row_subtitles: bool,
    /// Windows 11 Settings puts a search field above the navigation.
    pub nav_search: bool,
    pub shadow_alpha: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::settings::{MOD_ALT, MOD_SHIFT};

    #[test]
    fn mac_uses_glyphs_windows_uses_words() {
        assert_eq!(
            Platform::MacOs.format_hotkey(MOD_ALT | MOD_SHIFT, "T"),
            "⌥⇧T"
        );
        assert_eq!(
            Platform::Windows.format_hotkey(MOD_ALT | MOD_SHIFT, "T"),
            "Alt+Shift+T"
        );
    }

    #[test]
    fn the_windows_key_badge_is_spaced_out() {
        assert_eq!(
            Platform::Windows.format_hotkey_with(MOD_ALT | MOD_SHIFT, "T", true),
            "Alt + Shift + T"
        );
    }

    #[test]
    fn spacing_does_not_apply_to_the_mac_glyph_form() {
        assert_eq!(
            Platform::MacOs.format_hotkey_with(MOD_ALT | MOD_SHIFT, "T", true),
            "⌥⇧T"
        );
    }

    #[test]
    fn norepeat_and_other_stray_bits_do_not_appear() {
        // MOD_NOREPEAT is an OS-level flag, not something the user pressed.
        let with_norepeat = MOD_CONTROL | 0x4000;
        assert_eq!(
            Platform::Windows.format_hotkey(with_norepeat, "T"),
            "Ctrl+T"
        );
    }

    #[test]
    fn each_platform_names_the_copy_shortcut_its_own_way() {
        assert_eq!(Platform::Windows.copy_shortcut(), "Ctrl+C");
        assert_eq!(Platform::MacOs.copy_shortcut(), "⌘C");
    }

    #[test]
    fn the_platform_identities_stay_distinct() {
        // The borderless treatment is shared, but everything that makes a
        // platform recognisable is not.
        let w = Platform::Windows.metrics();
        let m = Platform::MacOs.metrics();
        assert_ne!(w.nav_style, m.nav_style);
        assert_ne!(w.caption, m.caption);
        assert_ne!(w.row_subtitles, m.row_subtitles);
        assert_ne!(w.nav_search, m.nav_search);
        assert_ne!(w.toggle_dim_knob_when_off, m.toggle_dim_knob_when_off);
    }
}
