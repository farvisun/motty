//! Themes: the picker (View › Theme › Switch Theme…, palette Switch
//! Theme), which previews the selected theme live and keeps it on Return;
//! Import Theme with its preview; and Open Theme File.

use gpui::{
    AnyElement, Context, FocusHandle, Hsla, KeyDownEvent, MouseButton, PathPromptOptions,
    SharedString, Window, div, prelude::*, px,
};
use motty_render::hsla;
use motty_theme::import::{self, Format, Imported};
use motty_theme::{Mode, Rgba};
use toml::Value;

use super::Workspace;
use super::picker::{self, Colors, edit_query};

const WIDTH: f32 = 620.;
const COLUMNS: usize = 3;
const TILE_HEIGHT: f32 = 62.;
const GAP: f32 = 8.;
const LIST_HEIGHT: f32 = 430.;

pub(super) struct ThemePicker {
    focus: FocusHandle,
    query: String,
    themes: Vec<Swatch>,
    /// Index into the filtered list.
    selected: usize,
    /// What the list previewed last, so moving back doesn't reload.
    showing: Option<String>,
}

pub(super) struct Swatch {
    name: String,
    mode: Mode,
    background: Rgba,
    foreground: Rgba,
    palette: [Rgba; 16],
}

/// An import waiting for Import or Cancel.
pub(super) struct ImportPreview {
    focus: FocusHandle,
    theme: Imported,
    /// "Switch to it now".
    switch: bool,
}

impl Workspace {
    pub(super) fn open_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.theme.meta.name.clone();
        let themes: Vec<Swatch> = motty_theme::config::list_themes()
            .into_iter()
            .filter_map(|(name, mode, _)| {
                let t = motty_theme::config::find_theme(&name)?.theme;
                Some(Swatch {
                    name,
                    mode,
                    background: t.terminal.background,
                    foreground: t.terminal.foreground,
                    palette: t.terminal.palette,
                })
            })
            .collect();
        let mut picker = ThemePicker {
            focus: cx.focus_handle(),
            query: String::new(),
            themes,
            selected: 0,
            showing: Some(current.clone()),
        };
        picker.selected = picker
            .filtered()
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(&current))
            .unwrap_or(0);
        window.focus(&picker.focus, cx);
        self.theme_picker = Some(picker);
        cx.notify();
    }

    /// Shows the selected theme without saving it.
    fn preview_selected(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self
            .theme_picker
            .as_ref()
            .and_then(|p| p.filtered().get(p.selected).map(|s| s.name.clone()))
        else {
            return;
        };
        let picker = self.theme_picker.as_mut().expect("picker is open");
        if picker.showing.as_deref() == Some(name.as_str()) {
            return;
        }
        picker.showing = Some(name.clone());
        let mut config = self.config.clone();
        config.theme = Some(name.clone());
        config.theme_dark = Some(name);
        let parsed = config.resolve_theme(self.mode);
        self.show_theme(parsed.theme, cx);
    }

    /// Return keeps the theme; Escape (or a click outside) goes back.
    fn close_theme_picker(&mut self, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.theme_picker.take() else {
            return;
        };
        let chosen = picker
            .filtered()
            .get(picker.selected)
            .map(|s| s.name.clone());
        match chosen.filter(|_| keep) {
            Some(name) => self.set_theme(&name, cx),
            None => self.reload_theme(cx),
        }
        self.activate(self.active, window, cx);
    }

    /// Saves `name` as the theme: in the dark slot while the system is
    /// dark and a separate dark theme is set, else the main one.
    pub(super) fn set_theme(&mut self, name: &str, cx: &mut Context<Self>) {
        let key = if self.mode == Mode::Dark && self.config.theme_dark.is_some() {
            "theme-dark"
        } else {
            "theme"
        };
        match motty_theme::config::save(&[(key, Some(Value::String(name.to_owned())))]) {
            Ok(_) => crate::workspace::reload_all(cx),
            Err(e) => tracing::error!("couldn't save the theme: {e}"),
        }
    }

    /// The theme's file in the editor; a built-in one is copied into the
    /// themes folder first, where the copy then takes its place.
    pub(super) fn open_theme_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.theme.meta.name.clone();
        let path = motty_theme::config::list_themes()
            .into_iter()
            .find(|(n, ..)| n.eq_ignore_ascii_case(&name))
            .and_then(|(.., path)| path);
        let path = match path {
            Some(path) => path,
            None => {
                let Some(src) = motty_theme::config::builtin_source(&name) else {
                    return;
                };
                let dir = motty_theme::config::user_themes_dir();
                let path = dir.join(format!("{}.ottytheme", import::slug(&name)));
                if std::fs::create_dir_all(&dir)
                    .and_then(|_| std::fs::write(&path, src))
                    .is_err()
                {
                    return;
                }
                path
            }
        };
        self.open_editor(Some(path), window, cx);
    }

    /// Import Theme: a file of the given kind (or any), then its preview.
    pub(super) fn import_theme(
        &mut self,
        format: Option<Format>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.preview_import(&path, format, window, cx)
            });
        })
        .detach();
    }

    /// Reads and converts a theme file, then asks.
    pub(crate) fn preview_import(
        &mut self,
        path: &std::path::Path,
        format: Option<Format>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let converted = std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                let text = String::from_utf8_lossy(&bytes);
                let format = format.unwrap_or_else(|| Format::detect(path, &text));
                import::convert(format, path, &text)
            });
        match converted {
            Ok(theme) => {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                self.import_preview = Some(ImportPreview {
                    focus,
                    theme,
                    switch: true,
                });
                cx.notify();
            }
            Err(e) => drop(window.prompt(
                gpui::PromptLevel::Warning,
                "This theme can't be imported",
                Some(&format!("{}: {e}", path.display())),
                &["OK"],
                cx,
            )),
        }
    }

    fn finish_import(&mut self, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preview) = self.import_preview.take() else {
            return;
        };
        if keep {
            match import::save(&preview.theme, &motty_theme::config::user_themes_dir()) {
                Ok(_) if preview.switch => self.set_theme(&preview.theme.name, cx),
                Ok(_) => crate::workspace::reload_all(cx),
                Err(e) => tracing::error!("couldn't save the theme: {e}"),
            }
        }
        self.activate(self.active, window, cx);
        cx.notify();
    }

    fn on_theme_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let Some(picker) = self.theme_picker.as_mut() else {
            return;
        };
        cx.stop_propagation();
        let count = picker.filtered().len();
        let step = |by: isize, p: &mut ThemePicker| {
            if count > 0 {
                p.selected = (p.selected as isize + by).clamp(0, count as isize - 1) as usize;
            }
        };
        match ks.key.as_str() {
            "escape" => return self.close_theme_picker(false, window, cx),
            "enter" => return self.close_theme_picker(true, window, cx),
            "left" => step(-1, picker),
            "right" => step(1, picker),
            "up" => step(-(COLUMNS as isize), picker),
            "down" => step(COLUMNS as isize, picker),
            _ => {
                if !edit_query(&mut picker.query, ks) {
                    return;
                }
                picker.selected = 0;
            }
        }
        self.preview_selected(cx);
        cx.notify();
    }

    fn on_import_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        match event.keystroke.key.as_str() {
            "escape" => self.finish_import(false, window, cx),
            "enter" => self.finish_import(true, window, cx),
            _ => {}
        }
    }

    pub(super) fn render_theme_picker(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(preview) = &self.import_preview {
            return Some(self.render_import_preview(preview, window, cx));
        }
        let p = self.theme_picker.as_ref()?;
        let c = Colors::new(&self.theme);
        let filtered = p.filtered();
        let accent = hsla(Rgba::rgb(0x047afe));
        let mut sections: Vec<AnyElement> = Vec::new();
        let mut index = 0;
        for (title, mode) in [("Dark", Mode::Dark), ("Light", Mode::Light)] {
            let tiles: Vec<AnyElement> = filtered
                .iter()
                .enumerate()
                .filter(|(_, s)| s.mode == mode)
                .map(|(ix, s)| {
                    index += 1;
                    tile(s, ix == p.selected, accent, &c)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(p) = this.theme_picker.as_mut() {
                                p.selected = ix;
                            }
                            this.preview_selected(cx);
                            this.close_theme_picker(true, window, cx);
                        }))
                        .into_any_element()
                })
                .collect();
            if tiles.is_empty() {
                continue;
            }
            sections.push(
                picker::header(title, picker::ROW_INSET + 4., sections.is_empty(), &c)
                    .into_any_element(),
            );
            sections.push(
                div()
                    .px(px(picker::ROW_INSET + 4.))
                    .flex()
                    .flex_wrap()
                    .gap(px(GAP))
                    .children(tiles)
                    .into_any_element(),
            );
        }
        let empty = index == 0;
        let card = picker::card(WIDTH, &c)
            .id("theme-picker")
            .key_context("ThemePicker")
            .track_focus(&p.focus)
            .on_key_down(cx.listener(Self::on_theme_key))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(picker::search_row(&p.query, "Switch Theme", &c))
            .child(div().flex_none().h(px(1.)).bg(c.separator))
            .child(
                div()
                    .id("theme-list")
                    .max_h(px(LIST_HEIGHT))
                    .overflow_y_scroll()
                    .pb(px(12.))
                    .children(sections)
                    .when(empty, |d| {
                        d.child(
                            div()
                                .p(px(16.))
                                .text_color(c.placeholder)
                                .child("No matching themes"),
                        )
                    }),
            );
        Some(scrim(
            "theme-scrim",
            window,
            card.into_any_element(),
            cx,
            |this, window, cx| this.close_theme_picker(false, window, cx),
        ))
    }

    fn render_import_preview(
        &self,
        preview: &ImportPreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = Colors::new(&self.theme);
        let t = &preview.theme;
        let swatch = Swatch {
            name: t.name.clone(),
            mode: t.mode,
            background: t.background,
            foreground: t.foreground,
            palette: t.palette,
        };
        let accent = hsla(Rgba::rgb(0x047afe));
        let check = div()
            .id("import-switch")
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .size(px(14.))
                    .rounded(px(4.))
                    .border(px(1.))
                    .border_color(if preview.switch { accent } else { c.border })
                    .when(preview.switch, |d| d.bg(accent))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.))
                    .text_color(gpui::white())
                    .when(preview.switch, |d| d.child("✓")),
            )
            .child("Switch to it now")
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(p) = this.import_preview.as_mut() {
                    p.switch = !p.switch;
                }
                cx.notify();
            }));
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(26.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(px(7.))
                .border(px(0.5))
                .border_color(if primary { accent } else { c.border })
                .bg(if primary { accent } else { c.keycap })
                .text_color(if primary { gpui::white() } else { c.text })
                .child(label)
        };
        let card = picker::card(420., &c)
            .id("import-preview")
            .key_context("ImportTheme")
            .track_focus(&preview.focus)
            .on_key_down(cx.listener(Self::on_import_key))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .p(px(16.))
            .gap(px(12.))
            .text_color(c.text)
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(SharedString::from(format!("Import “{}”", t.name))),
            )
            .child(
                div()
                    .text_color(c.placeholder)
                    .child(SharedString::from(format!(
                        "A {} theme, saved to your themes folder.",
                        if t.mode == Mode::Dark {
                            "dark"
                        } else {
                            "light"
                        }
                    ))),
            )
            .child(tile(&swatch, false, accent, &c).w_full())
            .child(check)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(button("import-cancel", "Cancel", false).on_click(
                        cx.listener(|this, _, window, cx| this.finish_import(false, window, cx)),
                    ))
                    .child(button("import-ok", "Import", true).on_click(
                        cx.listener(|this, _, window, cx| this.finish_import(true, window, cx)),
                    )),
            );
        scrim(
            "import-scrim",
            window,
            card.into_any_element(),
            cx,
            |this, window, cx| this.finish_import(false, window, cx),
        )
    }
}

impl ThemePicker {
    /// The themes the query matches, best first; all of them, by name,
    /// when there's no query.
    fn filtered(&self) -> Vec<&Swatch> {
        let mut out: Vec<(i32, &Swatch)> = self
            .themes
            .iter()
            .filter_map(|s| {
                if self.query.is_empty() {
                    Some((0, s))
                } else {
                    picker::score(&self.query, &s.name).map(|score| (score, s))
                }
            })
            .collect();
        // Dark ones first, as the list shows them.
        out.sort_by_key(|(score, s)| (s.mode != Mode::Dark, -score));
        out.into_iter().map(|(_, s)| s).collect()
    }
}

/// A theme as a small terminal: its background, its name in its text
/// color, and its palette.
fn tile(s: &Swatch, selected: bool, accent: Hsla, c: &Colors) -> gpui::Stateful<gpui::Div> {
    // The card's border takes a point on each side.
    let inner = WIDTH - 2. - 2. * (picker::ROW_INSET + 4.);
    let tile_width = ((inner - GAP * (COLUMNS as f32 - 1.)) / COLUMNS as f32).floor();
    div()
        .id(SharedString::from(format!("theme-{}", s.name)))
        .w(px(tile_width))
        .h(px(TILE_HEIGHT))
        .px(px(10.))
        .py(px(8.))
        .flex()
        .flex_col()
        .justify_between()
        .rounded(px(8.))
        .bg(hsla(s.background))
        .border(px(if selected { 2. } else { 1. }))
        .border_color(if selected { accent } else { c.border })
        .child(
            div()
                .truncate()
                .text_size(px(12.5))
                .text_color(hsla(s.foreground))
                .child(SharedString::from(s.name.clone())),
        )
        .child(
            div().flex().gap(px(4.)).children(
                (1..7)
                    .chain([9, 12])
                    .map(|i| div().size(px(10.)).rounded_full().bg(hsla(s.palette[i]))),
            ),
        )
}

/// The dimmed backdrop a picker sits on; a click on it closes.
fn scrim(
    id: &'static str,
    window: &Window,
    card: AnyElement,
    cx: &mut Context<Workspace>,
    close: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .absolute()
        .inset_0()
        .px(px(16.))
        .pt(px(picker::top(window)))
        .flex()
        .flex_col()
        .items_center()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| close(this, window, cx)),
        )
        .child(card)
        .into_any_element()
}

/// `ImportTheme`'s format, by name.
pub(super) fn format_named(name: &str) -> Option<Format> {
    Format::ALL
        .into_iter()
        .find(|f| f.name().eq_ignore_ascii_case(name))
}
