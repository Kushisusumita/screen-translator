use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use egui::{Color32, Context, Rect, ViewportBuilder, ViewportId};
use tracing::{error, info};

use crate::entities::settings::Settings;
use crate::features::capture::{capture_full_screen_image, OverlayState, render_overlay};
use crate::features::hotkey::HotkeyManager;
use crate::features::settings::{load_settings, save_settings, SettingsUi};
use crate::features::tooltip::render_tooltip;
use crate::features::tray::{TrayEvent, TrayManager};
use crate::features::translation::{run_pipeline, PipelineResult};
use crate::features::updater::{check_for_update, download_and_apply, UpdateInfo};
use crate::shared::utils::autostart::{get_current_exe_path, set_autostart};
use crate::shared::utils::clipboard::copy_text_to_clipboard;

/// Returns the primary monitor dimensions in logical pixels.
fn screen_size() -> (f32, f32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let w = unsafe { GetSystemMetrics(SM_CXSCREEN) } as f32;
    let h = unsafe { GetSystemMetrics(SM_CYSCREEN) } as f32;
    (w.max(1280.0), h.max(720.0))
}

fn real_screen_rect() -> egui::Rect {
    let (w, h) = screen_size();
    egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, h))
}

/// Loads the app icon from assets/icon.ico as egui IconData.
fn load_app_icon() -> Option<egui::IconData> {
    let bytes = std::fs::read("assets/icon.ico").ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(egui::IconData { rgba: rgba.into_raw(), width: w, height: h })
}

/// Loads the best available Windows CJK font as an egui proportional fallback.
/// Meiryo is preferred — designed for mixed Latin+Japanese screen rendering.
///
/// `y_offset_factor` compensates for Meiryo/YuGothic reporting a larger ascent
/// than egui's built-in NotoSans, which otherwise makes katakana glyphs float
/// above the Latin baseline in the same text run.
fn load_cjk_font() -> Option<egui::FontData> {
    // (path, y_offset_factor to align with NotoSans baseline)
    let candidates: &[(&str, f32)] = &[
        (r"C:\Windows\Fonts\meiryo.ttc",   0.15),
        (r"C:\Windows\Fonts\YuGothR.ttc",  0.10),
        (r"C:\Windows\Fonts\YuGothM.ttc",  0.10),
        (r"C:\Windows\Fonts\msgothic.ttc", 0.10),
    ];
    for &(path, y_offset) in candidates {
        if let Ok(data) = std::fs::read(path) {
            info!("Loaded CJK font: {}", path);
            let mut fd = egui::FontData::from_owned(data);
            fd.tweak.y_offset_factor = y_offset;
            return Some(fd);
        }
    }
    None
}

#[derive(Debug, Clone)]
pub enum TranslationState {
    Loading,
    Done(PipelineResult),
    Error(String),
}

/// App-level update lifecycle state.
#[derive(Debug, Clone)]
enum UpdateFetchState {
    Checking,
    UpToDate,
    Available(UpdateInfo),
    Downloading,
    Error(String),
}

pub struct App {
    settings: Arc<Mutex<Settings>>,
    rt: tokio::runtime::Runtime,
    hotkey_fired: Arc<AtomicBool>,
    tray_event: Arc<Mutex<Option<TrayEvent>>>,
    overlay_active: bool,
    overlay_state: OverlayState,
    translation_result: Arc<Mutex<Option<TranslationState>>>,
    show_settings: bool,
    /// Full-screen overlay tooltip active.
    show_tooltip: bool,
    /// Compact egui popup tooltip active (TooltipMode::Native).
    compact_tooltip_active: bool,
    tooltip_text: String,
    tooltip_bg: Option<egui::ColorImage>,
    tooltip_bg_handle: Option<egui::TextureHandle>,
    /// Last selection rect — used to position the compact tooltip.
    last_selection: Option<Rect>,
    settings_ui: SettingsUi,
    _hotkey_manager: HotkeyManager,
    _tray_manager: TrayManager,
    /// Set to true by tray Exit so the CancelClose guard doesn't fire.
    should_exit: bool,
    update_state: Arc<Mutex<UpdateFetchState>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Register CJK font as proportional fallback (Meiryo first for best Latin+JP mixing).
        let mut fonts = egui::FontDefinitions::default();
        if let Some(cjk) = load_cjk_font() {
            fonts.font_data.insert("cjk_fallback".to_owned(), cjk);
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .push("cjk_fallback".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk_fallback".to_owned());
        }
        cc.egui_ctx.set_fonts(fonts);

        let settings = load_settings();
        let hotkey_fired = Arc::new(AtomicBool::new(false));
        let tray_event: Arc<Mutex<Option<TrayEvent>>> = Arc::new(Mutex::new(None));

        let hotkey_manager = HotkeyManager::start(
            settings.hotkey_modifiers,
            settings.hotkey_key,
            Arc::clone(&hotkey_fired),
            cc.egui_ctx.clone(),
        );
        let tray_manager = TrayManager::start(Arc::clone(&tray_event), cc.egui_ctx.clone());
        let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");

        let update_state = Arc::new(Mutex::new(UpdateFetchState::Checking));
        {
            let state = Arc::clone(&update_state);
            rt.spawn(async move {
                match check_for_update().await {
                    Ok(Some(info)) => {
                        info!("Update available: v{}", info.version);
                        if let Ok(mut g) = state.lock() {
                            *g = UpdateFetchState::Available(info);
                        }
                    }
                    Ok(None) => {
                        if let Ok(mut g) = state.lock() {
                            *g = UpdateFetchState::UpToDate;
                        }
                    }
                    Err(e) => {
                        error!("Update check failed: {}", e);
                        if let Ok(mut g) = state.lock() {
                            *g = UpdateFetchState::Error(e);
                        }
                    }
                }
            });
        }

        cc.egui_ctx.request_repaint_after(Duration::from_millis(100));

        App {
            settings: Arc::new(Mutex::new(settings)),
            rt,
            hotkey_fired,
            tray_event,
            overlay_active: false,
            overlay_state: OverlayState::new(),
            translation_result: Arc::new(Mutex::new(None)),
            show_settings: false,
            show_tooltip: false,
            compact_tooltip_active: false,
            tooltip_text: String::new(),
            tooltip_bg: None,
            tooltip_bg_handle: None,
            last_selection: None,
            settings_ui: SettingsUi::new(),
            _hotkey_manager: hotkey_manager,
            _tray_manager: tray_manager,
            should_exit: false,
            update_state,
        }
    }

    fn check_hotkey(&mut self, _ctx: &Context) {
        if self.hotkey_fired.swap(false, Ordering::SeqCst) {
            info!("Hotkey detected, activating overlay");
            if !self.overlay_active {
                self.show_tooltip = false;
                self.compact_tooltip_active = false;
                self.tooltip_bg_handle = None;
                let bg = unsafe { capture_full_screen_image().ok() };
                self.tooltip_bg = bg.clone();
                self.overlay_state = match bg {
                    Some(img) => OverlayState::with_screenshot(img),
                    None => OverlayState::new(),
                };
                self.overlay_active = true;
            }
        }
    }

    fn check_tray_events(&mut self, ctx: &Context) {
        let event = self.tray_event.lock().ok().and_then(|mut g| g.take());
        match event {
            Some(TrayEvent::ShowSettings) => {
                info!("Tray: opening settings");
                self.show_settings = true;
            }
            Some(TrayEvent::Exit) => {
                info!("Tray: exit requested");
                self.should_exit = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            None => {}
        }
    }

    fn check_translation_result(&mut self) {
        let result = self.translation_result.lock().ok().and_then(|g| g.clone());
        let (mode, show_on_screen, copy_to_cb) = self
            .settings
            .lock()
            .map(|s| (s.tooltip_mode, s.show_translation, s.copy_to_clipboard))
            .unwrap_or_default();

        match result {
            Some(TranslationState::Done(ref res)) => {
                let text = res.translated.clone();
                if copy_to_cb {
                    if let Err(e) = copy_text_to_clipboard(&text) {
                        error!("Clipboard copy failed: {}", e);
                    }
                }
                if show_on_screen {
                    self.show_result(text, mode);
                }
                if let Ok(mut guard) = self.translation_result.lock() {
                    *guard = None;
                }
            }
            Some(TranslationState::Error(ref msg)) => {
                self.show_result(msg.clone(), mode);
                if let Ok(mut guard) = self.translation_result.lock() {
                    *guard = None;
                }
            }
            Some(TranslationState::Loading) | None => {}
        }
    }

    fn show_result(&mut self, text: String, mode: crate::entities::settings::TooltipMode) {
        use crate::entities::settings::TooltipMode;
        self.tooltip_text = text;
        match mode {
            TooltipMode::Overlay => {
                self.show_tooltip = true;
            }
            TooltipMode::Native => {
                self.compact_tooltip_active = true;
            }
        }
    }

    fn process_overlay_result(&mut self, selection: Rect) {
        self.last_selection = Some(selection);

        let (show_on_screen, copy_to_cb) = self
            .settings
            .lock()
            .map(|s| (s.show_translation, s.copy_to_clipboard))
            .unwrap_or((true, false));
        if !show_on_screen && !copy_to_cb {
            return;
        }

        let x = selection.min.x as i32;
        let y = selection.min.y as i32;
        let w = selection.width() as i32;
        let h = selection.height() as i32;

        let jpeg = unsafe { crate::features::capture::screenshot::capture_region(x, y, w, h) };

        match jpeg {
            Err(e) => {
                error!("Screenshot capture failed: {}", e);
                if let Ok(mut guard) = self.translation_result.lock() {
                    *guard = Some(TranslationState::Error(format!("Capture failed: {}", e)));
                }
            }
            Ok(jpeg_data) => {
                if let Ok(mut guard) = self.translation_result.lock() {
                    *guard = Some(TranslationState::Loading);
                }

                let result_arc = Arc::clone(&self.translation_result);
                let (src, tgt, use_yandex, use_google) = self
                    .settings
                    .lock()
                    .map(|s| {
                        (
                            s.source_lang.code().to_string(),
                            s.target_lang.code().to_string(),
                            s.use_yandex,
                            s.use_google,
                        )
                    })
                    .unwrap_or_else(|_| ("en".into(), "ru".into(), true, true));

                self.rt.spawn(async move {
                    match run_pipeline(&jpeg_data, &src, &tgt, use_yandex, use_google).await {
                        Ok(result) => {
                            if let Ok(mut guard) = result_arc.lock() {
                                *guard = Some(TranslationState::Done(result));
                            }
                        }
                        Err(e) => {
                            error!("Pipeline failed: {}", e);
                            if let Ok(mut guard) = result_arc.lock() {
                                *guard = Some(TranslationState::Error(e.to_string()));
                            }
                        }
                    }
                });
            }
        }
    }

    fn show_tooltip_viewport(&mut self, ctx: &Context) {
        let text = self.tooltip_text.clone();
        let bg_image = self.tooltip_bg.clone();
        let mut close = false;
        let screen = real_screen_rect();
        let mut bg_handle = self.tooltip_bg_handle.take();

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("tooltip"),
            ViewportBuilder::default()
                .with_position(egui::pos2(0.0, 0.0))
                .with_inner_size([screen.width(), screen.height()])
                .with_decorations(false)
                .with_always_on_top()
                .with_taskbar(false)
                .with_resizable(false)
                .with_transparent(true),
            |ctx, _| {
                if bg_handle.is_none() {
                    if let Some(ref img) = bg_image {
                        bg_handle = Some(ctx.load_texture(
                            "tooltip_bg",
                            img.clone(),
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }

                let mut visuals = ctx.style().visuals.clone();
                visuals.window_fill = egui::Color32::TRANSPARENT;
                visuals.panel_fill = egui::Color32::TRANSPARENT;
                ctx.set_visuals(visuals);

                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |_ui| {});

                let mut panel_close = false;
                render_tooltip(ctx, &text, &mut panel_close, bg_handle.as_ref());

                if panel_close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
                if close {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            },
        );

        if close {
            self.show_tooltip = false;
            self.tooltip_text.clear();
            self.tooltip_bg = None;
            self.tooltip_bg_handle = None;
        } else {
            self.tooltip_bg_handle = bg_handle;
        }
    }

    /// Small egui popup near the selection (replaces the broken Win32 tracking tooltip).
    fn show_compact_tooltip_viewport(&mut self, ctx: &Context) {
        let text = self.tooltip_text.clone();
        let (sw, sh) = screen_size();
        const TIP_W: f32 = 440.0;
        const TIP_H: f32 = 200.0;

        // Position below the selection; flip above if too close to screen bottom.
        let pos = self.last_selection.map(|r| {
            let x = r.min.x.clamp(0.0, (sw - TIP_W).max(0.0));
            let y = if r.max.y + TIP_H + 12.0 < sh {
                r.max.y + 8.0
            } else {
                (r.min.y - TIP_H - 8.0).max(0.0)
            };
            egui::pos2(x, y)
        }).unwrap_or(egui::pos2(100.0, 100.0));

        let mut close = false;

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("compact_tooltip"),
            ViewportBuilder::default()
                .with_position(pos)
                .with_inner_size([TIP_W, TIP_H])
                .with_decorations(false)
                .with_always_on_top()
                .with_taskbar(false)
                .with_resizable(false)
                .with_transparent(true),
            |ctx, _| {
                let mut vis = ctx.style().visuals.clone();
                vis.window_fill = Color32::TRANSPARENT;
                vis.panel_fill = Color32::TRANSPARENT;
                ctx.set_visuals(vis);

                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        let rect = ui.max_rect();

                        // Rounded dark pill background.
                        ui.painter().rect_filled(
                            rect,
                            10.0,
                            Color32::from_rgba_unmultiplied(18, 18, 28, 235),
                        );
                        // Subtle border.
                        ui.painter().rect_stroke(
                            rect.shrink(1.0),
                            10.0,
                            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(80, 120, 200, 120)),
                        );

                        egui::Frame::none()
                            .inner_margin(egui::Margin::same(14.0))
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .max_height(TIP_H - 28.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        ui.set_max_width(TIP_W - 28.0);
                                        ui.label(
                                            egui::RichText::new(&text)
                                                .size(14.0)
                                                .color(Color32::WHITE),
                                        );
                                    });
                            });

                        // Dismiss on click anywhere in the popup.
                        if ui.interact(
                            rect,
                            egui::Id::new("compact_dismiss"),
                            egui::Sense::click(),
                        ).clicked() {
                            close = true;
                        }
                    });

                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
                if close {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            },
        );

        if close {
            self.compact_tooltip_active = false;
            self.tooltip_text.clear();
        }
    }

    fn show_settings_viewport(&mut self, ctx: &Context) {
        let mut autostart_changed = false;
        let mut hotkey_changed = false;
        let mut close_settings = false;
        let mut settings_clone = self.settings.lock().map(|s| s.clone()).unwrap_or_default();

        let (update_status, update_check_enabled, update_install_enabled, install_url) = {
            let st = self.update_state.lock().unwrap_or_else(|e| e.into_inner());
            match &*st {
                UpdateFetchState::Checking => {
                    ("Checking for updates…".to_string(), false, false, None)
                }
                UpdateFetchState::UpToDate => (
                    format!("You are up to date  (v{})", env!("CARGO_PKG_VERSION")),
                    true, false, None,
                ),
                UpdateFetchState::Available(info) => (
                    format!("v{} is available", info.version),
                    true, true, Some(info.url.clone()),
                ),
                UpdateFetchState::Downloading => {
                    ("Downloading update…".to_string(), false, false, None)
                }
                UpdateFetchState::Error(e) => {
                    (format!("Error: {}", e), true, false, None)
                }
            }
        };

        let mut update_check_clicked = false;
        let mut update_install_clicked = false;

        // Build viewport — include app icon so settings window shows the real icon.
        let mut vp_builder = ViewportBuilder::default()
            .with_title("Screen Translator — Settings")
            .with_inner_size([460.0, 640.0])
            .with_resizable(false)
            .with_maximize_button(false);
        if let Some(icon) = load_app_icon() {
            vp_builder = vp_builder.with_icon(std::sync::Arc::new(icon));
        }

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("settings"),
            vp_builder,
            |ctx, _| {
                let mut vis = egui::Visuals::dark();
                vis.window_fill = Color32::from_rgb(35, 35, 48);
                vis.panel_fill = Color32::from_rgb(28, 28, 36);
                ctx.set_visuals(vis);

                self.settings_ui.show(
                    ctx,
                    &mut settings_clone,
                    &mut autostart_changed,
                    &mut hotkey_changed,
                    &update_status,
                    update_check_enabled,
                    update_install_enabled,
                    &mut update_check_clicked,
                    &mut update_install_clicked,
                );

                if ctx.input(|i| i.viewport().close_requested()) {
                    close_settings = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close_settings = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            },
        );

        if let Ok(mut s) = self.settings.lock() {
            *s = settings_clone.clone();
        }

        if autostart_changed {
            if let Err(e) = set_autostart(settings_clone.launch_at_startup, &get_current_exe_path()) {
                error!("Autostart update failed: {}", e);
            }
        }
        if hotkey_changed {
            self._hotkey_manager.update_hotkey(
                settings_clone.hotkey_modifiers,
                settings_clone.hotkey_key,
            );
        }

        if update_check_clicked {
            if let Ok(mut g) = self.update_state.lock() {
                *g = UpdateFetchState::Checking;
            }
            let state = Arc::clone(&self.update_state);
            self.rt.spawn(async move {
                match check_for_update().await {
                    Ok(Some(info)) => { if let Ok(mut g) = state.lock() { *g = UpdateFetchState::Available(info); } }
                    Ok(None)       => { if let Ok(mut g) = state.lock() { *g = UpdateFetchState::UpToDate; } }
                    Err(e)         => { if let Ok(mut g) = state.lock() { *g = UpdateFetchState::Error(e); } }
                }
            });
        }
        if update_install_clicked {
            if let Some(url) = install_url {
                if let Ok(mut g) = self.update_state.lock() { *g = UpdateFetchState::Downloading; }
                let state = Arc::clone(&self.update_state);
                self.rt.spawn(async move {
                    if let Err(e) = download_and_apply(&url).await {
                        error!("Update install failed: {}", e);
                        if let Ok(mut g) = state.lock() { *g = UpdateFetchState::Error(e); }
                    }
                });
            }
        }

        if close_settings {
            self.show_settings = false;
            if let Err(e) = save_settings(&settings_clone) {
                error!("Settings auto-save failed: {}", e);
            }
        }
    }

    fn show_overlay_viewport(&mut self, ctx: &Context) {
        let mut overlay_state = std::mem::replace(&mut self.overlay_state, OverlayState::new());
        let mut completed_selection: Option<Rect> = None;
        let mut should_close = false;

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("overlay"),
            ViewportBuilder::default()
                .with_fullscreen(true)
                .with_always_on_top()
                .with_decorations(false),
            |ctx, _| {
                render_overlay(ctx, &mut overlay_state);
                if overlay_state.cancelled { should_close = true; }
                if let Some(sel) = overlay_state.completed.take() {
                    completed_selection = Some(sel);
                    should_close = true;
                }
                if should_close {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            },
        );

        self.overlay_state = overlay_state;

        if should_close {
            self.overlay_active = false;
            if let Some(selection) = completed_selection {
                self.process_overlay_result(selection);
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(50));

        self.check_hotkey(ctx);
        self.check_tray_events(ctx);
        self.check_translation_result();

        if self.overlay_active          { self.show_overlay_viewport(ctx); }
        if self.show_tooltip            { self.show_tooltip_viewport(ctx); }
        if self.compact_tooltip_active  { self.show_compact_tooltip_viewport(ctx); }
        if self.show_settings           { self.show_settings_viewport(ctx); }

        if ctx.input(|i| i.viewport().close_requested()) && !self.should_exit {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }
}
