//! Find in All Tabs (⇧⌘F): a dialog that searches every pane's scrollback
//! and lists the matches in a tab of their own, or hands the query to the
//! current tab's find bar or to Search in Files.

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, Entity, FocusHandle, KeyDownEvent, MouseButton, Window, div, prelude::*,
    px,
};
use motty_term::alacritty_terminal::grid::Dimensions;
use motty_term::alacritty_terminal::index::{Column, Point};
use motty_term::search::{self, SearchOptions};

use super::picker::{self, Colors, edit_query};
use super::search_view::{Scope, SearchEvent, SearchView, TabHit};
use super::{Content, Tab, Workspace, tab_title, terminal_style};
use crate::actions::Paste;

const WIDTH: f32 = 560.;
/// Matches listed per pane, and in all.
const PER_PANE: usize = 1000;
const TOTAL: usize = 5000;

pub(super) struct GlobalSearch {
    pub focus: FocusHandle,
    pub query: String,
}

impl Workspace {
    pub(super) fn open_global_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Starts from what the focused pane last looked for.
        let query = self
            .focused_pane()
            .map(|v| v.read(cx).last_find_query())
            .unwrap_or_default();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.global = Some(GlobalSearch { focus, query });
        cx.notify();
    }

    fn close_global_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.global = None;
        self.activate(self.active, window, cx);
    }

    /// The dialog's three ways on: every tab, the current one, or files.
    fn finish_global_search(
        &mut self,
        how: Option<Scope>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(g) = self.global.take() else { return };
        match how {
            Some(scope) => {
                let folder = self.active_folder(cx);
                self.open_search_tab(scope, folder, g.query, window, cx);
            }
            None => {
                self.activate(self.active, window, cx);
                if let Some(pane) = self.focused_pane() {
                    pane.update(cx, |v, cx| {
                        v.show_find(&g.query, SearchOptions::default(), window, cx)
                    });
                }
            }
        }
        cx.notify();
    }

    /// View › Search Across Files: the focused pane's folder, empty.
    pub(super) fn search_in_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folder = self.active_folder(cx);
        self.open_search_tab(Scope::Files, folder, String::new(), window, cx);
    }

    /// A results tab, searching for `query` (when there is one) under
    /// `folder` or across the tabs. It lands where new tabs do.
    pub(super) fn open_search_tab(
        &mut self,
        scope: Scope,
        folder: Option<PathBuf>,
        query: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let style = terminal_style(&self.config, &self.theme, cx);
        let prefs = self.config.editor;
        let view = cx.new(|cx| SearchView::new(scope, folder, query, style, prefs, cx));
        self.watch_search(&view, window, cx);
        let ix = self.new_tab_index();
        self.tabs
            .insert(ix, Tab::holding(Content::Search(view.clone())));
        self.activate(ix, window, cx);
        view.update(cx, |v, cx| v.run(cx));
    }

    /// Follows a results tab: searches the tabs for it and opens what it
    /// picks.
    pub(super) fn watch_search(
        &mut self,
        view: &Entity<SearchView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let subscription = cx.subscribe_in(view, window, Self::on_search_event);
        self.pane_subscriptions
            .insert(view.entity_id(), subscription);
    }

    fn on_search_event(
        &mut self,
        view: &Entity<SearchView>,
        event: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::SearchTabs => {
                let (query, options) = {
                    let v = view.read(cx);
                    (v.query().to_owned(), v.options())
                };
                match self.scrollback_hits(&query, options, cx) {
                    Ok((hits, truncated)) => {
                        view.update(cx, |v, cx| v.set_tab_hits(hits, truncated, cx))
                    }
                    Err(e) => view.update(cx, |v, cx| v.set_error(Some(e), cx)),
                }
            }
            SearchEvent::RevealPane(pane, index) => {
                let Some(pane) = pane.upgrade() else { return };
                let (query, options) = {
                    let v = view.read(cx);
                    (v.query().to_owned(), v.options())
                };
                self.reveal_pane(&pane, window, cx);
                pane.update(cx, |v, cx| {
                    v.show_find_at(&query, options, *index, window, cx)
                });
            }
            SearchEvent::OpenFile(hit) => {
                self.open_editor(Some(hit.path.clone()), window, cx);
                if let Some(editor) = self.tabs.get(self.active).and_then(Tab::as_editor) {
                    editor.update(cx, |e, cx| {
                        e.show_match(hit.line, hit.line_cols.clone(), cx)
                    });
                }
            }
            SearchEvent::Changed => {}
        }
        cx.notify();
    }

    /// Every pane's matches, a line with several listed once, grouped by
    /// tab; and whether the list was cut off.
    fn scrollback_hits(
        &self,
        query: &str,
        options: SearchOptions,
        cx: &Context<Self>,
    ) -> Result<(Vec<TabHit>, bool), String> {
        let mut hits = Vec::new();
        for (group, tab) in self.tabs.iter().enumerate() {
            let Some(panes) = tab.panes() else { continue };
            let title = tab_title(tab, cx);
            for pane in panes.terminals() {
                let term = pane.read(cx).session().term.lock();
                let found = search::find_all(&*term, query, options, PER_PANE)?;
                let mut last_line = None;
                for (index, m) in found.iter().enumerate() {
                    let Some((start, stop)) = m.to_points(&*term) else {
                        continue;
                    };
                    if last_line.replace(start.line) == Some(start.line) {
                        continue;
                    }
                    let end = Point::new(start.line, term.grid().last_column());
                    let whole = term.bounds_to_string(Point::new(start.line, Column(0)), end);
                    let line = whole.trim().to_owned();
                    // Cells to characters, then bytes, past the trimmed indent.
                    let indent = whole.chars().count() - whole.trim_start().chars().count();
                    let first = start.column.0.saturating_sub(indent);
                    let last = if stop.line == start.line {
                        (stop.column.0 + 1).saturating_sub(indent)
                    } else {
                        usize::MAX
                    };
                    let byte =
                        |ix: usize| line.char_indices().nth(ix).map_or(line.len(), |(b, _)| b);
                    let cols = byte(first)..byte(last).max(byte(first));
                    hits.push(TabHit {
                        title: title.clone(),
                        group,
                        pane: pane.downgrade(),
                        index,
                        line,
                        cols,
                    });
                    if hits.len() >= TOTAL {
                        return Ok((hits, true));
                    }
                }
            }
        }
        Ok((hits, false))
    }

    fn on_global_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let Some(g) = self.global.as_mut() else {
            return;
        };
        match ks.key.as_str() {
            "escape" => self.close_global_search(window, cx),
            "enter" if !g.query.is_empty() => {
                self.finish_global_search(Some(Scope::Tabs), window, cx)
            }
            _ => {
                if edit_query(&mut g.query, ks) {
                    cx.notify();
                }
            }
        }
        cx.stop_propagation();
    }

    pub(super) fn render_global_search(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let g = self.global.as_ref()?;
        let c = Colors::new(&self.theme);
        let empty = g.query.is_empty();
        let accent = gpui::rgb(0x047afe);
        let button =
            |id: &'static str, label: &'static str, how: Option<Scope>, cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .h(px(26.))
                    .px(px(11.))
                    .flex()
                    .items_center()
                    .rounded(px(7.))
                    .border(px(0.5))
                    .border_color(c.border)
                    .bg(c.keycap)
                    .text_size(px(12.5))
                    .text_color(c.text)
                    .when(empty, |d| d.opacity(0.45))
                    .when(!empty, |d| d.hover(|d| d.bg(c.selected)))
                    .child(label)
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.global.as_ref().is_some_and(|g| !g.query.is_empty()) {
                            this.finish_global_search(how, window, cx);
                        }
                    }))
            };
        let footer = div()
            .flex_none()
            .px(px(12.))
            .py(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(button("global-current", "Search in Current Tab", None, cx))
            .child(button(
                "global-files",
                "Search Files",
                Some(Scope::Files),
                cx,
            ))
            .child(div().flex_1())
            .child(
                button("global-all", "Search All Tabs", Some(Scope::Tabs), cx)
                    .bg(accent)
                    .border_color(accent)
                    .text_color(gpui::white()),
            );
        let card = picker::card(WIDTH, &c)
            .id("global-search")
            .key_context("GlobalSearch")
            .track_focus(&g.focus)
            .on_key_down(cx.listener(Self::on_global_key))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                let text = cx.read_from_clipboard().and_then(|c| c.text());
                if let (Some(g), Some(text)) = (this.global.as_mut(), text) {
                    g.query.push_str(text.lines().next().unwrap_or_default());
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(picker::search_row(&g.query, "Find in All Tabs", &c))
            .child(div().flex_none().h(px(1.)).bg(c.separator))
            .child(footer);
        Some(
            div()
                .id("global-search-scrim")
                .absolute()
                .inset_0()
                .px(px(16.))
                .pt(px(picker::top(window)))
                .flex()
                .flex_col()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_global_search(window, cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}
