//! Custom views (`details-view`): a program in an embedded terminal, or a
//! web page, as a tab of the panel; Add View…, Configure… and Close.
//!
//! A program view's command and folder take `${…}` variables from the
//! focused pane, so it can follow focus: views that resolve to the same
//! command and folder share one program, and at most [`MAX_RUNNING`] are
//! kept, the least recently shown going first.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{
    AnyElement, App, Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px,
};
use motty_theme::details_view::{self, DetailsView, ViewKind};

use super::{CONTENT_TOP, FieldId, INSET, PanelColors, Workspace, header};
use crate::terminal_view::{TerminalEvent, TerminalView};
use crate::web_view::{WebView, WebViewOptions};

/// Programs kept running in a window.
const MAX_RUNNING: usize = 6;

#[derive(Default)]
pub(in crate::workspace) struct Views {
    /// Least recently shown first.
    running: Vec<Running>,
    web: HashMap<String, WebPage>,
    /// Add View's form is showing.
    pub(super) adding: Option<AddForm>,
}

struct Running {
    view: String,
    command: String,
    cwd: PathBuf,
    terminal: Entity<TerminalView>,
    exited: bool,
    _events: Subscription,
}

struct WebPage {
    url: String,
    mobile: bool,
    page: Entity<WebView>,
}

#[derive(Default)]
pub(super) struct AddForm {
    web: bool,
    error: Option<String>,
}

impl Workspace {
    /// The strip's tabs, in order: the switched-on ones, and the one
    /// showing even when it's switched off.
    pub(super) fn details_tabs(&self) -> Vec<DetailsView> {
        let shown = self.details_selected();
        details_view::resolve(&self.config.details_views)
            .into_iter()
            .filter(|v| v.enabled || v.id == shown)
            .collect()
    }

    /// The custom view's body: its terminal or page.
    pub(super) fn render_view(
        &mut self,
        id: &str,
        window: &mut Window,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = details_view::resolve(&self.config.details_views)
            .into_iter()
            .find(|v| v.id == id);
        let note = |text: String| {
            div()
                .px(px(INSET))
                .pt(px(12.))
                .text_color(c.tertiary)
                .child(text)
                .into_any_element()
        };
        let Some(view) = view else {
            return note(format!("There's no view named “{id}”."));
        };
        match view.kind {
            ViewKind::Web => {
                let page = self.web_page(&view, cx);
                fill(page.into_any_element())
            }
            ViewKind::Tui => {
                let ix = match self.view_program(&view, window, cx) {
                    Ok(ix) => ix,
                    Err(e) => return note(format!("Couldn't start the view: {e}")),
                };
                let run = &self.details.views.running[ix];
                if run.exited {
                    let restart = ix;
                    return div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(note(format!("“{}” exited.", run.command)))
                        .child(
                            div()
                                .id("details-view-restart")
                                .mx(px(INSET - 8.))
                                .px(px(8.))
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .text_color(c.accent)
                                .hover(|s| s.bg(c.hover))
                                .child("Restart")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let running = &mut this.details.views.running;
                                    if restart < running.len() {
                                        running.remove(restart);
                                    }
                                    cx.notify();
                                })),
                        )
                        .into_any_element();
                }
                fill(run.terminal.clone().into_any_element())
            }
            ViewKind::Builtin => div().into_any_element(),
        }
    }

    fn web_page(&mut self, view: &DetailsView, cx: &mut Context<Self>) -> Entity<WebView> {
        let url = view.url.clone().unwrap_or_default();
        let pages = &mut self.details.views.web;
        if let Some(p) = pages.get(&view.id)
            && p.url == url
            && p.mobile == view.mobile
        {
            return p.page.clone();
        }
        let dark = self.theme.meta.mode == motty_theme::Mode::Dark;
        let background: gpui::Rgba = motty_render::hsla(self.theme.details_panel.background).into();
        let options = WebViewOptions {
            mobile: view.mobile,
            ..Default::default()
        };
        let page = cx.new(|cx| {
            let mut page = WebView::new(options, cx);
            page.set_appearance(dark);
            page.set_background(background, cx);
            page.load_url(&url);
            page
        });
        pages.insert(
            view.id.clone(),
            WebPage {
                url,
                mobile: view.mobile,
                page: page.clone(),
            },
        );
        page
    }

    /// The running program for `view` as the focused pane resolves it,
    /// started when there's none; its index in `running`.
    fn view_program(
        &mut self,
        view: &DetailsView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<usize> {
        let vars = self.view_vars(cx);
        let lookup = |name: &str| vars.get(name).cloned();
        let command = expand(view.command.as_deref().unwrap_or_default(), &lookup);
        let cwd = view
            .cwd
            .as_deref()
            .map(|d| expand_home(&expand(d, &lookup)))
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or_else(|| {
                let home = view.home();
                if let Err(e) = std::fs::create_dir_all(&home) {
                    tracing::warn!("couldn't create {}: {e}", home.display());
                }
                home
            });
        let running = &mut self.details.views.running;
        if let Some(ix) = running
            .iter()
            .position(|r| r.view == view.id && r.command == command && r.cwd == cwd)
        {
            // The most recently shown goes last.
            let run = running.remove(ix);
            running.push(run);
            return Ok(running.len() - 1);
        }
        let terminal = self.view_terminal(&command, &cwd, window, cx)?;
        let id = terminal.entity_id();
        let events = cx.subscribe(&terminal, move |this, _, event: &TerminalEvent, cx| {
            if matches!(event, TerminalEvent::Exited)
                && let Some(run) = this
                    .details
                    .views
                    .running
                    .iter_mut()
                    .find(|r| r.terminal.entity_id() == id)
            {
                run.exited = true;
                cx.notify();
            }
        });
        let running = &mut self.details.views.running;
        running.push(Running {
            view: view.id.clone(),
            command,
            cwd,
            terminal,
            exited: false,
            _events: events,
        });
        if running.len() > MAX_RUNNING {
            running.remove(0);
        }
        Ok(running.len() - 1)
    }

    /// A terminal running `command` through the user's shell, sized to
    /// the panel. It isn't a pane: it can't be split, closed or found.
    fn view_terminal(
        &mut self,
        command: &str,
        cwd: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Entity<TerminalView>> {
        let config = &self.config;
        let style = super::super::terminal_style(config, &self.theme, cx);
        let size = gpui::size(
            px(self.details_width()),
            (window.viewport_size().height - px(CONTENT_TOP)).max(px(100.)),
        );
        let grid = motty_render::grid_for(&style, window, size);
        let id = crate::terminal_view::next_pane_id();
        let shell = config
            .shell
            .clone()
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/zsh".into());
        let program = (shell, vec!["-l".into(), "-c".into(), command.to_owned()]);
        let (session, events) = super::super::spawn_shell(
            config,
            Some(program),
            Some(cwd.to_path_buf()),
            None,
            grid,
            id,
        )?;
        let (meta, blink) = (config.option_as_meta, config.cursor_blink);
        Ok(cx.new(|cx| {
            let mut view = TerminalView::new(id, session, events, style, meta, blink, window, cx);
            view.set_link_options(crate::terminal_view::LinkOptions::from_config(config));
            view.set_behavior(config.behavior, config.progress_commands());
            view
        }))
    }

    /// The focused pane's `${…}` variables.
    fn view_vars(&self, cx: &App) -> HashMap<&'static str, String> {
        let mut vars = HashMap::new();
        if let Some(cwd) = &self.details.cwd {
            vars.insert("cwd", cwd.display().to_string());
            if let Some(name) = cwd.file_name() {
                vars.insert("folder", name.to_string_lossy().into_owned());
            }
        }
        if let Some(path) = self
            .focused_editor()
            .and_then(|e| e.read(cx).path().map(|p| p.display().to_string()))
        {
            vars.insert("file", path);
        }
        let mut user = std::env::var("USER").ok();
        let mut host = local_host();
        if let Some(view) = self.focused_pane() {
            let v = view.read(cx);
            vars.insert("pid", v.session().pid().to_string());
            let found = [
                ("command", v.command.clone()),
                ("branch", v.branch.clone()),
                ("shell", v.process.clone()),
                ("agent", v.status.as_ref().and_then(|s| s.agent_id.clone())),
            ];
            for (name, value) in found {
                if let Some(value) = value {
                    vars.insert(name, value);
                }
            }
            if let Some(dest) = v.system.ssh.as_deref().filter(|d| !d.is_empty()) {
                match dest.split_once('@') {
                    Some((u, h)) => (user, host) = (Some(u.to_owned()), Some(h.to_owned())),
                    None => host = Some(dest.to_owned()),
                }
            }
        }
        if !vars.contains_key("branch")
            && let Some(git) = &self.details.git
        {
            vars.insert("branch", git.branch.clone());
        }
        vars.extend(user.map(|u| ("user", u)));
        vars.extend(host.map(|h| ("host", h)));
        vars
    }

    /// Opens Add View's form in the panel.
    pub(super) fn show_add_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.details.visible = true;
        self.details.views.adding = Some(AddForm::default());
        self.details.field(FieldId::ViewName).text.clear();
        self.details.field(FieldId::ViewTarget).text.clear();
        self.focus_field(FieldId::ViewName, window, cx);
    }

    pub(super) fn render_add_view(
        &mut self,
        window: &mut Window,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(form) = &self.details.views.adding else {
            return div().into_any_element();
        };
        let web = form.web;
        let error = form.error.clone();
        let segment = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(if on { c.text } else { c.secondary })
                .when(on, |d| d.bg(c.selected))
                .child(label)
        };
        let kinds = div()
            .mx(px(INSET - 8.))
            .p(px(2.))
            .flex()
            .rounded(px(7.))
            .bg(c.hover)
            .child(
                segment("details-add-tui", "Terminal Program", !web).on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Some(f) = &mut this.details.views.adding {
                            f.web = false;
                        }
                        cx.notify();
                    },
                )),
            )
            .child(
                segment("details-add-web", "Web Page", web).on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Some(f) = &mut this.details.views.adding {
                            f.web = true;
                        }
                        cx.notify();
                    },
                )),
            );
        let label = |text: &'static str| {
            div()
                .px(px(INSET))
                .pt(px(10.))
                .pb(px(4.))
                .text_size(px(12.))
                .text_color(c.secondary)
                .child(text)
        };
        let name = self.render_field(FieldId::ViewName, "Docker", window, c, cx);
        let (target_label, placeholder, hint) = if web {
            (
                "URL",
                "http://localhost:3000",
                "Loads the site's phone layout, which fits the panel.",
            )
        } else {
            (
                "Command",
                "lazygit -p \"${cwd}\"",
                "Runs through your shell. ${cwd} ${folder} ${file} ${branch} ${pid} \
                 ${command} ${agent} ${host} ${user} ${shell} follow the focused pane; \
                 ${file|cwd} takes the first that's set.",
            )
        };
        let target = self.render_field(FieldId::ViewTarget, placeholder, window, c, cx);
        let button = |id: &'static str, text: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(24.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .map(|d| {
                    if primary {
                        d.bg(c.accent).text_color(gpui::white())
                    } else {
                        d.text_color(c.secondary).hover(|s| s.bg(c.hover))
                    }
                })
                .child(text)
        };
        div()
            .flex()
            .flex_col()
            .pt(px(12.))
            .child(header("Add View", c))
            .child(kinds)
            .child(label("Name"))
            .child(div().px(px(INSET - 8.)).child(name))
            .child(label(target_label))
            .child(div().px(px(INSET - 8.)).child(target))
            .child(
                div()
                    .px(px(INSET))
                    .pt(px(6.))
                    .text_size(px(11.5))
                    .text_color(c.tertiary)
                    .child(hint),
            )
            .children(error.map(|e| {
                div()
                    .px(px(INSET))
                    .pt(px(8.))
                    .text_size(px(12.))
                    .text_color(c.danger)
                    .child(e)
            }))
            .child(
                div()
                    .px(px(INSET - 8.))
                    .pt(px(14.))
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        button("details-add-cancel", "Cancel", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.details.views.adding = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        button("details-add-ok", "Add View", true)
                            .on_click(cx.listener(|this, _, _, cx| this.add_view(cx))),
                    ),
            )
            .into_any_element()
    }

    /// Saves the form as a new view at the strip's end and shows it.
    pub(super) fn add_view(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.details.views.adding else {
            return;
        };
        let web = form.web;
        let name = self.details.field(FieldId::ViewName).text.trim().to_owned();
        let target = self
            .details
            .field(FieldId::ViewTarget)
            .text
            .trim()
            .to_owned();
        let missing = if name.is_empty() {
            Some("Give the view a name.")
        } else if target.is_empty() && web {
            Some("Enter the page's URL.")
        } else if target.is_empty() {
            Some("Enter the command to run.")
        } else {
            None
        };
        if let Some(msg) = missing {
            if let Some(f) = &mut self.details.views.adding {
                f.error = Some(msg.into());
            }
            cx.notify();
            return;
        }
        let mut list = details_view::resolve(&self.config.details_views);
        let id = details_view::unique_id(&name, &list);
        let mut view = DetailsView::builtin(&id);
        view.title = Some(name);
        if web {
            view.kind = ViewKind::Web;
            view.url = Some(if target.contains("://") {
                target
            } else {
                format!("http://{target}")
            });
        } else {
            view.kind = ViewKind::Tui;
            view.command = Some(target);
        }
        list.push(view);
        if let Err(e) = self.save_views(list, cx) {
            if let Some(f) = &mut self.details.views.adding {
                f.error = Some(format!("Couldn't save config.toml: {e}"));
            }
            cx.notify();
            return;
        }
        self.details.views.adding = None;
        self.details.view = Some(id);
        cx.notify();
    }

    /// Takes a view's tab out of the strip (`enabled="false"`), stopping
    /// its programs.
    pub(super) fn close_view(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut list = details_view::resolve(&self.config.details_views);
        for v in list.iter_mut().filter(|v| v.id == id) {
            v.enabled = false;
        }
        if let Err(e) = self.save_views(list, cx) {
            tracing::warn!("couldn't save details-view: {e}");
            return;
        }
        let views = &mut self.details.views;
        views.running.retain(|r| r.view != id);
        views.web.remove(id);
        if self.details.view.as_deref() == Some(id) {
            self.details.view = None;
        }
        cx.notify();
    }

    /// Writes the views to config.toml, which every window then reloads.
    fn save_views(
        &mut self,
        list: Vec<DetailsView>,
        cx: &mut Context<Self>,
    ) -> std::io::Result<()> {
        details_view::save(&list)?;
        self.config.details_views = list;
        cx.defer(super::super::reload_all);
        Ok(())
    }

    /// Opens config.toml in an editor tab at view `id`'s line.
    pub(super) fn configure_view(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let line = self
            .config
            .path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|src| details_view::line_of(&src, id));
        self.open_config_at(line, window, cx);
    }

    /// The `open-with-app` lines in config.toml (Motty's Settings has no
    /// Open With section yet).
    pub(super) fn configure_open_with(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let line = self
            .config
            .path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|src| {
                src.lines()
                    .position(|l| l.trim_start().starts_with("open-with-app"))
            });
        self.open_config_at(line, window, cx);
    }

    /// config.toml in an editor tab, at `line` when given; in the default
    /// editor when there's no file yet.
    fn open_config_at(&mut self, line: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.config.path.clone().filter(|p| p.exists()) else {
            self.open_config_file();
            return;
        };
        self.open_editor(Some(path), window, cx);
        if let (Some(line), Some(editor)) = (line, self.focused_editor()) {
            editor.update(cx, |e, cx| e.show_match(line, 0..0, cx));
        }
    }
}

/// `child` over the whole body.
fn fill(child: AnyElement) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .child(child)
        .into_any_element()
}

/// `${name}` replaced by the variable's value; `${a|b}` takes the first
/// that's set; unknown or unset ones become nothing.
pub(super) fn expand(template: &str, vars: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let value = after[..end]
            .split('|')
            .find_map(|name| vars(name.trim()).filter(|v| !v.is_empty()))
            .unwrap_or_default();
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => dirs::home_dir()
            .unwrap_or_default()
            .join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

fn local_host() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is as large as the call is told it is.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    ok.then(|| String::from_utf8_lossy(&buf[..end]).into_owned())
        .filter(|h| !h.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_variables() {
        let vars = |name: &str| match name {
            "cwd" => Some("/a b".to_owned()),
            "file" => Some(String::new()),
            _ => None,
        };
        assert_eq!(
            expand("lazygit -p \"${cwd}\"", &vars),
            "lazygit -p \"/a b\""
        );
        assert_eq!(expand("bat ${file|cwd}", &vars), "bat /a b");
        assert_eq!(expand("x ${nope} y", &vars), "x  y");
        assert_eq!(expand("echo ${cwd", &vars), "echo ${cwd");
        assert_eq!(expand_home("/tmp"), PathBuf::from("/tmp"));
        assert!(expand_home("~/x").ends_with("x"));
    }
}
