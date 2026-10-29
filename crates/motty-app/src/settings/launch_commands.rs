//! Settings › Integrations › Coding Agents: each agent's row expands to its
//! launch command (an `agent-command` line, the program Motty starts,
//! resumes and forks it with) and the folder it keeps its settings in.

use gpui::{AnyElement, Context, Keystroke, SharedString, div, prelude::*, px, svg};
use motty_render::hsla;
use motty_theme::Rgba;

use super::{Colors, SettingsWindow, tilde};
use crate::agents::{self, hooks};

/// Which agent is expanded, and its launch command while it's typed.
#[derive(Default)]
pub(super) struct LaunchCommands {
    pub(super) open: Option<usize>,
    editing: Option<(usize, String)>,
}

impl LaunchCommands {
    /// Leaving the section drops what was typed.
    pub(super) fn stop_editing(&mut self) {
        self.editing = None;
    }
}

impl SettingsWindow {
    /// Typing into the launch command field; true when the key was taken.
    pub(super) fn launch_command_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some((ix, text)) = self.launch.editing.as_mut() else {
            return false;
        };
        let m = &ks.modifiers;
        match ks.key.as_str() {
            "escape" => self.launch.editing = None,
            "enter" | "tab" => {
                let (ix, text) = (*ix, text.clone());
                self.launch.editing = None;
                self.save_launch_command(ix, &text, cx);
            }
            "backspace" if m.alt || m.platform => {
                let keep = text.trim_end().rfind([' ', '/']).map_or(0, |i| i + 1);
                text.truncate(if m.platform { 0 } else { keep });
            }
            "backspace" => {
                text.pop();
            }
            "v" if m.platform => {
                let pasted = cx.read_from_clipboard().and_then(|c| c.text());
                // One line: a command, not a script.
                text.push_str(
                    pasted
                        .as_deref()
                        .unwrap_or_default()
                        .trim()
                        .lines()
                        .next()
                        .unwrap_or_default(),
                );
            }
            // ⌘W and the rest go on to the window.
            _ if m.platform || m.control => return false,
            _ => match ks.key_char.as_deref() {
                Some(c) if !c.chars().any(char::is_control) => text.push_str(c),
                _ => return true,
            },
        }
        cx.notify();
        true
    }

    fn save_launch_command(&mut self, ix: usize, command: &str, cx: &mut Context<Self>) {
        let agent = &agents::CATALOG[ix];
        match motty_theme::config::save_agent_command(agent.id, command) {
            Ok(_) => {
                self.status = None;
                crate::workspace::reload_all(cx);
                self.config = crate::load_config();
            }
            Err(e) => self.status = Some((false, format!("Couldn't save config.toml: {e}"))),
        }
        cx.notify();
    }

    /// The chevron that expands an agent's row.
    pub(super) fn launch_chevron(
        &self,
        ix: usize,
        agent: usize,
        open: bool,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(("launch-details", ix))
            .flex_none()
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .hover(|s| s.bg(c.selected))
            .child(
                svg()
                    .path("icons/chevron-down.svg")
                    .size(px(13.))
                    .text_color(c.secondary)
                    .with_transformation(gpui::Transformation::rotate(gpui::radians(if open {
                        0.
                    } else {
                        -std::f32::consts::FRAC_PI_2
                    }))),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.launch.open = (this.launch.open != Some(agent)).then_some(agent);
                this.launch.editing = None;
                cx.notify();
            }))
            .into_any_element()
    }

    /// An expanded agent: its launch command field and its config home.
    pub(super) fn launch_details(
        &self,
        ix: usize,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let agent = &agents::CATALOG[ix];
        let saved = self.config.agent_command(agent.id).map(str::to_owned);
        let editing = self
            .launch
            .editing
            .as_ref()
            .filter(|(i, _)| *i == ix)
            .map(|(_, t)| t.clone());
        let accent = hsla(Rgba::rgb(0x047afe));
        let shown = editing.clone().or(saved.clone()).unwrap_or_default();
        let line = |text: String, mono: bool| {
            div()
                .text_size(px(11.))
                .text_color(c.secondary)
                .when(mono, |d| d.font_family("Menlo"))
                .child(SharedString::from(text))
        };
        let heading = |text: &'static str| {
            div()
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c.text)
                .child(text)
        };
        let field = div()
            .id(("launch-command", ix))
            .flex_1()
            .min_w_0()
            .h(px(26.))
            .px(px(8.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(c.control)
            .border(px(if editing.is_some() { 1.5 } else { 0.5 }))
            .border_color(if editing.is_some() {
                accent
            } else {
                c.control_border
            })
            .font_family("Menlo")
            .text_size(px(11.5))
            .when(shown.is_empty(), |d| {
                d.text_color(c.secondary).child(agent.command)
            })
            .when(!shown.is_empty(), |d| {
                d.text_color(c.text)
                    .child(div().min_w_0().truncate().child(SharedString::from(shown)))
            })
            .when(editing.is_some(), |d| {
                d.child(div().w(px(1.5)).h(px(14.)).bg(accent))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                let current = this
                    .config
                    .agent_command(agents::CATALOG[ix].id)
                    .unwrap_or_default()
                    .to_owned();
                this.launch.editing = Some((ix, current));
                window.focus(&this.focus, cx);
                cx.notify();
            }));
        let reset = saved.filter(|_| editing.is_none()).map(|_| {
            div()
                .id(("launch-reset", ix))
                .flex_none()
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .text_color(c.secondary)
                .hover(|s| s.bg(c.selected))
                .child("✕")
                .on_click(cx.listener(move |this, _, _, cx| this.save_launch_command(ix, "", cx)))
        });
        let env = hooks::Env::current();
        let home = hooks::config_home(agent.id, &env);
        let moved = home
            .as_ref()
            .and_then(|(_, var)| *var)
            .map(|var| match (env.var)(var) {
                Some(_) => format!("From ${var}."),
                None => format!("Set ${var} to keep it somewhere else."),
            });
        div()
            .ml(px(38.))
            .mb(px(10.))
            .p(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .rounded(px(7.))
            .bg(c.field)
            .child(heading("Launch Command"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(field)
                    .children(reset),
            )
            .child(line(
                format!(
                    "Used to start, resume and fork {}; Motty adds the session arguments. \
                     Empty uses the built-in command.",
                    agent.label
                ),
                false,
            ))
            .children(home.map(|(dir, _)| {
                div()
                    .mt(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(heading("Config Home"))
                    .child(line(tilde(&dir), true))
                    .children(moved.map(|m| line(m, false)))
            }))
            .into_any_element()
    }
}
