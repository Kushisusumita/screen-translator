//! One capture, from the shortcut to the translation on screen.
//!
//! ```text
//! shortcut ─► freeze the desktop ─► select ─► crop ─► read + translate ─► show
//!                                                └───────────── watch ◄──────┘
//! ```
//!
//! Three rules shape everything here:
//!
//! * **One frame.** The desktop is grabbed once, when the shortcut is pressed.
//!   What the user selects on, what is recognised, and what the in-place view
//!   paints over are all that one image — never a second grab that could show
//!   the overlay itself, or the next line of dialogue.
//! * **One job.** A newer capture cancels the one in flight instead of racing
//!   it; a stale answer cannot overwrite a fresh one because it never arrives.
//! * **Nothing slow on the UI thread.** Encoding, recognition and translation
//!   run on the worker pool and report back over a channel.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use egui::{Context, ViewportBuilder, ViewportCommand, ViewportId};
use tracing::{debug, error, info, warn};

use super::result::{ResultUi, Stage};
use super::App;
use crate::entities::history::{History, HistoryEntry};
use crate::entities::settings::{CaptureMode, ResultView};
use crate::features::capture::frame::Signature;
use crate::features::capture::overlay::{self, Purpose};
use crate::features::capture::{
    capture_desktop, capture_frame, cursor_position, foreground_window_bounds, monitor_for,
    native, too_small, virtual_desktop, window_pick, Bounds, Frame, Geometry, OverlayState,
};
use crate::features::translation::{
    run_pipeline, CaptureKind, PipelineParams, PipelineResult, Progress,
};
use crate::shared::error::AppError;
use crate::shared::utils::clipboard::copy_text_to_clipboard;

/// Hands out job ids. A report carrying any other id than the current job's
/// belongs to work that has since been cancelled.
static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

/// How many frames to let pass after taking a window of ours off the screen
/// before grabbing the screen. The window is gone from the app's side at once;
/// the compositor needs a frame or two to agree.
const SETTLE_FRAMES: u8 = 3;

/// A translation in flight.
pub(super) struct Job {
    id: u64,
    handle: tokio::task::JoinHandle<()>,
    /// A watched area being read again: the result on screen stays as it is
    /// until there is something new to replace it with.
    quiet: bool,
    /// The pixels the job is working from, kept for the view that paints on
    /// them.
    shot: Frame,
}

/// What a job reports back.
pub(super) enum JobEvent {
    Progress(u64, Progress),
    Finished(u64, Result<PipelineResult, Failure>),
}

/// Why a capture produced no translation, in words for the screen.
pub(super) struct Failure {
    message: String,
    /// Nothing went wrong: there was simply no text. An outcome, not a fault.
    empty: bool,
}

/// The last thing captured, so it can be captured again.
#[derive(Debug, Clone, Copy)]
pub(super) struct LastCapture {
    bounds: Bounds,
    kind: CaptureKind,
}

/// Something that has to wait until the app's own windows are off the screen.
pub(super) enum Pending {
    /// Freeze the desktop and let the user select.
    Overlay {
        mode: CaptureMode,
        purpose: Purpose,
        frames: u8,
    },
    /// Grab an area that is already known.
    Grab {
        bounds: Bounds,
        kind: CaptureKind,
        purpose: Purpose,
        frames: u8,
    },
}

/// An area being translated again whenever its text changes.
pub(super) struct Watch {
    bounds: Bounds,
    /// What the area looked like when it was last sent off.
    translated: Signature,
    /// What it looked like at the previous look.
    last_seen: Signature,
    last_look: Instant,
    /// Since when the area has differed from what was translated.
    changed_since: Option<Instant>,
    last_job: Instant,
    /// The least time between two jobs. Grows while looks keep finding the
    /// same text — a moving background, not new dialogue — and resets when
    /// the text really changes.
    pause: Duration,
    /// The text of the last successful read.
    last_text: Option<String>,
}

/// A share of the picture small enough to be the mouse pointer passing
/// through; anything past it is treated as a change.
const CHANGED: f32 = 0.006;
/// Two looks this close are the same picture: text that has finished
/// appearing, as opposed to text still being typed out.
const SETTLED: f32 = 0.004;
const SHORTEST_PAUSE: Duration = Duration::from_millis(1200);
const LONGEST_PAUSE: Duration = Duration::from_secs(12);
/// A picture that never settles is read anyway after this long.
const OVERDUE: Duration = Duration::from_secs(3);

impl Watch {
    fn new(shot: &Frame) -> Self {
        let signature = shot.signature();
        let now = Instant::now();
        Self {
            bounds: shot.bounds,
            translated: signature.clone(),
            last_seen: signature,
            last_look: now,
            changed_since: None,
            last_job: now,
            pause: SHORTEST_PAUSE,
            last_text: None,
        }
    }

    /// Takes in a fresh look at the area and says whether it should be read
    /// again.
    ///
    /// Two things have to be true: the picture differs from the one that was
    /// translated, and it has stopped changing — a dialogue line that is still
    /// being typed out letter by letter is not worth reading yet. A picture
    /// that never settles (an animated backdrop) is read anyway once it has
    /// been different for long enough.
    fn should_read(&mut self, seen: &Signature, now: Instant) -> bool {
        let from_translated = seen.difference(&self.translated);
        let from_previous = seen.difference(&self.last_seen);
        self.last_seen = seen.clone();
        self.last_look = now;

        if from_translated < CHANGED {
            self.changed_since = None;
            return false;
        }

        let since = *self.changed_since.get_or_insert(now);
        let settled = from_previous < SETTLED;
        let overdue = now.duration_since(since) >= OVERDUE;
        (settled || overdue) && now.duration_since(self.last_job) >= self.pause
    }

    fn sent(&mut self, seen: Signature, now: Instant) {
        self.translated = seen;
        self.changed_since = None;
        self.last_job = now;
    }

    /// Records what a read came back with, and says whether it is new.
    fn read(&mut self, text: &str) -> bool {
        if self.last_text.as_deref() == Some(text) {
            // The picture moved but the words did not. Look less often.
            self.pause = (self.pause * 2).min(LONGEST_PAUSE);
            false
        } else {
            self.last_text = Some(text.to_string());
            self.pause = SHORTEST_PAUSE;
            true
        }
    }
}

impl App {
    // ── Starting a capture ───────────────────────────────────────────────────

    pub(super) fn begin_capture(&mut self, ctx: &Context, mode: CaptureMode, purpose: Purpose) {
        if self.overlay.is_some() {
            // Already selecting; a second press should not stack overlays.
            return;
        }
        self.stop_watching();
        self.cancel_job();

        // Anything of ours still on screen would be baked into the frozen
        // backdrop, so it goes first and the grab waits for it to be gone.
        let frames = if self.result.take().is_some() {
            SETTLE_FRAMES
        } else {
            0
        };
        self.pending = Some(Pending::Overlay {
            mode,
            purpose,
            frames,
        });
        ctx.request_repaint();
    }

    /// Translates the area captured last time once more, as it is now.
    pub(super) fn repeat_last_capture(&mut self, ctx: &Context) {
        let Some(last) = self.last_capture else {
            info!("Repeat requested but nothing has been captured yet");
            return;
        };
        if self.overlay.is_some() {
            return;
        }
        self.stop_watching();
        self.cancel_job();
        let frames = if self.result.take().is_some() {
            SETTLE_FRAMES
        } else {
            0
        };
        self.pending = Some(Pending::Grab {
            bounds: last.bounds,
            kind: last.kind,
            purpose: Purpose::Translate,
            frames,
        });
        ctx.request_repaint();
    }

    /// Translates a given rectangle of the screen — the command-line route.
    pub(super) fn capture_area(&mut self, ctx: &Context, bounds: Bounds) {
        self.pending = Some(Pending::Grab {
            bounds,
            kind: CaptureKind::Region,
            purpose: Purpose::Translate,
            frames: 0,
        });
        ctx.request_repaint();
    }

    pub(super) fn run_pending(&mut self, ctx: &Context) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match pending {
            Pending::Overlay {
                mode,
                purpose,
                frames,
            } if frames > 0 => {
                self.pending = Some(Pending::Overlay {
                    mode,
                    purpose,
                    frames: frames - 1,
                });
                ctx.request_repaint();
            }
            Pending::Grab {
                bounds,
                kind,
                purpose,
                frames,
            } if frames > 0 => {
                self.pending = Some(Pending::Grab {
                    bounds,
                    kind,
                    purpose,
                    frames: frames - 1,
                });
                ctx.request_repaint();
            }
            Pending::Overlay { mode, purpose, .. } => self.open_overlay(ctx, mode, purpose),
            Pending::Grab {
                bounds,
                kind,
                purpose,
                ..
            } => match capture_frame(bounds) {
                Ok(shot) => self.start(ctx, shot, kind, purpose, ctx.pixels_per_point()),
                Err(e) => {
                    error!(error = %e, "Capture failed");
                    self.show_failure(e.user_message(), false, bounds, ctx.pixels_per_point());
                }
            },
        }
    }

    fn open_overlay(&mut self, ctx: &Context, mode: CaptureMode, purpose: Purpose) {
        let desktop = virtual_desktop();
        let geometry = Geometry::new(desktop, ctx.pixels_per_point());

        let frame = match capture_desktop() {
            Ok(frame) => Some(frame),
            Err(e) => {
                warn!(error = %e, "Could not capture the desktop for the overlay");
                None
            }
        };

        // "The whole screen" needs no selecting: it is the display the pointer
        // is on.
        if mode == CaptureMode::FullScreen && purpose == Purpose::Translate {
            let screen = cursor_position()
                .map(|(x, y)| monitor_for(Bounds { x, y, w: 1, h: 1 }).bounds)
                .unwrap_or(desktop);
            match frame.as_ref().and_then(|f| f.crop(screen)) {
                Some(shot) => self.start(
                    ctx,
                    shot,
                    CaptureKind::Whole,
                    purpose,
                    geometry.points_per_pixel,
                ),
                None => self.show_failure(
                    AppError::Other(
                        crate::shared::i18n::t("The captured image could not be processed").into(),
                    )
                    .user_message(),
                    false,
                    screen,
                    geometry.points_per_pixel,
                ),
            }
            return;
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
        debug!(count = windows.len(), "Window list captured for the overlay");

        self.overlay = Some(
            OverlayState::new(geometry, mode, purpose, self.settings.show_mode_hud)
                .with_frame(frame)
                .with_windows(windows),
        );
    }

    pub(super) fn show_overlay(&mut self, ctx: &Context) {
        let Some(mut state) = self.overlay.take() else {
            return;
        };
        let theme = self.theme;
        let desktop = state.geometry.desktop;
        let takes_keyboard = !super::stays_in_background();

        let builder = ViewportBuilder::default()
            // Never drawn — the window has no chrome — but it is how the
            // platform layer finds this window again.
            .with_title(overlay::TITLE)
            .with_position(state.geometry.window_pos_points())
            .with_inner_size(state.geometry.window_size_points())
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(takes_keyboard)
            // Created hidden where it can then be told to appear without the
            // window manager's animation: a full-screen copy of the desktop
            // zooming into place is the one thing a "freeze" must not do.
            .with_visible(!(native::QUIET_WINDOWS && state.frames == 0))
            .with_always_on_top();

        // On macOS it is created *inactive*: letting winit make it key straight
        // away would activate the application, and an application that has not
        // yet been told this window belongs on every Space gets pulled back to
        // its own — taking the user out of the full-screen app they were
        // reading. `present_overlay` orders it front and takes the keyboard.
        #[cfg(target_os = "macos")]
        let builder = builder.with_active(false);

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("sakura_capture"),
            builder,
            |ctx, _| {
                native::apply(
                    overlay::TITLE,
                    native::Behaviour {
                        instant: true,
                        ..Default::default()
                    },
                );
                native::place(overlay::TITLE, desktop);

                // Without focus the overlay never sees a key press, and Esc does
                // nothing however clearly the hint advertises it. macOS goes
                // through AppKit instead: winit's focus call activates the
                // application, which is exactly the Space-switch being avoided.
                #[cfg(not(target_os = "macos"))]
                if takes_keyboard && state.frames > 0 && !ctx.input(|i| i.focused) {
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }

                // Re-applied each frame: the window is created by the frame
                // before this one, and AppKit resets the frame if a display is
                // reconfigured mid-capture.
                #[cfg(target_os = "macos")]
                crate::features::capture::mac_window::present_overlay(overlay::TITLE);

                overlay::render(ctx, &theme, &mut state);
                if state.cancelled || state.completed.is_some() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            },
        );

        if state.cancelled {
            info!("Capture cancelled");
            return;
        }
        if let Some(bounds) = state.completed {
            let kind = match state.mode {
                CaptureMode::Region => CaptureKind::Region,
                CaptureMode::Window | CaptureMode::FullScreen => CaptureKind::Whole,
            };
            let ppp = state.geometry.points_per_pixel;
            match state.frame.as_ref().and_then(|f| f.crop(bounds)) {
                Some(shot) => self.start(ctx, shot, kind, state.purpose, ppp),
                // The desktop could not be frozen, so there is nothing to crop.
                // Grab the area live once the overlay is out of the way.
                None => {
                    self.pending = Some(Pending::Grab {
                        bounds,
                        kind,
                        purpose: state.purpose,
                        frames: SETTLE_FRAMES,
                    });
                    ctx.request_repaint();
                }
            }
            return;
        }
        self.overlay = Some(state);
    }

    // ── Running the pipeline ─────────────────────────────────────────────────

    /// Puts a capture on screen as "working" and sends it off.
    fn start(&mut self, ctx: &Context, shot: Frame, kind: CaptureKind, purpose: Purpose, ppp: f32) {
        if let Some(e) = too_small(shot.bounds) {
            self.show_failure(e.user_message(), false, shot.bounds, ppp);
            return;
        }

        self.last_capture = Some(LastCapture {
            bounds: shot.bounds,
            kind,
        });
        self.last_ppp = ppp;
        if self.settings.play_sound {
            super::beep();
        }

        self.result = Some(self.new_result(&shot, ppp));
        if purpose == Purpose::Watch {
            self.start_watching(&shot);
        }
        self.spawn_job(ctx, shot, kind, false);
    }

    fn new_result(&self, shot: &Frame, ppp: f32) -> ResultUi {
        let mut ui = ResultUi::new(
            self.settings.result_view,
            shot.bounds,
            monitor_for(shot.bounds),
            ppp,
        )
        .with_shot(Some(shot.clone()));
        ui.pinned = self.settings.pin_result_window;
        ui
    }

    fn show_failure(&mut self, message: String, empty: bool, bounds: Bounds, ppp: f32) {
        let mut ui = ResultUi::new(
            // A failure has no pixels to be painted over; the popup needs none.
            ResultView::Popup,
            bounds,
            monitor_for(bounds),
            ppp,
        );
        ui.set_stage(if empty {
            Stage::Empty(message)
        } else {
            Stage::Error(message)
        });
        self.result = Some(ui);
    }

    fn spawn_job(&mut self, ctx: &Context, shot: Frame, kind: CaptureKind, quiet: bool) {
        self.cancel_job();

        let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        let params = PipelineParams::from(&self.settings);
        let events = self.job_events.0.clone();
        let repaint = ctx.clone();
        let frame = shot.clone();

        let handle = self.rt.spawn(async move {
            let progress_events = events.clone();
            let progress_repaint = repaint.clone();
            let outcome = run_pipeline(frame, kind, &params, move |progress| {
                let _ = progress_events.send(JobEvent::Progress(id, progress));
                progress_repaint.request_repaint();
            })
            .await
            .map_err(|e| {
                // Full detail to the log, a sentence to the screen — and a
                // flag for whether this is a failure at all.
                let empty = matches!(e, AppError::NoText);
                if empty {
                    info!("Capture contained no text");
                } else {
                    error!(error = %e, "Pipeline failed");
                }
                Failure {
                    message: e.user_message(),
                    empty,
                }
            });

            let _ = events.send(JobEvent::Finished(id, outcome));
            // Without this the result would sit in the channel until some
            // other event happened to wake the UI thread.
            repaint.request_repaint();
        });

        self.job = Some(Job {
            id,
            handle,
            quiet,
            shot,
        });
    }

    pub(super) fn cancel_job(&mut self) {
        if let Some(job) = self.job.take() {
            job.handle.abort();
        }
    }

    /// True while a capture is waiting on recognition or a translation engine.
    pub(super) fn is_translating(&self) -> bool {
        self.job.is_some()
    }

    pub(super) fn collect_job_events(&mut self) {
        while let Ok(event) = self.job_events.1.try_recv() {
            let current = self.job.as_ref().map(|job| job.id);
            match event {
                JobEvent::Progress(id, Progress::Recognised(text)) if Some(id) == current => {
                    let quiet = self.job.as_ref().is_some_and(|job| job.quiet);
                    if !quiet {
                        if let Some(ui) = self.result.as_mut() {
                            ui.set_recognised(text);
                        }
                    }
                }
                JobEvent::Finished(id, outcome) if Some(id) == current => {
                    if let Some(job) = self.job.take() {
                        self.finish(job, outcome);
                    }
                }
                // From a job that has since been cancelled.
                _ => {}
            }
        }
    }

    fn finish(&mut self, job: Job, outcome: Result<PipelineResult, Failure>) {
        match outcome {
            Ok(res) => {
                info!(
                    engine = res.engine.label(),
                    total_ms = res.elapsed.as_millis() as u64,
                    engine_ms = res.engine_elapsed.as_millis() as u64,
                    cached = res.from_cache,
                    placed = res.segments.len(),
                    "Capture complete"
                );

                // A watched area read again and found unchanged: the picture
                // moved, the words did not, and what is on screen still stands.
                if let Some(watch) = self.watch.as_mut() {
                    if !watch.read(&res.original) && job.quiet && self.result.is_some() {
                        return;
                    }
                }

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

                match self.result.as_mut() {
                    Some(ui) => {
                        if job.quiet {
                            // The in-place view paints on the capture, so it
                            // has to be handed the new one along with the text.
                            ui.set_shot(job.shot);
                        }
                        ui.set_stage(Stage::Done(Box::new(res)));
                    }
                    // A watched area that had gone quiet and has text again.
                    None => {
                        let ppp = self.last_ppp;
                        let mut ui = self.new_result(&job.shot, ppp);
                        ui.set_stage(Stage::Done(Box::new(res)));
                        self.result = Some(ui);
                    }
                }
            }
            Err(failure) => {
                if job.quiet {
                    // Watching carries on through a failed look. An area with
                    // no text in it any more has simply stopped having
                    // something to translate, so the old answer comes down.
                    if failure.empty {
                        if let Some(watch) = self.watch.as_mut() {
                            watch.last_text = None;
                        }
                        self.result = None;
                    }
                    return;
                }

                // Worth showing either way, even in clipboard-only mode:
                // otherwise a capture that produced nothing is
                // indistinguishable from one that worked.
                let stage = if failure.empty {
                    Stage::Empty(failure.message)
                } else {
                    Stage::Error(failure.message)
                };
                match self.result.as_mut() {
                    Some(ui) => ui.set_stage(stage),
                    None => {
                        let mut ui = self.new_result(&job.shot, self.last_ppp);
                        ui.set_stage(stage);
                        self.result = Some(ui);
                    }
                }
            }
        }
    }

    // ── Watching an area ─────────────────────────────────────────────────────

    fn start_watching(&mut self, shot: &Frame) {
        info!(
            w = shot.bounds.w,
            h = shot.bounds.h,
            "Watching an area for changes"
        );
        self.watch = Some(Watch::new(shot));

        // The in-place view sits on the very pixels that have to be looked at
        // again. Where the platform cannot leave a window out of a capture,
        // every look would see the previous translation — so the result moves
        // to where it is not in the way.
        if !cfg!(windows) {
            if let Some(ui) = self.result.as_mut() {
                if ui.view == ResultView::Inline {
                    ui.set_view(ResultView::Subtitle);
                }
            }
        }
    }

    pub(super) fn stop_watching(&mut self) {
        if self.watch.take().is_some() {
            info!("Stopped watching");
        }
    }

    pub(super) fn is_watching(&self) -> bool {
        self.watch.is_some()
    }

    /// Starts or stops watching the area of the result on screen.
    pub(super) fn toggle_watch(&mut self) {
        if self.watch.is_some() {
            self.stop_watching();
            return;
        }
        let shot = self.result.as_ref().and_then(|ui| ui.shot().cloned());
        if let Some(shot) = shot {
            self.start_watching(&shot);
            // What is on screen now is the translation of this very picture.
            if let (Some(watch), Some(ui)) = (self.watch.as_mut(), self.result.as_ref()) {
                watch.last_text = ui.result().map(|r| r.original.clone());
            }
        }
    }

    /// Looks at the watched area again when it is time to, and has it read
    /// again when it has changed.
    pub(super) fn tick_watch(&mut self, ctx: &Context) {
        let interval = Duration::from_millis(self.settings.watch_interval_ms.clamp(300, 5000) as u64);
        let Some(watch) = self.watch.as_ref() else {
            return;
        };
        if self.overlay.is_some() || self.pending.is_some() {
            return;
        }
        let now = Instant::now();
        if now.duration_since(watch.last_look) < interval {
            return;
        }

        // A translation painted over the area hides the area. It can only be
        // looked at through a window the platform leaves out of captures.
        if let Some(ui) = &self.result {
            if ui.covers_its_anchor() && !native::hidden_from_capture(ui.title()) {
                if let Some(watch) = self.watch.as_mut() {
                    watch.last_look = now;
                }
                return;
            }
        }

        let bounds = watch.bounds;
        let frame = match capture_frame(bounds) {
            Ok(frame) => frame,
            Err(e) => {
                warn!(error = %e, "Could not look at the watched area");
                if let Some(watch) = self.watch.as_mut() {
                    watch.last_look = now;
                }
                return;
            }
        };

        let seen = frame.signature();
        let busy = self.job.is_some();
        let Some(watch) = self.watch.as_mut() else {
            return;
        };
        if watch.should_read(&seen, now) && !busy {
            watch.sent(seen, now);
            self.spawn_job(ctx, frame, CaptureKind::Region, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(fill: u8, text: bool) -> Frame {
        let (w, h) = (320, 80);
        let mut bgra = vec![fill; (w * h * 4) as usize];
        if text {
            for y in 20..60 {
                for x in 20..300 {
                    let i = ((y * w + x) * 4) as usize;
                    bgra[i..i + 3].copy_from_slice(&[240, 240, 240]);
                }
            }
        }
        Frame::from_bgra(Bounds { x: 0, y: 0, w, h }, bgra)
    }

    fn later(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn an_area_that_has_not_changed_is_left_alone() {
        let shot = frame(20, true);
        let mut watch = Watch::new(&shot);
        let start = Instant::now();
        assert!(!watch.should_read(&shot.signature(), later(start, 5000)));
    }

    #[test]
    fn new_text_is_read_once_it_has_stopped_changing() {
        let before = frame(20, false);
        let after = frame(20, true);
        let mut watch = Watch::new(&before);
        let start = Instant::now();

        // The first look that differs: changed, but not yet seen twice the
        // same — it might still be appearing.
        assert!(!watch.should_read(&after.signature(), later(start, 1500)));
        // The next look finds the same picture: settled.
        assert!(watch.should_read(&after.signature(), later(start, 2500)));
    }

    #[test]
    fn two_reads_are_never_closer_than_the_pause() {
        let before = frame(20, false);
        let after = frame(20, true);
        let mut watch = Watch::new(&before);
        let start = Instant::now();

        assert!(!watch.should_read(&after.signature(), later(start, 100)));
        // Settled, but only 200 ms after the watch began.
        assert!(!watch.should_read(&after.signature(), later(start, 200)));
        assert!(watch.should_read(&after.signature(), later(start, 1300)));
    }

    #[test]
    fn a_picture_that_never_settles_is_read_anyway_after_a_while() {
        let mut watch = Watch::new(&frame(20, false));
        let start = Instant::now();

        // Two pictures, both unlike the one that was translated and unlike
        // each other, taking turns: never the same twice in a row.
        let flicker = [frame(20, true).signature(), frame(120, true).signature()];
        let mut read_at = None;
        for step in 1..=20u64 {
            let seen = &flicker[(step % 2) as usize];
            if watch.should_read(seen, later(start, step * 500)) {
                read_at = Some(step * 500);
                break;
            }
        }
        let read_at = read_at.expect("an unsettled picture was never read");
        assert!(read_at >= 3000, "read after only {read_at} ms");
    }

    #[test]
    fn once_sent_the_same_picture_is_not_sent_again() {
        let before = frame(20, false);
        let after = frame(20, true);
        let mut watch = Watch::new(&before);
        let start = Instant::now();

        watch.should_read(&after.signature(), later(start, 1500));
        assert!(watch.should_read(&after.signature(), later(start, 2500)));
        watch.sent(after.signature(), later(start, 2500));

        assert!(!watch.should_read(&after.signature(), later(start, 9000)));
    }

    #[test]
    fn finding_the_same_words_again_makes_the_watch_look_less_often() {
        let mut watch = Watch::new(&frame(20, true));
        assert!(watch.read("Hello"), "the first read is always news");
        assert_eq!(watch.pause, SHORTEST_PAUSE);

        assert!(!watch.read("Hello"));
        assert!(watch.pause > SHORTEST_PAUSE);

        for _ in 0..20 {
            watch.read("Hello");
        }
        assert_eq!(watch.pause, LONGEST_PAUSE, "the back-off has a ceiling");
    }

    #[test]
    fn new_words_bring_the_watch_back_to_full_attention() {
        let mut watch = Watch::new(&frame(20, true));
        watch.read("Hello");
        watch.read("Hello");
        watch.read("Hello");
        assert!(watch.read("Goodbye"));
        assert_eq!(watch.pause, SHORTEST_PAUSE);
    }
}
