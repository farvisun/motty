//! How a pane moves: smooth scrolling and its snap back to a row, the slide
//! when output scrolls the screen, scrolling past the ends, and the cursor's
//! glide. Everything here is a pure function of the state and the time
//! passed in; the view owns the state and asks for frames only while
//! something is moving, and the element draws what [`FrameMotion`] says.
//!
//! Scroll positions are in rows measured up from the live bottom, like a
//! display offset: 0 follows the output, positive shows older rows, and
//! negative is past the last line.

use std::f32::consts::PI;
use std::time::{Duration, Instant};

use motty_theme::{ScrollPastEnd, ScrollPastFirst};

/// The slide when output scrolls the screen, or a wheel notch moves it.
pub const SLIDE: Duration = Duration::from_millis(100);
/// Back to a whole row once a gesture ends.
pub const SNAP: Duration = Duration::from_millis(120);
/// How long a gesture must be quiet before it counts as over: trackpad
/// momentum arrives as more moves after the finger lifts.
pub const SNAP_IDLE: Duration = Duration::from_millis(80);
/// How far the output slide may trail what was printed, in rows.
pub const MAX_LAG: f32 = 4.;
/// The cursor's glide along its row.
pub const CURSOR_GLIDE: Duration = Duration::from_millis(90);
/// The cursor's pop on click and focus.
pub const CURSOR_POP: Duration = Duration::from_millis(220);
/// How much the pop grows the cursor at its peak.
const POP_SIZE: f32 = 0.18;
/// After a click or focus, a glide overshoots its target for this long.
const OVERSHOOT_WINDOW: Duration = Duration::from_millis(300);

/// Fast at first, settling gently.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1. - (1. - t).powi(3)
}

/// Like [`ease_out`], but runs a little past the end and comes back.
pub fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    const C1: f32 = 1.70158;
    const C3: f32 = C1 + 1.;
    1. + C3 * (t - 1.).powi(3) + C1 * (t - 1.).powi(2)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    EaseOut,
    Overshoot,
}

/// A value moving from `from` to `to` over `duration`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glide {
    pub from: f32,
    pub to: f32,
    pub start: Instant,
    pub duration: Duration,
    pub curve: Curve,
}

impl Glide {
    /// 0 at the start, 1 at the end.
    pub fn progress(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        (elapsed / self.duration.as_secs_f32().max(f32::EPSILON)).min(1.)
    }

    pub fn value(&self, now: Instant) -> f32 {
        let t = self.progress(now);
        let eased = match self.curve {
            Curve::EaseOut => ease_out(t),
            Curve::Overshoot => ease_out_back(t),
        };
        self.from + (self.to - self.from) * eased
    }

    pub fn done(&self, now: Instant) -> bool {
        self.progress(now) >= 1.
    }
}

/// What the element draws differently this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameMotion {
    /// Rows the view is shifted on top of the grid's display offset
    /// (positive shows older rows); fractional while moving.
    pub scroll: f32,
    /// The cursor's offset from its cell, in cells.
    pub cursor_dx: f32,
    /// The cursor's size, 1 at rest.
    pub cursor_scale: f32,
}

impl Default for FrameMotion {
    fn default() -> Self {
        Self {
            scroll: 0.,
            cursor_dx: 0.,
            cursor_scale: 1.,
        }
    }
}

/// What scrolling past the ends depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    /// Rows of scrollback.
    pub history: usize,
    /// Rows on screen.
    pub rows: usize,
    /// The bottom-most screen line with text, if any.
    pub last_content: Option<i32>,
    /// The cursor's screen line.
    pub cursor_line: i32,
    /// A full-screen program owns the screen: no scrolling past the ends.
    pub alt_screen: bool,
}

/// Where scrolling stops, in whole rows from the live bottom: `min` ≤ 0
/// (past the last line), `max` ≥ the history (past the first).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollRange {
    pub min: f32,
    pub max: f32,
}

impl ScrollRange {
    pub fn new(past_end: ScrollPastEnd, past_first: ScrollPastFirst, extent: Extent) -> Self {
        let history = extent.history as f32;
        if extent.alt_screen {
            return Self {
                min: 0.,
                max: history,
            };
        }
        let middle = (extent.rows / 2) as i32;
        // The screen line that should be able to reach the top.
        let anchor = match past_end {
            ScrollPastEnd::Disabled => None,
            ScrollPastEnd::LastLineWithContent => extent.last_content,
            ScrollPastEnd::LastLineInMiddle => extent.last_content.map(|l| l - middle),
            ScrollPastEnd::CursorLine => Some(extent.cursor_line),
        };
        let min = -(anchor.unwrap_or(0).max(0) as f32);
        let max = match past_first.resolve(past_end) {
            ScrollPastFirst::FirstLineWithContent => history + extent.rows as f32 - 1.,
            ScrollPastFirst::FirstLineInMiddle => history + middle as f32,
            ScrollPastFirst::Disabled | ScrollPastFirst::SameAsLastLine => history,
        };
        Self { min, max }
    }

    pub fn clamp(&self, pos: f32) -> f32 {
        pos.clamp(self.min, self.max)
    }
}

/// The view's offset on top of the grid's display offset, in rows. The grid
/// only scrolls by whole rows within its history; this holds the fraction a
/// trackpad leaves, how far the view is past an end, and the glide between
/// where the view was drawn and where it is going.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollMotion {
    /// Where it rests (and stays, once any glide is over).
    held: f32,
    glide: Option<Glide>,
}

impl ScrollMotion {
    /// The offset to draw now.
    pub fn delta(&self, now: Instant) -> f32 {
        self.glide.map_or(self.held, |g| g.value(now))
    }

    /// Where it rests: 0, a trackpad's fraction, or rows past an end.
    pub fn held(&self) -> f32 {
        self.held
    }

    pub fn moving(&self, now: Instant) -> bool {
        self.glide.is_some_and(|g| !g.done(now))
    }

    /// Forgets a finished glide.
    pub fn tick(&mut self, now: Instant) {
        if self.glide.is_some_and(|g| g.done(now)) {
            self.glide = None;
        }
    }

    /// Someone else moved the view: start over on the grid's row.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Stops a glide where it is (a finger came down on the trackpad).
    pub fn hold(&mut self, now: Instant) {
        self.held = self.delta(now);
        self.glide = None;
    }

    fn glide_to(&mut self, from: f32, to: f32, duration: Duration, now: Instant) {
        self.held = to;
        self.glide = ((from - to).abs() > 1e-3).then_some(Glide {
            from,
            to,
            start: now,
            duration,
            curve: Curve::EaseOut,
        });
    }

    /// Splits a position into the grid's row (returned) and the rest.
    fn place(&mut self, pos: f32, history: usize) -> usize {
        let grid = pos.round().clamp(0., history as f32) as usize;
        self.held = pos - grid as f32;
        grid
    }

    /// A trackpad moving the view by `rows`, pixel by pixel; returns the
    /// grid's new display offset.
    pub fn drag(
        &mut self,
        rows: f32,
        offset: usize,
        history: usize,
        range: ScrollRange,
        now: Instant,
    ) -> usize {
        let pos = range.clamp(offset as f32 + self.delta(now) + rows);
        self.glide = None;
        self.place(pos, history)
    }

    /// A whole-row move (a wheel notch, or smooth scrolling off): lands on
    /// a row, gliding there when `animate`. Steps add up from where the
    /// last one is going, so a quick run of notches isn't lost.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        rows: f32,
        offset: usize,
        history: usize,
        range: ScrollRange,
        animate: bool,
        now: Instant,
    ) -> usize {
        let visual = offset as f32 + self.delta(now);
        let target = range.clamp((offset as f32 + self.held + rows).round());
        let grid = self.place(target, history);
        let from = if animate {
            visual - grid as f32
        } else {
            self.held
        };
        self.glide_to(from, self.held, SLIDE, now);
        grid
    }

    /// Glides to the nearest whole row once the gesture is over.
    pub fn snap(
        &mut self,
        offset: usize,
        history: usize,
        range: ScrollRange,
        now: Instant,
    ) -> usize {
        let visual = offset as f32 + self.delta(now);
        let grid = self.place(range.clamp(visual.round()), history);
        self.glide_to(visual - grid as f32, self.held, SNAP, now);
        grid
    }

    /// The scrollbar's thumb dragged to `pos`, within the history; whole
    /// rows only when smooth scrolling is off.
    pub fn jump(&mut self, pos: f32, history: usize, whole_rows: bool) -> usize {
        let pos = pos.clamp(0., history as f32);
        self.glide = None;
        self.place(if whole_rows { pos.round() } else { pos }, history)
    }

    /// Typing: back to the live bottom.
    pub fn home(&mut self, animate: bool, now: Instant) {
        let from = if animate { self.delta(now) } else { 0. };
        self.glide_to(from, 0., SLIDE, now);
    }

    /// The terminal scrolled itself by `pushed` rows while the view was at
    /// the live bottom (display offset 0). The view starts where the rows
    /// were and slides up into place when `slide`, at most [`MAX_LAG`]
    /// rows behind; more than a screen at once jumps. Being past the last
    /// line is used up by the new rows unless `sticky`.
    #[allow(clippy::too_many_arguments)]
    pub fn output(
        &mut self,
        pushed: usize,
        rows: usize,
        history: usize,
        slide: bool,
        sticky: bool,
        now: Instant,
    ) {
        let n = pushed as f32;
        let target = if self.held < 0. && !sticky {
            (self.held + n).min(0.)
        } else {
            self.held
        };
        let from = if slide && pushed < rows {
            (self.delta(now) + n)
                .clamp(target - MAX_LAG, target + MAX_LAG)
                .min(history as f32)
        } else {
            target
        };
        self.glide_to(from, target, SLIDE, now);
    }
}

/// The cursor's glide along its row and its pop on click and focus.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CursorMotion {
    /// Where it was last frame: its row's name (`lines_pushed + line`) and
    /// column.
    last: Option<(i64, usize)>,
    /// Its offset from its cell, in cells, gliding to 0.
    glide: Option<Glide>,
    pop: Option<Glide>,
    /// Moves until then follow a click or focus, and overshoot.
    overshoot_until: Option<Instant>,
}

impl CursorMotion {
    /// The cursor is at `at` (`None` when hidden): a move along the same
    /// row glides, anything else jumps.
    pub fn moved(&mut self, at: Option<(i64, usize)>, now: Instant) {
        let previous = std::mem::replace(&mut self.last, at);
        match (previous, at) {
            (Some((row, from)), Some((to_row, to))) if row == to_row && from != to => {
                let curve = if self.overshoot_until.is_some_and(|t| now < t) {
                    Curve::Overshoot
                } else {
                    Curve::EaseOut
                };
                self.glide = Some(Glide {
                    from: self.dx(now) + from as f32 - to as f32,
                    to: 0.,
                    start: now,
                    duration: CURSOR_GLIDE,
                    curve,
                });
            }
            (a, b) if a == b => {}
            _ => self.glide = None,
        }
    }

    /// A click or focus: a small pop, and the next glide overshoots.
    pub fn pop(&mut self, now: Instant) {
        self.pop = Some(Glide {
            from: 0.,
            to: 1.,
            start: now,
            duration: CURSOR_POP,
            curve: Curve::EaseOut,
        });
        self.overshoot_until = Some(now + OVERSHOOT_WINDOW);
    }

    pub fn dx(&self, now: Instant) -> f32 {
        self.glide.map_or(0., |g| g.value(now))
    }

    pub fn scale(&self, now: Instant) -> f32 {
        self.pop
            .map_or(1., |g| 1. + POP_SIZE * (PI * g.progress(now)).sin())
    }

    pub fn moving(&self, now: Instant) -> bool {
        self.glide.is_some_and(|g| !g.done(now)) || self.pop.is_some_and(|g| !g.done(now))
    }

    /// Forgets what has finished.
    pub fn tick(&mut self, now: Instant) {
        if self.glide.is_some_and(|g| g.done(now)) {
            self.glide = None;
        }
        if self.pop.is_some_and(|g| g.done(now)) {
            self.pop = None;
        }
    }

    /// Animation off: everything stops where it should be.
    pub fn stop(&mut self) {
        *self = Self {
            last: self.last,
            ..Self::default()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn extent(history: usize) -> Extent {
        Extent {
            history,
            rows: 20,
            last_content: Some(5),
            cursor_line: 7,
            alt_screen: false,
        }
    }

    fn range(min: f32, max: f32) -> ScrollRange {
        ScrollRange { min, max }
    }

    #[test]
    fn curves_start_and_end_in_place() {
        for f in [ease_out, ease_out_back] {
            assert_eq!(f(0.), 0.);
            assert!((f(1.) - 1.).abs() < 1e-6);
        }
        assert!(ease_out(0.5) > 0.5, "front-loaded");
        assert!(
            (0..100).any(|i| ease_out_back(i as f32 / 100.) > 1.),
            "overshoots"
        );
        assert!((0..=100).all(|i| ease_out(i as f32 / 100.) <= 1.));
    }

    #[test]
    fn a_glide_is_driven_by_time() {
        let t0 = Instant::now();
        let g = Glide {
            from: 4.,
            to: 0.,
            start: t0,
            duration: ms(100),
            curve: Curve::EaseOut,
        };
        assert_eq!(g.value(t0), 4.);
        assert!(g.value(t0 + ms(50)) < 1.);
        assert_eq!(g.value(t0 + ms(100)), 0.);
        assert_eq!(g.value(t0 + ms(500)), 0.);
        assert!(!g.done(t0 + ms(99)) && g.done(t0 + ms(100)));
    }

    #[test]
    fn past_end_per_mode() {
        let e = extent(100);
        let none = ScrollPastFirst::Disabled;
        let r = |mode| ScrollRange::new(mode, none, e);
        assert_eq!(r(ScrollPastEnd::Disabled), range(0., 100.));
        // Line 5 (the prompt) can reach the top: 5 rows past the end.
        assert_eq!(r(ScrollPastEnd::LastLineWithContent).min, -5.);
        // …or the middle of 20 rows: it is already above it.
        assert_eq!(r(ScrollPastEnd::LastLineInMiddle).min, 0.);
        let deep = Extent {
            last_content: Some(17),
            ..e
        };
        assert_eq!(
            ScrollRange::new(ScrollPastEnd::LastLineInMiddle, none, deep).min,
            -7.
        );
        assert_eq!(r(ScrollPastEnd::CursorLine).min, -7.);
        // An empty screen has nothing to lift.
        let empty = Extent {
            last_content: None,
            ..e
        };
        assert_eq!(
            ScrollRange::new(ScrollPastEnd::LastLineWithContent, none, empty).min,
            0.
        );
    }

    #[test]
    fn past_first_line_per_mode() {
        let e = extent(100);
        let r = |end, first| ScrollRange::new(end, first, e).max;
        use ScrollPastEnd as E;
        use ScrollPastFirst as F;
        assert_eq!(r(E::Disabled, F::Disabled), 100.);
        // The first row lands at the bottom of 20 rows.
        assert_eq!(r(E::Disabled, F::FirstLineWithContent), 119.);
        assert_eq!(r(E::Disabled, F::FirstLineInMiddle), 110.);
        assert_eq!(r(E::Disabled, F::SameAsLastLine), 100.);
        assert_eq!(r(E::LastLineInMiddle, F::SameAsLastLine), 110.);
        assert_eq!(r(E::CursorLine, F::SameAsLastLine), 119.);
    }

    #[test]
    fn the_alternate_screen_never_scrolls_past() {
        let e = Extent {
            alt_screen: true,
            history: 0,
            ..extent(0)
        };
        let r = ScrollRange::new(
            ScrollPastEnd::CursorLine,
            ScrollPastFirst::FirstLineInMiddle,
            e,
        );
        assert_eq!(r, range(0., 0.));
    }

    #[test]
    fn a_drag_keeps_the_fraction_and_moves_the_grid_by_rows() {
        let now = Instant::now();
        let mut m = ScrollMotion::default();
        let r = range(0., 50.);
        let grid = m.drag(2.3, 0, 50, r, now);
        assert_eq!(grid, 2);
        assert!((m.delta(now) - 0.3).abs() < 1e-5);
        // Past the middle of a row the grid takes the next one.
        let grid = m.drag(0.4, grid, 50, r, now);
        assert_eq!(grid, 3);
        assert!((m.delta(now) + 0.3).abs() < 1e-5);
        // Clamped at the ends.
        assert_eq!(m.drag(-10., grid, 50, r, now), 0);
        assert_eq!(m.delta(now), 0.);
        assert_eq!(m.drag(100., 0, 50, r, now), 50);
        assert_eq!(m.delta(now), 0.);
    }

    #[test]
    fn dragging_past_the_end_is_held_beyond_the_grid() {
        let now = Instant::now();
        let mut m = ScrollMotion::default();
        let grid = m.drag(-3.5, 0, 50, range(-5., 50.), now);
        assert_eq!(grid, 0);
        assert_eq!(m.delta(now), -3.5);
        let grid = m.drag(-9., 0, 50, range(-5., 50.), now);
        assert_eq!((grid, m.delta(now)), (0, -5.));
        // Past the first line, over the top of the history.
        let grid = m.drag(65., 0, 50, range(-5., 60.), now);
        assert_eq!((grid, m.delta(now)), (50, 10.));
    }

    #[test]
    fn snapping_lands_on_the_nearest_row_with_pixels_aligned() {
        let t0 = Instant::now();
        let mut m = ScrollMotion::default();
        let r = range(0., 50.);
        let grid = m.drag(4.4, 0, 50, r, t0);
        let grid = m.snap(grid, 50, r, t0);
        assert_eq!(grid, 4);
        assert_eq!(m.held(), 0.);
        // It glides from where it was drawn…
        assert!((m.delta(t0) - 0.4).abs() < 1e-5);
        assert!(m.moving(t0 + ms(60)));
        // …and ends exactly on the row.
        assert_eq!(m.delta(t0 + SNAP), 0.);
        m.tick(t0 + SNAP);
        assert!(!m.moving(t0 + SNAP));
        let grid = m.drag(0.6, grid, 50, r, t0);
        assert_eq!(m.snap(grid, 50, r, t0), 5);
        // Past the end it snaps within the range, not to the grid.
        let mut m = ScrollMotion::default();
        m.drag(-2.7, 0, 50, range(-5., 50.), t0);
        m.snap(0, 50, range(-5., 50.), t0);
        assert_eq!(m.held(), -3.);
    }

    #[test]
    fn wheel_steps_add_up_and_glide() {
        let t0 = Instant::now();
        let mut m = ScrollMotion::default();
        let r = range(0., 50.);
        let grid = m.step(3., 0, 50, r, true, t0);
        assert_eq!(grid, 3);
        // Drawn where it was, three rows below.
        assert_eq!(m.delta(t0), -3.);
        // A second notch mid-glide counts from where the first is going.
        let grid = m.step(3., grid, 50, r, true, t0 + ms(20));
        assert_eq!(grid, 6);
        assert_eq!(m.delta(t0 + ms(20) + SLIDE), 0.);
        // Without animation it lands at once.
        let grid = m.step(-2., grid, 50, r, false, t0);
        assert_eq!((grid, m.delta(t0)), (4, 0.));
    }

    #[test]
    fn output_slides_in_and_keeps_up() {
        let t0 = Instant::now();
        let mut m = ScrollMotion::default();
        // One line: drawn a row down, sliding up over SLIDE.
        m.output(1, 30, 1000, true, false, t0);
        assert_eq!(m.delta(t0), 1.);
        assert!(m.delta(t0 + ms(50)) < 0.5);
        assert_eq!(m.delta(t0 + SLIDE), 0.);
        // Return held down: a line every frame never falls far behind.
        let mut m = ScrollMotion::default();
        let mut now = t0;
        let mut worst: f32 = 0.;
        for _ in 0..200 {
            now += ms(16);
            m.output(1, 30, 1000, true, false, now);
            worst = worst.max(m.delta(now));
        }
        assert!(worst <= 3., "lag {worst}");
        // A faster stream is capped at MAX_LAG.
        let mut m = ScrollMotion::default();
        for i in 0..50 {
            m.output(6, 30, 1000, true, false, t0 + ms(16 * i));
            assert!(m.delta(t0 + ms(16 * i)) <= MAX_LAG);
        }
    }

    #[test]
    fn output_that_outruns_the_slide_jumps() {
        let t0 = Instant::now();
        let mut m = ScrollMotion::default();
        m.output(30, 30, 1000, true, false, t0);
        assert_eq!(m.delta(t0), 0.);
        assert!(!m.moving(t0));
        // Animation off cuts too.
        m.output(1, 30, 1000, false, false, t0);
        assert_eq!(m.delta(t0), 0.);
        // Never further back than the history goes.
        m.output(3, 30, 1, true, false, t0);
        assert_eq!(m.delta(t0), 1.);
    }

    #[test]
    fn output_uses_up_scrolling_past_the_end_unless_sticky() {
        let t0 = Instant::now();
        let mut m = ScrollMotion::default();
        m.drag(-5., 0, 50, range(-5., 50.), t0);
        // Two new rows: the view stays put, two rows less past the end.
        m.output(2, 30, 50, true, false, t0);
        assert_eq!((m.delta(t0), m.held()), (-3., -3.));
        // Five more: the rest is used up and the remainder slides.
        m.output(5, 30, 50, true, false, t0);
        assert_eq!((m.delta(t0), m.held()), (2., 0.));
        // Sticky keeps the offset; the content slides under it.
        let mut m = ScrollMotion::default();
        m.drag(-5., 0, 50, range(-5., 50.), t0);
        m.output(2, 30, 50, true, true, t0);
        assert_eq!((m.delta(t0), m.held()), (-3., -5.));
        assert_eq!(m.delta(t0 + SLIDE), -5.);
    }

    #[test]
    fn the_thumb_jumps_within_the_history() {
        let t0 = Instant::now();
        let mut m = ScrollMotion::default();
        m.drag(-3., 0, 50, range(-5., 50.), t0);
        assert_eq!(m.jump(12.25, 50, false), 12);
        assert_eq!(m.delta(t0), 0.25);
        assert_eq!(m.jump(12.25, 50, true), 12);
        assert_eq!(m.delta(t0), 0.);
        // Never past either end.
        assert_eq!((m.jump(-4., 50, false), m.delta(t0)), (0, 0.));
        assert_eq!((m.jump(80., 50, false), m.delta(t0)), (50, 0.));
    }

    #[test]
    fn typing_goes_home() {
        let t0 = Instant::now();
        let mut m = ScrollMotion::default();
        m.drag(-4., 0, 50, range(-5., 50.), t0);
        m.home(true, t0);
        assert_eq!(m.delta(t0), -4.);
        assert_eq!(m.delta(t0 + SLIDE), 0.);
        m.drag(-4., 0, 50, range(-5., 50.), t0);
        m.home(false, t0);
        assert_eq!(m.delta(t0), 0.);
    }

    #[test]
    fn the_cursor_glides_along_its_row_only() {
        let t0 = Instant::now();
        let mut c = CursorMotion::default();
        c.moved(Some((10, 4)), t0);
        assert_eq!(c.dx(t0), 0.);
        c.moved(Some((10, 5)), t0);
        assert_eq!(c.dx(t0), -1.);
        assert!(c.moving(t0 + ms(30)));
        assert_eq!(c.dx(t0 + CURSOR_GLIDE), 0.);
        // Chained moves start from where it's drawn.
        c.moved(Some((10, 9)), t0 + ms(30));
        let dx = c.dx(t0 + ms(30));
        assert!(dx < -4. && dx > -5., "{dx}");
        // Another row: a jump.
        c.moved(Some((11, 0)), t0 + ms(40));
        assert_eq!(c.dx(t0 + ms(40)), 0.);
        assert!(!c.moving(t0 + ms(40)));
        // Hidden and back: a jump.
        c.moved(None, t0);
        c.moved(Some((11, 3)), t0);
        assert_eq!(c.dx(t0), 0.);
    }

    #[test]
    fn a_click_pops_and_the_next_glide_overshoots() {
        let t0 = Instant::now();
        let mut c = CursorMotion::default();
        c.moved(Some((0, 0)), t0);
        c.pop(t0);
        assert_eq!(c.scale(t0), 1.);
        let peak = c.scale(t0 + CURSOR_POP / 2);
        assert!(peak > 1.1 && peak <= 1. + POP_SIZE, "{peak}");
        assert!((c.scale(t0 + CURSOR_POP) - 1.).abs() < 1e-5);
        c.moved(Some((0, 6)), t0 + ms(10));
        let passed = (0..=90).any(|i| c.dx(t0 + ms(10 + i)) > 0.);
        assert!(passed, "runs past its cell and back");
        assert_eq!(c.dx(t0 + ms(10) + CURSOR_GLIDE), 0.);
        // Later moves don't.
        c.moved(Some((0, 12)), t0 + ms(400));
        assert!((0..=90).all(|i| c.dx(t0 + ms(400 + i)) <= 0.));
        c.tick(t0 + ms(1000));
        assert!(!c.moving(t0 + ms(1000)));
    }
}
