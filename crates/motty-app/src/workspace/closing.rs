//! Asking before a close ends something that's still running
//! (`confirm-close-pane|tab|window`).

use gpui::{Context, Entity, EntityId, PromptLevel, Window};
use motty_theme::ConfirmClose;

use super::{Content, Tab, Workspace};
use crate::panes::PaneItem;
use crate::terminal_view::TerminalView;

/// What a close would take.
pub(super) enum Closing {
    Pane(PaneItem),
    Tab(usize),
    Window,
}

/// Whether `rule` asks, given whether something is running and how many
/// tabs the window holds.
pub(super) fn asks(rule: ConfirmClose, running: bool, tabs: usize) -> bool {
    match rule {
        ConfirmClose::Never => false,
        ConfirmClose::Always => true,
        ConfirmClose::Process => running,
        ConfirmClose::MultipleTabs => tabs > 1,
    }
}

/// `“vim” is still running.` and the like.
fn detail(running: &[String], what: &str) -> Option<String> {
    let first = running.first()?;
    Some(match running.len() {
        1 => format!("“{first}” is still running. Closing the {what} will end it."),
        n => format!(
            "“{first}” and {} more are still running. Closing the {what} will end them.",
            n - 1
        ),
    })
}

/// What a close would end.
#[derive(Default)]
struct Running {
    busy: Vec<String>,
    /// Coding agents between turns.
    idle_agents: Vec<String>,
}

impl Running {
    /// Whether anything counts as running.
    fn counts(&self, ignore_idle_agents: bool) -> bool {
        !self.busy.is_empty() || (!ignore_idle_agents && !self.idle_agents.is_empty())
    }
}

/// The tab's identity across reorders: its first pane or its results.
pub(super) fn tab_key(tab: &Tab) -> EntityId {
    match &tab.content {
        Content::Panes(t) => t.root.leaves()[0].entity_id(),
        Content::Search(s) => s.entity_id(),
    }
}

impl Workspace {
    /// What's running in the panes `closing` would take, by name: busy
    /// programs, and coding agents waiting at their prompt.
    fn running(&self, closing: &Closing, cx: &gpui::App) -> Running {
        let panes: Vec<&Entity<TerminalView>> = match closing {
            Closing::Pane(item) => item.terminal().into_iter().collect(),
            Closing::Tab(ix) => self
                .tabs
                .get(*ix)
                .and_then(Tab::panes)
                .map(|t| t.terminals().collect())
                .unwrap_or_default(),
            Closing::Window => self
                .tabs
                .iter()
                .filter_map(Tab::panes)
                .flat_map(|t| t.terminals())
                .collect(),
        };
        let mut running = Running::default();
        for view in panes {
            let v = view.read(cx);
            let session = v.session();
            if !session.is_busy() {
                continue;
            }
            match v
                .status
                .as_ref()
                .filter(|s| !s.in_turn)
                .and_then(|s| s.agent.clone())
            {
                Some(agent) => running.idle_agents.push(agent),
                None => running.busy.push(
                    session
                        .foreground_process_name()
                        .unwrap_or_else(|| "A command".into()),
                ),
            }
        }
        running
    }

    /// Closes after asking, if the `confirm-close-*` rule says to. A tab
    /// that's the window's last follows the window's rule.
    pub(super) fn close_asking(
        &mut self,
        closing: Closing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An unsaved file asks on its own.
        match &closing {
            Closing::Tab(ix)
                if self
                    .tabs
                    .get(*ix)
                    .is_some_and(|t| t.dirty_editor(cx).is_some()) =>
            {
                return self.close_tab_at(*ix, window, cx);
            }
            Closing::Pane(PaneItem::File(editor)) if editor.read(cx).is_dirty() => {
                return self.confirm_close_file_pane(editor.clone(), window, cx);
            }
            _ => {}
        }
        let last_tab = matches!(closing, Closing::Tab(_)) && self.tabs.len() == 1;
        let (rule, what) = match closing {
            Closing::Pane(_) => (self.config.confirm_close_pane, "pane"),
            Closing::Tab(_) if !last_tab => (self.config.confirm_close_tab, "tab"),
            _ => (self.config.confirm_close_window, "window"),
        };
        let Running { busy, idle_agents } = self.running(&closing, cx);
        let ignore_idle = self.config.close_ignores_idle_agents;
        // Only idle agents: Otty names them and offers not to ask again.
        let only_idle = busy.is_empty() && !idle_agents.is_empty() && !ignore_idle;
        let running = if ignore_idle {
            busy
        } else {
            busy.into_iter()
                .chain(idle_agents.iter().cloned())
                .collect()
        };
        if !asks(rule, !running.is_empty(), self.tabs.len()) {
            return self.close_now(closing, window, cx);
        }
        // The tab may move while the question is up.
        let key = match &closing {
            Closing::Tab(ix) => self.tabs.get(*ix).map(tab_key),
            _ => None,
        };
        let detail = if only_idle {
            Some(format!(
                "{} is waiting at its prompt. Closing the {what} will end it.",
                idle_agents.join(", ")
            ))
        } else {
            detail(&running, what)
        };
        let answers: &[&str] = if only_idle {
            &["Close", "Cancel", "Don't Ask Again for Idle Coding Agents"]
        } else {
            &["Close", "Cancel"]
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Close this {what}?"),
            detail.as_deref(),
            answers,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            match answer.await {
                Ok(0) => {}
                Ok(2) => {
                    let value = toml::Value::Boolean(true);
                    if let Err(e) = motty_theme::config::save_values(&[(
                        "confirm-close-ignore-idle-agents",
                        value,
                    )]) {
                        tracing::warn!("couldn't save the close setting: {e}");
                    }
                    let _ = cx.update(|_, cx| cx.defer(super::reload_all));
                }
                _ => return,
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let closing = match closing {
                    Closing::Tab(_) => {
                        match this.tabs.iter().position(|t| Some(tab_key(t)) == key) {
                            Some(ix) => Closing::Tab(ix),
                            None => return,
                        }
                    }
                    other => other,
                };
                this.close_now(closing, window, cx);
            });
        })
        .detach();
    }

    fn close_now(&mut self, closing: Closing, window: &mut Window, cx: &mut Context<Self>) {
        match closing {
            Closing::Pane(item) => {
                if self.tab_of(&item).is_some() {
                    self.close_pane(&item, window, cx);
                }
            }
            Closing::Tab(ix) => self.close_tab_at(ix, window, cx),
            Closing::Window => {
                self.closing_window = true;
                window.remove_window();
            }
        }
    }

    /// The close button: asks first if the window rule says to.
    pub(super) fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.closing_window {
            return true;
        }
        let running = self
            .running(&Closing::Window, cx)
            .counts(self.config.close_ignores_idle_agents);
        if !asks(self.config.confirm_close_window, running, self.tabs.len()) {
            return true;
        }
        self.close_asking(Closing::Window, window, cx);
        false
    }

    /// ⌘W: the focused pane, the tab with its last pane, the window with
    /// its last tab.
    pub(super) fn close_cascade(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let closing = match self.tabs.get(self.active).map(|t| &t.content) {
            Some(Content::Panes(t)) if t.root.leaves().len() > 1 => {
                Closing::Pane(t.focused.clone())
            }
            Some(_) => Closing::Tab(self.active),
            None => return,
        };
        self.close_asking(closing, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        assert!(!asks(ConfirmClose::Process, false, 3));
        assert!(asks(ConfirmClose::Process, true, 1));
        assert!(asks(ConfirmClose::Always, false, 1));
        assert!(!asks(ConfirmClose::Never, true, 5));
        assert!(asks(ConfirmClose::MultipleTabs, false, 2));
        assert!(!asks(ConfirmClose::MultipleTabs, true, 1));
    }

    #[test]
    fn details() {
        assert_eq!(detail(&[], "tab"), None);
        assert_eq!(
            detail(&["vim".into()], "tab").unwrap(),
            "“vim” is still running. Closing the tab will end it."
        );
        assert!(
            detail(&["vim".into(), "npm".into(), "ssh".into()], "window")
                .unwrap()
                .starts_with("“vim” and 2 more")
        );
    }
}
