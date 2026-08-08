//! Application orchestrator.
//!
//! Owns the settings, drives the capture → translate → present flow, and hosts
//! the overlay, the result view and the settings window as egui viewports.
//!
//! Settings live here as a plain value rather than behind an `Arc<Mutex<_>>`.
//! Only the UI thread touches them; background work gets an immutable snapshot
//! at spawn time. That removes the whole class of "lock poisoned, app silently
//! stops working" failures the previous structure was prone to.

pub mod result;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{ColorImage, Context, ViewportBuilder, ViewportCommand, ViewportId};
use tracing::{error, info, warn};

use crate::entities::history::{History, HistoryEntry};
use crate::entities::settings::{CaptureMode, HotkeyAction, ResultView, Settings};
use crate::features::capture::{
    capture_desktop_image, capture_region_for_ocr, foreground_window_bounds, virtual_desktop,
    window_pick, Bounds, Geometry, OverlayState,
};
use crate::features::hotkey::HotkeyManager;
use crate::features::settings::save_settings;
use crate::features::settings::ui::{Section, SettingsContext, SettingsUi};
use crate::features::translation::{run_pipeline, PipelineParams, PipelineResult};
use crate::features::tray::{TrayEvent, TrayManager};
use crate::features::updater::{check_for_update, download_and_apply, UpdateInfo};
use crate::shared::logging;
use crate::shared::utils::autostart::{get_current_exe_path, set_autostart};
use crate::shared::utils::clipboard::copy_text_to_clipboard;
use crate::shared::utils::tts;
use crate::ui::{theme::Theme, widgets};

use result::{ResultAction, ResultUi, Stage};

/// Monotonic id for capture requests. A result whose id is not the current one
/// belongs to a capture the user has already replaced, and is dropped.
static GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
enum UpdateState {
    Checking,
    UpToDate,
    Available(UpdateInfo),
    Downloading,
    Error(String),
}

type PipelineSlot = Arc<Mutex<Option<(u64, Result<PipelineResult, String>)>>>;

pub struct App {
    settings: Settings,
    theme: Theme,
    history: History,
    rt: tokio::runtime::Runtime,

    hotkeys: HotkeyManager,
    tray: TrayManager,

    overlay: Option<OverlayState>,
    result: Option<ResultUi>,
    /// Frozen desktop, shared by the overlay and the inline result view.
    desktop_image: Option<ColorImage>,
    last_capture: Option<(Bounds, Geometry)>,

    settings_open: bool,
    settings_ui: SettingsUi,
    settings_dirty: bool,

    pipeline: PipelineSlot,
    update: Arc<Mutex<UpdateState>>,
    ai_test: Arc<Mutex<Option<String>>>,
    ai_test_running: bool,
    /// Last value handed to the tray, so the animation is only poked on a change.
    tray_busy: bool,

    should_exit: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        open_settings: Option<Section>,
    ) -> Self {
        cc.egui_ctx.set_fonts(crate::ui::theme::build_fonts());

        let theme = Theme::resolve(settings.theme);
        theme.apply(&cc.egui_ctx);

        let hotkeys = HotkeyManager::start(settings.hotkeys, cc.egui_ctx.clone());
        let tray = TrayManager::start(
            settings.hotkeys,
            cc.egui_ctx.clone(),
            !settings.hide_tray_icon,
        );

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Failed to create the Tokio runtime");

        let update = Arc::new(Mutex::new(UpdateState::Checking));
        spawn_update_check(&rt, Arc::clone(&update), cc.egui_ctx.clone());

        let history = History::new(settings.history_limit);

        App {
            settings,
            theme,
            history,
            rt,
            hotkeys,
            tray,
            overlay: None,
            result: None,
            desktop_image: None,
            last_capture: None,
            settings_open: open_settings.is_some(),
            settings_ui: SettingsUi::new(open_settings.unwrap_or(Section::General)),
            settings_dirty: false,
            pipeline: Arc::new(Mutex::new(None)),
            update,
            ai_test: Arc::new(Mutex::new(None)),
            ai_test_running: false,
            tray_busy: false,
            should_exit: false,
        }
    }

    // ── Input ────────────────────────────────────────────────────────────────

    fn drain_events(&mut self, ctx: &Context) {
        for action in self.hotkeys.poll() {
            match action {
                HotkeyAction::Region => self.begin_capture(ctx, CaptureMode::Region),
                HotkeyAction::Window => self.begin_capture(ctx, CaptureMode::Window),
                HotkeyAction::FullScreen => self.begin_capture(ctx, CaptureMode::FullScreen),
                HotkeyAction::Repeat => self.repeat_last_capture(),
            }
        }

        for event in self.tray.poll() {
            match event {
                TrayEvent::Capture(mode) => self.begin_capture(ctx, mode),
                TrayEvent::ShowSettings => {
                    self.settings_ui.on_open(&self.settings);
                    self.settings_open = true;
                }
                TrayEvent::Exit => {
                    info!("Exit requested from the tray");
                    self.shutdown(ctx);
                }
            }
        }
    }

    fn shutdown(&mut self, ctx: &Context) {
        if self.settings_dirty {
            if let Err(e) = save_settings(&self.settings) {
                error!(error = %e, "Could not save settings on exit");
            }
            self.settings_dirty = false;
        }
        self.hotkeys.shutdown();
        self.tray.shutdown();
        self.should_exit = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    // ── Capture ──────────────────────────────────────────────────────────────

    fn begin_capture(&mut self, ctx: &Context, mode: CaptureMode) {
        if self.overlay.is_some() {
            // Already picking a region; a second press should not stack overlays.
            return;
        }

        // Anything on screen from the previous translation would end up baked
        // into the frozen backdrop.
        self.result = None;
        ctx.request_repaint();

        let desktop = virtual_desktop();
        let ppp = ctx.pixels_per_point();
        let geometry = Geometry::new(desktop, ppp);

        let image = match capture_desktop_image() {
            Ok((img, _)) => Some(img),
            Err(e) => {
                warn!(error = %e, "Could not capture the desktop for the overlay");
                None
            }
        };

        if mode == CaptureMode::FullScreen {
            self.desktop_image = image;
            self.start_translation(ctx, desktop, geometry);
            return;
        }

        let mut state = OverlayState::new(geometry, mode, self.settings.show_mode_hud);
        if let Some(img) = image {
            self.desktop_image = Some(img.clone());
            state = state.with_background(img);
        }
        // Always, not only when the capture starts in window mode: Tab switches
        // modes while the overlay is up, and an empty list there means no window
        // can be picked at all. Enumeration has to happen now, before our own
        // full-screen overlay exists and becomes the only thing under the cursor.
        let mut windows = window_pick::enumerate();
        // The window that had focus is the likeliest target, so make sure it is
        // in the list even if enumeration missed it.
        if let Some(fg) = foreground_window_bounds() {
            if !windows.iter().any(|w| w.bounds == fg) {
                windows.insert(
                    0,
                    window_pick::WindowInfo {
                        bounds: fg,
                        title: String::new(),
                    },
                );
            }
        }
        info!(
            count = windows.len(),
            "Window list captured for the overlay"
        );
        state = state.with_windows(windows);

        self.overlay = Some(state);
    }

    fn repeat_last_capture(&mut self) {
        let Some((bounds, geometry)) = self.last_capture else {
            info!("Repeat requested but nothing has been captured yet");
            return;
        };
        let ctx = egui::Context::default();
        let _ = ctx;
        self.spawn_pipeline(bounds, geometry, None);
    }

    fn start_translation(&mut self, ctx: &Context, bounds: Bounds, geometry: Geometry) {
        self.spawn_pipeline(bounds, geometry, Some(ctx));
    }

    fn spawn_pipeline(&mut self, bounds: Bounds, geometry: Geometry, ctx: Option<&Context>) {
        self.last_capture = Some((bounds, geometry));

        let jpeg = match capture_region_for_ocr(bounds) {
            Ok(j) => j,
            Err(e) => {
                error!(error = %e, "Capture failed");
                self.show_stage(Stage::Error(e.to_string()), bounds, geometry);
                return;
            }
        };

        if self.settings.play_sound {
            beep();
        }

        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        self.show_stage(
            Stage::Loading {
                since: std::time::Instant::now(),
            },
            bounds,
            geometry,
        );

        let params = PipelineParams::from(&self.settings);
        let slot = Arc::clone(&self.pipeline);
        let repaint = ctx.cloned();

        self.rt.spawn(async move {
            let outcome = run_pipeline(&jpeg, &params)
                .await
                .map_err(|e| e.to_string());
            {
                let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
                *guard = Some((generation, outcome));
            }
            // Without this the result would sit in the slot until some other
            // event happened to wake the UI thread.
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        });
    }

    fn show_stage(&mut self, stage: Stage, bounds: Bounds, geometry: Geometry) {
        let mut ui = ResultUi::loading(self.settings.result_view, bounds, geometry)
            .with_background(self.desktop_image.clone());
        ui.pinned = self.settings.pin_result_window;
        ui.stage = stage;
        self.result = Some(ui);
    }

    fn collect_pipeline_result(&mut self) {
        let taken = {
            let mut guard = self.pipeline.lock().unwrap_or_else(|e| e.into_inner());
            guard.take()
        };
        let Some((generation, outcome)) = taken else {
            return;
        };

        // A newer capture has already superseded this one.
        if generation != GENERATION.load(Ordering::SeqCst) {
            info!(generation, "Dropping a superseded translation result");
            return;
        }

        match outcome {
            Ok(res) => {
                info!(
                    engine = res.engine.label(),
                    total_ms = res.elapsed.as_millis() as u64,
                    engine_ms = res.engine_elapsed.as_millis() as u64,
                    cached = res.from_cache,
                    "Capture complete"
                );
                if self.settings.copy_to_clipboard {
                    if let Err(e) = copy_text_to_clipboard(&res.translated) {
                        error!(error = %e, "Clipboard copy failed");
                    }
                }
                self.history.push(HistoryEntry {
                    original: res.original.clone(),
                    translated: res.translated.clone(),
                    source: res.source,
                    target: res.target,
                    engine: res.engine,
                });

                if self.settings.result_view == ResultView::None {
                    // Clipboard-only mode: nothing to show, and no empty popup.
                    self.result = None;
                    return;
                }
                if let Some(ui) = self.result.as_mut() {
                    ui.stage = Stage::Done(Box::new(res));
                }
            }
            Err(msg) => {
                error!(error = %msg, "Translation failed");
                // An error is always worth showing, even in clipboard-only mode —
                // otherwise a failed capture is indistinguishable from success.
                if let Some(ui) = self.result.as_mut() {
                    ui.stage = Stage::Error(msg);
                    if ui.view == ResultView::None {
                        ui.view = ResultView::Popup;
                    }
                } else {
                    let (bounds, geometry) = self
                        .last_capture
                        .unwrap_or((virtual_desktop(), Geometry::new(virtual_desktop(), 1.0)));
                    self.show_stage(Stage::Error(msg), bounds, geometry);
                }
            }
        }
    }

    // ── Viewports ────────────────────────────────────────────────────────────

    fn show_overlay(&mut self, ctx: &Context) {
        let Some(mut state) = self.overlay.take() else {
            return;
        };
        let geometry = state.geometry;
        let theme = self.theme;

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_capture"),
            ViewportBuilder::default()
                .with_position(geometry.window_pos_points())
                .with_inner_size(geometry.window_size_points())
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false)
                .with_active(true)
                .with_always_on_top(),
            |ctx, _| {
                // Without focus the overlay never sees a key press, and Esc does
                // nothing however clearly the hint advertises it.
                if !ctx.input(|i| i.focused) {
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                crate::features::capture::overlay::render(ctx, &theme, &mut state);
                if state.cancelled || state.completed.is_some() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            },
        );

        if state.cancelled {
            info!("Capture cancelled");
            self.settings.capture_mode = state.mode;
            return;
        }
        if let Some(bounds) = state.completed {
            self.settings.capture_mode = state.mode;
            self.start_translation(ctx, bounds, geometry);
            return;
        }
        self.overlay = Some(state);
    }

    fn show_result(&mut self, ctx: &Context) {
        let Some(mut ui) = self.result.take() else {
            return;
        };
        let theme = self.theme;
        ui.render(ctx, &theme, self.history.len());

        let mut close = false;
        for action in std::mem::take(&mut ui.actions) {
            match action {
                ResultAction::Copy => {
                    if let Some(r) = ui.result() {
                        if let Err(e) = copy_text_to_clipboard(&r.translated) {
                            error!(error = %e, "Clipboard copy failed");
                        }
                    }
                }
                ResultAction::Speak => {
                    if let Some(r) = ui.result() {
                        if let Err(e) = tts::speak(&r.translated) {
                            warn!(error = %e, "Speech synthesis unavailable");
                        }
                    }
                }
                ResultAction::SwitchTo(view) => {
                    ui.view = view;
                    if view == ResultView::Window {
                        ui.pinned = self.settings.pin_result_window;
                    }
                }
                ResultAction::Close => close = true,
            }
        }

        if !close {
            self.result = Some(ui);
        }
    }

    fn show_settings(&mut self, ctx: &Context) {
        let mut close = false;
        let theme = self.theme;

        let (status, check_enabled, install_enabled, install_url) = {
            let st = self.update.lock().unwrap_or_else(|e| e.into_inner());
            match &*st {
                UpdateState::Checking => ("Проверяю обновления…".to_string(), false, false, None),
                UpdateState::UpToDate => (
                    format!(
                        "Установлена последняя версия ({})",
                        env!("CARGO_PKG_VERSION")
                    ),
                    true,
                    false,
                    None,
                ),
                UpdateState::Available(info) => (
                    if info.size > 0 {
                        format!(
                            "Доступна версия {} · {:.1} МБ",
                            info.version,
                            info.size as f64 / (1024.0 * 1024.0)
                        )
                    } else {
                        format!("Доступна версия {}", info.version)
                    },
                    true,
                    true,
                    Some(info.url.clone()),
                ),
                UpdateState::Downloading => {
                    ("Загружаю обновление…".to_string(), false, false, None)
                }
                UpdateState::Error(e) => (format!("Ошибка: {e}"), true, false, None),
            }
        };

        let ai_status = self
            .ai_test
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default();
        let rejected = self.hotkeys.rejected();

        let mut builder = ViewportBuilder::default()
            .with_title("Sakura Screen Translator — Параметры")
            .with_inner_size([760.0, 560.0])
            .with_min_inner_size([680.0, 460.0]);
        builder = builder.with_icon(Arc::new(load_app_icon()));

        let mut settings = self.settings.clone();
        let mut out = crate::features::settings::ui::SettingsOutput::default();

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_settings"),
            builder,
            |ctx, _| {
                theme.apply(ctx);
                out = self.settings_ui.show(
                    ctx,
                    &theme,
                    &mut settings,
                    &SettingsContext {
                        update_status: &status,
                        update_check_enabled: check_enabled,
                        update_install_enabled: install_enabled,
                        ai_test_status: &ai_status,
                        ai_test_running: self.ai_test_running,
                        rejected_hotkeys: &rejected,
                        log_dir: logging::default_log_dir(),
                        history: &self.history,
                        translating: self.is_translating(),
                    },
                );

                if ctx.input(|i| i.viewport().close_requested())
                    || ctx.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    close = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            },
        );

        self.apply_settings_changes(settings, out, install_url);

        if close {
            self.settings_open = false;
            self.settings_ui.on_close();
            if let Err(e) = save_settings(&self.settings) {
                error!(error = %e, "Could not save settings");
            } else {
                self.settings_dirty = false;
            }
        }
    }

    fn apply_settings_changes(
        &mut self,
        next: Settings,
        out: crate::features::settings::ui::SettingsOutput,
        install_url: Option<String>,
    ) {
        let theme_changed = next.theme != self.settings.theme;
        let history_limit_changed = next.history_limit != self.settings.history_limit;
        let hotkeys_changed =
            out.hotkeys_changed || next.hotkeys.all() != self.settings.hotkeys.all();

        if !settings_equal(&next, &self.settings) {
            self.settings_dirty = true;
        }
        self.settings = next;

        if theme_changed {
            self.theme = Theme::resolve(self.settings.theme);
        }
        if history_limit_changed {
            self.history.set_limit(self.settings.history_limit);
        }
        if hotkeys_changed {
            self.hotkeys.update(self.settings.hotkeys);
            self.tray.update_hotkeys(self.settings.hotkeys);
        }
        if out.tray_changed {
            self.tray.set_visible(!self.settings.hide_tray_icon);
        }
        if out.engines_changed {
            // A translation produced by the engine the user just switched away
            // from is no longer the answer they want.
            crate::features::translation::cache::clear();
        }
        if out.logs_changed {
            logging::set_verbose(self.settings.logs.verbose);
        }
        if out.autostart_changed {
            if let Err(e) = set_autostart(self.settings.launch_at_startup, &get_current_exe_path())
            {
                error!(error = %e, "Could not update autostart");
            }
        }
        if out.clear_history {
            self.history.clear();
        }
        if out.open_log_dir {
            open_folder(&logging::default_log_dir());
        }
        if out.check_update {
            *self.update.lock().unwrap_or_else(|e| e.into_inner()) = UpdateState::Checking;
            spawn_update_check(&self.rt, Arc::clone(&self.update), egui::Context::default());
        }
        if out.install_update {
            if let Some(url) = install_url {
                *self.update.lock().unwrap_or_else(|e| e.into_inner()) = UpdateState::Downloading;
                let state = Arc::clone(&self.update);
                self.rt.spawn(async move {
                    if let Err(e) = download_and_apply(&url).await {
                        error!(error = %e, "Update failed");
                        *state.lock().unwrap_or_else(|p| p.into_inner()) = UpdateState::Error(e);
                    }
                });
            }
        }
        if out.test_ai && !self.ai_test_running {
            self.start_ai_test();
        }
    }

    fn start_ai_test(&mut self) {
        self.ai_test_running = true;
        *self.ai_test.lock().unwrap_or_else(|e| e.into_inner()) = Some("Проверяю…".to_string());

        let cfg = self.settings.engines.ai_config.clone();
        let target = self.settings.target_lang;
        let slot = Arc::clone(&self.ai_test);

        self.rt.spawn(async move {
            use crate::entities::language::Language;
            use crate::features::translation::providers::{ai, TranslateRequest};

            let req = TranslateRequest {
                text: "Hello, world.".to_string(),
                source: Language::En,
                target: if target == Language::En {
                    Language::Ru
                } else {
                    target
                },
            };
            let msg = match ai::translate(&req, &cfg).await {
                Ok(t) => format!("✓ Ответ получен: {}", logging::clip(t.trim(), 60)),
                Err(e) => format!("✗ {e}"),
            };
            *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(msg);
        });
    }

    fn poll_ai_test(&mut self) {
        if !self.ai_test_running {
            return;
        }
        let done = self
            .ai_test
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_deref()
            .is_some_and(|s| s != "Проверяю…");
        if done {
            self.ai_test_running = false;
        }
    }

    /// True while a capture is waiting on OCR or a translation engine.
    fn is_translating(&self) -> bool {
        self.result.as_ref().is_some_and(ResultUi::is_loading)
    }

    /// Tells the tray to spin its icon while work is in flight. Sent only on a
    /// change; the tray runs the animation itself from there.
    fn sync_tray_activity(&mut self) {
        let busy = self.is_translating();
        if busy != self.tray_busy {
            self.tray_busy = busy;
            self.tray.set_busy(busy);
        }
    }

    /// Repaint only as often as something is actually moving. The original asked
    /// for 20 fps forever, which kept a tray app's CPU busy around the clock.
    fn schedule_repaint(&self, ctx: &Context) {
        let busy = self.overlay.is_some()
            || self.result.as_ref().is_some_and(ResultUi::is_loading)
            || self.ai_test_running;

        let interval = if busy {
            Duration::from_millis(33)
        } else if self.result.is_some() || self.settings_open {
            Duration::from_millis(100)
        } else {
            // Idle: just often enough to notice a tray or hotkey event that
            // somehow did not request a repaint itself.
            Duration::from_secs(2)
        };
        ctx.request_repaint_after(interval);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain_events(ctx);
        self.collect_pipeline_result();
        self.poll_ai_test();
        self.sync_tray_activity();

        if self.overlay.is_some() {
            self.show_overlay(ctx);
        }
        if self.result.is_some() {
            self.show_result(ctx);
        }
        if self.settings_open {
            self.show_settings(ctx);
        }

        // The host window is a 1×1 placeholder parked off screen; closing it
        // would take the whole app down, which only the tray Exit may do.
        if ctx.input(|i| i.viewport().close_requested()) && !self.should_exit {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }

        self.schedule_repaint(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.hotkeys.shutdown();
        self.tray.shutdown();
        if self.settings_dirty {
            let _ = save_settings(&self.settings);
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn settings_equal(a: &Settings, b: &Settings) -> bool {
    // Cheap and exact enough: if the serialised form matches, nothing needs
    // writing to disk.
    match (toml::to_string(a), toml::to_string(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn spawn_update_check(rt: &tokio::runtime::Runtime, state: Arc<Mutex<UpdateState>>, ctx: Context) {
    rt.spawn(async move {
        let next = match check_for_update().await {
            Ok(Some(info)) => {
                info!(version = %info.version, "Update available");
                UpdateState::Available(info)
            }
            Ok(None) => UpdateState::UpToDate,
            Err(e) => {
                warn!(error = %e, "Update check failed");
                UpdateState::Error(e)
            }
        };
        *state.lock().unwrap_or_else(|p| p.into_inner()) = next;
        ctx.request_repaint();
    });
}

fn load_app_icon() -> egui::IconData {
    crate::features::tray::app_icon(256)
}

fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("explorer")
            .arg(path)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
}

fn beep() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows::Win32::UI::WindowsAndMessaging::MB_OK;
        let _ = MessageBeep(MB_OK);
    }
}

// Keeps the widget module referenced even in builds where nothing else uses it
// directly from here.
#[allow(unused)]
type _Widgets = widgets::IconFn;
