//! The menu bar. Items run the same actions as the key bindings (GPUI shows
//! each one's shortcut from the keymap and greys out items the focused
//! window can't run), so a rebinding shows up here once the menus are set
//! again.
//!
//! Like Otty's, the Shell menu describes the focused pane: its last
//! command, the ports its tab's programs listen on, its git repository and
//! the agent running in it. [`refresh`] rebuilds the menus when those
//! change; what takes a moment to find out (git, ports) is looked up off
//! the main thread first.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{App, AppContext, Global, Menu, MenuItem, OsAction, SystemMenuType, Task};

use crate::actions::*;

/// What the menus show about the frontmost window, cheap to gather.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub pinned: bool,
    /// Secure Keyboard Entry turned on by hand.
    pub secure: bool,
    pub pane: Option<u64>,
    pub cwd: Option<PathBuf>,
    /// The focused pane's last command.
    pub last: Option<LastCommand>,
    /// The agent in the focused pane: its name and session.
    pub agent: Option<(String, Option<String>)>,
    /// Each tab's title and its programs (pid, name), for Ports.
    pub tabs: Vec<(String, Vec<(i32, String)>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LastCommand {
    pub text: String,
    pub row: u64,
    pub exit: Option<i32>,
    pub duration: Option<Duration>,
    pub running: bool,
}

/// What's looked up in the background: the repository and the ports.
#[derive(Clone, Debug, Default, PartialEq)]
struct Live {
    git: Option<Git>,
    /// Per tab: (title, [(port, program)]).
    ports: Vec<(String, Vec<(u16, String)>)>,
}

#[derive(Clone, Debug, PartialEq)]
struct Git {
    branch: String,
    worktree: bool,
    web: Option<String>,
}

#[derive(Default)]
struct State {
    shown: Option<(Snapshot, Live)>,
    pending: Option<Task<()>>,
    /// A refresh was asked for while one was running.
    again: bool,
}

impl Global for State {}

/// The menus with nothing known about a window yet.
pub fn set(cx: &mut App) {
    build(&Snapshot::default(), &Live::default(), cx);
}

/// Rebuilds the menus for the frontmost window, once git and the ports
/// are known.
pub fn refresh(cx: &mut App) {
    // In the background nobody sees the menus; activation refreshes them.
    if cx.active_window().is_none() {
        return;
    }
    let snapshot = crate::workspace::menu_snapshot(cx).unwrap_or_default();
    let state = cx.default_global::<State>();
    if state.pending.is_some() {
        state.again = true;
        return;
    }
    let work = cx.background_spawn({
        let snapshot = snapshot.clone();
        async move { look_up(&snapshot) }
    });
    let task = cx.spawn(async move |cx| {
        let live = work.await;
        cx.update(|cx| {
            let state = cx.default_global::<State>();
            state.pending = None;
            let changed = state.shown.as_ref() != Some(&(snapshot.clone(), live.clone()));
            let again = std::mem::take(&mut state.again);
            if changed {
                state.shown = Some((snapshot.clone(), live.clone()));
                build(&snapshot, &live, cx);
            }
            if again {
                refresh(cx);
            }
        });
    });
    cx.default_global::<State>().pending = Some(task);
}

fn look_up(s: &Snapshot) -> Live {
    let git = s.cwd.as_deref().and_then(git_info);
    let pids: Vec<i32> = s
        .tabs
        .iter()
        .flat_map(|(_, procs)| procs.iter().map(|(pid, _)| *pid))
        .collect();
    let mut ports = Vec::new();
    if !pids.is_empty() {
        let found = crate::workspace::listening_ports(&pids);
        for (title, procs) in &s.tabs {
            let mut list: Vec<(u16, String)> = found
                .iter()
                .filter_map(|(port, pid)| {
                    let (_, name) = procs.iter().find(|(p, _)| p == pid)?;
                    Some((*port, name.clone()))
                })
                .collect();
            list.sort();
            list.dedup();
            if !list.is_empty() {
                ports.push((title.clone(), list));
            }
        }
    }
    Live { git, ports }
}

fn git_info(dir: &std::path::Path) -> Option<Git> {
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
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    let dirs = git(&[
        "rev-parse",
        "--abbrev-ref",
        "HEAD",
        "--git-dir",
        "--git-common-dir",
    ])?;
    let mut lines = dirs.lines();
    let branch = lines.next()?.to_owned();
    let (git_dir, common) = (lines.next()?, lines.next()?);
    let web = git(&["remote", "get-url", "origin"])
        .filter(|r| !r.is_empty())
        .map(|r| crate::workspace::web_url(&r));
    Some(Git {
        branch,
        worktree: git_dir != common,
        web,
    })
}

fn build(s: &Snapshot, live: &Live, cx: &mut App) {
    cx.set_menus([
        Menu::new("Motty").items([
            MenuItem::action("About Motty", About),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::action("Welcome Guide", ShowWelcome),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Motty", HideApp),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Motty", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Window", NewWindow),
            MenuItem::action("New Tab", NewTab),
            MenuItem::action("New File Editor", NewFileEditor),
            MenuItem::separator(),
            MenuItem::action("Open…", OpenFile),
            MenuItem::submenu(recent_menu()),
            MenuItem::action("Open Quickly…", OpenQuickly),
            MenuItem::separator(),
            MenuItem::action("Save", Save),
            MenuItem::separator(),
            MenuItem::action("Close", CloseTab),
            MenuItem::action("Close Window", CloseWindow),
            MenuItem::action("Reopen Closed Tab", ReopenClosedTab),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::submenu(Menu::new("Paste as").items([
                MenuItem::action("Paste Selection", PasteSelection),
                MenuItem::action("Paste Escaping Special Characters", PasteEscaped),
                MenuItem::action("Bracketed Paste", PasteBracketed),
            ])),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Find").items([
                MenuItem::action("Find…", Find),
                MenuItem::action("Find Next", FindNext),
                MenuItem::action("Find Previous", FindPrevious),
                MenuItem::separator(),
                MenuItem::action("Find in All Tabs…", GlobalSearch),
            ])),
            MenuItem::separator(),
            MenuItem::submenu(
                Menu::new("Insert from").items([MenuItem::action("File Path…", InsertFilePath)]),
            ),
            MenuItem::separator(),
            MenuItem::action("Secure Keyboard Entry", ToggleSecureKeyboardEntry).checked(s.secure),
        ]),
        Menu::new("View").items([
            MenuItem::action("Command Palette…", CommandPalette),
            MenuItem::action("Open Quickly…", OpenQuickly),
            MenuItem::action("Jump To…", JumpTo),
            MenuItem::action("Open Files Quickly…", OpenFilesQuickly),
            MenuItem::separator(),
            MenuItem::action("Tabs Panel", ToggleTabsPanel),
            MenuItem::action("Inbox", ToggleInbox),
            MenuItem::action("Details Panel", ToggleDetailsPanel),
            MenuItem::submenu(Menu::new("Details").items([
                MenuItem::action("Info", DetailsInfo),
                MenuItem::action("Outline", DetailsOutline),
                MenuItem::action("Git", DetailsGit),
                MenuItem::action("Files", DetailsFiles),
            ])),
            MenuItem::separator(),
            MenuItem::action("Bigger", IncreaseFontSize),
            MenuItem::action("Smaller", DecreaseFontSize),
            MenuItem::action("Actual Size", ResetFontSize),
            MenuItem::separator(),
            MenuItem::action("Scroll to Top", ScrollToTop),
            MenuItem::action("Scroll to Bottom", ScrollToBottom),
            MenuItem::separator(),
            MenuItem::action(
                if s.pinned {
                    "Unpin Window"
                } else {
                    "Pin Window"
                },
                PinWindow,
            ),
            MenuItem::action("Toggle Full Screen", ToggleFullScreen),
        ]),
        shell_menu(s, live),
        // AppKit lists the open windows after the last separator.
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", ZoomWindow),
            MenuItem::separator(),
            MenuItem::action("Show Next Tab", NextTab),
            MenuItem::action("Show Previous Tab", PreviousTab),
            MenuItem::action("Show Next Unread Tab", ShowNextUnread),
            MenuItem::action("Next Pane", FocusNextPane),
            MenuItem::action("Previous Pane", FocusPreviousPane),
            MenuItem::separator(),
            MenuItem::action("Move Tab Up", MoveTabUp),
            MenuItem::action("Move Tab Down", MoveTabDown),
            MenuItem::action("Move Tab to New Window", MoveTabToNewWindow),
            MenuItem::separator(),
        ]),
        Menu::new("Help").items([MenuItem::action("Motty Help", OpenHelp)]),
    ]);
}

fn recent_menu() -> Menu {
    let home = dirs::home_dir().unwrap_or_default();
    let folders = crate::folders::ranked();
    let mut items: Vec<MenuItem> = folders
        .into_iter()
        .take(10)
        .map(|path| {
            let label = match path.strip_prefix(&home) {
                Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => path.display().to_string(),
            };
            MenuItem::action(label, OpenFolder { path })
        })
        .collect();
    if items.is_empty() {
        items.push(MenuItem::action("No Recent Folders", OpenFolder::default()).disabled(true));
    }
    Menu::new("Open Recent").items(items)
}

fn shell_menu(s: &Snapshot, live: &Live) -> Menu {
    let mut items = vec![
        MenuItem::action("Split Right", SplitRight),
        MenuItem::action("Split Down", SplitDown),
        MenuItem::submenu(Menu::new("Split").items([
            MenuItem::action("Split Left", SplitLeft),
            MenuItem::action("Split Up", SplitUp),
            MenuItem::separator(),
            MenuItem::action("Equalize Splits", EqualizeSplits),
            MenuItem::action("Zoom Split", ZoomSplit),
        ])),
        MenuItem::separator(),
        MenuItem::submenu(about_line(s)),
        MenuItem::submenu(ports_menu(live)),
        MenuItem::submenu(git_menu(live)),
    ];
    if let Some((label, session)) = &s.agent {
        let mut agent = vec![];
        if let Some(id) = session {
            agent.push(MenuItem::action(
                "Copy Session ID",
                CopyText { text: id.clone() },
            ));
        }
        agent.push(MenuItem::action("Clear Badge", ClearBadge));
        items.push(MenuItem::submenu(Menu::new(label.clone()).items(agent)));
    }
    items.extend([
        MenuItem::separator(),
        MenuItem::action("Clear Screen", Clear),
        MenuItem::action("Clear Scrollback Above", ClearScrollbackAbove),
        MenuItem::separator(),
        MenuItem::action("Copy Path", CopyPath),
        MenuItem::action("Reveal in Finder", RevealInFinder),
        MenuItem::separator(),
        MenuItem::action("Vi Mode", ToggleViMode),
        MenuItem::submenu(Menu::new("Hint Mode").items([
            MenuItem::action("Open Link", HintOpen),
            MenuItem::action("Copy Link", HintCopy),
            MenuItem::action("Paste Word", HintPaste),
            MenuItem::action("Reveal Path", HintReveal),
        ])),
        MenuItem::action("Read Only", ToggleReadOnly),
        MenuItem::separator(),
        MenuItem::action("Rename Tab…", RenameTab),
        MenuItem::action("Clear Badge", ClearBadge),
    ]);
    Menu::new("Shell").items(items)
}

/// About This Line: the focused pane's last command, and the pane.
fn about_line(s: &Snapshot) -> Menu {
    let mut items = Vec::new();
    if let Some(last) = &s.last {
        items.push(MenuItem::action(
            truncate(&last.text, 60),
            RevealRow { row: last.row },
        ));
        let status = if last.running {
            "Running".to_owned()
        } else {
            let took = last.duration.map(short).unwrap_or_default();
            match last.exit {
                Some(0) => format!("Succeeded {took}"),
                Some(code) => format!("Exited with {code} {took}"),
                None => "Finished".to_owned(),
            }
        };
        items.push(MenuItem::action(status.trim().to_owned(), RevealRow::default()).disabled(true));
        items.push(MenuItem::action(
            "Copy Command",
            CopyText {
                text: last.text.clone(),
            },
        ));
        items.push(MenuItem::separator());
    }
    match s.pane {
        Some(id) => items.push(MenuItem::action(
            format!("Pane {id} — Copy ID"),
            CopyText {
                text: id.to_string(),
            },
        )),
        None => items.push(MenuItem::action("No Pane", CopyText::default()).disabled(true)),
    }
    Menu::new("About This Line").items(items)
}

fn ports_menu(live: &Live) -> Menu {
    let mut items = Vec::new();
    for (title, ports) in &live.ports {
        items.push(MenuItem::action(truncate(title, 40), CopyText::default()).disabled(true));
        for (port, name) in ports {
            let url = format!("http://localhost:{port}");
            items.push(MenuItem::submenu(
                Menu::new(format!("{port}  {name}")).items([
                    MenuItem::action("Open in Browser", OpenUrl { url: url.clone() }),
                    MenuItem::action("Copy URL", CopyText { text: url }),
                ]),
            ));
        }
    }
    if items.is_empty() {
        items.push(MenuItem::action("No Listening Ports", CopyText::default()).disabled(true));
    }
    Menu::new("Ports").items(items)
}

fn git_menu(live: &Live) -> Menu {
    let Some(git) = &live.git else {
        return Menu::new("Git").items([MenuItem::action(
            "Not a Git Repository",
            CopyText::default(),
        )
        .disabled(true)]);
    };
    let branch = if git.worktree {
        format!("{} (worktree)", git.branch)
    } else {
        git.branch.clone()
    };
    let mut items = vec![
        MenuItem::action(branch, CopyText::default()).disabled(true),
        MenuItem::separator(),
        MenuItem::action("Commit…", TypeCommand::new("git commit -m ", false)),
        MenuItem::action("Create Branch…", TypeCommand::new("git switch -c ", false)),
        MenuItem::separator(),
        MenuItem::action("Fetch…", TypeCommand::new("git fetch", true)),
        MenuItem::action("Pull…", TypeCommand::new("git pull", true)),
        MenuItem::action("Push…", TypeCommand::new("git push", true)),
    ];
    if let Some(web) = &git.web {
        items.push(MenuItem::separator());
        items.push(MenuItem::action(
            "Open Remote URL",
            OpenUrl { url: web.clone() },
        ));
    }
    Menu::new("Git").items(items)
}

fn truncate(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let mut out: String = line.chars().take(max - 1).collect();
    out.push('…');
    out
}

fn short(d: Duration) -> String {
    let secs = d.as_secs_f32();
    if secs < 1. {
        format!("in {}ms", d.as_millis())
    } else if secs < 60. {
        format!("in {secs:.1}s")
    } else {
        format!("in {}m {}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

/// The standard About panel, with the bundle's name, version and icon.
pub fn show_about() {
    #[cfg(target_os = "macos")]
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        app.orderFrontStandardAboutPanel(None);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_texts() {
        assert_eq!(
            truncate("cargo build --release\nmore", 60),
            "cargo build --release"
        );
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(short(Duration::from_millis(250)), "in 250ms");
        assert_eq!(short(Duration::from_secs(75)), "in 1m 15s");
    }
}
