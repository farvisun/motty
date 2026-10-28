//! Find in All Tabs (⇧⌘F): every pane's scrollback searched at once, the
//! matches listed by tab; picking one shows that pane with the find bar
//! on the match.

use gpui::{
    AnyElement, Context, Entity, FocusHandle, KeyDownEvent, MouseButton, ScrollHandle,
    SharedString, Window, div, prelude::*, px,
};
use motty_term::alacritty_terminal::grid::Dimensions;
use motty_term::alacritty_terminal::index::{Column, Point};
use motty_term::search::{self, SearchOptions};

use super::picker::{self, Colors, ROW_GAP, ROW_HEIGHT, ROW_INSET, ROW_RADIUS, edit_query};
use super::{Workspace, tab_title};
use crate::terminal_view::TerminalView;

const WIDTH: f32 = 620.;
const LIST_MAX_HEIGHT: f32 = 420.;
const TEXT_LEFT: f32 = 22.;
/// Matches listed per pane, and in all.
const PER_PANE: usize = 200;
const TOTAL: usize = 600;

pub(super) struct GlobalSearch {
    pub focus: FocusHandle,
    pub query: String,
    pub hits: Vec<Hit>,
    pub selected: usize,
    pub scroll: ScrollHandle,
}

pub(super) struct Hit {
    tab: usize,
    title: String,
    pane: Entity<TerminalView>,
    /// Which of the pane's matches, as its find bar counts them.
    index: usize,
    line: String,
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
        self.global = Some(GlobalSearch {
            focus,
            query,
            hits: Vec::new(),
            selected: 0,
            scroll: ScrollHandle::new(),
        });
        self.run_global_search(cx);
    }

    fn close_global_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.global = None;
        self.activate(self.active, window, cx);
    }

    fn run_global_search(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.global.as_ref().map(|g| g.query.clone()) else {
            return;
        };
        let mut hits = Vec::new();
        if !query.trim().is_empty() {
            'tabs: for (tab_ix, tab) in self.tabs.iter().enumerate() {
                let Some(terms) = tab.terms() else { continue };
                let title = tab_title(tab, cx);
                for pane in terms.root.leaves() {
                    let term = pane.read(cx).session().term.lock();
                    let Ok(found) =
                        search::find_all(&*term, &query, SearchOptions::default(), PER_PANE)
                    else {
                        continue;
                    };
                    // A line with several matches is listed once, at the first.
                    let mut last_line = None;
                    for (index, m) in found.iter().enumerate() {
                        let Some((start, _)) = m.to_points(&*term) else {
                            continue;
                        };
                        if last_line.replace(start.line) == Some(start.line) {
                            continue;
                        }
                        let end = Point::new(start.line, term.grid().last_column());
                        let line = term
                            .bounds_to_string(Point::new(start.line, Column(0)), end)
                            .trim()
                            .to_owned();
                        hits.push(Hit {
                            tab: tab_ix,
                            title: title.clone(),
                            pane: pane.clone(),
                            index,
                            line,
                        });
                        if hits.len() >= TOTAL {
                            break 'tabs;
                        }
                    }
                }
            }
        }
        if let Some(g) = self.global.as_mut() {
            g.hits = hits;
            g.selected = 0;
            g.scroll.scroll_to_item(0);
        }
        cx.notify();
    }

    fn open_hit(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = self.global.take() else { return };
        let Some(hit) = g.hits.get(ix) else {
            return self.activate(self.active, window, cx);
        };
        let (pane, index, query) = (hit.pane.clone(), hit.index, g.query);
        self.reveal_pane(&pane, window, cx);
        pane.update(cx, |v, cx| {
            v.show_find_at(&query, SearchOptions::default(), index, window, cx)
        });
        cx.notify();
    }

    fn on_global_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let Some(g) = self.global.as_mut() else {
            return;
        };
        let n = g.hits.len();
        match ks.key.as_str() {
            "escape" => self.close_global_search(window, cx),
            "enter" => {
                let ix = g.selected;
                self.open_hit(ix, window, cx);
            }
            "up" if n > 0 => {
                g.selected = (g.selected + n - 1) % n;
                g.scroll.scroll_to_item(g.selected);
                cx.notify();
            }
            "down" if n > 0 => {
                g.selected = (g.selected + 1) % n;
                g.scroll.scroll_to_item(g.selected);
                cx.notify();
            }
            _ => {
                if edit_query(&mut g.query, ks) {
                    self.run_global_search(cx);
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
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut tab = None;
        for (ix, hit) in g.hits.iter().enumerate() {
            if tab != Some(hit.tab) {
                rows.push(
                    picker::header(&hit.title, TEXT_LEFT, tab.is_none(), &c).into_any_element(),
                );
                tab = Some(hit.tab);
            }
            rows.push(
                div()
                    .id(ix)
                    .flex_none()
                    .mx(px(ROW_INSET))
                    .h(px(ROW_HEIGHT))
                    .pl(px(TEXT_LEFT - ROW_INSET))
                    .pr(px(12.))
                    .flex()
                    .items_center()
                    .rounded(px(ROW_RADIUS))
                    .when(ix == g.selected, |d| d.bg(c.selected))
                    .font_family(self.mono_font.clone())
                    .text_size(px(12.))
                    .text_color(c.text)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(hit.line.clone())),
                    )
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if let Some(g) = this.global.as_mut()
                            && g.selected != ix
                        {
                            g.selected = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_hit(ix, window, cx)))
                    .into_any_element(),
            );
        }
        if g.hits.is_empty() {
            let note = if g.query.trim().is_empty() {
                "Type to search every tab"
            } else {
                "No matches"
            };
            rows.push(
                div()
                    .h(px(ROW_HEIGHT))
                    .pl(px(TEXT_LEFT))
                    .flex()
                    .items_center()
                    .text_color(c.placeholder)
                    .child(note)
                    .into_any_element(),
            );
        }
        let card = picker::card(WIDTH, &c)
            .id("global-search")
            .key_context("GlobalSearch")
            .track_focus(&g.focus)
            .on_key_down(cx.listener(Self::on_global_key))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(picker::search_row(&g.query, "Find in All Tabs", &c))
            .child(div().flex_none().h(px(1.)).bg(c.separator))
            .child(
                div()
                    .id("global-search-list")
                    .max_h(px(LIST_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&g.scroll)
                    .pb(px(ROW_INSET))
                    .flex()
                    .flex_col()
                    .gap(px(ROW_GAP))
                    .children(rows),
            );
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
