//! Open Quickly (⇧⌘O): one fuzzy field over everything to jump to — open
//! windows and tabs, recently closed tabs, frequent folders, SSH hosts, the
//! focused pane's commands and links, and the files in its folder. Filters
//! run across the top (⌘0 All, ⌘W Opened, ⌘R Recent, ⌘Z Folders, ⌘S SSH,
//! ⌘J Current, ⌘P Files; Tab cycles); ⌘J opens straight to Current. ↩ runs
//! the highlighted item, ⌘1–⌘9 the first nine.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui::{
    AnyWindowHandle, ClipboardItem, Context, Entity, FocusHandle, Focusable, Hsla, KeyDownEvent,
    ModifiersChangedEvent, MouseButton, ScrollHandle, SharedString, Task, Window, div, prelude::*,
    px,
};
use motty_render::hsla;
use motty_term::links::{self, Matcher, Target as LinkTarget};
use motty_theme::{Mode, Rgba};

use super::picker::{self, Colors, ROW_GAP, ROW_HEIGHT, ROW_INSET, ROW_RADIUS, edit_query, score};
use super::{ClosedTabs, Workspace};
use crate::actions::*;
use crate::terminal_view::TerminalView;
use crate::titlebar::{icon, stroke};

const WIDTH: f32 = 583.;
const ICON_LEFT: f32 = 21.;
const TEXT_LEFT: f32 = 45.5;
const LIST_MAX_HEIGHT: f32 = 360.;
const CHIP_HEIGHT: f32 = 23.;
const FOOTER_HEIGHT: f32 = 31.;
/// Items a group shows in All (Files and Folders; the filters show more).
const ALL_LIMIT: usize = 5;
const FILTER_LIMIT: usize = 300;
const MAX_FILES: usize = 20_000;
const CHIP_ACCENT: u32 = 0x3f7045;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Filter {
    All,
    Opened,
    Recent,
    Folders,
    Ssh,
    Current,
    Files,
}

/// Each filter's chip label and its ⌘ key.
const FILTERS: [(Filter, &str, &str); 7] = [
    (Filter::All, "All", "0"),
    (Filter::Opened, "Opened", "w"),
    (Filter::Recent, "Recent", "r"),
    (Filter::Folders, "Folders", "z"),
    (Filter::Ssh, "SSH", "s"),
    (Filter::Current, "Current", "j"),
    (Filter::Files, "Files", "p"),
];

pub(super) struct OpenQuickly {
    focus: FocusHandle,
    query: String,
    filter: Filter,
    selected: usize,
    scroll: ScrollHandle,
    /// The pane that had focus: Current and Files are about it.
    target: Entity<TerminalView>,
    /// Files in the target's folder, newest first, once listed.
    files: Option<(PathBuf, Vec<PathBuf>)>,
    _list_files: Option<Task<()>>,
    /// ⌘ is held: rows show their ⌘1–⌘9.
    quick_select: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Window,
    Tab,
    Recent,
    Folder,
    Ssh,
    Command,
    Link,
    File,
    Path,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::Window => "Window",
            Kind::Tab => "Tab",
            Kind::Recent => "Recent",
            Kind::Folder => "Folder",
            Kind::Ssh => "SSH",
            Kind::Command => "Command",
            Kind::Link => "Link",
            Kind::File => "File",
            Kind::Path => "Path",
        }
    }

    /// The footer's name for ↩.
    fn verb(self) -> &'static str {
        match self {
            Kind::Window | Kind::Tab => "Switch to",
            Kind::Recent => "Reopen",
            Kind::Folder => "Change Directory",
            Kind::Ssh => "Connect",
            Kind::Command => "Jump to",
            Kind::Link | Kind::File | Kind::Path => "Open",
        }
    }
}

#[derive(Clone)]
enum Target {
    Window(AnyWindowHandle),
    Tab(AnyWindowHandle, Entity<TerminalView>),
    Closed(u64),
    Folder(PathBuf),
    Ssh(String),
    /// A stable row (see `motty_term::shell::Block`) in the target pane.
    Row(u64),
    Link(LinkTarget),
    File(PathBuf),
}

struct Item {
    group: &'static str,
    kind: Kind,
    title: String,
    detail: String,
    target: Target,
}

/// `~` for the home folder.
fn tilde(path: &Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rel) if rel.as_os_str().is_empty() => "~".into(),
        Some(rel) => format!("~/{}", rel.display()),
        None => path.display().to_string(),
    }
}

/// `~/x` and `/x` typed into the field, when that exists.
fn typed_path(query: &str) -> Option<PathBuf> {
    let path = match query.strip_prefix('~') {
        Some(rest) => dirs::home_dir()?.join(rest.trim_start_matches('/')),
        None if query.starts_with('/') => PathBuf::from(query),
        None => return None,
    };
    path.exists().then_some(path)
}

/// Hosts named in `~/.ssh/config`.
fn ssh_hosts() -> Vec<String> {
    dirs::home_dir()
        .and_then(|h| std::fs::read_to_string(h.join(".ssh/config")).ok())
        .map_or_else(Vec::new, |text| parse_ssh_hosts(&text))
}

/// `Host` names, patterns left out.
fn parse_ssh_hosts(text: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("host")) {
            continue;
        }
        for host in words {
            if !host.contains(['*', '?', '!']) && !hosts.iter().any(|h| h == host) {
                hosts.push(host.to_owned());
            }
        }
    }
    hosts
}

/// Files under `dir`, newest first, relative to it: what git tracks (and
/// untracked ones it doesn't ignore), else a walk that skips hidden files
/// and build output.
fn list_files(dir: &Path) -> Vec<PathBuf> {
    let git = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["ls-files", "-co", "--exclude-standard", "-z"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success());
    let mut files: Vec<PathBuf> = match git {
        Some(out) => out
            .stdout
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .take(MAX_FILES)
            .map(|p| PathBuf::from(String::from_utf8_lossy(p).into_owned()))
            .collect(),
        None => {
            const SKIP: [&str; 8] = [
                "node_modules",
                "target",
                "build",
                "dist",
                "Library",
                "vendor",
                "__pycache__",
                "Pods",
            ];
            let mut out = Vec::new();
            let mut stack = vec![(PathBuf::new(), 0)];
            while let Some((rel, depth)) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(dir.join(&rel)) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with('.') || SKIP.contains(&name.as_ref()) {
                        continue;
                    }
                    let path = rel.join(name.as_ref());
                    match entry.file_type() {
                        Ok(t) if t.is_dir() && depth < 6 => stack.push((path, depth + 1)),
                        Ok(t) if t.is_file() => out.push(path),
                        _ => {}
                    }
                }
                if out.len() >= MAX_FILES {
                    break;
                }
            }
            out
        }
    };
    let mut dated: Vec<(SystemTime, PathBuf)> = files
        .drain(..)
        .filter_map(|p| {
            let modified = std::fs::metadata(dir.join(&p)).ok()?.modified().ok()?;
            Some((modified, p))
        })
        .collect();
    dated.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    dated.into_iter().map(|(_, p)| p).collect()
}

/// Single quotes for a shell (bash, zsh and fish read them alike).
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn icon_for(kind: Kind, color: Hsla) -> gpui::AnyElement {
    let w = 1.1;
    match kind {
        Kind::Window => icon(14., 14., move |o, window| {
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(1.5, 3.));
                p.line_to(at(12.5, 3.));
                p.line_to(at(12.5, 11.5));
                p.line_to(at(1.5, 11.5));
                p.close();
            });
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(1.5, 5.5));
                p.line_to(at(12.5, 5.5));
            });
        })
        .into_any_element(),
        Kind::Tab | Kind::Recent => icon(14., 14., move |o, window| {
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(2., 5.));
                p.line_to(at(12., 5.));
                p.line_to(at(12., 12.));
                p.line_to(at(2., 12.));
                p.close();
            });
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(3.5, 3.));
                p.line_to(at(10.5, 3.));
            });
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(5., 1.25));
                p.line_to(at(9., 1.25));
            });
        })
        .into_any_element(),
        Kind::Folder => icon(14., 14., move |o, window| {
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(1.5, 3.));
                p.line_to(at(5.5, 3.));
                p.line_to(at(7., 4.5));
                p.line_to(at(12.5, 4.5));
                p.line_to(at(12.5, 11.5));
                p.line_to(at(1.5, 11.5));
                p.close();
            });
        })
        .into_any_element(),
        Kind::File | Kind::Path => icon(14., 14., move |o, window| {
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(3., 1.5));
                p.line_to(at(8.5, 1.5));
                p.line_to(at(11.5, 4.5));
                p.line_to(at(11.5, 12.5));
                p.line_to(at(3., 12.5));
                p.close();
            });
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(8.5, 1.5));
                p.line_to(at(8.5, 4.5));
                p.line_to(at(11.5, 4.5));
            });
        })
        .into_any_element(),
        Kind::Ssh | Kind::Command => icon(14., 14., move |o, window| {
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(2.5, 4.));
                p.line_to(at(6., 7.));
                p.line_to(at(2.5, 10.));
            });
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(7.5, 11.));
                p.line_to(at(12., 11.));
            });
        })
        .into_any_element(),
        Kind::Link => icon(14., 14., move |o, window| {
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(6., 8.));
                p.line_to(at(8., 6.));
            });
            for (x, y) in [(3.5, 10.5), (10.5, 3.5)] {
                stroke(o, color, window, w, move |p, at| {
                    p.move_to(at(x - 2.5, y));
                    p.line_to(at(x, y - 2.5));
                    p.line_to(at(x + 2.5, y));
                    p.line_to(at(x, y + 2.5));
                    p.close();
                });
            }
        })
        .into_any_element(),
    }
}

impl Workspace {
    /// ⇧⌘O opens Open Quickly on All; ⌘J opens it on Current.
    pub(super) fn show_open_quickly(
        &mut self,
        filter: Filter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(oq) = self.open_quickly.as_mut() {
            if oq.filter == filter {
                return self.close_open_quickly(window, cx);
            }
            oq.filter = filter;
            oq.selected = 0;
            return cx.notify();
        }
        let Some(target) = self.focused_pane() else {
            return;
        };
        self.palette = None;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Files are listed in the background; the list fills in when done.
        let list = target.read(cx).cwd.clone().map(|dir| {
            cx.spawn(async move |this, cx| {
                let files = cx
                    .background_spawn({
                        let dir = dir.clone();
                        async move { list_files(&dir) }
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(oq) = this.open_quickly.as_mut() {
                        oq.files = Some((dir, files));
                        cx.notify();
                    }
                });
            })
        });
        self.open_quickly = Some(OpenQuickly {
            focus,
            query: String::new(),
            filter,
            selected: 0,
            scroll: ScrollHandle::new(),
            target,
            files: None,
            _list_files: list,
            quick_select: false,
        });
        cx.notify();
    }

    fn close_open_quickly(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(oq) = self.open_quickly.take() {
            window.focus(&oq.target.focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// Everything that matches, in display order.
    fn open_quickly_items(&self, window: &Window, cx: &Context<Self>) -> Vec<Item> {
        let Some(oq) = &self.open_quickly else {
            return Vec::new();
        };
        let (filter, query) = (oq.filter, oq.query.trim());
        let wants = |f: Filter| filter == Filter::All || filter == f;
        let mut groups: Vec<Vec<Item>> = Vec::new();

        if let Some(path) = typed_path(query) {
            let kind = if path.is_dir() {
                Kind::Folder
            } else {
                Kind::Path
            };
            if wants(if path.is_dir() {
                Filter::Folders
            } else {
                Filter::Files
            }) {
                groups.push(vec![Item {
                    group: "Path",
                    kind,
                    title: tilde(&path),
                    detail: String::new(),
                    target: if path.is_dir() {
                        Target::Folder(path)
                    } else {
                        Target::File(path)
                    },
                }]);
            }
        }

        if wants(Filter::Opened) {
            let this = window.window_handle();
            let mut windows = Vec::new();
            let mut tabs = Vec::new();
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
                windows.push(Item {
                    group: "Windows",
                    kind: Kind::Window,
                    title: ws.window_title.clone(),
                    detail: format!(
                        "{} tab{}",
                        ws.tabs.len(),
                        if ws.tabs.len() == 1 { "" } else { "s" }
                    ),
                    target: Target::Window(handle),
                });
                for tab in ws.tabs.iter().filter_map(|t| t.terms()) {
                    let view = tab.focused.read(cx);
                    tabs.push(Item {
                        group: "Tabs",
                        kind: Kind::Tab,
                        title: view.display_title(),
                        detail: view.cwd.as_deref().map(tilde).unwrap_or_default(),
                        target: Target::Tab(handle, tab.focused.clone()),
                    });
                }
            }
            if windows.len() > 1 {
                groups.push(windows);
            }
            groups.push(tabs);
        }

        if wants(Filter::Recent)
            && let Some(closed) = cx.try_global::<ClosedTabs>()
        {
            groups.push(
                closed
                    .0
                    .iter()
                    .rev()
                    .map(|c| Item {
                        group: "Recently Closed",
                        kind: Kind::Recent,
                        title: c.title.clone(),
                        detail: c.cwd.as_deref().map(tilde).unwrap_or_default(),
                        target: Target::Closed(c.id),
                    })
                    .collect(),
            );
        }

        if wants(Filter::Folders) {
            groups.push(
                crate::folders::ranked()
                    .into_iter()
                    .map(|path| Item {
                        group: "Folders",
                        kind: Kind::Folder,
                        title: path
                            .file_name()
                            .map_or_else(|| tilde(&path), |n| n.to_string_lossy().into_owned()),
                        detail: tilde(&path),
                        target: Target::Folder(path),
                    })
                    .collect(),
            );
        }

        if wants(Filter::Ssh) {
            groups.push(
                ssh_hosts()
                    .into_iter()
                    .map(|host| Item {
                        group: "SSH",
                        kind: Kind::Ssh,
                        detail: format!("ssh {host}"),
                        title: host.clone(),
                        target: Target::Ssh(host),
                    })
                    .collect(),
            );
        }

        if wants(Filter::Current) {
            let view = oq.target.read(cx);
            let session = view.session();
            let blocks: Vec<_> = session
                .shell()
                .blocks
                .iter()
                .rev()
                .take(200)
                .cloned()
                .collect();
            groups.push(
                blocks
                    .iter()
                    .filter_map(|b| {
                        Some(Item {
                            group: "Commands",
                            kind: Kind::Command,
                            title: session.block_command(b)?,
                            detail: b
                                .exit
                                .filter(|&e| e != 0)
                                .map_or_else(String::new, |e| format!("exit {e}")),
                            target: Target::Row(b.prompt_row),
                        })
                    })
                    .collect(),
            );
            let term = session.term.lock();
            let mut seen = std::collections::HashSet::new();
            groups.push(
                links::hint_targets(&*term, Matcher::Link, view.cwd.as_deref())
                    .into_iter()
                    .filter_map(|t| {
                        let link = t.link?;
                        let title = link.display();
                        seen.insert(title.clone()).then(|| Item {
                            group: "Links",
                            kind: Kind::Link,
                            title,
                            detail: String::new(),
                            target: Target::Link(link),
                        })
                    })
                    .collect(),
            );
        }

        if wants(Filter::Files)
            && let Some((dir, files)) = &oq.files
        {
            groups.push(
                files
                    .iter()
                    .map(|rel| Item {
                        group: "Files",
                        kind: Kind::File,
                        title: rel
                            .file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                        detail: rel
                            .parent()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                        target: Target::File(dir.join(rel)),
                    })
                    .collect(),
            );
        }

        // Filter and rank within each group; All shows the long ones short.
        let mut out = Vec::new();
        for group in groups {
            let mut scored: Vec<(i32, Item)> = if query.is_empty() {
                group.into_iter().map(|i| (0, i)).collect()
            } else {
                group
                    .into_iter()
                    .filter_map(|i| {
                        let title = score(query, &i.title);
                        let detail =
                            score(query, &format!("{} {}", i.detail, i.title)).map(|s| s - 3);
                        Some((title.max(detail)?, i))
                    })
                    .collect()
            };
            if !query.is_empty() {
                scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
            }
            let limit = match scored.first().map(|(_, i)| i.kind) {
                Some(Kind::File | Kind::Folder) if filter == Filter::All => ALL_LIMIT,
                _ => FILTER_LIMIT,
            };
            out.extend(scored.into_iter().take(limit).map(|(_, i)| i));
        }
        out
    }

    fn move_open_quickly_selection(
        &mut self,
        delta: isize,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let items = self.open_quickly_items(window, cx);
        let Some(oq) = self.open_quickly.as_mut() else {
            return;
        };
        if items.is_empty() {
            return;
        }
        let n = items.len() as isize;
        oq.selected = (oq.selected as isize + delta).clamp(0, n - 1) as usize;
        // Headers are children of the list too.
        let mut group = "";
        let mut headers = 0;
        for item in &items[..=oq.selected] {
            if item.group != group {
                group = item.group;
                headers += 1;
            }
        }
        oq.scroll.scroll_to_item(oq.selected + headers);
        cx.notify();
    }

    fn run_open_quickly(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.open_quickly_items(window, cx).into_iter().nth(ix) else {
            return;
        };
        let Some(target) = self.open_quickly.as_ref().map(|oq| oq.target.clone()) else {
            return;
        };
        self.close_open_quickly(window, cx);
        let this = window.window_handle();
        match item.target {
            Target::Window(handle) => {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
            Target::Tab(handle, view) if handle == this => self.reveal_pane(&view, window, cx),
            Target::Tab(handle, view) => {
                if let Some(ws) = handle.downcast::<Workspace>() {
                    let _ = ws.update(cx, |ws, window, cx| {
                        ws.reveal_pane(&view, window, cx);
                        window.activate_window();
                    });
                }
            }
            Target::Closed(id) => {
                if let Err(e) = self.reopen_tab(Some(id), window, cx) {
                    tracing::warn!("{e:#}");
                }
            }
            Target::Folder(path) => self.run_in_pane(
                &target,
                &format!("cd -- {}", shell_quote(&path.to_string_lossy())),
                Some(path),
                window,
                cx,
            ),
            Target::Ssh(host) => self.run_in_pane(
                &target,
                &format!("ssh {}", shell_quote(&host)),
                None,
                window,
                cx,
            ),
            Target::Row(row) => target.update(cx, |view, cx| view.reveal_row(row, cx)),
            Target::Link(LinkTarget::Url(url)) => cx.open_url(&url),
            Target::Link(LinkTarget::Path { path, .. }) | Target::File(path) => {
                cx.open_with_system(&path)
            }
        }
    }

    /// Types `command` into `view`'s shell when it's at a prompt; else opens
    /// a tab for it (in `cwd`).
    fn run_in_pane(
        &mut self,
        view: &Entity<TerminalView>,
        command: &str,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !view.read(cx).session().is_busy() {
            // A leading space keeps it out of shell history where that's set up.
            let line = format!(" {command}\r");
            view.update(cx, |v, cx| v.write_input(&line, cx));
            return;
        }
        let tab = match cwd {
            Some(dir) => self.open_tab_with(None, Some(dir), window, cx),
            None => {
                let opened = self.open_tab_with(None, None, window, cx);
                if let Ok(new) = &opened {
                    let line = format!("{command}\r");
                    new.update(cx, |v, cx| v.write_input(&line, cx));
                }
                opened
            }
        };
        if let Err(e) = tab {
            tracing::error!("failed to start shell: {e:#}");
        }
    }

    fn on_open_quickly_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        let key = ks.key.as_str();
        if m.platform && !m.shift && !m.alt && !m.control {
            if let Some((filter, ..)) = FILTERS.iter().find(|(_, _, k)| *k == key) {
                if let Some(oq) = self.open_quickly.as_mut() {
                    oq.filter = *filter;
                    oq.selected = 0;
                    oq.scroll.scroll_to_item(0);
                }
                cx.notify();
                return cx.stop_propagation();
            }
            if let Some(n) = key.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                self.run_open_quickly(n - 1, window, cx);
                return cx.stop_propagation();
            }
            if key == "enter" {
                let ix = self.open_quickly.as_ref().map_or(0, |oq| oq.selected);
                self.run_open_quickly(ix, window, cx);
                return cx.stop_propagation();
            }
            // Other ⌘ shortcuts (paste, the palettes) go on.
            return;
        }
        match key {
            "escape" => self.close_open_quickly(window, cx),
            "enter" => {
                let ix = self.open_quickly.as_ref().map_or(0, |oq| oq.selected);
                self.run_open_quickly(ix, window, cx);
            }
            "up" => self.move_open_quickly_selection(-1, window, cx),
            "down" => self.move_open_quickly_selection(1, window, cx),
            "p" | "k" if m.control => self.move_open_quickly_selection(-1, window, cx),
            "n" | "j" if m.control => self.move_open_quickly_selection(1, window, cx),
            "pageup" => self.move_open_quickly_selection(-8, window, cx),
            "pagedown" => self.move_open_quickly_selection(8, window, cx),
            "home" => self.move_open_quickly_selection(isize::MIN / 2, window, cx),
            "end" => self.move_open_quickly_selection(isize::MAX / 2, window, cx),
            "tab" => {
                if let Some(oq) = self.open_quickly.as_mut() {
                    let i = FILTERS
                        .iter()
                        .position(|(f, ..)| *f == oq.filter)
                        .unwrap_or(0) as isize;
                    let next =
                        (i + if m.shift { -1 } else { 1 }).rem_euclid(FILTERS.len() as isize);
                    oq.filter = FILTERS[next as usize].0;
                    oq.selected = 0;
                    oq.scroll.scroll_to_item(0);
                    cx.notify();
                }
            }
            _ => {
                if let Some(oq) = self.open_quickly.as_mut()
                    && edit_query(&mut oq.query, ks)
                {
                    oq.selected = 0;
                    oq.scroll.scroll_to_item(0);
                    cx.notify();
                }
            }
        }
        cx.stop_propagation();
    }

    pub(super) fn render_open_quickly(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let oq = self.open_quickly.as_ref()?;
        let t = &self.theme;
        let dark = t.meta.mode == Mode::Dark;
        let c = Colors::new(t);
        let items = self.open_quickly_items(window, cx);
        let selected = oq.selected.min(items.len().saturating_sub(1));
        let accent = hsla(if dark {
            Rgba::rgb(0x8fd19e)
        } else {
            Rgba::rgb(CHIP_ACCENT)
        });

        let chips = div()
            .flex_none()
            .px(px(14.))
            .pb(px(10.))
            .flex()
            .flex_wrap()
            .gap(px(6.5))
            .children(FILTERS.iter().map(|&(filter, label, _)| {
                let on = filter == oq.filter;
                div()
                    .id(label)
                    .h(px(CHIP_HEIGHT))
                    .px(px(11.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border(px(1.))
                    .border_color(if on {
                        accent.opacity(0.35)
                    } else {
                        c.keycap_border
                    })
                    .when(on, |d| d.bg(accent.opacity(0.14)))
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(if on { accent } else { c.placeholder })
                    .child(label)
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(oq) = this.open_quickly.as_mut() {
                            oq.filter = filter;
                            oq.selected = 0;
                            oq.scroll.scroll_to_item(0);
                            cx.notify();
                        }
                    }))
            }));

        let mut children: Vec<gpui::AnyElement> = Vec::new();
        let mut group = "";
        for (ix, item) in items.iter().enumerate() {
            if item.group != group {
                let first = group.is_empty();
                group = item.group;
                children.push(picker::header(group, TEXT_LEFT, first, &c).into_any_element());
            }
            let badge = if oq.quick_select && ix < 9 {
                format!("⌘{}", ix + 1)
            } else {
                item.kind.tag().to_owned()
            };
            children.push(
                div()
                    .id(ix)
                    .flex_none()
                    .mx(px(ROW_INSET))
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(ROW_RADIUS))
                    .when(ix == selected, |d| d.bg(c.selected))
                    .child(
                        div()
                            .flex_none()
                            .w(px(TEXT_LEFT - ROW_INSET - 8.))
                            .pl(px(ICON_LEFT - ROW_INSET))
                            .child(icon_for(item.kind, c.placeholder)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c.text)
                            .child(SharedString::from(item.title.clone())),
                    )
                    .when(!item.detail.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_shrink(1.)
                                .min_w_0()
                                .max_w(px(220.))
                                .truncate()
                                .text_color(c.placeholder)
                                .child(SharedString::from(item.detail.clone())),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .mr(px(12.5))
                            .h(px(17.))
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .border(px(0.5))
                            .border_color(c.keycap_border)
                            .bg(c.keycap)
                            .text_size(px(11.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(c.keycap_text)
                            .child(SharedString::from(badge)),
                    )
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if let Some(oq) = this.open_quickly.as_mut()
                            && oq.selected != ix
                        {
                            oq.selected = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.run_open_quickly(ix, window, cx)
                        }),
                    )
                    .into_any_element(),
            );
        }
        if items.is_empty() {
            let loading = oq.filter == Filter::Files && oq.files.is_none();
            children.push(
                div()
                    .h(px(ROW_HEIGHT))
                    .pl(px(TEXT_LEFT))
                    .flex()
                    .items_center()
                    .text_color(c.placeholder)
                    .child(if loading {
                        "Listing files…"
                    } else {
                        "No results"
                    })
                    .into_any_element(),
            );
        }

        let verb = items.get(selected).map_or("Open", |i| i.kind.verb());
        let footer = div()
            .flex_none()
            .h(px(FOOTER_HEIGHT))
            .border_t(px(1.))
            .border_color(c.separator)
            .px(px(14.))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(c.placeholder)
            .child("Quick Select ⌘")
            .child(format!("{verb} ↩"));

        let card = picker::card(WIDTH, &c)
            .id("open-quickly")
            .key_context("OpenQuickly")
            .track_focus(&oq.focus)
            .on_key_down(cx.listener(Self::on_open_quickly_key))
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, cx| {
                if let Some(oq) = this.open_quickly.as_mut()
                    && oq.quick_select != e.modifiers.platform
                {
                    oq.quick_select = e.modifiers.platform;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .map(|t| t.replace(['\n', '\r'], " "));
                if let (Some(text), Some(oq)) = (text, this.open_quickly.as_mut()) {
                    oq.query.push_str(&text);
                    oq.selected = 0;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                if let Some(oq) = &this.open_quickly {
                    cx.write_to_clipboard(ClipboardItem::new_string(oq.query.clone()));
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(picker::search_row(
                &oq.query,
                "Search tabs, folders, files...",
                &c,
            ))
            .child(chips)
            .child(
                div()
                    .id("open-quickly-list")
                    .max_h(px(LIST_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&oq.scroll)
                    .pb(px(ROW_INSET))
                    .flex()
                    .flex_col()
                    .gap(px(ROW_GAP))
                    .children(children),
            )
            .child(footer);

        Some(
            div()
                .id("open-quickly-scrim")
                .absolute()
                .inset_0()
                .px(px(16.))
                .pt(px(picker::top(window)))
                .flex()
                .flex_col()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_open_quickly(window, cx)),
                )
                .child(card),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote("/a b/it's"), r"'/a b/it'\''s'");
    }

    #[test]
    fn reads_only_literal_ssh_hosts() {
        let config = "Host *\n  User me\nHost box db.example\nhost box !bad gw?\n";
        assert_eq!(parse_ssh_hosts(config), ["box", "db.example"]);
    }

    #[test]
    fn lists_files_skipping_build_output() {
        let dir = std::env::temp_dir().join(format!("motty-oq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("node_modules/skip.js"), "x").unwrap();
        let files = list_files(&dir);
        assert_eq!(files, vec![PathBuf::from("a.txt")]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
