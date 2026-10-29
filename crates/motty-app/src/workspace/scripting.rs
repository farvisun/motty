//! What AppleScript sees of a window (see `crate::applescript`): its tabs
//! that hold a terminal, the title, and the custom title and selection that
//! scripts may set.

use gpui::{App, Context, Entity, Window};

use super::tabs::TabName;
use super::{Tab, Workspace};
use crate::terminal_view::TerminalView;

/// A tab as scripts see it.
pub(crate) struct ScriptTab {
    /// Where it is in the window's tab list.
    pub ix: usize,
    /// The pane its properties read and `do script` types into.
    pub pane: Entity<TerminalView>,
    pub selected: bool,
}

/// The tab's focused pane, or its first terminal when a file or web page
/// has focus.
fn script_pane(tab: &Tab) -> Option<&Entity<TerminalView>> {
    let panes = tab.panes()?;
    panes.terminal().or_else(|| panes.terminals().next())
}

impl Workspace {
    /// The tabs that hold a terminal, in the tab list's order.
    pub(crate) fn script_tabs(&self) -> Vec<ScriptTab> {
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(ix, tab)| {
                Some(ScriptTab {
                    ix,
                    pane: script_pane(tab)?.clone(),
                    selected: ix == self.active,
                })
            })
            .collect()
    }

    /// The tab holding pane `id`, whichever of its panes has focus.
    pub(crate) fn script_tab_holding(&self, id: u64, cx: &App) -> Option<ScriptTab> {
        let ix = self.tabs.iter().position(|tab| {
            tab.panes()
                .is_some_and(|p| p.terminals().any(|v| v.read(cx).id == id))
        })?;
        Some(ScriptTab {
            ix,
            pane: script_pane(&self.tabs[ix])?.clone(),
            selected: ix == self.active,
        })
    }

    /// The selected tab, when it holds a terminal.
    pub(crate) fn script_selected_tab(&self) -> Option<ScriptTab> {
        let tab = self.tabs.get(self.active)?;
        Some(ScriptTab {
            ix: self.active,
            pane: script_pane(tab)?.clone(),
            selected: true,
        })
    }

    /// The name given with Rename Tab (or by a script), else "".
    pub(crate) fn custom_title(&self, ix: usize) -> String {
        self.tabs
            .get(ix)
            .and_then(|tab| tab.name.as_ref())
            .map(|name| name.text.clone())
            .unwrap_or_default()
    }

    /// Names the tab, as Rename Tab does, so programs no longer set its
    /// title; "" gives the title back to them.
    pub(crate) fn set_custom_title(&mut self, ix: usize, title: &str, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(ix) {
            let text = title.trim();
            tab.name = (!text.is_empty()).then(|| TabName {
                text: text.to_owned(),
                prefix: false,
            });
            cx.notify();
        }
    }

    /// Brings the tab forward.
    pub(crate) fn select_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() && ix != self.active {
            self.activate(ix, window, cx);
            cx.notify();
        }
    }
}
