//! Vi mode (⌃⇧Space): keys move a cursor through the screen and scrollback
//! instead of reaching the shell; `v`/`V`/⌃V select, `y` copies. Read-only
//! mode blocks input while output keeps streaming. Both show a pill in the
//! titlebar.

use gpui::{ClipboardItem, Context, KeyDownEvent, Window};
use motty_term::alacritty_terminal::grid::{Dimensions, Scroll};
use motty_term::alacritty_terminal::index::{Column, Point, Side};
use motty_term::alacritty_terminal::selection::{Selection, SelectionType};
use motty_term::alacritty_terminal::vi_mode::ViMotion;

use super::TerminalView;

#[derive(Default)]
pub struct ViState {
    /// Repeat count typed so far.
    count: Option<usize>,
    /// `g` was typed; a second one goes to the top.
    pending_g: bool,
    /// The last search went backwards (`?`), so `n` goes up.
    backward: bool,
}

/// What the titlebar pill shows for the focused pane.
#[derive(Clone, Debug, PartialEq)]
pub enum PaneMode {
    Vi { count: Option<usize> },
    ReadOnly,
}

impl TerminalView {
    pub fn mode(&self) -> Option<PaneMode> {
        match (&self.vi, self.read_only) {
            (Some(vi), _) => Some(PaneMode::Vi { count: vi.count }),
            (None, true) => Some(PaneMode::ReadOnly),
            (None, false) => None,
        }
    }

    /// The pill's ×: leaves vi mode, else read-only mode.
    pub fn exit_mode(&mut self, cx: &mut Context<Self>) {
        if self.vi.is_some() {
            self.toggle_vi_mode(cx);
        } else {
            self.set_read_only(false, cx);
        }
    }

    /// Switches to `mode` (from the control socket).
    pub fn set_mode(&mut self, mode: motty_ipc::PaneMode, cx: &mut Context<Self>) {
        let vi = mode == motty_ipc::PaneMode::Vi;
        if vi != self.vi.is_some() {
            self.toggle_vi_mode(cx);
        }
        self.set_read_only(mode == motty_ipc::PaneMode::ReadOnly, cx);
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.read_only = read_only;
        cx.notify();
    }

    pub(super) fn toggle_vi_mode(&mut self, cx: &mut Context<Self>) {
        let mut term = self.session.term.lock();
        term.toggle_vi_mode();
        if self.vi.take().is_none() {
            self.vi = Some(ViState::default());
        } else {
            term.selection = None;
            term.scroll_display(Scroll::Bottom);
        }
        drop(term);
        cx.notify();
    }

    /// Handles a key in vi mode; every key is consumed.
    pub(super) fn vi_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        if m.platform {
            // ⌘ shortcuts (copy, find, …) still work.
            return;
        }
        cx.stop_propagation();
        let Some(vi) = self.vi.as_mut() else {
            return;
        };
        let key = ks.key.as_str();
        let ch = ks.key_char.as_deref().unwrap_or(key);
        // Counts: `0` only continues one (alone it's "line start").
        if !m.control
            && let Some(d) = ch.chars().next().and_then(|c| c.to_digit(10))
            && ch.len() == 1
            && (d != 0 || vi.count.is_some())
        {
            vi.count = Some((vi.count.unwrap_or(0) * 10 + d as usize).min(9999));
            cx.notify();
            return;
        }
        let count = vi.count.take().unwrap_or(1);
        let pending_g = std::mem::take(&mut vi.pending_g);
        let backward = vi.backward;

        let motion = match (key, ch, m.control) {
            ("h" | "left", _, false) => Some(ViMotion::Left),
            ("j" | "down", _, false) => Some(ViMotion::Down),
            ("k" | "up", _, false) => Some(ViMotion::Up),
            ("l" | "right", _, false) => Some(ViMotion::Right),
            (_, "w", false) => Some(ViMotion::SemanticRight),
            (_, "b", false) => Some(ViMotion::SemanticLeft),
            (_, "e", false) => Some(ViMotion::SemanticRightEnd),
            (_, "W", false) => Some(ViMotion::WordRight),
            (_, "B", false) => Some(ViMotion::WordLeft),
            (_, "E", false) => Some(ViMotion::WordRightEnd),
            (_, "0", false) => Some(ViMotion::First),
            (_, "$", false) => Some(ViMotion::Last),
            (_, "^", false) => Some(ViMotion::FirstOccupied),
            (_, "H", false) => Some(ViMotion::High),
            (_, "M", false) => Some(ViMotion::Middle),
            (_, "L", false) => Some(ViMotion::Low),
            (_, "%", false) => Some(ViMotion::Bracket),
            (_, "{", false) => Some(ViMotion::ParagraphUp),
            (_, "}", false) => Some(ViMotion::ParagraphDown),
            _ => None,
        };
        if let Some(motion) = motion {
            let mut term = self.session.term.lock();
            for _ in 0..count {
                term.vi_motion(motion);
            }
            drop(term);
            cx.notify();
            return;
        }

        match (key, ch, m.control) {
            ("escape", ..) | (_, "q", false) => self.toggle_vi_mode(cx),
            (_, "g", false) if pending_g => self.vi_goto(true),
            (_, "g", false) => {
                if let Some(vi) = self.vi.as_mut() {
                    vi.pending_g = true;
                }
            }
            (_, "G", false) => self.vi_goto(false),
            ("u", _, true) => self.vi_page(0.5, count),
            ("d", _, true) => self.vi_page(-0.5, count),
            ("b", _, true) => self.vi_page(1., count),
            ("f", _, true) => self.vi_page(-1., count),
            ("v", _, true) => self.vi_select(SelectionType::Block),
            (_, "v", false) => self.vi_select(SelectionType::Simple),
            (_, "V", false) => self.vi_select(SelectionType::Lines),
            (_, "o", false) => self.vi_swap_selection_ends(),
            (_, "y", false) | ("enter", _, false) => {
                if let Some(text) = self.session.term.lock().selection_to_string() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                self.toggle_vi_mode(cx);
            }
            (_, "/", false) | (_, "?", false) => {
                if let Some(vi) = self.vi.as_mut() {
                    vi.backward = ch == "?";
                }
                self.open_find(window, cx);
            }
            (_, "n", false) => self.vi_search_step(!backward, window, cx),
            (_, "N", false) => self.vi_search_step(backward, window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn vi_goto(&mut self, top: bool) {
        let mut term = self.session.term.lock();
        let line = if top {
            term.topmost_line()
        } else {
            term.bottommost_line()
        };
        term.vi_goto_point(Point::new(line, Column(0)));
    }

    /// Scrolls `pages` of the screen (positive is up), carrying the cursor.
    fn vi_page(&mut self, pages: f32, count: usize) {
        let mut term = self.session.term.lock();
        let lines = (term.screen_lines() as f32 * pages) as i32 * count as i32;
        let cursor = term.vi_mode_cursor.scroll(&*term, lines);
        term.vi_mode_cursor = cursor;
        term.scroll_display(Scroll::Delta(lines));
    }

    /// `v`, `V`, ⌃V: start a selection of that type, switch to it, or
    /// clear it when it's the same.
    fn vi_select(&mut self, ty: SelectionType) {
        let mut term = self.session.term.lock();
        let cursor = term.vi_mode_cursor.point;
        match term.selection.as_ref().map(|s| s.ty) {
            Some(current) if current == ty => term.selection = None,
            Some(_) => {
                if let Some(sel) = term.selection.as_mut() {
                    sel.ty = ty;
                }
            }
            None => {
                let mut sel = Selection::new(ty, cursor, Side::Left);
                sel.update(cursor, Side::Right);
                term.selection = Some(sel);
            }
        }
    }

    /// `o`: the cursor jumps to the selection's other end, which then moves.
    fn vi_swap_selection_ends(&mut self) {
        let mut term = self.session.term.lock();
        let Some(range) = term.selection.as_ref().and_then(|s| s.to_range(&*term)) else {
            return;
        };
        let ty = term
            .selection
            .as_ref()
            .map_or(SelectionType::Simple, |s| s.ty);
        let cursor = term.vi_mode_cursor.point;
        let (anchor, other) = if cursor == range.end {
            (range.end, range.start)
        } else {
            (range.start, range.end)
        };
        let mut sel = Selection::new(ty, anchor, Side::Left);
        sel.update(other, Side::Right);
        term.selection = Some(sel);
        term.vi_goto_point(other);
    }

    /// `n` / `N`: the next match in the find bar, with the cursor on it.
    fn vi_search_step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            return;
        }
        self.find_step(forward, window, cx);
        let target = self.find.as_ref().and_then(|f| {
            let m = f.matches.get(f.current?)?;
            m.to_points(&*self.session.term.lock())
                .map(|(start, _)| start)
        });
        if let Some(point) = target {
            self.session.term.lock().vi_goto_point(point);
        }
    }
}
