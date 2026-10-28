//! Otty's switches for how a pane behaves: copy on select, trimming and
//! clearing selections, ⇧-arrow selection, Secure Keyboard Entry at
//! password prompts, and the spinner for slow commands
//! (`progress-bar-commands`).

use gpui::{ClipboardItem, Context, Keystroke, Window};
use motty_term::alacritty_terminal::grid::Dimensions;
use motty_term::alacritty_terminal::index::{Column, Line, Point, Side};
use motty_term::alacritty_terminal::selection::{Selection, SelectionType};
use motty_term::alacritty_terminal::term::TermMode;
use motty_term::shell::Progress;
use motty_theme::Behavior;

use super::TerminalView;

impl TerminalView {
    pub fn set_behavior(&mut self, behavior: Behavior, progress_commands: Vec<String>) {
        self.behavior = behavior;
        if self.progress_commands != progress_commands {
            self.progress_commands = progress_commands;
            self.auto_progress = None;
        }
    }

    /// OSC 9;4 progress, else a spinner while a `progress-bar-commands`
    /// command runs. True when it changed.
    pub(super) fn refresh_progress(&mut self) -> bool {
        let (reported, running) = {
            let shell = self.session.shell();
            let running = shell
                .blocks
                .back()
                .filter(|b| b.started.is_some() && b.finished.is_none())
                .cloned();
            (shell.progress, running)
        };
        let auto = running.is_some_and(|block| {
            // The command's text is read once per command.
            match self.auto_progress {
                Some((row, matched)) if row == block.prompt_row => matched,
                _ => {
                    let matched = self.session.block_command(&block).is_some_and(|cmd| {
                        self.progress_commands
                            .iter()
                            .any(|p| motty_theme::matches_prefix(&cmd, p))
                    });
                    self.auto_progress = Some((block.prompt_row, matched));
                    matched
                }
            }
        });
        let progress = match reported {
            // Otty ignores paused.
            Some(Progress::Paused(_)) | Some(Progress::Clear) => None,
            Some(p) => Some(p),
            None => auto.then_some(Progress::Indeterminate),
        };
        let changed = progress != self.progress;
        self.progress = progress;
        changed
    }

    /// The selected text, trailing spaces trimmed if asked.
    pub(super) fn selected_text(&self) -> Option<String> {
        let text = self.session.term.lock().selection_to_string()?;
        if text.is_empty() {
            return None;
        }
        Some(if self.behavior.trim_trailing_spaces {
            trim_trailing(&text)
        } else {
            text
        })
    }

    pub(super) fn copy_on_select(&self, cx: &mut Context<Self>) {
        if self.behavior.copy_on_select
            && let Some(text) = self.selected_text()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Typing drops the selection unless `selection-clear-on-typing` is off.
    pub(super) fn typed(&mut self) {
        self.kb_select = None;
        if self.behavior.clear_selection_on_typing {
            self.session.term.lock().selection = None;
        }
    }

    /// ⇧-arrows grow a selection from the cursor (⌥ for a block); true
    /// when the key was taken.
    pub(super) fn shift_select(&mut self, ks: &Keystroke, cx: &mut Context<Self>) -> bool {
        let m = &ks.modifiers;
        if !self.behavior.shift_arrow_select || !m.shift || m.platform || m.control {
            return false;
        }
        let (dl, dc): (i32, i32) = match ks.key.as_str() {
            "left" => (0, -1),
            "right" => (0, 1),
            "up" => (-1, 0),
            "down" => (1, 0),
            _ => return false,
        };
        let mut term = self.session.term.lock();
        // Full-screen programs keep their keys.
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return false;
        }
        let cols = term.columns() as i32;
        let (top, bottom) = (term.topmost_line().0, term.bottommost_line().0);
        let ty = if m.alt {
            SelectionType::Block
        } else {
            SelectionType::Simple
        };
        // Positions are boundaries between cells: (line, 0..=cols).
        let (anchor, end) = match self.kb_select.filter(|_| term.selection.is_some()) {
            Some(state) => state,
            None => {
                let c = term.grid().cursor.point;
                let at = (c.line.0, c.column.0 as i32);
                (at, at)
            }
        };
        let mut next = (end.0 + dl, end.1 + dc);
        if next.1 < 0 {
            next = if next.0 > top {
                (next.0 - 1, cols)
            } else {
                (next.0, 0)
            };
        } else if next.1 > cols {
            next = if next.0 < bottom {
                (next.0 + 1, 0)
            } else {
                (next.0, cols)
            };
        }
        next.0 = next.0.clamp(top, bottom);
        let boundary = |(line, col): (i32, i32)| {
            if col >= cols {
                (
                    Point::new(Line(line), Column((cols - 1) as usize)),
                    Side::Right,
                )
            } else {
                (Point::new(Line(line), Column(col as usize)), Side::Left)
            }
        };
        let (start, start_side) = boundary(anchor);
        let (point, side) = boundary(next);
        let mut selection = Selection::new(ty, start, start_side);
        selection.update(point, side);
        term.selection = Some(selection);
        drop(term);
        self.kb_select = Some((anchor, next));
        self.copy_on_select(cx);
        cx.notify();
        true
    }

    /// Secure Keyboard Entry follows the focused pane's password prompts.
    pub(super) fn check_secret(&mut self, window: &Window, cx: &mut Context<Self>) {
        let secret = self.behavior.auto_secure_input
            && self.focus_handle.is_focused(window)
            && self.session.reading_secret();
        if secret != self.secret {
            self.secret = secret;
            crate::secure_input::set_auto(secret, cx);
        }
    }

    /// Leaving the pane takes its password prompt's protection with it.
    pub(super) fn release_secret(&mut self, cx: &mut Context<Self>) {
        if self.secret {
            self.secret = false;
            crate::secure_input::set_auto(false, cx);
        }
    }
}

/// Each line without its trailing spaces and tabs.
fn trim_trailing(text: &str) -> String {
    text.split('\n')
        .map(|l| l.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_each_line() {
        assert_eq!(trim_trailing("a  \nb\t\n  c "), "a\nb\n  c");
    }
}
