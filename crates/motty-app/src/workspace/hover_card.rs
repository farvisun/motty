//! The card you get by hovering a tab: its title, what it's doing (an
//! agent's step, or a plain terminal's last two lines of output; a file tab's
//! opening lines) and its folder.

use gpui::{App, Context, FontWeight, Hsla, SharedString, Window, div, prelude::*, px};
use motty_term::Session;

use super::picker::Colors;
use super::{Content, Workspace};

const WIDTH: f32 = 300.;

pub(super) struct HoverCard {
    title: SharedString,
    lines: Vec<SharedString>,
    folder: Option<SharedString>,
    mono: SharedString,
    card: Hsla,
    border: Hsla,
    text: Hsla,
    secondary: Hsla,
    shadow: Hsla,
}

impl Workspace {
    /// The hover card for tab `ix`, read when it's about to show.
    pub(super) fn hover_card(&self, ix: usize, cx: &App) -> Option<HoverCard> {
        let tab = self.tabs.get(ix)?;
        let summary = self.tab_summary(tab, cx);
        let (lines, folder) = match &tab.content {
            Content::Terminals(t) => {
                let view = t.focused.read(cx);
                let lines = match summary.line {
                    Some(line) => vec![line],
                    None => output_tail(view.session(), view.busy, 2),
                };
                (lines, view.cwd.clone())
            }
            Content::Editor(e) => {
                let e = e.read(cx);
                (e.opening_lines(4), e.folder())
            }
        };
        let c = Colors::new(&self.theme);
        Some(HoverCard {
            title: summary.title.into(),
            lines: lines
                .into_iter()
                .map(|l| SharedString::from(l.chars().take(200).collect::<String>()))
                .collect(),
            folder: folder.map(|f| super::details::tilde(&f).into()),
            mono: self.mono_font.clone(),
            card: c.card,
            border: c.border,
            text: c.text,
            secondary: c.placeholder,
            shadow: c.shadow,
        })
    }
}

/// A terminal's last `n` lines of output: the running command's, or the
/// last finished one's (so not the prompt that follows it).
fn output_tail(session: &Session, busy: bool, n: usize) -> Vec<String> {
    let finished = (!busy)
        .then(|| {
            let block = session.shell().last_finished().cloned()?;
            session.block_output(&block)
        })
        .flatten();
    let text = finished.unwrap_or_else(|| {
        let mut text = session.last_lines(8);
        // Idle without integration: the last line is the prompt.
        if !busy && let Some(cut) = text.rfind('\n') {
            text.truncate(cut);
        }
        text
    });
    let mut lines: Vec<String> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .map(str::to_owned)
        .collect();
    lines.drain(..lines.len().saturating_sub(n));
    lines
}

impl Render for HoverCard {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(WIDTH))
            .p(px(10.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .rounded(px(8.))
            .border(px(0.5))
            .border_color(self.border)
            .bg(self.card)
            .shadow(vec![gpui::BoxShadow {
                color: self.shadow,
                offset: gpui::point(px(0.), px(4.)),
                blur_radius: px(14.),
                spread_radius: px(0.),
                inset: false,
            }])
            .text_color(self.text)
            .child(
                div()
                    .truncate()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title.clone()),
            )
            .when(!self.lines.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .font_family(self.mono.clone())
                        .text_size(px(11.))
                        .text_color(self.secondary)
                        .children(self.lines.iter().map(|l| div().truncate().child(l.clone()))),
                )
            })
            .children(self.folder.clone().map(|f| {
                div()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(self.secondary)
                    .child(f)
            }))
    }
}
