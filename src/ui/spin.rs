//! The working animation for the brand mark.
//!
//! While a translation is in flight the sakura turns and breathes. When the work
//! ends it does not snap back: it coasts on to the next fifth of a turn, where
//! the mark's five-fold symmetry makes it identical to rest, and the breath
//! eases out along the way. The eye sees a spin coming to a stop rather than a
//! frame that changed.
//!
//! Kept here, away from any toolkit, because both the settings window and the
//! tray thread run it — and because "does it actually settle?" is a question
//! worth answering with a test rather than by watching an icon.

use crate::shared::mark::SYMMETRY_TURN;

const TURNS_PER_SECOND: f32 = 0.55;
/// How much the mark shrinks at full speed. Enough to read as alive, not enough
/// to look like a rendering glitch.
const BREATH: f32 = 0.14;

#[derive(Debug, Default, Clone, Copy)]
pub struct Spin {
    /// Rotation, in turns. Always in `0.0..1.0`.
    turns: f32,
    /// How engaged the animation is: 1 at full speed, 0 at rest.
    energy: f32,
}

impl Spin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn turns(&self) -> f32 {
        self.turns
    }

    pub fn scale(&self) -> f32 {
        1.0 - BREATH * self.energy
    }

    /// True when the mark is exactly at rest and nothing needs redrawing.
    pub fn is_at_rest(&self) -> bool {
        self.energy <= 0.0
    }

    /// Advances by `dt` seconds. Returns whether anything is still moving, which
    /// is what the caller uses to decide between another frame and stopping.
    pub fn advance(&mut self, dt: f32, busy: bool) -> bool {
        let dt = dt.clamp(0.0, 0.1);

        if busy {
            self.energy += (1.0 - self.energy) * (dt * 6.0).min(1.0);
            self.turns = (self.turns + dt * TURNS_PER_SECOND) % 1.0;
            return true;
        }

        if self.is_at_rest() {
            return false;
        }

        // Coast to the next angle that looks like rest. The floor on the step
        // guarantees arrival: an easing that only ever covers a fraction of the
        // remaining distance would approach it forever.
        let landing = next_landing(self.turns);
        let remaining = landing - self.turns;
        let step = (remaining * (dt * 5.0).min(1.0)).max(dt * TURNS_PER_SECOND * 0.4);

        if step >= remaining {
            self.turns = landing % 1.0;
            self.energy = 0.0;
            return false;
        }

        self.turns += step;
        self.energy = (self.energy - dt * 1.6).max(0.0);
        true
    }
}

/// The next multiple of the symmetry step strictly ahead of `turns`.
fn next_landing(turns: f32) -> f32 {
    let steps = (turns / SYMMETRY_TURN).floor() + 1.0;
    steps * SYMMETRY_TURN
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 1.0 / 60.0;

    #[test]
    fn a_fresh_spin_is_at_rest() {
        let s = Spin::new();
        assert!(s.is_at_rest());
        assert_eq!(s.scale(), 1.0);
        assert_eq!(s.turns(), 0.0);
    }

    #[test]
    fn being_busy_turns_the_mark() {
        let mut s = Spin::new();
        for _ in 0..30 {
            assert!(s.advance(FRAME, true));
        }
        assert!(s.turns() > 0.0);
        assert!(!s.is_at_rest());
    }

    #[test]
    fn the_breath_deepens_while_busy_and_never_collapses() {
        let mut s = Spin::new();
        for _ in 0..120 {
            s.advance(FRAME, true);
        }
        let scale = s.scale();
        assert!(scale < 1.0, "the mark never started breathing");
        assert!(scale > 0.8, "the mark shrank too far: {scale}");
    }

    #[test]
    fn it_settles_on_a_symmetry_angle_after_the_work_ends() {
        let mut s = Spin::new();
        for _ in 0..100 {
            s.advance(FRAME, true);
        }

        let mut frames = 0;
        while s.advance(FRAME, false) {
            frames += 1;
            assert!(frames < 600, "the spin never came to rest");
        }

        assert!(s.is_at_rest());
        assert_eq!(s.scale(), 1.0, "the breath did not ease out");

        // A multiple of a fifth of a turn looks exactly like zero.
        let remainder = (s.turns() / SYMMETRY_TURN).fract();
        assert!(
            !(1e-3..=1.0 - 1e-3).contains(&remainder),
            "stopped at {} turns, not on a symmetry angle",
            s.turns()
        );
    }

    #[test]
    fn settling_takes_well_under_a_second() {
        // A stop that drags on reads as the app still working.
        let mut s = Spin::new();
        for _ in 0..100 {
            s.advance(FRAME, true);
        }
        let mut frames = 0;
        while s.advance(FRAME, false) {
            frames += 1;
        }
        assert!(frames < 60, "took {frames} frames to settle");
    }

    #[test]
    fn advancing_at_rest_reports_nothing_to_do() {
        let mut s = Spin::new();
        assert!(!s.advance(FRAME, false));
        assert!(!s.advance(FRAME, false));
    }

    #[test]
    fn the_rotation_never_runs_away() {
        let mut s = Spin::new();
        for _ in 0..10_000 {
            s.advance(FRAME, true);
            assert!(
                (0.0..1.0).contains(&s.turns()),
                "turns escaped the unit range: {}",
                s.turns()
            );
        }
    }

    #[test]
    fn a_long_stalled_frame_does_not_make_it_jump() {
        // A frame delayed by a second must not spin the mark five times.
        let mut s = Spin::new();
        s.advance(5.0, true);
        assert!(s.turns() <= 0.1 * TURNS_PER_SECOND + 1e-6);
    }

    #[test]
    fn becoming_busy_again_mid_settle_resumes() {
        let mut s = Spin::new();
        for _ in 0..60 {
            s.advance(FRAME, true);
        }
        s.advance(FRAME, false);
        assert!(s.advance(FRAME, true));
        assert!(!s.is_at_rest());
    }

    #[test]
    fn the_landing_is_always_ahead() {
        for turns in [0.0, 0.01, 0.199, 0.2, 0.75, 0.999] {
            let landing = next_landing(turns);
            assert!(landing > turns, "landing {landing} is not ahead of {turns}");
            assert!(landing - turns <= SYMMETRY_TURN + 1e-6);
        }
    }
}
