//! Tab badges and notifications from coding agents' hooks and from
//! commands finishing, as Otty's Agent Behavior settings describe them.

use std::time::SystemTime;

use gpui::{Context, Entity, Window};
use motty_ipc::{AgentState, AgentStateParams};

use super::Workspace;
use crate::agents::{self, Badge, Status};
use crate::terminal_view::TerminalView;

impl Workspace {
    /// Whether the user is looking at `view` right now.
    fn looking_at(&self, view: &Entity<TerminalView>, window: &Window) -> bool {
        window.is_window_active() && self.is_pane_shown(view)
    }

    /// An agent's hook reported in.
    pub fn agent_state(
        &mut self,
        view: &Entity<TerminalView>,
        p: AgentStateParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if p.state == AgentState::Ended {
            view.update(cx, |v, _| v.status = None);
            return self.touched(view, cx);
        }
        let b = self.config.badges;
        let looking = self.looking_at(view, window);
        let (prev, process, title) = {
            let v = view.read(cx);
            (v.status.clone(), v.process.clone(), v.display_title())
        };
        // Grok runs Claude's hooks too: the program in the pane says which.
        let label = p
            .label
            .clone()
            .or_else(|| {
                let a = agents::by_command(process.as_deref()?)?;
                Some(a.label.to_owned())
            })
            .unwrap_or_else(|| agents::label_for(&p.agent));
        let prev_badge = prev.as_ref().and_then(|s| s.badge);
        let in_turn = prev.as_ref().is_some_and(|s| s.in_turn);
        let (badge, still_in_turn, stock) = match p.state {
            AgentState::Processing => (b.agent_processing.then_some(Badge::Running), true, None),
            AgentState::Awaiting => (
                b.agent_awaiting.then_some(Badge::Awaiting),
                true,
                (b.notify_awaiting && prev_badge != Some(Badge::Awaiting))
                    .then_some("Needs your input"),
            ),
            // An idle report without a turn before it is a session starting.
            AgentState::Idle if in_turn => (
                (b.agent_complete && !looking).then_some(Badge::Done),
                false,
                b.notify_complete.then_some("Finished"),
            ),
            AgentState::Idle => (None, false, None),
            AgentState::Error => (Some(Badge::Error), false, Some("Failed")),
            AgentState::Ended => unreachable!(),
        };
        // A step without words keeps the last one.
        let message = p.message.clone().or_else(|| {
            (p.state == AgentState::Processing)
                .then(|| prev.as_ref()?.message.clone())
                .flatten()
        });
        if let Some(stock) = stock
            && !looking
        {
            let body = format!("{} — {title}", message.as_deref().unwrap_or(stock));
            crate::notifications::notify(&label, &body);
        }
        let session = p
            .session_id
            .clone()
            .or_else(|| prev.as_ref()?.session.clone());
        view.update(cx, |v, _| {
            v.status = Some(Status {
                agent: Some(label),
                badge,
                message,
                session,
                in_turn: still_in_turn,
            })
        });
        self.touched(view, cx);
    }

    /// A command run from an integrated shell finished: an accent dot or
    /// an error triangle, unless an agent in the pane speaks for it.
    pub(super) fn command_status(
        &mut self,
        view: &Entity<TerminalView>,
        exit: Option<i32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.config.badges.exit_status
            || view
                .read(cx)
                .status
                .as_ref()
                .is_some_and(|s| s.agent.is_some())
        {
            return;
        }
        let looking = self.looking_at(view, window);
        let (badge, message) = match exit {
            Some(code) if code != 0 => (Some(Badge::Error), Some(format!("Exited with {code}"))),
            _ if looking => (None, None),
            _ => (Some(Badge::Finished), None),
        };
        view.update(cx, |v, _| {
            v.status = badge.map(|badge| Status {
                agent: None,
                badge: Some(badge),
                message,
                session: None,
                in_turn: false,
            })
        });
        self.touched(view, cx);
    }

    /// Looking at a tab clears the marks of what happened while away; a
    /// spinner or a hand stays while it's true.
    pub(super) fn clear_seen(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(terms) = self.tabs.get(ix).and_then(|t| t.terms()) else {
            return;
        };
        for view in terms.root.leaves() {
            let stale = view
                .read(cx)
                .status
                .as_ref()
                .and_then(|s| s.badge)
                .is_some_and(|b| !b.ongoing());
            if stale {
                view.update(cx, |v, _| {
                    if let Some(s) = &mut v.status {
                        s.badge = None;
                    }
                });
            }
        }
    }

    /// The tab holding `view` changed: it moves up in Inbox.
    fn touched(&mut self, view: &Entity<TerminalView>, cx: &mut Context<Self>) {
        if let Some(ix) = self.tab_of(view) {
            self.tabs[ix].updated = SystemTime::now();
        }
        cx.notify();
    }

    /// The pane an agent's report is about: by pane id, else the pane whose
    /// processes include the agent.
    pub fn pane_for_agent(
        &self,
        pane: Option<u64>,
        pid: Option<u32>,
        cx: &gpui::App,
    ) -> Option<Entity<TerminalView>> {
        if let Some(id) = pane {
            return self.find_pane(id, cx);
        }
        let pid = pid? as i32;
        self.panes()
            .map(|(_, v)| v)
            .find(|v| {
                v.read(cx)
                    .session()
                    .processes()
                    .iter()
                    .any(|p| p.pid == pid)
            })
            .cloned()
    }
}
