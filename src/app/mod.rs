//! Application orchestrator.
//!
//! Owns the settings and the two background helpers (hotkeys, tray), and hosts
//! every window as an egui viewport. The work itself lives next door:
//!
//! * [`session`] — one capture, from the shortcut to the translation on screen;
//! * [`result`] — how that translation is shown;
//! * [`updates`] — finding and installing a newer release.
//!
//! Settings live here as a plain value rather than behind an `Arc<Mutex<_>>`.
//! Only the UI thread touches them; background work gets an immutable snapshot
//! at spawn time. That removes the whole class of "lock poisoned, app silently
//! stops working" failures the previous structure was prone to.

#[cfg(debug_assertions)]
mod automation;
pub mod result;
mod session;
mod updates;

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{Context, ViewportBuilder, ViewportCommand, ViewportId};
use tracing::{error, info, warn};

use crate::entities::history::History;
use crate::entities::settings::{CaptureMode, EngineKind, HotkeyAction, ResultView, Settings};
use crate::features::capture::overlay::Purpose;
use crate::features::capture::{cursor_position, monitor_for, native, Bounds, OverlayState};
use crate::features::hotkey::HotkeyManager;
use crate::features::settings::save_settings;
use crate::features::settings::ui::{Section, SettingsContext, SettingsOutput, SettingsUi};
use crate::features::translation::PipelineResult;
use crate::features::tray::{TrayEvent, TrayManager};
use crate::shared::i18n::t;
use crate::shared::logging;
use crate::shared::utils::autostart::{get_current_exe_path, set_autostart};
use crate::shared::utils::clipboard::copy_text_to_clipboard;
use crate::shared::utils::tts;
use crate::ui::theme::Theme;

use result::{ResultAction, ResultUi, Stage};
use session::{Job, JobEvent, LastCapture, Pending, Watch};
use updates::{SharedUpdate, UpdateState};

/// Whether the app has been asked not to bring its windows to the front.
///
/// Set by `SAKURA_NO_FOCUS`, for checking the app on a machine someone is
/// using: its windows still open, but none of them takes the keyboard from
/// whatever that person is doing.
pub(crate) fn stays_in_background() -> bool {
    static ASKED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ASKED.get_or_init(|| std::env::var_os("SAKURA_NO_FOCUS").is_some())
}

/// Something the command line asked to happen as soon as the app is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Startup {
    /// Translate this rectangle of the screen, in desktop pixels.
    Area(Bounds),
}

pub struct App {
    settings: Settings,
    theme: Theme,
    history: History,
    rt: tokio::runtime::Runtime,

    hotkeys: HotkeyManager,
    tray: TrayManager,

    // ── The capture in progress ──────────────────────────────────────────────
    /// The selection overlay, while the user is choosing what to translate.
    overlay: Option<OverlayState>,
    /// The translation on screen, from "working…" to the answer.
    result: Option<ResultUi>,
    /// The translation in flight, if any.
    job: Option<Job>,
    job_events: (Sender<JobEvent>, Receiver<JobEvent>),
    /// A capture waiting for the app's own windows to leave the screen.
    pending: Option<Pending>,
    /// The area being translated again whenever its text changes.
    watch: Option<Watch>,
    last_capture: Option<LastCapture>,
    /// Scale factor of the last capture, for a result that has to be rebuilt
    /// without one at hand.
    last_ppp: f32,
    startup: Option<Startup>,

    // ── The settings window ──────────────────────────────────────────────────
    settings_open: bool,
    /// The settings window has just been asked for and still needs to be
    /// brought to the front. An accessory application does not come forward
    /// on its own — but doing it every frame means the user cannot click away
    /// into another app at all.
    settings_wants_focus: bool,
    settings_ui: SettingsUi,
    settings_dirty: bool,

    update: SharedUpdate,
    ai_test: Arc<Mutex<Option<String>>>,
    ai_test_running: bool,
    /// Last value handed to the tray, so the animation is only poked on a change.
    tray_busy: bool,

    /// Steps to carry out from a file, in a development build. See
    /// [`automation`].
    #[cfg(debug_assertions)]
    script: Option<automation::Script>,

    should_exit: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        open_settings: Option<Section>,
        startup: Option<Startup>,
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

        let update: SharedUpdate = Arc::new(Mutex::new(UpdateState::Checking));
        updates::spawn_check(&rt, Arc::clone(&update), cc.egui_ctx.clone());

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
            job: None,
            job_events: channel(),
            pending: None,
            watch: None,
            last_capture: None,
            last_ppp: 1.0,
            startup,
            settings_open: open_settings.is_some(),
            settings_wants_focus: open_settings.is_some(),
            settings_ui: SettingsUi::new(open_settings.unwrap_or(Section::General)),
            settings_dirty: false,
            update,
            ai_test: Arc::new(Mutex::new(None)),
            ai_test_running: false,
            tray_busy: false,
            #[cfg(debug_assertions)]
            script: automation::Script::from_env(),
            should_exit: false,
        }
    }

    // ── Input ────────────────────────────────────────────────────────────────

    fn drain_events(&mut self, ctx: &Context) {
        if let Some(Startup::Area(bounds)) = self.startup.take() {
            self.capture_area(ctx, bounds);
        }
        #[cfg(debug_assertions)]
        self.run_script(ctx);

        for action in self.hotkeys.poll() {
            match action {
                HotkeyAction::Region => {
                    self.begin_capture(ctx, CaptureMode::Region, Purpose::Translate)
                }
                HotkeyAction::Window => {
                    self.begin_capture(ctx, CaptureMode::Window, Purpose::Translate)
                }
                HotkeyAction::FullScreen => {
                    self.begin_capture(ctx, CaptureMode::FullScreen, Purpose::Translate)
                }
                HotkeyAction::Repeat => self.repeat_last_capture(ctx),
                HotkeyAction::Watch => self.watch_or_stop(ctx),
                HotkeyAction::Hide => self.dismiss(),
            }
        }

        for event in self.tray.poll() {
            match event {
                TrayEvent::Capture(mode) => self.begin_capture(ctx, mode, Purpose::Translate),
                TrayEvent::Watch => self.watch_or_stop(ctx),
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

    /// The watch shortcut is a toggle: it starts a watch, and pressed again it
    /// ends the one that is running.
    fn watch_or_stop(&mut self, ctx: &Context) {
        if self.is_watching() {
            self.dismiss();
        } else {
            self.begin_capture(ctx, CaptureMode::Region, Purpose::Watch);
        }
    }

    /// Takes the translation off the screen and stops whatever was producing
    /// it.
    fn dismiss(&mut self) {
        self.stop_watching();
        self.cancel_job();
        self.result = None;
    }

    fn shutdown(&mut self, ctx: &Context) {
        if self.settings_dirty {
            if let Err(e) = save_settings(&self.settings) {
                error!(error = %e, "Could not save settings on exit");
            }
            self.settings_dirty = false;
        }
        self.cancel_job();
        self.hotkeys.shutdown();
        self.tray.shutdown();
        self.should_exit = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    // ── The result on screen ─────────────────────────────────────────────────

    fn show_result(&mut self, ctx: &Context) {
        let Some(mut ui) = self.result.take() else {
            return;
        };
        let theme = self.theme;
        // Re-read every frame: a change in the settings should take effect on
        // the result already on screen, not on the next one.
        ui.style = self.settings.presentation;
        ui.close_on_click_away = self.settings.close_result_on_focus_loss;
        ui.watching = self.is_watching();
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
                    ui.set_view(view);
                    if view == ResultView::Window {
                        ui.pinned = self.settings.pin_result_window;
                    }
                }
                ResultAction::ToggleWatch => {
                    // Needs the result back in place to read its capture from.
                    self.result = Some(ui);
                    self.toggle_watch();
                    match self.result.take() {
                        Some(back) => ui = back,
                        None => return,
                    }
                }
                ResultAction::Close => close = true,
            }
        }

        if close {
            // Closing the translation of a watched area ends the watch: there
            // is no point reading what nobody is being shown.
            self.stop_watching();
            self.cancel_job();
        } else {
            self.result = Some(ui);
        }
    }

    /// Puts a sample subtitle on screen, so its size, place and opacity can be
    /// tuned without capturing anything.
    fn show_sample(&mut self, ctx: &Context) {
        self.dismiss();

        let monitor = cursor_position()
            .map(|(x, y)| monitor_for(Bounds { x, y, w: 1, h: 1 }))
            .unwrap_or_else(|| {
                monitor_for(Bounds {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1,
                })
            });
        // Somewhere out of the subtitle's way: it steps aside from what it
        // translates, and a sample translates nothing.
        let anchor = Bounds {
            x: monitor.bounds.x + monitor.bounds.w / 2,
            y: monitor.bounds.y + monitor.bounds.h / 3,
            w: 2,
            h: 2,
        };

        let mut ui = ResultUi::new(
            ResultView::Subtitle,
            anchor,
            monitor,
            ctx.pixels_per_point(),
        );
        ui.keep = true;
        ui.set_stage(Stage::Done(Box::new(PipelineResult {
            original: "You need the silver key to open this door.".to_string(),
            translated: t("This is what a translated line will look like.").to_string(),
            segments: Vec::new(),
            engine: self
                .settings
                .engines
                .active()
                .first()
                .copied()
                .unwrap_or(EngineKind::Yandex),
            source: crate::entities::language::Language::En,
            target: self.settings.target_lang,
            elapsed: Duration::ZERO,
            engine_elapsed: Duration::ZERO,
            from_cache: false,
        })));
        self.result = Some(ui);
    }

    /// Holds Esc as a global shortcut for exactly as long as there is a
    /// floating translation to dismiss with it.
    ///
    /// Not while an area is being watched: that translation stays up for as
    /// long as the game is played, and Esc is the game's menu key.
    fn sync_escape(&self) {
        let wanted = native::QUIET_WINDOWS
            && self.settings.presentation.esc_hides
            && self.overlay.is_none()
            && !self.is_watching()
            && self
                .result
                .as_ref()
                .is_some_and(|ui| ui.effective_view().floats());
        self.hotkeys.set_dismiss(wanted);
    }

    // ── The settings window ──────────────────────────────────────────────────

    fn show_settings(&mut self, ctx: &Context) {
        let mut close = false;
        let theme = self.theme;
        let update = self.update_row();

        let ai_status = self
            .ai_test
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default();
        let rejected = self.hotkeys.rejected();
        let quiet = stays_in_background();

        let builder = ViewportBuilder::default()
            .with_title(t("Sakura Screen Translator — Settings"))
            .with_inner_size([820.0, 680.0])
            .with_min_inner_size([700.0, 480.0])
            .with_active(!quiet)
            .with_icon(Arc::new(load_app_icon()));

        // Read before the closure borrows `self`.
        let wants_focus = std::mem::take(&mut self.settings_wants_focus) && !quiet;

        let mut settings = self.settings.clone();
        let mut out = SettingsOutput::default();

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_settings"),
            builder,
            |ctx, _| {
                // Once, when the window is asked for. An accessory application
                // does not come forward on its own, but doing this every frame
                // pins the focus here and the user cannot switch to anything
                // else while settings are open.
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
                        update_status: &update.status,
                        update_check_enabled: update.can_check,
                        update_install_enabled: update.can_install,
                        update_progress: update.progress,
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

        self.apply_settings_changes(ctx, settings, out, update.url);

        if close {
            self.settings_open = false;
            self.settings_ui.on_close();
            // A sample put up from this window has no business outliving it.
            if self.result.as_ref().is_some_and(|ui| ui.keep) {
                self.result = None;
            }
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
        out: SettingsOutput,
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
            self.check_for_update(ctx);
        }
        if out.install_update {
            if let Some(url) = install_url {
                self.install_update(ctx, url);
            }
        }
        if out.test_ai && !self.ai_test_running {
            self.start_ai_test(ctx);
        }
        if out.show_sample {
            self.show_sample(ctx);
        }
    }

    fn start_ai_test(&mut self, ctx: &Context) {
        self.ai_test_running = true;
        *self.ai_test.lock().unwrap_or_else(|e| e.into_inner()) = Some(t("Checking…").to_string());

        let cfg = self.settings.engines.ai_config.clone();
        let target = self.settings.target_lang;
        let slot = Arc::clone(&self.ai_test);
        let repaint = ctx.clone();

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
                Err(e) => format!("✗ {}", e.user_message()),
            };
            *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(msg);
            repaint.request_repaint();
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

    // ── Housekeeping ─────────────────────────────────────────────────────────

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

    /// Repaint only as often as something is actually moving. An idle tray app
    /// that draws twenty frames a second keeps a CPU core warm around the
    /// clock for nothing.
    fn schedule_repaint(&self, ctx: &Context) {
        let moving = self.overlay.is_some()
            || self.pending.is_some()
            || self.ai_test_running
            || self.result.as_ref().is_some_and(ResultUi::is_animating);

        let interval = if moving {
            Duration::from_millis(16)
        } else if let Some(ui) = &self.result {
            // A floating view cannot tell from focus that the user clicked
            // somewhere else; it has to look, and often enough not to miss a
            // click.
            let polls_the_mouse = native::QUIET_WINDOWS
                && ui.effective_view().floats()
                && self.settings.close_result_on_focus_loss
                && !self.is_watching()
                && !ui.keep;
            if polls_the_mouse {
                Duration::from_millis(40)
            } else {
                Duration::from_millis(100)
            }
        } else if self.is_watching() {
            Duration::from_millis(self.settings.watch_interval_ms.clamp(300, 5000) as u64 / 2)
        } else if self.settings_open {
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
        self.collect_job_events();
        self.run_pending(ctx);
        self.tick_watch(ctx);
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
        self.sync_escape();

        // The host window is a 1×1 placeholder parked off screen; closing it
        // would take the whole app down, which only the tray Exit may do.
        if ctx.input(|i| i.viewport().close_requested()) && !self.should_exit {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }

        self.schedule_repaint(ctx);
    }

    /// The host window is asked for with an alpha channel so that the floating
    /// views — which share its GL configuration — can have one too. Nothing is
    /// ever drawn into it.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cancel_job();
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
