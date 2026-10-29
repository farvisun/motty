//! The details panel (⌘⇧R): a column on the right about the focused pane,
//! mirroring the tabs panel on the left. Four tabs, picked from the strip at
//! its top: Info (working directory, open-in actions, processes, listening
//! ports), Outline (the pane's commands), Git (branch and changed files) and
//! Files (a tree of the working directory).
//!
//! Geometry is measured from Otty's screenshots at 2x; the icons are drawn
//! here in the style of the system's symbols.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    AnyElement, Context, Entity, Focusable, FontWeight, Hsla, MouseButton, ScrollHandle,
    SharedString, Task, Window, div, prelude::*, px,
};
use motty_render::hsla;
use motty_term::ProcessInfo;

use super::{Workspace, native_label};
use crate::actions::*;
use crate::terminal_view::TerminalView;
use crate::titlebar::{self, icon, stroke};

/// The strip's buttons: 24pt tall, 4pt from the top, from 8pt in; an
/// unselected tab is a 28pt box, the selected one a pill with its name.
const STRIP_TOP: f32 = titlebar::TOP;
const STRIP_LEFT: f32 = 8.;
const TAB_WIDTH: f32 = 28.;
const TAB_GAP: f32 = 4.;
const PILL_PAD: f32 = 7.;
/// Where content starts below the window's top.
const CONTENT_TOP: f32 = 44.;
const INSET: f32 = 18.5;
const ROW_HEIGHT: f32 = 28.;
const LIST_ROW: f32 = 22.;
const TREE_ROW: f32 = 25.;
const TREE_INDENT: f32 = 18.;
const SECTION_GAP: f32 = 22.;
/// The slowest refreshes run at most this often.
const GIT_INTERVAL: Duration = Duration::from_secs(3);
const PORTS_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum DetailsTab {
    #[default]
    Info,
    Outline,
    Git,
    Files,
}

impl DetailsTab {
    const ALL: [Self; 4] = [Self::Info, Self::Outline, Self::Git, Self::Files];

    fn label(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Outline => "Outline",
            Self::Git => "Git",
            Self::Files => "Files",
        }
    }
}

#[derive(Default)]
pub(super) struct Details {
    pub visible: bool,
    tab: DetailsTab,
    /// The folder the data below describes.
    cwd: Option<PathBuf>,
    pane: Option<gpui::EntityId>,
    processes: Vec<ProcessInfo>,
    ports: Vec<(u16, i32)>,
    ports_at: Option<Instant>,
    git: Option<GitSummary>,
    git_at: Option<Instant>,
    files: FileTree,
    pending: Option<Task<()>>,
    scroll: ScrollHandle,
}

/// What `git status` says about a repository.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct GitSummary {
    branch: String,
    added: u32,
    removed: u32,
    remote: Option<String>,
    staged: Vec<Change>,
    unstaged: Vec<Change>,
    untracked: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Change {
    status: char,
    path: String,
}

#[derive(Default)]
struct FileTree {
    root: Option<PathBuf>,
    expanded: HashSet<PathBuf>,
    children: HashMap<PathBuf, Vec<FileEntry>>,
    show_hidden: bool,
}

#[derive(Clone, Debug)]
struct FileEntry {
    path: PathBuf,
    name: String,
    dir: bool,
}

impl FileTree {
    fn entries(&mut self, dir: &Path) -> Vec<FileEntry> {
        if let Some(list) = self.children.get(dir) {
            return list.clone();
        }
        let mut list: Vec<FileEntry> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| FileEntry {
                name: e.file_name().to_string_lossy().into_owned(),
                dir: e.path().is_dir(),
                path: e.path(),
            })
            .collect();
        list.sort_by(|a, b| {
            b.dir
                .cmp(&a.dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.children.insert(dir.to_path_buf(), list.clone());
        list
    }

    /// The visible rows, depth first: (depth, entry).
    fn rows(&mut self) -> Vec<(usize, FileEntry)> {
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut stack: Vec<(usize, FileEntry)> = self
            .entries(&root)
            .into_iter()
            .rev()
            .map(|e| (0, e))
            .collect();
        while let Some((depth, entry)) = stack.pop() {
            if !self.show_hidden && entry.name.starts_with('.') {
                continue;
            }
            if entry.dir && self.expanded.contains(&entry.path) {
                stack.extend(
                    self.entries(&entry.path)
                        .into_iter()
                        .rev()
                        .map(|e| (depth + 1, e)),
                );
            }
            out.push((depth, entry));
            if out.len() >= 2000 {
                break;
            }
        }
        out
    }

    fn set_root(&mut self, root: Option<PathBuf>) {
        if root != self.root {
            self.root = root;
            self.expanded.clear();
            self.children.clear();
        }
    }
}

impl Workspace {
    pub(super) fn details_width(&self) -> f32 {
        let border = self
            .theme
            .details_panel
            .border
            .as_ref()
            .map_or(0., |b| b.width);
        self.config
            .details_panel_width
            .unwrap_or(self.theme.details_panel.width)
            + border
    }

    pub(super) fn toggle_details(&mut self, cx: &mut Context<Self>) {
        self.details.visible = !self.details.visible;
        self.refresh_details(true, cx);
        cx.notify();
    }

    pub(super) fn show_details_tab(&mut self, tab: DetailsTab, cx: &mut Context<Self>) {
        self.details.visible = true;
        self.details.tab = tab;
        self.refresh_details(true, cx);
        cx.notify();
    }

    /// Re-reads what the panel shows about the focused pane: processes now,
    /// git and ports in the background (at most every few seconds unless
    /// `now`).
    pub(super) fn refresh_details(&mut self, now: bool, cx: &mut Context<Self>) {
        if !self.details.visible {
            return;
        }
        let Some(view) = self.focused_pane() else {
            return;
        };
        let (cwd, processes) = {
            let v = view.read(cx);
            (v.cwd.clone(), v.session().processes())
        };
        let d = &mut self.details;
        let moved = d.cwd != cwd || d.pane != Some(view.entity_id());
        if moved {
            d.cwd = cwd.clone();
            d.pane = Some(view.entity_id());
            d.git = None;
            d.ports.clear();
            d.git_at = None;
            d.ports_at = None;
        }
        if d.processes != processes {
            d.processes = processes;
            cx.notify();
        }
        d.files.set_root(cwd.clone());
        if d.pending.is_some() {
            return;
        }
        let due =
            |at: Option<Instant>, every| now || moved || at.is_none_or(|t| t.elapsed() >= every);
        let git = (d.tab == DetailsTab::Git && due(d.git_at, GIT_INTERVAL))
            .then(|| cwd.clone())
            .flatten();
        let pids: Vec<i32> = d.processes.iter().map(|p| p.pid).collect();
        let ports =
            d.tab == DetailsTab::Info && due(d.ports_at, PORTS_INTERVAL) && !pids.is_empty();
        if git.is_none() && !ports {
            return;
        }
        let work = cx.background_spawn(async move {
            let git = git.map(|dir| git_summary(&dir));
            let ports = ports.then(|| listening_ports(&pids));
            (git, ports)
        });
        d.pending = Some(cx.spawn(async move |this, cx| {
            let (git, ports) = work.await;
            this.update(cx, |this, cx| {
                let d = &mut this.details;
                d.pending = None;
                if let Some(git) = git {
                    d.git_at = Some(Instant::now());
                    if d.git != git {
                        d.git = git;
                        cx.notify();
                    }
                }
                if let Some(ports) = ports {
                    d.ports_at = Some(Instant::now());
                    if d.ports != ports {
                        d.ports = ports;
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }

    pub(super) fn render_details(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let c = PanelColors {
            text: hsla(t.token.foreground),
            secondary: hsla(t.token.secondary),
            tertiary: hsla(t.token.tertiary),
            accent: hsla(t.token.accent),
            hover: hsla(native_label(t, 0.05, 0.08)),
            selected: hsla(native_label(t, 0.08, 0.12)),
            icon: hsla(native_label(t, 0.5, 0.55)),
            danger: hsla(t.token.danger),
            warning: hsla(t.token.warning),
            success: hsla(t.token.success),
        };
        let view = self.focused_pane();
        let body = match self.details.tab {
            DetailsTab::Info => self.render_info(view.as_ref(), &c, cx),
            DetailsTab::Outline => self.render_outline(view.as_ref(), &c, cx),
            DetailsTab::Git => self.render_git(&c),
            DetailsTab::Files => self.render_files(&c, cx),
        };
        let t = &self.theme;
        let side = &t.details_panel;
        let mut panel = div()
            .id("details-panel")
            .relative()
            .w(px(self.details_width()))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(hsla(side.background))
            .child(self.render_details_strip(&c, cx))
            .child(
                div()
                    .id("details-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.details.scroll)
                    .font_family(self.ui_font.clone())
                    .text_size(px(13.))
                    .pb(px(16.))
                    .child(body),
            );
        if let Some(b) = &side.border {
            panel = panel
                .border_l(px(b.width))
                .border_color(hsla(t.border_color(b)));
        }
        panel.into_any_element()
    }

    fn render_details_strip(&self, c: &PanelColors, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.details.tab;
        let tabs = DetailsTab::ALL.map(|tab| {
            let on = tab == selected;
            let color = if on { c.text } else { c.icon };
            div()
                .id(SharedString::from(format!("details-tab-{}", tab.label())))
                .h(px(titlebar::BUTTON))
                .flex()
                .items_center()
                .rounded(px(titlebar::RADIUS))
                .map(|d| {
                    if on {
                        d.px(px(PILL_PAD))
                            .gap(px(5.))
                            .bg(c.selected)
                            .child(tab_icon(tab, color))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(color)
                                    .child(tab.label()),
                            )
                    } else {
                        d.w(px(TAB_WIDTH))
                            .justify_center()
                            .hover(|s| s.bg(c.hover))
                            .child(tab_icon(tab, color))
                    }
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| this.show_details_tab(tab, cx)))
        });
        div()
            .flex_none()
            .h(px(CONTENT_TOP - 12.))
            .pt(px(STRIP_TOP))
            .pl(px(STRIP_LEFT))
            .flex()
            .items_start()
            .gap(px(TAB_GAP))
            .font_family(self.ui_font.clone())
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .children(tabs)
            .into_any_element()
    }

    fn render_info(
        &self,
        view: Option<&Entity<TerminalView>>,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let d = &self.details;
        let mut col = div().flex().flex_col().pt(px(12.));
        if let Some(cwd) = &d.cwd {
            col = col.child(header("Working Directory", c)).child(
                div()
                    .px(px(INSET))
                    .pb(px(10.))
                    .font_family(self.mono_font.clone())
                    .text_color(c.text)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(tilde(cwd)),
            );
            let target = view.cloned();
            let mut actions: Vec<(SharedString, Icon, Run)> = vec![
                (
                    "Copy Path".into(),
                    Icon::Copy,
                    dispatch(target.clone(), || Box::new(CopyPath)),
                ),
                (
                    "Reveal in Finder".into(),
                    Icon::Folder,
                    dispatch(target.clone(), || Box::new(RevealInFinder)),
                ),
            ];
            for (name, bundle) in open_with_apps(&self.config.open_with_apps) {
                let path = cwd.clone();
                actions.push((
                    format!("Open in {name}").into(),
                    Icon::External,
                    Box::new(move |_, _| open_in(&bundle, &path)),
                ));
            }
            for (ix, (label, glyph, run)) in actions.into_iter().enumerate() {
                col = col.child(
                    div()
                        .id(("details-action", ix))
                        .mx(px(INSET - 8.))
                        .px(px(8.))
                        .h(px(ROW_HEIGHT))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded(px(6.))
                        .text_color(c.accent)
                        .hover(|s| s.bg(c.hover))
                        .child(glyph.draw(c.accent))
                        .child(label)
                        .on_click(move |_, window, cx| run(window, cx)),
                );
            }
        }
        let now = SystemTime::now();
        col = col
            .child(div().h(px(SECTION_GAP)))
            .child(header("Process", c));
        for p in &d.processes {
            let age = p
                .started
                .and_then(|s| now.duration_since(s).ok())
                .map(short_duration)
                .unwrap_or_default();
            col = col.child(
                div()
                    .px(px(INSET))
                    .h(px(LIST_ROW))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_none().size(px(8.)).rounded_full().bg(c.success))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(c.text)
                            .child(p.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.5))
                            .text_color(c.tertiary)
                            .child(p.pid.to_string()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(c.tertiary)
                            .child(age),
                    ),
            );
        }
        col = col
            .child(div().h(px(SECTION_GAP)))
            .child(header("Ports", c));
        if d.ports.is_empty() {
            col = col.child(
                div()
                    .px(px(INSET))
                    .text_color(c.tertiary)
                    .child("No listening ports"),
            );
        }
        for (ix, &(port, pid)) in d.ports.iter().enumerate() {
            let url = format!("http://localhost:{port}");
            let name = d
                .processes
                .iter()
                .find(|p| p.pid == pid)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            col = col.child(
                div()
                    .id(("details-port", ix))
                    .mx(px(INSET - 8.))
                    .px(px(8.))
                    .h(px(LIST_ROW + 2.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(c.hover))
                    .child(
                        div()
                            .flex_1()
                            .text_color(c.accent)
                            .child(format!(":{port}")),
                    )
                    .child(div().text_size(px(11.5)).text_color(c.tertiary).child(name))
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        let _ = cx;
        col.into_any_element()
    }

    fn render_outline(
        &self,
        view: Option<&Entity<TerminalView>>,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut col = div().flex().flex_col().pt(px(8.));
        let Some(view) = view else {
            return col.into_any_element();
        };
        let (blocks, commands) = {
            let session = view.read(cx).session();
            let blocks: Vec<_> = session
                .shell()
                .blocks
                .iter()
                .rev()
                .take(300)
                .cloned()
                .collect();
            let commands: Vec<Option<String>> =
                blocks.iter().map(|b| session.block_command(b)).collect();
            (blocks, commands)
        };
        if let Some(cwd) = &self.details.cwd {
            col = col.child(
                div()
                    .px(px(INSET - 8.))
                    .h(px(LIST_ROW))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.secondary)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(format!("{}/", tilde(cwd))),
                    ),
            );
        }
        let mut any = false;
        for (ix, (block, command)) in blocks.iter().zip(commands).enumerate().rev() {
            let Some(command) = command else { continue };
            any = true;
            let ago = block
                .started
                .map(|s| format!("{} ago", short_duration(s.elapsed())))
                .unwrap_or_default();
            let failed = block.exit.is_some_and(|e| e != 0);
            let row = block.prompt_row;
            let target = view.clone();
            col = col.child(
                div()
                    .id(("details-outline", ix))
                    .mx(px(4.))
                    .px(px(INSET - 12.))
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(c.hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(if failed { c.danger } else { c.text })
                            .child(command.lines().next().unwrap_or_default().to_owned()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(c.tertiary)
                            .child(ago),
                    )
                    .on_click(move |_, window, cx| {
                        target.update(cx, |v, cx| v.reveal_row(row, cx));
                        window.focus(&target.focus_handle(cx), cx);
                    }),
            );
        }
        if !any {
            col = col.child(
                div()
                    .px(px(INSET - 8.))
                    .pt(px(6.))
                    .text_color(c.tertiary)
                    .child("No commands yet"),
            );
        }
        col.into_any_element()
    }

    fn render_git(&self, c: &PanelColors) -> AnyElement {
        let mut col = div().flex().flex_col().pt(px(12.));
        let Some(git) = &self.details.git else {
            let msg = if self.details.git_at.is_some() || self.details.cwd.is_none() {
                "Not a git repository"
            } else {
                "Loading…"
            };
            return col
                .child(div().px(px(INSET)).text_color(c.tertiary).child(msg))
                .into_any_element();
        };
        col = col.child(
            div()
                .px(px(INSET))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c.text)
                        .child(git.branch.clone()),
                )
                .when(git.added + git.removed > 0, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .text_color(c.secondary)
                            .child(format!("+{} −{}", git.added, git.removed)),
                    )
                }),
        );
        if let Some(remote) = &git.remote {
            let url = web_url(remote);
            col = col.child(
                div()
                    .id("details-git-remote")
                    .px(px(INSET))
                    .pt(px(2.))
                    .text_size(px(12.))
                    .text_color(c.accent)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .child(url.clone())
                    .when(url.starts_with("http"), |d| {
                        d.on_click(move |_, _, cx| cx.open_url(&url))
                    }),
            );
        }
        col = col.child(
            div()
                .mx(px(INSET))
                .mt(px(14.))
                .mb(px(8.))
                .h(px(1.))
                .bg(c.hover),
        );
        let root = self.details.cwd.clone();
        let sections: [(&str, Vec<Change>); 3] = [
            ("Staged", git.staged.clone()),
            ("Unstaged", git.unstaged.clone()),
            (
                "Untracked",
                git.untracked
                    .iter()
                    .map(|p| Change {
                        status: '?',
                        path: p.clone(),
                    })
                    .collect(),
            ),
        ];
        if sections.iter().all(|(_, l)| l.is_empty()) {
            col = col.child(
                div()
                    .px(px(INSET))
                    .pt(px(4.))
                    .text_color(c.tertiary)
                    .child("No changes"),
            );
        }
        let mut ix = 0usize;
        for (name, list) in sections {
            if list.is_empty() {
                continue;
            }
            col = col.child(header_text(format!("{name} ({})", list.len()), c));
            for change in list {
                ix += 1;
                let color = match change.status {
                    'A' | '?' => c.success,
                    'D' => c.danger,
                    _ => c.warning,
                };
                let path = root.as_ref().map(|r| r.join(&change.path));
                col = col.child(
                    div()
                        .id(("details-git", ix))
                        .mx(px(INSET - 8.))
                        .px(px(8.))
                        .h(px(LIST_ROW))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .rounded(px(6.))
                        .hover(|s| s.bg(c.hover))
                        .child(
                            div()
                                .flex_none()
                                .w(px(10.))
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(color)
                                .child(change.status.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(c.text)
                                .child(left_ellipsis(&change.path, 34)),
                        )
                        .when_some(path, |d, path| {
                            d.on_click(move |_, _, cx| {
                                if path.exists() {
                                    cx.open_with_system(&path)
                                }
                            })
                        }),
                );
            }
            col = col.child(div().h(px(10.)));
        }
        col.into_any_element()
    }

    fn render_files(&mut self, c: &PanelColors, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.details.files.rows();
        let hidden = self.details.files.show_hidden;
        let tools = div()
            .px(px(INSET - 8.))
            .pt(px(6.))
            .pb(px(6.))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.secondary)
                    .children(self.details.files.root.as_deref().map(tilde)),
            )
            .child(
                tool_button("details-files-up", Icon::Up, c).on_click(cx.listener(
                    |this, _, _, cx| {
                        let parent = this
                            .details
                            .files
                            .root
                            .as_deref()
                            .and_then(Path::parent)
                            .map(Path::to_path_buf);
                        if parent.is_some() {
                            this.details.files.set_root(parent);
                            cx.notify();
                        }
                    },
                )),
            )
            .child(
                tool_button(
                    "details-files-hidden",
                    if hidden { Icon::Eye } else { Icon::EyeSlash },
                    c,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.details.files.show_hidden = !this.details.files.show_hidden;
                    cx.notify();
                })),
            );
        let mut col = div().flex().flex_col().child(tools);
        for (ix, (depth, entry)) in rows.into_iter().enumerate() {
            let open = entry.dir && self.details.files.expanded.contains(&entry.path);
            let path = entry.path.clone();
            let dir = entry.dir;
            col = col.child(
                div()
                    .id(("details-file", ix))
                    .mx(px(4.))
                    .pl(px(4. + depth as f32 * TREE_INDENT))
                    .h(px(TREE_ROW))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(c.hover))
                    .child(div().flex_none().w(px(14.)).when(dir, |d| {
                        d.child(if open {
                            Icon::ChevronDown.draw(c.icon)
                        } else {
                            Icon::ChevronRight.draw(c.icon)
                        })
                    }))
                    .child(if dir {
                        Icon::Folder.draw(c.icon)
                    } else {
                        Icon::File.draw(c.icon)
                    })
                    .child(
                        div()
                            .pl(px(4.))
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(c.text)
                            .child(entry.name),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if dir {
                            let expanded = &mut this.details.files.expanded;
                            if !expanded.remove(&path) {
                                expanded.insert(path.clone());
                            }
                            cx.notify();
                        } else {
                            cx.open_with_system(&path);
                        }
                    })),
            );
        }
        col.into_any_element()
    }
}

/// What an Info action does when clicked.
type Run = Box<dyn Fn(&mut Window, &mut gpui::App)>;

struct PanelColors {
    text: Hsla,
    secondary: Hsla,
    tertiary: Hsla,
    accent: Hsla,
    hover: Hsla,
    selected: Hsla,
    icon: Hsla,
    danger: Hsla,
    warning: Hsla,
    success: Hsla,
}

fn header(text: &'static str, c: &PanelColors) -> impl IntoElement {
    header_text(text.to_owned(), c)
}

fn header_text(text: String, c: &PanelColors) -> impl IntoElement {
    div()
        .px(px(INSET))
        .pb(px(6.))
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c.tertiary)
        .child(text)
}

fn tool_button(id: &'static str, glyph: Icon, c: &PanelColors) -> gpui::Stateful<gpui::Div> {
    let hover = c.hover;
    div()
        .id(id)
        .flex_none()
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .hover(move |s| s.bg(hover))
        .child(glyph.draw(c.icon))
}

/// Runs `action` in the pane, as its key would.
fn dispatch(
    target: Option<Entity<TerminalView>>,
    action: impl Fn() -> Box<dyn gpui::Action> + 'static,
) -> Run {
    Box::new(move |window, cx| {
        if let Some(target) = &target {
            let focus = target.focus_handle(cx);
            focus.dispatch_action(&*action(), window, cx);
        }
    })
}

/// `open-with-app = "Name|bundle.id"` entries, else the editors found in
/// /Applications.
pub(crate) fn open_with_apps(configured: &[String]) -> Vec<(String, String)> {
    if !configured.is_empty() {
        return configured
            .iter()
            .filter_map(|e| {
                let (name, bundle) = e.split_once('|')?;
                Some((name.trim().to_owned(), bundle.trim().to_owned()))
            })
            .collect();
    }
    [
        ("VS Code", "Visual Studio Code", "com.microsoft.VSCode"),
        ("Cursor", "Cursor", "com.todesktop.230313mzl4w4u92"),
        ("Zed", "Zed", "dev.zed.Zed"),
        ("Xcode", "Xcode", "com.apple.dt.Xcode"),
    ]
    .into_iter()
    .filter(|(_, app, _)| Path::new(&format!("/Applications/{app}.app")).exists())
    .map(|(name, _, bundle)| (name.to_owned(), bundle.to_owned()))
    .collect()
}

pub(crate) fn open_in(bundle: &str, path: &Path) {
    let result = std::process::Command::new("/usr/bin/open")
        .arg("-b")
        .arg(bundle)
        .arg(path)
        .spawn();
    if let Err(e) = result {
        tracing::warn!("couldn't open {} with {bundle}: {e}", path.display());
    }
}

/// The installed apps that can open `target` (a path or a URL), the
/// default first, then `extra` (the configured editors) not already there;
/// as (name, bundle id).
pub(crate) fn apps_for(target: &str, extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut apps = Vec::new();
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSBundle, NSString, NSURL};
        let text = NSString::from_str(target);
        let url = if target.contains("://") {
            NSURL::URLWithString(&text)
        } else {
            Some(NSURL::fileURLWithPath(&text))
        };
        if let Some(url) = url {
            let workspace = NSWorkspace::sharedWorkspace();
            for app in workspace.URLsForApplicationsToOpenURL(&url).iter() {
                let Some(id) = NSBundle::bundleWithURL(&app).and_then(|b| b.bundleIdentifier())
                else {
                    continue;
                };
                let Some(name) = app
                    .lastPathComponent()
                    .map(|n| n.to_string().trim_end_matches(".app").to_owned())
                else {
                    continue;
                };
                let id = id.to_string();
                if !apps.iter().any(|(_, b)| *b == id) {
                    apps.push((name, id));
                }
            }
        }
    }
    for (name, bundle) in extra {
        if !apps.iter().any(|(_, b)| b == bundle) {
            apps.push((name.clone(), bundle.clone()));
        }
    }
    apps
}

pub(super) fn tilde(path: &Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rel) if rel.as_os_str().is_empty() => "~".into(),
        Some(rel) => format!("~/{}", rel.display()),
        None => path.display().to_string(),
    }
}

/// `34s`, `5m`, `2h`, `3d`.
fn short_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3_600 => format!("{}m", s / 60),
        3_600..86_400 => format!("{}h", s / 3_600),
        _ => format!("{}d", s / 86_400),
    }
}

/// The end of a long path, which says more than its start.
fn left_ellipsis(path: &str, max: usize) -> String {
    let n = path.chars().count();
    if n <= max {
        return path.to_owned();
    }
    let tail: String = path.chars().skip(n - (max - 1)).collect();
    match tail.find('/') {
        Some(i) if i < tail.len() - 1 => format!("…{}", &tail[i..]),
        _ => format!("…{tail}"),
    }
}

/// A remote as a browsable URL (`git@github.com:a/b.git` → https).
pub(crate) fn web_url(remote: &str) -> String {
    let r = remote.trim().trim_end_matches(".git");
    if let Some(rest) = r.strip_prefix("git@")
        && let Some((host, path)) = rest.split_once(':')
    {
        return format!("https://{host}/{path}");
    }
    if let Some(rest) = r.strip_prefix("ssh://git@") {
        return format!("https://{rest}");
    }
    r.to_owned()
}

fn git_summary(dir: &Path) -> Option<GitSummary> {
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let status = git(&["status", "--porcelain=v2", "--branch", "-z"])?;
    let mut summary = parse_status(&status);
    if let Some(stat) = git(&["diff", "HEAD", "--numstat"]) {
        (summary.added, summary.removed) = parse_numstat(&stat);
    }
    summary.remote = git(&["remote", "get-url", "origin"])
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    Some(summary)
}

/// `git status --porcelain=v2 --branch -z`.
fn parse_status(out: &str) -> GitSummary {
    let mut s = GitSummary::default();
    let mut fields = out.split('\0');
    while let Some(entry) = fields.next() {
        if let Some(head) = entry.strip_prefix("# branch.head ") {
            s.branch = head.to_owned();
        } else if let Some(path) = entry.strip_prefix("? ") {
            s.untracked.push(path.to_owned());
        } else if entry.starts_with("1 ") || entry.starts_with("2 ") || entry.starts_with("u ") {
            let kind = entry.as_bytes()[0];
            // Ordinary: 8 fields before the path; renamed: 9; unmerged: 10.
            let skip = match kind {
                b'1' => 8,
                b'2' => 9,
                _ => 10,
            };
            let mut parts = entry.splitn(skip + 1, ' ');
            let xy: Vec<char> = parts.nth(1).unwrap_or("..").chars().collect();
            let path = parts.nth(skip - 2).unwrap_or_default().to_owned();
            if kind == b'2' {
                fields.next(); // the original path
            }
            let (x, y) = (
                xy.first().copied().unwrap_or('.'),
                xy.get(1).copied().unwrap_or('.'),
            );
            if kind == b'u' {
                s.unstaged.push(Change { status: 'U', path });
                continue;
            }
            if x != '.' {
                s.staged.push(Change {
                    status: x,
                    path: path.clone(),
                });
            }
            if y != '.' {
                s.unstaged.push(Change { status: y, path });
            }
        }
    }
    if s.branch == "(detached)" {
        s.branch = "detached HEAD".into();
    }
    s
}

fn parse_numstat(out: &str) -> (u32, u32) {
    out.lines().fold((0, 0), |(a, r), line| {
        let mut f = line.split('\t');
        let add = f.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
        let del = f.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
        (a + add, r + del)
    })
}

/// TCP ports the processes listen on, as (port, pid).
pub(crate) fn listening_ports(pids: &[i32]) -> Vec<(u16, i32)> {
    let list = pids
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let Ok(out) = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-Fpn", "-p", &list])
        .output()
    else {
        return Vec::new();
    };
    parse_lsof(&String::from_utf8_lossy(&out.stdout))
}

/// The current user's other listening TCP ports, with their process
/// names: Shell › Ports' Other group. Root-owned system services, Motty
/// itself and `listed` are left out. Nothing is connected to.
pub(crate) fn other_listening_ports(listed: &[i32]) -> Vec<(u16, String)> {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() }.to_string();
    let Ok(out) = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-Fpcn", "-u", &uid])
        .output()
    else {
        return Vec::new();
    };
    let me = std::process::id() as i32;
    parse_lsof_named(&String::from_utf8_lossy(&out.stdout))
        .into_iter()
        .filter(|(_, pid, _)| *pid != me && !listed.contains(pid) && !system_process(*pid))
        .map(|(port, _, name)| (port, name))
        .collect()
}

/// A macOS service (AirPlay's ControlCenter and the like), by where its
/// executable is.
fn system_process(pid: i32) -> bool {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is as large as the call is told it is.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return true;
    }
    let path = String::from_utf8_lossy(&buf[..n as usize]);
    ["/System/", "/usr/libexec/", "/usr/sbin/", "/sbin/"]
        .iter()
        .any(|p| path.starts_with(p))
}

fn parse_lsof_named(out: &str) -> Vec<(u16, i32, String)> {
    let mut pid = 0;
    let mut name = String::new();
    let mut ports: Vec<(u16, i32, String)> = Vec::new();
    for line in out.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().unwrap_or(0);
        } else if let Some(c) = line.strip_prefix('c') {
            name = c.to_owned();
        } else if let Some(addr) = line.strip_prefix('n')
            && let Some(port) = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok())
            && !ports.iter().any(|(p, _, _)| *p == port)
        {
            ports.push((port, pid, name.clone()));
        }
    }
    ports.sort_unstable();
    ports
}

fn parse_lsof(out: &str) -> Vec<(u16, i32)> {
    let mut pid = 0;
    let mut ports = Vec::new();
    for line in out.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().unwrap_or(0);
        } else if let Some(addr) = line.strip_prefix('n')
            && let Some(port) = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok())
            && !ports.iter().any(|&(p, _)| p == port)
        {
            ports.push((port, pid));
        }
    }
    ports.sort_unstable();
    ports
}

#[derive(Clone, Copy)]
enum Icon {
    Copy,
    Folder,
    External,
    File,
    ChevronRight,
    ChevronDown,
    Up,
    Eye,
    EyeSlash,
}

/// A circle from four quarter arcs (k: the bezier handle length).
fn circle(
    p: &mut gpui::PathBuilder,
    at: &dyn Fn(f32, f32) -> gpui::Point<gpui::Pixels>,
    cx: f32,
    cy: f32,
    r: f32,
) {
    let k = r * 0.5523;
    p.move_to(at(cx + r, cy));
    p.cubic_bezier_to(at(cx, cy + r), at(cx + r, cy + k), at(cx + k, cy + r));
    p.cubic_bezier_to(at(cx - r, cy), at(cx - k, cy + r), at(cx - r, cy + k));
    p.cubic_bezier_to(at(cx, cy - r), at(cx - r, cy - k), at(cx - k, cy - r));
    p.cubic_bezier_to(at(cx + r, cy), at(cx + k, cy - r), at(cx + r, cy - k));
    p.close();
}

fn rounded_rect(
    p: &mut gpui::PathBuilder,
    at: &dyn Fn(f32, f32) -> gpui::Point<gpui::Pixels>,
    (x, y, w, h): (f32, f32, f32, f32),
    r: f32,
) {
    let k = r * 0.4477;
    p.move_to(at(x + r, y));
    p.line_to(at(x + w - r, y));
    p.cubic_bezier_to(at(x + w, y + r), at(x + w - k, y), at(x + w, y + k));
    p.line_to(at(x + w, y + h - r));
    p.cubic_bezier_to(
        at(x + w - r, y + h),
        at(x + w, y + h - k),
        at(x + w - k, y + h),
    );
    p.line_to(at(x + r, y + h));
    p.cubic_bezier_to(at(x, y + h - r), at(x + k, y + h), at(x, y + h - k));
    p.line_to(at(x, y + r));
    p.cubic_bezier_to(at(x + r, y), at(x, y + k), at(x + k, y));
    p.close();
}

fn folder_shape(p: &mut gpui::PathBuilder, at: &dyn Fn(f32, f32) -> gpui::Point<gpui::Pixels>) {
    p.move_to(at(1.5, 4.));
    p.line_to(at(1.5, 3.));
    p.cubic_bezier_to(at(2.5, 2.), at(1.5, 2.4), at(1.9, 2.));
    p.line_to(at(5.6, 2.));
    p.line_to(at(7., 3.5));
    p.line_to(at(12.5, 3.5));
    p.cubic_bezier_to(at(13.5, 4.5), at(13.1, 3.5), at(13.5, 3.9));
    p.line_to(at(13.5, 11.));
    p.cubic_bezier_to(at(12.5, 12.), at(13.5, 11.6), at(13.1, 12.));
    p.line_to(at(2.5, 12.));
    p.cubic_bezier_to(at(1.5, 11.), at(1.9, 12.), at(1.5, 11.6));
    p.close();
}

impl Icon {
    fn draw(self, color: Hsla) -> AnyElement {
        const W: f32 = 1.1;
        icon(15., 14., move |o, window| match self {
            Icon::Copy => {
                stroke(o, color, window, W, |p, at| {
                    rounded_rect(p, at, (4.5, 1.5, 8., 9.5), 1.5)
                });
                stroke(o, color, window, W, |p, at| {
                    p.move_to(at(2.5, 4.));
                    p.line_to(at(2.5, 11.));
                    p.cubic_bezier_to(at(4., 12.5), at(2.5, 11.9), at(3.1, 12.5));
                    p.line_to(at(9.5, 12.5));
                });
            }
            Icon::Folder => stroke(o, color, window, W, |p, at| folder_shape(p, at)),
            Icon::External => {
                stroke(o, color, window, W, |p, at| {
                    rounded_rect(p, at, (2., 1.5, 11., 11.), 2.5)
                });
                stroke(o, color, window, W, |p, at| {
                    p.move_to(at(5.5, 9.));
                    p.line_to(at(9.5, 5.));
                    p.move_to(at(6.2, 5.));
                    p.line_to(at(9.5, 5.));
                    p.line_to(at(9.5, 8.3));
                });
            }
            Icon::File => stroke(o, color, window, W, |p, at| {
                p.move_to(at(3., 2.5));
                p.cubic_bezier_to(at(4., 1.5), at(3., 1.9), at(3.4, 1.5));
                p.line_to(at(8.5, 1.5));
                p.line_to(at(12., 5.));
                p.line_to(at(12., 11.5));
                p.cubic_bezier_to(at(11., 12.5), at(12., 12.1), at(11.6, 12.5));
                p.line_to(at(4., 12.5));
                p.cubic_bezier_to(at(3., 11.5), at(3.4, 12.5), at(3., 12.1));
                p.close();
                p.move_to(at(8.5, 1.5));
                p.line_to(at(8.5, 5.));
                p.line_to(at(12., 5.));
            }),
            Icon::ChevronRight => stroke(o, color, window, 1.3, |p, at| {
                p.move_to(at(5.5, 3.5));
                p.line_to(at(9., 7.));
                p.line_to(at(5.5, 10.5));
            }),
            Icon::ChevronDown => stroke(o, color, window, 1.3, |p, at| {
                p.move_to(at(4., 5.5));
                p.line_to(at(7.5, 9.));
                p.line_to(at(11., 5.5));
            }),
            Icon::Up => stroke(o, color, window, 1.2, |p, at| {
                p.move_to(at(7.5, 12.));
                p.line_to(at(7.5, 2.5));
                p.move_to(at(3.5, 6.5));
                p.line_to(at(7.5, 2.5));
                p.line_to(at(11.5, 6.5));
            }),
            Icon::Eye | Icon::EyeSlash => {
                stroke(o, color, window, W, |p, at| {
                    p.move_to(at(1.5, 7.));
                    p.cubic_bezier_to(at(13.5, 7.), at(4., 2.5), at(11., 2.5));
                    p.cubic_bezier_to(at(1.5, 7.), at(11., 11.5), at(4., 11.5));
                    p.close();
                });
                stroke(o, color, window, W, |p, at| circle(p, at, 7.5, 7., 2.));
                if matches!(self, Icon::EyeSlash) {
                    stroke(o, color, window, 1.2, |p, at| {
                        p.move_to(at(2.5, 1.5));
                        p.line_to(at(12.5, 12.5));
                    });
                }
            }
        })
        .into_any_element()
    }
}

/// The strip's icons: info, outline (a bulleted list), git (two arrows
/// in a loop) and files (a folder).
fn tab_icon(tab: DetailsTab, color: Hsla) -> AnyElement {
    const W: f32 = 1.1;
    icon(15., 14., move |o, window| match tab {
        DetailsTab::Info => {
            stroke(o, color, window, W, |p, at| circle(p, at, 7.5, 7., 5.8));
            stroke(o, color, window, 1.3, |p, at| {
                p.move_to(at(7.5, 6.2));
                p.line_to(at(7.5, 10.));
            });
            stroke(o, color, window, 1.6, |p, at| {
                p.move_to(at(7.5, 3.9));
                p.line_to(at(7.5, 4.5));
            });
        }
        DetailsTab::Outline => {
            for y in [3.5, 7., 10.5] {
                stroke(o, color, window, 1.6, |p, at| {
                    p.move_to(at(2., y));
                    p.line_to(at(2.8, y));
                });
                stroke(o, color, window, W, |p, at| {
                    p.move_to(at(5., y));
                    p.line_to(at(13.5, y));
                });
            }
        }
        DetailsTab::Git => {
            stroke(o, color, window, W, |p, at| {
                p.move_to(at(2.5, 7.5));
                p.line_to(at(2.5, 5.5));
                p.cubic_bezier_to(at(4.5, 3.5), at(2.5, 4.4), at(3.4, 3.5));
                p.line_to(at(12., 3.5));
                p.move_to(at(10., 1.5));
                p.line_to(at(12., 3.5));
                p.line_to(at(10., 5.5));
            });
            stroke(o, color, window, W, |p, at| {
                p.move_to(at(12.5, 6.5));
                p.line_to(at(12.5, 8.5));
                p.cubic_bezier_to(at(10.5, 10.5), at(12.5, 9.6), at(11.6, 10.5));
                p.line_to(at(3., 10.5));
                p.move_to(at(5., 8.5));
                p.line_to(at(3., 10.5));
                p.line_to(at(5., 12.5));
            });
        }
        DetailsTab::Files => stroke(o, color, window, W, |p, at| folder_shape(p, at)),
    })
    .into_any_element()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_lsof_names() {
        let out =
            "p664\ncControlCenter\nf9\nn*:7000\nf10\nn*:7000\np900\ncnode\nf3\nn127.0.0.1:5173\n";
        assert_eq!(
            parse_lsof_named(out),
            vec![
                (5173, 900, "node".to_owned()),
                (7000, 664, "ControlCenter".to_owned())
            ]
        );
    }

    use super::*;

    #[test]
    fn porcelain_v2_status() {
        let out = "# branch.oid abc\0# branch.head main\0\
            1 .M N... 100644 100644 100644 aaa bbb src/lib.rs\0\
            1 A. N... 000000 100644 100644 000 ccc new file.txt\0\
            1 MM N... 100644 100644 100644 aaa bbb both.rs\0\
            2 R. N... 100644 100644 100644 aaa bbb R100 renamed.rs\0old.rs\0\
            ? notes.md\0";
        let s = parse_status(out);
        assert_eq!(s.branch, "main");
        let names = |l: &[Change]| -> Vec<(char, String)> {
            l.iter().map(|c| (c.status, c.path.clone())).collect()
        };
        assert_eq!(
            names(&s.staged),
            [
                ('A', "new file.txt".to_owned()),
                ('M', "both.rs".to_owned()),
                ('R', "renamed.rs".to_owned())
            ]
        );
        assert_eq!(
            names(&s.unstaged),
            [('M', "src/lib.rs".to_owned()), ('M', "both.rs".to_owned())]
        );
        assert_eq!(s.untracked, ["notes.md"]);
    }

    #[test]
    fn lsof_ports_and_helpers() {
        let out = "p100\nf5\nn*:3000\nf6\nn[::1]:3000\np200\nf7\nn127.0.0.1:5173\n";
        assert_eq!(parse_lsof(out), [(3000, 100), (5173, 200)]);
        assert_eq!(parse_numstat("3\t1\ta\n-\t-\tbin\n10\t0\tb\n"), (13, 1));
        assert_eq!(web_url("git@github.com:a/b.git"), "https://github.com/a/b");
        assert_eq!(
            left_ellipsis("docs/user/workspace/file-pane.md", 20),
            "…/file-pane.md"
        );
        assert_eq!(short_duration(Duration::from_secs(125)), "2m");
    }
}
