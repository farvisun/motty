//! Settings › Appearance › Edit Theme: the current theme's terminal colors.
//! Saving writes the theme to Motty's themes folder, where it takes the
//! place of a built-in theme or of Otty's file of the same name; Reset to
//! Built-in removes that copy again.

use gpui::{AnyElement, Context, Keystroke, SharedString, div, prelude::*, px};
use motty_render::hsla;
use motty_theme::import::{hex, slug};
use motty_theme::{Mode, Rgba};

use super::{Colors, SettingsWindow};

const NAMED: [(&str, &str); 5] = [
    ("Foreground", "foreground"),
    ("Background", "background"),
    ("Cursor", "cursor"),
    ("Cursor Text", "cursor-text"),
    ("Selection", "selection-background"),
];

const ANSI: [&str; 8] = [
    "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White",
];

pub(super) struct ThemeEditor {
    name: String,
    /// The theme file's text to start from.
    source: String,
    /// Foreground … selection, then the 16 palette colors.
    colors: Vec<Rgba>,
    /// The color being typed, and its text so far.
    editing: Option<(usize, String)>,
    error: Option<String>,
}

/// The theme shown for `mode`, by name.
pub(super) fn active_theme(config: &motty_theme::Config, mode: Mode) -> String {
    config
        .theme_name(mode)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if mode == Mode::Dark {
                "Motty Dark"
            } else {
                "Motty Light"
            }
            .to_owned()
        })
}

/// Motty's own copy of `name` that stands in for a built-in one.
pub(super) fn override_path(name: &str) -> Option<std::path::PathBuf> {
    motty_theme::config::builtin_source(name)?;
    let path = motty_theme::config::user_themes_dir().join(format!("{}.ottytheme", slug(name)));
    path.exists().then_some(path)
}

impl SettingsWindow {
    pub(super) fn open_theme_editor(&mut self, mode: Mode) {
        let name = active_theme(&self.config, mode);
        let Some(parsed) = motty_theme::config::find_theme(&name) else {
            self.status = Some((false, format!("Theme “{name}” wasn't found")));
            return;
        };
        let path = motty_theme::config::list_themes()
            .into_iter()
            .find(|(n, ..)| n.eq_ignore_ascii_case(&name))
            .and_then(|(.., p)| p);
        let source = path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .or_else(|| motty_theme::config::builtin_source(&name).map(str::to_owned))
            .unwrap_or_default();
        let t = &parsed.theme;
        let mut colors = vec![
            t.terminal.foreground,
            t.terminal.background,
            t.cursor.color,
            t.cursor.text.unwrap_or(t.terminal.background),
            t.terminal.selection_background,
        ];
        colors.extend(t.terminal.palette);
        self.theme_editor = Some(ThemeEditor {
            name: t.meta.name.clone(),
            source,
            colors,
            editing: None,
            error: None,
        });
    }

    fn save_theme_edits(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.theme_editor.as_mut() else {
            return;
        };
        let mut doc: toml::Table = match editor.source.parse() {
            Ok(doc) => doc,
            Err(e) => {
                editor.error = Some(format!("The theme file isn't valid TOML: {e}"));
                return cx.notify();
            }
        };
        let terminal = doc
            .entry("terminal")
            .or_insert_with(|| toml::Value::Table(Default::default()));
        let Some(terminal) = terminal.as_table_mut() else {
            editor.error = Some("[terminal] isn't a table".into());
            return cx.notify();
        };
        for (i, (_, key)) in NAMED.iter().enumerate() {
            terminal.insert((*key).into(), toml::Value::String(hex(editor.colors[i])));
        }
        terminal.insert(
            "palette".into(),
            toml::Value::Array(
                editor.colors[NAMED.len()..]
                    .iter()
                    .map(|c| toml::Value::String(hex(*c)))
                    .collect(),
            ),
        );
        let text = match toml::to_string_pretty(&doc) {
            Ok(text) => format!("# Edited in Motty's Settings.\n\n{text}"),
            Err(e) => {
                editor.error = Some(e.to_string());
                return cx.notify();
            }
        };
        let dir = motty_theme::config::user_themes_dir();
        let path = dir.join(format!("{}.ottytheme", slug(&editor.name)));
        match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text)) {
            Ok(()) => {
                self.theme_editor = None;
                self.status = Some((true, format!("Saved {}", path.display())));
                crate::workspace::reload_all(cx);
            }
            Err(e) => editor.error = Some(format!("Couldn't save: {e}")),
        }
        cx.notify();
    }

    /// Removes Motty's copy of a built-in theme.
    pub(super) fn reset_theme(&mut self, mode: Mode, cx: &mut Context<Self>) {
        let name = active_theme(&self.config, mode);
        if let Some(path) = override_path(&name) {
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    self.status = Some((true, format!("“{name}” is back to the built-in colors")));
                    crate::workspace::reload_all(cx);
                }
                Err(e) => self.status = Some((false, format!("Couldn't remove it: {e}"))),
            }
        }
        cx.notify();
    }

    /// Typing a color; true when the key was taken.
    pub(super) fn theme_editor_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.theme_editor.as_mut() else {
            return false;
        };
        let m = &ks.modifiers;
        match (ks.key.as_str(), editor.editing.as_mut()) {
            ("escape", None) => self.theme_editor = None,
            ("escape", Some(_)) => editor.editing = None,
            ("enter", None) if m.platform => {
                self.save_theme_edits(cx);
                return true;
            }
            ("enter" | "tab", Some((ix, text))) => {
                let ix = *ix;
                match motty_theme::css::parse_color(&format!(
                    "#{}",
                    text.trim().trim_start_matches('#')
                )) {
                    Ok(c) => {
                        editor.colors[ix] = c;
                        editor.error = None;
                        editor.editing = None;
                    }
                    Err(_) => editor.error = Some(format!("“{text}” isn't a color")),
                }
            }
            ("backspace", Some((_, text))) => {
                text.pop();
            }
            (_, Some((_, text))) if !m.platform && !m.control => {
                if let Some(c) = ks
                    .key_char
                    .as_deref()
                    .filter(|c| c.chars().all(|c| c.is_ascii_hexdigit() || c == '#'))
                {
                    text.push_str(c);
                }
            }
            _ => return m.platform && ks.key == "w",
        }
        cx.notify();
        true
    }

    pub(super) fn render_theme_editor(
        &self,
        e: &ThemeEditor,
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
        let swatch = |ix: usize, title: SharedString| {
            let color = e.colors[ix];
            let editing = e.editing.as_ref().filter(|(i, _)| *i == ix);
            let value: SharedString = match editing {
                Some((_, text)) => format!("#{}", text.trim_start_matches('#')).into(),
                None => hex(color).into(),
            };
            div()
                .id(("theme-color", ix))
                .w(px(150.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .p(px(4.))
                .rounded(px(6.))
                .when(editing.is_some(), |d| d.bg(c.selected))
                .child(
                    div()
                        .flex_none()
                        .size(px(22.))
                        .rounded(px(5.))
                        .border(px(1.))
                        .border_color(c.control_border)
                        .bg(hsla(color)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(12.)).child(title))
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_family("Menlo")
                                .text_color(if editing.is_some() {
                                    c.accent
                                } else {
                                    c.secondary
                                })
                                .child(value),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(e) = this.theme_editor.as_mut() {
                        e.editing = Some((ix, String::new()));
                    }
                    cx.notify();
                }))
        };
        let named = NAMED
            .iter()
            .enumerate()
            .map(|(i, (title, _))| swatch(i, (*title).into()));
        let normal = (0..8).map(|i| swatch(NAMED.len() + i, ANSI[i].into()));
        let bright =
            (0..8).map(|i| swatch(NAMED.len() + 8 + i, format!("Bright {}", ANSI[i]).into()));
        let button = |id: &'static str, text: &'static str, primary: bool| {
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
        let card = div()
            .id("theme-editor")
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
                    .child(SharedString::from(format!("Edit “{}”", e.name))),
            )
            .child(
                div()
                    .id("theme-colors")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(label("TERMINAL"))
                    .child(div().flex().flex_wrap().gap(px(4.)).children(named))
                    .child(label("NORMAL"))
                    .child(div().flex().flex_wrap().gap(px(4.)).children(normal))
                    .child(label("BRIGHT"))
                    .child(div().flex().flex_wrap().gap(px(4.)).children(bright)),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(c.secondary)
                    .child("Click a color and type its hex value, then Return. Saving keeps a copy in your themes folder."),
            )
            .children(e.error.clone().map(|err| {
                div()
                    .text_size(px(12.))
                    .text_color(c.error)
                    .child(SharedString::from(err))
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(button("theme-edit-cancel", "Cancel", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.theme_editor = None;
                            cx.notify();
                        },
                    )))
                    .child(
                        button("theme-edit-save", "Save", true)
                            .on_click(cx.listener(|this, _, _, cx| this.save_theme_edits(cx))),
                    ),
            );
        div()
            .id("theme-editor-backdrop")
            .absolute()
            .inset_0()
            .pt(px(36.))
            .px(px(10.))
            .pb(px(10.))
            .bg(hsla(Rgba::rgb(0x000000).with_alpha(0.25)))
            .occlude()
            .child(card)
            .into_any_element()
    }
}
