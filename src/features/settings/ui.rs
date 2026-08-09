//! The settings window.
//!
//! One screen per section, navigation on the left. The two platforms get their
//! own dialect of that shape, straight from the two rounds of the design: macOS
//! groups rows into one card with hairlines and fills the selected nav row with
//! the accent; Windows 11 gives every row its own card with an icon and a
//! one-line explanation, and marks the selected nav row with a short accent bar
//! next to a search field.
//!
//! None of that branching lives in this file — it all comes from
//! `theme.metrics`, and the screens below are written once.

use egui::{Color32, Sense, Vec2};

use crate::entities::history::{History, HistoryEntry};
use crate::entities::language::Language;
use crate::entities::settings::{
    AiProtocol, CaptureMode, EngineKind, Hotkey, HotkeyAction, ResultView, Settings, AI_PRESETS,
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
};
use crate::shared::i18n::{t, Lang};
use crate::shared::secret::{sealing_available, Secret};
use crate::ui::platform::Platform;
use crate::ui::theme::{text, ThemeMode};
use crate::ui::widgets::RowSpec;
use crate::ui::{icons, widgets, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Keys,
    Languages,
    Engine,
    Appearance,
    History,
    Logs,
    About,
}

impl Section {
    fn all() -> [Section; 8] {
        [
            Section::General,
            Section::Keys,
            Section::Languages,
            Section::Engine,
            Section::Appearance,
            Section::History,
            Section::Logs,
            Section::About,
        ]
    }

    /// Windows Settings names its pages in full; macOS System Settings keeps
    /// them to a word. Both designs show it, so both are here.
    fn label(self, platform: Platform) -> &'static str {
        match (self, platform) {
            (Section::General, _) => t("General"),
            (Section::Keys, Platform::Windows) => t("Keyboard shortcuts"),
            (Section::Keys, Platform::MacOs) => t("Keys"),
            (Section::Languages, _) => t("Languages"),
            (Section::Engine, Platform::Windows) => t("Translation engine"),
            (Section::Engine, Platform::MacOs) => t("Engine"),
            (Section::Appearance, _) => t("Appearance"),
            (Section::History, _) => t("History"),
            (Section::Logs, _) => t("Logs"),
            (Section::About, _) => t("About"),
        }
    }

    /// Shown under the page heading on Windows, where Settings always explains
    /// the page.
    fn caption(self) -> &'static str {
        match self {
            Section::General => t("App behaviour and the language pair"),
            Section::Keys => t("Work system-wide while the app is running"),
            Section::Languages => t("Which language to translate from and into"),
            Section::Engine => t("Engine order and access keys"),
            Section::Appearance => t("Theme and how the translation is shown"),
            Section::History => t("Recent translations from this session"),
            Section::Logs => t("What is written to disk, and for how long"),
            Section::About => t("Version, updates and links"),
        }
    }

    /// Extra words the search should match beyond the visible label.
    fn keywords(self) -> &'static str {
        match self {
            Section::General => t("autostart clipboard tray icon notifications history"),
            Section::Keys => t("hotkey shortcut keys keyboard tab"),
            Section::Languages => t("translate language source target"),
            Section::Engine => t("yandex google deepl ai openai anthropic gemini key token model"),
            Section::Appearance => t("theme light dark popup window appearance"),
            Section::History => t("history translations recent clear memory"),
            Section::Logs => t("log journal debug privacy"),
            Section::About => t("version update license author donate"),
        }
    }

    fn icon(self) -> widgets::IconFn {
        match self {
            Section::General => icons::gear,
            Section::Keys => icons::keyboard,
            Section::Languages => icons::globe,
            Section::Engine => icons::swap,
            Section::Appearance => icons::appearance,
            Section::History => icons::clock,
            Section::Logs => icons::journal,
            Section::About => icons::sakura,
        }
    }

    /// Only used by the macOS nav, which puts the icon on a coloured tile.
    fn tile(self) -> Color32 {
        match self {
            Section::General => Color32::from_rgb(0x8E, 0x8E, 0x93),
            Section::Keys => Color32::from_rgb(0x5E, 0x5C, 0xE6),
            Section::Languages => Color32::from_rgb(0x34, 0xC7, 0x59),
            Section::Engine => Color32::from_rgb(0xAF, 0x52, 0xDE),
            Section::Appearance => Color32::from_rgb(0x1A, 0x1A, 0x1E),
            Section::History => Color32::from_rgb(0x30, 0xB0, 0xC7),
            Section::Logs => Color32::from_rgb(0xFF, 0x9F, 0x0A),
            Section::About => Color32::from_rgb(0xE8, 0x7C, 0x9E),
        }
    }

    /// Lets a shortcut (or `--settings engine`) open straight to a page.
    pub fn from_name(name: &str) -> Option<Section> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "general" | "основные" | "общие" => Section::General,
            "keys" | "клавиши" => Section::Keys,
            "languages" | "языки" => Section::Languages,
            "engine" | "движок" => Section::Engine,
            "appearance" | "вид" => Section::Appearance,
            "history" | "история" => Section::History,
            "logs" | "журнал" => Section::Logs,
            "about" | "о-программе" => Section::About,
            _ => return None,
        })
    }

    fn matches(self, query: &str, platform: Platform) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        self.label(platform).to_lowercase().contains(&q) || self.keywords().contains(&q)
    }
}

/// Everything the window needs from the app but does not own.
pub struct SettingsContext<'a> {
    pub update_status: &'a str,
    pub update_check_enabled: bool,
    pub update_install_enabled: bool,
    /// How far the download has got, when one is running.
    pub update_progress: Option<f32>,
    pub ai_test_status: &'a str,
    pub ai_test_running: bool,
    pub rejected_hotkeys: &'a [HotkeyAction],
    pub log_dir: std::path::PathBuf,
    pub history: &'a History,
}

/// What the app has to act on after the frame.
#[derive(Debug, Default, Clone, Copy)]
pub struct SettingsOutput {
    pub autostart_changed: bool,
    pub hotkeys_changed: bool,
    pub engines_changed: bool,
    pub logs_changed: bool,
    pub tray_changed: bool,
    pub check_update: bool,
    pub install_update: bool,
    /// Platforms without a binary in the release open the page instead.
    pub open_release_page: bool,
    pub test_ai: bool,
    pub open_log_dir: bool,
    pub clear_history: bool,
}

pub struct SettingsUi {
    pub section: Section,
    recording: Option<HotkeyAction>,
    search: String,
    ai_key: String,
    deepl_key: String,
    reveal_keys: bool,
    synced: bool,
    /// Which history entry is expanded, if any. A capture is often a whole
    /// paragraph, and one truncated line is no way to read it back.
    ///
    /// Held by entry id rather than by position: new translations arrive at
    /// the front of the list, so an index would quietly come to mean a
    /// different entry the moment one did.
    open_entry: Option<u64>,
}

impl Default for SettingsUi {
    fn default() -> Self {
        Self::new(Section::General)
    }
}

impl SettingsUi {
    pub fn new(section: Section) -> Self {
        Self {
            section,
            recording: None,
            search: String::new(),
            ai_key: String::new(),
            deepl_key: String::new(),
            reveal_keys: false,
            synced: false,
            open_entry: None,
        }
    }

    /// Pulls the stored secrets into editable buffers. Called when the window
    /// opens so a key edited elsewhere is picked up.
    pub fn on_open(&mut self, settings: &Settings) {
        self.ai_key = settings.engines.ai_config.api_key.expose().to_string();
        self.deepl_key = settings.engines.deepl_key.expose().to_string();
        self.reveal_keys = false;
        self.recording = None;
        self.synced = true;
    }

    pub fn on_close(&mut self) {
        // Do not leave plaintext keys sitting in the UI struct for the rest of
        // the session.
        self.ai_key.clear();
        self.deepl_key.clear();
        self.synced = false;
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        theme: &Theme,
        settings: &mut Settings,
        info: &SettingsContext<'_>,
    ) -> SettingsOutput {
        if !self.synced {
            self.on_open(settings);
        }

        let mut out = SettingsOutput::default();
        let platform = theme.platform;

        egui::SidePanel::left("settings_nav")
            .exact_width(theme.metrics.nav_width)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(theme.chrome)
                    .inner_margin(egui::Margin::symmetric(8.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add_space(2.0);

                if theme.metrics.nav_search {
                    widgets::search_field(ui, theme, &mut self.search);
                    ui.add_space(8.0);
                }

                let mut shown = 0;
                for section in Section::all() {
                    if !section.matches(&self.search, platform) && section != self.section {
                        continue;
                    }
                    shown += 1;
                    if widgets::sidebar_item(
                        ui,
                        theme,
                        section.icon(),
                        section.tile(),
                        section.label(platform),
                        self.section == section,
                    )
                    .clicked()
                    {
                        self.section = section;
                        self.recording = None;
                    }
                }
                if shown == 0 {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(t("Nothing found"))
                            .font(text::caption())
                            .color(theme.text_faint),
                    );
                }
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme.window)
                    .inner_margin(egui::Margin::symmetric(22.0, 18.0)),
            )
            .show(ctx, |ui| {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(self.section.label(platform))
                        .font(egui::FontId::proportional(theme.metrics.page_title_size))
                        .strong()
                        .color(theme.text),
                );
                if theme.metrics.row_subtitles {
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(self.section.caption())
                            .font(text::small())
                            .color(theme.text_dim),
                    );
                }
                ui.add_space(12.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Breathing room at both ends of the scrolled content, so
                        // the first and last rows are not flush against the edge.
                        ui.add_space(2.0);
                        match self.section {
                            Section::General => self.general(ui, theme, settings, info, &mut out),
                            Section::Keys => self.keys(ui, theme, settings, info, &mut out),
                            Section::Languages => self.languages(ui, theme, settings),
                            Section::Engine => self.engine(ui, theme, settings, info, &mut out),
                            Section::Appearance => self.appearance(ui, theme, settings),
                            Section::History => self.history(ui, theme, settings, info, &mut out),
                            Section::Logs => self.logs(ui, theme, settings, info, &mut out),
                            Section::About => self.about(ui, theme, settings, info, &mut out),
                        }
                        ui.add_space(2.0);
                    });
            });

        out
    }

    // ── Общие ────────────────────────────────────────────────────────────────

    fn general(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        s: &mut Settings,
        _info: &SettingsContext<'_>,
        out: &mut SettingsOutput,
    ) {
        let startup_label = if theme.platform == Platform::Windows {
            t("Start when I sign in to Windows")
        } else {
            t("Start when I log in")
        };

        widgets::list(ui, theme, |ui| {
            widgets::row(
                ui,
                theme,
                RowSpec::new(startup_label)
                    .icon(icons::startup)
                    .subtitle(t("Starts minimised to the notification area")),
                |ui| {
                    if widgets::toggle(ui, theme, &mut s.launch_at_startup).changed() {
                        out.autostart_changed = true;
                    }
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Hide the notification area icon"))
                    .icon(icons::bell)
                    .subtitle(t("Access by keyboard shortcut only")),
                |ui| {
                    if widgets::toggle(ui, theme, &mut s.hide_tray_icon).changed() {
                        out.tray_changed = true;
                    }
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Copy the translation to the clipboard"))
                    .icon(icons::clipboard)
                    .subtitle(t("As soon as the text is recognised")),
                |ui| {
                    widgets::toggle(ui, theme, &mut s.copy_to_clipboard);
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Interface language"))
                    .icon(icons::globe)
                    .subtitle(t("Restart is not needed — the change applies at once")),
                |ui| {
                    ui_language_picker(ui, &mut s.ui_language);
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Translation languages"))
                    .icon(icons::globe)
                    .subtitle(t("Detect the source language automatically")),
                |ui| {
                    let w = ((ui.available_width() - 26.0) / 2.0).clamp(84.0, 130.0);
                    compact_lang_picker(ui, "gen_tgt", w, &mut s.target_lang, false);
                    ui.label(
                        egui::RichText::new("→")
                            .font(text::small())
                            .color(theme.text_dim),
                    );
                    compact_lang_picker(ui, "gen_src", w, &mut s.source_lang, true);
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Translation engine"))
                    .icon(icons::swap)
                    .subtitle(&engine_summary(s))
                    .last(),
                |ui| {
                    if widgets::ghost_button(ui, theme, t("Configure")).clicked() {
                        self.section = Section::Engine;
                    }
                },
            );
        });

        if s.hide_tray_icon {
            notice(
                ui,
                theme,
                theme.warning,
                &t("Without the icon this window can only be reopened by starting the app \
                    with --settings. Capture stays on {hotkey}.")
                .replace("{hotkey}", &s.hotkeys.region.display()),
            );
        }
        if s.produces_no_output() {
            notice(
                ui,
                theme,
                theme.warning,
                t("Nothing will happen: the result is neither shown nor copied. \
                   Turn on copying or pick a way to show the result."),
            );
        }

        widgets::section_caption(ui, theme, t("Default capture mode"));
        widgets::list(ui, theme, |ui| {
            let modes = CaptureMode::all();
            for (i, mode) in modes.into_iter().enumerate() {
                let spec = RowSpec::new(mode.label_menu());
                let spec = if i == modes.len() - 1 {
                    spec.last()
                } else {
                    spec
                };
                widgets::row(ui, theme, spec, |ui| {
                    if radio_dot(ui, theme, s.capture_mode == mode).clicked() {
                        s.capture_mode = mode;
                    }
                });
            }
        });
    }

    // ── Сочетания клавиш ─────────────────────────────────────────────────────

    fn keys(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        s: &mut Settings,
        info: &SettingsContext<'_>,
        out: &mut SettingsOutput,
    ) {
        if let Some(action) = self.recording {
            if let Some(hk) = capture_combination(ui.ctx()) {
                // `None` here means the user pressed Escape to cancel.
                if let Some(hk) = hk {
                    *s.hotkeys.slot_mut(action) = hk;
                    out.hotkeys_changed = true;
                }
                self.recording = None;
            }
        }

        widgets::list(ui, theme, |ui| {
            let actions = HotkeyAction::all();
            for (i, action) in actions.into_iter().enumerate() {
                let recording = self.recording == Some(action);
                let hk = *s.hotkeys.slot_mut(action);
                let spec = RowSpec::new(action.label()).highlighted(recording);
                let spec = if i == actions.len() - 1 {
                    spec.last()
                } else {
                    spec
                };

                widgets::row(ui, theme, spec, |ui| {
                    // Right-to-left layout: the trailing control is added first.
                    if recording {
                        if widgets::ghost_button(ui, theme, t("Cancel")).clicked() {
                            self.recording = None;
                        }
                        widgets::hotkey_badge(ui, theme, t("press a shortcut…"), true);
                    } else {
                        // Reading right to left, so this lays out as
                        // badge · Change · Clear.
                        if hk.is_bound() && widgets::ghost_button(ui, theme, t("Clear")).clicked() {
                            *s.hotkeys.slot_mut(action) = Hotkey::unbound();
                            out.hotkeys_changed = true;
                        }
                        if widgets::ghost_button(
                            ui,
                            theme,
                            if hk.is_bound() {
                                t("Change")
                            } else {
                                t("Set")
                            },
                        )
                        .clicked()
                        {
                            self.recording = Some(action);
                        }
                        if hk.is_bound() {
                            let label = theme.platform.format_hotkey_with(
                                hk.modifiers,
                                crate::entities::settings::vk_name(hk.key),
                                true,
                            );
                            if widgets::hotkey_badge(ui, theme, &label, false).clicked() {
                                self.recording = Some(action);
                            }
                        } else if widgets::unbound_badge(ui, theme, t("not set")).clicked() {
                            self.recording = Some(action);
                        }
                    }
                });
            }
        });

        for (a, b) in s.hotkeys.conflicts() {
            notice(
                ui,
                theme,
                theme.warning,
                &t("\"{first}\" and \"{second}\" are on the same shortcut — only one will fire.")
                    .replace("{first}", a.label())
                    .replace("{second}", b.label()),
            );
        }
        for action in info.rejected_hotkeys {
            notice(
                ui,
                theme,
                theme.danger,
                &t("The shortcut for \"{action}\" is taken by another program. \
                    The previous one still applies.")
                .replace("{action}", action.label()),
            );
        }

        widgets::section_caption(ui, theme, t("While capturing"));
        widgets::list(ui, theme, |ui| {
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Show the mode switcher (Tab)"))
                    .subtitle(t("Region · Window · Full screen over the dimmed overlay")),
                |ui| {
                    widgets::toggle(ui, theme, &mut s.show_mode_hud);
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Sound when text is captured"))
                    .subtitle(t("A short beep once the region is captured"))
                    .last(),
                |ui| {
                    widgets::toggle(ui, theme, &mut s.play_sound);
                },
            );
        });

        hint(
            ui,
            theme,
            t("Shortcuts work in any program, including full-screen games."),
        );
    }

    // ── Языки ────────────────────────────────────────────────────────────────

    fn languages(&mut self, ui: &mut egui::Ui, theme: &Theme, s: &mut Settings) {
        widgets::card(ui, theme, |ui| {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let w = (ui.available_width() - 62.0) / 2.0;
                lang_picker(ui, "src", w, &mut s.source_lang, true);
                ui.add_space(6.0);
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                ui.painter()
                    .circle_filled(rect.center(), 14.0, theme.hover_fill());
                icons::swap(ui.painter(), rect.shrink(7.0), theme.text);
                if resp.on_hover_text(t("Swap")).clicked()
                    && s.source_lang != Language::Auto
                {
                    std::mem::swap(&mut s.source_lang, &mut s.target_lang);
                }
                ui.add_space(6.0);
                lang_picker(ui, "tgt", w, &mut s.target_lang, false);
                ui.add_space(14.0);
            });
            ui.add_space(12.0);
        });

        if s.source_lang == Language::Auto {
            hint(
                ui,
                theme,
                t("The source language is detected from the recognised text, which is more \
                   accurate than setting it by hand."),
            );
        } else if s.source_lang == s.target_lang {
            notice(
                ui,
                theme,
                theme.warning,
                t("The source and target languages are the same — the text will come back \
                   unchanged."),
            );
        }
    }

    // ── Движок ───────────────────────────────────────────────────────────────

    fn engine(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        s: &mut Settings,
        info: &SettingsContext<'_>,
        out: &mut SettingsOutput,
    ) {
        hint(
            ui,
            theme,
            t("Engines are tried from the top down until one answers."),
        );
        ui.add_space(6.0);

        let order: Vec<EngineKind> = {
            let mut o = s.engines.order.clone();
            for k in EngineKind::all() {
                if !o.contains(&k) {
                    o.push(k);
                }
            }
            o
        };

        let mut move_up: Option<usize> = None;
        for (i, kind) in order.iter().enumerate() {
            let kind = *kind;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // Reserve the trailing reorder button before measuring the card.
                // The slot is reserved even for the first row, which has no
                // button, so every card comes out the same width.
                const REORDER_SLOT: f32 = 30.0;
                let card_width =
                    ui.available_width() - theme.metrics.toggle_size.0 - REORDER_SLOT - 24.0;

                let mut on = s.engines.is_enabled(kind);
                if widgets::toggle(ui, theme, &mut on).changed() {
                    s.engines.set_enabled(kind, on);
                    out.engines_changed = true;
                }
                let selected = on && s.engines.is_configured(kind);
                let resp = widgets::select_card(
                    ui,
                    theme,
                    widgets::SelectCard::new(kind.label(), s.engines.status(kind))
                        .selected(selected)
                        .ready(kind.needs_key() && s.engines.is_configured(kind))
                        .width(card_width),
                );
                if resp.clicked() {
                    s.engines.set_enabled(kind, !on);
                    out.engines_changed = true;
                }
                if i > 0 {
                    if ui
                        .small_button("↑")
                        .on_hover_text(t("Move up the queue"))
                        .clicked()
                    {
                        move_up = Some(i);
                    }
                } else {
                    // Keeps the first card the same width as the rest.
                    ui.add_space(REORDER_SLOT - 8.0);
                }
            });
            ui.add_space(6.0);
        }

        if let Some(i) = move_up {
            let mut o = order;
            o.swap(i - 1, i);
            s.engines.order = o;
            out.engines_changed = true;
        }

        widgets::section_caption(ui, theme, "DeepL");
        widgets::list(ui, theme, |ui| {
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("API key"))
                    .subtitle(t("Hidden; a free-plan key ends in :fx"))
                    .last(),
                |ui| {
                    if secret_field(ui, "deepl_key", &mut self.deepl_key, self.reveal_keys) {
                        s.engines.deepl_key = Secret::new(self.deepl_key.trim());
                        out.engines_changed = true;
                    }
                },
            );
        });

        widgets::section_caption(ui, theme, t("AI translator"));
        hint(
            ui,
            theme,
            t("Any model with a token, cloud or local. Pick a preset or enter the address \
               and model by hand."),
        );
        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            for preset in AI_PRESETS {
                let active = s.engines.ai_config.preset_name == preset.name;
                if preset_chip(ui, theme, preset.display_name(), active).clicked() {
                    s.engines.ai_config.apply_preset(preset);
                    out.engines_changed = true;
                }
            }
        });
        ui.add_space(8.0);

        widgets::list(ui, theme, |ui| {
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Protocol"))
                    .subtitle(t("The request format the address understands")),
                |ui| {
                    egui::ComboBox::from_id_salt("ai_protocol")
                        .selected_text(s.engines.ai_config.protocol.label())
                        .width(190.0)
                        .show_ui(ui, |ui| {
                            for p in AiProtocol::all() {
                                if ui
                                    .selectable_value(
                                        &mut s.engines.ai_config.protocol,
                                        p,
                                        p.label(),
                                    )
                                    .clicked()
                                {
                                    out.engines_changed = true;
                                }
                            }
                        });
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("API address")).subtitle(t("Base URL without a trailing path")),
                |ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut s.engines.ai_config.base_url)
                                .desired_width(field_width(ui))
                                .hint_text("https://api.example.com/v1"),
                        )
                        .changed()
                    {
                        out.engines_changed = true;
                    }
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Model")).subtitle(t("The model id at the provider")),
                |ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut s.engines.ai_config.model)
                                .desired_width(field_width(ui))
                                .hint_text("gpt-4o-mini"),
                        )
                        .changed()
                    {
                        out.engines_changed = true;
                    }
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Token"))
                    .subtitle(t("Hidden and stored encrypted; localhost needs no key")),
                |ui| {
                    if secret_field(ui, "ai_key", &mut self.ai_key, self.reveal_keys) {
                        s.engines.ai_config.api_key = Secret::new(self.ai_key.trim());
                        out.engines_changed = true;
                    }
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Show keys")).subtitle(t("Unmasks the fields above")),
                |ui| {
                    widgets::toggle(ui, theme, &mut self.reveal_keys);
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Temperature")).subtitle(t("Lower is more literal and predictable")),
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut s.engines.ai_config.temperature)
                            .range(0.0..=1.5)
                            .speed(0.05)
                            .fixed_decimals(2),
                    );
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Timeout, s"))
                    .subtitle(t("How long to wait for an answer before moving to the next engine"))
                    .last(),
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut s.engines.ai_config.timeout_secs)
                            .range(5..=300)
                            .speed(1.0),
                    );
                },
            );
        });

        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(t("Extra instructions for the model"))
                .font(text::small())
                .color(theme.text_dim),
        );
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::multiline(&mut s.engines.ai_config.extra_instructions)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text(t("For example: keep it informal, do not translate command names")),
        );

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_enabled_ui(
                !info.ai_test_running && s.engines.ai_config.is_usable(),
                |ui| {
                    if widgets::primary_button(ui, theme, t("Test connection")).clicked() {
                        out.test_ai = true;
                    }
                },
            );
            if !info.ai_test_status.is_empty() {
                ui.label(
                    egui::RichText::new(info.ai_test_status)
                        .font(text::small())
                        .color(if info.ai_test_status.starts_with('✓') {
                            theme.success
                        } else {
                            theme.danger
                        }),
                );
            }
        });

        if !sealing_available() {
            notice(
                ui,
                theme,
                theme.warning,
                t("On this platform the token is saved in the settings file as-is — \
                   key encryption is implemented for Windows only so far."),
            );
        }

        widgets::section_caption(ui, theme, t("Advanced"));
        widgets::list(ui, theme, |ui| {
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Use Yandex through a browser when the API fails"))
                    .subtitle(t("Runs Chrome in the background and adds a few seconds"))
                    .last(),
                |ui| {
                    if widgets::toggle(ui, theme, &mut s.engines.yandex_headless_fallback).changed()
                    {
                        out.engines_changed = true;
                    }
                },
            );
        });
    }

    // ── Внешний вид ──────────────────────────────────────────────────────────

    fn appearance(&mut self, ui: &mut egui::Ui, theme: &Theme, s: &mut Settings) {
        widgets::list(ui, theme, |ui| {
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Theme"))
                    .icon(icons::appearance)
                    .subtitle(t("Light, dark or match the system"))
                    .last(),
                |ui| {
                    egui::ComboBox::from_id_salt("theme_mode")
                        .selected_text(s.theme.label())
                        .width(170.0)
                        .show_ui(ui, |ui| {
                            for m in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
                                ui.selectable_value(&mut s.theme, m, m.label());
                            }
                        });
                },
            );
        });

        widgets::section_caption(ui, theme, t("How to show the translation"));
        for view in ResultView::all() {
            let selected = s.result_view == view;
            let resp = view_card(ui, theme, view.label(), view.description(), selected);
            if resp.clicked() {
                s.result_view = view;
            }
            ui.add_space(6.0);
        }

        widgets::section_caption(ui, theme, t("Result window"));
        widgets::list(ui, theme, |ui| {
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Keep on top of other windows"))
                    .subtitle(t("The translation window stays in front")),
                |ui| {
                    widgets::toggle(ui, theme, &mut s.pin_result_window);
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Close when it loses focus"))
                    .subtitle(t("Clicking outside dismisses the translation"))
                    .last(),
                |ui| {
                    widgets::toggle(ui, theme, &mut s.close_result_on_focus_loss);
                },
            );
        });
    }

    // ── История ─────────────────────────────────────────────────────────────

    fn history(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        s: &mut Settings,
        info: &SettingsContext<'_>,
        out: &mut SettingsOutput,
    ) {
        widgets::list(ui, theme, |ui| {
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Remember translations"))
                    .subtitle(t("In memory only — nothing is written to disk")),
                |ui| {
                    widgets::toggle(ui, theme, &mut s.keep_history);
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("How many translations to keep"))
                    .subtitle(t("The oldest are pushed out by new ones"))
                    .last(),
                |ui| {
                    ui.add_enabled_ui(s.keep_history, |ui| {
                        let mut n = s.history_limit as u32;
                        if ui
                            .add(egui::DragValue::new(&mut n).range(1..=500).speed(1.0))
                            .changed()
                        {
                            s.history_limit = n as usize;
                        }
                    });
                },
            );
        });

        if !s.keep_history {
            hint(
                ui,
                theme,
                t("History is off. Translations are not saved anywhere."),
            );
            return;
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(
                    t("Recent translations · {count}")
                        .replace("{count}", &info.history.len().to_string()),
                )
                .font(text::small())
                .color(theme.text_dim),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!info.history.is_empty(), |ui| {
                    if widgets::ghost_button(ui, theme, t("Clear history")).clicked() {
                        out.clear_history = true;
                    }
                });
            });
        });
        ui.add_space(6.0);

        if info.history.is_empty() {
            hint(ui, theme, t("Nothing has been translated in this session yet."));
            return;
        }

        // The whole list, not the first six: this is the page for it now, and
        // it scrolls with the rest of the page.
        let mut toggled = None;
        widgets::card(ui, theme, |ui| {
            let total = info.history.len();
            for (i, entry) in info.history.iter().enumerate() {
                let open = self.open_entry == Some(entry.id);
                if history_row(ui, theme, entry, i + 1 == total && !open, open).clicked() {
                    toggled = Some(entry.id);
                }
                if open {
                    history_detail(ui, theme, entry, i + 1 == total);
                }
            }
        });
        if let Some(id) = toggled {
            // Clicking the open one closes it, so the list can be collapsed back
            // without hunting for a control.
            self.open_entry = if self.open_entry == Some(id) { None } else { Some(id) };
        }

        if let Some(latest) = info.history.latest() {
            hint(
                ui,
                theme,
                &t("The last translation came from {engine} ({from} → {to}).")
                    .replace("{engine}", latest.engine.label())
                    .replace("{from}", latest.source.badge())
                    .replace("{to}", latest.target.badge()),
            );
        }
    }

    // ── Журнал ───────────────────────────────────────────────────────────────

    fn logs(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        s: &mut Settings,
        info: &SettingsContext<'_>,
        out: &mut SettingsOutput,
    ) {
        widgets::list(ui, theme, |ui| {
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Keep for days"))
                    .icon(icons::journal)
                    .subtitle(t("Older files are deleted automatically")),
                |ui| {
                    let mut n = s.logs.retention_days;
                    if ui
                        .add(egui::DragValue::new(&mut n).range(1..=30).speed(1.0))
                        .changed()
                    {
                        s.logs.retention_days = n;
                        out.logs_changed = true;
                    }
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Daily limit, MB"))
                    .subtitle(t("Past the limit, entries are dropped")),
                |ui| {
                    let mut n = s.logs.max_mb_per_day;
                    if ui
                        .add(egui::DragValue::new(&mut n).range(1..=256).speed(1.0))
                        .changed()
                    {
                        s.logs.max_mb_per_day = n;
                        out.logs_changed = true;
                    }
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Write the recognised text to the log"))
                    .subtitle(t("Only for tracking down a bug"))
                    .last(),
                |ui| {
                    if widgets::toggle(ui, theme, &mut s.logs.verbose).changed() {
                        out.logs_changed = true;
                    }
                },
            );
        });

        if s.logs.verbose {
            notice(
                ui,
                theme,
                theme.warning,
                t("The log will contain every recognised and translated line, which means \
                   whatever is on your screen."),
            );
        }

        hint(
            ui,
            theme,
            t("Each day goes into its own file and old ones are deleted automatically, \
               so the log does not grow."),
        );

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if widgets::secondary_button(ui, theme, t("Open the log folder")).clicked() {
                out.open_log_dir = true;
            }
        });
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(info.log_dir.display().to_string())
                .font(text::caption())
                .color(theme.text_faint),
        );

        if out.logs_changed {
            hint(
                ui,
                theme,
                t("The new values take effect on the next launch."),
            );
        }
    }

    // ── О программе ──────────────────────────────────────────────────────────

    fn about(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        s: &mut Settings,
        info: &SettingsContext<'_>,
        out: &mut SettingsOutput,
    ) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(42.0), Sense::hover());
            icons::sakura(ui.painter(), rect, theme.sakura);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Sakura Screen Translator")
                        .font(egui::FontId::proportional(17.0))
                        .strong()
                        .color(theme.text),
                );
                ui.label(
                    egui::RichText::new(
                        t("Version {version}").replace("{version}", env!("CARGO_PKG_VERSION")),
                    )
                    .font(text::small())
                    .color(theme.text_dim),
                );
            });
        });

        widgets::section_caption(ui, theme, t("Updates"));
        widgets::list(ui, theme, |ui| {
            widgets::row(
                ui,
                theme,
                RowSpec::new(t("Notify me about new versions"))
                    .subtitle(t("A system notification when an update is released")),
                |ui| {
                    widgets::toggle(ui, theme, &mut s.notify_about_updates);
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new(info.update_status)
                    .subtitle(t("Downloaded only from this project's GitHub"))
                    .last(),
                |ui| {
                    ui.add_enabled_ui(info.update_check_enabled, |ui| {
                        if widgets::ghost_button(ui, theme, t("Check")).clicked() {
                            out.check_update = true;
                        }
                    });
                    // Only Windows has a binary to install: the release
                    // carries a .exe and nothing else. Offering "Install"
                    // elsewhere downloads ten megabytes to be rejected by the
                    // signature check at the end of it.
                    if cfg!(windows) {
                        ui.add_enabled_ui(info.update_install_enabled, |ui| {
                            if widgets::primary_button(ui, theme, t("Install")).clicked() {
                                out.install_update = true;
                            }
                        });
                    } else {
                        ui.add_enabled_ui(info.update_install_enabled, |ui| {
                            if widgets::primary_button(ui, theme, t("Open the release")).clicked()
                            {
                                out.open_release_page = true;
                            }
                        });
                    }
                },
            );
        });

        if let Some(fraction) = info.update_progress {
            ui.add_space(6.0);
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_height(6.0)
                    .fill(theme.accent)
                    .rounding(egui::Rounding::same(3.0)),
            );
        }

        widgets::section_caption(ui, theme, t("Links"));
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t("Source code"))
                    .font(text::small())
                    .color(theme.text_dim),
            );
            ui.hyperlink_to(
                "github.com/Kushisusumita/screen-translator",
                "https://github.com/Kushisusumita/screen-translator",
            );
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t("Author"))
                    .font(text::small())
                    .color(theme.text_dim),
            );
            ui.hyperlink_to("クシススミタ", "https://github.com/Kushisusumita");
        });

        widgets::section_caption(ui, theme, t("Support the project"));
        // Two labels rather than one string with a newline: a line continuation
        // inside a translated sentence is easy to get subtly wrong, and did.
        for line in [
            t("The app is free and will stay free."),
            t("Supporting its development, or just saying thanks, is up to you."),
        ] {
            ui.label(
                egui::RichText::new(line)
                    .font(text::small())
                    .color(theme.text_dim),
            );
        }
        ui.add_space(9.0);

        const BINANCE_URL: &str = "https://app.binance.com/uni-qr/5gjTx7at";
        const USDT_ADDRESS: &str = "TNwbTzUgk2F11PDfqP8J9fpZCAN3K4yXPQ";

        widgets::list(ui, theme, |ui| {
            widgets::row(
                ui,
                theme,
                RowSpec::new("Binance Pay")
                    .icon(icons::binance)
                    // Brand colours: a brand mark in the interface's own grey
                    // is not recognisable as the brand.
                    .icon_tint(Color32::from_rgb(0xF0, 0xB9, 0x0B))
                    .subtitle(t("Pay by QR link, no fee")),
                |ui| {
                    if widgets::secondary_button(ui, theme, t("Open")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(BINANCE_URL));
                    }
                },
            );
            widgets::row(
                ui,
                theme,
                RowSpec::new("USDT · TRC20")
                    .icon(icons::tether)
                    .icon_tint(Color32::from_rgb(0x26, 0xA1, 0x7B))
                    .subtitle(t("No Binance account needed"))
                    .last(),
                |ui| {
                    if widgets::secondary_button(ui, theme, t("Copy address")).clicked() {
                        ui.output_mut(|o| o.copied_text = USDT_ADDRESS.to_string());
                    }
                    ui.label(
                        egui::RichText::new(USDT_ADDRESS)
                            .font(text::mono())
                            .color(theme.text_dim),
                    );
                },
            );
        });
    }
}

// ── Small building blocks ────────────────────────────────────────────────────

/// One-line summary of the translation chain, for the General page.
fn engine_summary(s: &Settings) -> String {
    let active = s.engines.active();
    match active.first() {
        None => t("No engine is ready").to_string(),
        Some(first) => {
            let rest = active.len().saturating_sub(1);
            let head = format!("{} · {}", first.label(), s.engines.status(*first));
            if rest == 0 {
                head
            } else {
                t("{engines}, then {count} more")
                    .replace("{engines}", &head)
                    .replace("{count}", &rest.to_string())
            }
        }
    }
}

fn hint(ui: &mut egui::Ui, theme: &Theme, msg: &str) {
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(msg)
            .font(text::caption())
            .color(theme.text_faint),
    );
}

fn notice(ui: &mut egui::Ui, theme: &Theme, color: Color32, msg: &str) {
    ui.add_space(8.0);
    egui::Frame::none()
        .fill(theme.tint(color, if theme.dark { 40 } else { 28 }))
        .rounding(theme.group_rounding())
        .inner_margin(egui::Margin::symmetric(13.0, 10.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
                warning_glyph(ui.painter(), rect, color);
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(msg)
                        .font(text::small())
                        .color(if theme.dark { theme.text } else { color }),
                );
            });
        });
}

/// Exclamation in a triangle, drawn rather than relying on the ⚠ glyph.
fn warning_glyph(p: &egui::Painter, rect: egui::Rect, color: Color32) {
    let stroke = egui::Stroke::new(1.3, color);
    let top = egui::pos2(rect.center().x, rect.min.y + 1.0);
    let left = egui::pos2(rect.min.x + 1.0, rect.max.y - 2.0);
    let right = egui::pos2(rect.max.x - 1.0, rect.max.y - 2.0);
    p.line_segment([top, left], stroke);
    p.line_segment([left, right], stroke);
    p.line_segment([right, top], stroke);
    p.line_segment(
        [
            egui::pos2(rect.center().x, rect.min.y + 5.0),
            egui::pos2(rect.center().x, rect.max.y - 6.0),
        ],
        stroke,
    );
    p.circle_filled(egui::pos2(rect.center().x, rect.max.y - 4.0), 0.9, color);
}

/// Frameless radio: a filled disc for the chosen row, a soft well for the rest.
fn radio_dot(ui: &mut egui::Ui, theme: &Theme, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    let c = rect.center();
    let p = ui.painter();
    if selected {
        p.circle_filled(c, 8.0, theme.accent);
        p.circle_filled(c, 3.2, theme.on_accent);
    } else {
        p.circle_filled(
            c,
            8.0,
            if resp.hovered() {
                theme.control_fill_hover()
            } else {
                theme.control_fill()
            },
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn lang_picker(ui: &mut egui::Ui, id: &str, width: f32, lang: &mut Language, allow_auto: bool) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(lang.name_ru())
        .width(width)
        .show_ui(ui, |ui| {
            if allow_auto {
                for l in Language::all() {
                    ui.selectable_value(lang, *l, l.name_ru());
                }
            } else {
                // A target language has to be concrete; `targets()` is the list
                // that guarantees it.
                for l in Language::targets() {
                    ui.selectable_value(lang, l, l.name_ru());
                }
            }
        });
}

/// Narrow variant that shows "Авто" rather than the full sentence.
fn compact_lang_picker(
    ui: &mut egui::Ui,
    id: &str,
    width: f32,
    lang: &mut Language,
    allow_auto: bool,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(lang.short_ru())
        .width(width)
        .show_ui(ui, |ui| {
            if allow_auto {
                for l in Language::all() {
                    ui.selectable_value(lang, *l, l.name_ru());
                }
            } else {
                for l in Language::targets() {
                    ui.selectable_value(lang, l, l.name_ru());
                }
            }
        });
}

fn preset_chip(ui: &mut egui::Ui, theme: &Theme, label: &str, active: bool) -> egui::Response {
    let fg = if active { theme.on_accent } else { theme.text };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), text::small(), fg);
    let (rect, resp) =
        ui.allocate_exact_size(galley.size() + Vec2::new(20.0, 12.0), Sense::click());
    let bg = if active {
        theme.accent
    } else if resp.is_pointer_button_down_on() {
        theme.control_fill_active()
    } else if resp.hovered() {
        theme.control_fill_hover()
    } else {
        theme.control_fill()
    };
    ui.painter().rect_filled(rect, theme.control_rounding(), bg);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn view_card(
    ui: &mut egui::Ui,
    theme: &Theme,
    title: &str,
    body: &str,
    selected: bool,
) -> egui::Response {
    widgets::select_card(
        ui,
        theme,
        widgets::SelectCard::new(title, body).selected(selected),
    )
}

/// One line of the recent-translations list: the original, dimmed, with the
/// translation under it.
/// One line of the history list: the original above, the translation below.
///
/// Both are laid out to a single row and truncated with an ellipsis. Painting
/// them as raw text — which this did — puts a multi-line OCR result on screen
/// as multiple lines inside a 42-point row, so it ran over the row below it,
/// over the caption, and out of the card.
///
/// The row is clickable: a truncated line is enough to *find* a translation and
/// nowhere near enough to *read* one, so clicking opens the full text below it.
fn history_row(
    ui: &mut egui::Ui,
    theme: &Theme,
    entry: &HistoryEntry,
    last: bool,
    open: bool,
) -> egui::Response {
    const HEIGHT: f32 = 46.0;
    const PAD: f32 = 14.0;
    /// Room for the target-language badge and the chevron on the right.
    const TRAILING: f32 = 58.0;

    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), HEIGHT),
        Sense::click(),
    );
    let width = (rect.width() - PAD * 2.0 - TRAILING).max(60.0);

    let original = one_line(ui, &entry.original, text::caption(), theme.text_faint, width);
    let translated = one_line(ui, &entry.translated, text::small(), theme.text, width);

    let painter = ui.painter();
    // Inset and rounded rather than a full-bleed rectangle: the card it sits
    // in has rounded corners of its own, and a highlight running to the edge
    // squares them off on the first and last row. The open row gets a faint
    // wash of the brand colour so it stays marked once the pointer leaves.
    let bg = rect.shrink2(Vec2::new(5.0, 2.0));
    if open {
        painter.rect_filled(bg, 7.0, theme.tint(theme.sakura, 20));
    } else if response.hovered() {
        painter.rect_filled(bg, 7.0, theme.hover_fill());
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    painter.galley(
        egui::pos2(rect.min.x + PAD, rect.min.y + 7.0),
        original,
        theme.text_faint,
    );
    painter.galley(
        egui::pos2(rect.min.x + PAD, rect.min.y + 24.0),
        translated,
        theme.text,
    );
    painter.text(
        egui::pos2(rect.max.x - PAD - 18.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        entry.target.badge(),
        text::caption(),
        theme.sakura_deep,
    );
    // Chevron: down when the entry is open, right when it is not.
    let cx = rect.max.x - PAD - 4.0;
    let cy = rect.center().y;
    let s = egui::Stroke::new(1.4, theme.text_dim);
    if open {
        painter.line_segment([egui::pos2(cx - 5.0, cy - 2.0), egui::pos2(cx, cy + 3.0)], s);
        painter.line_segment([egui::pos2(cx, cy + 3.0), egui::pos2(cx + 5.0, cy - 2.0)], s);
    } else {
        painter.line_segment([egui::pos2(cx - 2.0, cy - 5.0), egui::pos2(cx + 3.0, cy)], s);
        painter.line_segment([egui::pos2(cx + 3.0, cy), egui::pos2(cx - 2.0, cy + 5.0)], s);
    }
    if !last {
        painter.hline(
            (rect.min.x + PAD)..=rect.max.x,
            rect.max.y,
            egui::Stroke::new(1.0, theme.separator),
        );
    }
    response
}

/// The expanded entry: the whole original and the whole translation, scrollable,
/// with a button to put the translation back on the clipboard.
///
/// Bounded in height so one long capture cannot push the rest of the list off
/// the page — inside that box the text scrolls, however much of it there is.
fn history_detail(ui: &mut egui::Ui, theme: &Theme, entry: &HistoryEntry, last: bool) {
    const MAX_HEIGHT: f32 = 220.0;

    egui::Frame::none()
        .inner_margin(egui::Margin {
            left: 14.0,
            right: 14.0,
            top: 2.0,
            bottom: 10.0,
        })
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                // Vertical only, and never wider than the block: without this
                // the labels lay themselves out on one endless line and the
                // text runs under the scrollbar instead of wrapping.
                .auto_shrink([false, false])
                // Keyed by the entry, so a scroll position belongs to the
                // text it was scrolled through.
                .id_salt(entry.id)
                .max_height(MAX_HEIGHT)
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(
                            t("{lang} · original").replace("{lang}", entry.source.badge()),
                        )
                        .font(text::caption())
                        .color(theme.text_faint),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(entry.original.trim())
                            .font(text::small())
                            .color(theme.text_dim),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            t("{lang} · translation").replace("{lang}", entry.target.badge()),
                        )
                        .font(text::caption())
                        .color(theme.sakura_deep),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(entry.translated.trim())
                            .font(text::body())
                            .color(theme.text),
                    );
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::secondary_button(ui, theme, t("Copy translation")).clicked() {
                    ui.output_mut(|o| o.copied_text = entry.translated.clone());
                }
                ui.label(
                    egui::RichText::new(
                        t("{engine} · {count} characters")
                            .replace("{engine}", entry.engine.label())
                            .replace("{count}", &entry.translated.chars().count().to_string()),
                    )
                    .font(text::caption())
                    .color(theme.text_faint),
                );
            });
        });

    if !last {
        let y = ui.min_rect().max.y;
        ui.painter().hline(
            (ui.min_rect().min.x + 14.0)..=ui.min_rect().max.x,
            y,
            egui::Stroke::new(1.0, theme.separator),
        );
    }
}

/// Collapses a captured block of text into one row that fits `width`.
///
/// OCR output is full of newlines, and a history row has space for exactly one
/// line of each half.
fn one_line(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut job = egui::text::LayoutJob::simple(flat, font, color, width);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    ui.fonts(|f| f.layout_job(job))
}

/// Interface language, with "match the system" as the first entry.
///
/// Every language names itself: a list written in the *current* interface
/// language is no help to someone who cannot read the current interface
/// language, which is exactly who opens this.
fn ui_language_picker(ui: &mut egui::Ui, value: &mut Option<Lang>) {
    let selected = match value {
        Some(lang) => lang.endonym(),
        None => t("Match the system language"),
    };

    egui::ComboBox::from_id_salt("ui_language")
        .selected_text(selected)
        .width(field_width(ui))
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, t("Match the system language"));
            for lang in Lang::ALL {
                ui.selectable_value(value, Some(lang), lang.endonym());
            }
        });
}

/// Width for an input inside a settings row: as wide as fits, capped so the
/// label beside it stays readable.
fn field_width(ui: &egui::Ui) -> f32 {
    (ui.available_width() - 8.0).clamp(120.0, 250.0)
}

/// Password-style field. Returns true when the value changed.
fn secret_field(ui: &mut egui::Ui, id: &str, buf: &mut String, reveal: bool) -> bool {
    ui.add(
        egui::TextEdit::singleline(buf)
            .id_salt(id)
            .password(!reveal)
            .desired_width(field_width(ui))
            .hint_text(t("paste the key")),
    )
    .changed()
}

/// Reads one key combination from the current frame's input.
///
/// `Some(Some(hk))` — bound, `Some(None)` — the user pressed Escape to cancel,
/// `None` — nothing pressed yet.
fn capture_combination(ctx: &egui::Context) -> Option<Option<Hotkey>> {
    ctx.input(|i| {
        for event in &i.raw.events {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                continue;
            };
            if *key == egui::Key::Escape {
                return Some(None);
            }
            let Some(vk) = egui_key_to_vk(*key) else {
                continue;
            };
            let mut mods = MOD_NOREPEAT;
            if modifiers.ctrl {
                mods |= MOD_CONTROL;
            }
            if modifiers.alt {
                mods |= MOD_ALT;
            }
            if modifiers.shift {
                mods |= MOD_SHIFT;
            }
            // A bare letter would swallow that key everywhere on the system.
            if mods == MOD_NOREPEAT {
                continue;
            }
            return Some(Some(Hotkey {
                modifiers: mods,
                key: vk,
                enabled: true,
            }));
        }
        None
    })
}

fn egui_key_to_vk(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    let vk = match key {
        A => 0x41,
        B => 0x42,
        C => 0x43,
        D => 0x44,
        E => 0x45,
        F => 0x46,
        G => 0x47,
        H => 0x48,
        I => 0x49,
        J => 0x4A,
        K => 0x4B,
        L => 0x4C,
        M => 0x4D,
        N => 0x4E,
        O => 0x4F,
        P => 0x50,
        Q => 0x51,
        R => 0x52,
        S => 0x53,
        T => 0x54,
        U => 0x55,
        V => 0x56,
        W => 0x57,
        X => 0x58,
        Y => 0x59,
        Z => 0x5A,
        Num0 => 0x30,
        Num1 => 0x31,
        Num2 => 0x32,
        Num3 => 0x33,
        Num4 => 0x34,
        Num5 => 0x35,
        Num6 => 0x36,
        Num7 => 0x37,
        Num8 => 0x38,
        Num9 => 0x39,
        F1 => 0x70,
        F2 => 0x71,
        F3 => 0x72,
        F4 => 0x73,
        F5 => 0x74,
        F6 => 0x75,
        F7 => 0x76,
        F8 => 0x77,
        F9 => 0x78,
        F10 => 0x79,
        F11 => 0x7A,
        F12 => 0x7B,
        Space => 0x20,
        Backtick => 0xC0,
        Minus => 0xBD,
        Equals => 0xBB,
        OpenBracket => 0xDB,
        CloseBracket => 0xDD,
        Semicolon => 0xBA,
        Quote => 0xDE,
        Comma => 0xBC,
        Period => 0xBE,
        Slash => 0xBF,
        _ => return None,
    };
    Some(vk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::settings::vk_name;

    #[test]
    fn section_names_round_trip() {
        for section in Section::all() {
            let name = format!("{section:?}").to_ascii_lowercase();
            assert_eq!(
                Section::from_name(&name),
                Some(section),
                "{name} did not map back"
            );
        }
    }

    #[test]
    fn an_unknown_section_name_is_rejected() {
        assert_eq!(Section::from_name("nope"), None);
    }

    #[test]
    fn both_platform_spellings_of_a_page_resolve() {
        assert_eq!(Section::from_name("общие"), Some(Section::General));
        assert_eq!(Section::from_name("основные"), Some(Section::General));
    }

    #[test]
    fn every_section_has_a_distinct_label_on_each_platform() {
        for platform in [Platform::Windows, Platform::MacOs] {
            let mut labels: Vec<&str> = Section::all().iter().map(|s| s.label(platform)).collect();
            labels.sort_unstable();
            let n = labels.len();
            labels.dedup();
            assert_eq!(n, labels.len(), "duplicate label on {platform:?}");
        }
    }

    #[test]
    fn the_two_platforms_name_some_pages_differently() {
        assert_ne!(
            Section::Keys.label(Platform::Windows),
            Section::Keys.label(Platform::MacOs)
        );
    }

    #[test]
    fn an_empty_search_matches_everything() {
        for section in Section::all() {
            assert!(section.matches("", Platform::Windows));
        }
    }

    #[test]
    fn search_finds_a_page_by_its_label() {
        assert!(Section::Languages.matches("language", Platform::Windows));
        assert!(!Section::Logs.matches("language", Platform::Windows));
    }

    #[test]
    fn search_finds_a_page_by_what_is_on_it() {
        // "deepl" is nowhere in the label, but it is what the page is for.
        assert!(Section::Engine.matches("deepl", Platform::Windows));
        assert!(Section::General.matches("autostart", Platform::Windows));
    }

    #[test]
    fn search_is_case_insensitive() {
        assert!(Section::Engine.matches("DeepL", Platform::Windows));
    }

    #[test]
    fn recorded_keys_map_back_to_printable_names() {
        for key in [
            egui::Key::T,
            egui::Key::F5,
            egui::Key::Num7,
            egui::Key::Slash,
        ] {
            let vk = egui_key_to_vk(key).expect("mappable");
            assert_ne!(vk_name(vk), "?", "{key:?} has no printable name");
        }
    }

    #[test]
    fn modifier_only_keys_are_not_bindable() {
        // Binding plain Enter would make it the hotkey everywhere.
        assert_eq!(egui_key_to_vk(egui::Key::Enter), None);
        assert_eq!(egui_key_to_vk(egui::Key::Tab), None);
    }

    #[test]
    fn the_engine_summary_names_the_first_engine_in_the_chain() {
        let s = Settings::default();
        let summary = engine_summary(&s);
        assert!(summary.starts_with("Yandex"), "{summary}");
        assert!(summary.contains("then 1 more"), "{summary}");
    }

    #[test]
    fn the_engine_summary_says_so_when_nothing_is_ready() {
        let mut s = Settings::default();
        s.engines.yandex = false;
        s.engines.google = false;
        assert_eq!(engine_summary(&s), "No engine is ready");
    }
}
