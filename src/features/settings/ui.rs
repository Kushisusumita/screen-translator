use crate::entities::language::Language;
use crate::entities::settings::{
    Settings, TooltipMode, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VK_T,
};
use egui::{Color32, RichText, Separator};

pub struct SettingsUi {
    pub hotkey_recording: bool,
    pub pending_hotkey: Option<(u32, u32, String)>,
}

impl SettingsUi {
    pub fn new() -> Self {
        Self { hotkey_recording: false, pending_hotkey: None }
    }

    /// Renders the settings panel.
    ///
    /// Out-params:
    /// - `autostart_changed`    — autostart toggle was flipped
    /// - `hotkey_changed`       — a new hotkey was applied
    /// - `update_check_clicked` — user clicked "Check for updates"
    /// - `update_install_clicked` — user clicked "Download & Install"
    ///
    /// `update_status`       — label shown in the Updates section
    /// `update_check_enabled`  — whether "Check" button is clickable
    /// `update_install_enabled` — whether "Download & Install" is clickable
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
        autostart_changed: &mut bool,
        hotkey_changed: &mut bool,
        update_status: &str,
        update_check_enabled: bool,
        update_install_enabled: bool,
        update_check_clicked: &mut bool,
        update_install_clicked: &mut bool,
    ) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(28, 28, 36))
                    .inner_margin(egui::Margin::same(20.0)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                ui.set_min_width(400.0);

                // ── Title ──────────────────────────────────────────────────────
                ui.label(
                    RichText::new("Screen Translator")
                        .size(18.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.label(
                    RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .size(11.0)
                        .color(Color32::GRAY),
                );
                ui.add_space(14.0);

                // ── Languages ──────────────────────────────────────────────────
                section_header(ui, "Languages");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Source");
                    egui::ComboBox::from_id_salt("src_lang")
                        .selected_text(settings.source_lang.to_string())
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for lang in Language::all() {
                                ui.selectable_value(
                                    &mut settings.source_lang,
                                    *lang,
                                    lang.to_string(),
                                );
                            }
                        });
                    if ui
                        .add(egui::Button::new(
                            RichText::new("↔").color(Color32::from_rgb(160, 200, 255)),
                        ))
                        .on_hover_text("Swap languages")
                        .clicked()
                    {
                        std::mem::swap(&mut settings.source_lang, &mut settings.target_lang);
                    }
                    ui.label("Target");
                    egui::ComboBox::from_id_salt("tgt_lang")
                        .selected_text(settings.target_lang.to_string())
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for lang in Language::all() {
                                ui.selectable_value(
                                    &mut settings.target_lang,
                                    *lang,
                                    lang.to_string(),
                                );
                            }
                        });
                });
                ui.add_space(16.0);

                // ── Hotkey ─────────────────────────────────────────────────────
                section_header(ui, "Hotkey");
                ui.add_space(6.0);

                if self.hotkey_recording {
                    ui.label(RichText::new("Press a key combination…").color(Color32::YELLOW));
                    let input = ctx.input(|i| i.clone());
                    'rec: for event in &input.raw.events {
                        if let egui::Event::Key { key, pressed: true, modifiers, .. } = event {
                            if *key == egui::Key::Escape {
                                self.hotkey_recording = false;
                                break 'rec;
                            }
                            let mut mods: u32 = MOD_NOREPEAT;
                            let mut parts = Vec::new();
                            if modifiers.ctrl  { mods |= MOD_CONTROL; parts.push("Ctrl"); }
                            if modifiers.alt   { mods |= MOD_ALT;     parts.push("Alt"); }
                            if modifiers.shift { mods |= MOD_SHIFT;   parts.push("Shift"); }
                            if let Some(vk) = egui_key_to_vk(*key) {
                                parts.push(egui_key_to_display(*key));
                                settings.hotkey_modifiers = mods;
                                settings.hotkey_key = vk;
                                settings.hotkey_display = parts.join("+");
                                self.pending_hotkey = None;
                                self.hotkey_recording = false;
                                *hotkey_changed = true;
                            }
                            break 'rec;
                        }
                    }
                    if ui.small_button("Cancel").clicked() {
                        self.hotkey_recording = false;
                    }
                } else {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&settings.hotkey_display)
                                .color(Color32::from_rgb(64, 196, 255))
                                .strong(),
                        );
                        if ui.small_button("Change").clicked() {
                            self.hotkey_recording = true;
                        }
                        let is_default = settings.hotkey_modifiers == (MOD_CONTROL | MOD_NOREPEAT)
                            && settings.hotkey_key == VK_T;
                        ui.add_enabled_ui(!is_default, |ui| {
                            if ui
                                .small_button("Reset to default")
                                .on_hover_text("Restore hotkey to Ctrl+T")
                                .clicked()
                            {
                                settings.hotkey_modifiers = MOD_CONTROL | MOD_NOREPEAT;
                                settings.hotkey_key = VK_T;
                                settings.hotkey_display = "Ctrl+T".to_string();
                                self.pending_hotkey = None;
                                *hotkey_changed = true;
                            }
                        });
                    });
                }
                ui.add_space(16.0);

                // ── Translation Backends ───────────────────────────────────────
                section_header(ui, "Translation Backends");
                ui.add_space(6.0);
                ui.checkbox(&mut settings.use_yandex, "Yandex Translate  (web scrape + API)");
                ui.add_space(3.0);
                ui.checkbox(&mut settings.use_google, "Google Translate  (fallback)");
                if !settings.use_yandex && !settings.use_google {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("⚠  Enable at least one backend.")
                            .color(Color32::YELLOW)
                            .size(11.0),
                    );
                }
                ui.add_space(16.0);

                // ── Display Style ──────────────────────────────────────────────
                section_header(ui, "Display Style");
                ui.add_space(6.0);
                ui.radio_value(
                    &mut settings.tooltip_mode,
                    TooltipMode::Overlay,
                    "Full-screen overlay  (frozen screenshot + tint)",
                )
                .on_hover_text("Covers the whole screen with the frozen desktop image and shows the translation top-left. Click anywhere to dismiss.");
                ui.add_space(3.0);
                ui.radio_value(
                    &mut settings.tooltip_mode,
                    TooltipMode::Native,
                    "Compact hint near selection  (Windows tooltip)",
                )
                .on_hover_text("Shows a small native Windows tooltip just below the selected region. Click anywhere to dismiss.");
                ui.add_space(16.0);

                // ── Options ────────────────────────────────────────────────────
                section_header(ui, "Options");
                ui.add_space(6.0);
                if ui
                    .checkbox(&mut settings.launch_at_startup, "Launch at Windows startup")
                    .changed()
                {
                    *autostart_changed = true;
                }
                ui.add_space(3.0);
                ui.checkbox(&mut settings.show_translation, "Show translation on screen");
                ui.add_space(3.0);
                ui.checkbox(&mut settings.copy_to_clipboard, "Copy translation to clipboard");

                if !settings.show_translation && !settings.copy_to_clipboard {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "⚠  Nothing will happen — enable at least one of\n\
                             \"Show translation on screen\" or \"Copy to clipboard\".",
                        )
                        .color(Color32::YELLOW)
                        .size(11.0),
                    );
                }
                ui.add_space(16.0);

                // ── Updates ────────────────────────────────────────────────────
                section_header(ui, "Updates");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(update_check_enabled, |ui| {
                        if ui.small_button("Check for updates").clicked() {
                            *update_check_clicked = true;
                        }
                    });
                    ui.add_enabled_ui(update_install_enabled, |ui| {
                        if ui.small_button("Download & Install").clicked() {
                            *update_install_clicked = true;
                        }
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(update_status)
                        .size(11.0)
                        .color(if update_install_enabled {
                            Color32::from_rgb(100, 220, 100)
                        } else {
                            Color32::GRAY
                        }),
                );
                ui.add_space(16.0);

                // ── About ──────────────────────────────────────────────────────
                section_header(ui, "About");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Repository:").color(Color32::GRAY));
                    ui.hyperlink_to(
                        "github.com/Kushisusumita/screen-translator",
                        "https://github.com/Kushisusumita/screen-translator",
                    );
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Author:").color(Color32::GRAY));
                    ui.hyperlink_to("@クシススミタ", "https://github.com/Kushisusumita");
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "Hotkey {} — translates any screen region.",
                        settings.hotkey_display
                    ))
                    .size(11.0)
                    .color(Color32::GRAY),
                );
                ui.add_space(16.0);

                // ── Support ────────────────────────────────────────────────────
                section_header(ui, "Support the Project");
                ui.add_space(6.0);
                ui.label(
                    RichText::new("If this tool saves you time, consider a small donation.")
                        .size(11.0)
                        .color(Color32::GRAY),
                );
                ui.add_space(6.0);

                // Binance Pay
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Binance Pay")
                            .size(11.5)
                            .color(Color32::from_rgb(240, 185, 11)), // Binance yellow
                    );
                    ui.label(RichText::new("(no fees)").size(11.0).color(Color32::GRAY));
                });
                ui.add_space(3.0);
                ui.hyperlink_to(
                    "app.binance.com/uni-qr/5gjTx7at",
                    "https://app.binance.com/uni-qr/5gjTx7at",
                );
                ui.add_space(8.0);

                // USDT TRC20
                ui.label(
                    RichText::new("USDT TRC20  (without Binance account)")
                        .size(11.5)
                        .color(Color32::from_rgb(38, 161, 123)), // Tether green
                );
                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("TNwbTzUgk2F11PDfqP8J9fpZCAN3K4yXPQ")
                            .size(10.5)
                            .color(Color32::from_rgb(200, 200, 200))
                            .monospace(),
                    );
                    if ui.small_button("📋 Copy").on_hover_text("Copy address to clipboard").clicked() {
                        ui.output_mut(|o| {
                            o.copied_text = "TNwbTzUgk2F11PDfqP8J9fpZCAN3K4yXPQ".to_string();
                        });
                    }
                });

                ui.add_space(8.0);
                }); // ScrollArea
            });
    }
}

fn section_header(ui: &mut egui::Ui, title: &str) {
    ui.label(
        RichText::new(title)
            .strong()
            .size(11.5)
            .color(Color32::from_rgb(80, 160, 220)),
    );
    ui.add(Separator::default().spacing(5.0));
}

fn egui_key_to_vk(key: egui::Key) -> Option<u32> {
    match key {
        egui::Key::A => Some(0x41), egui::Key::B => Some(0x42),
        egui::Key::C => Some(0x43), egui::Key::D => Some(0x44),
        egui::Key::E => Some(0x45), egui::Key::F => Some(0x46),
        egui::Key::G => Some(0x47), egui::Key::H => Some(0x48),
        egui::Key::I => Some(0x49), egui::Key::J => Some(0x4A),
        egui::Key::K => Some(0x4B), egui::Key::L => Some(0x4C),
        egui::Key::M => Some(0x4D), egui::Key::N => Some(0x4E),
        egui::Key::O => Some(0x4F), egui::Key::P => Some(0x50),
        egui::Key::Q => Some(0x51), egui::Key::R => Some(0x52),
        egui::Key::S => Some(0x53), egui::Key::T => Some(0x54),
        egui::Key::U => Some(0x55), egui::Key::V => Some(0x56),
        egui::Key::W => Some(0x57), egui::Key::X => Some(0x58),
        egui::Key::Y => Some(0x59), egui::Key::Z => Some(0x5A),
        egui::Key::F1  => Some(0x70), egui::Key::F2  => Some(0x71),
        egui::Key::F3  => Some(0x72), egui::Key::F4  => Some(0x73),
        egui::Key::F5  => Some(0x74), egui::Key::F6  => Some(0x75),
        egui::Key::F7  => Some(0x76), egui::Key::F8  => Some(0x77),
        egui::Key::F9  => Some(0x78), egui::Key::F10 => Some(0x79),
        egui::Key::F11 => Some(0x7A), egui::Key::F12 => Some(0x7B),
        egui::Key::Num0 => Some(0x30), egui::Key::Num1 => Some(0x31),
        egui::Key::Num2 => Some(0x32), egui::Key::Num3 => Some(0x33),
        egui::Key::Num4 => Some(0x34), egui::Key::Num5 => Some(0x35),
        egui::Key::Num6 => Some(0x36), egui::Key::Num7 => Some(0x37),
        egui::Key::Num8 => Some(0x38), egui::Key::Num9 => Some(0x39),
        _ => None,
    }
}

fn egui_key_to_display(key: egui::Key) -> &'static str {
    match key {
        egui::Key::A => "A", egui::Key::B => "B", egui::Key::C => "C",
        egui::Key::D => "D", egui::Key::E => "E", egui::Key::F => "F",
        egui::Key::G => "G", egui::Key::H => "H", egui::Key::I => "I",
        egui::Key::J => "J", egui::Key::K => "K", egui::Key::L => "L",
        egui::Key::M => "M", egui::Key::N => "N", egui::Key::O => "O",
        egui::Key::P => "P", egui::Key::Q => "Q", egui::Key::R => "R",
        egui::Key::S => "S", egui::Key::T => "T", egui::Key::U => "U",
        egui::Key::V => "V", egui::Key::W => "W", egui::Key::X => "X",
        egui::Key::Y => "Y", egui::Key::Z => "Z",
        egui::Key::F1  => "F1",  egui::Key::F2  => "F2",
        egui::Key::F3  => "F3",  egui::Key::F4  => "F4",
        egui::Key::F5  => "F5",  egui::Key::F6  => "F6",
        egui::Key::F7  => "F7",  egui::Key::F8  => "F8",
        egui::Key::F9  => "F9",  egui::Key::F10 => "F10",
        egui::Key::F11 => "F11", egui::Key::F12 => "F12",
        egui::Key::Num0 => "0", egui::Key::Num1 => "1",
        egui::Key::Num2 => "2", egui::Key::Num3 => "3",
        egui::Key::Num4 => "4", egui::Key::Num5 => "5",
        egui::Key::Num6 => "6", egui::Key::Num7 => "7",
        egui::Key::Num8 => "8", egui::Key::Num9 => "9",
        _ => "?",
    }
}
