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
use crate::shared::i18n::t;
use crate::shared::logging;
use crate::shared::utils::autostart::{get_current_exe_path, set_autostart};
use crate::shared::utils::clipboard::copy_text_to_clipboard;
use crate::shared::utils::notify;
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
    Downloading { received: u64, total: u64 },
    /// Installed and waiting to be restarted. The restart happens on the UI
    /// thread so the settings are saved and the tray icon removed first.
    Installed(std::path::PathBuf),
    Error(String),
}

/// The error side carries the message and whether it is a failure at all:
/// "no text in this rectangle" is an outcome, not a fault.
type PipelineSlot = Arc<Mutex<Option<(u64, Result<PipelineResult, (String, bool)>)>>>;

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
    /// The settings window has just been asked for and still needs to be
    /// brought to the front. An accessory application does not come forward
    /// on its own — but doing it every frame, as this used to, means the
    /// user cannot click away into another app at all.
    settings_wants_focus: bool,
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

        // This is a menu-bar app, not a windowed one. Saying so removes the Dock
        // icon — and, more importantly, the Space the application would
        // otherwise own and drag the user back to when the overlay opens.
        #[cfg(target_os = "macos")]
        crate::features::capture::mac_window::become_menu_bar_agent();

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
            settings_wants_focus: open_settings.is_some(),
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
                    self.settings_wants_focus = true;
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
                self.show_stage(Stage::Error(e.user_message()), bounds, geometry);
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
            let outcome = run_pipeline(&jpeg, &params).await.map_err(|e| {
                // Full detail to the log, a sentence to the screen — and a
                // flag for whether this is a failure at all.
                let empty = matches!(e, crate::shared::error::AppError::NoText);
                if empty {
                    info!("Capture contained no text");
                } else {
                    error!(error = %e, "Pipeline failed");
                }
                (e.user_message(), empty)
            });
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
                if self.settings.keep_history {
                    self.history.push(HistoryEntry {
                        id: History::next_id(),
                        original: res.original.clone(),
                        translated: res.translated.clone(),
                        source: res.source,
                        target: res.target,
                        engine: res.engine,
                    });
                }

                if self.settings.result_view == ResultView::None {
                    // Clipboard-only mode: nothing to show, and no empty popup.
                    self.result = None;
                    return;
                }
                if let Some(ui) = self.result.as_mut() {
                    ui.stage = Stage::Done(Box::new(res));
                }
            }
            Err((msg, empty)) => {
                // Worth showing either way, even in clipboard-only mode:
                // otherwise a capture that produced nothing is
                // indistinguishable from one that worked.
                let stage = if empty {
                    Stage::Empty(msg)
                } else {
                    Stage::Error(msg)
                };
                if let Some(ui) = self.result.as_mut() {
                    ui.stage = stage;
                    if ui.view == ResultView::None {
                        ui.view = ResultView::Popup;
                    }
                } else {
                    let (bounds, geometry) = self
                        .last_capture
                        .unwrap_or((virtual_desktop(), Geometry::new(virtual_desktop(), 1.0)));
                    self.show_stage(stage, bounds, geometry);
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

        let builder = ViewportBuilder::default()
            .with_position(geometry.window_pos_points())
            .with_inner_size(geometry.window_size_points())
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(true)
            .with_always_on_top();

        // The title is never drawn — the window has no chrome — but on macOS it
        // is how this window is picked out of `NSApp.windows` so it can be
        // raised over the menu bar and the Dock.
        //
        // It is also created *inactive*: letting winit make it key straight away
        // would activate the application, and an application that has not yet
        // been told this window belongs on every Space gets pulled back to its
        // own — taking the user out of the full-screen app they were reading.
        // `present_overlay` orders it front and takes the keyboard instead.
        #[cfg(target_os = "macos")]
        let builder = builder
            .with_title(crate::features::capture::mac_window::OVERLAY_TITLE)
            .with_active(false);

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_capture"),
            builder,
            |ctx, _| {
                // Without focus the overlay never sees a key press, and Esc does
                // nothing however clearly the hint advertises it. macOS goes
                // through AppKit instead: winit's focus call activates the
                // application, which is exactly the Space-switch being avoided.
                #[cfg(not(target_os = "macos"))]
                if !ctx.input(|i| i.focused) {
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }

                // Re-applied each frame: the window is created by the frame
                // before this one, and AppKit resets the frame if a display is
                // reconfigured mid-capture.
                #[cfg(target_os = "macos")]
                crate::features::capture::mac_window::present_overlay(
                    crate::features::capture::mac_window::OVERLAY_TITLE,
                );

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
        // Re-read every frame: toggling the setting should take effect on the
        // result already on screen, not on the next one.
        ui.close_on_focus_loss = self.settings.close_result_on_focus_loss;
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

    /// Starts the newly installed build and quits this one.
    ///
    /// Done here rather than inside the installer, which runs on a worker
    /// thread and used to call `process::exit(0)` — that skipped saving the
    /// settings, left the tray icon behind, and from the outside looked exactly
    /// like the app crashing after a download.
    fn restart_if_updated(&mut self, ctx: &Context) {
        let path = {
            let state = self.update.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                UpdateState::Installed(path) => path.clone(),
                _ => return,
            }
        };

        // The new copy opens on the About page, so the first thing the user sees
        // is the version they just installed rather than an app that vanished.
        match std::process::Command::new(&path)
            .arg("--settings")
            .arg("about")
            .spawn()
        {
            Ok(_) => {
                info!(path = %path.display(), "Restarting into the new version");
                self.shutdown(ctx);
            }
            Err(e) => {
                error!(error = %e, "Could not start the new version");
                *self.update.lock().unwrap_or_else(|p| p.into_inner()) = UpdateState::Error(
                    t("The update is installed but could not be started — open the program again")
                        .to_string(),
                );
            }
        }
    }

    /// Tells the user a new release exists, once per release.
    ///
    /// The app has no window of its own most of the time, so the only place a
    /// notice would otherwise appear is a settings page nobody has open. The
    /// version is remembered so the same release does not announce itself at
    /// every launch — turning the setting off and on again does not replay it
    /// either, which is the behaviour a user would expect from "notify me".
    fn announce_update(&mut self) {
        if !self.settings.notify_about_updates {
            return;
        }

        let available = {
            let state = self.update.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                UpdateState::Available(info) => Some(info.version.clone()),
                _ => None,
            }
        };
        let Some(version) = available else { return };

        if self.settings.notified_version == version {
            return;
        }
        self.settings.notified_version.clone_from(&version);
        self.settings_dirty = true;

        info!(%version, "Notifying about a new release");
        notify::show(
            "Sakura Screen Translator",
            &t("Version {version} is available. You can install it in Settings → About.")
                .replace("{version}", &version),
        );
    }

    fn show_settings(&mut self, ctx: &Context) {
        let mut close = false;
        let theme = self.theme;

        // Filled in while reading the update state, so the row can draw a bar.
        let mut progress: Option<f32> = None;
        let (status, check_enabled, install_enabled, install_url) = {
            let st = self.update.lock().unwrap_or_else(|e| e.into_inner());
            match &*st {
                UpdateState::Checking => {
                    (t("Checking for updates…").to_string(), false, false, None)
                }
                UpdateState::UpToDate => (
                    t("You are on the latest version ({version})")
                        .replace("{version}", env!("CARGO_PKG_VERSION")),
                    true,
                    false,
                    None,
                ),
                UpdateState::Available(info) => (
                    if info.size > 0 {
                        t("Version {version} is available · {size} MB")
                            .replace("{version}", &info.version)
                            .replace(
                                "{size}",
                                &format!("{:.1}", info.size as f64 / (1024.0 * 1024.0)),
                            )
                    } else {
                        t("Version {version} is available").replace("{version}", &info.version)
                    },
                    true,
                    true,
                    Some(info.url.clone()),
                ),
                UpdateState::Downloading { received, total } => {
                    // A ten-megabyte download with no sign of movement looks
                    // like a hang. Bytes when the server gives a length, and
                    // a running total when it does not.
                    let text = if *total > 0 {
                        t("Downloading the update… {done} of {total} MB")
                            .replace("{done}", &format!("{:.1}", *received as f64 / 1048576.0))
                            .replace("{total}", &format!("{:.1}", *total as f64 / 1048576.0))
                    } else {
                        t("Downloading the update… {done} MB")
                            .replace("{done}", &format!("{:.1}", *received as f64 / 1048576.0))
                    };
                    progress = if *total > 0 {
                        Some(*received as f32 / *total as f32)
                    } else {
                        None
                    };
                    (text, false, false, None)
                }
                UpdateState::Installed(_) => (
                    t("Update installed — restarting…").to_string(),
                    false,
                    false,
                    None,
                ),
                UpdateState::Error(e) => (
                    // Already a sentence written for the user; wrapping it in
                    // "Error:" only makes it look like a crash report.
                    e.clone(),
                    true,
                    false,
                    None,
                ),
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
            .with_title(t("Sakura Screen Translator — Settings"))
            .with_inner_size([785.0, 662.0])
            .with_min_inner_size([680.0, 460.0]);
        builder = builder.with_icon(Arc::new(load_app_icon()));

        // Read before the closure borrows `self`.
        let wants_focus = std::mem::take(&mut self.settings_wants_focus);

        let mut settings = self.settings.clone();
        let mut out = crate::features::settings::ui::SettingsOutput::default();

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_settings"),
            builder,
            |ctx, _| {
                // Once, when the window is asked for. An accessory
                // application does not come forward on its own, but doing
                // this every frame pins the focus here and the user cannot
                // switch to anything else while settings are open.
                if wants_focus {
                    #[cfg(target_os = "macos")]
                    crate::features::capture::mac_window::activate();
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }

                theme.apply(ctx);
                out = self.settings_ui.show(
                    ctx,
                    &theme,
                    &mut settings,
                    &SettingsContext {
                        update_status: &status,
                        update_check_enabled: check_enabled,
                        update_install_enabled: install_enabled,
                    update_progress: progress,
                        ai_test_status: &ai_status,
                        ai_test_running: self.ai_test_running,
                        rejected_hotkeys: &rejected,
                        log_dir: logging::default_log_dir(),
                        history: &self.history,
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

        self.apply_settings_changes(ctx, settings, out, install_url);

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
        ctx: &Context,
        next: Settings,
        out: crate::features::settings::ui::SettingsOutput,
        install_url: Option<String>,
    ) {
        let theme_changed = next.theme != self.settings.theme;
        let language_changed = next.ui_language != self.settings.ui_language;
        let history_limit_changed = next.history_limit != self.settings.history_limit;
        // Turning the history off throws away what is already in it: leaving
        // the list behind would be keeping exactly what the user just asked
        // not to keep.
        let history_switched_off = self.settings.keep_history && !next.keep_history;
        let hotkeys_changed =
            out.hotkeys_changed || next.hotkeys.all() != self.settings.hotkeys.all();

        if !settings_equal(&next, &self.settings) {
            self.settings_dirty = true;
        }
        self.settings = next;

        // Applied before anything else reads a string this frame.
        if language_changed {
            let language = self
                .settings
                .ui_language
                .or_else(crate::shared::i18n::detect_system)
                .unwrap_or(crate::shared::i18n::Lang::En);
            crate::shared::i18n::set(language);
            // Fonts are chosen for the language, so they have to be rebuilt
            // with it. Without this, an app started in Japanese keeps the CJK
            // face at the front of the family after a switch to Russian, and
            // Cyrillic comes out in full-width CJK metrics — letters spaced
            // like a ransom note.
            ctx.set_fonts(crate::ui::theme::build_fonts());
            self.tray.update_hotkeys(self.settings.hotkeys);
        }
        if theme_changed {
            self.theme = Theme::resolve(self.settings.theme);
        }
        if history_limit_changed {
            self.history.set_limit(self.settings.history_limit);
        }
        if history_switched_off {
            self.history.clear();
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
                *self.update.lock().unwrap_or_else(|e| e.into_inner()) =
                    UpdateState::Downloading {
                        received: 0,
                        total: 0,
                    };
                let state = Arc::clone(&self.update);
                let repaint = ctx.clone();
                self.rt.spawn(async move {
                    let progress_state = Arc::clone(&state);
                    let progress_ctx = repaint.clone();
                    let outcome = download_and_apply(&url, move |received, total| {
                        *progress_state.lock().unwrap_or_else(|p| p.into_inner()) =
                            UpdateState::Downloading { received, total };
                        // The window is idle while this runs; without a nudge
                        // the bar would only move when something else happened
                        // to wake it.
                        progress_ctx.request_repaint();
                    })
                    .await;

                    *state.lock().unwrap_or_else(|p| p.into_inner()) = match outcome {
                        Ok(path) => UpdateState::Installed(path),
                        Err(e) => {
                            error!(error = %e, "Update failed");
                            UpdateState::Error(e)
                        }
                    };
                    repaint.request_repaint();
                });
            }
        }
        if out.test_ai && !self.ai_test_running {
            self.start_ai_test();
        }
    }

    fn start_ai_test(&mut self) {
        self.ai_test_running = true;
        *self.ai_test.lock().unwrap_or_else(|e| e.into_inner()) = Some(t("Checking…").to_string());

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
                Ok(reply) => t("✓ Got a reply: {reply}")
                    .replace("{reply}", logging::clip(reply.trim(), 60)),
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
            .is_some_and(|s| s != t("Checking…"));
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
    fn sync_tray_activity(&mut self, ctx: &Context) {
        let busy = self.is_translating();
        if busy != self.tray_busy {
            self.tray_busy = busy;
            self.tray.set_busy(busy);
        }

        // macOS and Linux have no tray thread of their own to run the spin on —
        // a status item may only be touched from this thread — so the frames
        // come from here, and keep coming until the mark has settled.
        if self.tray.tick() {
            ctx.request_repaint_after(Duration::from_millis(50));
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
        self.sync_tray_activity(ctx);
        self.announce_update();
        self.restart_if_updated(ctx);

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
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Part of xdg-utils, present on every desktop install.
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

fn beep() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows::Win32::UI::WindowsAndMessaging::MB_OK;
        let _ = MessageBeep(MB_OK);
    }
    #[cfg(target_os = "macos")]
    {
        // Detached on purpose: the capture should not wait on the sound.
        let _ = std::process::Command::new("afplay")
            .arg("/System/Library/Sounds/Tink.aiff")
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // canberra-gtk-play ships with most desktops; the terminal bell is the
        // fallback when it does not.
        if std::process::Command::new("canberra-gtk-play")
            .args(["-i", "message"])
            .spawn()
            .is_err()
        {
            use std::io::Write as _;
            let mut out = std::io::stdout();
            let _ = out.write_all(b"\x07");
            let _ = out.flush();
        }
    }
}

// Keeps the widget module referenced even in builds where nothing else uses it
// directly from here.
#[allow(unused)]
type _Widgets = widgets::IconFn;
