//! A tab that edits a text file, as Otty's New File Editor does: line
//! numbers in a gutter, the cursor's line highlighted, and Save / Discard
//! while there are changes.
//!
//! Text is drawn on a canvas, one shaped line per visible row, so files of
//! any length cost only what's on screen.

mod buffer;
mod disk;
pub mod highlight;
mod preview_mode;
mod prose;
mod rows;
mod toolbar;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    App, Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, Font, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, Point, ScrollEvent, ShapedLine, SharedString, TextAlign, TextRun, UTF16Selection,
    Window, WrappedLine, canvas, div, fill, point, prelude::*, px, size,
};
use motty_render::{TerminalStyle, hsla};
use motty_term::alacritty_terminal::vte::ansi::NamedColor;
use motty_theme::Rgba;

use buffer::order;
pub use buffer::{Buffer, Pos};
use motty_theme::EditorPrefs;
use rows::{DisplayRow, Rows};

use crate::actions::{Copy, Cut, Paste, Redo, Save, SelectAll, Undo};

/// The toolbar above the text.
/// Space between the gutter's numbers and the text.
const GUTTER_PAD_LEFT: f32 = 12.;
const GUTTER_PAD_RIGHT: f32 = 7.;
const TEXT_PAD: f32 = 4.;
/// Undo keeps this many steps.
const MAX_UNDO: usize = 500;
/// Files bigger than this open read-only.
const MAX_EDITABLE: u64 = 20 << 20;

pub enum EditorEvent {
    /// The title or the unsaved-changes mark changed.
    Changed,
}

#[derive(Clone)]
struct Snapshot {
    buffer: Buffer,
    cursor: Pos,
}

/// What the canvas measured on its last paint, for the mouse and scrolling.
#[derive(Default)]
struct Layout {
    /// The text area, right of the gutter.
    bounds: Bounds<Pixels>,
    line_height: Pixels,
    /// x of column 0 (after horizontal scrolling).
    text_left: Pixels,
    /// Every line's rows at the width painted.
    rows: Rc<Rows>,
    /// The rows drawn, from `first_row`.
    first_row: usize,
    drawn: Vec<RowLayout>,
    /// The widest row drawn, for horizontal scrolling.
    widest: Pixels,
    /// Prose: every line wrapped, and how far below the top each starts.
    wrapped: Vec<WrappedLine>,
    tops: Vec<Pixels>,
    content_height: Pixels,
}

/// One row on screen: a slice of a line, as drawn.
struct RowLayout {
    line: usize,
    range: std::ops::Range<usize>,
    display: DisplayRow,
    shaped: ShapedLine,
}

/// Rows already worked out for this text, width and tab size.
#[derive(Default)]
struct RowCache {
    key: (u64, Option<usize>, usize),
    rows: Rc<Rows>,
}

/// Colors for some lines' byte ranges (syntax highlighting).
pub type LineColors = std::collections::HashMap<usize, Vec<(std::ops::Range<usize>, Hsla, bool)>>;

pub struct FileEditor {
    focus: FocusHandle,
    path: Option<PathBuf>,
    buffer: Buffer,
    /// The text as last loaded or saved.
    saved: String,
    cursor: Pos,
    /// The other end of the selection.
    anchor: Option<Pos>,
    /// Up/down keep to this x while they pass shorter lines.
    goal_x: Option<Pixels>,
    marked: Option<String>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Typing that continues where this left off joins the last undo step.
    typing_at: Option<Pos>,
    scroll: Point<Pixels>,
    /// Scroll the cursor into view on the next paint.
    reveal: bool,
    selecting: bool,
    read_only: bool,
    /// Whether the file may be edited at all: not too big, readable.
    can_edit: bool,
    error: Option<String>,
    /// Syntax colors, for a file in a known language.
    highlighter: Option<highlight::Highlighter>,
    /// The rendered page of a Markdown, SVG or HTML file.
    preview: Option<preview_mode::Preview>,
    /// ⌘⌥W's soft wrap for this pane, over Settings › Editor's.
    wrap: Option<bool>,
    /// The file's modification time as last read or written.
    mtime: Option<std::time::SystemTime>,
    /// Something else changed the file while it had unsaved edits.
    disk_changed: bool,
    /// Polls the file (see `disk`); only an open pane's editor has one.
    watching: Option<gpui::Task<()>>,
    style: Rc<TerminalStyle>,
    layout: Rc<RefCell<Layout>>,
    /// A text field rather than a file (see `prose`).
    prose: Option<prose::Prose>,
    /// Settings › Editor.
    prefs: EditorPrefs,
    /// Bumped on every change to the text, so rows are worked out again.
    revision: u64,
    rows: Rc<RefCell<RowCache>>,
    /// A preview (Search in Files): read-only until its lock is opened.
    lockable: bool,
    /// The next reveal centers the cursor's line.
    center: bool,
    /// A `.diff` or `.patch`: read-only, with no switch.
    diff: bool,
    /// The language picked from the toolbar (`Some(None)`: plain text),
    /// over the detected one.
    language_override: Option<Option<&'static str>>,
    /// The agent session this file is, shown as its History (see
    /// `preview_mode`).
    transcript: Option<crate::agents::sessions::Session>,
    /// View as › JSONL: a transcript's log as text.
    raw: bool,
    /// What's selected on the History page.
    page_selection: String,
    /// A right-click on the History page to answer with a menu, at this
    /// point of the page.
    page_menu: Option<Point<Pixels>>,
}

impl EventEmitter<EditorEvent> for FileEditor {}

impl Drop for FileEditor {
    fn drop(&mut self) {
        if self.watching.is_some() {
            self.clear_stash();
        }
    }
}

impl Focusable for FileEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FileEditor {
    /// An editor for `path`, or an empty Untitled one.
    pub fn new(path: Option<PathBuf>, style: TerminalStyle, cx: &mut Context<Self>) -> Self {
        let mut editor = Self {
            focus: cx.focus_handle(),
            path: None,
            buffer: Buffer::default(),
            saved: String::new(),
            cursor: Pos::default(),
            anchor: None,
            goal_x: None,
            marked: None,
            undo: Vec::new(),
            redo: Vec::new(),
            typing_at: None,
            scroll: Point::default(),
            reveal: false,
            selecting: false,
            read_only: false,
            can_edit: true,
            error: None,
            highlighter: None,
            preview: None,
            wrap: None,
            mtime: None,
            disk_changed: false,
            watching: None,
            style: Rc::new(style),
            layout: Rc::default(),
            prose: None,
            prefs: EditorPrefs::default(),
            revision: 0,
            rows: Rc::default(),
            lockable: false,
            center: false,
            diff: false,
            language_override: None,
            transcript: None,
            raw: false,
            page_selection: String::new(),
            page_menu: None,
        };
        if let Some(path) = path {
            editor.load(path);
        }
        editor
    }

    fn load(&mut self, path: PathBuf) {
        match read_text(&path) {
            Ok((text, editable)) => {
                self.buffer = Buffer::from_text(&text);
                self.saved = text;
                self.revision += 1;
                // A diff is only read, as Otty shows one.
                self.diff = matches!(
                    crate::file_kind::extension(&path).as_str(),
                    "diff" | "patch"
                );
                // An agent's session log reads as its History, and is only read.
                self.transcript = crate::history_page::is_session_log(&path)
                    .then(|| {
                        let roots = crate::agents::sessions::Roots::current();
                        crate::agents::sessions::at(&path, &roots)
                    })
                    .flatten();
                let log = self.diff || self.transcript.is_some();
                self.read_only = !editable || log;
                self.can_edit = editable && !log;
                self.error = None;
            }
            Err(e) => {
                self.read_only = true;
                self.can_edit = false;
                self.error = Some(e);
            }
        }
        self.mtime = disk::modified(&path);
        let language = match self.language_override {
            Some(chosen) => chosen,
            None => highlight::detect(Some(&path), self.buffer.line(0)),
        };
        self.highlighter = language.map(|l| highlight::Highlighter::new(Some(l)));
        self.path = Some(path);
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn title(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.txt".into())
    }

    /// The first `n` lines with text in them, for the tab's hover card.
    pub fn opening_lines(&self, n: usize) -> Vec<String> {
        (0..self.buffer.line_count())
            .map(|ix| self.buffer.line(ix))
            .filter(|l| !l.trim().is_empty())
            .take(n)
            .map(str::to_owned)
            .collect()
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.text() != self.saved
    }

    /// Where a new terminal opened from here should start.
    pub fn folder(&self) -> Option<PathBuf> {
        self.path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
    }

    /// Locks the file for reading, with a lock on the toolbar to edit it;
    /// a file too big to edit, or unreadable, stays locked.
    pub fn set_lockable(&mut self) {
        self.lockable = !self.read_only;
        self.read_only = true;
    }

    /// Selects columns `cols` of `line` and centers it in view.
    pub fn show_match(
        &mut self,
        line: usize,
        cols: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) {
        let start = self.buffer.clamp(Pos::new(line, cols.start));
        self.anchor = Some(start);
        self.cursor = self.buffer.clamp(Pos::new(line, cols.end));
        self.goal_x = None;
        self.reveal = true;
        self.center = true;
        cx.notify();
    }

    /// Settings › Editor.
    pub fn set_prefs(&mut self, prefs: EditorPrefs, cx: &mut Context<Self>) {
        self.prefs = prefs;
        cx.notify();
    }

    pub fn set_style(&mut self, style: TerminalStyle, cx: &mut Context<Self>) {
        self.style = Rc::new(style);
        cx.notify();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        self.reveal = true;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    // Editing

    fn selection(&self) -> Option<(Pos, Pos)> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| order(anchor, self.cursor))
    }

    fn push_undo(&mut self) {
        self.undo.push(Snapshot {
            buffer: self.buffer.clone(),
            cursor: self.cursor,
        });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Replaces the selection (or inserts at the cursor) with `text`.
    fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let selection = self.selection();
        // A run of typed word characters is one undo step.
        let joins = selection.is_none()
            && self.typing_at == Some(self.cursor)
            && text.chars().count() == 1
            && !text.contains(char::is_whitespace);
        if !joins {
            self.push_undo();
        }
        let (first, removed) = match selection {
            Some((a, b)) => (a.line, b.line - a.line + 1),
            None => (self.cursor.line, 1),
        };
        if let Some(h) = &mut self.highlighter {
            h.edit(first, removed, 1 + text.matches('\n').count());
        }
        if let Some((a, b)) = selection {
            self.buffer.delete(a, b);
            self.cursor = a;
        }
        self.cursor = self.buffer.insert(self.cursor, text);
        self.anchor = None;
        self.goal_x = None;
        self.typing_at = Some(self.cursor);
        self.changed(cx);
    }

    /// Deletes the selection, or from the cursor to `to`.
    fn delete_to(&mut self, to: Pos, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let (a, b) = self.selection().unwrap_or_else(|| order(self.cursor, to));
        if a == b {
            return;
        }
        self.push_undo();
        if let Some(h) = &mut self.highlighter {
            h.edit(a.line, b.line - a.line + 1, 1);
        }
        self.buffer.delete(a, b);
        self.cursor = a;
        self.anchor = None;
        self.goal_x = None;
        self.typing_at = None;
        self.changed(cx);
    }

    fn move_to(&mut self, to: Pos, select: bool, cx: &mut Context<Self>) {
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = self.buffer.clamp(to);
        self.typing_at = None;
        self.reveal = true;
        cx.notify();
    }

    /// The row `delta` rows away, at the remembered x.
    fn vertical(&mut self, delta: isize, window: &mut Window) -> Pos {
        let goal = match self.goal_x {
            Some(x) => x,
            None => self.x_for(self.cursor, window),
        };
        self.goal_x = Some(goal);
        let mut rows = self.layout.borrow().rows.clone();
        // Not painted since the text changed: a row per line will do.
        if rows.lines() != self.buffer.line_count() {
            rows = Rc::new(Rows::new(
                (0..self.buffer.line_count()).map(|i| self.buffer.line(i)),
                self.prefs.tab_size,
                None,
            ));
        }
        let row = rows.row_of(self.cursor.line, self.cursor.col) as isize + delta;
        if row < 0 {
            return Pos::default();
        }
        if row as usize >= rows.total() {
            return self.buffer.end();
        }
        let (line, seg) = rows.locate(row as usize);
        if line >= self.buffer.line_count() {
            return self.buffer.end();
        }
        let range = rows.segment(line, seg, self.buffer.line(line).len());
        let (display, shaped) = self.shape_row(line, range.clone(), window);
        let col = range.start + display.to_source(shaped.closest_index_for_x(goal));
        // The end of a wrapped row is the start of the next.
        let col = if col == range.end && range.end < self.buffer.line(line).len() {
            self.buffer.prev(Pos::new(line, col)).col
        } else {
            col
        };
        Pos::new(line, col)
    }

    /// `range` of `line` as drawn, plain.
    fn shape_row(
        &self,
        line: usize,
        range: std::ops::Range<usize>,
        window: &mut Window,
    ) -> (DisplayRow, ShapedLine) {
        let text = self.buffer.line(line);
        let tab = self.prefs.tab_size;
        let col = rows::columns(&text[..range.start], 0, tab);
        let display = DisplayRow::new(&text[range], col, tab, self.prefs.whitespace);
        let shaped = shape(
            &SharedString::from(display.text.clone()),
            &self.style.font,
            self.style.font_size,
            hsla(self.foreground()),
            window,
        );
        (display, shaped)
    }

    /// `pos`'s x within its row.
    fn x_for(&self, pos: Pos, window: &mut Window) -> Pixels {
        let rows = self.layout.borrow().rows.clone();
        let range = rows.segment(
            pos.line,
            rows.segment_of(pos.line, pos.col),
            self.buffer.line(pos.line).len(),
        );
        let (display, shaped) = self.shape_row(pos.line, range.clone(), window);
        shaped.x_for_index(display.to_display(pos.col.saturating_sub(range.start)))
    }

    fn page_lines(&self) -> isize {
        let layout = self.layout.borrow();
        if layout.line_height <= px(0.) {
            return 20;
        }
        ((layout.bounds.size.height / layout.line_height).floor() as isize - 1).max(1)
    }

    fn indent_unit(&self) -> &'static str {
        let tabs =
            (0..self.buffer.line_count().min(1000)).any(|i| self.buffer.line(i).starts_with('\t'));
        if tabs { "\t" } else { "    " }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        let select = m.shift;
        let cursor = self.cursor;
        let b = &self.buffer;
        let vertical = matches!(ks.key.as_str(), "up" | "down" | "pageup" | "pagedown");
        if !vertical {
            self.goal_x = None;
        }
        match ks.key.as_str() {
            "left" if self.selection().is_some() && !select && !m.platform && !m.alt => {
                let (a, _) = self.selection().unwrap_or((cursor, cursor));
                self.move_to(a, false, cx);
            }
            "right" if self.selection().is_some() && !select && !m.platform && !m.alt => {
                let (_, z) = self.selection().unwrap_or((cursor, cursor));
                self.move_to(z, false, cx);
            }
            "left" => {
                let to = if m.platform {
                    smart_home(b, cursor)
                } else if m.alt {
                    b.word_left(cursor)
                } else {
                    b.prev(cursor)
                };
                self.move_to(to, select, cx);
            }
            "right" => {
                let to = if m.platform {
                    Pos::new(cursor.line, b.line(cursor.line).len())
                } else if m.alt {
                    b.word_right(cursor)
                } else {
                    b.next(cursor)
                };
                self.move_to(to, select, cx);
            }
            "up" if m.platform => self.move_to(Pos::default(), select, cx),
            "down" if m.platform => {
                let end = b.end();
                self.move_to(end, select, cx);
            }
            "up" | "down" | "pageup" | "pagedown" => {
                let page = self.page_lines();
                let delta = match ks.key.as_str() {
                    "up" => -1,
                    "down" => 1,
                    "pageup" => -page,
                    _ => page,
                };
                let to = if self.prose.is_some() {
                    self.prose_vertical(delta, window)
                } else {
                    self.vertical(delta, window)
                };
                self.move_to(to, select, cx);
            }
            "home" => self.move_to(smart_home(b, cursor), select, cx),
            "end" => {
                let end = Pos::new(cursor.line, b.line(cursor.line).len());
                self.move_to(end, select, cx);
            }
            "backspace" => {
                let to = if m.platform {
                    Pos::new(cursor.line, 0)
                } else if m.alt {
                    b.word_left(cursor)
                } else {
                    b.prev(cursor)
                };
                self.delete_to(to, cx);
            }
            "delete" => {
                let to = if m.platform {
                    Pos::new(cursor.line, b.line(cursor.line).len())
                } else if m.alt {
                    b.word_right(cursor)
                } else {
                    b.next(cursor)
                };
                self.delete_to(to, cx);
            }
            "enter" if !m.platform && !m.control => {
                // A new line keeps the indentation.
                let indent =
                    b.line(cursor.line)[..b.indent_end(cursor.line).min(cursor.col)].to_owned();
                self.insert(&format!("\n{indent}"), cx);
            }
            "tab" if !m.platform && !m.control && !m.alt && self.prose.is_none() => {
                if select {
                    self.outdent(cx);
                } else {
                    let unit = self.indent_unit();
                    self.insert(unit, cx);
                }
            }
            "escape" if self.anchor.is_some() => {
                self.anchor = None;
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// ⇧⇥ takes one indent off the cursor's line.
    fn outdent(&mut self, cx: &mut Context<Self>) {
        let line = self.cursor.line;
        let text = self.buffer.line(line);
        let remove = if text.starts_with('\t') {
            1
        } else {
            text.len() - text.trim_start_matches(' ').len()
        }
        .min(4);
        if remove == 0 || self.read_only {
            return;
        }
        self.push_undo();
        self.buffer
            .delete(Pos::new(line, 0), Pos::new(line, remove));
        self.cursor = Pos::new(line, self.cursor.col.saturating_sub(remove));
        self.anchor = None;
        self.changed(cx);
    }

    pub fn selected_text(&self) -> Option<String> {
        if self.transcript.is_some() && self.previewing() {
            return self.page_text();
        }
        let (a, b) = self.selection()?;
        Some(self.buffer.slice(a, b))
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((a, b)) = self.selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.buffer.slice(a, b)));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
        let cursor = self.cursor;
        if self.selection().is_some() {
            self.delete_to(cursor, cx);
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        // A text field takes files and pictures as their paths.
        let files = self
            .prose
            .is_some()
            .then(|| crate::terminal_view::clipboard_files(&item))
            .flatten();
        if let Some(text) = files.or_else(|| item.text()) {
            self.typing_at = None;
            self.insert(&text, cx);
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.anchor = Some(Pos::default());
        self.cursor = self.buffer.end();
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(step) = self.undo.pop() {
            self.redo.push(Snapshot {
                buffer: std::mem::replace(&mut self.buffer, step.buffer),
                cursor: self.cursor,
            });
            self.cursor = self.buffer.clamp(step.cursor);
            self.anchor = None;
            self.typing_at = None;
            self.rehighlight();
            self.changed(cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(step) = self.redo.pop() {
            self.undo.push(Snapshot {
                buffer: std::mem::replace(&mut self.buffer, step.buffer),
                cursor: self.cursor,
            });
            self.cursor = self.buffer.clamp(step.cursor);
            self.anchor = None;
            self.typing_at = None;
            self.rehighlight();
            self.changed(cx);
        }
    }

    // Files

    fn save_action(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        self.save(window, cx);
    }

    /// Writes the file, first asking where for an Untitled one.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.path.clone() {
            self.write(&path, cx);
            return;
        }
        let dir = dirs::home_dir().unwrap_or_default();
        let picked = cx.prompt_for_new_path(&dir, Some("Untitled.txt"));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.write(&path, cx) {
                    this.path = Some(path);
                    cx.emit(EditorEvent::Changed);
                }
            });
        })
        .detach();
    }

    fn write(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let text = self.buffer.text();
        match std::fs::write(path, &text) {
            Ok(()) => {
                self.saved = text;
                self.mtime = disk::modified(path);
                self.disk_changed = false;
                self.error = None;
                cx.emit(EditorEvent::Changed);
                cx.notify();
                true
            }
            Err(e) => {
                self.error = Some(format!("Couldn't save {}: {e}", path.display()));
                cx.notify();
                false
            }
        }
    }

    /// Back to the text as last saved.
    pub fn discard(&mut self, cx: &mut Context<Self>) {
        self.push_undo();
        self.buffer = Buffer::from_text(&self.saved);
        self.revision += 1;
        self.rehighlight();
        self.cursor = self.buffer.clamp(self.cursor);
        self.anchor = None;
        self.changed(cx);
    }

    // Mouse

    fn pos_at(&self, at: Point<Pixels>, window: &mut Window) -> Pos {
        if self.prose.is_some() {
            return self.prose_pos_at(at);
        }
        let layout = self.layout.borrow();
        let lh = layout.line_height.max(px(1.));
        let rows = layout.rows.clone();
        let y = at.y - layout.bounds.top() + self.scroll.y;
        let x = at.x - layout.text_left;
        if rows.lines() != self.buffer.line_count() || rows.total() == 0 {
            let line = ((y / lh).floor().max(0.) as usize).min(self.buffer.line_count() - 1);
            return Pos::new(line, 0);
        }
        let row = ((y / lh).floor().max(0.) as usize).min(rows.total() - 1);
        if let Some(r) = layout.drawn.get(row.wrapping_sub(layout.first_row)) {
            return self.row_pos(r.line, &r.range, &r.display, &r.shaped, x);
        }
        drop(layout);
        let (line, seg) = rows.locate(row);
        let range = rows.segment(line, seg, self.buffer.line(line).len());
        let (display, shaped) = self.shape_row(line, range.clone(), window);
        self.row_pos(line, &range, &display, &shaped, x)
    }

    /// The position at `x` on a row; past a wrapped row's end it stays on
    /// that row.
    fn row_pos(
        &self,
        line: usize,
        range: &std::ops::Range<usize>,
        display: &DisplayRow,
        shaped: &ShapedLine,
        x: Pixels,
    ) -> Pos {
        let col = range.start + display.to_source(shaped.closest_index_for_x(x));
        if col == range.end && range.end < self.buffer.line(line).len() {
            return self.buffer.prev(Pos::new(line, col));
        }
        Pos::new(line, col)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let pos = self.pos_at(event.position, window);
        self.goal_x = None;
        self.typing_at = None;
        match event.click_count {
            2 => {
                let (a, b) = self.buffer.word_at(pos);
                self.anchor = Some(a);
                self.cursor = b;
            }
            3 => {
                self.anchor = Some(Pos::new(pos.line, 0));
                self.cursor = if pos.line + 1 < self.buffer.line_count() {
                    Pos::new(pos.line + 1, 0)
                } else {
                    Pos::new(pos.line, self.buffer.line(pos.line).len())
                };
            }
            _ => {
                if event.modifiers.shift {
                    self.anchor.get_or_insert(self.cursor);
                } else {
                    self.anchor = Some(pos);
                }
                self.cursor = pos;
                self.selecting = true;
            }
        }
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            return;
        }
        self.cursor = self.pos_at(event.position, window);
        self.reveal = true;
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.prose.is_some() {
            return self.prose_scroll(event, cx);
        }
        let layout = self.layout.borrow();
        let lh = layout.line_height.max(px(1.));
        let delta = event.delta.pixel_delta(lh);
        let content = lh * layout.rows.total().max(1) as f32;
        let height = layout.bounds.size.height;
        // `editor-scroll-past-end`: the last line can reach the top.
        let max_y = if self.prefs.scroll_past_end {
            content - lh
        } else {
            content - height + lh / 2.
        }
        .max(px(0.));
        let max_x = if self.wraps() {
            px(0.)
        } else {
            (layout.widest - layout.bounds.size.width + lh * 2.).max(self.scroll.x)
        };
        drop(layout);
        self.scroll.y = (self.scroll.y - delta.y).clamp(px(0.), max_y);
        self.scroll.x = (self.scroll.x - delta.x).clamp(px(0.), max_x.max(px(0.)));
        cx.notify();
    }

    /// Whether long lines wrap: ⌘⌥W's choice, else Settings › Editor's.
    fn wraps(&self) -> bool {
        self.wrap.unwrap_or(self.prefs.line_wrap)
    }

    /// Colors everything again, after the whole text was replaced.
    fn rehighlight(&mut self) {
        if let Some(h) = &mut self.highlighter {
            h.invalidate_from(0);
        }
    }

    /// The language the file is colored as.
    pub fn language(&self) -> Option<&'static str> {
        self.highlighter.as_ref().and_then(|h| h.language())
    }

    /// The toolbar's language dropdown: colors the file as `language`
    /// (`Some(None)`: plain text), or as detected again with `None`.
    pub fn set_language(&mut self, language: Option<Option<&'static str>>, cx: &mut Context<Self>) {
        self.language_override = language;
        let language = language
            .unwrap_or_else(|| highlight::detect(self.path.as_deref(), self.buffer.line(0)));
        self.highlighter = language.map(|l| highlight::Highlighter::new(Some(l)));
        cx.notify();
    }

    /// Syntax colors for the lines about to be drawn: those the last paint
    /// showed, with a screen's margin either side for scrolling.
    fn line_colors(&mut self) -> LineColors {
        let Some(h) = &mut self.highlighter else {
            return LineColors::default();
        };
        let count = self.buffer.line_count();
        let (first, last) = {
            let layout = self.layout.borrow();
            let lh = layout.line_height.max(px(1.));
            let screen = ((layout.bounds.size.height / lh).ceil() as usize).max(60);
            let top = (self.scroll.y / lh).max(0.) as usize;
            let row = |r: usize| {
                if layout.rows.lines() == count {
                    layout
                        .rows
                        .locate(r.min(layout.rows.total().saturating_sub(1)))
                        .0
                } else {
                    r
                }
            };
            (row(top.saturating_sub(screen)), row(top + screen * 2) + 1)
        };
        let (first, last) = (first.min(count), last.min(count));
        let spans = h.highlight(|i| self.buffer.line(i), first..last);
        let style = &self.style;
        spans
            .into_iter()
            .enumerate()
            .filter_map(|(ix, spans)| {
                let colored: Vec<_> = spans
                    .into_iter()
                    .filter_map(|span| {
                        let (color, italic) = highlight::token_color(span.token, style)?;
                        Some((span.range, hsla(color), italic))
                    })
                    .collect();
                (!colored.is_empty()).then_some((first + ix, colored))
            })
            .collect()
    }

    fn foreground(&self) -> Rgba {
        self.style.palette[NamedColor::Foreground as usize]
    }
}

/// Whether `path` has a rendered Preview (Markdown, SVG, HTML).
pub fn previews(path: &Path) -> bool {
    preview_mode::Kind::of(path).is_some()
}

/// A toolbar button with a label only.
fn button_row(
    id: impl Into<gpui::ElementId>,
    label: SharedString,
    text: Hsla,
    hover: Hsla,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(24.))
        .px(px(8.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .text_size(px(13.))
        .text_color(text)
        .hover(move |s| s.bg(hover))
        .child(label)
}

/// ⌘← goes to the text after the indentation, then to column 0.
fn smart_home(b: &Buffer, cursor: Pos) -> Pos {
    let indent = b.indent_end(cursor.line);
    Pos::new(cursor.line, if cursor.col == indent { 0 } else { indent })
}

fn shape(
    text: &SharedString,
    font: &Font,
    size: Pixels,
    color: Hsla,
    window: &mut Window,
) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.clone(), size, &[run], None)
}

/// The file's text, and whether it may be edited; binary files are refused.
fn read_text(path: &Path) -> Result<(String, bool), String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Couldn't open {}: {e}", path.display()))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(format!("{} isn't a text file", path.display()));
    }
    let editable = bytes.len() as u64 <= MAX_EDITABLE;
    Ok((String::from_utf8_lossy(&bytes).into_owned(), editable))
}

impl Render for FileEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.prose.is_some() {
            return self.render_prose(window, cx);
        }
        let style = self.style.clone();
        let fg = self.foreground();
        let dark = style.dark;
        let ink = |a: f32| {
            let mut c = fg;
            c.a = a;
            hsla(c)
        };
        let accent = hsla(Rgba::rgb(0x047afe));
        let toolbar = self.render_toolbar(window, cx);
        let previewing = self.previewing();
        let preview = if previewing {
            self.preview_view(cx)
        } else {
            None
        };
        if let Some(at) = self.page_menu.take() {
            cx.defer_in(window, move |this, window, cx| {
                this.show_page_menu(at, window, cx)
            });
        }

        let focused = self.focus.is_focused(window);
        self.sync_page_focus(focused, cx);
        let reveal = std::mem::take(&mut self.reveal);
        let center = reveal && std::mem::take(&mut self.center);
        let paint_state = PaintState {
            lines: self.buffer.clone(),
            cursor: self.cursor,
            selection: self.selection(),
            marked: self.marked.clone(),
            scroll: self.scroll,
            reveal,
            center,
            focused,
            style: style.clone(),
            fg: hsla(fg),
            numbers: self.prefs.line_numbers,
            gutter_bg: ink(if dark { 0.04 } else { 0.025 }),
            number: ink(0.38),
            current_line: if dark {
                gpui::hsla(0.6, 0.5, 0.5, 0.12)
            } else {
                gpui::hsla(0.61, 0.7, 0.6, 0.13)
            },
            selection_bg: hsla(style.selection_background),
            caret: accent,
            faint: ink(0.25),
            prefs: EditorPrefs {
                line_wrap: self.wraps(),
                ..self.prefs
            },
            revision: self.revision,
            rows: self.rows.clone(),
            colors: Rc::new(self.line_colors()),
            layout: self.layout.clone(),
        };
        let entity = cx.entity();
        let focus = self.focus.clone();
        let text = canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let scroll = paint_text(paint_state, bounds, window, cx);
                if let Some(scroll) = scroll {
                    entity.update(cx, |this, cx| {
                        this.scroll = scroll;
                        cx.notify();
                    });
                }
                window.handle_input(&focus, ElementInputHandler::new(bounds, entity.clone()), cx);
            },
        )
        .size_full();

        div()
            .id("file-editor")
            .key_context("Editor")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(style.background))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::save_action))
            .on_action(
                cx.listener(|this, _: &crate::actions::TogglePreview, _, cx| {
                    this.toggle_read_only(cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::actions::ToggleReadOnly, _, cx| {
                    this.toggle_read_only(cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::actions::ToggleSoftWrap, _, cx| {
                    this.wrap = Some(!this.wraps());
                    cx.notify();
                }),
            )
            .on_action(cx.listener(Self::reload_action))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(toolbar)
            .when(self.disk_changed, |d| d.child(self.render_disk_banner(cx)))
            .when_some(preview, |d, view| {
                d.child(div().flex_1().min_h_0().child(view))
            })
            .when(!previewing, |d| {
                d.child(
                    div()
                        .id("file-editor-text")
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .cursor_text()
                        .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                        .when(self.transcript.is_some(), |d| {
                            d.on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, e: &gpui::MouseDownEvent, window, cx| {
                                    this.show_page_menu(e.position, window, cx)
                                }),
                            )
                        })
                        .on_mouse_move(cx.listener(Self::on_mouse_move))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, _, _| this.selecting = false),
                        )
                        .on_scroll(cx.listener(Self::on_scroll))
                        .child(text),
                )
            })
            .into_any_element()
    }
}

struct PaintState {
    lines: Buffer,
    cursor: Pos,
    selection: Option<(Pos, Pos)>,
    marked: Option<String>,
    scroll: Point<Pixels>,
    reveal: bool,
    center: bool,
    focused: bool,
    style: Rc<TerminalStyle>,
    fg: Hsla,
    /// Line numbers in a gutter (not for a transcript).
    numbers: bool,
    gutter_bg: Hsla,
    number: Hsla,
    current_line: Hsla,
    selection_bg: Hsla,
    caret: Hsla,
    /// Whitespace glyphs.
    faint: Hsla,
    prefs: EditorPrefs,
    revision: u64,
    rows: Rc<RefCell<RowCache>>,
    colors: Rc<LineColors>,
    layout: Rc<RefCell<Layout>>,
}

/// Paints the gutter and the visible rows; returns a new scroll offset when
/// the cursor had to be brought into view.
fn paint_text(
    s: PaintState,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Point<Pixels>> {
    let font_size = s.style.font_size;
    let lh = (font_size * 1.6).round();
    let font = &s.style.font;
    let count = s.lines.line_count();
    let digits = count.to_string().len().max(2);
    let digit = shape(&"0".into(), font, font_size, s.number, window).width;
    let gutter = if s.numbers {
        px(GUTTER_PAD_LEFT) + digit * digits as f32 + px(GUTTER_PAD_RIGHT)
    } else {
        px(GUTTER_PAD_LEFT)
    };
    let text_bounds = Bounds::from_corners(
        point(bounds.left() + gutter + px(TEXT_PAD), bounds.top()),
        bounds.bottom_right(),
    );
    let tab = s.prefs.tab_size;
    let wrap = s.prefs.line_wrap;
    let cols = wrap
        .then(|| ((text_bounds.size.width - px(TEXT_PAD)) / digit.max(px(1.))).floor() as usize);
    let rows = {
        let mut cache = s.rows.borrow_mut();
        let key = (s.revision, cols, tab);
        if cache.key != key || cache.rows.lines() != count {
            cache.rows = Rc::new(Rows::new((0..count).map(|i| s.lines.line(i)), tab, cols));
            cache.key = key;
        }
        cache.rows.clone()
    };

    // A row as drawn: its slice of the line in the highlighter's colors,
    // whitespace glyphs faint, the IME's composition underlined.
    let lay_row = |line: usize, range: std::ops::Range<usize>, window: &mut Window| {
        let text = s.lines.line(line);
        let col = rows::columns(&text[..range.start], 0, tab);
        let mut display = DisplayRow::new(&text[range.clone()], col, tab, s.prefs.whitespace);
        let mut overlays: Vec<(std::ops::Range<usize>, Hsla, bool)> = Vec::new();
        for (r, color, italic) in s.colors.get(&line).into_iter().flatten() {
            let (a, b) = (r.start.max(range.start), r.end.min(range.end));
            if a < b {
                let a = display.to_display(a - range.start);
                let b = display.to_display(b - range.start);
                overlays.push((a..b, *color, *italic));
            }
        }
        for g in &display.glyphs {
            overlays.push((g.clone(), s.faint, false));
        }
        let mut underline = None;
        if let Some(m) = &s.marked
            && line == s.cursor.line
            && (range.contains(&s.cursor.col) || s.cursor.col == range.end)
        {
            let at = display.to_display(s.cursor.col - range.start);
            display.text.insert_str(at, m);
            for (r, _, _) in &mut overlays {
                if r.start >= at {
                    r.start += m.len();
                }
                if r.end > at {
                    r.end += m.len();
                }
            }
            underline = Some(at..at + m.len());
        }
        let runs = styled_runs(display.text.len(), font, s.fg, &overlays, underline);
        let shaped =
            window
                .text_system()
                .shape_line(display.text.clone().into(), font_size, &runs, None);
        RowLayout {
            line,
            range,
            display,
            shaped,
        }
    };
    let segment = |line: usize, col: usize| {
        rows.segment(line, rows.segment_of(line, col), s.lines.line(line).len())
    };

    // Keep the cursor in view after it moved.
    let mut scroll = s.scroll;
    if wrap {
        scroll.x = px(0.);
    }
    let mut moved = false;
    let cursor_row = rows.row_of(s.cursor.line, s.cursor.col);
    if s.reveal {
        let top = lh * cursor_row as f32;
        let height = text_bounds.size.height;
        if s.center && (top < scroll.y || top + lh > scroll.y + height) {
            scroll.y = (top - (height - lh) / 2.).max(px(0.));
            moved = true;
        } else if top < scroll.y {
            scroll.y = top;
            moved = true;
        } else if top + lh > scroll.y + height {
            scroll.y = top + lh - height;
            moved = true;
        }
        if !wrap {
            let r = lay_row(s.cursor.line, segment(s.cursor.line, s.cursor.col), window);
            let x = r
                .shaped
                .x_for_index(r.display.to_display(s.cursor.col - r.range.start));
            let width = text_bounds.size.width - lh;
            if x < scroll.x {
                scroll.x = (x - lh * 2.).max(px(0.));
                moved = true;
            } else if x > scroll.x + width {
                scroll.x = x - width;
                moved = true;
            }
        }
    }

    let total = rows.total();
    let first_row = (scroll.y / lh).floor() as usize;
    let visible = (text_bounds.size.height / lh).ceil() as usize + 1;
    let last_row = (first_row + visible).min(total);
    let top = text_bounds.top() - scroll.y + lh * first_row as f32;
    let text_left = text_bounds.left() - scroll.x;

    if s.numbers {
        window.paint_quad(fill(
            Bounds::new(bounds.origin, size(gutter, bounds.size.height)),
            s.gutter_bg,
        ));
    }

    let mut drawn = Vec::with_capacity(last_row.saturating_sub(first_row));
    let mut widest = px(0.);
    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        for row in first_row..last_row {
            let (line, seg) = rows.locate(row);
            let y = top + lh * (row - first_row) as f32;
            let len = s.lines.line(line).len();
            let r = lay_row(line, rows.segment(line, seg, len), window);
            let x_of = |col: usize| {
                let col = col.clamp(r.range.start, r.range.end) - r.range.start;
                r.shaped.x_for_index(r.display.to_display(col))
            };

            if s.numbers && line == s.cursor.line && s.selection.is_none() {
                window.paint_quad(fill(
                    Bounds::new(
                        point(text_bounds.left() - px(TEXT_PAD), y),
                        size(text_bounds.size.width + px(TEXT_PAD), lh),
                    ),
                    s.current_line,
                ));
            }
            if let Some((a, b)) = s.selection
                && line >= a.line
                && line <= b.line
            {
                let from = if line == a.line { a.col } else { 0 };
                let to = if line == b.line { b.col } else { usize::MAX };
                if from <= r.range.end && to >= r.range.start {
                    let start = x_of(from);
                    // The line break shows as a bit of selection.
                    let end = if to > len || (to == usize::MAX && r.range.end == len) {
                        r.shaped.width + digit
                    } else {
                        x_of(to)
                    };
                    if end > start {
                        window.paint_quad(fill(
                            Bounds::new(point(text_left + start, y), size(end - start, lh)),
                            s.selection_bg,
                        ));
                    }
                }
            }

            if s.numbers && seg == 0 {
                let number = shape(
                    &SharedString::from((line + 1).to_string()),
                    font,
                    font_size,
                    s.number,
                    window,
                );
                let _ = number.paint(
                    point(
                        bounds.left() + gutter - px(GUTTER_PAD_RIGHT) - number.width,
                        y,
                    ),
                    lh,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            window.with_content_mask(
                Some(gpui::ContentMask {
                    bounds: text_bounds,
                }),
                |window| {
                    let _ =
                        r.shaped
                            .paint(point(text_left, y), lh, TextAlign::Left, None, window, cx);
                    if s.focused && row == cursor_row {
                        let marked = s.marked.as_ref().map_or(0, String::len);
                        let col = r.display.to_display(s.cursor.col - r.range.start) + marked;
                        let x = text_left + r.shaped.x_for_index(col);
                        window.paint_quad(fill(
                            Bounds::new(point(x, y + px(2.)), size(px(2.), lh - px(4.))),
                            s.caret,
                        ));
                    }
                },
            );
            widest = widest.max(r.shaped.width);
            drawn.push(r);
        }
    });

    *s.layout.borrow_mut() = Layout {
        bounds: text_bounds,
        line_height: lh,
        text_left,
        rows,
        first_row,
        drawn,
        widest,
        ..Layout::default()
    };
    moved.then_some(scroll)
}

fn runs(
    text: &str,
    font: &Font,
    color: Hsla,
    marked: Option<std::ops::Range<usize>>,
) -> Vec<TextRun> {
    styled_runs(text.len(), font, color, &[], marked)
}

/// Runs over `len` bytes in `base`, with `overlays` (later ones win)
/// recoloring and italicizing ranges and `underline` marking one.
fn styled_runs(
    len: usize,
    font: &Font,
    base: Hsla,
    overlays: &[(std::ops::Range<usize>, Hsla, bool)],
    underline: Option<std::ops::Range<usize>>,
) -> Vec<TextRun> {
    let mut cuts = vec![0, len];
    for (r, _, _) in overlays {
        cuts.extend([r.start.min(len), r.end.min(len)]);
    }
    if let Some(u) = &underline {
        cuts.extend([u.start.min(len), u.end.min(len)]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let (color, italic) = overlays
                .iter()
                .rev()
                .find(|(r, _, _)| r.start <= a && b <= r.end)
                .map_or((base, false), |(_, c, i)| (*c, *i));
            let mut font = font.clone();
            if italic {
                font.style = gpui::FontStyle::Italic;
            }
            let underlined = underline
                .as_ref()
                .is_some_and(|u| u.start <= a && b <= u.end);
            TextRun {
                len: b - a,
                font,
                color,
                background_color: None,
                underline: underlined.then(|| gpui::UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                }),
                strikethrough: None,
            }
        })
        .collect()
}

impl EntityInputHandler for FileEditor {
    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self.marked.as_ref().map_or(0, |t| t.encode_utf16().count());
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.marked.as_ref().map(|t| 0..t.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        if !text.is_empty() {
            self.insert(text, cx);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        new_text: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| new_text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if self.prose.is_some() {
            return Some(self.prose_caret());
        }
        let x = self.x_for(self.cursor, window);
        let layout = self.layout.borrow();
        let row = layout.rows.row_of(self.cursor.line, self.cursor.col);
        let y = layout.bounds.top() + layout.line_height * row as f32 - self.scroll.y;
        Some(Bounds::new(
            point(layout.text_left + x, y),
            size(px(2.), layout.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
