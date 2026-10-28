//! What the command palette and Open Quickly share: Otty's floating card
//! with a search field over a list of 34pt rows, its colors, and fuzzy
//! matching.

use gpui::{BoxShadow, Div, Hsla, Keystroke, SharedString, Window, div, point, prelude::*, px};
use motty_render::hsla;
use motty_theme::{Mode, Rgba, Theme};

use crate::titlebar::{icon, stroke};

pub(super) const RADIUS: f32 = 14.;
pub(super) const SEARCH_HEIGHT: f32 = 46.;
/// Where the search caret starts.
pub(super) const CARET_LEFT: f32 = 41.;
pub(super) const ROW_HEIGHT: f32 = 34.;
pub(super) const ROW_GAP: f32 = 2.;
pub(super) const ROW_INSET: f32 = 8.;
pub(super) const ROW_RADIUS: f32 = 7.;
/// Plus the row gap; later headers get [`HEADER_SPACE`] more above.
pub(super) const HEADER_HEIGHT: f32 = 26.;
pub(super) const HEADER_SPACE: f32 = 1.5;
const CARET: u32 = 0x047afe;

/// Otty puts the card 15% of the way down the window.
pub(super) fn top(window: &Window) -> f32 {
    f32::from(window.viewport_size().height) * 0.15
}

/// How well `query` matches `text`: its characters in order, ignoring case,
/// scoring runs and word starts. `None` when it doesn't match.
pub(super) fn score(query: &str, text: &str) -> Option<i32> {
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

pub(super) struct Colors {
    pub card: Hsla,
    pub border: Hsla,
    pub separator: Hsla,
    pub text: Hsla,
    pub placeholder: Hsla,
    pub header: Hsla,
    pub selected: Hsla,
    pub keycap: Hsla,
    pub keycap_border: Hsla,
    pub keycap_text: Hsla,
    pub shadow: Hsla,
}

impl Colors {
    /// Measured from Otty's light palette (#f5f4f0 card, #e1e0de row,
    /// #1a1a1a text, #c1beb5 headers); dark themes mirror the alphas.
    pub(super) fn new(theme: &Theme) -> Self {
        let panel = theme.panel.background.with_alpha(1.);
        let dark = theme.meta.mode == Mode::Dark;
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
pub(super) fn magnifier(color: Hsla) -> impl IntoElement {
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

/// Applies a typing key to `query`: text, ⌫, ⌥⌫ (a word). False for keys
/// that aren't editing.
pub(super) fn edit_query(query: &mut String, ks: &Keystroke) -> bool {
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

/// The search field: magnifier, then the caret before the placeholder or
/// after typed text.
pub(super) fn search_row(query: &str, placeholder: &'static str, c: &Colors) -> Div {
    let caret = div()
        .flex_none()
        .w(px(1.5))
        .h(px(18.))
        .bg(hsla(Rgba::rgb(CARET)));
    let query = query.to_owned();
    let (text, placeholder_color) = (c.text, c.placeholder);
    div()
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
                    if query.is_empty() {
                        d.child(caret)
                            .child(div().text_color(placeholder_color).child(placeholder))
                    } else {
                        d.child(div().text_color(text).child(SharedString::from(query)))
                            .child(caret.ml(px(0.5)))
                    }
                }),
        )
}

/// The card itself, `width` wide (narrower windows shrink it).
pub(super) fn card(width: f32, c: &Colors) -> Div {
    div()
        .w(px(width))
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
}

/// A section header (`first` sits right under the search field).
pub(super) fn header(text: &str, left: f32, first: bool, c: &Colors) -> Div {
    div()
        .flex_none()
        .h(px(HEADER_HEIGHT))
        .when(!first, |d| d.mt(px(HEADER_SPACE)))
        .pl(px(left))
        .flex()
        .items_center()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(c.header)
        .child(SharedString::from(text.to_uppercase()))
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
