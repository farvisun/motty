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
        let b = self.badges_for(view);
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
        // Grok runs Claude's hooks: the program says which it is.
        let agent_id = process
            .as_deref()
            .and_then(agents::by_command)
            .map_or_else(|| p.agent.clone(), |a| a.id.to_owned());
        // A finished task is when the plan's numbers move.
        if p.state == AgentState::Idle && in_turn {
            agents::quota::task_finished(&agent_id, cx);
        }
        if let Some(path) = &p.transcript
            && matches!(p.agent.as_str(), "claude" | "grok")
        {
            agents::sessions::remember_claude_root(std::path::Path::new(path));
        }
        view.update(cx, |v, cx| {
            v.status = Some(Status {
                agent: Some(label),
                agent_id: Some(agent_id),
                badge,
                message,
                session,
                in_turn: still_in_turn,
            });
            // The queue's next prompt goes when a turn ends.
            if matches!(p.state, AgentState::Idle | AgentState::Error) {
                v.turn_ended(window, cx);
            }
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
        let b = self.badges_for(view);
        let agent = view
            .read(cx)
            .status
            .as_ref()
            .is_some_and(|s| s.agent.is_some());
        if !agent {
            self.command_alerts(view, exit, b, window, cx);
        }
        if !b.exit_status || agent {
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
                agent_id: None,
                badge: Some(badge),
                message,
                session: None,
                in_turn: false,
            })
        });
        self.touched(view, cx);
    }

    /// Gives each pane its tab's Title and Mouse — Shell Controlled; run
    /// when they change, on reload, and for a new pane.
    pub(super) fn sync_shell_privileges(&mut self, cx: &mut Context<Self>) {
        let defaults = self.config.badges;
        // Title From OSC off: the rules (or the folder) name every tab.
        let programs = self.config.programs_set_title();
        for tab in &self.tabs {
            let b = tab.privileges.unwrap_or(defaults);
            let title = b.title_shell && programs;
            let Some(panes) = tab.panes() else { continue };
            for view in panes.terminals() {
                let v = view.read(cx);
                if (v.allow_title, v.allow_mouse) != (title, b.mouse_shell) {
                    view.update(cx, |v, cx| {
                        v.allow_title = title;
                        v.allow_mouse = b.mouse_shell;
                        if !programs {
                            v.title = None;
                        }
                        cx.notify();
                    });
                }
            }
        }
    }

    /// Sound on Error Exit, Notify on Error and Notify on Finish.
    fn command_alerts(
        &mut self,
        view: &Entity<TerminalView>,
        exit: Option<i32>,
        b: motty_theme::Badges,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let failed = exit.is_some_and(|code| code != 0);
        if failed && b.sound_on_error {
            window.play_system_bell();
        }
        let body = match exit {
            Some(code) if code != 0 && b.notify_error => format!("Exited with {code}"),
            Some(0) if b.notify_finish => "Finished".to_owned(),
            _ => return,
        };
        // Named after the command, when the shell said which.
        let title = {
            let v = view.read(cx);
            let session = v.session();
            session
                .shell()
                .last_finished()
                .and_then(|block| session.block_command(block))
                .unwrap_or_else(|| v.display_title())
        };
        self.banner(view, &title, &body, window, cx);
    }

    /// Posts a banner about `view`, as `notification-foreground` allows
    /// while Motty is in front, and bounces the Dock icon when it isn't.
    pub(super) fn banner(
        &mut self,
        view: &Entity<TerminalView>,
        title: &str,
        body: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use motty_theme::NotificationForeground;
        let in_front = cx.active_window().is_some();
        let show = !in_front
            || match self.config.notification_foreground {
                NotificationForeground::Off => false,
                NotificationForeground::Always => true,
                NotificationForeground::TabUnfocused => !self.looking_at(view, window),
            };
        if show {
            crate::notifications::notify(title, body);
            if !in_front {
                crate::notifications::bounce_dock();
            }
        }
    }

    /// OSC 52 copy: `clipboard-write`, under Clipboard — Shell Controlled.
    pub(super) fn clipboard_write(
        &mut self,
        view: &Entity<TerminalView>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use motty_theme::Access;
        if !self.badges_for(view).clipboard_shell {
            return;
        }
        match self.config.clipboard_write {
            Access::Deny => {}
            Access::Allow => cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)),
            Access::Ask => {
                let program = clipboard_asker(view, cx);
                let answer = window.prompt(
                    gpui::PromptLevel::Info,
                    &format!("Allow {program} to copy to the clipboard?"),
                    Some(&format!("{} characters", text.chars().count())),
                    &["Allow", "Deny"],
                    cx,
                );
                cx.spawn(async move |_, cx| {
                    if answer.await == Ok(0) {
                        cx.update(|cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text))
                        });
                    }
                })
                .detach();
            }
        }
    }

    /// OSC 52 paste: `clipboard-read` (ask by default: a program reading the
    /// clipboard could take what you copied elsewhere).
    pub(super) fn clipboard_read(
        &mut self,
        view: &Entity<TerminalView>,
        format: std::sync::Arc<dyn Fn(&str) -> String + Send + Sync>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use motty_theme::Access;
        if !self.badges_for(view).clipboard_shell {
            return;
        }
        let answer = move |view: &Entity<TerminalView>, cx: &mut gpui::App| {
            let text = cx
                .read_from_clipboard()
                .and_then(|c| c.text())
                .unwrap_or_default();
            view.read(cx).answer_clipboard(&*format, &text);
        };
        match self.config.clipboard_read {
            Access::Deny => {}
            Access::Allow => answer(view, cx),
            Access::Ask => {
                let program = clipboard_asker(view, cx);
                let reply = window.prompt(
                    gpui::PromptLevel::Warning,
                    &format!("Allow {program} to read the clipboard?"),
                    Some("It asked for what you last copied (OSC 52)."),
                    // Deny is the default button, so a stray Return
                    // doesn't hand the clipboard over.
                    &["Deny", "Allow"],
                    cx,
                );
                let view = view.clone();
                cx.spawn(async move |_, cx| {
                    if reply.await == Ok(1) {
                        cx.update(|cx| answer(&view, cx));
                    }
                })
                .detach();
            }
        }
    }

    /// Looking at a tab clears the marks of what happened while away; a
    /// spinner or a hand stays while it's true.
    pub(super) fn clear_seen(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(panes) = self.tabs.get(ix).and_then(|t| t.panes()) else {
            return;
        };
        for view in panes.terminals() {
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
    /// Notifications & Privileges for the tab `view` is in.
    pub(super) fn badges_for(&self, view: &Entity<TerminalView>) -> motty_theme::Badges {
        self.tab_of(view)
            .and_then(|ix| self.tabs[ix].privileges)
            .unwrap_or(self.config.badges)
    }

    fn touched(&mut self, view: &Entity<TerminalView>, cx: &mut Context<Self>) {
        if let Some(ix) = self.tab_of(view) {
            self.tabs[ix].updated = SystemTime::now();
        }
        cx.notify();
    }

    /// Whether an agent in this window is working on something, for
    /// Prevent Sleep While Processing.
    pub fn agent_working(&self, cx: &gpui::App) -> bool {
        self.panes().any(|(ix, v)| {
            self.tabs[ix]
                .privileges
                .unwrap_or(self.config.badges)
                .caffeinate
                && v.read(cx).status.as_ref().is_some_and(|s| {
                    s.agent.is_some() && s.in_turn && s.badge != Some(Badge::Awaiting)
                })
        })
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

/// Who's asking, for the clipboard prompts: the program in front, else the
/// pane.
fn clipboard_asker(view: &Entity<TerminalView>, cx: &gpui::App) -> String {
    let v = view.read(cx);
    match &v.foreground {
        Some(program) => format!("\u{201c}{program}\u{201d}"),
        None => "the program in this pane".to_owned(),
    }
}
