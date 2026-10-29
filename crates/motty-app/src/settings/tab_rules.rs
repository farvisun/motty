//! Settings › View › Tabs & Title: the `tab-rule` editor. Rules are listed
//! in priority order on the left; the selected one's condition, alias,
//! icon and title template are edited on the right.

use gpui::{AnyElement, Context, Hsla, Keystroke, SharedString, div, prelude::*, px, svg};
use motty_render::hsla;
use motty_theme::{Rgba, TabRule};

use super::{Colors, SettingsWindow};

/// What a rule matches on, in the order the picker shows them.
const WHEN: [(&str, &str); 5] = [
    ("Folder", "path"),
    ("Command", "command"),
    ("Agent", "agent"),
    ("SSH Host", "host"),
    ("File", "file"),
];

const PLACEHOLDER: [&str; 5] = ["~/Work/project", "ssh *", "claude", "db-*", "*.md"];

/// Click one to put it in the title template.
const VARIABLES: [&str; 12] = [
    "alias", "folder", "cwd", "branch", "title", "command", "agent", "user", "host", "shell",
    "file", "index",
];

const COLORS: [&str; 8] = [
    "red", "green", "yellow", "blue", "magenta", "cyan", "white", "black",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Pattern,
    Alias,
    /// An emoji typed in place of an icon.
    Emoji,
    Title,
}

pub(super) struct RuleEditor {
    rules: Vec<TabRule>,
    selected: usize,
    field: Option<Field>,
    /// The palette, for the color swatches.
    palette: [Rgba; 16],
}

impl RuleEditor {
    fn rule(&mut self) -> Option<&mut TabRule> {
        self.rules.get_mut(self.selected)
    }

    fn kind(rule: &TabRule) -> usize {
        rule.condition()
            .and_then(|(k, _)| WHEN.iter().position(|(_, key)| *key == k))
            .unwrap_or(0)
    }

    fn pattern(rule: &mut TabRule, kind: usize) -> &mut Option<String> {
        match WHEN[kind].1 {
            "command" => &mut rule.command,
            "agent" => &mut rule.agent,
            "host" => &mut rule.host,
            "file" => &mut rule.file,
            _ => &mut rule.path,
        }
    }

    fn text(&mut self, field: Field) -> Option<&mut Option<String>> {
        let rule = self.rule()?;
        Some(match field {
            Field::Pattern => {
                let kind = Self::kind(rule);
                Self::pattern(rule, kind)
            }
            Field::Alias => &mut rule.alias,
            Field::Emoji => &mut rule.icon,
            Field::Title => &mut rule.title,
        })
    }
}

impl SettingsWindow {
    pub(super) fn open_rule_editor(&mut self, field: Option<Field>, mode: motty_theme::Mode) {
        let palette = self.config.resolve_theme(mode).theme.terminal.palette;
        let rules = self.config.tab_rules.clone();
        self.rules = Some(RuleEditor {
            field: field.filter(|_| !rules.is_empty()),
            rules,
            selected: 0,
            palette,
        });
    }

    fn save_rules(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.rules.take() else {
            return;
        };
        // A rule that gives nothing would be refused when read back.
        let rules: Vec<_> = editor
            .rules
            .into_iter()
            .filter(|r| r.alias.is_some() || r.icon.is_some() || r.title.is_some())
            .collect();
        match motty_theme::config::save_tab_rules(&rules) {
            Ok(_) => {
                self.status = None;
                crate::workspace::reload_all(cx);
                self.config = crate::load_config();
            }
            Err(e) => self.status = Some((false, format!("Couldn't save config.toml: {e}"))),
        }
        cx.notify();
    }

    /// Typing into the focused field; true when the key was taken.
    pub(super) fn rule_editor_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.rules.as_mut() else {
            return false;
        };
        let m = &ks.modifiers;
        if ks.key == "escape" {
            if editor.field.take().is_none() {
                self.rules = None;
            }
            cx.notify();
            return true;
        }
        if ks.key == "enter" && m.platform {
            self.save_rules(cx);
            return true;
        }
        let Some(field) = editor.field else {
            return m.platform && ks.key == "w";
        };
        let paste = m.platform && ks.key == "v";
        let pasted = paste
            .then(|| cx.read_from_clipboard().and_then(|c| c.text()))
            .flatten();
        let Some(slot) = editor.text(field) else {
            return true;
        };
        let mut text = slot.take().unwrap_or_default();
        // Typing an emoji replaces a picked icon.
        if field == Field::Emoji && crate::tab_icons::path(&text).is_some() {
            text.clear();
        }
        match ks.key.as_str() {
            "enter" | "tab" => editor.field = None,
            "backspace" if m.alt || m.platform => {
                let keep = text.trim_end().rfind([' ', '/', '·']).map_or(0, |i| i + 1);
                text.truncate(if m.platform { 0 } else { keep });
            }
            "backspace" => {
                text.pop();
            }
            _ if paste => text.push_str(
                pasted
                    .as_deref()
                    .unwrap_or_default()
                    .lines()
                    .next()
                    .unwrap_or_default(),
            ),
            _ if m.platform || m.control => {}
            _ => {
                if let Some(c) = ks
                    .key_char
                    .as_deref()
                    .filter(|c| !c.chars().any(char::is_control))
                {
                    text.push_str(c);
                }
            }
        }
        let slot = editor.text(field).expect("the rule is still there");
        *slot = Some(text).filter(|t| !t.is_empty());
        cx.notify();
        true
    }

    pub(super) fn render_rule_editor(
        &self,
        e: &RuleEditor,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = |text: &'static str| {
            div()
                .flex_none()
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c.header)
                .child(text)
        };
        let button = |id: SharedString, text: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(26.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .when(primary, |d| {
                    d.bg(c.accent).text_color(hsla(Rgba::rgb(0xffffff)))
                })
                .when(!primary, |d| {
                    d.bg(c.control)
                        .border(px(0.5))
                        .border_color(c.control_border)
                })
                .shadow_xs()
                .child(text)
        };

        // The list, highest priority first.
        let list = e.rules.iter().enumerate().map(|(ix, rule)| {
            let (kind, pattern) = rule.condition().unwrap_or(("path", "Every tab"));
            let when = WHEN
                .iter()
                .find(|(_, k)| *k == kind)
                .map_or("", |(l, _)| *l);
            let gives = [
                rule.alias.as_deref().map(|a| format!("“{a}”")),
                rule.icon.clone(),
                rule.title.clone(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            div()
                .id(("rule", ix))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .gap(px(8.))
                .when(ix == e.selected, |d| d.bg(c.selected))
                .children(rule_icon(rule, &e.palette, c.text))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .child(SharedString::from(pattern.to_owned())),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(c.secondary)
                                .child(SharedString::from(format!("{when} · {gives}"))),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(e) = this.rules.as_mut() {
                        e.selected = ix;
                        e.field = None;
                    }
                    cx.notify();
                }))
        });
        let tools = div()
            .flex()
            .gap(px(4.))
            .child(
                button("rule-add".into(), "+", false).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(e) = this.rules.as_mut() {
                        e.rules.push(TabRule::default());
                        e.selected = e.rules.len() - 1;
                        e.field = Some(Field::Pattern);
                    }
                    cx.notify();
                })),
            )
            .child(
                button("rule-remove".into(), "−", false).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(e) = this.rules.as_mut()
                        && e.selected < e.rules.len()
                    {
                        e.rules.remove(e.selected);
                        e.selected = e.selected.min(e.rules.len().saturating_sub(1));
                        e.field = None;
                    }
                    cx.notify();
                })),
            )
            .child(div().flex_1())
            .child(
                button("rule-up".into(), "↑", false).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(e) = this.rules.as_mut()
                        && e.selected > 0
                    {
                        e.rules.swap(e.selected, e.selected - 1);
                        e.selected -= 1;
                    }
                    cx.notify();
                })),
            )
            .child(button("rule-down".into(), "↓", false).on_click(cx.listener(
                |this, _, _, cx| {
                    if let Some(e) = this.rules.as_mut()
                        && e.selected + 1 < e.rules.len()
                    {
                        e.rules.swap(e.selected, e.selected + 1);
                        e.selected += 1;
                    }
                    cx.notify();
                },
            )));
        let left = div()
            .w(px(210.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .id("rule-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(4.))
                    .rounded(px(8.))
                    .bg(c.field)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(list)
                    .when(e.rules.is_empty(), |d| {
                        d.child(
                            div()
                                .p(px(10.))
                                .text_color(c.secondary)
                                .child("No rules yet. Add one with +."),
                        )
                    }),
            )
            .child(tools)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(c.secondary)
                    .child("The topmost rule that matches wins, separately for the alias, the icon and the title."),
            );

        let right = e.rules.get(e.selected).map(|rule| {
            let kind = RuleEditor::kind(rule);
            let field = |id: &'static str, f: Field, value: Option<&str>, hint: &'static str| {
                let focused = e.field == Some(f);
                div()
                    .id(id)
                    .flex_none()
                    .h(px(28.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .bg(c.field)
                    .border(px(1.))
                    .border_color(if focused { c.accent } else { c.control_border })
                    .overflow_hidden()
                    // While typing, the end (and the caret) stays in view.
                    .when(focused, |d| d.justify_end())
                    .child(match value {
                        Some(v) => div().flex_none().child(SharedString::from(v.to_owned())),
                        None if focused => div(),
                        None => div().text_color(c.secondary).child(hint),
                    })
                    .when(focused, |d| {
                        d.child(div().flex_none().w(px(1.5)).h(px(15.)).bg(c.accent))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(e) = this.rules.as_mut() {
                            e.field = Some(f);
                        }
                        cx.notify();
                    }))
            };
            let segments = WHEN.iter().enumerate().map(|(ix, (name, _))| {
                div()
                    .id(("rule-when", ix))
                    .flex_1()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .when(ix == kind, |d| d.bg(c.control).shadow_xs())
                    .child(*name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(e) = this.rules.as_mut()
                            && let Some(rule) = e.rule()
                        {
                            let old = RuleEditor::kind(rule);
                            let value = RuleEditor::pattern(rule, old).take();
                            *RuleEditor::pattern(rule, ix) = value;
                            e.field = Some(Field::Pattern);
                        }
                        cx.notify();
                    }))
            });
            let icons = std::iter::once(None)
                .chain(crate::tab_icons::NAMES.iter().copied().map(Some))
                .map(|name| {
                    let on = rule.icon.as_deref() == name;
                    div()
                        .id(SharedString::from(format!(
                            "rule-icon-{}",
                            name.unwrap_or("none")
                        )))
                        .size(px(26.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .when(on, |d| {
                            d.bg(c.selected).border(px(1.)).border_color(c.accent)
                        })
                        .child(match name.and_then(crate::tab_icons::path) {
                            Some(path) => svg()
                                .path(path)
                                .size(px(16.))
                                .text_color(c.text)
                                .into_any_element(),
                            None => div()
                                .text_size(px(10.))
                                .text_color(c.secondary)
                                .child("None")
                                .into_any_element(),
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(e) = this.rules.as_mut()
                                && let Some(rule) = e.rule()
                            {
                                rule.icon = name.map(str::to_owned);
                                e.field = None;
                            }
                            cx.notify();
                        }))
                });
            let emoji = rule
                .icon
                .as_deref()
                .filter(|i| crate::tab_icons::path(i).is_none());
            let swatches = std::iter::once(None)
                .chain(COLORS.iter().copied().map(Some))
                .map(|name| {
                    let on = rule.icon_color.as_deref() == name;
                    let color = name
                        .and_then(|n| COLORS.iter().position(|c| *c == n))
                        .map(|i| hsla(e.palette[[1, 2, 3, 4, 5, 6, 7, 0][i]]));
                    div()
                        .id(SharedString::from(format!(
                            "rule-color-{}",
                            name.unwrap_or("none")
                        )))
                        .size(px(18.))
                        .rounded_full()
                        .border(px(if on { 2. } else { 1. }))
                        .border_color(if on { c.accent } else { c.control_border })
                        // No color: the tab's own text color, shown hollow.
                        .when_some(color, |d, color| d.bg(color))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(e) = this.rules.as_mut()
                                && let Some(rule) = e.rule()
                            {
                                rule.icon_color = name.map(str::to_owned);
                            }
                            cx.notify();
                        }))
                });
            let pills = VARIABLES.iter().map(|v| {
                div()
                    .id(SharedString::from(format!("rule-var-{v}")))
                    .px(px(7.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(c.field)
                    .text_size(px(11.))
                    .child(SharedString::from(format!("${{{v}}}")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(e) = this.rules.as_mut()
                            && let Some(rule) = e.rule()
                        {
                            let mut title = rule.title.take().unwrap_or_default();
                            title.push_str(&format!("${{{v}}}"));
                            rule.title = Some(title);
                            e.field = Some(Field::Title);
                        }
                        cx.notify();
                    }))
            });
            let pattern = match kind {
                0 => rule.path.as_deref(),
                1 => rule.command.as_deref(),
                2 => rule.agent.as_deref(),
                3 => rule.host.as_deref(),
                _ => rule.file.as_deref(),
            };
            div()
                .id("rule-form")
                .flex_1()
                .min_w_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(label("WHEN"))
                .child(
                    div()
                        .flex_none()
                        .p(px(2.))
                        .rounded(px(7.))
                        .bg(c.field)
                        .flex()
                        .children(segments),
                )
                .child(field(
                    "rule-pattern",
                    Field::Pattern,
                    pattern,
                    PLACEHOLDER[kind],
                ))
                .child(label("PROJECT ALIAS"))
                .child(field(
                    "rule-alias",
                    Field::Alias,
                    rule.alias.as_deref(),
                    "Otty",
                ))
                .child(label("ICON"))
                .child(
                    div()
                        .id("rule-icons")
                        .flex_none()
                        .h(px(84.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_wrap()
                        .gap(px(2.))
                        .children(icons),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().w(px(150.)).child(field(
                            "rule-emoji",
                            Field::Emoji,
                            emoji,
                            "Or type an emoji",
                        )))
                        .children(swatches),
                )
                .child(label("TITLE"))
                .child(field(
                    "rule-title",
                    Field::Title,
                    rule.title.as_deref(),
                    "${folder} · ${branch}",
                ))
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .children(pills),
                )
        });

        // The dialog fills the window below the traffic lights.
        let card =
            div()
                .id("rules-dialog")
                .occlude()
                .size_full()
                .p(px(16.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .rounded(px(12.))
                .bg(c.card)
                .border(px(1.))
                .border_color(c.separator)
                .shadow_lg()
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Tab Rules"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .gap(px(16.))
                        .child(left)
                        .children(right),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(button("rules-cancel".into(), "Cancel", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.rules = None;
                                cx.notify();
                            }),
                        ))
                        .child(
                            button("rules-save".into(), "Save", true)
                                .on_click(cx.listener(|this, _, _, cx| this.save_rules(cx))),
                        ),
                );
        div()
            .id("rules-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .pt(px(36.))
            .px(px(10.))
            .pb(px(10.))
            .bg(hsla(Rgba::rgb(0x000000).with_alpha(0.25)))
            .occlude()
            .child(card)
            .into_any_element()
    }
}

/// The rule's icon in its list row.
fn rule_icon(rule: &TabRule, palette: &[Rgba; 16], text: Hsla) -> Option<AnyElement> {
    let name = rule.icon.as_deref()?;
    let color = rule
        .icon_color
        .as_deref()
        .and_then(|n| COLORS.iter().position(|c| *c == n))
        .map_or(text, |i| hsla(palette[[1, 2, 3, 4, 5, 6, 7, 0][i]]));
    Some(match crate::tab_icons::path(name) {
        Some(path) => svg()
            .path(path)
            .flex_none()
            .size(px(16.))
            .text_color(color)
            .into_any_element(),
        None => div()
            .flex_none()
            .w(px(16.))
            .child(SharedString::from(name.to_owned()))
            .into_any_element(),
    })
}
