//! Scrolling a pane and the cursor's motion: the wheel and trackpad (pixel
//! by pixel, snapping back to a row when the gesture ends), the slide when
//! output scrolls the screen, scrolling past the ends, the overlay
//! scrollbar, and the cursor's glide. The math is in
//! [`motty_render::motion`]; this keeps the state, notices who moved the
//! view, and asks for frames only while something is moving.

use std::time::{Duration, Instant};

use gpui::{
    Bounds, Context, DispatchPhase, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, ScrollDelta, ScrollEvent, Task, TouchPhase, Window, canvas, fill, point, prelude::*,
    px, size,
};
use motty_render::FrameMotion;
use motty_render::motion::{CursorMotion, Extent, SNAP_IDLE, ScrollMotion, ScrollRange};
use motty_term::alacritty_terminal::grid::{Dimensions, Scroll};
use motty_term::alacritty_terminal::index::Line;
use motty_term::alacritty_terminal::term::TermMode;
use motty_term::alacritty_terminal::vte::ansi::NamedColor;
use motty_term::input::{self, MouseAction};
use motty_theme::{CursorAnimation, Motion, ScrollPastEnd};

use super::TerminalView;

/// How long the scrollbar stays after the view stops moving.
const BAR_LINGER: Duration = Duration::from_millis(1000);
/// The thumb's width, its gap to the edge, and its shortest length.
const THUMB_WIDTH: f32 = 6.;
const THUMB_INSET: f32 = 3.;
const THUMB_MIN: f32 = 24.;

/// Programs that redraw what they show rather than scrolling it, as far as
/// the view can tell: whatever runs behind them looks like a full-screen
/// program.
const REDRAWS_IN_PLACE: &[&str] = &["ssh", "mosh-client", "tmux", "screen", "zellij"];

#[derive(Default)]
pub(super) struct Scrolling {
    options: Motion,
    motion: ScrollMotion,
    cursor: CursorMotion,
    /// Wheel rows carried between events while scrolling by whole rows.
    remainder: f32,
    /// What the last frame saw, to tell the output's scrolling from
    /// everyone else's.
    seen: Option<Seen>,
    /// Snaps to a row once a trackpad gesture is quiet.
    snap: Option<Task<()>>,
    focused: bool,
    bar: Bar,
}

#[derive(Clone, Copy, PartialEq)]
struct Seen {
    pushed: u64,
    offset: usize,
    size: (usize, usize),
    redraws: u64,
    alt: bool,
}

#[derive(Default)]
struct Bar {
    /// Shown until then (unless following output hides it).
    until: Option<Instant>,
    /// Hides it once `until` passes.
    task: Option<Task<()>>,
    /// Dragging the thumb, grabbed this far below its top.
    grab: Option<Pixels>,
}

/// The grid as the scroll code needs it.
struct View {
    pushed: u64,
    offset: usize,
    extent: Extent,
    cursor: Option<(i64, usize)>,
}

impl TerminalView {
    pub fn set_motion(&mut self, motion: Motion) {
        let s = &mut self.scrolling;
        if !motion.smooth_scroll && s.options.smooth_scroll {
            s.motion.reset();
        }
        if motion.cursor_animation == CursorAnimation::Off {
            s.cursor.stop();
        }
        s.options = motion;
        self.session.set_dedup_repaints(motion.dedup_repaints);
    }

    fn view(&self) -> View {
        let term = self.session.term.lock();
        let grid = term.grid();
        let rows = term.screen_lines();
        let mode = *term.mode();
        let pushed = grid.lines_pushed();
        let cursor = grid.cursor.point;
        // Only the past-end modes that need it scan the screen.
        let by_content = matches!(
            self.scrolling.options.past_end,
            ScrollPastEnd::LastLineWithContent | ScrollPastEnd::LastLineInMiddle
        );
        View {
            pushed,
            offset: grid.display_offset(),
            extent: Extent {
                history: grid.history_size(),
                rows,
                last_content: by_content
                    .then(|| (0..rows as i32).rev().find(|&l| !grid[Line(l)].is_clear()))
                    .flatten(),
                cursor_line: cursor.line.0,
                alt_screen: mode.contains(TermMode::ALT_SCREEN),
            },
            cursor: (mode.contains(TermMode::SHOW_CURSOR) && !mode.contains(TermMode::VI))
                .then_some((pushed as i64 + cursor.line.0 as i64, cursor.column.0)),
        }
    }

    fn scroll_range(&self, view: &View) -> ScrollRange {
        let o = &self.scrolling.options;
        ScrollRange::new(o.past_end, o.past_first_line, view.extent)
    }

    /// Moves the grid to `offset`, which the scroll code chose.
    fn set_offset(&mut self, from: usize, offset: usize) {
        if offset != from {
            self.session
                .term
                .lock()
                .scroll_display(Scroll::Delta(offset as i32 - from as i32));
        }
        if let Some(seen) = self.scrolling.seen.as_mut() {
            seen.offset = offset;
        }
    }

    /// Whether the program in front redraws in place as far as the view
    /// can tell (ssh, tmux): its output never slides.
    fn redraws_in_place(&self) -> bool {
        self.system.ssh.is_some()
            || self
                .foreground
                .as_deref()
                .is_some_and(|f| REDRAWS_IN_PLACE.contains(&f))
    }

    pub(super) fn on_scroll(
        &mut self,
        event: &ScrollEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(layout) = self.layout.get() else {
            return;
        };
        let mode = self.term_mode();
        let to_program = self.reports_mouse(mode) && !event.modifiers.shift && !self.read_only;
        if to_program || mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            self.scroll_program(event, layout.line_height, to_program);
            return;
        }
        let now = Instant::now();
        let view = self.view();
        let range = self.scroll_range(&view);
        let history = view.extent.history;
        let smooth = self.scrolling.options.smooth_scroll;
        let offset = match event.delta {
            ScrollDelta::Pixels(p) if smooth => {
                if event.touch_phase == TouchPhase::Started {
                    self.scrolling.motion.hold(now);
                }
                let rows = f32::from(p.y) / f32::from(layout.line_height);
                let offset = self
                    .scrolling
                    .motion
                    .drag(rows, view.offset, history, range, now);
                self.snap_when_quiet(cx);
                offset
            }
            delta => {
                let lines = match delta {
                    ScrollDelta::Lines(p) => p.y,
                    ScrollDelta::Pixels(p) => f32::from(p.y) / f32::from(layout.line_height),
                } + self.scrolling.remainder;
                self.scrolling.remainder = lines.fract();
                if lines.trunc() == 0. {
                    return;
                }
                self.scrolling
                    .motion
                    .step(lines.trunc(), view.offset, history, range, smooth, now)
            }
        };
        self.set_offset(view.offset, offset);
        self.show_scrollbar(now, cx);
        cx.notify();
    }

    /// The wheel when a program has it: mouse reports, or arrow keys on
    /// the alternate screen.
    fn scroll_program(&mut self, event: &ScrollEvent, line_height: Pixels, report: bool) {
        let lines = match event.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => f32::from(p.y) / f32::from(line_height),
        } + self.scrolling.remainder;
        let whole = lines.trunc() as i32;
        self.scrolling.remainder = lines.fract();
        if whole == 0 {
            return;
        }
        if report {
            let button = if whole > 0 {
                input::MouseButton::WheelUp
            } else {
                input::MouseButton::WheelDown
            };
            for _ in 0..whole.unsigned_abs().min(10) {
                self.report_mouse(button, MouseAction::Press, event.position, &event.modifiers);
            }
        } else {
            let key: &[u8] = match (whole > 0, self.term_mode().contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.session
                .write(key.repeat(whole.unsigned_abs() as usize));
        }
    }

    /// Trackpad momentum keeps sending moves after the finger lifts; the
    /// snap waits for them to stop.
    fn snap_when_quiet(&mut self, cx: &mut Context<Self>) {
        self.scrolling.snap = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SNAP_IDLE).await;
            let _ = this.update(cx, |view, cx| view.snap_scroll(cx));
        }));
    }

    fn snap_scroll(&mut self, cx: &mut Context<Self>) {
        self.scrolling.snap = None;
        if self.scrolling.bar.grab.is_some() {
            return;
        }
        let now = Instant::now();
        let view = self.view();
        let range = self.scroll_range(&view);
        let offset = self
            .scrolling
            .motion
            .snap(view.offset, view.extent.history, range, now);
        self.set_offset(view.offset, offset);
        cx.notify();
    }

    /// Typing: back to the live bottom, from past the end too.
    pub(super) fn scroll_home(&mut self) {
        let animate = self.scrolling.options.smooth_scroll;
        self.scrolling.motion.home(animate, Instant::now());
    }

    /// A click in the pane: the cursor pops.
    pub(super) fn cursor_clicked(&mut self) {
        if self.scrolling.options.cursor_animation == CursorAnimation::Smooth {
            self.scrolling.cursor.pop(Instant::now());
        }
    }

    /// Works out this frame's motion: notices output that scrolled the
    /// screen (and slides it), someone else moving the view, and the
    /// cursor moving; asks for another frame while anything moves.
    pub(super) fn frame_motion(&mut self, focused: bool, window: &mut Window) -> FrameMotion {
        let now = Instant::now();
        let view = self.view();
        let redraws = self.session.redraws();
        let in_place = self.redraws_in_place();
        let extent = view.extent;
        let size = (self.session.size().cols as usize, extent.rows);
        let seen = Seen {
            pushed: view.pushed,
            offset: view.offset,
            size,
            redraws,
            alt: extent.alt_screen,
        };
        let s = &mut self.scrolling;
        let (mut user_moved, mut output_moved) = (false, false);
        if let Some(last) = s.seen {
            // Fewer rows pushed than before: a repaint's copy was dropped.
            let pushed = view.pushed.saturating_sub(last.pushed) as usize;
            let expected = if last.offset > 0 {
                (last.offset + pushed).min(extent.history)
            } else {
                0
            };
            if last.size != size || last.alt != seen.alt || seen.alt {
                s.motion.reset();
            } else if view.offset != expected {
                // Keys, find, vi mode, jumping between commands, typing.
                s.motion.reset();
                user_moved = true;
            } else if view.offset == 0 && pushed > 0 {
                let slide = s.options.slides_output() && redraws == last.redraws && !in_place;
                let o = s.options;
                s.motion.output(
                    pushed,
                    extent.rows,
                    extent.history,
                    slide,
                    o.past_end_sticky,
                    now,
                );
                output_moved = true;
            }
        }
        s.seen = Some(seen);

        let animate_cursor = s.options.cursor_animation == CursorAnimation::Smooth;
        if animate_cursor {
            if focused && !s.focused {
                s.cursor.pop(now);
            }
            s.cursor.moved(view.cursor, now);
        }
        s.focused = focused;

        if user_moved || (output_moved && !s.options.hide_scrollbar_following) {
            s.bar.until = Some(now + BAR_LINGER);
        }
        let frame = FrameMotion {
            scroll: s.motion.delta(now),
            cursor_dx: if animate_cursor { s.cursor.dx(now) } else { 0. },
            cursor_scale: if animate_cursor {
                s.cursor.scale(now)
            } else {
                1.
            },
        };
        if s.motion.moving(now) || (animate_cursor && s.cursor.moving(now)) {
            window.request_animation_frame();
        }
        s.motion.tick(now);
        s.cursor.tick(now);
        frame
    }

    /// Shows the scrollbar for a while.
    fn show_scrollbar(&mut self, now: Instant, cx: &mut Context<Self>) {
        self.scrolling.bar.until = Some(now + BAR_LINGER);
        self.hide_scrollbar_later(cx);
    }

    /// A single task hides the scrollbar once its time is up.
    fn hide_scrollbar_later(&mut self, cx: &mut Context<Self>) {
        let bar = &mut self.scrolling.bar;
        if bar.task.is_none() && bar.until.is_some() {
            bar.task = Some(cx.spawn(async move |this, cx| {
                loop {
                    let wait = this
                        .update(cx, |view, _| {
                            let left = view
                                .scrolling
                                .bar
                                .until
                                .map(|t| t.saturating_duration_since(Instant::now()));
                            if left.is_none_or(|l| l.is_zero()) {
                                view.scrolling.bar.until = None;
                            }
                            left.filter(|l| !l.is_zero())
                        })
                        .ok()
                        .flatten();
                    match wait {
                        Some(wait) => cx.background_executor().timer(wait).await,
                        None => break,
                    }
                }
                let _ = this.update(cx, |view, cx| {
                    view.scrolling.bar.task = None;
                    cx.notify();
                });
            }));
        }
    }

    /// The overlay scrollbar, while it shows: when the view moves (by
    /// output too, unless following output hides it), while it's parked
    /// away from the live output with that option, and while its thumb is
    /// held.
    pub(super) fn render_scrollbar(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let now = Instant::now();
        let view = self.view();
        let extent = view.extent;
        let s = &self.scrolling;
        let pos = view.offset as f32 + s.motion.delta(now);
        let away = view.offset as f32 + s.motion.held() != 0.;
        let shown = if s.options.hide_scrollbar_following {
            away
        } else {
            s.bar.until.is_some_and(|t| now < t)
        };
        let dragging = s.bar.grab.is_some();
        if extent.alt_screen || (extent.history == 0 && !away) || !(shown || dragging) {
            return None;
        }
        // The frame that saw the view move can't start the hide timer.
        self.hide_scrollbar_later(cx);
        let history = extent.history as f32;
        let total = history + extent.rows as f32;
        let fraction = (extent.rows as f32 / total).min(1.);
        let top = ((history - pos) / total).clamp(0., 1. - fraction);
        let fg = self.style.palette[NamedColor::Foreground as usize];
        let color = motty_render::hsla(fg.with_alpha(if dragging { 0.55 } else { 0.35 }));
        let entity = cx.entity().downgrade();
        Some(
            canvas(
                |_, _, _| {},
                move |track, _, window, _| {
                    let height = (track.size.height * fraction).max(px(THUMB_MIN));
                    let room = track.size.height - height;
                    let y = track.origin.y + room * (top / (1. - fraction).max(f32::EPSILON));
                    let thumb = Bounds::new(
                        point(
                            track.origin.x + track.size.width - px(THUMB_WIDTH + THUMB_INSET),
                            y,
                        ),
                        size(px(THUMB_WIDTH), height),
                    );
                    window.paint_quad(fill(thumb, color).corner_radii(px(THUMB_WIDTH / 2.)));
                    let grip = Bounds::new(
                        point(thumb.origin.x - px(THUMB_INSET), track.origin.y),
                        size(px(THUMB_WIDTH + THUMB_INSET * 2.), track.size.height),
                    );
                    let e = entity.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble
                            && event.button == MouseButton::Left
                            && grip.contains(&event.position)
                        {
                            cx.stop_propagation();
                            let _ = e.update(cx, |view, cx| {
                                view.grab_thumb(event.position.y, track, thumb, cx)
                            });
                        }
                    });
                    // Moves and the release reach the thumb wherever the
                    // pointer is; a press may not have drawn a frame yet.
                    let e = entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture
                            && e.update(cx, |view, cx| {
                                view.drag_thumb(event.position.y, track, height, cx)
                            })
                            .unwrap_or(false)
                        {
                            cx.stop_propagation();
                        }
                    });
                    let e = entity.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture
                            && event.button == MouseButton::Left
                            && e.update(cx, |view, cx| view.release_thumb(cx))
                                .unwrap_or(false)
                        {
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .top_0()
            .right_0()
            .h_full()
            .w(px(THUMB_WIDTH + THUMB_INSET * 2.)),
        )
    }

    /// A press on the thumb grabs it where it is; on the track, it grabs
    /// the middle and jumps there.
    fn grab_thumb(
        &mut self,
        y: Pixels,
        track: Bounds<Pixels>,
        thumb: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let on_thumb = (thumb.top()..thumb.bottom()).contains(&y);
        let grab = if on_thumb {
            y - thumb.top()
        } else {
            thumb.size.height / 2.
        };
        self.scrolling.bar.grab = Some(grab);
        self.scrolling.snap = None;
        if !on_thumb {
            let _ = self.drag_thumb(y, track, thumb.size.height, cx);
        }
        cx.notify();
    }

    fn drag_thumb(
        &mut self,
        y: Pixels,
        track: Bounds<Pixels>,
        height: Pixels,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(grab) = self.scrolling.bar.grab else {
            return false;
        };
        let now = Instant::now();
        let view = self.view();
        let history = view.extent.history;
        let room = f32::from(track.size.height - height).max(1.);
        let top = (f32::from(y - grab - track.top()) / room).clamp(0., 1.);
        // The thumb's top at 0 is the oldest row, at the bottom the live one.
        let pos = history as f32 * (1. - top);
        let whole = !self.scrolling.options.smooth_scroll;
        let offset = self.scrolling.motion.jump(pos, history, whole);
        self.set_offset(view.offset, offset);
        self.show_scrollbar(now, cx);
        cx.notify();
        true
    }

    /// Lets go of the thumb; false when it wasn't held.
    fn release_thumb(&mut self, cx: &mut Context<Self>) -> bool {
        if self.scrolling.bar.grab.take().is_none() {
            return false;
        }
        if self.scrolling.options.smooth_scroll {
            self.snap_scroll(cx);
        }
        self.show_scrollbar(Instant::now(), cx);
        cx.notify();
        true
    }
}
