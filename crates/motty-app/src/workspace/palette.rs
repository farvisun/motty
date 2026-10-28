//! Command palette (⇧⌘P): every action, with or without a shortcut, found
//! by typing. ↩ runs the selected one in the pane that had focus, ⌘↩ runs it
//! and keeps the palette open, Esc closes it. Metrics are Otty's: a 560pt
//! card 70pt below the window top, 34pt rows on a 36pt pitch.

use gpui::{
    Action, BoxShadow, ClipboardItem, Context, Entity, FocusHandle, Focusable, Hsla, KeyDownEvent,
    Keystroke, MouseButton, ScrollHandle, SharedString, Window, div, point, prelude::*, px,
};
use motty_render::hsla;
use motty_theme::{Mode, Rgba};

use super::Workspace;
use crate::actions::*;
use crate::terminal_view::{PaneMode, TerminalView};
use crate::titlebar::{icon, stroke};

const WIDTH: f32 = 560.;
const TOP: f32 = 70.;
const RADIUS: f32 = 14.;
const SEARCH_HEIGHT: f32 = 46.;
/// Where row text starts (Otty's glyphs start at 42.5).
const TEXT_LEFT: f32 = 41.5;
/// Where the search caret starts.
const CARET_LEFT: f32 = 41.;
const ROW_HEIGHT: f32 = 34.;
const ROW_GAP: f32 = 2.;
const ROW_INSET: f32 = 8.;
const ROW_RADIUS: f32 = 7.;
/// Plus the row gap; later headers get [`HEADER_SPACE`] more above.
const HEADER_HEIGHT: f32 = 26.;
const HEADER_SPACE: f32 = 1.5;
const LIST_MAX_HEIGHT: f32 = 320.;
const KEYCAP_WIDTH: f32 = 21.;
const KEYCAP_HEIGHT: f32 = 20.;
const CARET: u32 = 0x047afe;
const CHECK: u32 = 0x2b5b38;

pub(super) struct Palette {
    focus: FocusHandle,
    query: String,
    /// Index into the filtered commands.
    selected: usize,
    /// The pane commands run in.
    target: Entity<TerminalView>,
    scroll: ScrollHandle,
}

struct Command {
    group: &'static str,
    title: &'static str,
    action: Box<dyn Action>,
    checked: bool,
}

impl Command {
    fn new(group: &'static str, title: &'static str, action: impl Action) -> Self {
        Self {
            group,
            title,
            action: Box::new(action),
            checked: false,
        }
    }

    fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
}

/// Everything the palette offers, in display order.
fn commands(sidebar_visible: bool, target: &TerminalView) -> Vec<Command> {
    let mode = target.mode();
    let mut list = Vec::new();
    if target.cwd.is_some() {
        list.extend([
            Command::new("Working Directory", "Copy Path", CopyPath),
            Command::new("Working Directory", "Reveal in Finder", RevealInFinder),
        ]);
    }
    list.extend([
        Command::new("View", "Toggle Tabs Panel", ToggleTabsPanel).checked(sidebar_visible),
        Command::new("View", "Toggle Full Screen", ToggleFullScreen),
        Command::new("View", "Increase Font Size", IncreaseFontSize),
        Command::new("View", "Decrease Font Size", DecreaseFontSize),
        Command::new("View", "Reset Font Size", ResetFontSize),
        Command::new("Window", "New Tab", NewTab),
        Command::new("Window", "Close Tab", CloseTab),
        Command::new("Window", "Reopen Closed Tab", ReopenClosedTab),
        Command::new("Window", "Next Tab", NextTab),
        Command::new("Window", "Previous Tab", PreviousTab),
        Command::new("Window", "New Window", NewWindow),
        Command::new("Window", "Close Window", CloseWindow),
        Command::new("Window", "Minimize", Minimize),
        Command::new("Pane", "Split Right", SplitRight),
        Command::new("Pane", "Split Left", SplitLeft),
        Command::new("Pane", "Split Down", SplitDown),
        Command::new("Pane", "Split Up", SplitUp),
        Command::new("Pane", "Equalize Splits", EqualizeSplits),
        Command::new("Pane", "Zoom Split", ZoomSplit),
        Command::new("Pane", "Focus Next Pane", FocusNextPane),
        Command::new("Pane", "Focus Previous Pane", FocusPreviousPane),
        Command::new("Terminal", "Find", Find),
        Command::new("Terminal", "Clear Screen", Clear),
        Command::new("Terminal", "Copy", Copy),
        Command::new("Terminal", "Paste", Paste),
        Command::new("Terminal", "Select All", SelectAll),
        Command::new("Terminal", "Scroll to Top", ScrollToTop),
        Command::new("Terminal", "Scroll to Bottom", ScrollToBottom),
        Command::new("Terminal", "Vi Mode", ToggleViMode)
            .checked(matches!(mode, Some(PaneMode::Vi { .. }))),
        Command::new("Terminal", "Read-Only Mode", ToggleReadOnly)
            .checked(mode == Some(PaneMode::ReadOnly)),
        Command::new("Terminal", "Hints: Open Link", HintOpen),
        Command::new("Terminal", "Hints: Copy Link", HintCopy),
        Command::new("Terminal", "Hints: Paste Word", HintPaste),
        Command::new("Terminal", "Hints: Reveal Path", HintReveal),
        Command::new("Settings", "Open Config File", OpenConfigFile),
        Command::new("Motty", "Quit Motty", Quit),
    ]);
    list
}

/// How well `query` matches `text`: its characters in order, ignoring case,
/// scoring runs and word starts. `None` when it doesn't match.
fn score(query: &str, text: &str) -> Option<i32> {
    let text: Vec<char> = text.chars().collect();
    let mut score = 0;
    let mut pos = 0;
    let mut last: Option<usize> = None;
    for q in query.chars().filter(|c| !c.is_whitespace()) {
        let q = q.to_lowercase().next()?;
        let found = (pos..text.len()).find(|&i| text[i].to_lowercase().next() == Some(q))?;
        let word_start = found == 0 || !text[found - 1].is_alphanumeric();
        score += match () {
            _ if last == Some(found.wrapping_sub(1)) => 8,
            _ if word_start => 6,
            _ => 1,
        };
        // Matches near the start count a little more.
        score -= (found as i32 - pos as i32).min(10) / 3;
        last = Some(found);
        pos = found + 1;
    }
    Some(score)
}

/// The commands shown for `query`: all of them grouped, or the matches
/// best first.
fn filter(commands: Vec<Command>, query: &str) -> Vec<Command> {
    if query.trim().is_empty() {
        return commands;
    }
    let mut scored: Vec<(i32, Command)> = commands
        .into_iter()
        .filter_map(|c| {
            let title = score(query, c.title);
            let full = score(query, &format!("{}: {}", c.group, c.title)).map(|s| s - 2);
            Some((title.max(full)?, c))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(_, c)| c).collect()
}

/// Key caps for a binding, modifiers in macOS order.
fn keycaps(keystroke: &Keystroke) -> Vec<SharedString> {
    let m = &keystroke.modifiers;
    let mut caps: Vec<SharedString> = [
        (m.control, "⌃"),
        (m.alt, "⌥"),
        (m.shift, "⇧"),
        (m.platform, "⌘"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, glyph)| glyph.into())
    .collect();
    let key = match keystroke.key.as_str() {
        "enter" => "↩".into(),
        "escape" => "⎋".into(),
        "backspace" => "⌫".into(),
        "delete" => "⌦".into(),
        "tab" => "⇥".into(),
        "space" => "Space".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "home" => "↖".into(),
        "end" => "↘".into(),
        "pageup" => "⇞".into(),
        "pagedown" => "⇟".into(),
        key => key.to_uppercase(),
    };
    caps.push(key.into());
    caps
}

struct Colors {
    card: Hsla,
    border: Hsla,
    separator: Hsla,
    text: Hsla,
    placeholder: Hsla,
    header: Hsla,
    selected: Hsla,
    keycap: Hsla,
    keycap_border: Hsla,
    keycap_text: Hsla,
    shadow: Hsla,
}

impl Colors {
    /// Measured from Otty's light palette (#f5f4f0 card, #e1e0de row,
    /// #1a1a1a text, #c1beb5 headers); dark themes mirror the alphas.
    fn new(panel: Rgba, dark: bool) -> Self {
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
            border: ink(if dark { 0.14 } else { 0.18 }),
            separator: ink(0.035),
            text: ink(if dark { 0.92 } else { 0.9 }),
            placeholder: ink(0.42),
            header: ink(if dark { 0.35 } else { 0.24 }),
            selected: ink(if dark { 0.1 } else { 0.08 }),
            keycap: hsla(if dark {
                Rgba::rgb(0xffffff).with_alpha(0.06)
            } else {
                Rgba::rgb(0xffffff).with_alpha(0.55)
            }),
            keycap_border: ink(if dark { 0.1 } else { 0.06 }),
            keycap_text: ink(0.45),
            shadow: hsla(Rgba {
                r: 0.,
                g: 0.,
                b: 0.,
                a: if dark { 0.45 } else { 0.25 },
            }),
        }
    }
}

/// A 10.5pt magnifying glass.
fn magnifier(color: Hsla) -> impl IntoElement {
    icon(12., 12., move |o, window| {
        stroke(o, color, window, 1.3, |p, at| {
            // A circle from four quarter arcs (k: the bezier handle length).
            let (cx, cy, r) = (4.75, 4.75, 4.);
            let k = r * 0.5523;
            p.move_to(at(cx + r, cy));
            p.cubic_bezier_to(at(cx, cy + r), at(cx + r, cy + k), at(cx + k, cy + r));
            p.cubic_bezier_to(at(cx - r, cy), at(cx - k, cy + r), at(cx - r, cy + k));
            p.cubic_bezier_to(at(cx, cy - r), at(cx - r, cy - k), at(cx - k, cy - r));
            p.cubic_bezier_to(at(cx + r, cy), at(cx + k, cy - r), at(cx + r, cy - k));
            p.close();
        });
        stroke(o, color, window, 1.3, |p, at| {
            p.move_to(at(7.75, 7.75));
            p.line_to(at(11.25, 11.25));
        });
    })
}

impl Workspace {
    /// ⇧⌘P opens the palette over the focused pane (or closes it).
    pub(super) fn toggle_palette(
        &mut self,
        _: &CommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette.is_some() {
            return self.close_palette(window, cx);
        }
        let Some(target) = self.focused_pane() else {
            return;
        };
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.palette = Some(Palette {
            focus,
            query: String::new(),
            selected: 0,
            target,
            scroll: ScrollHandle::new(),
        });
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(palette) = self.palette.take() {
            window.focus(&palette.target.focus_handle(cx), cx);
        }
        cx.notify();
    }

    fn palette_commands(&self, cx: &Context<Self>) -> Vec<Command> {
        let Some(palette) = &self.palette else {
            return Vec::new();
        };
        filter(
            commands(self.sidebar_visible, palette.target.read(cx)),
            &palette.query,
        )
    }

    fn edit_palette_query(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut String)) {
        if let Some(palette) = self.palette.as_mut() {
            edit(&mut palette.query);
            palette.selected = 0;
            palette.scroll.scroll_to_item(0);
            cx.notify();
        }
    }

    fn move_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.palette_commands(cx).len();
        let grouped = self
            .palette
            .as_ref()
            .is_some_and(|p| p.query.trim().is_empty());
        let headers = if grouped {
            self.palette_headers_before(cx)
        } else {
            Vec::new()
        };
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        if n == 0 {
            return;
        }
        palette.selected = (palette.selected as isize + delta).rem_euclid(n as isize) as usize;
        // Headers are children of the list too.
        let child = palette.selected + headers.get(palette.selected).copied().unwrap_or(0);
        palette.scroll.scroll_to_item(child);
        cx.notify();
    }

    /// For each grouped command, how many headers come before it.
    fn palette_headers_before(&self, cx: &Context<Self>) -> Vec<usize> {
        let mut out = Vec::new();
        let mut group = "";
        let mut headers = 0;
        for c in self.palette_commands(cx) {
            if c.group != group {
                group = c.group;
                headers += 1;
            }
            out.push(headers);
        }
        out
    }

    /// Runs the `ix`th shown command in the target pane; `keep` leaves the
    /// palette open.
    fn run_palette_command(
        &mut self,
        ix: usize,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = self.palette_commands(cx).into_iter().nth(ix) else {
            return;
        };
        let Some(target) = self.palette.as_ref().map(|p| p.target.focus_handle(cx)) else {
            return;
        };
        if !keep {
            self.close_palette(window, cx);
        }
        // Dispatched from the pane, so both its actions and the window's
        // reach it; deferred, since the window's handlers need `self`.
        let action = command.action;
        cx.defer_in(window, move |_, window, cx| {
            target.dispatch_action(&*action, window, cx);
        });
    }

    fn on_palette_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        match ks.key.as_str() {
            "enter" => {
                let ix = self.palette.as_ref().map_or(0, |p| p.selected);
                self.run_palette_command(ix, m.platform, window, cx);
            }
            _ if m.platform => return,
            "escape" => self.close_palette(window, cx),
            "up" => self.move_palette_selection(-1, cx),
            "down" => self.move_palette_selection(1, cx),
            "p" | "k" if m.control => self.move_palette_selection(-1, cx),
            "n" | "j" if m.control => self.move_palette_selection(1, cx),
            "backspace" if m.alt => self.edit_palette_query(cx, |q| {
                let trimmed = q.trim_end_matches(|c: char| !c.is_alphanumeric());
                let word = trimmed.trim_end_matches(char::is_alphanumeric);
                q.truncate(word.len());
            }),
            "backspace" => self.edit_palette_query(cx, |q| {
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
                    self.edit_palette_query(cx, |q| q.push_str(&text));
                }
            }
        }
        cx.stop_propagation();
    }

    pub(super) fn render_palette(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let palette = self.palette.as_ref()?;
        let t = &self.theme;
        let c = Colors::new(t.panel.background.with_alpha(1.), t.meta.mode == Mode::Dark);
        let target = palette.target.focus_handle(cx);
        let commands = self.palette_commands(cx);
        let grouped = palette.query.trim().is_empty();
        let selected = palette.selected.min(commands.len().saturating_sub(1));

        let mut children: Vec<gpui::AnyElement> = Vec::new();
        let mut group = "";
        for (ix, command) in commands.iter().enumerate() {
            if grouped && command.group != group {
                let first = group.is_empty();
                group = command.group;
                children.push(
                    div()
                        .flex_none()
                        .h(px(HEADER_HEIGHT))
                        .when(!first, |d| d.mt(px(HEADER_SPACE)))
                        .pl(px(TEXT_LEFT))
                        .flex()
                        .items_center()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(c.header)
                        .child(SharedString::from(group.to_uppercase()))
                        .into_any_element(),
                );
            }
            let caps = window
                .highest_precedence_binding_for_action_in(&*command.action, &target)
                .and_then(|b| b.keystrokes().first().map(|k| keycaps(k.inner())))
                .unwrap_or_default();
            children.push(
                div()
                    .id(ix)
                    .flex_none()
                    .mx(px(ROW_INSET))
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .rounded(px(ROW_RADIUS))
                    .when(ix == selected, |d| d.bg(c.selected))
                    .child(
                        div()
                            .w(px(TEXT_LEFT - ROW_INSET))
                            .flex_none()
                            .pl(px(10.5))
                            .text_color(hsla(if t.meta.mode == Mode::Dark {
                                Rgba::rgb(0x6fbf7f)
                            } else {
                                Rgba::rgb(CHECK)
                            }))
                            .when(command.checked, |d| d.child("✓")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c.text)
                            .child(SharedString::from(command.title)),
                    )
                    .child(div().flex_none().pr(px(13.)).flex().gap(px(5.)).children(
                        caps.into_iter().map(|cap| {
                            div()
                                .min_w(px(KEYCAP_WIDTH))
                                .h(px(KEYCAP_HEIGHT))
                                .px(px(4.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.))
                                .border(px(0.5))
                                .border_color(c.keycap_border)
                                .bg(c.keycap)
                                .text_size(px(11.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(c.keycap_text)
                                .child(cap)
                        }),
                    ))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.palette.as_mut()
                            && p.selected != ix
                        {
                            p.selected = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_palette_command(ix, false, window, cx)
                    }))
                    .into_any_element(),
            );
        }
        if commands.is_empty() {
            children.push(
                div()
                    .h(px(ROW_HEIGHT))
                    .pl(px(TEXT_LEFT))
                    .flex()
                    .items_center()
                    .text_color(c.placeholder)
                    .child("No matching commands")
                    .into_any_element(),
            );
        }

        let query = palette.query.clone();
        let caret = div()
            .flex_none()
            .w(px(1.5))
            .h(px(18.))
            .bg(hsla(Rgba::rgb(CARET)));
        let search = div()
            .flex_none()
            .h(px(SEARCH_HEIGHT))
            .flex()
            .items_center()
            .child(
                div()
                    .w(px(CARET_LEFT))
                    .flex_none()
                    .pl(px(15.))
                    .child(magnifier(c.placeholder)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pr(px(16.))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .text_size(px(14.5))
                    .map(|d| {
                        // The caret sits before the placeholder, after typed text.
                        if query.is_empty() {
                            d.child(caret).child(
                                div()
                                    .text_color(c.placeholder)
                                    .child("Search for commands..."),
                            )
                        } else {
                            d.child(div().text_color(c.text).child(SharedString::from(query)))
                                .child(caret.ml(px(0.5)))
                        }
                    }),
            );

        let card = div()
            .id("command-palette")
            .key_context("CommandPalette")
            .track_focus(&palette.focus)
            .on_key_down(cx.listener(Self::on_palette_key))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .map(|t| t.replace(['\n', '\r'], " "));
                if let Some(text) = text {
                    this.edit_palette_query(cx, |q| q.push_str(&text));
                }
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                if let Some(p) = &this.palette {
                    cx.write_to_clipboard(ClipboardItem::new_string(p.query.clone()));
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(WIDTH))
            .max_w(gpui::relative(1.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(RADIUS))
            .border(px(0.5))
            .border_color(c.border)
            .bg(c.card)
            .shadow(vec![BoxShadow {
                color: c.shadow,
                offset: point(px(0.), px(10.)),
                blur_radius: px(30.),
                spread_radius: px(0.),
                inset: false,
            }])
            .text_size(px(13.))
            .cursor(gpui::CursorStyle::Arrow)
            .child(search)
            .child(div().flex_none().h(px(1.)).bg(c.separator))
            .child(
                div()
                    .id("palette-list")
                    .max_h(px(LIST_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&palette.scroll)
                    .pb(px(ROW_INSET))
                    .when(!grouped, |d| d.pt(px(6.)))
                    .flex()
                    .flex_col()
                    .gap(px(ROW_GAP))
                    .children(children),
            );

        // A click outside the card closes it.
        Some(
            div()
                .id("palette-scrim")
                .absolute()
                .inset_0()
                .px(px(16.))
                .pt(px(TOP))
                .flex()
                .flex_col()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
                )
                .child(card),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn scores_runs_and_word_starts_above_scattered_letters() {
        assert!(score("spr", "Split Right").is_some());
        assert!(score("xyz", "Split Right").is_none());
        let run = score("split", "Split Right").unwrap();
        let initials = score("sr", "Split Right").unwrap();
        let scattered = score("pt", "Split Right").unwrap();
        assert!(run > initials && initials > scattered);
        assert!(score("NEW TAB", "New Tab") > score("NEW TAB", "Reopen Closed Tab"));
    }
}
