//! A pane that browses a folder, as Otty's folder pane does: a toolbar
//! (Find, up, Open in Terminal, hidden files, Send to Chat, Share, Close),
//! the path as a breadcrumb, and the folder's tree with Finder's icons.
//!
//! Geometry is measured from Otty's screenshots at 2x. The folders shown
//! are checked for changes every second.

mod icons;
mod toolbar;
mod tree;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Hsla,
    KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, Pixels, Point, ScrollStrategy, Task,
    UniformListScrollHandle, Window, div, img, prelude::*, px, uniform_list,
};
use motty_render::{TerminalStyle, hsla};
use motty_term::alacritty_terminal::vte::ansi::NamedColor;
use motty_theme::Rgba;

use crate::native_menu::{self, Entry as MenuEntry, Item};
use icons::Icons;
use tree::{Listing, Row};

/// A row of the tree, and of the breadcrumb.
const ROW: f32 = 25.;
/// How far each level of the tree is indented.
const INDENT: f32 = 16.;
/// The disclosure triangle's box, then the icon's left edge (rows are
/// inset 2pt for the selection).
const DISCLOSURE: f32 = 12.;
const ICON: f32 = 16.;
const ICON_LEFT: f32 = 19.;
/// The breadcrumb's icons, and the first one's left edge.
const CRUMB_ICON: f32 = 14.;
const CRUMB_LEFT: f32 = 21.;
/// How often the folders shown are checked for changes.
const POLL: Duration = Duration::from_secs(1);

pub enum FolderEvent {
    /// The folder shown changed (the tab's title).
    Changed,
    /// A file was opened (double-click, Return or the menu's Open).
    OpenFile(PathBuf),
    /// Open in Terminal: a terminal starting in this folder.
    OpenTerminal(PathBuf),
}

/// What a row's menu item does.
enum Pick {
    Open,
    OpenWith(String),
    Reveal,
    CopyPath,
    Terminal,
}

pub struct FolderView {
    focus: FocusHandle,
    /// The Find field's.
    find_focus: FocusHandle,
    path: PathBuf,
    style: Rc<TerminalStyle>,
    /// The folders read: the root and the expanded ones shown.
    lists: HashMap<PathBuf, Listing>,
    expanded: HashSet<PathBuf>,
    /// Dot-files are shown.
    hidden: bool,
    /// The Find field's text.
    query: String,
    /// What's on screen, worked out again after every change.
    rows: Rc<Vec<Row>>,
    selected: Option<PathBuf>,
    scroll: UniformListScrollHandle,
    icons: Icons,
    _poll: Task<()>,
}

impl EventEmitter<FolderEvent> for FolderView {}

impl Focusable for FolderView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FolderView {
    pub fn new(path: PathBuf, style: TerminalStyle, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        });
        let mut view = Self {
            focus: cx.focus_handle(),
            find_focus: cx.focus_handle(),
            path,
            style: Rc::new(style),
            lists: HashMap::new(),
            expanded: HashSet::new(),
            hidden: false,
            query: String::new(),
            rows: Rc::default(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            icons: Icons::default(),
            _poll: poll,
        };
        view.rebuild();
        view
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The folder's name, for the tab.
    pub fn title(&self) -> String {
        icons::display_name(&self.path)
    }

    pub fn set_style(&mut self, style: TerminalStyle, cx: &mut Context<Self>) {
        self.style = Rc::new(style);
        cx.notify();
    }

    fn foreground(&self) -> Rgba {
        self.style.palette[NamedColor::Foreground as usize]
    }

    /// Shows `path`, selecting the folder it was reached from when that's
    /// in it (going up).
    fn navigate(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if path == self.path {
            return;
        }
        let from = std::mem::replace(&mut self.path, path);
        self.selected = from
            .ancestors()
            .find(|a| a.parent() == Some(self.path.as_path()))
            .map(Path::to_path_buf);
        self.expanded.clear();
        self.lists.clear();
        self.query.clear();
        self.rebuild();
        self.reveal_selected();
        cx.emit(FolderEvent::Changed);
        cx.notify();
    }

    fn go_up(&mut self, cx: &mut Context<Self>) {
        if let Some(parent) = self.path.parent().map(Path::to_path_buf) {
            self.navigate(parent, cx);
        }
    }

    /// Reads the folders now shown that haven't been, forgets the others,
    /// and works out the rows.
    fn rebuild(&mut self) {
        loop {
            let want = tree::wanted(&self.path, &self.lists, &self.expanded, self.hidden);
            let missing: Vec<PathBuf> = want
                .iter()
                .filter(|d| !self.lists.contains_key(*d))
                .cloned()
                .collect();
            if missing.is_empty() {
                self.lists.retain(|d, _| want.contains(d));
                break;
            }
            for dir in missing {
                let listing = Listing::read(&dir);
                self.lists.insert(dir, listing);
            }
        }
        self.rows = Rc::new(tree::flatten(
            &self.path,
            &self.lists,
            &self.expanded,
            self.hidden,
            &self.query,
        ));
    }

    /// Reads again the folders that changed on disk.
    fn poll(&mut self, cx: &mut Context<Self>) {
        let stale: Vec<PathBuf> = self
            .lists
            .iter()
            .filter(|(dir, listing)| listing.stale(dir))
            .map(|(dir, _)| dir.clone())
            .collect();
        if stale.is_empty() {
            return;
        }
        for dir in stale {
            let listing = Listing::read(&dir);
            self.lists.insert(dir, listing);
        }
        self.rebuild();
        cx.notify();
    }

    fn toggle(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_path_buf());
        }
        self.rebuild();
        cx.notify();
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.rebuild();
        if !self.query.is_empty() && self.selected_ix().is_none() {
            self.selected = self.rows.first().map(|r| r.entry.path.clone());
        }
        self.reveal_selected();
        cx.notify();
    }

    fn selected_ix(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.rows.iter().position(|r| &r.entry.path == selected)
    }

    fn select_ix(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(ix) {
            self.selected = Some(row.entry.path.clone());
            self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn reveal_selected(&self) {
        if let Some(ix) = self.selected_ix() {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
    }

    /// Opens a row: a folder is browsed into, a file is handed on.
    fn open(&mut self, path: PathBuf, dir: bool, cx: &mut Context<Self>) {
        if dir {
            self.navigate(path, cx);
        } else {
            cx.emit(FolderEvent::OpenFile(path));
        }
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let ix = self
            .selected_ix()
            .or(if self.query.is_empty() { None } else { Some(0) });
        if let Some(row) = ix.and_then(|ix| self.rows.get(ix)).cloned() {
            self.open(row.entry.path, row.entry.dir, cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        if self.find_focus.is_focused(window) {
            match ks.key.as_str() {
                "escape" => {
                    self.set_query(String::new(), cx);
                    window.focus(&self.focus, cx);
                }
                "down" if !m.platform => {
                    window.focus(&self.focus, cx);
                    if self.selected_ix().is_none() {
                        self.select_ix(0, cx);
                    }
                }
                "enter" => self.open_selected(cx),
                _ => {
                    let mut query = self.query.clone();
                    if !edit_query(&mut query, ks) {
                        return;
                    }
                    self.set_query(query, cx);
                }
            }
            cx.stop_propagation();
            return;
        }
        if m.control || m.alt {
            return;
        }
        let ix = self.selected_ix();
        let row = ix.and_then(|ix| self.rows.get(ix)).cloned();
        match (ks.key.as_str(), m.platform) {
            ("up", true) => self.go_up(cx),
            ("down", true) | ("enter", false) => self.open_selected(cx),
            ("up", false) => self.select_ix(ix.map_or(0, |ix| ix.saturating_sub(1)), cx),
            ("down", false) => self.select_ix(
                ix.map_or(0, |ix| (ix + 1).min(self.rows.len().saturating_sub(1))),
                cx,
            ),
            ("right", false) => match (row, ix) {
                (Some(row), Some(ix)) if row.entry.dir => {
                    if row.open {
                        if self.rows.get(ix + 1).is_some_and(|r| r.depth > row.depth) {
                            self.select_ix(ix + 1, cx);
                        }
                    } else {
                        self.toggle(&row.entry.path, cx);
                    }
                }
                _ => return,
            },
            ("left", false) => match (row, ix) {
                (Some(row), _) if row.open => self.toggle(&row.entry.path, cx),
                (Some(row), Some(ix)) if row.depth > 0 => {
                    if let Some(parent) = self.rows[..ix].iter().rposition(|r| r.depth < row.depth)
                    {
                        self.select_ix(parent, cx);
                    }
                }
                _ => return,
            },
            ("escape", false) if !self.query.is_empty() => self.set_query(String::new(), cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The native menu for a row, or for the folder shown.
    fn show_menu(
        &mut self,
        path: PathBuf,
        dir: bool,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut entries: Vec<MenuEntry> = Vec::new();
        let mut picks = Vec::new();
        if path != self.path {
            entries.push(Item::new("Open").into());
            picks.push(Pick::Open);
        }
        let apps = crate::workspace::apps_for(&path.to_string_lossy(), &[]);
        if !apps.is_empty() {
            entries.push(MenuEntry::Submenu {
                label: "Open With".into(),
                entries: apps
                    .iter()
                    .map(|(name, _)| Item::new(name.clone()).into())
                    .collect(),
            });
            picks.extend(apps.into_iter().map(|(_, bundle)| Pick::OpenWith(bundle)));
        }
        entries.push(MenuEntry::Separator);
        entries.push(Item::new("Reveal in Finder").into());
        picks.push(Pick::Reveal);
        entries.push(Item::new("Copy Path").into());
        picks.push(Pick::CopyPath);
        entries.push(MenuEntry::Separator);
        entries.push(Item::new("Open in Terminal").into());
        picks.push(Pick::Terminal);

        let this = cx.entity().downgrade();
        native_menu::popup(entries, at, None, window, cx, move |ix, _, cx| {
            let Some(pick) = picks.into_iter().nth(ix) else {
                return;
            };
            let _ = this.update(cx, |this, cx| match pick {
                Pick::Open => this.open(path, dir, cx),
                Pick::OpenWith(bundle) => crate::workspace::open_in(&bundle, &path),
                Pick::Reveal => cx.reveal_path(&path),
                Pick::CopyPath => {
                    cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()))
                }
                Pick::Terminal => {
                    let folder = if dir {
                        path
                    } else {
                        path.parent().map_or(path.clone(), Path::to_path_buf)
                    };
                    cx.emit(FolderEvent::OpenTerminal(folder));
                }
            });
        });
    }

    /// The path as a row of folders, each with its icon; clicking one goes
    /// there.
    fn render_crumbs(&mut self, ink: impl Fn(f32) -> Hsla, cx: &mut Context<Self>) -> AnyElement {
        let mut parts: Vec<PathBuf> = self.path.ancestors().map(Path::to_path_buf).collect();
        parts.reverse();
        let last = parts.len().saturating_sub(1);
        let hover = ink(0.07);
        let mut row = div()
            .flex_none()
            .h(px(ROW))
            .pl(px(CRUMB_LEFT - 4.))
            .pr(px(10.))
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(12.))
            .text_color(ink(0.75));
        for (ix, part) in parts.into_iter().enumerate() {
            let icon = self.icon(&part, true, CRUMB_ICON, ink(0.6));
            let name = icons::display_name(&part);
            row = row.child(
                div()
                    .id(("folder-crumb", ix))
                    .flex_none()
                    .h(px(20.))
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(4.))
                    .hover(move |s| s.bg(hover))
                    .child(icon)
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(part.clone(), cx))),
            );
            if ix < last {
                row = row.child(
                    gpui::svg()
                        .flex_none()
                        .path("icons/chevron-right.svg")
                        .size(px(9.))
                        .mx(px(1.))
                        .text_color(ink(0.5)),
                );
            }
        }
        row.into_any_element()
    }

    /// Finder's icon for `path`, else a plain glyph.
    fn icon(&mut self, path: &Path, dir: bool, size: f32, color: Hsla) -> AnyElement {
        match self.icons.get(path, dir) {
            Some(image) => img(image).flex_none().size(px(size)).into_any_element(),
            None => gpui::svg()
                .flex_none()
                .path(if dir {
                    "icons/folder.svg"
                } else {
                    "icons/file.svg"
                })
                .size(px(size - 2.))
                .text_color(color)
                .into_any_element(),
        }
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let fg = self.foreground();
        let ink = move |a: f32| {
            let mut c = fg;
            c.a = a;
            hsla(c)
        };
        let focused = self.focus.is_focused(window);
        let rows = self.rows.clone();
        rows[range.clone()]
            .iter()
            .zip(range)
            .map(|(row, ix)| {
                let selected = self.selected.as_ref() == Some(&row.entry.path);
                let (bg, text, glyph) = match (selected, focused) {
                    (true, true) => (Some(accent()), gpui::white(), gpui::white()),
                    (true, false) => (Some(ink(0.1)), ink(0.9), ink(0.6)),
                    _ => (None, ink(0.9), ink(0.6)),
                };
                let icon = self.icon(&row.entry.path, row.entry.dir, ICON, ink(0.6));
                let indent = row.depth as f32 * INDENT;
                let path = row.entry.path.clone();
                let dir = row.entry.dir;
                let toggle_path = path.clone();
                let menu_path = path.clone();
                div()
                    .id(("folder-row", ix))
                    .h(px(ROW))
                    .mx(px(2.))
                    .pl(px(indent))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .when_some(bg, |d, bg| d.bg(bg))
                    .child(
                        div()
                            .id(("folder-disclosure", ix))
                            .flex_none()
                            .w(px(DISCLOSURE))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(dir, |d| {
                                d.child(
                                    gpui::svg()
                                        .path(if row.open {
                                            "icons/chevron-down.svg"
                                        } else {
                                            "icons/chevron-right.svg"
                                        })
                                        .size(px(11.))
                                        .text_color(glyph),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        window.focus(&this.focus, cx);
                                        this.toggle(&toggle_path, cx);
                                    }),
                                )
                            }),
                    )
                    .child(div().flex_none().w(px(ICON_LEFT - 2. - DISCLOSURE)))
                    .child(icon)
                    .child(
                        div()
                            .pl(px(4.))
                            .pr(px(8.))
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(13.))
                            .text_color(text)
                            .child(row.entry.name.clone()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            window.focus(&this.focus, cx);
                            this.selected = Some(path.clone());
                            if event.click_count == 2 {
                                this.open(path.clone(), dir, cx);
                            }
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            window.focus(&this.focus, cx);
                            this.selected = Some(menu_path.clone());
                            cx.notify();
                            this.show_menu(menu_path.clone(), dir, event.position, window, cx);
                        }),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

/// The selection's color: the system accent blue.
fn accent() -> Hsla {
    hsla(Rgba::rgb(0x0a84ff))
}

/// Applies a typing key to `query`: text, ⌫, ⌥⌫ (a word). False for keys
/// that aren't editing.
fn edit_query(query: &mut String, ks: &Keystroke) -> bool {
    let m = &ks.modifiers;
    if m.platform || m.control {
        return false;
    }
    match ks.key.as_str() {
        "backspace" if m.alt => {
            let trimmed = query.trim_end_matches(|c: char| !c.is_alphanumeric());
            let word = trimmed.trim_end_matches(char::is_alphanumeric);
            query.truncate(word.len());
        }
        "backspace" => {
            query.pop();
        }
        "enter" | "escape" | "tab" | "up" | "down" | "left" | "right" | "pageup" | "pagedown"
        | "home" | "end" => return false,
        _ => match ks
            .key_char
            .as_deref()
            .filter(|t| !t.chars().any(char::is_control))
        {
            Some(text) => query.push_str(text),
            None => return false,
        },
    }
    true
}

impl Render for FolderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fg = self.foreground();
        let ink = move |a: f32| {
            let mut c = fg;
            c.a = a;
            hsla(c)
        };
        let toolbar = self.render_toolbar(window, cx);
        let crumbs = self.render_crumbs(ink, cx);

        let empty = match self.lists.get(&self.path).map(|l| &l.entries) {
            Some(Err(e)) => Some(e.clone()),
            _ if self.rows.is_empty() && !self.query.is_empty() => Some("No Results".into()),
            _ if self.rows.is_empty() => Some("Empty Folder".into()),
            _ => None,
        };
        let body = match empty {
            Some(text) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .px(px(20.))
                .text_size(px(13.))
                .text_color(ink(0.35))
                .child(text)
                .into_any_element(),
            None => uniform_list(
                "folder-rows",
                self.rows.len(),
                cx.processor(|this, range, window, cx| this.render_rows(range, window, cx)),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element(),
        };

        div()
            .id("folder-view")
            .key_context("FolderView")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(self.style.background))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(toolbar)
            .child(crumbs)
            .child(
                div()
                    .id("folder-tree")
                    .flex_1()
                    .min_h_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.focus(&this.focus, cx);
                            this.selected = None;
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            this.show_menu(this.path.clone(), true, event.position, window, cx);
                        }),
                    )
                    .child(body),
            )
    }
}
