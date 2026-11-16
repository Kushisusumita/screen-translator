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
//!
//! Watching an area is the same loop run by a clock instead of a key press —
//! and it is two clocks, because looking and reading cost very differently.
//! *Looking* is a grab of the area and some arithmetic: it happens a few times
//! a second and is what takes a translation down the moment its text has gone.
//! *Reading* is a round trip to a recogniser and an engine: it happens only
//! once the picture has changed and come to rest, and less and less often
//! while it refuses to.
//!
//! Unless the reading can be done on the machine itself. Then it costs about
//! what a look does, and there is a third clock: the area is *read* every
//! fraction of a second, a scene in motion included, and only *translated* —
//! the one step that still goes out to a service — when what it says is new
//! and has stopped changing. That is what lets subtitles over a moving
//! picture be followed at all: such a picture never comes to rest, but the
//! words on it do.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use egui::{Context, ViewportBuilder, ViewportCommand, ViewportId};
use tracing::{debug, error, info, warn};

use super::result::{ResultUi, Stage};
use super::App;
use crate::entities::history::{History, HistoryEntry};
use crate::entities::settings::{CaptureMode, Recogniser, ResultView};
use crate::features::capture::frame::Signature;
use crate::features::capture::overlay::{self, Purpose};
use crate::features::capture::{
    capture_desktop, capture_frame, cursor_position, foreground_window_bounds, monitor_for,
    native, too_small, virtual_desktop, window_pick, Bounds, Frame, Geometry, OverlayState,
};
use crate::features::translation::ocr::local;
use crate::features::translation::{
    run_pipeline, Again, CaptureKind, Outcome, PipelineParams, Progress, Reading,
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
    /// The terms it was sent off on, when it is a reading made on this
    /// machine to see whether a watched area has changed.
    again: Option<Again>,
    /// A reading by a recogniser that has not yet read anything in this
    /// area. If it finds nothing, that is not yet news.
    untried: bool,
}

/// What a job reports back.
pub(super) enum JobEvent {
    Progress(u64, Progress),
    Finished(u64, Result<Outcome, Failure>),
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
    /// How the area was chosen, which decides how it is read: a rectangle
    /// around some text, or a whole window of many separate pieces.
    kind: CaptureKind,
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
    /// How many reads in a row were made of a picture that had not come to
    /// rest. Each one lengthens the wait before the next.
    restless: u32,
    /// The text of the last successful read.
    last_text: Option<String>,
    /// A translation painted over the area has had its words change under
    /// it. That is a change whatever the picture as a whole looks like: a
    /// line of dialogue replaced by another of the same length, in the same
    /// box, in the same type, moves the picture hardly at all.
    outlived: bool,
    /// What the area said the last time it was read, translated or not.
    heard: Option<String>,
    /// The recogniser on this machine has read text in this area. From then
    /// on it is taken at its word about the area — including when its word
    /// is that there is nothing there.
    read_here: bool,
    /// It found nothing where the online recogniser found text: lettering it
    /// was not made for. It is not asked on its own again.
    not_for_here: bool,
    /// What the area looked like when it was last read by any means. Kept
    /// apart from `translated`: a reading made here in passing must not
    /// make the online way think it has seen the picture.
    last_read: Signature,
    /// When the area was last read on this machine, and how long that took.
    last_here: Instant,
    cost_here: Duration,
    /// Since when readings made here have kept coming back with a text
    /// different from the one before.
    unsteady_since: Option<Instant>,
    /// There is something the next reading would settle — a translation has
    /// just lost its words, or a new text has been seen once and needs
    /// seeing again — so it is not made to wait its turn.
    hurry: bool,
    allowance: Allowance,
}

/// What there is to read a watched area with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Means {
    /// The online recogniser only.
    Online,
    /// The one on this machine, with the online one behind it.
    Either,
    /// The one on this machine and nothing else.
    Here,
}

/// A decision to read a watched area again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Read {
    reason: Reason,
    /// The online recogniser may be turned to. A reading that may not is
    /// one made on this machine alone, which is what makes it cheap.
    online: bool,
}

/// How many translations a watched area may still ask for.
///
/// A line with a clock in it is a new text every second, and every new text
/// is a request to a service that is someone else's. A few in quick
/// succession are let through — a conversation moves in bursts — and after
/// that they are let through at the rate a conversation keeps up.
#[derive(Debug, Clone, Copy)]
struct Allowance {
    left: f32,
    as_of: Instant,
}

impl Allowance {
    const MOST: f32 = 6.0;
    /// How long it takes for one more to be allowed.
    const EACH: Duration = Duration::from_secs(2);

    fn full(now: Instant) -> Self {
        Self {
            left: Self::MOST,
            as_of: now,
        }
    }

    fn top_up(&mut self, now: Instant) {
        let gained = now.duration_since(self.as_of).as_secs_f32() / Self::EACH.as_secs_f32();
        self.left = (self.left + gained).min(Self::MOST);
        self.as_of = now;
    }

    fn has(&mut self, now: Instant) -> bool {
        self.top_up(now);
        self.left >= 1.0
    }

    fn spend(&mut self, now: Instant) {
        self.top_up(now);
        self.left = (self.left - 1.0).max(0.0);
    }
}

/// How many cells of the picture have to look different before it counts
/// as a change. Fewer is a caret blinking or a spark drifting past; a word
/// coming up is at least this many.
const CHANGED: usize = 4;
/// Two looks no further apart than this are the same picture: text that has
/// finished appearing, as opposed to text still being typed out.
const SETTLED: usize = 2;
const SHORTEST_PAUSE: Duration = Duration::from_millis(1200);
const LONGEST_PAUSE: Duration = Duration::from_secs(12);
/// A picture that never settles is read anyway after this long.
const OVERDUE: Duration = Duration::from_secs(3);
/// How often a watched area is looked at when a translation is painted over
/// it. A look decides whether each plate still belongs where it is, so this
/// is how long a plate can outstay its text.
const LIVE_LOOK: Duration = Duration::from_millis(400);
/// How often it is looked at when it is read on this machine: every look
/// may then be a reading, and a reading is what notices a new line.
const QUICK_LOOK: Duration = Duration::from_millis(250);
/// The longest an area read on this machine is left between two readings,
/// however long each of them takes.
const SLOWEST_HERE: Duration = Duration::from_secs(1);
/// A text that will not hold still for this long is translated as it is.
const UNSTEADY_LONG_ENOUGH: Duration = Duration::from_millis(1500);

/// Why a watched area is being read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// It changed and has come to rest: new text, most likely.
    Settled,
    /// It has not stopped changing for a while: a scene in motion, with or
    /// without new text in it.
    Overdue,
    /// It is still changing, and reading it costs little enough to find out
    /// what it says meanwhile.
    Moving,
}

impl Watch {
    fn new(shot: &Frame, kind: CaptureKind) -> Self {
        let signature = shot.signature();
        let now = Instant::now();
        Self {
            bounds: shot.bounds,
            kind,
            translated: signature.clone(),
            last_read: signature.clone(),
            last_seen: signature,
            last_look: now,
            changed_since: None,
            last_job: now,
            pause: SHORTEST_PAUSE,
            restless: 0,
            last_text: None,
            outlived: false,
            heard: None,
            read_here: false,
            not_for_here: false,
            last_here: now,
            cost_here: Duration::ZERO,
            unsteady_since: None,
            hurry: false,
            allowance: Allowance::full(now),
        }
    }

    /// Notes, at each look, whether a translation on screen has outlived the
    /// words it was put up for.
    fn translation_outlived(&mut self, outlived: bool) {
        // The moment it happens is the moment to find out what is there now.
        self.hurry |= outlived && !self.outlived;
        self.outlived = outlived;
    }

    /// A read that came back with nothing to show for it — the service was
    /// out of reach, or refused. Asking again at once would only be refused
    /// again, so the wait before the next read grows as it does for a picture
    /// whose words have not changed.
    fn failed(&mut self) {
        self.pause = (self.pause * 2).min(LONGEST_PAUSE);
        // And as long as there is between two readings made here.
        self.cost_here = self.cost_here.max(SLOWEST_HERE / 8);
    }

    /// Whether the area is read on this machine alone, which is what lets
    /// it be read often.
    fn reads_here(&self, means: Means) -> bool {
        match means {
            Means::Online => false,
            Means::Here => true,
            Means::Either => self.read_here && !self.not_for_here,
        }
    }

    /// The least time between two readings made on this machine: several
    /// times what one takes, so that reading stays a small part of what the
    /// machine is doing while a game runs on it.
    fn gap_here(&self) -> Duration {
        if self.hurry {
            return QUICK_LOOK;
        }
        (self.cost_here * 8).clamp(QUICK_LOOK, SLOWEST_HERE)
    }

    /// Takes in what a reading turned out to be made with.
    fn learnt(&mut self, reading: Reading) {
        if reading.here {
            self.cost_here = reading.elapsed;
            if reading.found {
                self.read_here = true;
                self.not_for_here = false;
            }
        } else if reading.found && !self.read_here {
            self.not_for_here = true;
        }
    }

    /// The terms a reading made on this machine alone is sent off on.
    fn terms(&mut self, read: Read, now: Instant) -> Option<Again> {
        if read.online {
            return None;
        }
        let waited_enough = self
            .unsteady_since
            .is_some_and(|since| now.duration_since(since) >= UNSTEADY_LONG_ENOUGH);
        Some(Again {
            // A translation that has come down is to be put up again from
            // this picture, same words or not.
            shown: if self.outlived {
                None
            } else {
                self.last_text.clone()
            },
            heard: self.heard.clone(),
            settled: read.reason == Reason::Settled || waited_enough,
            may_translate: self.allowance.has(now),
            online: false,
        })
    }

    /// A reading came back with a text that was not worth translating yet.
    fn held(&mut self, text: String, now: Instant) {
        if self.last_text.as_deref() == Some(text.as_str()) {
            self.unsteady_since = None;
        } else if self.heard.as_deref() != Some(text.as_str()) {
            // A text not seen before. The first of them is worth looking at
            // again straight away, to see whether it has come to stay; the
            // ones after it are a text on the move, and can wait their turn.
            self.hurry |= self.unsteady_since.is_none();
            self.unsteady_since.get_or_insert(now);
        }
        self.heard = Some(text);
    }

    /// The least time to leave after the last read before the next.
    ///
    /// Two things lengthen it. Finding the same words again does — see
    /// [`Self::read`]. And so does a picture that will not come to rest: a
    /// scene in motion, read every few seconds for as long as it moves, would
    /// be a round trip to the recogniser twenty times a minute, mostly to be
    /// told what it said last time. Each read of a restless picture doubles
    /// the wait; the first read of one at rest puts it back.
    fn wait(&self) -> Duration {
        let restless = match self.restless {
            0 => Duration::ZERO,
            n => (OVERDUE * 2u32.pow(n.min(4) - 1)).min(LONGEST_PAUSE),
        };
        self.pause.max(restless)
    }

    /// Takes in a fresh look at the area and says whether it should be read
    /// again, and why.
    ///
    /// Two things have to be true: the picture differs from the one that was
    /// translated, and it has stopped changing — a dialogue line that is still
    /// being typed out letter by letter is not worth reading yet. A picture
    /// that never settles (an animated backdrop) is read anyway once it has
    /// been different for long enough.
    #[cfg(test)]
    fn should_read(&mut self, seen: &Signature, now: Instant) -> Option<Reason> {
        self.look(seen, now, Means::Online).map(|read| read.reason)
    }

    /// The same, with what there is to read the area with taken into
    /// account.
    ///
    /// Read online, the rule above is the whole of it. Read on this machine,
    /// the area need not have come to rest: it is read as soon as it differs
    /// and a little time has passed since the last reading. And while the
    /// recogniser here has not yet shown that it can read this area, both
    /// go on side by side — it is tried as often as it likes, and the
    /// online one is turned to no more often than it ever was.
    fn look(&mut self, seen: &Signature, now: Instant, means: Means) -> Option<Read> {
        let from_translated = seen.cells_changed(&self.translated);
        let from_read = seen.cells_changed(&self.last_read);
        let from_previous = seen.cells_changed(&self.last_seen);
        self.last_seen = seen.clone();
        self.last_look = now;
        let settled = from_previous <= SETTLED;

        // The dear way: once the picture differs from the one that was sent
        // off, and has come to rest or has refused to for long enough.
        let dear = if from_translated < CHANGED && !self.outlived {
            self.changed_since = None;
            None
        } else {
            let since = *self.changed_since.get_or_insert(now);
            let reason = if now.duration_since(self.last_job) < self.wait() {
                None
            } else if settled {
                Some(Reason::Settled)
            } else if now.duration_since(since) >= OVERDUE {
                Some(Reason::Overdue)
            } else {
                None
            };
            reason.map(|reason| Read {
                reason,
                online: true,
            })
        };

        // The cheap way: as soon as the picture differs from the one that
        // was last read, at rest or not.
        let differs = from_read >= CHANGED || self.outlived;
        let cheap = (differs && now.duration_since(self.last_here) >= self.gap_here()).then_some(
            Read {
                reason: if settled {
                    Reason::Settled
                } else {
                    Reason::Moving
                },
                online: false,
            },
        );

        match means {
            Means::Online => dear,
            Means::Either if self.not_for_here => dear,
            _ if self.reads_here(means) => cheap,
            _ => dear.or(cheap),
        }
    }

    #[cfg(test)]
    fn sent(&mut self, seen: Signature, now: Instant, reason: Reason) {
        self.began(
            seen,
            now,
            Read {
                reason,
                online: true,
            },
        );
    }

    /// Notes that the area has been sent off to be read.
    fn began(&mut self, seen: Signature, now: Instant, read: Read) {
        self.last_here = now;
        self.hurry = false;
        if read.online {
            self.translated = seen.clone();
            self.changed_since = None;
            self.last_job = now;
            self.restless = match read.reason {
                Reason::Settled => 0,
                Reason::Overdue => self.restless.saturating_add(1),
                Reason::Moving => self.restless,
            };
        }
        self.last_read = seen;
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

/// Takes the picture of an area and notes how long that took: the one step
/// of a capture that is neither the recogniser's nor the translator's.
fn timed_capture(bounds: Bounds) -> Result<Frame, AppError> {
    let asked = Instant::now();
    let frame = capture_frame(bounds)?;
    debug!(
        us = asked.elapsed().as_micros() as u64,
        w = bounds.w,
        h = bounds.h,
        "Area captured"
    );
    Ok(frame)
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

    /// Takes a rectangle that is already known, for a translation or for a
    /// watch, without anyone selecting it — the scripted route. No overlay
    /// goes up, so nothing but the rectangle itself is ever touched: whoever
    /// is at the machine keeps the rest of their screen.
    #[cfg(debug_assertions)]
    pub(super) fn capture_known(
        &mut self,
        ctx: &Context,
        bounds: Bounds,
        kind: CaptureKind,
        purpose: Purpose,
    ) {
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
            bounds,
            kind,
            purpose,
            frames,
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
            } => match timed_capture(bounds) {
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
                native::reveal(ctx, state.frames);

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
            self.start_watching(&shot, kind);
        }
        self.spawn_job(ctx, shot, kind, false, None, false);
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

    fn spawn_job(
        &mut self,
        ctx: &Context,
        shot: Frame,
        kind: CaptureKind,
        quiet: bool,
        again: Option<Again>,
        untried: bool,
    ) {
        self.cancel_job();

        let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        let params = PipelineParams::from(&self.settings);
        let events = self.job_events.0.clone();
        let repaint = ctx.clone();
        let frame = shot.clone();
        let terms = again.clone();

        let handle = self.rt.spawn(async move {
            let progress_events = events.clone();
            let progress_repaint = repaint.clone();
            let outcome = run_pipeline(frame, kind, &params, terms.as_ref(), move |progress| {
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
            again,
            untried,
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
                JobEvent::Progress(id, Progress::Read(reading)) if Some(id) == current => {
                    if let Some(watch) = self.watch.as_mut() {
                        watch.learnt(reading);
                    }
                }
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

    fn finish(&mut self, job: Job, outcome: Result<Outcome, Failure>) {
        let now = Instant::now();
        match outcome {
            // Read, and nothing in it to translate yet.
            Ok(Outcome::Held(text)) => {
                if let Some(watch) = self.watch.as_mut() {
                    watch.held(text, now);
                }
            }
            Ok(Outcome::Translated(res)) => {
                let res = *res;
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
                //
                // Unless a translation has come down in the meantime. Then
                // what is on screen does not stand: the words read the same,
                // but something about them — their colour, what is behind
                // them — is no longer what the plates were made for, and they
                // are made again from this picture.
                let outlived = self.result.as_ref().is_some_and(|ui| ui.outlived());
                if let Some(watch) = self.watch.as_mut() {
                    watch.heard = Some(res.original.clone());
                    watch.unsteady_since = None;
                    if !res.from_cache && !res.engine_elapsed.is_zero() {
                        watch.allowance.spend(now);
                    }
                    let news = watch.read(&res.original);
                    if !news && !outlived && job.quiet && self.result.is_some() {
                        return;
                    }
                    // The same words, read here a moment ago, under plates
                    // that had come down: text that moves and has not
                    // changed. Its translation goes back up to stay.
                    if !news && outlived && job.again.is_some() {
                        if let Some(ui) = self.result.as_mut() {
                            let restless = ui.outlived_texts();
                            debug!(texts = restless.len(), "Text that moves is kept translated");
                            ui.keep_up(restless);
                        }
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
                        // Unless it was a recogniser that has yet to read
                        // anything here saying so: that it finds nothing is
                        // as likely to be about it as about the area.
                        if job.untried {
                            return;
                        }
                        if let Some(watch) = self.watch.as_mut() {
                            // Over a picture in motion, nothing has to be
                            // found twice running, as a new text has to be:
                            // one reading in which the letters were lost
                            // against what passed behind them is not the
                            // end of a line of dialogue.
                            let moving = job.again.as_ref().is_some_and(|again| !again.settled);
                            let had_text = watch.heard.take().is_some();
                            watch.unsteady_since = None;
                            if moving && had_text {
                                return;
                            }
                            watch.last_text = None;
                        }
                        match self.result.as_mut() {
                            // Painted over the area itself, the result is a
                            // sheet of glass with nothing on it now — which
                            // is right, and it stays up so that its mark
                            // still says the area is being watched.
                            Some(ui) if ui.covers_its_anchor() => {
                                ui.set_stage(Stage::Empty(failure.message));
                            }
                            _ => self.result = None,
                        }
                    } else if let Some(watch) = self.watch.as_mut() {
                        watch.failed();
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

    fn start_watching(&mut self, shot: &Frame, kind: CaptureKind) {
        info!(
            w = shot.bounds.w,
            h = shot.bounds.h,
            kind = ?kind,
            "Watching an area for changes"
        );
        self.watch = Some(Watch::new(shot, kind));

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
            // Read again the way it was read the first time.
            let kind = self
                .last_capture
                .filter(|last| last.bounds == shot.bounds)
                .map_or(CaptureKind::Region, |last| last.kind);
            self.start_watching(&shot, kind);
            // What is on screen now is the translation of this very picture.
            if let (Some(watch), Some(ui)) = (self.watch.as_mut(), self.result.as_ref()) {
                watch.last_text = ui.result().map(|r| r.original.clone());
                watch.heard = watch.last_text.clone();
            }
        }
    }

    /// Looks at the watched area again when it is time to, and has it read
    /// again when it has changed.
    pub(super) fn tick_watch(&mut self, ctx: &Context) {
        let Some(watch) = self.watch.as_ref() else {
            return;
        };
        if self.overlay.is_some() || self.pending.is_some() {
            return;
        }
        let now = Instant::now();
        if now.duration_since(watch.last_look) < self.look_interval() {
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
        let looking = Instant::now();
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

        // First what costs nothing: is each translation on screen still
        // over the text it translates?
        let mut outlived = false;
        if let Some(ui) = self.result.as_mut() {
            if ui.covers_its_anchor() {
                if ui.look_again(&frame) {
                    ctx.request_repaint();
                }
                outlived = ui.outlived();
            }
        }

        let seen = frame.signature();
        let busy = self.job.is_some();
        let means = self.means();
        let Some(watch) = self.watch.as_mut() else {
            return;
        };
        watch.translation_outlived(outlived);
        let read = watch.look(&seen, now, means);
        tracing::trace!(
            us = looking.elapsed().as_micros() as u64,
            w = bounds.w,
            h = bounds.h,
            "Looked at the watched area"
        );
        if let (Some(read), false) = (read, busy) {
            if read.online {
                debug!(reason = ?read.reason, "Reading the watched area again");
            } else {
                tracing::trace!(reason = ?read.reason, "Reading the watched area again, here");
            }
            let again = watch.terms(read, now);
            let untried = !read.online && !watch.reads_here(means);
            watch.began(seen, now, read);
            let kind = watch.kind;
            self.spawn_job(ctx, frame, kind, true, again, untried);
        }
    }

    /// What there is to read the watched area with, as things are set now.
    fn means(&self) -> Means {
        let Some(watch) = self.watch.as_ref() else {
            return Means::Online;
        };
        let settings = &self.settings;
        let here = local::language_for(settings.source_lang, settings.target_lang).is_some();
        match settings.recogniser {
            Recogniser::Online => Means::Online,
            Recogniser::Local if here => Means::Here,
            // Nothing to read it with: every attempt will say so, and they
            // are spaced out the way failures are.
            Recogniser::Local => Means::Online,
            // A picture that is shown to a model is not read at all.
            Recogniser::Auto
                if watch.kind == CaptureKind::Region && settings.engines.reads_screenshots() =>
            {
                Means::Online
            }
            Recogniser::Auto if here => Means::Either,
            Recogniser::Auto => Means::Online,
        }
    }

    /// How long to leave between two looks at a watched area.
    ///
    /// The setting, unless a translation is painted over the area: then each
    /// look is also what takes a plate down once its text has gone, and a
    /// second is a long time for the wrong words to sit on a line of dialogue.
    pub(super) fn look_interval(&self) -> Duration {
        let chosen = Duration::from_millis(self.settings.watch_interval_ms.clamp(300, 5000) as u64);
        let painted_over = self
            .result
            .as_ref()
            .is_some_and(|ui| ui.covers_its_anchor());
        // Read on this machine, a look is also what decides that it is time
        // to read: the sooner the look, the sooner a new line is noticed.
        let means = self.means();
        let read_here = self.watch.as_ref().is_some_and(|watch| watch.reads_here(means));
        if read_here {
            chosen.min(QUICK_LOOK)
        } else if painted_over {
            chosen.min(LIVE_LOOK)
        } else {
            chosen
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
        let mut watch = Watch::new(&shot, CaptureKind::Region);
        let start = Instant::now();
        assert_eq!(watch.should_read(&shot.signature(), later(start, 5000)), None);
    }

    #[test]
    fn new_text_is_read_once_it_has_stopped_changing() {
        let before = frame(20, false);
        let after = frame(20, true);
        let mut watch = Watch::new(&before, CaptureKind::Region);
        let start = Instant::now();

        // The first look that differs: changed, but not yet seen twice the
        // same — it might still be appearing.
        assert_eq!(watch.should_read(&after.signature(), later(start, 1500)), None);
        // The next look finds the same picture: settled.
        assert_eq!(
            watch.should_read(&after.signature(), later(start, 2500)),
            Some(Reason::Settled)
        );
    }

    #[test]
    fn two_reads_are_never_closer_than_the_pause() {
        let before = frame(20, false);
        let after = frame(20, true);
        let mut watch = Watch::new(&before, CaptureKind::Region);
        let start = Instant::now();

        assert_eq!(watch.should_read(&after.signature(), later(start, 100)), None);
        // Settled, but only 200 ms after the watch began.
        assert_eq!(watch.should_read(&after.signature(), later(start, 200)), None);
        assert!(watch.should_read(&after.signature(), later(start, 1300)).is_some());
    }

    #[test]
    fn a_picture_that_never_settles_is_read_anyway_after_a_while() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Region);
        let start = Instant::now();

        // Two pictures, both unlike the one that was translated and unlike
        // each other, taking turns: never the same twice in a row.
        let flicker = [frame(20, true).signature(), frame(120, true).signature()];
        let mut read_at = None;
        for step in 1..=20u64 {
            let seen = &flicker[(step % 2) as usize];
            if let Some(reason) = watch.should_read(seen, later(start, step * 500)) {
                assert_eq!(reason, Reason::Overdue);
                read_at = Some(step * 500);
                break;
            }
        }
        let read_at = read_at.expect("an unsettled picture was never read");
        assert!(read_at >= 3000, "read after only {read_at} ms");
    }

    /// A scene in motion, as it looks at one moment: no two moments alike,
    /// which is what tells motion from a picture that flips between states.
    fn moving(step: u64) -> Signature {
        let (w, h) = (320usize, 80usize);
        let mut bgra = vec![20u8; w * h * 4];
        // Something bright crossing the picture, somewhere new at every look.
        let left = (step * 53 % 280) as usize;
        for y in 0..h {
            for x in left..left + 40 {
                let i = (y * w + x) * 4;
                bgra[i..i + 3].copy_from_slice(&[240, 240, 240]);
            }
        }
        let bounds = Bounds {
            x: 0,
            y: 0,
            w: w as i32,
            h: h as i32,
        };
        Frame::from_bgra(bounds, bgra).signature()
    }

    /// Runs a watch over a picture that changes at every look, for a minute,
    /// and returns when it was read.
    fn reads_of_a_restless_scene() -> Vec<u64> {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Whole);
        let start = Instant::now();
        let mut reads = Vec::new();
        for step in 1..=150u64 {
            let at = later(start, step * 400);
            let seen = moving(step);
            if let Some(reason) = watch.should_read(&seen, at) {
                watch.sent(seen, at, reason);
                reads.push(step * 400);
            }
        }
        reads
    }

    #[test]
    fn words_that_changed_under_a_translation_are_news_whatever_the_picture_says() {
        // One line of dialogue replaced by another of the same length: to
        // the coarse picture the area is the same as it was.
        let shot = frame(20, true);
        let mut watch = Watch::new(&shot, CaptureKind::Whole);
        let start = watch.last_look;
        let same = shot.signature();
        assert_eq!(watch.should_read(&same, later(start, 1500)), None);

        watch.translation_outlived(true);
        assert_eq!(
            watch.should_read(&same, later(start, 1900)),
            Some(Reason::Settled)
        );
        watch.sent(same.clone(), later(start, 1900), Reason::Settled);

        // The new translation is up and nothing has outlived anything.
        watch.translation_outlived(false);
        assert_eq!(watch.should_read(&same, later(start, 9000)), None);
    }

    const HERE: Read = Read {
        reason: Reason::Moving,
        online: false,
    };

    /// What a reading made on this machine reports back.
    fn reading(found: bool, ms: u64) -> Reading {
        Reading {
            here: true,
            found,
            elapsed: Duration::from_millis(ms),
        }
    }

    #[test]
    fn an_area_read_on_this_machine_is_read_again_while_it_is_still_moving() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Region);
        let start = watch.last_look;
        watch.learnt(reading(true, 10));

        // Subtitles over a scene: no two looks alike, and no waiting for
        // them to be.
        let mut reads = 0;
        for step in 1..=20u64 {
            let now = later(start, step * 250);
            let seen = moving(step);
            if let Some(read) = watch.look(&seen, now, Means::Either) {
                assert_eq!(read, HERE);
                watch.began(seen, now, read);
                reads += 1;
            }
        }
        // Five seconds of it, a quarter of a second apart.
        assert!(reads >= 18, "read only {reads} times in five seconds");
    }

    #[test]
    fn an_area_that_is_slow_to_read_is_read_less_often() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Whole);
        let start = watch.last_look;
        // A whole display: a tenth of a second a reading.
        watch.learnt(reading(true, 100));

        let mut reads = 0;
        for step in 1..=20u64 {
            let now = later(start, step * 250);
            let seen = moving(step);
            if let Some(read) = watch.look(&seen, now, Means::Either) {
                watch.began(seen, now, read);
                reads += 1;
            }
        }
        assert!((4..=7).contains(&reads), "read {reads} times in five seconds");
    }

    #[test]
    fn a_new_text_is_looked_at_again_without_waiting_its_turn() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Whole);
        let start = watch.last_look;
        // A whole display: a second between two readings as a rule.
        watch.learnt(reading(true, 150));
        watch.last_text = Some("The pass is closed.".into());
        assert_eq!(watch.look(&moving(1), later(start, 500), Means::Either), None);
        let seen = moving(2);
        let read = watch.look(&seen, later(start, 1000), Means::Either).expect("due");
        watch.began(seen, later(start, 1000), read);

        // It comes back with words nobody has seen: the next look reads.
        watch.held("You will have to wait.".into(), later(start, 1150));
        let seen = moving(3);
        let read = watch.look(&seen, later(start, 1250), Means::Either).expect("at once");
        watch.began(seen, later(start, 1250), read);

        // Still changing: a line being typed out. That is not hurried.
        watch.held("You will have to wait here until".into(), later(start, 1400));
        assert_eq!(watch.look(&moving(4), later(start, 1500), Means::Either), None);
    }

    #[test]
    fn a_translation_that_loses_its_words_is_a_reason_to_read_at_once() {
        let shot = frame(20, true);
        let mut watch = Watch::new(&shot, CaptureKind::Whole);
        let start = watch.last_look;
        watch.learnt(reading(true, 150));
        let same = shot.signature();
        watch.began(same.clone(), later(start, 1000), HERE);

        // The picture as a whole has not moved; the words under a plate have.
        watch.translation_outlived(true);
        assert!(watch.look(&same, later(start, 1250), Means::Either).is_some());
        watch.began(same.clone(), later(start, 1250), HERE);
        // And it stays that way without being a reason every time.
        watch.translation_outlived(true);
        assert_eq!(watch.look(&same, later(start, 1500), Means::Either), None);
    }

    #[test]
    fn a_picture_that_has_not_changed_is_not_read_again_however_cheap_it_is() {
        let shot = frame(20, true);
        let mut watch = Watch::new(&shot, CaptureKind::Region);
        let start = watch.last_look;
        watch.learnt(reading(true, 10));
        for step in 1..=8u64 {
            assert_eq!(watch.look(&shot.signature(), later(start, step * 250), Means::Here), None);
        }
    }

    #[test]
    fn a_recogniser_that_has_yet_to_read_the_area_does_not_hurry_the_online_one() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Region);
        let start = watch.last_look;

        let mut online = Vec::new();
        let mut here = 0;
        for step in 1..=40u64 {
            let now = later(start, step * 250);
            let seen = moving(step);
            if let Some(read) = watch.look(&seen, now, Means::Either) {
                if read.online {
                    online.push(step * 250);
                } else {
                    here += 1;
                    // It looked, and found nothing it could read.
                    watch.learnt(reading(false, 10));
                }
                watch.began(seen, now, read);
            }
        }
        // It was tried all along…
        assert!(here >= 20, "tried here only {here} times");
        // …and the online one was asked when it would have been anyway: not
        // before a picture in motion is overdue, and then at growing gaps.
        assert!(online[0] >= 3000, "asked online after {} ms", online[0]);
        assert!(online.len() <= 3, "asked online {} times in ten seconds", online.len());
    }

    #[test]
    fn lettering_the_recogniser_here_cannot_read_is_left_to_the_online_one() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Region);
        assert!(!watch.reads_here(Means::Either));
        // Nothing found here, and then text found online, in the same job.
        watch.learnt(reading(false, 10));
        watch.learnt(Reading {
            here: false,
            found: true,
            elapsed: Duration::from_millis(700),
        });
        assert!(watch.not_for_here);
        let start = watch.last_look;
        // No more tries here in between: only what the online way allows.
        assert_eq!(watch.look(&moving(1), later(start, 300), Means::Either), None);

        // Until it does read something, on a reading made for the other.
        watch.learnt(reading(true, 12));
        assert!(watch.reads_here(Means::Either));
    }

    #[test]
    fn told_to_read_here_only_it_is_trusted_from_the_start() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Region);
        let start = watch.last_look;
        assert!(watch.reads_here(Means::Here));
        assert_eq!(watch.look(&moving(1), later(start, 300), Means::Here), Some(HERE));
        // And with nothing to read here with at all, there is only online.
        assert!(!watch.reads_here(Means::Online));
    }

    #[test]
    fn a_text_is_translated_once_it_holds_still() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Region);
        let start = watch.last_look;
        watch.last_text = Some("The pass is closed.".into());

        // Over a picture in motion, what was read has to be read again.
        let terms = watch.terms(HERE, later(start, 300)).expect("a reading made here");
        assert!(!terms.settled && !terms.online && terms.may_translate);
        assert_eq!(terms.shown.as_deref(), Some("The pass is closed."));
        assert_eq!(terms.heard, None);

        // A line being typed out: a different text at every reading.
        watch.held("You w".into(), later(start, 300));
        watch.held("You will ha".into(), later(start, 550));
        let terms = watch.terms(HERE, later(start, 800)).unwrap();
        assert!(!terms.settled);
        assert_eq!(terms.heard.as_deref(), Some("You will ha"));

        // A picture at rest needs no second reading.
        let at_rest = Read {
            reason: Reason::Settled,
            online: false,
        };
        assert!(watch.terms(at_rest, later(start, 800)).unwrap().settled);

        // And a text that never holds still is taken as it is in the end.
        assert!(watch.terms(HERE, later(start, 2000)).unwrap().settled);
    }

    #[test]
    fn a_reading_that_may_go_online_is_sent_on_no_terms() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Region);
        let start = watch.last_look;
        let read = Read {
            reason: Reason::Settled,
            online: true,
        };
        assert!(watch.terms(read, later(start, 1500)).is_none());
    }

    #[test]
    fn a_translation_that_has_come_down_is_put_up_again_whatever_the_words() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Region);
        let start = watch.last_look;
        watch.last_text = Some("Same words.".into());
        watch.translation_outlived(true);
        assert_eq!(watch.terms(HERE, later(start, 300)).unwrap().shown, None);
    }

    #[test]
    fn translations_are_let_through_in_a_burst_and_then_at_a_steady_rate() {
        let start = Instant::now();
        let mut allowance = Allowance::full(start);
        // A clock in a line of text: a new text every second.
        let mut spent = 0;
        for second in 0..60u64 {
            let now = later(start, second * 1000);
            if allowance.has(now) {
                allowance.spend(now);
                spent += 1;
            }
        }
        // Six at once, and one every two seconds after that.
        assert!((30..=36).contains(&spent), "{spent} translations in a minute");
        // Left alone, it fills up again — and no further than it began.
        let mut later_on = allowance;
        assert!(later_on.has(later(start, 120_000)));
        assert!(later_on.left <= Allowance::MOST);
    }

    #[test]
    fn a_reading_that_failed_here_is_not_tried_again_at_once() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Region);
        let start = watch.last_look;
        watch.learnt(reading(true, 10));
        watch.began(moving(1), later(start, 300), HERE);
        watch.failed();
        assert_eq!(watch.look(&moving(2), later(start, 600), Means::Either), None);
        assert_eq!(watch.look(&moving(3), later(start, 1400), Means::Either), Some(HERE));
    }

    #[test]
    fn a_read_that_failed_is_not_asked_for_again_at_once() {
        let shot = frame(20, true);
        let mut watch = Watch::new(&shot, CaptureKind::Region);
        let start = watch.last_look;
        let same = shot.signature();
        watch.translation_outlived(true);
        let reason = watch
            .should_read(&same, later(start, 1300))
            .expect("due the first time");
        watch.sent(same.clone(), later(start, 1300), reason);
        watch.failed();

        // Still outlived, since nothing came back to replace it — but the
        // shortest pause no longer applies.
        assert_eq!(watch.should_read(&same, later(start, 2600)), None);
        assert!(watch.should_read(&same, later(start, 3800)).is_some());
    }

    #[test]
    fn a_scene_that_never_rests_is_read_less_and_less_often() {
        let reads = reads_of_a_restless_scene();
        assert!(reads.len() >= 3, "{reads:?}");
        // A minute of nothing but motion: a handful of reads, not twenty.
        assert!(reads.len() <= 8, "read {} times in a minute: {reads:?}", reads.len());
        let gaps: Vec<u64> = reads.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(gaps.windows(2).all(|pair| pair[1] >= pair[0]), "{gaps:?}");
        assert!(*gaps.last().unwrap() >= 10_000, "{gaps:?}");
        assert!(gaps.iter().all(|gap| *gap <= 15_000), "the wait has a ceiling: {gaps:?}");
    }

    #[test]
    fn a_picture_that_comes_to_rest_ends_the_long_waits() {
        let mut watch = Watch::new(&frame(20, false), CaptureKind::Whole);
        let start = Instant::now();

        // Restless long enough to be read twice.
        let mut overdue = 0;
        let mut steps = 0;
        for step in 1..=100u64 {
            steps = step;
            let seen = moving(step);
            if let Some(reason) = watch.should_read(&seen, later(start, step * 400)) {
                assert_eq!(reason, Reason::Overdue);
                watch.sent(seen, later(start, step * 400), reason);
                overdue += 1;
                if overdue == 2 {
                    break;
                }
            }
        }
        assert_eq!(overdue, 2, "a scene in motion was not read twice in 40 s");
        assert!(watch.wait() >= Duration::from_secs(6), "{:?}", watch.wait());

        // Then a line of dialogue appears and stays put.
        let line = frame(250, false).signature();
        let mut read = None;
        for step in 1..=60u64 {
            let now = later(start, (steps + step) * 400);
            if let Some(reason) = watch.should_read(&line, now) {
                watch.sent(line.clone(), now, reason);
                read = Some(reason);
                break;
            }
        }
        assert_eq!(read, Some(Reason::Settled));
        assert_eq!(watch.wait(), SHORTEST_PAUSE);
    }

    #[test]
    fn once_sent_the_same_picture_is_not_sent_again() {
        let before = frame(20, false);
        let after = frame(20, true);
        let mut watch = Watch::new(&before, CaptureKind::Region);
        let start = Instant::now();

        watch.should_read(&after.signature(), later(start, 1500));
        let reason = watch
            .should_read(&after.signature(), later(start, 2500))
            .expect("settled by the second look");
        watch.sent(after.signature(), later(start, 2500), reason);

        assert_eq!(watch.should_read(&after.signature(), later(start, 9000)), None);
    }

    #[test]
    fn finding_the_same_words_again_makes_the_watch_look_less_often() {
        let mut watch = Watch::new(&frame(20, true), CaptureKind::Region);
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
        let mut watch = Watch::new(&frame(20, true), CaptureKind::Region);
        watch.read("Hello");
        watch.read("Hello");
        watch.read("Hello");
        assert!(watch.read("Goodbye"));
        assert_eq!(watch.pause, SHORTEST_PAUSE);
    }
}
