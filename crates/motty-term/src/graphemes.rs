//! Grapheme clustering on top of alacritty's VT handler.
//!
//! alacritty gives every non-zero-width character its own cells, so an emoji
//! ZWJ sequence (👩‍💻) or a skin-tone modifier takes two or more wide
//! cells. Like Otty (and Ghostty's mode 2027), characters that continue a
//! wide grapheme cluster are stored with its first cell, so the cluster
//! renders as one glyph in two cells.

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::cursor_icon::CursorIcon;
use alacritty_terminal::vte::ansi::{
    Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink, KeyboardModes,
    KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, PrivateMode, Rgb,
    ScpCharPath, ScpUpdateMode, StandardCharset, TabulationClearMode,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::repaint::Repaint;

/// The grapheme cluster most recently written at the cursor; anything but
/// more text ends it.
#[derive(Default)]
pub struct Cluster(String);

impl Cluster {
    fn clear(&mut self) {
        self.0.clear();
    }
}

pub struct Graphemes<'a, T: EventListener> {
    pub term: &'a mut Term<T>,
    pub cluster: &'a mut Cluster,
    /// The pointer shape a program asked for (OSC 22), until taken.
    pub pointer: &'a mut Option<CursorIcon>,
    /// In-place redraws, and repaints that leave copies in scrollback.
    pub repaint: &'a mut Repaint,
}

impl<T: EventListener> Graphemes<'_, T> {
    /// Adds `c` to the wide cell just before the cursor, where alacritty
    /// puts zero-width characters. False if that cell isn't wide.
    fn extend_wide_cell(&mut self, c: char) -> bool {
        let grid = self.term.grid_mut();
        let mut column = grid.cursor.point.column;
        if !grid.cursor.input_needs_wrap {
            if column.0 == 0 {
                return false;
            }
            column.0 -= 1;
        }
        let line = grid.cursor.point.line;
        if grid[line][column].flags.contains(Flags::WIDE_CHAR_SPACER) {
            column.0 = column.saturating_sub(1);
        }
        let cell = &mut grid[line][column];
        if !cell.flags.contains(Flags::WIDE_CHAR) {
            return false;
        }
        cell.push_zerowidth(c);
        true
    }
}

impl<T: EventListener> Handler for Graphemes<'_, T> {
    fn input(&mut self, c: char) {
        self.repaint.text();
        if c.is_ascii() {
            self.cluster.0.clear();
            self.cluster.0.push(c);
            return self.term.input(c);
        }
        let continues = !self.cluster.0.is_empty() && {
            self.cluster.0.push(c);
            let single = self.cluster.0.graphemes(true).nth(1).is_none();
            self.cluster.0.pop();
            single
        };
        if continues
            && unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) > 0
            && self.extend_wide_cell(c)
        {
            self.cluster.0.push(c);
            return;
        }
        if !continues {
            self.cluster.0.clear();
        }
        self.cluster.0.push(c);
        self.term.input(c);
    }

    fn set_title(&mut self, a0: Option<String>) {
        self.cluster.clear();
        self.term.set_title(a0);
    }
    fn set_cursor_style(&mut self, a0: Option<CursorStyle>) {
        self.cluster.clear();
        self.term.set_cursor_style(a0);
    }
    fn set_cursor_shape(&mut self, a0: CursorShape) {
        self.cluster.clear();
        self.term.set_cursor_shape(a0);
    }
    fn goto(&mut self, a0: i32, a1: usize) {
        self.cluster.clear();
        let from = self.term.grid().cursor.point.line;
        self.term.goto(a0, a1);
        if self.term.grid().cursor.point.line < from {
            self.repaint.redraw();
        }
    }
    fn goto_line(&mut self, a0: i32) {
        self.cluster.clear();
        let from = self.term.grid().cursor.point.line;
        self.term.goto_line(a0);
        if self.term.grid().cursor.point.line < from {
            self.repaint.redraw();
        }
    }
    fn goto_col(&mut self, a0: usize) {
        self.cluster.clear();
        self.term.goto_col(a0);
    }
    fn insert_blank(&mut self, a0: usize) {
        self.cluster.clear();
        self.term.insert_blank(a0);
    }
    fn move_up(&mut self, a0: usize) {
        self.cluster.clear();
        let from = self.term.grid().cursor.point.line.0;
        self.term.move_up(a0);
        self.repaint.moved_up(self.term, from, a0);
    }
    fn move_down(&mut self, a0: usize) {
        self.cluster.clear();
        self.term.move_down(a0);
    }
    // DA1/DA2 and the pixel size are answered as Motty by `crate::vt`.
    fn identify_terminal(&mut self, _: Option<char>) {
        self.cluster.clear();
    }
    fn device_status(&mut self, a0: usize) {
        self.cluster.clear();
        self.term.device_status(a0);
    }
    fn move_forward(&mut self, a0: usize) {
        self.cluster.clear();
        self.term.move_forward(a0);
    }
    fn move_backward(&mut self, a0: usize) {
        self.cluster.clear();
        self.term.move_backward(a0);
    }
    fn move_down_and_cr(&mut self, a0: usize) {
        self.cluster.clear();
        self.term.move_down_and_cr(a0);
    }
    fn move_up_and_cr(&mut self, a0: usize) {
        self.cluster.clear();
        let from = self.term.grid().cursor.point.line.0;
        self.term.move_up_and_cr(a0);
        self.repaint.moved_up(self.term, from, a0);
    }
    fn put_tab(&mut self, a0: u16) {
        self.cluster.clear();
        self.term.put_tab(a0);
    }
    fn backspace(&mut self) {
        self.cluster.clear();
        self.term.backspace();
    }
    fn carriage_return(&mut self) {
        self.cluster.clear();
        self.term.carriage_return();
    }
    fn linefeed(&mut self) {
        self.cluster.clear();
        self.repaint.text();
        self.term.linefeed();
    }
    fn bell(&mut self) {
        self.cluster.clear();
        self.term.bell();
    }
    fn substitute(&mut self) {
        self.cluster.clear();
        self.term.substitute();
    }
    fn newline(&mut self) {
        self.cluster.clear();
        self.repaint.text();
        self.term.newline();
    }
    fn set_horizontal_tabstop(&mut self) {
        self.cluster.clear();
        self.term.set_horizontal_tabstop();
    }
    fn scroll_up(&mut self, a0: usize) {
        self.cluster.clear();
        self.repaint.reset();
        self.term.scroll_up(a0);
    }
    fn scroll_down(&mut self, a0: usize) {
        self.cluster.clear();
        self.repaint.reset();
        self.term.scroll_down(a0);
    }
    fn insert_blank_lines(&mut self, a0: usize) {
        self.cluster.clear();
        self.repaint.reset();
        self.term.insert_blank_lines(a0);
    }
    fn delete_lines(&mut self, a0: usize) {
        self.cluster.clear();
        self.repaint.reset();
        self.term.delete_lines(a0);
    }
    fn erase_chars(&mut self, a0: usize) {
        self.cluster.clear();
        self.term.erase_chars(a0);
    }
    fn delete_chars(&mut self, a0: usize) {
        self.cluster.clear();
        self.term.delete_chars(a0);
    }
    fn move_backward_tabs(&mut self, a0: u16) {
        self.cluster.clear();
        self.term.move_backward_tabs(a0);
    }
    fn move_forward_tabs(&mut self, a0: u16) {
        self.cluster.clear();
        self.term.move_forward_tabs(a0);
    }
    fn save_cursor_position(&mut self) {
        self.cluster.clear();
        self.term.save_cursor_position();
    }
    fn restore_cursor_position(&mut self) {
        self.cluster.clear();
        self.term.restore_cursor_position();
    }
    fn clear_line(&mut self, a0: LineClearMode) {
        self.cluster.clear();
        self.term.clear_line(a0);
    }
    fn clear_screen(&mut self, a0: ClearMode) {
        self.cluster.clear();
        // Clearing below the cursor is part of a repaint; the rest moves
        // or drops rows.
        if matches!(a0, ClearMode::Below) {
            self.repaint.redraw();
        } else {
            self.repaint.reset();
        }
        self.term.clear_screen(a0);
    }
    fn clear_tabs(&mut self, a0: TabulationClearMode) {
        self.cluster.clear();
        self.term.clear_tabs(a0);
    }
    fn set_tabs(&mut self, a0: u16) {
        self.cluster.clear();
        self.term.set_tabs(a0);
    }
    fn reset_state(&mut self) {
        self.cluster.clear();
        self.repaint.reset();
        self.term.reset_state();
    }
    fn reverse_index(&mut self) {
        self.cluster.clear();
        self.repaint.reset();
        self.term.reverse_index();
    }
    fn terminal_attribute(&mut self, a0: Attr) {
        self.cluster.clear();
        self.term.terminal_attribute(a0);
    }
    fn set_mode(&mut self, a0: Mode) {
        self.cluster.clear();
        self.term.set_mode(a0);
    }
    fn unset_mode(&mut self, a0: Mode) {
        self.cluster.clear();
        self.term.unset_mode(a0);
    }
    fn report_mode(&mut self, a0: Mode) {
        self.cluster.clear();
        self.term.report_mode(a0);
    }
    fn set_private_mode(&mut self, a0: PrivateMode) {
        self.cluster.clear();
        self.term.set_private_mode(a0);
    }
    fn unset_private_mode(&mut self, a0: PrivateMode) {
        self.cluster.clear();
        self.term.unset_private_mode(a0);
    }
    fn report_private_mode(&mut self, a0: PrivateMode) {
        self.cluster.clear();
        self.term.report_private_mode(a0);
    }
    fn set_scrolling_region(&mut self, a0: usize, a1: Option<usize>) {
        self.cluster.clear();
        self.repaint.reset();
        self.term.set_scrolling_region(a0, a1);
    }
    fn set_keypad_application_mode(&mut self) {
        self.cluster.clear();
        self.term.set_keypad_application_mode();
    }
    fn unset_keypad_application_mode(&mut self) {
        self.cluster.clear();
        self.term.unset_keypad_application_mode();
    }
    fn set_active_charset(&mut self, a0: CharsetIndex) {
        self.cluster.clear();
        self.term.set_active_charset(a0);
    }
    fn configure_charset(&mut self, a0: CharsetIndex, a1: StandardCharset) {
        self.cluster.clear();
        self.term.configure_charset(a0, a1);
    }
    fn set_color(&mut self, a0: usize, a1: Rgb) {
        self.cluster.clear();
        self.term.set_color(a0, a1);
    }
    fn dynamic_color_sequence(&mut self, a0: String, a1: usize, a2: &str) {
        self.cluster.clear();
        self.term.dynamic_color_sequence(a0, a1, a2);
    }
    fn reset_color(&mut self, a0: usize) {
        self.cluster.clear();
        self.term.reset_color(a0);
    }
    fn clipboard_store(&mut self, a0: u8, a1: &[u8]) {
        self.cluster.clear();
        self.term.clipboard_store(a0, a1);
    }
    fn clipboard_load(&mut self, a0: u8, a1: &str) {
        self.cluster.clear();
        self.term.clipboard_load(a0, a1);
    }
    fn decaln(&mut self) {
        self.cluster.clear();
        self.term.decaln();
    }
    fn push_title(&mut self) {
        self.cluster.clear();
        self.term.push_title();
    }
    fn pop_title(&mut self) {
        self.cluster.clear();
        self.term.pop_title();
    }
    fn text_area_size_pixels(&mut self) {
        self.cluster.clear();
    }
    fn text_area_size_chars(&mut self) {
        self.cluster.clear();
        self.term.text_area_size_chars();
    }
    fn set_hyperlink(&mut self, a0: Option<Hyperlink>) {
        self.cluster.clear();
        self.term.set_hyperlink(a0);
    }
    fn set_mouse_cursor_icon(&mut self, a0: CursorIcon) {
        self.cluster.clear();
        *self.pointer = Some(a0);
    }
    fn report_keyboard_mode(&mut self) {
        self.cluster.clear();
        self.term.report_keyboard_mode();
    }
    fn push_keyboard_mode(&mut self, a0: KeyboardModes) {
        self.cluster.clear();
        self.term.push_keyboard_mode(a0);
    }
    fn pop_keyboard_modes(&mut self, a0: u16) {
        self.cluster.clear();
        self.term.pop_keyboard_modes(a0);
    }
    fn set_keyboard_mode(&mut self, a0: KeyboardModes, a1: KeyboardModesApplyBehavior) {
        self.cluster.clear();
        self.term.set_keyboard_mode(a0, a1);
    }
    fn set_modify_other_keys(&mut self, a0: ModifyOtherKeys) {
        self.cluster.clear();
        self.term.set_modify_other_keys(a0);
    }
    fn report_modify_other_keys(&mut self) {
        self.cluster.clear();
        self.term.report_modify_other_keys();
    }
    fn set_scp(&mut self, a0: ScpCharPath, a1: ScpUpdateMode) {
        self.cluster.clear();
        self.term.set_scp(a0, a1);
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::index::{Column, Line, Point};
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;
    use crate::GridSize;

    fn render(text: &str) -> Term<VoidListener> {
        let size = GridSize {
            cols: 20,
            rows: 2,
            cell_width: 8.,
            cell_height: 16.,
        };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut cluster = Cluster::default();
        let mut parser: Processor = Processor::new();
        parser.advance(
            &mut Graphemes {
                term: &mut term,
                cluster: &mut cluster,
                pointer: &mut None,
                repaint: &mut Repaint::default(),
            },
            text.as_bytes(),
        );
        term
    }

    fn cursor_column(term: &Term<VoidListener>) -> usize {
        term.grid().cursor.point.column.0
    }

    #[test]
    fn osc_22_sets_the_pointer() {
        let size = GridSize {
            cols: 20,
            rows: 2,
            cell_width: 8.,
            cell_height: 16.,
        };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut pointer = None;
        let mut parser: Processor = Processor::new();
        parser.advance(
            &mut Graphemes {
                term: &mut term,
                cluster: &mut Cluster::default(),
                pointer: &mut pointer,
                repaint: &mut Repaint::default(),
            },
            b"\x1b]22;pointer\x07ab",
        );
        assert_eq!(pointer, Some(CursorIcon::Pointer));
    }

    #[test]
    fn zwj_sequence_takes_one_wide_cell() {
        let term = render("👩\u{200d}💻x");
        let cell = &term.grid()[Point::new(Line(0), Column(0))];
        assert_eq!(cell.c, '👩');
        assert_eq!(cell.zerowidth(), Some(&['\u{200d}', '💻'][..]));
        assert_eq!(term.grid()[Point::new(Line(0), Column(2))].c, 'x');
        assert_eq!(cursor_column(&term), 3);
    }

    #[test]
    fn skin_tone_joins_its_emoji() {
        let term = render("👍🏽");
        assert_eq!(cursor_column(&term), 2);
    }

    #[test]
    fn separate_emoji_stay_separate() {
        let term = render("🚀🚀");
        assert_eq!(cursor_column(&term), 4);
        // Anything between (here a cursor move) ends the cluster.
        let term = render("👩\u{1b}[C\u{200d}💻");
        assert!(cursor_column(&term) > 3);
    }
}
