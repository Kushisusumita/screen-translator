//! How a translation is put on screen.
//!
//! Four views share one state object, so switching between them mid-result —
//! which their own controls do — keeps the text, the timing and the engine:
//!
//! * **in place** ([`inplace`]) — the translation painted over the original,
//!   paragraph by paragraph, in the size and colours the original had;
//! * **subtitle** ([`subtitle`]) — a line at the edge of the screen;
//! * **popup** ([`popup`]) — a card next to the captured area, with the
//!   original above the translation;
//! * **window** ([`window`]) — a free-floating two-column window.
//!
//! The first three *float*: they never take the keyboard focus, so a game under
//! them keeps running and keeps receiving input. That is also why none of them
//! can rely on focus to know when to go away, and why dismissal lives here in
//! one place — a timer, a click somewhere else, or Esc — rather than being
//! reinvented per view.

mod chrome;
pub mod fit;
mod inplace;
mod popup;
mod subtitle;
mod window;

use std::time::{Duration, Instant};

use egui::{Pos2, Rect, TextureHandle, Vec2};

use crate::entities::settings::{Presentation, ResultView};
use crate::features::capture::{native, Bounds, Frame, Monitor};
use crate::features::translation::PipelineResult;
use crate::shared::i18n::t;
use crate::ui::Theme;

use chrome::HeaderInfo;

#[derive(Debug)]
pub enum Stage {
    Loading {
        since: Instant,
        /// The text, once it has been read and while it is being translated.
        recognised: Option<String>,
    },
    Done(Box<PipelineResult>),
    Error(String),
    /// Nothing went wrong; there was simply nothing to translate. Shown in
    /// the ordinary text colour, because red says "you broke something" to
    /// someone who merely selected a photograph.
    Empty(String),
}

impl Stage {
    pub fn loading() -> Self {
        Stage::Loading {
            since: Instant::now(),
            recognised: None,
        }
    }
}

/// Something the user asked for that the app, not the view, has to carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultAction {
    Copy,
    Speak,
    SwitchTo(ResultView),
    /// Start, or stop, re-translating this area as its text changes.
    ToggleWatch,
    Close,
}

pub struct ResultUi {
    pub stage: Stage,
    pub view: ResultView,
    /// What was captured, in desktop pixels.
    pub anchor: Bounds,
    /// The display it was captured from.
    pub monitor: Monitor,
    /// egui points → physical pixels.
    pub ppp: f32,
    /// The floating window stays above other windows.
    pub pinned: bool,
    /// Show the untranslated text instead, for a quick compare.
    pub show_original: bool,
    pub actions: Vec<ResultAction>,

    /// Mirrors of the settings, refreshed every frame so a change applies to
    /// the result already on screen rather than to the next one.
    pub style: Presentation,
    pub close_on_click_away: bool,
    /// The captured area is being watched: the result is replaced as the text
    /// under it changes, and is not something to time out or click away.
    pub watching: bool,
    /// Stays until closed on purpose — a sample put up from the settings
    /// window, which has to survive the clicks that tune it.
    pub keep: bool,
    /// A read or a translation is in flight. Only a watched area shows it:
    /// there the previous answer stays up while the next one is fetched.
    pub busy: bool,

    /// The captured pixels. The in-place view paints over them and reads its
    /// colours from them.
    shot: Option<Frame>,
    texture: Option<TextureHandle>,
    /// When the stage now on screen arrived.
    since: Instant,
    /// When this result was first put on screen.
    created: Instant,
    /// Bumped whenever what is shown changes, so cached layouts know to go.
    revision: u64,
    /// How many frames have been drawn. The floating views create their
    /// window hidden and show it on the second frame, once it has been told
    /// not to animate in or take the focus.
    frames: u32,
    /// The pointer was over the result until at least this moment.
    hover_until: Option<Instant>,
    /// A clean "no button down" reading has been seen since the result
    /// appeared. Without it the release of the selection drag itself counts
    /// as a click somewhere else.
    armed: bool,
    /// A window is not focused on the frame it is created, and closing on that
    /// would mean the result never appears at all. Only a *loss* counts.
    had_focus: bool,

    inplace: inplace::Cache,
    popup: popup::Measured,
    subtitle: subtitle::Cache,
    /// Remembered so the floating window reopens where the user left it.
    window_pos: Option<Pos2>,
}

impl ResultUi {
    pub fn new(view: ResultView, anchor: Bounds, monitor: Monitor, ppp: f32) -> Self {
        let now = Instant::now();
        Self {
            stage: Stage::loading(),
            view,
            anchor,
            monitor,
            ppp: if ppp > 0.0 { ppp } else { 1.0 },
            pinned: false,
            show_original: false,
            actions: Vec::new(),
            style: Presentation::default(),
            close_on_click_away: true,
            watching: false,
            keep: false,
            busy: false,
            shot: None,
            texture: None,
            since: now,
            created: now,
            revision: 0,
            frames: 0,
            hover_until: None,
            armed: false,
            had_focus: false,
            inplace: inplace::Cache::default(),
            popup: popup::Measured::default(),
            subtitle: subtitle::Cache::default(),
            window_pos: None,
        }
    }

    pub fn with_shot(mut self, shot: Option<Frame>) -> Self {
        self.shot = shot;
        self
    }

    /// Replaces what is shown.
    pub fn set_stage(&mut self, stage: Stage) {
        self.stage = stage;
        self.since = Instant::now();
        self.revision += 1;
        self.hover_until = None;
    }

    /// Swaps in a fresh capture of the same area — a watched region whose text
    /// has moved on.
    pub fn set_shot(&mut self, shot: Frame) {
        self.shot = Some(shot);
        self.texture = None;
        self.revision += 1;
    }

    /// Notes that the text has been read, so the wait can show it.
    pub fn set_recognised(&mut self, text: String) {
        if let Stage::Loading { recognised, .. } = &mut self.stage {
            *recognised = Some(text);
            self.revision += 1;
        }
    }

    pub fn set_view(&mut self, view: ResultView) {
        if self.view != view {
            self.view = view;
            self.frames = 0;
            self.revision += 1;
        }
    }

    /// The captured pixels this result was made from.
    pub fn shot(&self) -> Option<&Frame> {
        self.shot.as_ref()
    }

    pub fn result(&self) -> Option<&PipelineResult> {
        match &self.stage {
            Stage::Done(r) => Some(r.as_ref()),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.stage, Stage::Loading { .. })
    }

    /// The view that will actually be drawn.
    ///
    /// "Don't show" still needs somewhere to report a failure, and the in-place
    /// view needs the pixels it paints over; both fall back to the popup, which
    /// needs nothing.
    pub fn effective_view(&self) -> ResultView {
        match self.view {
            ResultView::Inline if self.shot.is_none() => ResultView::Popup,
            ResultView::None => ResultView::Popup,
            other => other,
        }
    }

    /// Title of the window the effective view lives in — how the platform
    /// layer finds it.
    pub fn title(&self) -> &'static str {
        match self.effective_view() {
            ResultView::Inline => inplace::TITLE,
            ResultView::Subtitle => subtitle::TITLE,
            ResultView::Window => window::TITLE,
            ResultView::Popup | ResultView::None => popup::TITLE,
        }
    }

    /// Whether the result is on screen in a window that covers the area it was
    /// captured from — which is what makes grabbing that area again a problem.
    pub fn covers_its_anchor(&self) -> bool {
        self.effective_view() == ResultView::Inline
    }

    /// Whether something on screen is still moving and wants another frame.
    pub fn is_animating(&self) -> bool {
        self.is_loading() || self.since.elapsed() < Duration::from_millis(260)
    }

    pub fn render(&mut self, ctx: &egui::Context, theme: &Theme, history_len: usize) {
        match self.effective_view() {
            ResultView::Inline => self.render_inplace(ctx, theme),
            ResultView::Subtitle => self.render_subtitle(ctx, theme),
            ResultView::Popup | ResultView::None => self.render_popup(ctx, theme),
            ResultView::Window => self.render_window(ctx, theme, history_len),
        }
        self.frames = self.frames.saturating_add(1);
    }

    // ── Geometry ─────────────────────────────────────────────────────────────

    fn to_points(&self, b: Bounds) -> Rect {
        Rect::from_min_size(
            Pos2::new(b.x as f32 / self.ppp, b.y as f32 / self.ppp),
            Vec2::new(b.w as f32 / self.ppp, b.h as f32 / self.ppp),
        )
    }

    fn anchor_points(&self) -> Rect {
        self.to_points(self.anchor)
    }

    /// Where a window may sit on the capture's display, in points.
    fn work_points(&self) -> Rect {
        self.to_points(self.monitor.work)
    }

    /// How much larger than a 1080-line screen this display is, in points.
    /// Type that reads well at one size is unreadable across the room on a
    /// television and oversized on a laptop.
    fn scale(&self) -> f32 {
        (self.monitor.bounds.h as f32 / self.ppp / 1080.0).clamp(0.85, 2.0)
    }

    /// Whether the floating views should be created hidden this frame. Only
    /// where the platform lets them be tuned before they are shown.
    fn starts_hidden(&self) -> bool {
        native::QUIET_WINDOWS && self.frames == 0
    }

    // ── Dismissal ────────────────────────────────────────────────────────────

    /// How much text is on screen, for timing how long it takes to read.
    fn visible_chars(&self) -> usize {
        match &self.stage {
            Stage::Done(r) => r.translated.chars().count(),
            Stage::Error(m) | Stage::Empty(m) => m.chars().count(),
            Stage::Loading { .. } => 0,
        }
    }

    /// How opaque a floating view is right now: it eases in when it appears
    /// and out before a timed dismissal.
    fn presence(&self, ctx: &egui::Context) -> f32 {
        const FADE: f32 = 0.16;
        let appear = (self.created.elapsed().as_secs_f32() / FADE).min(1.0);

        let leave = match self.deadline() {
            Some(deadline) if !self.hovered_recently() => {
                let left = deadline.saturating_duration_since(Instant::now()).as_secs_f32();
                (left / FADE).clamp(0.0, 1.0)
            }
            _ => 1.0,
        };

        let presence = appear.min(leave);
        if presence < 1.0 {
            ctx.request_repaint();
        }
        // Eased, so it settles rather than stops.
        presence * presence * (3.0 - 2.0 * presence)
    }

    fn deadline(&self) -> Option<Instant> {
        if self.is_loading() || self.watching || self.keep {
            return None;
        }
        self.style
            .auto_hide
            .duration(self.visible_chars())
            .map(|limit| self.since + limit)
    }

    fn hovered_recently(&self) -> bool {
        self.hover_until.is_some_and(|until| Instant::now() < until)
    }

    /// Whether a floating view should go away now. Called from inside its
    /// viewport every frame, with whether the pointer is over it.
    fn should_dismiss(&mut self, ctx: &egui::Context, hovered: bool) -> bool {
        let now = Instant::now();
        if hovered {
            // Someone reading with the pointer on the text is still reading.
            self.hover_until = Some(now + Duration::from_millis(1200));
        }

        if self.is_loading() || self.watching || self.keep {
            // Nothing to dismiss yet — but keep draining the button state, so
            // a click made while waiting is not held against the result.
            if native::QUIET_WINDOWS {
                let _ = native::clicked_elsewhere(self.title());
            }
            self.armed = false;
            return false;
        }

        if let Some(deadline) = self.deadline() {
            if now >= deadline && !self.hovered_recently() {
                return true;
            }
            ctx.request_repaint_after(
                deadline
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(250)),
            );
        }

        if !self.close_on_click_away {
            return false;
        }

        if native::QUIET_WINDOWS {
            let elsewhere = native::clicked_elsewhere(self.title());
            if !self.armed {
                self.armed = !elsewhere;
                false
            } else {
                elsewhere
            }
        } else {
            let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
            if focused {
                self.had_focus = true;
                false
            } else {
                self.had_focus
            }
        }
    }

    // ── Shared content ───────────────────────────────────────────────────────

    fn describe(&self) -> HeaderInfo {
        match &self.stage {
            Stage::Done(r) => HeaderInfo {
                source: r.source.badge().to_string(),
                target: r.target.badge().to_string(),
                right: if r.from_cache {
                    t("{engine} · from cache").replace("{engine}", r.engine.label())
                } else {
                    t("{engine} · {seconds} s")
                        .replace("{engine}", r.engine.label())
                        .replace(
                            "{seconds}",
                            &format!("{:.1}", r.engine_elapsed.as_secs_f32()),
                        )
                },
            },
            Stage::Loading { .. } => HeaderInfo {
                source: "—".into(),
                target: "—".into(),
                right: String::new(),
            },
            Stage::Error(_) => HeaderInfo {
                source: "—".into(),
                target: "—".into(),
                right: t("error").into(),
            },
            // No badge at all: nothing failed, so labelling the header
            // would be inventing a problem.
            Stage::Empty(_) => HeaderInfo {
                source: "—".into(),
                target: "—".into(),
                right: String::new(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::language::Language;
    use crate::entities::settings::{AutoHide, EngineKind};

    fn bounds(x: i32, y: i32, w: i32, h: i32) -> Bounds {
        Bounds { x, y, w, h }
    }

    fn ui(view: ResultView) -> ResultUi {
        let screen = bounds(0, 0, 1920, 1080);
        ResultUi::new(
            view,
            bounds(100, 100, 400, 80),
            Monitor {
                bounds: screen,
                work: bounds(0, 0, 1920, 1040),
            },
            1.0,
        )
    }

    fn done(text: &str) -> Stage {
        Stage::Done(Box::new(PipelineResult {
            original: "hello".into(),
            translated: text.into(),
            segments: Vec::new(),
            engine: EngineKind::Google,
            source: Language::En,
            target: Language::Ru,
            elapsed: Duration::ZERO,
            engine_elapsed: Duration::ZERO,
            from_cache: false,
        }))
    }

    fn a_frame() -> Frame {
        Frame::from_bgra(bounds(100, 100, 2, 2), vec![0; 16])
    }

    #[test]
    fn the_in_place_view_needs_the_captured_pixels() {
        // Nothing to paint over: the popup stands in.
        assert_eq!(ui(ResultView::Inline).effective_view(), ResultView::Popup);
        let with_pixels = ui(ResultView::Inline).with_shot(Some(a_frame()));
        assert_eq!(with_pixels.effective_view(), ResultView::Inline);
    }

    #[test]
    fn dont_show_still_has_somewhere_to_report_a_failure() {
        assert_eq!(ui(ResultView::None).effective_view(), ResultView::Popup);
    }

    #[test]
    fn each_view_lives_in_a_window_of_its_own() {
        let titles = [
            ui(ResultView::Inline).with_shot(Some(a_frame())).title(),
            ui(ResultView::Subtitle).title(),
            ui(ResultView::Popup).title(),
            ui(ResultView::Window).title(),
        ];
        for (i, a) in titles.iter().enumerate() {
            for b in &titles[i + 1..] {
                assert_ne!(a, b, "two views share a window title");
            }
        }
    }

    #[test]
    fn only_the_in_place_view_sits_on_top_of_what_it_captured() {
        assert!(ui(ResultView::Inline)
            .with_shot(Some(a_frame()))
            .covers_its_anchor());
        assert!(!ui(ResultView::Subtitle).covers_its_anchor());
        assert!(!ui(ResultView::Popup).covers_its_anchor());
    }

    #[test]
    fn a_result_still_loading_has_no_deadline() {
        let mut r = ui(ResultView::Subtitle);
        r.style.auto_hide = AutoHide::Seconds5;
        assert!(r.deadline().is_none());
    }

    #[test]
    fn a_finished_result_gets_a_deadline_only_when_asked_for() {
        let mut r = ui(ResultView::Subtitle);
        r.set_stage(done("привет"));
        assert!(r.deadline().is_none(), "the default is to stay");

        r.style.auto_hide = AutoHide::Seconds5;
        let deadline = r.deadline().expect("a timer was asked for");
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(left > Duration::from_secs(4) && left <= Duration::from_secs(5));
    }

    #[test]
    fn a_watched_area_is_never_timed_out() {
        let mut r = ui(ResultView::Subtitle);
        r.set_stage(done("привет"));
        r.style.auto_hide = AutoHide::Seconds5;
        r.watching = true;
        assert!(r.deadline().is_none());
    }

    #[test]
    fn a_sample_from_the_settings_window_is_never_timed_out() {
        let mut r = ui(ResultView::Subtitle);
        r.set_stage(done("привет"));
        r.style.auto_hide = AutoHide::Seconds5;
        r.keep = true;
        assert!(r.deadline().is_none());
    }

    #[test]
    fn a_new_stage_restarts_the_clock_and_invalidates_the_layout() {
        let mut r = ui(ResultView::Subtitle);
        let before = r.revision;
        r.set_stage(done("привет"));
        assert!(r.revision > before);
        assert!(r.since.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn recognised_text_is_only_recorded_while_waiting() {
        let mut r = ui(ResultView::Popup);
        r.set_recognised("hello".into());
        assert!(matches!(
            &r.stage,
            Stage::Loading { recognised: Some(text), .. } if text == "hello"
        ));

        r.set_stage(done("привет"));
        let revision = r.revision;
        r.set_recognised("late".into());
        assert_eq!(r.revision, revision, "a late progress note changed a finished result");
    }

    #[test]
    fn positions_are_converted_with_the_scale_factor() {
        let mut r = ui(ResultView::Popup);
        r.ppp = 1.5;
        let a = r.anchor_points();
        assert_eq!(a.min, Pos2::new(100.0 / 1.5, 100.0 / 1.5));
        assert_eq!(a.width(), 400.0 / 1.5);
    }

    #[test]
    fn type_scales_with_the_screen_within_sane_limits() {
        let mut r = ui(ResultView::Subtitle);
        assert_eq!(r.scale(), 1.0);

        r.monitor.bounds = bounds(0, 0, 3840, 2160);
        assert_eq!(r.scale(), 2.0);

        // The same 4K panel at 200 % scaling is a 1080-line screen in points.
        r.ppp = 2.0;
        assert_eq!(r.scale(), 1.0);

        r.monitor.bounds = bounds(0, 0, 800, 480);
        r.ppp = 1.0;
        assert_eq!(r.scale(), 0.85);
    }

    #[test]
    fn a_zero_scale_factor_is_not_trusted() {
        let r = ResultUi::new(
            ResultView::Popup,
            bounds(0, 0, 10, 10),
            Monitor {
                bounds: bounds(0, 0, 100, 100),
                work: bounds(0, 0, 100, 100),
            },
            0.0,
        );
        assert_eq!(r.ppp, 1.0);
        assert!(r.anchor_points().width().is_finite());
    }
}
