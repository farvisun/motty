//! Recipes in a window: the Save (⌘S) and Export (⇧⌘S) dialog, and
//! opening a saved recipe or a recipe file, asking first before a file
//! from elsewhere runs commands.

use std::path::Path;

use gpui::{
    AnyElement, Context, Entity, FocusHandle, Hsla, KeyDownEvent, MouseButton, PromptButton,
    PromptLevel, SharedString, Window, div, prelude::*, px,
};
use motty_theme::Replay;

use super::picker::{self, Colors, edit_query};
use super::{Tab, Workspace, tab_state};
use crate::actions::Paste;
use crate::recipes::{self, Bases, Layout, Meta, Portable, Recipe, Scope, Snippet, expand_snippet};
use crate::recovery::TabState;
use crate::terminal_view::TerminalView;

const WIDTH: f32 = 520.;
const FIELD_HEIGHT: f32 = 28.;
const ACCENT: u32 = 0x047afe;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Alias,
    Text,
}

pub(super) struct RecipeDialog {
    pub focus: FocusHandle,
    /// Export to a file instead of saving.
    export: bool,
    name: String,
    scope: Scope,
    include_commands: bool,
    portable: Portable,
    /// Commands to choose from, oldest first, and whether each is ticked.
    commands: Vec<(String, bool)>,
    alias: String,
    text: String,
    field: Field,
    error: Option<String>,
}

impl Workspace {
    /// File › Save… (⌘S): over an editor it saves the file.
    pub(super) fn save_recipe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.tabs.get(self.active).and_then(Tab::as_editor) {
            editor.update(cx, |e, cx| e.save(window, cx));
            return;
        }
        self.open_recipe_dialog(false, None, window, cx);
    }

    pub(super) fn export_recipe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_recipe_dialog(true, None, window, cx);
    }

    /// Save Selection to Recipe: the selection as a snippet, or its lines
    /// as commands.
    pub(super) fn save_selection_to_recipe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self
            .focused_pane()
            .and_then(|v| v.read(cx).session().term.lock().selection_to_string())
            .filter(|s| !s.trim().is_empty());
        let Some(selection) = selection else {
            window.play_system_bell();
            return;
        };
        self.open_recipe_dialog(false, Some(selection), window, cx);
    }

    /// Settings › Recipes › Text Snippet › Add…
    pub(crate) fn new_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_recipe_dialog(false, None, window, cx);
        if let Some(d) = self.recipe_dialog.as_mut() {
            d.scope = Scope::Snippet;
            d.name.clear();
        }
    }

    fn open_recipe_dialog(
        &mut self,
        export: bool,
        selection: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = self.focused_pane();
        let (commands, scope, text) = match &selection {
            Some(text) => (
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(|l| (l.to_owned(), true))
                    .collect(),
                Scope::Snippet,
                text.trim_end().to_owned(),
            ),
            None => (
                pane.as_ref()
                    .map(|p| p.read(cx).recent_commands())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|c| (c, false))
                    .collect(),
                if pane.is_some() {
                    Scope::Tab
                } else {
                    Scope::Commands
                },
                String::new(),
            ),
        };
        // The folder's name, which is what a layout is usually about.
        let name = self
            .active_folder(cx)
            .and_then(|f| f.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.recipe_dialog = Some(RecipeDialog {
            focus,
            export,
            name,
            scope,
            include_commands: false,
            portable: Portable::Off,
            commands,
            alias: String::new(),
            text,
            field: Field::Name,
            error: None,
        });
        cx.notify();
    }

    fn close_recipe_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recipe_dialog = None;
        self.activate(self.active, window, cx);
    }

    /// What the dialog would save, or why it can't.
    fn build_recipe(
        &self,
        d: &RecipeDialog,
        file: Option<&Path>,
        cx: &gpui::App,
    ) -> Result<Recipe, String> {
        let name = d.name.trim();
        if name.is_empty() {
            return Err("Give the recipe a name".into());
        }
        let mut recipe = Recipe {
            recipe: Meta {
                name: name.to_owned(),
                version: 1,
                scope: d.scope,
                commands: Vec::new(),
            },
            window: None,
            commands: Vec::new(),
            snippet: None,
        };
        let bases = Bases::new(self.active_folder(cx), file);
        match d.scope {
            Scope::Tab | Scope::Window => {
                let tabs: Vec<&Tab> = match d.scope {
                    Scope::Tab => self.tabs.get(self.active).into_iter().collect(),
                    _ => self.tabs.iter().collect(),
                };
                let layouts: Vec<_> = tabs
                    .into_iter()
                    .filter_map(|tab| {
                        let state: TabState = tab_state(tab, None, cx)?;
                        let commands: Vec<Vec<String>> = tab
                            .terms()?
                            .root
                            .leaves()
                            .iter()
                            .map(|v| {
                                if d.include_commands {
                                    v.read(cx).recent_commands()
                                } else {
                                    Vec::new()
                                }
                            })
                            .collect();
                        let title = tab.name.as_ref().map(|n| n.text.clone());
                        Some(recipes::tab_layout(
                            &state, title, &commands, d.portable, &bases,
                        ))
                    })
                    .collect();
                if layouts.is_empty() {
                    return Err("There are no terminal tabs to save".into());
                }
                recipe.window = Some(Layout { tabs: layouts });
            }
            Scope::Commands => {
                recipe.commands = d
                    .commands
                    .iter()
                    .filter(|(_, on)| *on)
                    .map(|(c, _)| c.clone())
                    .collect();
                if recipe.commands.is_empty() {
                    return Err("Tick at least one command".into());
                }
            }
            Scope::Snippet => {
                let alias = d.alias.trim();
                if alias.is_empty() || alias.contains(char::is_whitespace) {
                    return Err("The alias is one word, typed at the prompt".into());
                }
                if d.text.is_empty() {
                    return Err("The snippet has no text".into());
                }
                recipe.snippet = Some(Snippet {
                    alias: alias.to_owned(),
                    text: d.text.clone(),
                });
            }
        }
        Ok(recipe)
    }

    fn commit_recipe_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.recipe_dialog.as_ref() else {
            return;
        };
        // Checked now, so a mistake shows before the file panel does.
        let recipe = match self.build_recipe(d, None, cx) {
            Ok(r) => r,
            Err(e) => {
                if let Some(d) = self.recipe_dialog.as_mut() {
                    d.error = Some(e);
                }
                cx.notify();
                return;
            }
        };
        let Some(d) = self.recipe_dialog.take() else {
            return;
        };
        self.activate(self.active, window, cx);
        if !d.export {
            if let Err(e) = recipes::save(&recipe) {
                tracing::warn!("couldn't save the recipe: {e}");
            }
            cx.defer(crate::menus::refresh);
            return;
        }
        let dir = self
            .active_folder(cx)
            .or_else(dirs::home_dir)
            .unwrap_or_default();
        let file = format!("{}.{}", recipe.recipe.name, recipes::EXTENSION);
        let picked = cx.prompt_for_new_path(&dir, Some(&file));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            // Recipe Location paths are relative to where it's saved.
            let Ok(Ok(recipe)) = this.update(cx, |this, cx| this.build_recipe(&d, Some(&path), cx))
            else {
                return;
            };
            if let Err(e) = std::fs::write(&path, recipe.to_toml()) {
                tracing::warn!("couldn't export the recipe to {}: {e}", path.display());
            }
        })
        .detach();
    }

    /// A saved recipe, by id or name, as `recipe-replay-saved` says.
    pub(crate) fn apply_saved_recipe(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(saved) = recipes::find(key) else {
            window.play_system_bell();
            return;
        };
        let bases = Bases::new(self.active_folder(cx), None);
        let replay = self.config.replay_saved();
        self.apply_recipe(saved.recipe, bases, replay, window, cx);
    }

    /// File › Open Recipe File…
    pub(super) fn open_recipe_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.open_recipe_path(&path, window, cx)
            });
        })
        .detach();
    }

    /// A recipe file from elsewhere: its commands are shown first unless
    /// it was trusted before (by its SHA-256).
    pub(crate) fn open_recipe_path(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parsed = std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                let recipe = Recipe::parse(&String::from_utf8_lossy(&bytes))?;
                Ok((recipe, recipes::hash(&bytes)))
            });
        let (recipe, hash) = match parsed {
            Ok(r) => r,
            Err(e) => {
                drop(window.prompt(
                    PromptLevel::Warning,
                    "This recipe can't be opened",
                    Some(&format!("{}: {e}", path.display())),
                    &["OK"],
                    cx,
                ));
                return;
            }
        };
        let bases = Bases::new(self.active_folder(cx), Some(path));
        let replay = self.config.replay_file();
        let commands = recipe.all_commands();
        if commands.is_empty() || replay == Replay::Skip || recipes::is_trusted(&hash) {
            return self.apply_recipe(recipe, bases, replay, window, cx);
        }
        let mut detail = String::new();
        let mut last = None;
        for (tab, pane, command) in &commands {
            if recipe.window.is_some() && last != Some((*tab, *pane)) {
                detail.push_str(&format!("Tab {} · Pane {}\n", tab + 1, pane + 1));
                last = Some((*tab, *pane));
            }
            detail.push_str(&format!("    {command}\n"));
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Run the commands in “{}”?", recipe.recipe.name),
            Some(detail.trim_end()),
            // Return presses nothing, Escape cancels and the button the
            // keyboard starts on (the last) only shows the file, so no
            // stray key runs or trusts it.
            &[
                PromptButton::cancel("Cancel"),
                PromptButton::new("Run Once"),
                PromptButton::new("Always Trust"),
                PromptButton::new("Show Recipe"),
            ],
            cx,
        );
        let path = path.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            let _ = this.update_in(cx, |this, window, cx| match choice {
                Ok(1 | 2) => {
                    if choice == Ok(2) {
                        recipes::trust(&hash);
                    }
                    this.apply_recipe(recipe, bases, replay, window, cx)
                }
                Ok(3) => this.open_editor(Some(path), window, cx),
                _ => {}
            });
        })
        .detach();
    }

    /// Opens what `recipe` holds: its tabs here, its window as a new one,
    /// its commands or snippet in the focused pane.
    fn apply_recipe(
        &mut self,
        recipe: Recipe,
        bases: Bases,
        replay: Replay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs: Vec<_> = recipe
            .window
            .iter()
            .flat_map(|w| w.tabs.iter())
            .map(|t| recipes::tab_state(t, &bases))
            .collect();
        match recipe.recipe.scope {
            Scope::Tab => self.add_recipe_tabs(tabs, replay, false, window, cx),
            Scope::Window => {
                let config = self.config.clone();
                let cwd = bases.current.clone();
                cx.defer(
                    move |cx| match crate::open_window_for_recipe(config, cwd, cx) {
                        Ok(handle) => {
                            let _ = handle.update(cx, |ws, window, cx| {
                                ws.add_recipe_tabs(tabs, replay, true, window, cx)
                            });
                        }
                        Err(e) => tracing::error!("couldn't open the recipe's window: {e:#}"),
                    },
                );
            }
            Scope::Commands => {
                if let Some(pane) = self.focused_pane() {
                    pane.update(cx, |v, cx| v.replay(recipe.commands, replay, window, cx));
                }
            }
            Scope::Snippet => {
                if let (Some(pane), Some(snippet)) = (self.focused_pane(), recipe.snippet) {
                    type_snippet(&pane, &snippet.text, cx);
                }
            }
        }
    }

    /// Opens `tabs` after the active one (with `replace`, in place of the
    /// window's first, blank, tab) and replays each pane's commands.
    fn add_recipe_tabs(
        &mut self,
        tabs: Vec<(TabState, Vec<Vec<String>>)>,
        replay: Replay,
        replace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let blank = replace.then_some(self.tabs.len());
        let first = self.new_tab_index();
        let mut ix = first;
        for (state, commands) in tabs {
            let tab = match self.restore_tab(&state, &|_| None, window, cx) {
                Ok(tab) => tab,
                Err(e) => {
                    tracing::error!("couldn't open a recipe's tab: {e:#}");
                    continue;
                }
            };
            let leaves: Vec<Entity<TerminalView>> = tab
                .terms()
                .map(|t| t.root.leaves().into_iter().cloned().collect())
                .unwrap_or_default();
            self.tabs.insert(ix, tab);
            ix += 1;
            for (view, commands) in leaves.iter().zip(commands) {
                view.update(cx, |v, cx| v.replay(commands, replay, window, cx));
            }
        }
        if ix == first {
            return;
        }
        self.activate(first, window, cx);
        if let Some(blank) = blank {
            for _ in 0..blank {
                self.remove_tab(0, window, cx);
            }
        }
    }

    fn on_recipe_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let Some(d) = self.recipe_dialog.as_mut() else {
            return;
        };
        match ks.key.as_str() {
            "escape" => self.close_recipe_dialog(window, cx),
            "enter" => self.commit_recipe_dialog(window, cx),
            "tab" => {
                let fields: &[Field] = match d.scope {
                    Scope::Snippet => &[Field::Name, Field::Alias, Field::Text],
                    _ => &[Field::Name],
                };
                let at = fields.iter().position(|f| *f == d.field).unwrap_or(0);
                let step = if ks.modifiers.shift {
                    fields.len() - 1
                } else {
                    1
                };
                d.field = fields[(at + step) % fields.len()];
                cx.notify();
            }
            _ => {
                let text = match d.field {
                    Field::Name => &mut d.name,
                    Field::Alias => &mut d.alias,
                    Field::Text => &mut d.text,
                };
                if edit_query(text, ks) {
                    d.error = None;
                    cx.notify();
                }
            }
        }
        cx.stop_propagation();
    }

    pub(super) fn render_recipe_dialog(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let d = self.recipe_dialog.as_ref()?;
        let c = Colors::new(&self.theme);
        let accent: Hsla = gpui::rgb(ACCENT).into();
        let label = |text: &'static str| {
            div()
                .flex_none()
                .w(px(92.))
                .text_size(px(12.5))
                .text_color(c.placeholder)
                .child(text)
        };
        let row = || {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_h(px(FIELD_HEIGHT))
        };
        let field =
            |which: Field, text: &str, placeholder: &'static str, cx: &mut Context<Self>| {
                let editing = d.field == which;
                div()
                    .id(match which {
                        Field::Name => "recipe-name",
                        Field::Alias => "recipe-alias",
                        Field::Text => "recipe-text",
                    })
                    .flex_1()
                    .min_w_0()
                    .h(px(FIELD_HEIGHT))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .border(px(1.))
                    .border_color(if editing {
                        accent.opacity(0.6)
                    } else {
                        c.border
                    })
                    .bg(c.keycap)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(13.))
                    .text_color(c.text)
                    .child(SharedString::from(text.to_owned()))
                    .when(editing, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .w(px(1.5))
                                .h(px(15.))
                                .ml(px(0.5))
                                .bg(accent),
                        )
                    })
                    .when(text.is_empty(), |d| {
                        d.child(
                            div()
                                .ml(px(2.))
                                .text_color(c.placeholder)
                                .child(placeholder),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this.recipe_dialog.as_mut() {
                            d.field = which;
                            cx.notify();
                        }
                    }))
            };
        // A segmented control; `pick` sets the choice.
        let segments = |id: &'static str,
                        options: Vec<(&'static str, bool)>,
                        pick: fn(&mut RecipeDialog, usize),
                        cx: &mut Context<Self>| {
            div()
                .flex()
                .p(px(2.))
                .gap(px(2.))
                .rounded(px(7.))
                .bg(c.selected)
                .children(options.into_iter().enumerate().map(|(ix, (text, on))| {
                    div()
                        .id((id, ix))
                        .h(px(22.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .text_size(px(12.))
                        .text_color(c.text)
                        .when(on, |d| d.bg(c.card).shadow_sm())
                        .child(text)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(d) = this.recipe_dialog.as_mut() {
                                pick(d, ix);
                                d.error = None;
                                cx.notify();
                            }
                        }))
                }))
        };
        let mut body = div()
            .px(px(16.))
            .py(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                row()
                    .child(label("Name"))
                    .child(field(Field::Name, &d.name, "Recipe name", cx)),
            )
            .child(row().child(label("Scope")).child(segments(
                "recipe-scope",
                vec![
                    ("Current Tab", d.scope == Scope::Tab),
                    ("Current Window", d.scope == Scope::Window),
                    ("Commands", d.scope == Scope::Commands),
                    ("Snippet", d.scope == Scope::Snippet),
                ],
                |d, ix| {
                    d.scope = [Scope::Tab, Scope::Window, Scope::Commands, Scope::Snippet][ix];
                    if d.scope != Scope::Snippet {
                        d.field = Field::Name;
                    }
                },
                cx,
            )));
        match d.scope {
            Scope::Tab | Scope::Window => {
                let mut paths = vec![
                    ("Absolute", d.portable == Portable::Off),
                    ("Current Folder", d.portable == Portable::CurrentFolder),
                    ("Home Folder", d.portable == Portable::Home),
                ];
                if d.export {
                    paths.push(("Recipe Location", d.portable == Portable::RecipeFolder));
                }
                body = body
                    .child(row().child(label("Content")).child(segments(
                        "recipe-content",
                        vec![
                            ("Layout Only", !d.include_commands),
                            ("Include Commands", d.include_commands),
                        ],
                        |d, ix| d.include_commands = ix == 1,
                        cx,
                    )))
                    .child(row().child(label("Paths")).child(segments(
                        "recipe-paths",
                        paths,
                        |d, ix| {
                            d.portable = [
                                Portable::Off,
                                Portable::CurrentFolder,
                                Portable::Home,
                                Portable::RecipeFolder,
                            ][ix]
                        },
                        cx,
                    )));
            }
            Scope::Commands => {
                let all = !d.commands.is_empty() && d.commands.iter().all(|(_, on)| *on);
                let list = div()
                    .id("recipe-commands")
                    .max_h(px(200.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .rounded(px(6.))
                    .border(px(1.))
                    .border_color(c.border)
                    .children(d.commands.iter().enumerate().map(|(ix, (command, on))| {
                        div()
                            .id(("recipe-command", ix))
                            .h(px(26.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .hover(|d| d.bg(c.selected))
                            .child(checkbox(*on, accent, c.border))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(self.mono_font.clone())
                                    .text_size(px(12.))
                                    .text_color(c.text)
                                    .child(SharedString::from(command.clone())),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(d) = this.recipe_dialog.as_mut()
                                    && let Some(row) = d.commands.get_mut(ix)
                                {
                                    row.1 = !row.1;
                                    d.error = None;
                                    cx.notify();
                                }
                            }))
                    }))
                    .when(d.commands.is_empty(), |l| {
                        l.child(
                            div()
                                .p(px(10.))
                                .text_size(px(12.5))
                                .text_color(c.placeholder)
                                .child("No commands have run in this pane yet"),
                        )
                    });
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .id("recipe-select-all")
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(12.5))
                                .text_color(c.text)
                                .child(checkbox(all, accent, c.border))
                                .child("Select All")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(d) = this.recipe_dialog.as_mut() {
                                        for row in &mut d.commands {
                                            row.1 = !all;
                                        }
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(list),
                );
            }
            Scope::Snippet => {
                body = body
                    .child(row().child(label("Alias")).child(field(
                        Field::Alias,
                        &d.alias,
                        "Typed at the prompt",
                        cx,
                    )))
                    .child(row().child(label("Text")).child(field(
                        Field::Text,
                        &d.text,
                        "{{clipboard}} {{date}} {{time}} {{cursor}}",
                        cx,
                    )));
            }
        }
        let button = |id: &'static str, text: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(26.))
                .px(px(14.))
                .flex()
                .items_center()
                .rounded(px(7.))
                .border(px(0.5))
                .border_color(if primary { accent } else { c.border })
                .bg(if primary { accent } else { c.keycap })
                .text_size(px(12.5))
                .text_color(if primary { gpui::white() } else { c.text })
                .child(text)
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        };
        let footer = div()
            .px(px(16.))
            .pb(px(14.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(gpui::rgb(0xd70015))
                    .children(d.error.clone()),
            )
            .child(
                button("recipe-cancel", "Cancel", false).on_click(
                    cx.listener(|this, _, window, cx| this.close_recipe_dialog(window, cx)),
                ),
            )
            .child(
                button(
                    "recipe-save",
                    if d.export { "Export…" } else { "Save" },
                    true,
                )
                .on_click(cx.listener(|this, _, window, cx| this.commit_recipe_dialog(window, cx))),
            );
        let card = picker::card(WIDTH, &c)
            .id("recipe-dialog")
            .key_context("RecipeDialog")
            .track_focus(&d.focus)
            .on_key_down(cx.listener(Self::on_recipe_key))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                let text = cx.read_from_clipboard().and_then(|c| c.text());
                if let (Some(d), Some(text)) = (this.recipe_dialog.as_mut(), text) {
                    let target = match d.field {
                        Field::Name => &mut d.name,
                        Field::Alias => &mut d.alias,
                        Field::Text => &mut d.text,
                    };
                    target.push_str(text.lines().next().unwrap_or_default());
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px(px(16.))
                    .pt(px(14.))
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(c.text)
                    .child(if d.export {
                        "Export Recipe"
                    } else {
                        "Save Recipe"
                    }),
            )
            .child(body)
            .child(footer);
        Some(
            div()
                .id("recipe-scrim")
                .absolute()
                .inset_0()
                .px(px(16.))
                .pt(px(picker::top(window)))
                .flex()
                .flex_col()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_recipe_dialog(window, cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

fn checkbox(on: bool, accent: Hsla, border: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .size(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border(px(1.))
        .border_color(if on { accent } else { border })
        .when(on, |d| d.bg(accent))
        .text_size(px(10.))
        .text_color(gpui::white())
        .when(on, |d| d.child("✓"))
}

/// Types a snippet at the prompt, leaving the cursor at `{{cursor}}`.
pub(super) fn type_snippet(pane: &Entity<TerminalView>, text: &str, cx: &mut gpui::App) {
    let clipboard = if text.contains("{{clipboard}}") {
        cx.read_from_clipboard()
            .and_then(|c| c.text())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let (text, back) = expand_snippet(text, &clipboard, recipes::date_time());
    let input = format!("{text}{}", "\x1b[D".repeat(back));
    pane.update(cx, |v, cx| v.write_input(&input, cx));
}
