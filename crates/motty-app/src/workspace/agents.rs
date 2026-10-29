//! Coding agents across panes: Send to Chat (⌘⌃↩) hands the selection or
//! the last command's output to an agent's Composer, and agents start,
//! resume and fork in a split, a tab or a window.

use std::path::PathBuf;

use gpui::{
    AnyWindowHandle, App, AppContext, Context, Entity, EntityId, Global, PromptLevel, SharedString,
    Window,
};

use super::{Tab, Workspace, terminal_style};
use crate::actions::ResumeSession;
use crate::agents::{self, sessions};
use crate::editor::FileEditor;
use crate::native_menu::{self, Entry, Item};
use crate::panes::Direction;
use crate::terminal_view::{Chip, TerminalView};

/// Where a new agent goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Tab,
    Window,
    Split(Direction),
}

impl Place {
    /// `tab`, `window`, or a side for a split; a tab when unknown.
    pub fn from_key(key: &str) -> Self {
        match key {
            "window" => Place::Window,
            "right" => Place::Split(Direction::Right),
            "left" => Place::Split(Direction::Left),
            "down" => Place::Split(Direction::Down),
            "up" => Place::Split(Direction::Up),
            _ => Place::Tab,
        }
    }
}

/// The agent Send to Chat used last: first in the picker.
struct LastChat(EntityId);

impl Global for LastChat {}

/// An agent pane, wherever it is.
struct ChatTarget {
    window: AnyWindowHandle,
    view: Entity<TerminalView>,
    label: String,
}

/// The agent running in `view`: reported by its hooks, or recognised by
/// its program's name.
pub fn agent_in(view: &TerminalView) -> Option<String> {
    if let Some(agent) = view.status.as_ref().and_then(|s| s.agent.clone()) {
        return Some(agent);
    }
    let process = view.process.as_deref()?;
    Some(agents::by_command(process)?.label.to_owned())
}

/// The first catalog agent installed here.
fn default_agent() -> Option<&'static agents::Agent> {
    agents::CATALOG
        .iter()
        .find(|a| agents::locate(a.command).is_some())
}

fn deliver(target: &ChatTarget, chip: Chip, cx: &mut App) {
    cx.set_global(LastChat(target.view.entity_id()));
    let view = target.view.clone();
    let Some(ws) = target.window.downcast::<Workspace>() else {
        return;
    };
    let _ = ws.update(cx, |ws, window, cx| {
        ws.reveal_pane(&view, window, cx);
        view.update(cx, |v, cx| v.attach(chip, window, cx));
        window.activate_window();
    });
}

impl Workspace {
    /// Runs `line` (unless empty) in a new shell at `place`, in `cwd`.
    pub fn start_in(
        &mut self,
        line: &str,
        cwd: Option<PathBuf>,
        place: Place,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalView>> {
        let view = match place {
            Place::Tab => self.open_tab_with(None, cwd, window, cx),
            Place::Split(direction) => self.split_pane(direction, None, cwd, true, window, cx),
            Place::Window => {
                let line = line.to_owned();
                let config = self.config.clone();
                cx.defer(move |cx| {
                    if let Err(e) = crate::open_window_running(config, cwd, &line, cx) {
                        tracing::error!("failed to open a window: {e:#}");
                    }
                });
                return None;
            }
        };
        match view {
            Ok(view) => {
                if !line.is_empty() {
                    let line = format!("{line}\r");
                    view.update(cx, |v, cx| v.write_input(&line, cx));
                }
                Some(view)
            }
            Err(e) => {
                tracing::error!("failed to start shell: {e:#}");
                None
            }
        }
    }

    /// Every agent pane in every window, the one used last first.
    fn chat_targets(&self, window: &Window, cx: &App) -> Vec<ChatTarget> {
        let this = window.window_handle();
        let mut targets = Vec::new();
        for handle in cx.windows() {
            let Some(ws_handle) = handle.downcast::<Workspace>() else {
                continue;
            };
            // This window's workspace is `self` (and is being updated).
            let ws = if handle == this {
                Some(self)
            } else {
                ws_handle.read(cx).ok()
            };
            let Some(ws) = ws else { continue };
            for (_, view) in ws.panes() {
                let v = view.read(cx);
                if let Some(agent) = agent_in(v) {
                    targets.push(ChatTarget {
                        window: handle,
                        view: view.clone(),
                        label: format!("{agent} — {}", v.display_title()),
                    });
                }
            }
        }
        if let Some(last) = cx.try_global::<LastChat>().map(|l| l.0)
            && let Some(ix) = targets.iter().position(|t| t.view.entity_id() == last)
        {
            let t = targets.remove(ix);
            targets.insert(0, t);
        }
        targets
    }

    /// What Send to Chat takes: the selection, else the last command and
    /// its output; an editor tab's selected lines.
    fn chat_source(&self, cx: &App) -> Option<(Option<Entity<TerminalView>>, Chip)> {
        match &self.tabs.get(self.active)?.content {
            super::Content::Search(_) => None,
            super::Content::Panes(t) => {
                if let Some(view) = t.terminal() {
                    let chip = view.read(cx).chat_context()?;
                    return Some((Some(view.clone()), chip));
                }
                let editor = t.focused.file()?.read(cx);
                let text = editor.selected_text()?;
                Some((
                    None,
                    Chip {
                        label: format!("From {}", editor.title()),
                        text,
                    },
                ))
            }
        }
    }

    /// ⌘⌃↩: the context goes to the agent's Composer, with a picker when
    /// several are running; with none, one starts beside the pane.
    pub(super) fn send_to_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((source, chip)) = self.chat_source(cx) else {
            return window.play_system_bell();
        };
        let mut targets = self.chat_targets(window, cx);
        // An agent pane's own text goes to another agent.
        if targets.len() > 1 {
            targets.retain(|t| Some(&t.view) != source.as_ref());
        }
        match targets.len() {
            0 => self.chat_in_new_agent(chip, window, cx),
            1 => {
                let target = targets.remove(0);
                if target.window == window.window_handle() {
                    cx.set_global(LastChat(target.view.entity_id()));
                    self.reveal_pane(&target.view, window, cx);
                    target.view.update(cx, |v, cx| v.attach(chip, window, cx));
                } else {
                    cx.defer(move |cx| deliver(&target, chip, cx));
                }
            }
            _ => {
                let mut entries: Vec<Entry> = targets
                    .iter()
                    .map(|t| Item::new(t.label.clone()).into())
                    .collect();
                entries.push(Entry::Separator);
                entries.push(Item::new("New Session").into());
                let at = window.mouse_position();
                let this = cx.entity().downgrade();
                native_menu::popup(entries, at, Some(0), window, cx, move |ix, window, cx| {
                    match targets.get(ix) {
                        Some(target) => deliver(target, chip, cx),
                        None => {
                            let _ = this
                                .update(cx, |this, cx| this.chat_in_new_agent(chip, window, cx));
                        }
                    }
                });
            }
        }
    }

    /// Starts the first installed agent beside the focused pane and opens
    /// its Composer with `chip`.
    fn chat_in_new_agent(&mut self, chip: Chip, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = default_agent() else {
            // An OK-only notice: nothing waits for the answer.
            drop(window.prompt(
                PromptLevel::Info,
                "No coding agent found",
                Some("Install Claude Code, Codex, OpenCode or another supported agent to send it context."),
                &["OK"],
                cx,
            ));
            return;
        };
        let cwd = self.active_folder(cx);
        let place = if self.active_panes().is_some() {
            Place::Split(Direction::Right)
        } else {
            Place::Tab
        };
        if let Some(view) = self.start_in(agent.command, cwd, place, window, cx) {
            cx.set_global(LastChat(view.entity_id()));
            view.update(cx, |v, cx| v.attach(chip, window, cx));
        }
    }

    /// The Composer of a pane whose tab isn't showing, when it's pinned.
    pub(super) fn render_pinned_composer(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !crate::terminal_view::composer_pinned(cx) {
            return None;
        }
        let view = self
            .panes()
            .filter(|(ix, _)| *ix != self.active)
            .map(|(_, v)| v)
            .find(|v| v.read(cx).composer_open())?
            .clone();
        view.update(cx, |v, cx| v.render_composer(cx))
    }

    /// The pane running agent session `id`, in any window.
    fn live_session(&self, id: &str, window: &Window, cx: &App) -> Option<ChatTarget> {
        let this = window.window_handle();
        cx.windows().into_iter().find_map(|handle| {
            let ws_handle = handle.downcast::<Workspace>()?;
            let ws = if handle == this {
                self
            } else {
                ws_handle.read(cx).ok()?
            };
            let view = ws.panes().map(|(_, v)| v).find(|v| {
                v.read(cx)
                    .status
                    .as_ref()
                    .is_some_and(|s| s.session.as_deref() == Some(id))
            })?;
            Some(ChatTarget {
                window: handle,
                view: view.clone(),
                label: String::new(),
            })
        })
    }

    /// Continues a past session (or shows it, while it still runs), or
    /// forks it, with the agent's own command.
    pub(super) fn resume_session(
        &mut self,
        a: &ResumeSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !a.fork
            && let Some(live) = self.live_session(&a.id, window, cx)
        {
            if live.window == window.window_handle() {
                self.reveal_pane(&live.view, window, cx);
            } else if let Some(ws) = live.window.downcast::<Workspace>() {
                let view = live.view.clone();
                cx.defer(move |cx| {
                    let _ = ws.update(cx, |ws, window, cx| {
                        ws.reveal_pane(&view, window, cx);
                        window.activate_window();
                    });
                });
            }
            return;
        }
        let command = if a.fork {
            sessions::fork_command(&a.agent, &a.id)
        } else {
            sessions::resume_command(&a.agent, &a.id)
        };
        let Some(command) = command else {
            return window.play_system_bell();
        };
        let cwd = a.cwd.clone().filter(|d| d.is_dir());
        self.start_in(&command, cwd, Place::from_key(&a.place), window, cx);
    }

    /// The focused pane's agent session, as its hooks reported it.
    pub(super) fn focused_session(&self, cx: &App) -> Option<ResumeSession> {
        let view = self.focused_pane()?;
        let v = view.read(cx);
        let status = v.status.as_ref()?;
        Some(ResumeSession {
            agent: status.agent_id.clone()?,
            id: status.session.clone()?,
            cwd: v.cwd.clone(),
            fork: false,
            place: String::new(),
        })
    }

    /// Branch in… / Fork in…: the focused agent's session, copied to `place`.
    pub(super) fn fork_focused(
        &mut self,
        place: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut session) = self.focused_session(cx) else {
            // An OK-only notice: nothing waits for the answer.
            drop(window.prompt(
                PromptLevel::Info,
                "No agent session to fork",
                Some(
                    "Motty forks a session the agent reported. Turn on its integration in Settings → Agents, then restart the agent.",
                ),
                &["OK"],
                cx,
            ));
            return;
        };
        session.fork = true;
        session.place = place.to_owned();
        self.resume_session(&session, window, cx);
    }

    /// The focused agent's session as a transcript tab.
    pub(super) fn view_focused_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(wanted) = self.focused_session(cx) else {
            return self.show_agent_history(window, cx);
        };
        cx.spawn_in(window, async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    sessions::list(&sessions::Roots::current(), usize::MAX)
                        .into_iter()
                        .find(|s| s.id == wanted.id)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match found {
                Some(session) => this.view_history(session, window, cx),
                None => this.show_agent_history(window, cx),
            });
        })
        .detach();
    }

    /// Opens `session`'s transcript in a tab, with Resume and Fork on top.
    pub(super) fn view_history(
        &mut self,
        session: sessions::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let (session, text) = cx
                .background_spawn(async move {
                    let messages = sessions::transcript(&session);
                    let text = sessions::render(&session, &messages, 100);
                    (session, text)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.open_transcript(session, text, window, cx)
            });
        })
        .detach();
    }

    fn open_transcript(
        &mut self,
        session: sessions::Session,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let resume = |fork: bool| ResumeSession {
            agent: session.agent.to_owned(),
            id: session.id.clone(),
            cwd: session.cwd.clone(),
            fork,
            place: "tab".into(),
        };
        let mut buttons: Vec<(SharedString, Box<dyn gpui::Action>)> =
            vec![("Resume".into(), Box::new(resume(false)))];
        if sessions::fork_command(session.agent, &session.id).is_some() {
            let label = format!("{} in New Tab", sessions::fork_label(session.agent));
            buttons.push((label.into(), Box::new(resume(true))));
        }
        let title: String = session.title.chars().take(40).collect();
        let name = format!("{} — {title}", agents::label_for(session.agent));
        let style = terminal_style(&self.config, &self.theme, cx);
        let editor = cx.new(|cx| FileEditor::viewer(name, text, buttons, style, cx));
        self.watch_editor(&editor, cx);
        self.tabs.push(Tab::editor(editor));
        self.activate(self.tabs.len() - 1, window, cx);
    }

    /// Open Quickly on its Agents list.
    pub(super) fn show_agent_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_open_quickly(super::open_quickly::Filter::Agents, window, cx);
    }
}
