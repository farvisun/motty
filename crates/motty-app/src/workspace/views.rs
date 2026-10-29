//! Panes that aren't terminals or files: folder panes, and View ▸ Switch
//! to View, which shows a terminal's folder in its place and back.

use std::path::PathBuf;

use gpui::{Context, Entity, Window, prelude::*};

use super::{Content, Panes, Place, Tab, Workspace, terminal_style};
use crate::actions::{NewFileBrowser, SwitchToFileBrowser, SwitchToTerminal};
use crate::folder_view::{FolderEvent, FolderView};
use crate::panes::PaneItem;

impl Workspace {
    /// A folder pane, styled and watched: a file it opens gets a tab, as
    /// does a terminal it asks for.
    pub(super) fn make_folder(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<FolderView> {
        let style = terminal_style(&self.config, &self.theme, cx);
        let folder = cx.new(|cx| FolderView::new(path, style, cx));
        self.watch_folder(&folder, window, cx);
        folder
    }

    pub(super) fn watch_folder(
        &mut self,
        folder: &Entity<FolderView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let subscription =
            cx.subscribe_in(folder, window, |this, _, event, window, cx| match event {
                FolderEvent::Changed => cx.notify(),
                FolderEvent::OpenFile(path) => this.open_editor(Some(path.clone()), window, cx),
                FolderEvent::OpenTerminal(path) => {
                    this.start_in("", Some(path.clone()), Place::Tab, window, cx);
                }
            });
        self.pane_subscriptions
            .insert(folder.entity_id(), subscription);
    }

    /// A folder pane in a new tab, on the focused pane's folder.
    pub(super) fn new_file_browser(
        &mut self,
        _: &NewFileBrowser,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.focused_folder(cx);
        let folder = self.make_folder(path, window, cx);
        self.tabs
            .push(Tab::holding(Content::Panes(Panes::new(folder.into()))));
        self.activate(self.tabs.len() - 1, window, cx);
    }

    fn focused_folder(&self, cx: &gpui::App) -> PathBuf {
        self.tabs
            .get(self.active)
            .and_then(|t| t.panes())
            .and_then(|p| p.focused.folder(cx))
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| "/".into())
    }

    /// Shows the focused terminal's folder in its place; the terminal keeps
    /// running, and comes back with Switch to View ▸ Terminal.
    pub(super) fn switch_to_file_browser(
        &mut self,
        _: &SwitchToFileBrowser,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panes) = self.tabs.get(self.active).and_then(|t| t.panes()) else {
            return;
        };
        let PaneItem::Terminal(terminal) = panes.focused.clone() else {
            return;
        };
        let path = panes
            .focused
            .folder(cx)
            .unwrap_or_else(|| self.focused_folder(cx));
        let folder = self.make_folder(path, window, cx);
        self.switched
            .insert(folder.entity_id(), PaneItem::Terminal(terminal.clone()));
        self.replace_pane(&terminal.into(), folder.into(), window, cx);
    }

    /// Back to the terminal a folder pane stood in for, or a new one there.
    pub(super) fn switch_to_terminal(
        &mut self,
        _: &SwitchToTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panes) = self.tabs.get(self.active).and_then(|t| t.panes()) else {
            return;
        };
        let PaneItem::Folder(folder) = panes.focused.clone() else {
            return;
        };
        let item = match self.switched.remove(&folder.entity_id()) {
            Some(terminal) => terminal,
            None => {
                let path = folder.read(cx).path().to_path_buf();
                match self.new_terminal(None, Some(path), [0., 0., 1., 1.], window, cx) {
                    Ok(view) => view.into(),
                    Err(e) => return tracing::warn!("switch to terminal: {e:#}"),
                }
            }
        };
        self.replace_pane(&folder.into(), item, window, cx);
    }

    /// Puts `new` where `old` is in the active tab, and focuses it.
    fn replace_pane(
        &mut self,
        old: &PaneItem,
        new: PaneItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panes) = self.tabs.get_mut(self.active).and_then(|t| t.panes_mut()) else {
            return;
        };
        if panes.root.replace(old, new.clone()) {
            panes.focused = new.clone();
            self.focus_pane(new, window, cx);
            cx.notify();
        }
    }
}
