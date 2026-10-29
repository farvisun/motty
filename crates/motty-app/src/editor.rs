//! A tab that edits a text file, as Otty's New File Editor does: line
//! numbers in a gutter, the cursor's line highlighted, and Save / Discard
//! while there are changes.
//!
//! Text is drawn on a canvas, one shaped line per visible row, so files of
//! any length cost only what's on screen.

mod buffer;
mod prose;

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

use crate::actions::{Copy, Cut, Paste, Redo, Save, SelectAll, Undo};

/// The toolbar above the text.
const TOOLBAR: f32 = 32.;
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
    first: usize,
    lines: Vec<ShapedLine>,
    /// Prose: every line wrapped, and how far below the top each starts.
    wrapped: Vec<WrappedLine>,
    tops: Vec<Pixels>,
    content_height: Pixels,
}

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
    error: Option<String>,
    style: Rc<TerminalStyle>,
    layout: Rc<RefCell<Layout>>,
    /// A text field rather than a file (see `prose`).
    prose: Option<prose::Prose>,
    /// A document's name, for one that isn't a file (a transcript).
    name: Option<String>,
    /// Buttons on the toolbar's right, each sending its action.
    buttons: Vec<(SharedString, Box<dyn gpui::Action>)>,
}

impl EventEmitter<EditorEvent> for FileEditor {}

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
            error: None,
            style: Rc::new(style),
            layout: Rc::default(),
            prose: None,
            name: None,
            buttons: Vec::new(),
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
                self.read_only = !editable;
                self.error = None;
            }
            Err(e) => {
                self.read_only = true;
                self.error = Some(e);
            }
        }
        self.path = Some(path);
    }

    /// `text` to read, not edit, under `name`, with `buttons` on the
    /// toolbar (History's Resume).
    pub fn viewer(
        name: String,
        text: String,
        buttons: Vec<(SharedString, Box<dyn gpui::Action>)>,
        style: TerminalStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut editor = Self::new(None, style, cx);
        editor.buffer = Buffer::from_text(&text);
        editor.saved = text;
        editor.read_only = true;
        editor.name = Some(name);
        editor.buttons = buttons;
        editor
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn title(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.txt".into())
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

    pub fn set_style(&mut self, style: TerminalStyle, cx: &mut Context<Self>) {
        self.style = Rc::new(style);
        cx.notify();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
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

    /// The line `delta` lines away, at the remembered x.
    fn vertical(&mut self, delta: isize, window: &mut Window) -> Pos {
        let goal = match self.goal_x {
            Some(x) => x,
            None => self.x_for(self.cursor, window),
        };
        self.goal_x = Some(goal);
        let last = self.buffer.line_count() as isize - 1;
        let target = self.cursor.line as isize + delta;
        if target < 0 {
            return Pos::default();
        }
        if target > last {
            return self.buffer.end();
        }
        let line = target as usize;
        let shaped = self.shape(line, window);
        Pos::new(line, shaped.closest_index_for_x(goal))
    }

    fn shape(&self, line: usize, window: &mut Window) -> ShapedLine {
        let text = SharedString::from(self.buffer.line(line).to_owned());
        shape(
            &text,
            &self.style.font,
            self.style.font_size,
            hsla(self.foreground()),
            window,
        )
    }

    fn x_for(&self, pos: Pos, window: &mut Window) -> Pixels {
        self.shape(pos.line, window).x_for_index(pos.col)
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
        let y = at.y - layout.bounds.top() + self.scroll.y;
        let line = ((y / lh).floor().max(0.) as usize).min(self.buffer.line_count() - 1);
        let x = at.x - layout.text_left;
        let col = match layout.lines.get(line.wrapping_sub(layout.first)) {
            Some(shaped) => shaped.closest_index_for_x(x),
            None => {
                drop(layout);
                self.shape(line, window).closest_index_for_x(x)
            }
        };
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
        let content = lh * self.buffer.line_count() as f32;
        let max_y = (content - layout.bounds.size.height + lh * 3.).max(px(0.));
        let widest = layout
            .lines
            .iter()
            .map(|l| l.width)
            .fold(px(0.), Pixels::max);
        let max_x = (widest - layout.bounds.size.width + lh * 2.).max(self.scroll.x);
        drop(layout);
        self.scroll.y = (self.scroll.y - delta.y).clamp(px(0.), max_y);
        self.scroll.x = (self.scroll.x - delta.x).clamp(px(0.), max_x.max(px(0.)));
        cx.notify();
    }

    fn foreground(&self) -> Rgba {
        self.style.palette[NamedColor::Foreground as usize]
    }
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
        let dirty = self.is_dirty();
        let button = |id: &'static str, glyph: &'static str, label: &'static str| {
            div()
                .id(id)
                .h(px(24.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(5.))
                .rounded(px(6.))
                .text_size(px(13.))
                .text_color(ink(0.85))
                .hover(move |s| s.bg(ink(0.07)))
                .child(div().text_size(px(12.)).child(glyph))
                .child(label)
        };
        let toolbar = div()
            .flex_none()
            .h(px(TOOLBAR))
            .px(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .text_color(ink(0.5))
                    .when(self.read_only && self.name.is_none(), |d| {
                        d.child("Read Only")
                    })
                    .children(self.error.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .children(
                        self.buttons
                            .iter()
                            .enumerate()
                            .map(|(ix, (label, action))| {
                                let action = action.boxed_clone();
                                button_row(
                                    ("editor-button", ix),
                                    label.clone(),
                                    ink(0.85),
                                    ink(0.07),
                                )
                                .on_click(move |_, window, cx| {
                                    window.dispatch_action(action.boxed_clone(), cx)
                                })
                            }),
                    )
                    .when(dirty && !self.read_only, |d| {
                        d.child(
                            button("save", "✓", "Save")
                                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                        )
                        .child(
                            button("discard", "✕", "Discard")
                                .on_click(cx.listener(|this, _, _, cx| this.discard(cx))),
                        )
                    }),
            );

        let focused = self.focus.is_focused(window);
        let reveal = std::mem::take(&mut self.reveal);
        let paint_state = PaintState {
            lines: self.buffer.clone(),
            cursor: self.cursor,
            selection: self.selection(),
            marked: self.marked.clone(),
            scroll: self.scroll,
            reveal,
            focused,
            style: style.clone(),
            fg: hsla(fg),
            numbers: self.name.is_none(),
            gutter_bg: ink(if dark { 0.04 } else { 0.025 }),
            number: ink(0.38),
            current_line: if dark {
                gpui::hsla(0.6, 0.5, 0.5, 0.12)
            } else {
                gpui::hsla(0.61, 0.7, 0.6, 0.13)
            },
            selection_bg: hsla(style.selection_background),
            caret: accent,
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
            .on_key_down(cx.listener(Self::on_key_down))
            .child(toolbar)
            .child(
                div()
                    .id("file-editor-text")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .cursor_text()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.selecting = false),
                    )
                    .on_scroll(cx.listener(Self::on_scroll))
                    .child(text),
            )
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
    layout: Rc<RefCell<Layout>>,
}

/// Paints the gutter and the visible lines; returns a new scroll offset when
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

    // Keep the cursor in view after it moved.
    let mut scroll = s.scroll;
    let mut moved = false;
    if s.reveal {
        let top = lh * s.cursor.line as f32;
        if top < scroll.y {
            scroll.y = top;
            moved = true;
        } else if top + lh > scroll.y + text_bounds.size.height {
            scroll.y = top + lh - text_bounds.size.height;
            moved = true;
        }
        let line = shape(
            &SharedString::from(s.lines.line(s.cursor.line).to_owned()),
            font,
            font_size,
            s.fg,
            window,
        );
        let x = line.x_for_index(s.cursor.col);
        let width = text_bounds.size.width - lh;
        if x < scroll.x {
            scroll.x = (x - lh * 2.).max(px(0.));
            moved = true;
        } else if x > scroll.x + width {
            scroll.x = x - width;
            moved = true;
        }
    }

    let first = (scroll.y / lh).floor() as usize;
    let visible = (text_bounds.size.height / lh).ceil() as usize + 1;
    let last = (first + visible).min(count);
    let top = text_bounds.top() - scroll.y + lh * first as f32;
    let text_left = text_bounds.left() - scroll.x;

    if s.numbers {
        window.paint_quad(fill(
            Bounds::new(bounds.origin, size(gutter, bounds.size.height)),
            s.gutter_bg,
        ));
    }

    let mut shaped = Vec::with_capacity(last.saturating_sub(first));
    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        for line in first..last {
            let y = top + lh * (line - first) as f32;
            let mut text = s.lines.line(line).to_owned();
            // The IME's composition shows at the cursor.
            let marked_range = (line == s.cursor.line)
                .then_some(s.marked.as_ref())
                .flatten()
                .map(|m| {
                    text.insert_str(s.cursor.col, m);
                    s.cursor.col..s.cursor.col + m.len()
                });
            let runs = runs(&text, font, s.fg, marked_range.clone());
            let layout = window
                .text_system()
                .shape_line(text.into(), font_size, &runs, None);

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
                let start = if line == a.line {
                    layout.x_for_index(a.col)
                } else {
                    px(0.)
                };
                let end = if line == b.line {
                    layout.x_for_index(b.col)
                } else {
                    // The line break shows as a bit of selection.
                    layout.width + digit
                };
                if end > start {
                    window.paint_quad(fill(
                        Bounds::new(point(text_left + start, y), size(end - start, lh)),
                        s.selection_bg,
                    ));
                }
            }

            if s.numbers {
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
                        layout.paint(point(text_left, y), lh, TextAlign::Left, None, window, cx);
                    if s.focused && line == s.cursor.line {
                        let col = s.cursor.col + marked_range.as_ref().map_or(0, |r| r.len());
                        let x = text_left + layout.x_for_index(col);
                        window.paint_quad(fill(
                            Bounds::new(point(x, y + px(2.)), size(px(2.), lh - px(4.))),
                            s.caret,
                        ));
                    }
                },
            );
            shaped.push(layout);
        }
    });

    *s.layout.borrow_mut() = Layout {
        bounds: text_bounds,
        line_height: lh,
        text_left,
        first,
        lines: shaped,
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
    let run = |len, underline: bool| TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: underline.then(|| gpui::UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: false,
        }),
        strikethrough: None,
    };
    match marked {
        Some(r) => [
            (r.start, false),
            (r.len(), true),
            (text.len() - r.end, false),
        ]
        .into_iter()
        .filter(|(len, _)| *len > 0)
        .map(|(len, u)| run(len, u))
        .collect(),
        None if text.is_empty() => Vec::new(),
        None => vec![run(text.len(), false)],
    }
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
        let y = layout.bounds.top() + layout.line_height * self.cursor.line as f32 - self.scroll.y;
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
