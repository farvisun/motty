//! Programs that redraw in place, and the stale copies some leave behind.
//!
//! Two things are watched on the PTY thread:
//!
//! - **Redraws.** Any upward cursor move, line insert/delete or erase of the
//!   screen bumps a counter. The view only slides output the terminal
//!   scrolled by itself; a frame in which the program redrew in place
//!   (Claude Code, a prompt redrawn after Return, a multi-line progress
//!   display) cuts instead of gliding.
//!
//! - **Repaints taller than the window** (`dedup-repaint-scrollback`).
//!   Renderers like Ink (Claude Code's classic one) erase their last frame
//!   by moving the cursor up once per line it drew, then draw the new
//!   frame. When that frame is taller than the window the cursor can't get
//!   above the top row: the moves that would have reached the rows already
//!   in scrollback are clamped, and drawing the new frame pushes its top
//!   rows into scrollback again, after the old ones. Every frame leaves one
//!   more copy.
//!
//! The detector is deliberately narrow. A *pass* starts when a cursor-up
//! is clamped at the top row (the program wanted rows above the screen);
//! consecutive clamped moves, with nothing printed in between, add to how
//! far it reached (`reach`). Those `reach` rows just above the screen are
//! what it meant to overwrite. Once the program has pushed at least `reach`
//! new rows into scrollback, the old `reach` rows are compared with the
//! first `reach` new ones, cell by cell (text, colors, attributes, links).
//! Only an exact match of at least [`MIN_BLOCK`] rows, not all blank, is
//! dropped, and only the old copy, so scrollback reads as if the screen had
//! been tall enough. A pass that hasn't pushed its rows within
//! [`PASS_TIMEOUT`] (the frame came out shorter) is dropped, so later,
//! unrelated output is never compared. Anything else ends the pass without
//! touching a row:
//! different content, a resize, the alternate screen, origin mode, scroll
//! regions, inserted or deleted lines, a reset, a clear of the screen or
//! history. Output that repeats without first reaching above the screen (a
//! status line after each command, `yes`) is never looked at.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;

/// The fewest rows a stale copy must have to be dropped: a single repeated
/// row (a lone prompt) is too likely to repeat by chance.
pub const MIN_BLOCK: usize = 2;

/// How long a repaint may take to push the rows it reached: renderers
/// write the erase and the new frame in one go.
pub const PASS_TIMEOUT: Duration = Duration::from_millis(500);

/// Watches one terminal's output; lives on its PTY thread.
pub struct Repaint {
    redraws: Arc<AtomicU64>,
    enabled: Arc<AtomicBool>,
    pending: Option<Pass>,
}

/// A repaint that reached above the screen, waiting for its new rows.
#[derive(Clone, Copy, Debug)]
struct Pass {
    /// `lines_pushed` when it first reached above the top row.
    anchor: u64,
    /// How many rows above the screen it tried to reach.
    reach: usize,
    /// Still moving up: further clamped moves add to `reach`.
    open: bool,
    /// Columns and screen lines; a resize ends the pass.
    size: (usize, usize),
    started: Instant,
}

impl Default for Repaint {
    fn default() -> Self {
        Self {
            redraws: Default::default(),
            enabled: Arc::new(AtomicBool::new(true)),
            pending: None,
        }
    }
}

impl Repaint {
    /// The redraw counter, for the UI.
    pub fn redraws(&self) -> Arc<AtomicU64> {
        self.redraws.clone()
    }

    /// The `dedup-repaint-scrollback` switch, for the UI.
    pub fn enabled(&self) -> Arc<AtomicBool> {
        self.enabled.clone()
    }

    /// The program redrew something in place.
    pub fn redraw(&mut self) {
        self.redraws.fetch_add(1, Ordering::Relaxed);
    }

    /// Something that breaks the pattern: forget the pass.
    pub fn reset(&mut self) {
        self.redraw();
        self.pending = None;
    }

    /// Text or a new line was written: a pass stops reaching further.
    #[inline]
    pub fn text(&mut self) {
        if let Some(pass) = &mut self.pending {
            pass.open = false;
        }
    }

    /// The cursor moved up `lines` from line `from` (already applied).
    pub fn moved_up<T: EventListener>(&mut self, term: &mut Term<T>, from: i32, lines: usize) {
        if lines == 0 {
            return;
        }
        self.redraw();
        if !self.enabled.load(Ordering::Relaxed)
            || term
                .mode()
                .intersects(TermMode::ALT_SCREEN | TermMode::ORIGIN)
        {
            self.pending = None;
            return;
        }
        let grid = term.grid();
        let reach = lines.saturating_sub(from.max(0) as usize);
        if reach == 0 || grid.cursor.point.line != 0 {
            return;
        }
        let (pushed, history) = (grid.lines_pushed(), grid.history_size());
        match &mut self.pending {
            Some(pass) if pass.open && pass.anchor == pushed => {
                pass.reach = (pass.reach + reach).min(history);
            }
            _ => {
                // Two frames in one read: the last one settles first.
                self.settle(term);
                let grid = term.grid();
                let (pushed, reach) = (grid.lines_pushed(), reach.min(grid.history_size()));
                self.pending = (reach > 0).then(|| Pass {
                    anchor: pushed,
                    reach,
                    open: true,
                    size: (term.columns(), term.screen_lines()),
                    started: Instant::now(),
                });
            }
        }
    }

    /// Drops the stale copy once the pass has pushed enough new rows to
    /// compare; called after each batch of output (and before a new pass).
    /// Returns the stable row (`lines_pushed` + line) of the first removed
    /// row and how many went, so images on later rows can follow.
    pub fn settle<T: EventListener>(&mut self, term: &mut Term<T>) -> Option<(u64, u64)> {
        self.settle_at(term, Instant::now())
    }

    fn settle_at<T: EventListener>(
        &mut self,
        term: &mut Term<T>,
        now: Instant,
    ) -> Option<(u64, u64)> {
        let pass = self.pending?;
        if !self.enabled.load(Ordering::Relaxed)
            || term.mode().contains(TermMode::ALT_SCREEN)
            || (term.columns(), term.screen_lines()) != pass.size
        {
            self.pending = None;
            return None;
        }
        let grid = term.grid();
        let Some(pushed) = grid.lines_pushed().checked_sub(pass.anchor) else {
            self.pending = None;
            return None;
        };
        if pushed < pass.reach as u64 {
            if now.duration_since(pass.started) > PASS_TIMEOUT {
                self.pending = None;
            }
            return None;
        }
        self.pending = None;
        let history = grid.history_size();
        if pass.reach < MIN_BLOCK || pushed + pass.reach as u64 > history as u64 {
            return None;
        }
        let new_top = -(pushed as i32);
        let old_top = new_top - pass.reach as i32;
        let cols = grid.columns();
        let same = (0..pass.reach as i32).all(|i| {
            let (old, new) = (&grid[Line(old_top + i)], &grid[Line(new_top + i)]);
            (0..cols).all(|c| old[Column(c)] == new[Column(c)])
        });
        let blank = (0..pass.reach as i32).all(|i| grid[Line(old_top + i)].is_clear());
        if !same || blank {
            return None;
        }
        tracing::trace!(rows = pass.reach, "dropping a repaint's stale copy");
        let first = (grid.lines_pushed() as i64 + i64::from(old_top)) as u64;
        // The removed copy equals the one below it, so a selection in it
        // moves onto that one; older rows move down with the history.
        let range = Line(term.topmost_line().0)..Line(new_top);
        term.selection = term
            .selection
            .take()
            .and_then(|s| s.rotate(&*term, &range, -(pass.reach as i32)));
        term.grid_mut()
            .remove_history_lines(Line(old_top), pass.reach);
        Some((first, pass.reach as u64))
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;
    use crate::GridSize;
    use crate::graphemes::{Cluster, Graphemes};

    struct Harness {
        term: Term<VoidListener>,
        parser: Processor,
        repaint: Repaint,
    }

    impl Harness {
        fn new(rows: u16, enabled: bool) -> Self {
            let size = GridSize {
                cols: 20,
                rows,
                cell_width: 8.,
                cell_height: 16.,
            };
            let repaint = Repaint::default();
            repaint.enabled.store(enabled, Ordering::Relaxed);
            Self {
                term: Term::new(Config::default(), &size, VoidListener),
                parser: Processor::new(),
                repaint,
            }
        }

        /// Feeds output as one read and settles, as the PTY thread does.
        fn feed(&mut self, bytes: &str) {
            self.parser.advance(
                &mut Graphemes {
                    term: &mut self.term,
                    cluster: &mut Cluster::default(),
                    pointer: &mut None,
                    repaint: &mut self.repaint,
                    graphics: &mut crate::graphics::Graphics::default(),
                },
                bytes.as_bytes(),
            );
            self.repaint.settle(&mut self.term);
        }

        /// Every row, scrollback first, trailing blanks trimmed; blank rows
        /// at the bottom of the screen dropped.
        fn lines(&self) -> Vec<String> {
            let grid = self.term.grid();
            let mut out: Vec<String> = (grid.topmost_line().0..=grid.bottommost_line().0)
                .map(|l| {
                    let row = &grid[Line(l)];
                    (0..grid.columns())
                        .map(|c| row[Column(c)].c)
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect();
            while out.last().is_some_and(|l| l.is_empty()) {
                out.pop();
            }
            out
        }
    }

    /// One Ink (log-update) frame: erase the previous one line by line,
    /// moving up, then draw the new one without a trailing newline (its
    /// `\n`s arrive as `\r\n`, the PTY's ONLCR).
    fn ink_frame(previous: usize, frame: &[&str]) -> String {
        let mut out = String::new();
        for i in 0..previous {
            out.push_str("\x1b[2K");
            if i + 1 < previous {
                out.push_str("\x1b[1A");
            }
        }
        if previous > 0 {
            out.push_str("\x1b[G");
        }
        out.push_str(&frame.join("\r\n"));
        out
    }

    fn frame(n: usize, footer: &str) -> Vec<String> {
        let mut lines: Vec<String> = (0..n).map(|i| format!("message {i}")).collect();
        lines.push(footer.to_owned());
        lines
    }

    fn refs(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }

    fn count(lines: &[String], text: &str) -> usize {
        lines.iter().filter(|l| *l == text).count()
    }

    #[test]
    fn a_frame_taller_than_the_window_keeps_one_copy() {
        let mut h = Harness::new(5, true);
        h.feed("$ claude\r\n");
        let mut previous = 0;
        for tick in 0..6 {
            let f = frame(9, &format!("spinner {tick}"));
            h.feed(&ink_frame(previous, &refs(&f)));
            previous = f.len();
        }
        let lines = h.lines();
        assert_eq!(count(&lines, "message 0"), 1, "{lines:#?}");
        assert_eq!(count(&lines, "message 4"), 1, "{lines:#?}");
        assert_eq!(lines.first().map(String::as_str), Some("$ claude"));
        assert_eq!(lines.last().map(String::as_str), Some("spinner 5"));
        // Nothing but the frame and the command line: no stale spinner.
        assert_eq!(lines.len(), 11, "{lines:#?}");
    }

    #[test]
    fn without_dedup_the_copies_pile_up() {
        let mut h = Harness::new(5, false);
        let mut previous = 0;
        for tick in 0..4 {
            let f = frame(9, &format!("spinner {tick}"));
            h.feed(&ink_frame(previous, &refs(&f)));
            previous = f.len();
        }
        assert_eq!(count(&h.lines(), "message 0"), 4);
    }

    #[test]
    fn a_growing_frame_keeps_one_copy_and_its_new_rows() {
        let mut h = Harness::new(5, true);
        let mut previous = 0;
        for n in 8..13 {
            let f = frame(n, "footer");
            h.feed(&ink_frame(previous, &refs(&f)));
            previous = f.len();
        }
        let lines = h.lines();
        for i in 0..12 {
            assert_eq!(count(&lines, &format!("message {i}")), 1, "{lines:#?}");
        }
    }

    #[test]
    fn frames_split_across_reads_and_batched_in_one() {
        let mut h = Harness::new(5, true);
        let f = frame(9, "a");
        h.feed(&ink_frame(0, &refs(&f)));
        // One frame in pieces.
        let text = ink_frame(f.len(), &refs(&frame(9, "b")));
        let (x, y) = text.split_at(text.len() / 2);
        h.feed(x);
        h.feed(y);
        // Two frames in one read.
        let two =
            ink_frame(f.len(), &refs(&frame(9, "c"))) + &ink_frame(f.len(), &refs(&frame(9, "d")));
        h.feed(&two);
        assert_eq!(count(&h.lines(), "message 0"), 1, "{:#?}", h.lines());
    }

    #[test]
    fn changed_content_is_left_alone() {
        let mut h = Harness::new(5, true);
        let f = frame(9, "x");
        h.feed(&ink_frame(0, &refs(&f)));
        let mut g = frame(9, "x");
        g[0] = "edited 0".into();
        h.feed(&ink_frame(f.len(), &refs(&g)));
        let lines = h.lines();
        assert_eq!(count(&lines, "message 0"), 1);
        assert_eq!(count(&lines, "edited 0"), 1);
        // The rest of the old copy stays too: all or nothing.
        assert_eq!(count(&lines, "message 1"), 2, "{lines:#?}");
    }

    #[test]
    fn colors_count_as_content() {
        let mut h = Harness::new(5, true);
        let f = frame(9, "x");
        h.feed(&ink_frame(0, &refs(&f)));
        let mut g = frame(9, "x");
        g[1] = "\x1b[31mmessage 1\x1b[0m".into();
        h.feed(&ink_frame(f.len(), &refs(&g)));
        assert_eq!(count(&h.lines(), "message 0"), 2);
    }

    #[test]
    fn repeated_output_is_never_touched() {
        let mut h = Harness::new(5, true);
        // A status line after every command, and a repeating block.
        for _ in 0..10 {
            h.feed("$ make\r\nstatus: ok\r\nblock a\r\nblock b\r\n");
        }
        let lines = h.lines();
        assert_eq!(count(&lines, "status: ok"), 10);
        assert_eq!(count(&lines, "block a"), 10);
    }

    #[test]
    fn redraws_that_fit_the_window_are_never_touched() {
        let mut h = Harness::new(10, true);
        // A three-line progress display redrawn in place, then output.
        for i in 0..5 {
            if i > 0 {
                h.feed("\x1b[3A\r");
            }
            h.feed(&format!("one {i}\x1b[K\r\ntwo\x1b[K\r\nthree\x1b[K\r\n"));
        }
        for _ in 0..20 {
            h.feed("two\r\nthree\r\n");
        }
        assert_eq!(count(&h.lines(), "two"), 21);
    }

    #[test]
    fn a_single_row_or_blank_rows_are_kept() {
        // Two rows reached and repeated: dropped (the control).
        let mut h = Harness::new(3, true);
        h.feed("a\r\nb\r\nc\r\nd\r\ne");
        h.feed("\x1b[4A\ra\r\nb\r\nc\r\nd\r\ne");
        assert_eq!(h.lines(), ["a", "b", "c", "d", "e"]);
        // One row of reach (the `a` above the screen): under MIN_BLOCK.
        let mut h = Harness::new(3, true);
        h.feed("a\r\nb\r\nc\r\nd");
        h.feed("\x1b[3A\ra\r\nb\r\nc\r\nd");
        assert_eq!(count(&h.lines(), "a"), 2, "{:#?}", h.lines());
        // Blank rows: two reached, two blank ones pushed again.
        let mut h = Harness::new(3, true);
        h.feed("\r\n\r\n\r\n\r\nx");
        h.feed("\x1b[4A\r\r\n\r\n\r\n\r\ny");
        assert_eq!(h.term.grid().history_size(), 4);
    }

    #[test]
    fn a_resize_or_the_alternate_screen_ends_the_pass() {
        let mut h = Harness::new(5, true);
        let f = frame(9, "x");
        h.feed(&ink_frame(0, &refs(&f)));
        let erase = ink_frame(f.len(), &[]);
        h.feed(&erase);
        h.term.resize(GridSize {
            cols: 20,
            rows: 6,
            cell_width: 8.,
            cell_height: 16.,
        });
        h.feed(&refs(&f).join("\r\n"));
        assert_eq!(count(&h.lines(), "message 0"), 2);

        let mut h = Harness::new(5, true);
        h.feed(&ink_frame(0, &refs(&f)));
        h.feed("\x1b[?1049h");
        h.feed(&ink_frame(f.len(), &refs(&f)));
        h.feed("\x1b[?1049l");
        assert_eq!(count(&h.lines(), "message 0"), 1);
    }

    #[test]
    fn a_pass_that_comes_out_short_expires() {
        let mut h = Harness::new(5, true);
        let f = frame(9, "x");
        h.feed(&ink_frame(0, &refs(&f)));
        // The next frame fits the window: nothing reaches scrollback.
        h.feed(&ink_frame(f.len(), &["short"]));
        h.repaint
            .settle_at(&mut h.term, Instant::now() + PASS_TIMEOUT * 2);
        // Later output that happens to repeat the reached rows is kept.
        h.feed("\r");
        h.feed(&refs(&f).join("\r\n"));
        assert_eq!(count(&h.lines(), "message 0"), 2, "{:#?}", h.lines());
    }

    #[test]
    fn inserted_lines_end_the_pass() {
        let mut h = Harness::new(5, true);
        let f = frame(9, "x");
        h.feed(&ink_frame(0, &refs(&f)));
        h.feed(&(ink_frame(f.len(), &[]) + "\x1b[L" + &refs(&f).join("\r\n")));
        assert_eq!(count(&h.lines(), "message 0"), 2);
    }

    #[test]
    fn redraws_are_counted() {
        let mut h = Harness::new(5, true);
        let counter = h.repaint.redraws();
        h.feed("plain output\r\nmore\r\n");
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        h.feed("\x1b[1A");
        assert_eq!(counter.load(Ordering::Relaxed), 1);
        h.feed("\x1b[2J");
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn removing_history_keeps_older_rows_and_the_view() {
        let mut h = Harness::new(3, false);
        h.feed("a\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng");
        let pushed = h.term.grid().lines_pushed();
        h.term.scroll_display(alacritty_terminal::grid::Scroll::Top);
        // Rows: a b c d | e f g. Drop c and d.
        h.term.grid_mut().remove_history_lines(Line(-2), 2);
        assert_eq!(h.lines(), ["a", "b", "e", "f", "g"]);
        assert_eq!(h.term.grid().lines_pushed(), pushed - 2);
        assert_eq!(h.term.grid().display_offset(), 2);
    }
}
