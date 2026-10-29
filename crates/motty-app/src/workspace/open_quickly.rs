//! Open Quickly (⇧⌘O): one fuzzy field over everything to jump to — open
//! windows and tabs, recently closed tabs, frequent folders, SSH hosts, the
//! focused pane's commands and links, the files in its folder, and past
//! coding-agent sessions. Filters run across the top (⌘0 All, ⌘W Opened,
//! ⌘R Recent, ⌘G Agents, ⌘Z Folders, ⌘S SSH, ⌘J Current, ⌘P Files; Tab
//! cycles); ⌘J opens straight to Current. ↩ runs the highlighted item,
//! ⌘1–⌘9 the first nine; ⌘K offers a session's other actions.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui::{
    AnyWindowHandle, ClipboardItem, Context, Entity, FocusHandle, Hsla, KeyDownEvent,
    ModifiersChangedEvent, MouseButton, Pixels, Point, ScrollHandle, SharedString, Task, Window,
    div, point, prelude::*, px,
};
use motty_render::hsla;
use motty_term::links::{self, Matcher, Target as LinkTarget};
use motty_theme::{Mode, Rgba};

use super::agents::Place;
use super::picker::{self, Colors, ROW_GAP, ROW_HEIGHT, ROW_INSET, ROW_RADIUS, edit_query, score};
use super::{ClosedTabs, Workspace};
use crate::actions::*;
use crate::agents::{self, sessions::Session};
use crate::panes::Direction;
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
    Agents,
    Folders,
    Ssh,
    Current,
    Files,
}

/// A session that mentions the searched words, and the line that does.
type Mention = (Session, String);

/// Each filter's chip label and its ⌘ key.
const FILTERS: [(Filter, &str, &str); 8] = [
    (Filter::All, "All", "0"),
    (Filter::Opened, "Opened", "w"),
    (Filter::Recent, "Recent", "r"),
    (Filter::Agents, "Agents", "g"),
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
    /// The pane that had focus: Current is about it. None over an editor.
    target: Option<Entity<TerminalView>>,
    /// Where focus goes back to.
    back: FocusHandle,
    /// Files in the target's folder, newest first, once listed.
    files: Option<(PathBuf, Vec<PathBuf>)>,
    _list_files: Option<Task<()>>,
    /// Past agent sessions, newest first, once read.
    sessions: Option<Vec<Session>>,
    _list_sessions: Task<()>,
    /// ⌘ is held: rows show their ⌘1–⌘9.
    quick_select: bool,
    /// Search "…" in agent conversations: the words, and the sessions that
    /// mention them all with the line that matched (None while looking).
    conversations: Option<(String, Option<Vec<Mention>>)>,
    _search_conversations: Option<Task<()>>,
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
    Session,
    /// The Search "…" in agent conversations row.
    SearchAgents,
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
            Kind::Session => "Agent",
            Kind::SearchAgents => "Search",
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
            Kind::Session => "Resume",
            Kind::SearchAgents => "Search",
        }
    }
}

/// What an entry in a row's ⌘K menu does.
enum Act {
    /// The row's ↩ action.
    Run,
    Copy(String),
    Reveal(PathBuf),
    Forget(PathBuf),
    Start(PathBuf, Place),
    Ssh(String, Place),
    /// The command, and whether in a new tab.
    Rerun(String, bool),
    Cd(PathBuf),
    Edit(PathBuf),
    CloseTab(AnyWindowHandle, Entity<TerminalView>),
    TabToWindow(AnyWindowHandle, Entity<TerminalView>),
}

#[derive(Clone)]
enum Target {
    Window(AnyWindowHandle),
    Tab(AnyWindowHandle, Entity<TerminalView>),
    /// A file editor tab.
    Editor(AnyWindowHandle, Entity<crate::editor::FileEditor>),
    Closed(u64),
    Folder(PathBuf),
    Ssh(String),
    /// A stable row (see `motty_term::shell::Block`) in the target pane.
    Row(u64),
    Link(LinkTarget),
    File(PathBuf),
    Session(Box<Session>),
    /// Look for the words in what was said in agent sessions.
    SearchAgents(String),
}

struct Item {
    group: &'static str,
    kind: Kind,
    title: String,
    detail: String,
    target: Target,
}

/// The list child showing item `ix`: group headers are children too.
fn row_child(items: &[Item], ix: usize) -> usize {
    let mut group = "";
    let mut headers = 0;
    for item in &items[..=ix.min(items.len().saturating_sub(1))] {
        if item.group != group {
            group = item.group;
            headers += 1;
        }
    }
    ix + headers
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

/// How long ago, in Otty's short form.
fn ago(time: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(time)
        .map_or(0, |d| d.as_secs());
    match secs {
        0..60 => "now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

fn icon_for(kind: Kind, color: Hsla) -> gpui::AnyElement {
    let w = 1.1;
    match kind {
        // A speech bubble.
        Kind::Session | Kind::SearchAgents => icon(14., 14., move |o, window| {
            stroke(o, color, window, w, |p, at| {
                p.move_to(at(2., 2.5));
                p.line_to(at(12., 2.5));
                p.line_to(at(12., 9.5));
                p.line_to(at(6.5, 9.5));
                p.line_to(at(3.5, 12.));
                p.line_to(at(3.5, 9.5));
                p.line_to(at(2., 9.5));
                p.close();
            });
        })
        .into_any_element(),
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
        let Some(back) = self.active_focus(cx) else {
            return;
        };
        let target = self.focused_pane();
        self.palette = None;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Files are listed in the background; the list fills in when done.
        let list = self.active_folder(cx).map(|dir| {
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
        let sessions = cx.spawn(async move |this, cx| {
            let sessions = cx
                .background_spawn(async move {
                    crate::agents::sessions::list(&crate::agents::sessions::Roots::current(), 500)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(oq) = this.open_quickly.as_mut() {
                    oq.sessions = Some(sessions);
                    cx.notify();
                }
            });
        });
        self.open_quickly = Some(OpenQuickly {
            focus,
            query: String::new(),
            filter,
            selected: 0,
            scroll: ScrollHandle::new(),
            target,
            back,
            files: None,
            _list_files: list,
            sessions: None,
            _list_sessions: sessions,
            quick_select: false,
            conversations: None,
            _search_conversations: None,
        });
        cx.notify();
    }

    fn close_open_quickly(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(oq) = self.open_quickly.take() {
            window.focus(&oq.back, cx);
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
                for tab in &ws.tabs {
                    if let Some(editor) = tab.as_editor() {
                        let path = editor.read(cx).path().map(Path::to_path_buf);
                        tabs.push(Item {
                            group: "Tabs",
                            kind: Kind::File,
                            title: path.as_deref().and_then(Path::file_name).map_or_else(
                                || "Untitled".into(),
                                |n| n.to_string_lossy().into_owned(),
                            ),
                            detail: path.as_deref().map(tilde).unwrap_or_default(),
                            target: Target::Editor(handle, editor.clone()),
                        });
                        continue;
                    }
                    let Some(tab) = tab.terms() else { continue };
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

        // The conversations search's results take the Agents list's place.
        if wants(Filter::Agents)
            && let Some((words, found)) = &oq.conversations
        {
            groups.push(
                found
                    .iter()
                    .flatten()
                    .map(|(s, line)| Item {
                        group: "Conversations",
                        kind: Kind::Session,
                        title: s.title.clone(),
                        detail: format!("{} · {line}", agents::label_for(s.agent)),
                        target: Target::Session(Box::new(s.clone())),
                    })
                    .collect(),
            );
            if found.is_none() {
                groups.push(vec![Item {
                    group: "Conversations",
                    kind: Kind::SearchAgents,
                    title: format!("Searching for “{words}”…"),
                    detail: String::new(),
                    target: Target::SearchAgents(words.clone()),
                }]);
            }
        } else if wants(Filter::Agents)
            && let Some(sessions) = &oq.sessions
        {
            groups.push(
                sessions
                    .iter()
                    .map(|s| {
                        let mut detail = agents::label_for(s.agent);
                        if let Some(cwd) = &s.cwd {
                            detail.push_str(" · ");
                            detail.push_str(&tilde(cwd));
                        }
                        detail.push_str(" · ");
                        detail.push_str(&ago(s.modified));
                        Item {
                            group: "Agents",
                            kind: Kind::Session,
                            title: s.title.clone(),
                            detail,
                            target: Target::Session(Box::new(s.clone())),
                        }
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

        if wants(Filter::Current)
            && let Some(view) = &oq.target
        {
            let view = view.read(cx);
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
                Some(Kind::File | Kind::Folder | Kind::Session) if filter == Filter::All => {
                    ALL_LIMIT
                }
                _ => FILTER_LIMIT,
            };
            out.extend(scored.into_iter().take(limit).map(|(_, i)| i));
        }
        // Below the sessions (in All, so above Files): search what was
        // said in them.
        if wants(Filter::Agents) && !query.is_empty() && oq.conversations.is_none() {
            let row = Item {
                group: "Agents",
                kind: Kind::SearchAgents,
                title: format!("Search “{query}” in agent conversations"),
                detail: String::new(),
                target: Target::SearchAgents(query.to_owned()),
            };
            let at = out
                .iter()
                .rposition(|i| i.group == "Agents")
                .map(|i| i + 1)
                .or_else(|| out.iter().position(|i| i.group == "Files"))
                .unwrap_or(out.len());
            out.insert(at, row);
        }
        out
    }

    /// Searches the agent sessions' conversations for every word of
    /// `query`, and shows the sessions that have them all.
    fn search_conversations(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(oq) = self.open_quickly.as_mut() else {
            return;
        };
        let sessions = oq.sessions.clone().unwrap_or_default();
        oq.filter = Filter::Agents;
        oq.selected = 0;
        oq.conversations = Some((query.clone(), None));
        oq._search_conversations = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { conversation_matches(&sessions, &query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(oq) = this.open_quickly.as_mut()
                    && let Some((_, slot)) = &mut oq.conversations
                {
                    *slot = Some(found);
                    cx.notify();
                }
            });
        }));
        cx.notify();
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
        oq.scroll.scroll_to_item(row_child(&items, oq.selected));
        cx.notify();
    }

    /// Where ⌘K's menu opens: at the end of the highlighted row, else at
    /// the pointer.
    fn actions_anchor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Point<Pixels> {
        let items = self.open_quickly_items(window, cx);
        self.open_quickly
            .as_ref()
            .and_then(|oq| oq.scroll.bounds_for_item(row_child(&items, oq.selected)))
            .map(|b| point(b.right() - px(160.), b.top()))
            .unwrap_or_else(|| window.mouse_position())
    }

    fn run_open_quickly(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.open_quickly_items(window, cx).into_iter().nth(ix) else {
            return;
        };
        let Some(target) = self.open_quickly.as_ref().map(|oq| oq.target.clone()) else {
            return;
        };
        if let Target::SearchAgents(query) = item.target {
            return self.search_conversations(query, cx);
        }
        self.close_open_quickly(window, cx);
        let this = window.window_handle();
        match item.target {
            Target::Window(handle) => {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
            Target::Tab(handle, view) if handle == this => self.reveal_pane(&view, window, cx),
            Target::Editor(handle, editor) if handle == this => {
                self.reveal_editor(&editor, window, cx)
            }
            Target::Editor(handle, editor) => {
                if let Some(ws) = handle.downcast::<Workspace>() {
                    let _ = ws.update(cx, |ws, window, cx| {
                        ws.reveal_editor(&editor, window, cx);
                        window.activate_window();
                    });
                }
            }
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
            // Over an editor, folders and hosts open in a new tab.
            Target::Folder(path) => match &target {
                Some(target) => self.run_in_pane(
                    target,
                    &format!("cd -- {}", shell_quote(&path.to_string_lossy())),
                    Some(path),
                    window,
                    cx,
                ),
                None => {
                    if let Err(e) = self.add_tab(None, Some(path), window, cx) {
                        tracing::error!("failed to open a tab: {e:#}");
                    }
                }
            },
            Target::Ssh(host) => match &target {
                Some(target) => self.run_in_pane(
                    target,
                    &format!("ssh {}", shell_quote(&host)),
                    None,
                    window,
                    cx,
                ),
                None => {
                    if let Err(e) = self.connect_to_host(&host, window, cx) {
                        tracing::error!("failed to open a tab: {e:#}");
                    }
                }
            },
            Target::Row(row) => {
                if let Some(target) = target {
                    target.update(cx, |view, cx| view.reveal_row(row, cx))
                }
            }
            // Handled before the panel closes.
            Target::SearchAgents(_) => {}
            Target::Session(s) => self.resume_session(
                &ResumeSession {
                    agent: s.agent.to_owned(),
                    id: s.id,
                    cwd: s.cwd,
                    fork: false,
                    place: "tab".into(),
                },
                window,
                cx,
            ),
            Target::Link(LinkTarget::Url(url)) => cx.open_url(&url),
            Target::Link(LinkTarget::Path { path, .. }) | Target::File(path) => {
                cx.open_with_system(&path)
            }
        }
    }

    /// ⌘K: the Actions menu for the highlighted row, by kind (Otty's table:
    /// tab, folder, SSH host, agent session, command, file or link).
    fn item_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::native_menu::Item as MenuItem;
        let selected = self.open_quickly.as_ref().map_or(0, |oq| oq.selected);
        let Some(item) = self
            .open_quickly_items(window, cx)
            .into_iter()
            .nth(selected)
        else {
            return;
        };
        let actions: Vec<(&'static str, Act)> = match item.target {
            Target::Session(session) => return self.session_actions(*session, window, cx),
            Target::SearchAgents(_) => vec![("Search", Act::Run)],
            Target::Editor(_, editor) => {
                let mut v = vec![("Switch to Tab", Act::Run)];
                if let Some(path) = editor.read(cx).path().map(Path::to_path_buf) {
                    v.push(("Reveal in Finder", Act::Reveal(path.clone())));
                    v.push(("Copy Path", Act::Copy(path.display().to_string())));
                }
                v
            }
            Target::Tab(handle, view) => {
                let cwd = view.read(cx).cwd.clone();
                let mut v = vec![
                    ("Switch to Tab", Act::Run),
                    ("Close Tab", Act::CloseTab(handle, view.clone())),
                    ("Move Tab to New Window", Act::TabToWindow(handle, view)),
                ];
                if let Some(cwd) = cwd {
                    v.push(("Reveal CWD in Finder", Act::Reveal(cwd.clone())));
                    v.push(("Copy CWD Path", Act::Copy(cwd.display().to_string())));
                }
                v
            }
            Target::Folder(path) => vec![
                ("Open in New Tab", Act::Start(path.clone(), Place::Tab)),
                (
                    "Open in New Window",
                    Act::Start(path.clone(), Place::Window),
                ),
                (
                    "Split Right",
                    Act::Start(path.clone(), Place::Split(Direction::Right)),
                ),
                (
                    "Split Down",
                    Act::Start(path.clone(), Place::Split(Direction::Down)),
                ),
                ("Change Directory Here", Act::Run),
                ("Reveal in Finder", Act::Reveal(path.clone())),
                ("Copy Path", Act::Copy(path.display().to_string())),
                ("Forget This Folder", Act::Forget(path)),
            ],
            Target::Ssh(host) => vec![
                ("Connect", Act::Run),
                ("Connect in New Tab", Act::Ssh(host.clone(), Place::Tab)),
                (
                    "Connect in New Window",
                    Act::Ssh(host.clone(), Place::Window),
                ),
                (
                    "Copy Command",
                    Act::Copy(format!("ssh {}", shell_quote(&host))),
                ),
            ],
            Target::Row(_) => vec![
                ("Jump to", Act::Run),
                (
                    "Re-Run in Current Pane",
                    Act::Rerun(item.title.clone(), false),
                ),
                ("Re-Run in New Tab", Act::Rerun(item.title.clone(), true)),
                ("Copy Command", Act::Copy(item.title)),
            ],
            Target::Link(LinkTarget::Url(url)) => {
                vec![("Open in Browser", Act::Run), ("Copy URL", Act::Copy(url))]
            }
            Target::Link(LinkTarget::Path { path, .. }) | Target::File(path) => {
                let dir = if path.is_dir() {
                    Some(path.clone())
                } else {
                    path.parent().map(Path::to_path_buf)
                };
                let mut v = vec![("Open in Default App", Act::Run)];
                if path.is_file() {
                    v.push(("Open in Motty", Act::Edit(path.clone())));
                }
                if let Some(dir) = dir {
                    v.push((
                        "Open Folder in New Tab",
                        Act::Start(dir.clone(), Place::Tab),
                    ));
                    v.push(("Change Directory Here", Act::Cd(dir)));
                }
                v.push(("Reveal in Finder", Act::Reveal(path.clone())));
                v.push(("Copy Path", Act::Copy(path.display().to_string())));
                v
            }
            Target::Closed(_) => vec![("Reopen Tab", Act::Run)],
            Target::Window(_) => vec![("Switch to Window", Act::Run)],
        };
        let entries = actions
            .iter()
            .map(|(label, _)| MenuItem::new(*label).into())
            .collect();
        let at = self.actions_anchor(window, cx);
        let this = cx.entity().downgrade();
        crate::native_menu::popup(entries, at, Some(0), window, cx, move |ix, window, cx| {
            let Some((_, act)) = actions.into_iter().nth(ix) else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.run_item_action(selected, act, window, cx)
            });
        });
    }

    fn run_item_action(
        &mut self,
        selected: usize,
        act: Act,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.open_quickly.as_ref().and_then(|oq| oq.target.clone());
        match act {
            Act::Run => return self.run_open_quickly(selected, window, cx),
            Act::Copy(text) => return cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Act::Reveal(path) => return cx.reveal_path(&path),
            Act::Forget(path) => {
                crate::folders::forget(&path);
                return cx.notify();
            }
            _ => {}
        }
        self.close_open_quickly(window, cx);
        match act {
            Act::Start(dir, place) => {
                self.start_in("", Some(dir), place, window, cx);
            }
            Act::Ssh(host, Place::Tab) => {
                if let Err(e) = self.connect_to_host(&host, window, cx) {
                    tracing::error!("failed to open a tab: {e:#}");
                }
            }
            Act::Ssh(host, place) => {
                self.start_in(
                    &format!("ssh {}", shell_quote(&host)),
                    None,
                    place,
                    window,
                    cx,
                );
            }
            Act::Rerun(command, new_tab) => match target.filter(|_| !new_tab) {
                Some(view) => self.run_in_pane(&view, &command, None, window, cx),
                None => {
                    let cwd = self.active_folder(cx);
                    self.start_in(&command, cwd, Place::Tab, window, cx);
                }
            },
            Act::Cd(dir) => {
                let line = format!("cd -- {}", shell_quote(&dir.to_string_lossy()));
                match target {
                    Some(view) => self.run_in_pane(&view, &line, Some(dir), window, cx),
                    None => {
                        self.start_in("", Some(dir), Place::Tab, window, cx);
                    }
                }
            }
            Act::Edit(path) => self.open_editor(Some(path), window, cx),
            Act::CloseTab(handle, view) => {
                let _ = handle.update(cx, |any, window, cx| {
                    if let Ok(ws) = any.downcast::<Workspace>() {
                        ws.update(cx, |ws, cx| {
                            if let Some(ix) = ws.tab_of(&view) {
                                ws.close_asking(super::closing::Closing::Tab(ix), window, cx)
                            }
                        })
                    }
                });
            }
            Act::TabToWindow(handle, view) => {
                let _ = handle.update(cx, |any, window, cx| {
                    if let Ok(ws) = any.downcast::<Workspace>() {
                        ws.update(cx, |ws, cx| {
                            if let Some(ix) = ws.tab_of(&view) {
                                ws.move_tab_to_new_window(ix, window, cx)
                            }
                        })
                    }
                });
            }
            Act::Run | Act::Copy(_) | Act::Reveal(_) | Act::Forget(_) => {}
        }
    }

    /// ⌘K on a session: resume, fork it somewhere, read it, copy its id.
    fn session_actions(&mut self, session: Session, window: &mut Window, cx: &mut Context<Self>) {
        let fork = crate::agents::sessions::fork_command(session.agent, &session.id).is_some();
        let label = crate::agents::sessions::fork_label(session.agent);
        let places = [
            ("tab", "New Tab"),
            ("right", "Split Right"),
            ("down", "Split Down"),
            ("window", "New Window"),
        ];
        let mut entries: Vec<crate::native_menu::Entry> =
            vec![crate::native_menu::Item::new("Resume").into()];
        for (_, name) in places {
            entries.push(
                crate::native_menu::Item::new(format!("{label} in {name}"))
                    .enabled(fork)
                    .into(),
            );
        }
        entries.push(crate::native_menu::Entry::Separator);
        entries.push(crate::native_menu::Item::new("View Session History").into());
        entries.push(crate::native_menu::Item::new("Copy Session ID").into());
        let at = self.actions_anchor(window, cx);
        let this = cx.entity().downgrade();
        crate::native_menu::popup(entries, at, Some(0), window, cx, move |ix, window, cx| {
            let _ = this.update(cx, |this, cx| {
                let resume = |fork: bool, place: &str| ResumeSession {
                    agent: session.agent.to_owned(),
                    id: session.id.clone(),
                    cwd: session.cwd.clone(),
                    fork,
                    place: place.into(),
                };
                match ix {
                    0 => {
                        this.close_open_quickly(window, cx);
                        this.resume_session(&resume(false, "tab"), window, cx);
                    }
                    1..=4 => {
                        this.close_open_quickly(window, cx);
                        this.resume_session(&resume(true, places[ix - 1].0), window, cx);
                    }
                    5 => {
                        this.close_open_quickly(window, cx);
                        this.view_history(session.clone(), window, cx);
                    }
                    _ => cx.write_to_clipboard(ClipboardItem::new_string(session.id.clone())),
                }
            });
        });
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
            if key == "k" {
                self.item_actions(window, cx);
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
                    oq.conversations = None;
                    oq._search_conversations = None;
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
            let loading = match oq.filter {
                Filter::Files if oq.files.is_none() => Some("Listing files…"),
                Filter::Agents if oq.sessions.is_none() => Some("Reading agent sessions…"),
                _ => None,
            };
            children.push(
                div()
                    .h(px(ROW_HEIGHT))
                    .pl(px(TEXT_LEFT))
                    .flex()
                    .items_center()
                    .text_color(c.placeholder)
                    .child(loading.unwrap_or("No results"))
                    .into_any_element(),
            );
        }

        let verb = items.get(selected).map_or("Open", |i| i.kind.verb());
        let more = items.get(selected).is_some_and(|i| i.kind == Kind::Session);
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
            .child(if more {
                format!("Actions ⌘K   {verb} ↩")
            } else {
                format!("{verb} ↩")
            });

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

impl Workspace {
    /// Brings forward the tab editing `editor`.
    fn reveal_editor(
        &mut self,
        editor: &Entity<crate::editor::FileEditor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.tabs.iter().position(|t| t.as_editor() == Some(editor)) {
            self.activate(ix, window, cx);
        }
    }
}

/// The sessions whose conversation (prompts and replies, not tool calls or
/// their output) has every word of `query`, newest first, each with the
/// line that has the most of them.
fn conversation_matches(sessions: &[Session], query: &str) -> Vec<Mention> {
    use crate::agents::sessions::{Role, transcript};
    const MAX: usize = 100;
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for session in sessions {
        let mut seen = vec![false; words.len()];
        let mut best: Option<(usize, String)> = None;
        for message in transcript(session) {
            if message.role == Role::Tool || message.text.starts_with('⏺') {
                continue;
            }
            for line in message.text.lines() {
                let lower = line.to_lowercase();
                let hits = words
                    .iter()
                    .enumerate()
                    .filter(|(i, w)| {
                        let hit = lower.contains(w.as_str());
                        if hit {
                            seen[*i] = true;
                        }
                        hit
                    })
                    .count();
                if hits > 0 && best.as_ref().is_none_or(|(n, _)| hits > *n) {
                    best = Some((hits, line.trim().chars().take(160).collect()));
                }
            }
        }
        if seen.iter().all(|&s| s)
            && let Some((_, line)) = best
        {
            out.push((session.clone(), line));
            if out.len() == MAX {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote("/a b/it's"), r"'/a b/it'\''s'");
    }

    #[test]
    fn searches_what_was_said_not_tool_output() {
        let dir = std::env::temp_dir().join(format!("motty-oq-talk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"user","message":{"content":"Fix the flaky login test"}}"#,
                "\n",
                r#"{"type":"assistant","message":{"content":"The login test races the cookie jar."}}"#,
                "\n",
                r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"zebra"}]}}"#,
                "\n",
            ),
        )
        .unwrap();
        let session = Session {
            agent: "claude",
            id: "s".into(),
            path: path.clone(),
            cwd: None,
            title: "t".into(),
            modified: SystemTime::now(),
        };
        let found = conversation_matches(std::slice::from_ref(&session), "LOGIN cookie");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, "The login test races the cookie jar.");
        assert!(conversation_matches(std::slice::from_ref(&session), "zebra").is_empty());
        assert!(conversation_matches(&[session], "login giraffe").is_empty());
        let _ = std::fs::remove_dir_all(dir);
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
