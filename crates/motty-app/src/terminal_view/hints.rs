//! Hint mode: every link (or path, word, IP, hash) on screen gets a short
//! label; typing it opens, copies, pastes or reveals that target. Shift on
//! the last letter copies instead. Labels sit in the cell before their
//! target, yellow on a dark outline like Otty's.

use gpui::{ClipboardItem, Context, KeyDownEvent, SharedString, Window};
use motty_term::alacritty_terminal::index::{Column, Point};
use motty_term::input;
use motty_term::links::{self, HintTarget, Matcher, Target};

use super::TerminalView;

/// Label letters, home row first (as in Vimium, which Otty follows).
const ALPHABET: &str = "sadfjklewcmpgh";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HintAction {
    #[default]
    Open,
    Copy,
    Paste,
    Reveal,
}

pub struct HintState {
    action: HintAction,
    targets: Vec<(String, HintTarget)>,
    typed: String,
}

/// `n` labels of equal length, so none is a prefix of another.
fn labels(n: usize) -> Vec<String> {
    let a: Vec<char> = ALPHABET.chars().collect();
    let k = a.len();
    let len = if n <= k {
        1
    } else if n <= k * k {
        2
    } else {
        3
    };
    (0..n.min(k.pow(3)))
        .map(|i| {
            (0..len)
                .rev()
                .map(|place| a[(i / k.pow(place)) % k])
                .collect()
        })
        .collect()
}

impl TerminalView {
    pub fn start_hints(
        &mut self,
        matcher: Matcher,
        action: HintAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = {
            let term = self.session.term.lock();
            links::hint_targets(&*term, matcher, self.cwd.as_deref())
        };
        if targets.is_empty() {
            window.play_system_bell();
            return;
        }
        self.hints = Some(HintState {
            action,
            targets: labels(targets.len()).into_iter().zip(targets).collect(),
            typed: String::new(),
        });
        cx.notify();
    }

    pub(super) fn stop_hints(&mut self, cx: &mut Context<Self>) {
        self.hints = None;
        cx.notify();
    }

    pub(super) fn hint_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        if ks.modifiers.platform {
            return;
        }
        cx.stop_propagation();
        let Some(hints) = self.hints.as_mut() else {
            return;
        };
        match ks.key.as_str() {
            "escape" => return self.stop_hints(cx),
            "backspace" => {
                hints.typed.pop();
            }
            key => {
                let Some(c) = key.chars().next().filter(|c| ALPHABET.contains(*c)) else {
                    return;
                };
                let typed = format!("{}{c}", hints.typed);
                if !hints.targets.iter().any(|(l, _)| l.starts_with(&typed)) {
                    window.play_system_bell();
                    return;
                }
                if let Some((_, target)) = hints.targets.iter().find(|(l, _)| *l == typed) {
                    let target = target.clone();
                    let action = if ks.modifiers.shift {
                        HintAction::Copy
                    } else {
                        hints.action
                    };
                    self.hints = None;
                    self.run_hint(&target, action, cx);
                } else {
                    hints.typed = typed;
                }
            }
        }
        cx.notify();
    }

    fn run_hint(&mut self, target: &HintTarget, action: HintAction, cx: &mut Context<Self>) {
        let copy_text = match &target.link {
            Some(Target::Url(url)) => url.clone(),
            Some(Target::Path { path, .. }) => path.display().to_string(),
            None => target.text.clone(),
        };
        match (action, &target.link) {
            (HintAction::Open, Some(Target::Url(url))) => cx.open_url(url),
            (HintAction::Open, Some(Target::Path { path, .. })) => cx.open_with_system(path),
            (HintAction::Reveal, Some(Target::Path { path, .. })) => cx.reveal_path(path),
            (HintAction::Paste, _) if !self.read_only => {
                let bytes = input::paste_bytes(&target.text, self.term_mode());
                self.session.write(bytes);
            }
            (HintAction::Copy | HintAction::Open | HintAction::Reveal, _) => {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_text))
            }
            (HintAction::Paste, _) => {}
        }
    }

    /// Targets still matching what's typed.
    fn live_hints(&self) -> impl Iterator<Item = &(String, HintTarget)> {
        self.hints.iter().flat_map(|h| {
            h.targets
                .iter()
                .filter(move |(label, _)| label.starts_with(&h.typed))
        })
    }

    /// Underlined ranges: hint targets, and the link under the pointer.
    pub(super) fn underlines(&self) -> Vec<(Point, Point)> {
        self.live_hints()
            .map(|(_, t)| (t.start, t.end))
            .chain(self.hovered_link.as_ref().map(|l| (l.start, l.end)))
            .collect()
    }

    /// The untyped rest of each label, ending just before its target (or
    /// over its start at the left edge).
    pub(super) fn hint_labels(&self) -> Vec<(Point, SharedString)> {
        let typed = self.hints.as_ref().map_or(0, |h| h.typed.len());
        self.live_hints()
            .map(|(label, t)| {
                let rest = &label[typed..];
                let col = t
                    .start
                    .column
                    .0
                    .checked_sub(rest.len())
                    .unwrap_or(t.start.column.0);
                (
                    Point::new(t.start.line, Column(col)),
                    SharedString::from(rest.to_owned()),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::labels;

    #[test]
    fn labels_are_unique_and_prefix_free() {
        assert_eq!(labels(3), ["s", "a", "d"]);
        let two = labels(20);
        assert_eq!((two[0].as_str(), two[19].as_str()), ("ss", "ak"));
        let mut sorted = labels(300);
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 300);
    }
}
