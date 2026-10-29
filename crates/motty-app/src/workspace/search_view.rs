//! A search's results in a tab of their own: Find in All Tabs (every
//! pane's scrollback) and Search in Files (the files under a folder, with
//! the selected one previewed beside the list when there's room).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, KeyDownEvent,
    MouseButton, Pixels, ScrollHandle, SharedString, Task, WeakEntity, Window, canvas, div,
    prelude::*, px,
};
use motty_render::{TerminalStyle, hsla};
use motty_term::search::SearchOptions;
use motty_theme::Rgba;

use super::picker::edit_query;
use crate::actions::Paste;
use crate::editor::FileEditor;
use crate::file_search::{self, FileMatch, MAX_MATCHES, Query};
use crate::terminal_view::TerminalView;

/// Rows the list draws; the rest are counted, not shown.
const MAX_ROWS: usize = 800;
/// From this width on, the selected file shows beside the list.
const TWO_COLUMNS: f32 = 700.;
const LIST_WIDTH: f32 = 0.42;
const ROW_HEIGHT: f32 = 24.;
const HEADER_HEIGHT: f32 = 26.;
const FIELD_HEIGHT: f32 = 26.;
const CARET: u32 = 0x047afe;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every tab's scrollback.
    Tabs,
    /// The files under the folder.
    Files,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Query,
    Include,
    Exclude,
}

pub enum SearchEvent {
    /// The tabs' scrollback should be searched (again).
    SearchTabs,
    /// Show this pane on its `index`th match.
    RevealPane(WeakEntity<TerminalView>, usize),
    /// Open the file on the match.
    OpenFile(FileMatch),
    /// The title or count changed.
    Changed,
}

/// A match in a pane's scrollback.
pub struct TabHit {
    /// The tab's title, which heads its group.
    pub title: String,
    /// Tells tabs with the same title apart.
    pub group: usize,
    pub pane: WeakEntity<TerminalView>,
    /// Which of the pane's matches, as its find bar counts them.
    pub index: usize,
    pub line: String,
    /// The match in `line`.
    pub cols: std::ops::Range<usize>,
}

pub struct SearchView {
    focus: FocusHandle,
    scope: Scope,
    /// The folder Search in Files covers, captured when the tab opened.
    folder: Option<PathBuf>,
    query: String,
    include: String,
    exclude: String,
    field: Field,
    options: SearchOptions,
    /// What was last searched for (the title).
    searched: Option<String>,
    tab_hits: Vec<TabHit>,
    file_hits: Vec<FileMatch>,
    files: usize,
    truncated: bool,
    running: bool,
    error: Option<String>,
    selected: Option<usize>,
    scroll: ScrollHandle,
    width: Pixels,
    preview: Option<Entity<FileEditor>>,
    style: TerminalStyle,
    prefs: motty_theme::EditorPrefs,
    cancel: Arc<AtomicBool>,
    _task: Option<Task<()>>,
}

impl EventEmitter<SearchEvent> for SearchView {}

impl Focusable for SearchView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Drop for SearchView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl SearchView {
    pub fn new(
        scope: Scope,
        folder: Option<PathBuf>,
        query: String,
        style: TerminalStyle,
        prefs: motty_theme::EditorPrefs,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            scope,
            folder,
            query,
            include: String::new(),
            exclude: String::new(),
            field: Field::Query,
            options: SearchOptions::default(),
            searched: None,
            tab_hits: Vec::new(),
            file_hits: Vec::new(),
            files: 0,
            truncated: false,
            running: false,
            error: None,
            selected: None,
            scroll: ScrollHandle::new(),
            width: px(0.),
            preview: None,
            style,
            prefs,
            cancel: Arc::default(),
            _task: None,
        }
    }

    pub fn title(&self) -> String {
        match (&self.searched, self.scope) {
            (Some(q), Scope::Tabs) => format!("Find “{q}”"),
            (Some(q), Scope::Files) => format!("Search “{q}”"),
            (None, Scope::Tabs) => "Find in All Tabs".into(),
            (None, Scope::Files) => "Search in Files".into(),
        }
    }

    pub fn folder(&self) -> Option<PathBuf> {
        self.folder.clone()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn options(&self) -> SearchOptions {
        self.options
    }

    /// What the tab card says under the title.
    pub fn summary(&self) -> Option<String> {
        self.searched.as_ref()?;
        Some(if self.running {
            "Searching…".into()
        } else {
            self.count()
        })
    }

    /// The preview, when it holds unsaved edits.
    pub fn dirty_preview(&self, cx: &App) -> Option<Entity<FileEditor>> {
        self.preview.clone().filter(|e| e.read(cx).is_dirty())
    }

    pub fn set_style(
        &mut self,
        style: TerminalStyle,
        prefs: motty_theme::EditorPrefs,
        cx: &mut Context<Self>,
    ) {
        self.style = style.clone();
        self.prefs = prefs;
        if let Some(preview) = &self.preview {
            preview.update(cx, |e, cx| {
                e.set_style(style, cx);
                e.set_prefs(prefs, cx);
            });
        }
        cx.notify();
    }

    /// Runs the search for what's in the fields.
    pub fn run(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.error = None;
        self.selected = None;
        self.truncated = false;
        self.files = 0;
        self.tab_hits.clear();
        self.file_hits.clear();
        self.scroll.scroll_to_item(0);
        if self.query.is_empty() {
            self.searched = None;
            self.running = false;
            cx.emit(SearchEvent::Changed);
            cx.notify();
            return;
        }
        self.searched = Some(self.query.clone());
        match self.scope {
            Scope::Tabs => cx.emit(SearchEvent::SearchTabs),
            Scope::Files => self.search_files(cx),
        }
        cx.emit(SearchEvent::Changed);
        cx.notify();
    }

    /// The tabs' matches, from the workspace.
    pub fn set_tab_hits(&mut self, hits: Vec<TabHit>, truncated: bool, cx: &mut Context<Self>) {
        self.files = {
            let mut groups: Vec<usize> = hits.iter().map(|h| h.group).collect();
            groups.dedup();
            groups.len()
        };
        self.tab_hits = hits;
        self.truncated = truncated;
        self.selected = (!self.tab_hits.is_empty()).then_some(0);
        cx.emit(SearchEvent::Changed);
        cx.notify();
    }

    pub fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.error = error;
        cx.notify();
    }

    fn search_files(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.folder.clone() else {
            self.error = Some("No folder to search".into());
            return;
        };
        let query = Query {
            text: self.query.clone(),
            case_sensitive: self.options.case_sensitive,
            whole_word: self.options.whole_word,
            regex: self.options.regex,
            include: self.include.clone(),
            exclude: self.exclude.clone(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self.running = true;
        let (tx, rx) = async_channel::unbounded();
        std::thread::spawn(move || {
            let done = file_search::search(&folder, &query, &cancel, |found| {
                let _ = tx.send_blocking(Ok(found));
            });
            let _ = tx.send_blocking(Err(done));
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            while let Ok(message) = rx.recv().await {
                let stop = this.update(cx, |this, cx| {
                    match message {
                        Ok(found) => {
                            this.files += 1;
                            this.file_hits.extend(found);
                            if this.selected.is_none() {
                                this.select(0, cx);
                            }
                        }
                        Err(done) => {
                            this.running = false;
                            match done {
                                Ok(truncated) => this.truncated = truncated,
                                Err(e) => this.error = Some(e),
                            }
                            cx.emit(SearchEvent::Changed);
                        }
                    }
                    cx.notify();
                });
                if stop.is_err() {
                    break;
                }
            }
        }));
    }

    fn hit_count(&self) -> usize {
        match self.scope {
            Scope::Tabs => self.tab_hits.len(),
            Scope::Files => self.file_hits.len(),
        }
    }

    /// "128 matches in 14 files", with what was cut off.
    fn count(&self) -> String {
        let n = self.hit_count();
        let what = match self.scope {
            Scope::Tabs => "tab",
            Scope::Files => "file",
        };
        let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
        if n == 0 {
            return "No matches".into();
        }
        let mut count = format!("{} in {}", plural(n, "match"), plural(self.files, what));
        if self.scope == Scope::Files && self.truncated {
            count = format!(
                "Stopped at {MAX_MATCHES} matches, in {}",
                plural(self.files, what)
            );
        }
        if n > MAX_ROWS {
            count.push_str(&format!(" — showing the first {MAX_ROWS}"));
        }
        count
    }

    fn wide(&self) -> bool {
        self.scope == Scope::Files && self.width >= px(TWO_COLUMNS)
    }

    /// Moves the selection to match `ix`; the preview follows.
    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        let ix = ix.min(self.hit_count().min(MAX_ROWS).saturating_sub(1));
        if self.hit_count() == 0 {
            return;
        }
        self.selected = Some(ix);
        self.scroll.scroll_to_item(self.row_of(ix));
        if self.scope == Scope::Files && self.wide() {
            self.preview_match(ix, cx);
        }
        cx.notify();
    }

    /// The list's child for match `ix`: its group headers come before it.
    fn row_of(&self, ix: usize) -> usize {
        let headers = match self.scope {
            Scope::Tabs => {
                let mut groups: Vec<usize> = self.tab_hits[..=ix].iter().map(|h| h.group).collect();
                groups.dedup();
                groups.len()
            }
            Scope::Files => {
                let mut paths: Vec<&Path> = self.file_hits[..=ix]
                    .iter()
                    .map(|h| h.path.as_path())
                    .collect();
                paths.dedup();
                paths.len()
            }
        };
        ix + headers
    }

    /// Shows the match in the preview, unless the preview holds unsaved
    /// edits to another file.
    fn preview_match(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(hit) = self.file_hits.get(ix).cloned() else {
            return;
        };
        let same = self
            .preview
            .as_ref()
            .is_some_and(|e| e.read(cx).path() == Some(hit.path.as_path()));
        if !same {
            if self.dirty_preview(cx).is_some() {
                return;
            }
            let style = self.style.clone();
            let prefs = self.prefs;
            let path = hit.path.clone();
            self.preview = Some(cx.new(|cx| {
                let mut editor = FileEditor::new(Some(path), style, cx);
                editor.set_prefs(prefs, cx);
                editor.set_lockable();
                editor
            }));
        }
        if let Some(preview) = &self.preview {
            preview.update(cx, |e, cx| {
                e.show_match(hit.line, hit.line_cols.clone(), cx)
            });
        }
    }

    /// A click: the preview jumps there when it's showing, else the match
    /// opens.
    fn click(&mut self, ix: usize, count: usize, cx: &mut Context<Self>) {
        self.select(ix, cx);
        match self.scope {
            Scope::Tabs => self.open(ix, cx),
            Scope::Files if count >= 2 || !self.wide() => self.open(ix, cx),
            Scope::Files => {}
        }
    }

    fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.scope {
            Scope::Tabs => {
                if let Some(hit) = self.tab_hits.get(ix) {
                    cx.emit(SearchEvent::RevealPane(hit.pane.clone(), hit.index));
                }
            }
            Scope::Files => {
                if let Some(hit) = self.file_hits.get(ix) {
                    cx.emit(SearchEvent::OpenFile(hit.clone()));
                }
            }
        }
    }

    fn field_mut(&mut self) -> &mut String {
        match self.field {
            Field::Query => &mut self.query,
            Field::Include => &mut self.include,
            Field::Exclude => &mut self.exclude,
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Not keys meant for the preview.
        if !self.focus.is_focused(window) {
            return;
        }
        let ks = &event.keystroke;
        let n = self.hit_count().min(MAX_ROWS);
        let fields = match self.scope {
            Scope::Tabs => 1,
            Scope::Files => 3,
        };
        match ks.key.as_str() {
            "enter" if ks.modifiers.platform => {
                if let Some(ix) = self.selected {
                    self.open(ix, cx);
                }
            }
            "enter" => self.run(cx),
            "tab" => {
                let order = [Field::Query, Field::Include, Field::Exclude];
                let at = order.iter().position(|f| *f == self.field).unwrap_or(0);
                let step = if ks.modifiers.shift { fields - 1 } else { 1 };
                self.field = order[(at + step) % fields];
                cx.notify();
            }
            "up" if n > 0 => {
                let ix = self.selected.map_or(0, |s| s.saturating_sub(1));
                self.select(ix, cx);
            }
            "down" if n > 0 => {
                let ix = self.selected.map_or(0, |s| (s + 1).min(n - 1));
                self.select(ix, cx);
            }
            _ => {
                if !edit_query(self.field_mut(), ks) {
                    return;
                }
                cx.notify();
            }
        }
        cx.stop_propagation();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return;
        };
        let line = text.lines().next().unwrap_or_default().to_owned();
        self.field_mut().push_str(&line);
        cx.notify();
    }
}

/// The colors a search tab draws with, from its terminal's.
struct Ink {
    text: Hsla,
    secondary: Hsla,
    faint: Hsla,
    border: Hsla,
    field: Hsla,
    hover: Hsla,
    selected: Hsla,
    active: Hsla,
    found: Hsla,
}

impl Ink {
    fn new(style: &TerminalStyle) -> Self {
        let base = if style.dark { 1. } else { 0. };
        let ink = |a: f32| {
            hsla(Rgba {
                r: base,
                g: base,
                b: base,
                a,
            })
        };
        Self {
            text: ink(if style.dark { 0.92 } else { 0.88 }),
            secondary: ink(0.55),
            faint: ink(0.38),
            border: ink(if style.dark { 0.12 } else { 0.1 }),
            field: ink(if style.dark { 0.08 } else { 0.05 }),
            hover: ink(0.05),
            selected: hsla(Rgba::rgb(CARET).with_alpha(if style.dark { 0.3 } else { 0.18 })),
            active: hsla(Rgba::rgb(CARET).with_alpha(0.22)),
            found: hsla(Rgba::rgb(0xffd60a).with_alpha(if style.dark { 0.35 } else { 0.55 })),
        }
    }
}

impl SearchView {
    fn render_field(
        &self,
        field: Field,
        placeholder: &'static str,
        ink: &Ink,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let text = match field {
            Field::Query => &self.query,
            Field::Include => &self.include,
            Field::Exclude => &self.exclude,
        };
        let editing = self.field == field;
        div()
            .id(match field {
                Field::Query => "search-query",
                Field::Include => "search-include",
                Field::Exclude => "search-exclude",
            })
            .flex_1()
            .min_w_0()
            .h(px(FIELD_HEIGHT))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(6.))
            .border(px(1.))
            .border_color(if editing { ink.active } else { ink.border })
            .bg(ink.field)
            .text_size(px(12.5))
            .overflow_hidden()
            .whitespace_nowrap()
            .when(field == Field::Query, |d| {
                d.child(
                    gpui::svg()
                        .path("icons/search.svg")
                        .flex_none()
                        .size(px(13.))
                        .text_color(ink.secondary),
                )
            })
            .when(field != Field::Query, |d| {
                d.child(div().flex_none().text_color(ink.faint).child(placeholder))
            })
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .text_color(ink.text)
                    .child(SharedString::from(text.clone()))
                    .when(editing, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .w(px(1.5))
                                .h(px(15.))
                                .ml(px(0.5))
                                .bg(hsla(Rgba::rgb(CARET))),
                        )
                    })
                    .when(text.is_empty() && field == Field::Query, |d| {
                        d.child(div().text_color(ink.faint).child(placeholder))
                    }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.field = field;
                window.focus(&this.focus, cx);
                cx.notify();
            }))
    }

    fn render_toggle(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        flip: fn(&mut SearchOptions),
        ink: &Ink,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .id(id)
            .flex_none()
            .w(px(28.))
            .h(px(FIELD_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .border(px(1.))
            .border_color(ink.border)
            .when(on, |d| d.bg(ink.active))
            .hover(|d| d.bg(ink.hover))
            .text_size(px(12.))
            .text_color(ink.text)
            .child(label)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(cx.listener(move |this, _, _, cx| {
                flip(&mut this.options);
                cx.notify();
            }))
    }

    fn render_list(&self, ink: &Ink, cx: &mut Context<Self>) -> AnyElement {
        let mono = self.style.font.family.clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        let row = |ix: usize, selected: bool, cx: &mut Context<Self>| {
            div()
                .id(("search-hit", ix))
                .flex_none()
                .h(px(ROW_HEIGHT))
                .mx(px(6.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(5.))
                .when(selected, |d| d.bg(ink.selected))
                .when(!selected, |d| d.hover(|d| d.bg(ink.hover)))
                .font_family(mono.clone())
                .text_size(px(12.))
                .text_color(ink.text)
                .on_click(
                    cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                        window.focus(&this.focus, cx);
                        this.click(ix, event.click_count(), cx);
                    }),
                )
        };
        let header = |title: String, detail: Option<String>, first: bool| {
            div()
                .flex_none()
                .h(px(HEADER_HEIGHT))
                .when(!first, |d| d.mt(px(6.)))
                .px(px(16.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(12.))
                .text_color(ink.secondary)
                .child(
                    div()
                        .flex_none()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .children(detail.map(|d| div().min_w_0().truncate().text_color(ink.faint).child(d)))
                .into_any_element()
        };
        match self.scope {
            Scope::Tabs => {
                let mut group = None;
                for (ix, hit) in self.tab_hits.iter().take(MAX_ROWS).enumerate() {
                    if group != Some(hit.group) {
                        rows.push(header(hit.title.clone(), None, group.is_none()));
                        group = Some(hit.group);
                    }
                    rows.push(
                        row(ix, self.selected == Some(ix), cx)
                            .child(highlighted(&hit.line, hit.cols.clone(), ink))
                            .into_any_element(),
                    );
                }
            }
            Scope::Files => {
                let folder = self.folder.clone().unwrap_or_default();
                let mut path: Option<&Path> = None;
                for (ix, hit) in self.file_hits.iter().take(MAX_ROWS).enumerate() {
                    if path != Some(hit.path.as_path()) {
                        let relative = hit.path.strip_prefix(&folder).unwrap_or(&hit.path);
                        let name = relative
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let parent = relative
                            .parent()
                            .map(|p| p.display().to_string())
                            .filter(|p| !p.is_empty());
                        rows.push(header(name, parent, path.is_none()));
                        path = Some(&hit.path);
                    }
                    rows.push(
                        row(ix, self.selected == Some(ix), cx)
                            .child(
                                div()
                                    .flex_none()
                                    .min_w(px(28.))
                                    .text_right()
                                    .text_color(ink.faint)
                                    .child((hit.line + 1).to_string()),
                            )
                            .child(highlighted(&hit.text, hit.cols.clone(), ink))
                            .into_any_element(),
                    );
                }
            }
        }
        let note = match (&self.error, &self.searched) {
            (Some(error), _) => Some(error.clone()),
            (None, None) => Some(match self.scope {
                Scope::Tabs => "Type, then press ↩ to search every tab".to_owned(),
                Scope::Files => "Type, then press ↩ to search the files in this folder".to_owned(),
            }),
            (None, Some(_)) if rows.is_empty() && !self.running => Some("No matches".into()),
            _ => None,
        };
        div()
            .id("search-list")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .children(rows)
            .children(note.map(|note| {
                div()
                    .px(px(16.))
                    .py(px(10.))
                    .text_size(px(12.5))
                    .text_color(ink.secondary)
                    .child(note)
            }))
            .into_any_element()
    }
}

/// A result's line with its match marked.
fn highlighted(text: &str, cols: std::ops::Range<usize>, ink: &Ink) -> impl IntoElement + use<> {
    let cols = cols.start.min(text.len())..cols.end.min(text.len());
    let (cols, text) = if text.is_char_boundary(cols.start) && text.is_char_boundary(cols.end) {
        (cols, text)
    } else {
        (0..0, text)
    };
    div()
        .min_w_0()
        .flex()
        .overflow_hidden()
        .whitespace_nowrap()
        .child(SharedString::from(text[..cols.start].to_owned()))
        .child(
            div()
                .flex_none()
                .rounded(px(2.))
                .bg(ink.found)
                .child(SharedString::from(text[cols.clone()].to_owned())),
        )
        .child(SharedString::from(text[cols.end..].to_owned()))
}

impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ink = Ink::new(&self.style);
        let files = self.scope == Scope::Files;
        let toggles = div()
            .flex_none()
            .flex()
            .gap(px(4.))
            .child(self.render_toggle(
                "search-case",
                "Aa",
                self.options.case_sensitive,
                |o| o.case_sensitive = !o.case_sensitive,
                &ink,
                cx,
            ))
            .child(self.render_toggle(
                "search-word",
                "ab",
                self.options.whole_word,
                |o| o.whole_word = !o.whole_word,
                &ink,
                cx,
            ))
            .child(self.render_toggle(
                "search-regex",
                ".*",
                self.options.regex,
                |o| o.regex = !o.regex,
                &ink,
                cx,
            ));
        let placeholder = if files {
            "Search in files"
        } else {
            "Find in all tabs"
        };
        let mut bar = div()
            .flex_none()
            .px(px(12.))
            .pt(px(10.))
            .pb(px(8.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(self.render_field(Field::Query, placeholder, &ink, cx))
                    .child(toggles),
            );
        if files {
            bar = bar.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(self.render_field(Field::Include, "include", &ink, cx))
                    .child(self.render_field(Field::Exclude, "exclude", &ink, cx)),
            );
        }
        let folder = self.folder.as_deref().map(|f| {
            let home = dirs::home_dir().unwrap_or_default();
            match f.strip_prefix(&home) {
                Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => f.display().to_string(),
            }
        });
        let status = div()
            .flex_none()
            .h(px(22.))
            .px(px(16.))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(11.5))
            .text_color(ink.secondary)
            .when(self.searched.is_some(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .child(if self.running && self.hit_count() == 0 {
                            "Searching…".to_owned()
                        } else {
                            self.count()
                        }),
                )
            })
            .when_some(folder.filter(|_| files), |d, folder| {
                d.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(ink.faint)
                        .child(folder),
                )
            });
        let entity = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                if let Some(this) = entity.upgrade() {
                    this.update(cx, |this, cx| {
                        let was = this.wide();
                        this.width = bounds.size.width;
                        if this.wide() != was {
                            if let Some(ix) = this.selected.filter(|_| this.wide()) {
                                this.preview_match(ix, cx);
                            }
                            cx.notify();
                        }
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let wide = self.wide();
        let list = self.render_list(&ink, cx);
        let body = div()
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .child(measure)
            .child(
                div()
                    .h_full()
                    .min_w_0()
                    .when(wide, |d| d.w(gpui::relative(LIST_WIDTH)).flex_none())
                    .when(!wide, |d| d.flex_1())
                    .child(list),
            )
            .when_some(self.preview.clone().filter(|_| wide), |d, preview| {
                d.child(div().flex_none().w(px(1.)).h_full().bg(ink.border))
                    .child(div().flex_1().min_w_0().h_full().child(preview))
            });
        div()
            .id("search-view")
            .key_context("SearchView")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(self.style.background))
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(Self::paste))
            .child(bar)
            .child(status)
            .child(div().flex_none().h(px(1.)).bg(ink.border))
            .child(body)
    }
}
