//! File editor tabs: opening one (once per file) and asking before one
//! with unsaved changes closes.

use std::path::PathBuf;

use gpui::{Context, PromptLevel, Window, prelude::*};

use super::{Tab, Workspace, terminal_style};
use crate::editor::{EditorEvent, FileEditor};

impl Workspace {
    /// Opens `path` (or an empty Untitled file) in an editor tab; a file
    /// that's already open has its tab brought forward.
    pub fn open_editor(
        &mut self,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = &path {
            crate::folders::record_file(path);
        }
        let open = path.as_deref().and_then(|path| {
            self.tabs
                .iter()
                .filter_map(|t| t.panes())
                .flat_map(|p| p.files())
                .find(|e| e.read(cx).path() == Some(path))
                .cloned()
        });
        if let Some(editor) = open {
            self.reveal_item(&editor.into(), window, cx);
            return;
        }
        let editor = self.make_editor(path, cx);
        self.tabs.push(Tab::editor(editor));
        self.activate(self.tabs.len() - 1, window, cx);
    }

    /// A file pane for `path`, styled and watched.
    pub(super) fn make_editor(
        &mut self,
        path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<FileEditor> {
        let style = terminal_style(&self.config, &self.theme, cx);
        let prefs = self.config.editor;
        let editor = cx.new(|cx| {
            let mut editor = FileEditor::new(path, style, cx);
            editor.set_prefs(prefs, cx);
            editor
        });
        self.watch_editor(&editor, cx);
        editor
    }

    /// Redraws the tab (its title, the unsaved star) as the editor changes.
    pub(super) fn watch_editor(
        &mut self,
        editor: &gpui::Entity<FileEditor>,
        cx: &mut Context<Self>,
    ) {
        let subscription = cx.subscribe(editor, |_, _, _: &EditorEvent, cx| cx.notify());
        self.pane_subscriptions
            .insert(editor.entity_id(), subscription);
    }

    /// Save, Don't Save or Cancel, as a Mac app asks.
    pub(super) fn confirm_close_editor(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(ix) else { return };
        let Some(editor) = tab.dirty_editor(cx) else {
            return;
        };
        let key = super::closing::tab_key(tab);
        self.activate(ix, window, cx);
        let message = format!(
            "Do you want to save the changes you made to {}?",
            editor.read(cx).title()
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some("Your changes will be lost if you don't save them."),
            &["Save", "Don't Save", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = answer.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(ix) = this
                    .tabs
                    .iter()
                    .position(|t| super::closing::tab_key(t) == key)
                else {
                    return;
                };
                match choice {
                    0 => {
                        editor.update(cx, |e, cx| e.save(window, cx));
                        // An Untitled file is still being named; it stays.
                        if !editor.read(cx).is_dirty() {
                            this.remove_tab(ix, window, cx);
                        }
                    }
                    1 => this.remove_tab(ix, window, cx),
                    _ => {}
                }
            });
        })
        .detach();
    }

    /// A file pane in a split with unsaved changes: Save, Don't Save or
    /// Cancel, then the pane closes.
    pub(super) fn confirm_close_file_pane(
        &mut self,
        editor: gpui::Entity<FileEditor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = format!(
            "Do you want to save the changes you made to {}?",
            editor.read(cx).title()
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some("Your changes will be lost if you don't save them."),
            &["Save", "Don't Save", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = answer.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let item = editor.clone().into();
                match choice {
                    0 => {
                        editor.update(cx, |e, cx| e.save(window, cx));
                        if !editor.read(cx).is_dirty() {
                            this.close_pane(&item, window, cx);
                        }
                    }
                    1 => this.close_pane(&item, window, cx),
                    _ => {}
                }
            });
        })
        .detach();
    }

    pub(super) fn new_file_editor(
        &mut self,
        _: &crate::actions::NewFileEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_editor(None, window, cx);
    }
}
