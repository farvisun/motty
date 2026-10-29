//! Edit › Copy / Paste as and the other Edit and File items that work on a
//! pane's text: the selection or the screen copied plain, with style or as
//! ANSI sequences; Export Text As; a file pasted Base64-encoded; Insert ›
//! Screenshot; Use Selection for Find and Jump to Selection.

use std::fmt::Write;
use std::path::PathBuf;

use gpui::{AppContext, ClipboardItem, Context, PathPromptOptions, Window};
use motty_render::TerminalStyle;
use motty_term::alacritty_terminal::grid::{Dimensions, Scroll};
use motty_term::alacritty_terminal::index::{Column, Line, Point};
use motty_term::alacritty_terminal::term::cell::Flags;
use motty_term::alacritty_terminal::term::color::Colors;
use motty_term::alacritty_terminal::vte::ansi::{Color, NamedColor};
use motty_term::snapshot::{self, Row};
use motty_theme::Rgba;

use super::{TerminalView, shell_escape};
use crate::actions::*;

/// What a copy takes.
#[derive(Clone, Copy)]
enum Range {
    Selection,
    /// The rows in view.
    Screen,
    /// Scrollback and screen.
    All,
}

/// How a copy puts it on the clipboard.
#[derive(Clone, Copy)]
enum As {
    Plain,
    Styled,
    Ansi,
}

impl TerminalView {
    pub(super) fn register_copy_as<E: gpui::InteractiveElement>(
        &self,
        el: E,
        cx: &mut Context<Self>,
    ) -> E {
        el.on_action(cx.listener(|this, _: &CopySelection, w, cx| {
            this.copy_as(Range::Selection, As::Plain, w, cx)
        }))
        .on_action(cx.listener(|this, _: &CopyWithStyle, w, cx| {
            this.copy_as(Range::Selection, As::Styled, w, cx)
        }))
        .on_action(cx.listener(|this, _: &CopyAsAnsi, w, cx| {
            this.copy_as(Range::Selection, As::Ansi, w, cx)
        }))
        .on_action(
            cx.listener(|this, _: &CopyScreen, w, cx| {
                this.copy_as(Range::Screen, As::Plain, w, cx)
            }),
        )
        .on_action(cx.listener(|this, _: &CopyScreenWithStyle, w, cx| {
            this.copy_as(Range::Screen, As::Styled, w, cx)
        }))
        .on_action(cx.listener(|this, _: &CopyScreenAsAnsi, w, cx| {
            this.copy_as(Range::Screen, As::Ansi, w, cx)
        }))
        .on_action(cx.listener(|this, _: &ExportText, w, cx| this.export_text(Range::All, w, cx)))
        .on_action(cx.listener(|this, _: &ExportSelectedText, w, cx| {
            this.export_text(Range::Selection, w, cx)
        }))
        .on_action(cx.listener(Self::paste_file_base64))
        .on_action(cx.listener(Self::paste_to_composer))
        .on_action(cx.listener(Self::insert_screenshot))
        .on_action(cx.listener(Self::use_selection_for_find))
        .on_action(cx.listener(Self::jump_to_selection))
        .on_action(cx.listener(|this, _: &MarkMode, _, cx| {
            if this.vi.is_none() {
                this.toggle_vi_mode(cx);
            }
        }))
        .on_action(cx.listener(|this, _: &ReloadTerminalView, window, cx| {
            this.session.term.lock().scroll_display(Scroll::Bottom);
            window.refresh();
            cx.notify();
        }))
    }

    /// The rows `range` covers, `None` when there's no selection.
    fn rows_of(&self, range: Range) -> Option<Vec<Row>> {
        let term = self.session.term.lock();
        let grid = term.grid();
        let last = grid.last_column();
        let (start, end) = match range {
            Range::Selection => {
                let r = term.selection.as_ref()?.to_range(&term)?;
                (r.start, r.end)
            }
            Range::Screen => {
                let top = -(grid.display_offset() as i32);
                let bottom = top + term.screen_lines() as i32 - 1;
                (
                    Point::new(Line(top), Column(0)),
                    Point::new(Line(bottom), last),
                )
            }
            Range::All => (
                Point::new(grid.topmost_line(), Column(0)),
                Point::new(grid.bottommost_line(), last),
            ),
        };
        let mut rows = snapshot::rows(&*term, start, end);
        // The blank rows under the last line of output aren't text.
        while rows.last().is_some_and(|r| r.cells.is_empty()) && rows.len() > 1 {
            rows.pop();
        }
        Some(rows)
    }

    fn copy_as(&mut self, range: Range, how: As, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rows) = self.rows_of(range).filter(|r| !r.is_empty()) else {
            return window.play_system_bell();
        };
        let plain = snapshot::plain(&rows);
        match how {
            As::Plain => cx.write_to_clipboard(ClipboardItem::new_string(plain)),
            As::Ansi => cx.write_to_clipboard(ClipboardItem::new_string(snapshot::ansi(&rows))),
            As::Styled => {
                let colors = *self.session.term.lock().colors();
                crate::pasteboard::write_html(&plain, &html(&rows, &self.style, &colors));
            }
        }
    }

    /// File › Export Text As…: the whole scrollback (or the selection) to a
    /// text file.
    fn export_text(&mut self, range: Range, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rows) = self.rows_of(range) else {
            return window.play_system_bell();
        };
        let mut text = snapshot::plain(&rows);
        text.push('\n');
        let dir = self
            .cwd
            .clone()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        let name = format!("{}.txt", sanitize(&self.display_title()));
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn(async move |_, _| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            if let Err(e) = std::fs::write(&path, text) {
                tracing::warn!("couldn't export to {}: {e}", path.display());
            }
        })
        .detach();
    }

    /// Paste File Base64-Encoded…: a file's bytes as one Base64 line, to
    /// carry a binary over a plain text session (`base64 -d > file`).
    fn paste_file_base64(
        &mut self,
        _: &PasteFileBase64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Paste".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(bytes) = paths.first().and_then(|p| std::fs::read(p).ok()) else {
                return;
            };
            let text = base64(&bytes);
            let _ = this.update_in(cx, |this, window, _| this.paste_text(&text, false, window));
        })
        .detach();
    }

    /// Paste and continue in Composer: the clipboard goes to the end of the
    /// Composer's draft instead of the prompt.
    fn paste_to_composer(
        &mut self,
        _: &PasteToComposer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return window.play_system_bell();
        };
        self.open_composer(window, cx);
        if let Some(c) = &self.composer {
            c.editor.update(cx, |e, cx| {
                let mut draft = e.text();
                if !draft.is_empty() && !draft.ends_with(char::is_whitespace) {
                    draft.push(' ');
                }
                draft.push_str(&text);
                e.set_text(&draft, cx);
            });
        }
    }

    /// Insert › Screenshot: an area picked on screen, saved as a picture,
    /// its path typed at the prompt (for a coding agent to read).
    fn insert_screenshot(
        &mut self,
        _: &InsertScreenshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = std::env::temp_dir().join("motty-screenshots");
        let _ = std::fs::create_dir_all(&dir);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let path = dir.join(format!("Screenshot {stamp}.png"));
        let capture = cx.background_spawn({
            let path = path.clone();
            async move {
                std::process::Command::new("/usr/sbin/screencapture")
                    .arg("-i")
                    .arg(&path)
                    .status()
                    .is_ok_and(|s| s.success())
                    && path.exists()
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            if !capture.await {
                return;
            }
            let text = shell_escape(&path.to_string_lossy());
            let _ = this.update_in(cx, |this, window, _| this.paste_text(&text, false, window));
        })
        .detach();
    }

    /// Use Selection for Find: the selection becomes what ⌘G looks for.
    fn use_selection_for_find(
        &mut self,
        _: &UseSelectionForFind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.selected_text().filter(|t| !t.contains('\n')) else {
            return window.play_system_bell();
        };
        match self.find.as_mut() {
            Some(find) => {
                find.query = text;
                self.run_find(true, cx);
            }
            None => self.last_find.0 = text,
        }
        cx.notify();
    }

    /// Jump to Selection: scrolls the selection into view.
    fn jump_to_selection(
        &mut self,
        _: &JumpToSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut term = self.session.term.lock();
        let Some(start) = term
            .selection
            .as_ref()
            .and_then(|s| s.to_range(&term))
            .map(|r| r.start)
        else {
            drop(term);
            return window.play_system_bell();
        };
        let offset = term.grid().display_offset() as i32;
        let lines = term.screen_lines() as i32;
        let top = -offset;
        if start.line.0 < top || start.line.0 >= top + lines {
            // A third of the way down the screen.
            let target = (-start.line.0 + lines / 3).max(0);
            term.scroll_display(Scroll::Delta(target - offset));
        }
        drop(term);
        cx.notify();
    }
}

/// The rows as an HTML `<pre>` in the theme's colors and the pane's font.
fn html(rows: &[Row], style: &TerminalStyle, colors: &Colors) -> String {
    let fg0 = style.resolve(Color::Named(NamedColor::Foreground), colors);
    let bg0 = opaque(style.resolve(Color::Named(NamedColor::Background), colors));
    let mut out = format!(
        "<pre style=\"font-family:'{}',monospace;font-size:{}px;color:{};background-color:{};padding:8px;\">",
        escape(&style.font.family),
        f32::from(style.font_size),
        hex(fg0),
        hex(bg0),
    );
    for (i, row) in rows.iter().enumerate() {
        let mut open: Option<String> = None;
        for cell in &row.cells {
            let (mut fg, mut bg) = (
                style.resolve(cell.fg, colors),
                style.resolve(cell.bg, colors),
            );
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
                bg = opaque(bg);
            }
            let mut css = String::new();
            if fg != fg0 {
                let _ = write!(css, "color:{};", hex(fg));
            }
            if bg != bg0 && bg.a > 0. {
                let _ = write!(css, "background-color:{};", hex(bg));
            }
            if cell.flags.contains(Flags::BOLD) {
                css.push_str("font-weight:bold;");
            }
            if cell.flags.contains(Flags::ITALIC) {
                css.push_str("font-style:italic;");
            }
            if cell.flags.intersects(Flags::ALL_UNDERLINES) {
                css.push_str("text-decoration:underline;");
            } else if cell.flags.contains(Flags::STRIKEOUT) {
                css.push_str("text-decoration:line-through;");
            }
            if open.as_deref() != Some(css.as_str()) {
                if open.as_ref().is_some_and(|o| !o.is_empty()) {
                    out.push_str("</span>");
                }
                if !css.is_empty() {
                    let _ = write!(out, "<span style=\"{css}\">");
                }
                open = Some(css);
            }
            let mut text = String::new();
            text.push(cell.c);
            for &c in cell.zerowidth().unwrap_or_default() {
                text.push(c);
            }
            out.push_str(&escape(&text));
        }
        if open.is_some_and(|o| !o.is_empty()) {
            out.push_str("</span>");
        }
        if !row.wraps && i + 1 < rows.len() {
            out.push('\n');
        }
    }
    out.push_str("</pre>");
    out
}

fn opaque(c: Rgba) -> Rgba {
    Rgba { a: 1., ..c }
}

fn hex(c: Rgba) -> String {
    let v = |x: f32| (x.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}", v(c.r), v(c.g), v(c.b))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A title as a file name.
fn sanitize(title: &str) -> String {
    let name: String = title
        .chars()
        .map(|c| {
            if matches!(c, '/' | ':' | '\0') {
                '-'
            } else {
                c
            }
        })
        .collect();
    match name.trim() {
        "" => "Terminal".to_owned(),
        n => n.to_owned(),
    }
}

/// Standard Base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn file_names_from_titles() {
        assert_eq!(sanitize("~/src: zsh"), "~-src- zsh");
        assert_eq!(sanitize("  "), "Terminal");
    }
}
