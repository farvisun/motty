//! Inline suggestions at the prompt: the rest of a command run here before,
//! drawn dim after the cursor. The accept key (Tab by default,
//! `autocomplete-shortcut`) types it; Esc hides it until the line changes.
//! See [`crate::history`] for what's remembered. A snippet's alias offers
//! the snippet instead, which replaces the alias when accepted.

use gpui::{Context, Keystroke, SharedString};
use motty_theme::{AcceptKey, Autocomplete};

use super::TerminalView;

/// The suggestion showing, and the line it was made for.
#[derive(Clone, Debug, Default)]
pub(super) struct Suggestion {
    line: String,
    rest: String,
    /// The snippet the line is the alias of.
    snippet: Option<String>,
}

impl TerminalView {
    pub fn set_autocomplete(&mut self, options: Autocomplete, cx: &mut Context<Self>) {
        self.autocomplete = options;
        self.update_suggestion();
        cx.notify();
    }

    /// Re-reads the prompt line and what to suggest for it. Runs on every
    /// render (output after a wakeup isn't announced again until a frame is
    /// drawn) and before a key acts on the suggestion.
    pub(super) fn update_suggestion(&mut self) {
        let wanted = self.autocomplete.inline
            && self.autocomplete.shortcut != AcceptKey::Disable
            && self.vi.is_none()
            && self.hints.is_none()
            && !self.read_only;
        let line = wanted.then(|| self.session.prompt_input()).flatten();
        // Esc holds until the line changes.
        if self.dismissed.is_some() && line != self.dismissed {
            self.dismissed = None;
        }
        let next = line.and_then(|line| {
            if self.dismissed.as_deref() == Some(line.as_str()) {
                return None;
            }
            if !line.contains(char::is_whitespace)
                && let Some(text) = crate::recipes::snippet_for(&line)
            {
                let (date, time) = crate::recipes::date_time();
                let first = text.lines().next().unwrap_or_default();
                let preview = first
                    .replace("{{cursor}}", "")
                    .replace("{{date}}", &date)
                    .replace("{{time}}", &time);
                return Some(Suggestion {
                    rest: format!("  → {preview}"),
                    line,
                    snippet: Some(text),
                });
            }
            // Typing what the suggestion said keeps it, one character
            // shorter, without another lookup.
            if let Some(s) = &self.suggestion
                && s.snippet.is_none()
                && let Some(typed) = line.strip_prefix(s.line.as_str())
                && let Some(rest) = s.rest.strip_prefix(typed)
                && !rest.is_empty()
            {
                return Some(Suggestion {
                    rest: rest.to_owned(),
                    line,
                    snippet: None,
                });
            }
            let rest = crate::history::suggest(self.cwd.as_deref(), &line)?;
            Some(Suggestion {
                line,
                rest,
                snippet: None,
            })
        });
        self.suggestion = next;
    }

    /// The text to draw after the cursor.
    pub(super) fn ghost_text(&self) -> Option<SharedString> {
        self.suggestion.as_ref().map(|s| s.rest.clone().into())
    }

    /// Types the suggestion's text; false when there's none.
    pub(super) fn accept_suggestion(&mut self, cx: &mut Context<Self>) -> bool {
        self.update_suggestion();
        let Some(s) = self.suggestion.take() else {
            return false;
        };
        let Some(text) = s.snippet else {
            self.write_input(&s.rest, cx);
            return true;
        };
        // The alias goes, the snippet comes in its place.
        let clipboard = if text.contains("{{clipboard}}") {
            cx.read_from_clipboard()
                .and_then(|c| c.text())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let (text, back) =
            crate::recipes::expand_snippet(&text, &clipboard, crate::recipes::date_time());
        let erase = "\x7f".repeat(s.line.chars().count());
        self.write_input(&format!("{erase}{text}{}", "\x1b[D".repeat(back)), cx);
        true
    }

    /// Accepts or dismisses the suggestion for `ks`; true when it did.
    pub(super) fn suggestion_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) -> bool {
        self.update_suggestion();
        let Some(s) = &self.suggestion else {
            return false;
        };
        let m = &ks.modifiers;
        let plain = !(m.platform || m.control || m.alt || m.shift || m.function);
        let accept = match (self.autocomplete.shortcut, ks.key.as_str()) {
            (AcceptKey::Tab | AcceptKey::TabRightArrow, "tab") => plain,
            (AcceptKey::TabRightArrow | AcceptKey::RightArrow, "right") => plain,
            (AcceptKey::CtrlSpace, "space") => m.control && !(m.platform || m.alt || m.shift),
            _ => false,
        };
        if accept {
            return self.accept_suggestion(cx);
        }
        if ks.key == "escape" && plain {
            self.dismissed = Some(s.line.clone());
            self.suggestion = None;
            cx.notify();
            return true;
        }
        false
    }

    /// Remembers the command that just finished, run in `cwd`.
    pub(super) fn record_command(&self, cwd: Option<std::path::PathBuf>, exit: Option<i32>) {
        let input = {
            let shell = self.session.shell();
            shell
                .blocks
                .iter()
                .rev()
                .find(|b| b.end_row.is_some())
                .cloned()
        };
        if let Some(command) = input.and_then(|b| self.session.block_input(&b)) {
            crate::history::record(cwd.as_deref(), &command, exit, &self.autocomplete);
        }
    }
}
