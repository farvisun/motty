//! What the control socket (`crate::ipc`) does to a window's tabs and
//! panes beyond what keys and menus do: ids, names, moves, badges and
//! closes that refuse rather than ask.

use std::path::PathBuf;

use anyhow::{Context as _, bail};
use gpui::{App, Context, Entity, Window};
use motty_ipc::TabInfo;

use super::closing::Closing;
use super::{Content, MovedTab, Place, Tab, Workspace, tab_title};
use crate::agents::Badge;
use crate::panes::{Direction, PaneItem};
use crate::terminal_view::TerminalView;

impl Workspace {
    /// `w_<id>`'s number.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Its name, set with `motty window rename`.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The window's title: its name, else the active tab's title.
    pub fn title(&self, cx: &App) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => self
                .tabs
                .get(self.active)
                .map(|tab| tab_title(tab, cx))
                .unwrap_or_default(),
        }
    }

    pub fn set_name(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.name = name;
        cx.notify();
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab(&self) -> usize {
        self.active
    }

    /// The tab ids, in the window's order.
    pub fn tab_ids(&self) -> Vec<u64> {
        self.tabs.iter().map(|t| t.id).collect()
    }

    /// The terminal panes of the tab at `ix`, in reading order.
    pub fn tab_panes(&self, ix: usize) -> Vec<Entity<TerminalView>> {
        self.tabs
            .get(ix)
            .and_then(Tab::panes)
            .map(|p| p.terminals().cloned().collect())
            .unwrap_or_default()
    }

    /// The tab's focused pane, when it's a terminal.
    pub fn tab_focused_pane(&self, ix: usize) -> Option<Entity<TerminalView>> {
        self.tabs.get(ix)?.panes()?.terminal().cloned()
    }

    /// The index of the tab holding `view`.
    pub fn tab_of_pane(&self, view: &Entity<TerminalView>) -> Option<usize> {
        self.tab_of(view)
    }

    pub fn tab_info(&self, ix: usize, window_id: &str, cx: &App) -> Option<TabInfo> {
        let tab = self.tabs.get(ix)?;
        Some(TabInfo {
            id: format!("t_{}", tab.id),
            window_id: window_id.to_owned(),
            index: ix + 1,
            title: tab_title(tab, cx),
            name: tab.name.as_ref().map(|n| n.text.clone()),
            active: ix == self.active,
            panes: self.tab_panes(ix).iter().map(|v| v.read(cx).id).collect(),
            badge: self.tab_summary(tab, cx).badge.map(|b| b.name().to_owned()),
            zoomed: tab.panes().is_some_and(|p| p.zoomed),
        })
    }

    /// Activates the tab at `ix` and brings the window forward.
    pub fn focus_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.activate(ix, window, cx);
        window.activate_window();
    }

    /// Names the tab, or with `None` gives it back its title.
    pub fn rename_tab(&mut self, ix: usize, name: Option<String>, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(ix) {
            tab.name = name.map(|text| super::tabs::TabName {
                text,
                prefix: false,
            });
            cx.notify();
        }
    }

    /// Moves the tab at `from` to `to`, both in this window.
    pub fn move_tab_within(
        &mut self,
        from: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_tab(from, to.min(self.tabs.len().saturating_sub(1)), window, cx);
    }

    /// Takes the tab at `ix` out to go to another window; the last tab
    /// stays (its window would close).
    pub fn take_tab(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<MovedTab> {
        anyhow::ensure!(ix < self.tabs.len(), "no such tab");
        anyhow::ensure!(self.tabs.len() > 1, "it's the window's only tab");
        let tab = self.tabs.remove(ix);
        self.unwatch(&tab);
        let next = if self.active > ix {
            self.active - 1
        } else {
            self.active.min(self.tabs.len() - 1)
        };
        self.activate(next, window, cx);
        Ok(MovedTab(tab))
    }

    /// Puts a tab taken from another window here, at `index` if given.
    pub fn put_tab(
        &mut self,
        moved: MovedTab,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adopt(moved, window, cx);
        if let Some(to) = index {
            let from = self.tabs.len() - 1;
            self.move_tab_within(from, to, window, cx);
        }
    }

    pub fn tab_to_new_window(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.move_tab_to_new_window(ix, window, cx);
    }

    /// Shows only `view` in its tab, or all of them again; `None` toggles.
    pub fn zoom_pane(
        &mut self,
        view: &Entity<TerminalView>,
        zoom: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<bool> {
        self.reveal_pane(view, window, cx);
        let tab = self.active_panes_mut().context("no panes")?;
        anyhow::ensure!(
            matches!(tab.root, crate::panes::Pane::Split(_)),
            "the tab has only one pane"
        );
        tab.zoomed = zoom.unwrap_or(!tab.zoomed);
        let zoomed = tab.zoomed;
        cx.notify();
        Ok(zoomed)
    }

    /// Moves the dividers beside `view` by whole cells, as ⌃⇧⌘ arrows do.
    pub fn resize_pane(
        &mut self,
        view: &Entity<TerminalView>,
        [left, right, up, down]: [u16; 4],
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let size = view.read(cx).session().size();
        let ix = self.tab_of(view).context("no such pane")?;
        let tab = self.tabs[ix].panes_mut().context("no panes")?;
        let item: PaneItem = view.clone().into();
        let moves = [
            (
                gpui::Axis::Horizontal,
                size.cell_width,
                right as f32 - left as f32,
            ),
            (
                gpui::Axis::Vertical,
                size.cell_height,
                down as f32 - up as f32,
            ),
        ];
        let mut moved = false;
        for (axis, cell, cells) in moves {
            if cells == 0. {
                continue;
            }
            if let Some(split) = tab.root.enclosing_split(&item, axis) {
                split.nudge(cells * cell);
                moved = true;
            }
        }
        if !moved && (left | right | up | down) != 0 {
            bail!("no divider beside that pane in that direction");
        }
        cx.notify();
        Ok(())
    }

    /// Gives the pane `view` (just split off) `share` of its split.
    pub fn set_split_share(
        &mut self,
        view: &Entity<TerminalView>,
        direction: Direction,
        share: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.tab_of(view) else { return };
        let Some(tab) = self.tabs[ix].panes_mut() else {
            return;
        };
        let item: PaneItem = view.clone().into();
        let axis = match direction {
            Direction::Left | Direction::Right => gpui::Axis::Horizontal,
            Direction::Up | Direction::Down => gpui::Axis::Vertical,
        };
        if let Some(split) = tab.root.enclosing_split(&item, axis) {
            let first = split.first.contains(&item);
            split.ratio = if first { share } else { 1. - share };
            cx.notify();
        }
    }

    /// Sets the tab's badge as Mark Tab does, or clears it.
    pub fn set_tab_badge(&mut self, ix: usize, badge: Option<Badge>, cx: &mut Context<Self>) {
        match badge {
            Some(badge) => {
                if let Some(tab) = self.tabs.get_mut(ix) {
                    tab.mark = Some(badge);
                    cx.notify();
                }
            }
            None => self.clear_badge(ix, cx),
        }
    }

    pub fn close_pane_from_cli(
        &mut self,
        view: &Entity<TerminalView>,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.close_unasked(Closing::Pane(view.clone().into()), force, window, cx)
    }

    pub fn close_tab_from_cli(
        &mut self,
        ix: usize,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.close_unasked(Closing::Tab(ix), force, window, cx)
    }

    pub fn close_window_from_cli(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.close_unasked(Closing::Window, force, window, cx)
    }

    /// `motty view` / `motty edit`: a file (read-only unless `edit`) or an
    /// http(s) page as a pane, at `place` beside the focused pane. With
    /// `replace_first`, it takes the place of the window's first tab (the
    /// shell a new window starts with).
    pub fn view_target(
        &mut self,
        target: &str,
        edit: bool,
        place: Place,
        replace_first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let item: PaneItem = if target.starts_with("http://") || target.starts_with("https://") {
            self.make_web(Some(target.to_owned()), cx).into()
        } else {
            let path = PathBuf::from(target);
            anyhow::ensure!(path.is_absolute(), "{target} isn't an absolute path");
            anyhow::ensure!(path.is_file(), "no such file: {target}");
            crate::folders::record_file(&path);
            self.make_file_pane(path, cx)
        };
        if let PaneItem::File(editor) = &item {
            editor.update(cx, |e, cx| e.set_read_only(!edit, cx));
        }
        match place {
            Place::Split(direction) => self.split_with(item, direction, true, window, cx)?,
            Place::Tab | Place::Window => {
                let ix = self.new_tab_index();
                self.tabs
                    .insert(ix, Tab::holding(Content::Panes(super::Panes::new(item))));
                self.activate(ix, window, cx);
                if replace_first && ix > 0 {
                    self.remove_tab(0, window, cx);
                }
            }
        }
        window.activate_window();
        Ok(())
    }

    /// The config's `keybind` lines.
    pub fn keybinds(&self) -> &[motty_theme::config::Keybind] {
        &self.config.keybinds
    }
}
