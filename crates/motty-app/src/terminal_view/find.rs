//! The find bar (⌘F): a card at the pane's top right that searches the
//! visible screen and scrollback, highlights every match and steps through
//! them. Geometry is measured from Otty's documentation screenshot at 2x.

use std::time::Duration;

use gpui::{
    BoxShadow, ClipboardItem, Context, FocusHandle, Hsla, KeyDownEvent, MouseButton, SharedString,
    Task, Window, div, point, prelude::*, px,
};
use motty_render::{Highlight, hsla};
use motty_term::alacritty_terminal::grid::{Dimensions, Scroll};
use motty_term::search::{self, SearchMatch, SearchOptions};
use motty_theme::Rgba;

use super::TerminalView;
use crate::actions::*;
use crate::titlebar::{icon, stroke};

/// Matches counted at most; past this the counter shows "N+".
const MAX_MATCHES: usize = 10_000;
/// Output while the bar is open re-runs the search at most this often.
const REFRESH: Duration = Duration::from_millis(150);

const CARD_WIDTH: f32 = 340.;
pub(super) const CARD_RADIUS: f32 = 10.;
pub(super) const CARD_INSET: f32 = 8.;
const INPUT_HEIGHT: f32 = 30.;
const INPUT_RADIUS: f32 = 6.;
const TOGGLE_WIDTH: f32 = 21.;
const TOGGLE_HEIGHT: f32 = 18.;
const TOGGLE_GAP: f32 = 5.;
/// The chevrons, the search-all-tabs button and close sit 26pt apart.
const BUTTON_WIDTH: f32 = 26.;
const FONT_SIZE: f32 = 13.;
/// The text field's caret: the system accent blue.
const CARET: u32 = 0x047afe;

pub struct Find {
    pub query: String,
    pub options: SearchOptions,
    pub matches: Vec<SearchMatch>,
    pub current: Option<usize>,
    /// The query is not a valid regex.
    pub invalid: bool,
    pub focus: FocusHandle,
    refresh: Option<Task<()>>,
}

/// Colors derived from the theme: overlays of black on light themes and
/// white on dark ones, over the panel background.
pub(super) struct Colors {
    pub card: Hsla,
    pub border: Hsla,
    input: Hsla,
    pub toggle_border: Hsla,
    pub text: Hsla,
    pub secondary: Hsla,
    icon: Hsla,
    hover: Hsla,
    active: Hsla,
    pub shadow: Hsla,
}

impl Colors {
    pub(super) fn new(panel: Rgba, dark: bool) -> Self {
        let ink = |a: f32| {
            let base = if dark { 1. } else { 0. };
            hsla(Rgba {
                r: base,
                g: base,
                b: base,
                a,
            })
        };
        Self {
            card: hsla(panel),
            border: ink(if dark { 0.12 } else { 0.15 }),
            input: ink(if dark { 0.12 } else { 0.17 }),
            toggle_border: ink(0.06),
            text: ink(if dark { 0.92 } else { 0.88 }),
            secondary: ink(0.5),
            icon: ink(if dark { 0.75 } else { 0.71 }),
            hover: ink(0.08),
            active: hsla(Rgba::rgb(CARET).with_alpha(0.22)),
            shadow: hsla(Rgba {
                r: 0.,
                g: 0.,
                b: 0.,
                a: if dark { 0.5 } else { 0.3 },
            }),
        }
    }
}

impl TerminalView {
    /// Opens the bar (or re-focuses it), seeded with a one-line selection.
    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self
            .session
            .term
            .lock()
            .selection_to_string()
            .filter(|s| !s.is_empty() && !s.contains('\n'));
        let find = self.find.get_or_insert_with(|| Find {
            query: self.last_find.0.clone(),
            options: self.last_find.1,
            matches: Vec::new(),
            current: None,
            invalid: false,
            focus: cx.focus_handle(),
            refresh: None,
        });
        if let Some(text) = selected {
            find.query = text;
        }
        window.focus(&find.focus.clone(), cx);
        self.run_find(true, cx);
    }

    /// Opens the bar with `query` (from the control socket).
    pub fn show_find(
        &mut self,
        query: &str,
        options: SearchOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_find(window, cx);
        if let Some(find) = self.find.as_mut() {
            find.query = query.to_owned();
            find.options = options;
        }
        self.run_find(true, cx);
    }

    /// Opens the bar on the `index`th match of `query` (Find in All Tabs).
    pub fn show_find_at(
        &mut self,
        query: &str,
        options: SearchOptions,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_find(query, options, window, cx);
        if let Some(find) = self.find.as_mut()
            && index < find.matches.len()
        {
            find.current = Some(index);
            self.reveal_current();
        }
        cx.notify();
    }

    /// What the bar holds, or held when it last closed.
    pub fn last_find_query(&self) -> String {
        self.find
            .as_ref()
            .map_or_else(|| self.last_find.0.clone(), |f| f.query.clone())
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(find) = self.find.take() {
            self.last_find = (find.query, find.options);
        }
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// ⌘G / ⇧⌘G: the next or previous match; opens the bar if it's closed.
    pub(super) fn find_step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            self.open_find(window, cx);
            return;
        };
        let n = find.matches.len();
        if n == 0 {
            return;
        }
        find.current = Some(match find.current {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        });
        self.reveal_current();
        cx.notify();
    }

    /// Re-runs the search. A new query starts at the first match from the
    /// top of the view; otherwise the current match is kept.
    pub(super) fn run_find(&mut self, new_query: bool, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let term = self.session.term.lock();
        let previous = find.current.and_then(|i| find.matches.get(i)).copied();
        match search::find_all(&term, &find.query, find.options, MAX_MATCHES) {
            Ok(matches) => {
                find.matches = matches;
                find.invalid = false;
            }
            Err(_) => {
                find.matches.clear();
                find.invalid = true;
            }
        }
        let grid = term.grid();
        let view_top = grid.lines_pushed() - grid.display_offset() as u64;
        drop(term);
        find.current = if find.matches.is_empty() {
            None
        } else if let Some(prev) = previous.filter(|_| !new_query) {
            // The same match, else the nearest one after where it was.
            Some(
                find.matches
                    .partition_point(|m| m < &prev)
                    .min(find.matches.len() - 1),
            )
        } else {
            Some(
                find.matches
                    .iter()
                    .position(|m| m.start.0 >= view_top)
                    .unwrap_or(find.matches.len() - 1),
            )
        };
        if new_query {
            self.reveal_current();
        }
        cx.notify();
    }

    /// Output arrived while the bar is open: search again shortly.
    pub(super) fn schedule_find_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        if find.refresh.is_some() || find.query.is_empty() {
            return;
        }
        find.refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH).await;
            let _ = this.update(cx, |view, cx| {
                if let Some(find) = view.find.as_mut() {
                    find.refresh = None;
                }
                view.run_find(false, cx);
            });
        }));
    }

    /// Scrolls the current match into view.
    fn reveal_current(&mut self) {
        let Some(m) = self
            .find
            .as_ref()
            .and_then(|f| f.matches.get(f.current?))
            .copied()
        else {
            return;
        };
        let mut term = self.session.term.lock();
        if let Some((start, _)) = m.to_points(&*term) {
            // Center it when it's off screen.
            let offset = term.grid().display_offset() as i32;
            let rows = term.screen_lines() as i32;
            let row = start.line.0 + offset;
            if !(0..rows).contains(&row) {
                term.scroll_display(Scroll::Delta(rows / 2 - row));
            }
        }
    }

    /// Scrolls so the stable `row` (see [`motty_term::shell::Block`]) is at
    /// the top of the screen, as far as the scrollback allows.
    pub fn reveal_row(&mut self, row: u64, cx: &mut Context<Self>) {
        let mut term = self.session.term.lock();
        let top = term.grid().lines_pushed() as i64 - term.grid().display_offset() as i64;
        term.scroll_display(Scroll::Delta((top - row as i64) as i32));
        drop(term);
        cx.notify();
    }

    /// Matches on screen, for the terminal element.
    pub(super) fn find_highlights(&self) -> Vec<Highlight> {
        let Some(find) = &self.find else {
            return Vec::new();
        };
        let term = self.session.term.lock();
        let grid = term.grid();
        let top = grid.lines_pushed() - grid.display_offset() as u64;
        let bottom = top + term.screen_lines() as u64;
        let first = find.matches.partition_point(|m| m.end.0 < top);
        find.matches[first..]
            .iter()
            .enumerate()
            .take_while(|(_, m)| m.start.0 < bottom)
            .filter_map(|(i, m)| {
                let (start, end) = m.to_points(&*term)?;
                Some(Highlight {
                    start,
                    end,
                    current: find.current == Some(first + i),
                })
            })
            .collect()
    }

    fn edit_query(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut String)) {
        if let Some(find) = self.find.as_mut() {
            edit(&mut find.query);
            self.run_find(true, cx);
        }
    }

    fn on_find_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        // ⌘ shortcuts go on to the actions; nothing else reaches the shell.
        if m.platform {
            return;
        }
        cx.stop_propagation();
        match ks.key.as_str() {
            // In vi mode Esc goes back to the buffer, where n/N step.
            "escape" if self.vi.is_some() => window.focus(&self.focus_handle, cx),
            "escape" => self.close_find(window, cx),
            "enter" => self.find_step(!m.shift, window, cx),
            "backspace" if m.alt => self.edit_query(cx, |q| {
                let trimmed = q.trim_end_matches(|c: char| !c.is_alphanumeric());
                let word = trimmed.trim_end_matches(char::is_alphanumeric);
                q.truncate(word.len());
            }),
            "backspace" => self.edit_query(cx, |q| {
                q.pop();
            }),
            _ if m.control => {}
            _ => {
                if let Some(text) = ks
                    .key_char
                    .as_deref()
                    .filter(|t| !t.chars().any(char::is_control))
                {
                    let text = text.to_owned();
                    self.edit_query(cx, |q| q.push_str(&text));
                }
            }
        }
    }

    fn toggle_option(&mut self, toggle: fn(&mut SearchOptions), cx: &mut Context<Self>) {
        if let Some(find) = self.find.as_mut() {
            toggle(&mut find.options);
            self.run_find(true, cx);
        }
    }

    pub(super) fn render_find_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let find = self.find.as_ref()?;
        let c = Colors::new(self.style.panel_background, self.style.dark);
        let counter = (!find.query.is_empty() && !find.invalid).then(|| {
            let total = if find.matches.len() >= MAX_MATCHES {
                format!("{MAX_MATCHES}+")
            } else {
                find.matches.len().to_string()
            };
            let n = find.current.map_or(0, |i| i + 1);
            SharedString::from(format!("{n} of {total}"))
        });
        let toggle = |id: &'static str,
                      on: bool,
                      label: gpui::AnyElement,
                      flip: fn(&mut SearchOptions),
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .w(px(TOGGLE_WIDTH))
                .h(px(TOGGLE_HEIGHT))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.5))
                .border(px(0.5))
                .border_color(c.toggle_border)
                .when(on, |d| d.bg(c.active))
                .text_size(px(11.))
                .text_color(c.icon)
                .child(label)
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_option(flip, cx)))
        };
        let button = |id: &'static str, glyph: gpui::AnyElement| {
            div()
                .id(id)
                .w(px(BUTTON_WIDTH))
                .h(px(INPUT_HEIGHT))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .hover(|d| d.bg(c.hover))
                .child(glyph)
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
        };
        let icon_color = c.icon;
        let input = div()
            .flex_1()
            .min_w_0()
            .h(px(INPUT_HEIGHT))
            .flex()
            .items_center()
            .pl(px(8.))
            .pr(px(4.))
            .gap(px(TOGGLE_GAP))
            .rounded(px(INPUT_RADIUS))
            .border(px(0.5))
            .border_color(c.toggle_border)
            .bg(c.input)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .text_size(px(FONT_SIZE))
                    .text_color(if find.invalid {
                        hsla(Rgba::rgb(0xd70015))
                    } else {
                        c.text
                    })
                    .child(SharedString::from(find.query.clone()))
                    .child(
                        div()
                            .flex_none()
                            .w(px(1.5))
                            .h(px(16.))
                            .ml(px(0.5))
                            .bg(hsla(Rgba::rgb(CARET))),
                    ),
            )
            .children(counter.map(|text| {
                div()
                    .flex_none()
                    .mr(px(2.))
                    .text_size(px(11.))
                    .text_color(c.secondary)
                    .child(text)
            }))
            .child(toggle(
                "find-case",
                find.options.case_sensitive,
                div().child("Aa").into_any_element(),
                |o| o.case_sensitive = !o.case_sensitive,
                cx,
            ))
            .child(toggle(
                "find-word",
                find.options.whole_word,
                whole_word_glyph(icon_color).into_any_element(),
                |o| o.whole_word = !o.whole_word,
                cx,
            ))
            .child(toggle(
                "find-regex",
                find.options.regex,
                div().child(".*").into_any_element(),
                |o| o.regex = !o.regex,
                cx,
            ));
        Some(
            div()
                .id("find-bar")
                .key_context("FindBar")
                .track_focus(&find.focus)
                .absolute()
                .top(px(CARD_INSET))
                .right(px(CARD_INSET))
                .w(px(CARD_WIDTH))
                .p(px(CARD_INSET))
                .flex()
                .items_center()
                .rounded(px(CARD_RADIUS))
                .border(px(0.5))
                .border_color(c.border)
                .bg(c.card)
                .shadow(vec![BoxShadow {
                    color: c.shadow,
                    offset: point(px(0.), px(3.)),
                    blur_radius: px(8.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .cursor(gpui::CursorStyle::Arrow)
                .on_key_down(cx.listener(Self::on_find_key))
                .on_action(cx.listener(|this, _: &Paste, _, cx| {
                    let text = cx.read_from_clipboard().and_then(|c| c.text());
                    if let Some(line) = text.as_deref().and_then(|t| t.lines().next()) {
                        let line = line.to_owned();
                        this.edit_query(cx, |q| q.push_str(&line));
                    }
                }))
                .on_action(cx.listener(|this, _: &Copy, _, cx| {
                    if let Some(find) = &this.find {
                        cx.write_to_clipboard(ClipboardItem::new_string(find.query.clone()));
                    }
                }))
                .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                    // No selection in the field: ⌘A then typing replaces.
                    this.edit_query(cx, String::clear);
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        if let Some(find) = &this.find {
                            window.focus(&find.focus.clone(), cx);
                        }
                        cx.stop_propagation();
                    }),
                )
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(input)
                .child(div().w(px(5.)).flex_none())
                .child(
                    button(
                        "find-previous",
                        chevron(icon_color, true).into_any_element(),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.find_step(false, window, cx))),
                )
                .child(
                    button("find-next", chevron(icon_color, false).into_any_element()).on_click(
                        cx.listener(|this, _, window, cx| this.find_step(true, window, cx)),
                    ),
                )
                .child(
                    button("find-all-tabs", all_tabs(icon_color).into_any_element()).on_click(
                        |_, window, cx| window.dispatch_action(Box::new(GlobalSearch), cx),
                    ),
                )
                .child(
                    button("find-close", close(icon_color).into_any_element())
                        .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
                ),
        )
    }
}

/// "ab" over an open bracket, for whole-word matching.
fn whole_word_glyph(color: Hsla) -> impl IntoElement {
    div()
        .relative()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .pb(px(2.))
        .child("ab")
        .child(div().absolute().left_0().top_0().child(icon(
            TOGGLE_WIDTH,
            TOGGLE_HEIGHT,
            move |o, window| {
                stroke(o, color, window, 1., |p, at| {
                    p.move_to(at(5., 12.5));
                    p.line_to(at(5., 14.));
                    p.line_to(at(16., 14.));
                    p.line_to(at(16., 12.5));
                });
            },
        )))
}

/// A 10pt chevron pointing up or down.
fn chevron(color: Hsla, up: bool) -> impl IntoElement {
    icon(BUTTON_WIDTH, INPUT_HEIGHT, move |o, window| {
        let (tip, arms) = if up { (12.5, 17.) } else { (17.5, 13.) };
        stroke(o, color, window, 1.3, |p, at| {
            p.move_to(at(8.5, arms));
            p.line_to(at(13., tip));
            p.line_to(at(17.5, arms));
        });
    })
}

/// A box with a tab edge above it: search every tab.
fn all_tabs(color: Hsla) -> impl IntoElement {
    icon(BUTTON_WIDTH, INPUT_HEIGHT, move |o, window| {
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(8., 13.));
            p.line_to(at(18., 13.));
            p.line_to(at(18., 21.));
            p.line_to(at(8., 21.));
            p.close();
        });
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(9.5, 10.));
            p.line_to(at(16.5, 10.));
        });
    })
}

/// A 9pt ×.
fn close(color: Hsla) -> impl IntoElement {
    icon(BUTTON_WIDTH, INPUT_HEIGHT, move |o, window| {
        stroke(o, color, window, 1.3, |p, at| {
            p.move_to(at(9., 11.));
            p.line_to(at(17., 19.));
        });
        stroke(o, color, window, 1.3, |p, at| {
            p.move_to(at(17., 11.));
            p.line_to(at(9., 19.));
        });
    })
}
