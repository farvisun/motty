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
    /// What forking that session is called (`Branch`), when it can be.
    pub fork: Option<&'static str>,
    /// Each tab's title and its programs (pid, name), for Ports.
    pub tabs: Vec<(String, Vec<(i32, String)>)>,
    pub read_only: bool,
    pub vi: bool,
    /// The active tab's Notifications & Privileges switches, and whether
    /// they're its own rather than Settings'.
    pub privileges: Option<(Vec<(&'static str, bool)>, bool)>,
    /// Open in: (name, bundle id).
    pub open_with: Vec<(String, String)>,
    /// The window's theme, checked in View › Theme.
    pub theme: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LastCommand {
    pub text: String,
    pub row: u64,
    pub exit: Option<i32>,
    pub duration: Option<Duration>,
    pub running: bool,
    /// `14:32:05`: when it started.
    pub ran_at: Option<String>,
}

/// What's looked up in the background: the repository and the ports.
#[derive(Clone, Debug, Default, PartialEq)]
struct Live {
    git: Option<Git>,
    /// Per tab: (title, [(port, program)]).
    ports: Vec<(String, Vec<(u16, String)>)>,
    shells: Vec<PathBuf>,
    hosts: Vec<String>,
    folders: Vec<PathBuf>,
    files: Vec<PathBuf>,
    /// Saved recipes: (id, name).
    recipes: Vec<(String, String)>,
    /// Every theme: (name, dark).
    themes: Vec<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq)]
struct Git {
    branch: String,
    worktree: bool,
    web: Option<String>,
    /// The other local branches, for Merge and Rebase.
    branches: Vec<String>,
    /// The repository's other worktrees.
    worktrees: Vec<std::path::PathBuf>,
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
    // What's quick to read is there from the start.
    let live = Live {
        shells: crate::workspace::shells(),
        hosts: crate::workspace::ssh_hosts(),
        files: crate::folders::recent_files(),
        recipes: saved_recipes(),
        themes: theme_names(),
        ..Live::default()
    };
    build(&Snapshot::default(), &live, cx);
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

/// The focused pane's repository as the menu bar last found it: its
/// branch (marked when a worktree) and its web page.
pub fn git(cx: &App) -> Option<(String, Option<String>)> {
    let state = cx.try_global::<State>()?;
    let git = state.shown.as_ref()?.1.git.as_ref()?;
    let branch = if git.worktree {
        format!("{} (worktree)", git.branch)
    } else {
        git.branch.clone()
    };
    Some((branch, git.web.clone()))
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
    // Servers started elsewhere, or left running detached.
    let other = crate::workspace::other_listening_ports(&pids);
    if !other.is_empty() {
        ports.push(("Other".to_owned(), other));
    }
    Live {
        git,
        ports,
        shells: crate::workspace::shells(),
        hosts: crate::workspace::ssh_hosts(),
        folders: crate::folders::ranked().into_iter().take(RECENT).collect(),
        files: crate::folders::recent_files(),
        recipes: saved_recipes(),
        themes: theme_names(),
    }
}

fn theme_names() -> Vec<(String, bool)> {
    motty_theme::config::list_themes()
        .into_iter()
        .map(|(name, mode, _)| (name, mode == motty_theme::Mode::Dark))
        .collect()
}

/// File › Recipe lists the saved recipes, newest first.
fn saved_recipes() -> Vec<(String, String)> {
    crate::recipes::list()
        .into_iter()
        .map(|s| (s.id, s.recipe.recipe.name))
        .collect()
}

/// Open Folder and Open Recent list this many.
const RECENT: usize = 10;

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
    let branches = git(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .unwrap_or_default()
        .lines()
        .filter(|b| *b != branch)
        .take(30)
        .map(str::to_owned)
        .collect();
    let top = git(&["rev-parse", "--show-toplevel"]).map(std::path::PathBuf::from);
    let worktrees = git(&["worktree", "list", "--porcelain"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(std::path::PathBuf::from)
        .filter(|w| Some(w) != top.as_ref())
        .collect();
    Some(Git {
        branch,
        worktree: git_dir != common,
        web,
        branches,
        worktrees,
    })
}

fn build(s: &Snapshot, live: &Live, cx: &mut App) {
    cx.set_menus([
        Menu::new("Motty").items([
            MenuItem::action("About Motty", About),
            MenuItem::separator(),
            MenuItem::action("Settings...", OpenSettings),
            MenuItem::action("Welcome Guide", ShowWelcome),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Motty", HideApp),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Motty", Quit),
            MenuItem::action("Quit and Keep Windows", QuitKeepWindows),
        ]),
        file_menu(live),
        edit_menu(s),
        shell_menu(s, live),
        view_menu(s, live),
        // AppKit lists the open windows after the last separator.
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", ZoomWindow),
            MenuItem::separator(),
            MenuItem::action("Show Previous Tab", PreviousTab),
            MenuItem::action("Show Next Tab", NextTab),
            MenuItem::action("Show Unread Tab", ShowNextUnread),
            MenuItem::action("Move Tab to New Window", MoveTabToNewWindow),
            MenuItem::action("Merge All Windows", MergeAllWindows),
            MenuItem::separator(),
            MenuItem::action("Memory Report", MemoryReport),
            MenuItem::separator(),
        ]),
        Menu::new("Help").items([
            MenuItem::action("Motty Help", OpenHelp),
            MenuItem::action(
                "Send Feedback...",
                OpenUrl {
                    url: "https://github.com/farvisun/motty/issues".into(),
                },
            ),
            MenuItem::separator(),
            MenuItem::action("Reload Terminal View", ReloadTerminalView),
        ]),
    ]);
    crate::window_menu::patch();
}

fn file_menu(live: &Live) -> Menu {
    let home = dirs::home_dir().unwrap_or_default();
    let shells: Vec<MenuItem> = live
        .shells
        .iter()
        .map(|path| {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into(),
            );
            MenuItem::action(name, NewShell { path: path.clone() })
        })
        .collect();

    let mut recent: Vec<MenuItem> = live
        .files
        .iter()
        .map(|path| {
            MenuItem::action(
                tilde(path, &home),
                EditFiles {
                    paths: vec![path.clone()],
                },
            )
        })
        .collect();
    if recent.is_empty() {
        recent.push(MenuItem::action("No Recent Items", CopyText::default()).disabled(true));
    } else {
        recent.push(MenuItem::separator());
        recent.push(MenuItem::action("Clear Menu", ClearRecentFiles));
    }

    let mut recipe = vec![
        MenuItem::action("Save...", SaveRecipe),
        MenuItem::action("Export...", ExportRecipe),
        MenuItem::action("Save Selection to Recipe...", SaveSelectionToRecipe),
        MenuItem::separator(),
        MenuItem::action("Open Recipe File...", OpenRecipeFile),
    ];
    if !live.recipes.is_empty() {
        recipe.push(MenuItem::separator());
        recipe.extend(
            live.recipes
                .iter()
                .map(|(id, name)| MenuItem::action(name.clone(), ApplyRecipe { id: id.clone() })),
        );
    }

    let mut folders = vec![MenuItem::action("~/", OpenFolder { path: home.clone() })];
    folders.extend(
        live.folders
            .iter()
            .map(|path| MenuItem::action(tilde(path, &home), OpenFolder { path: path.clone() })),
    );
    folders.push(MenuItem::separator());
    folders.push(MenuItem::action("Config...", OpenConfigFolder));

    let mut hosts: Vec<MenuItem> = live
        .hosts
        .iter()
        .map(|host| MenuItem::action(host.clone(), ConnectToHost { host: host.clone() }))
        .collect();
    if hosts.is_empty() {
        hosts.push(MenuItem::action("No Hosts Yet", CopyText::default()).disabled(true));
    }

    Menu::new("File").items([
        MenuItem::action("New Window", NewWindow),
        MenuItem::action("New Tab", NewTab),
        MenuItem::submenu(Menu::new("New Shell").items(shells)),
        MenuItem::action("Duplicate Tab", DuplicateTab),
        MenuItem::separator(),
        MenuItem::action("New File Editor", NewFileEditor),
        MenuItem::action("New File Browser", NewFileBrowser),
        MenuItem::action("New Web Browser", NewWebBrowser),
        MenuItem::separator(),
        MenuItem::submenu(Menu::new("Open Recent").items(recent)),
        MenuItem::submenu(Menu::new("Open Folder").items(folders)),
        MenuItem::submenu(Menu::new("Connect to Host").items(hosts)),
        MenuItem::separator(),
        MenuItem::action("Export Text As...", ExportText),
        MenuItem::action("Export Selected Text As...", ExportSelectedText),
        MenuItem::separator(),
        MenuItem::action("Save...", SaveRecipe),
        MenuItem::action("Export Recipe...", ExportRecipe),
        MenuItem::action("Open Recipe...", OpenRecipe),
        MenuItem::submenu(Menu::new("Recipe").items(recipe)),
        MenuItem::separator(),
        MenuItem::action("Close", CloseTab),
        MenuItem::action("Close Window", CloseWindow),
        MenuItem::action("Close All", CloseAll),
    ])
}

fn edit_menu(s: &Snapshot) -> Menu {
    Menu::new("Edit").items([
        MenuItem::os_action("Undo", Undo, OsAction::Undo),
        MenuItem::os_action("Redo", Redo, OsAction::Redo),
        MenuItem::separator(),
        MenuItem::os_action("Cut", Cut, OsAction::Cut),
        MenuItem::os_action("Copy", Copy, OsAction::Copy),
        MenuItem::os_action("Paste", Paste, OsAction::Paste),
        MenuItem::submenu(Menu::new("Copy / Paste as").items([
            MenuItem::action("Copy Selection", CopySelection),
            MenuItem::action("Copy with Style", CopyWithStyle),
            MenuItem::action("Copy as ANSI Sequence", CopyAsAnsi),
            MenuItem::separator(),
            MenuItem::action("Copy Screen", CopyScreen),
            MenuItem::action("Copy Screen with Style", CopyScreenWithStyle),
            MenuItem::action("Copy Screen as ANSI Sequence", CopyScreenAsAnsi),
            MenuItem::separator(),
            MenuItem::action("Paste Selection", PasteSelection),
            MenuItem::action("Paste File Base64-Encoded…", PasteFileBase64),
            MenuItem::action("Paste Escaping Special Characters", PasteEscaped),
            MenuItem::action("Bracketed Paste", PasteBracketed),
            MenuItem::separator(),
            MenuItem::action("Paste and continue in Composer", PasteToComposer),
        ])),
        MenuItem::separator(),
        MenuItem::submenu(Menu::new("Insert").items([
            MenuItem::action("File Path...", InsertFilePath),
            MenuItem::action("Screenshot", InsertScreenshot),
        ])),
        MenuItem::action("Composer", ToggleComposer),
        MenuItem::action("Prompt Queue...", TogglePromptQueue),
        MenuItem::action("Send to Chat...", SendToChat),
        MenuItem::separator(),
        MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        MenuItem::separator(),
        MenuItem::submenu(Menu::new("Find").items([
            MenuItem::action("Find...", Find),
            MenuItem::action("Find Next", FindNext),
            MenuItem::action("Find Previous", FindPrevious),
            MenuItem::separator(),
            MenuItem::action("Use Selection for Find", UseSelectionForFind),
            MenuItem::action("Jump to Selection", JumpToSelection),
            MenuItem::separator(),
            MenuItem::action("Find in All Tabs...", GlobalSearch),
        ])),
        MenuItem::separator(),
        MenuItem::action("Secure Keyboard Entry", ToggleSecureKeyboardEntry).checked(s.secure),
    ])
}

fn view_menu(s: &Snapshot, live: &Live) -> Menu {
    // Theme: Switch Theme… and the files, then every theme, dark first.
    let mut themes = vec![
        MenuItem::action("Switch Theme…", SwitchTheme),
        MenuItem::submenu(
            Menu::new("Import Theme").items(motty_theme::import::Format::ALL.map(|f| {
                MenuItem::action(
                    format!("{}…", f.name()),
                    ImportTheme {
                        format: f.name().to_owned(),
                    },
                )
            })),
        ),
        MenuItem::action("Edit Theme…", EditTheme),
        MenuItem::action("Open Theme File", OpenThemeFile),
        MenuItem::action("Reload Theme", ReloadTheme),
    ];
    for dark in [true, false] {
        themes.push(MenuItem::separator());
        themes.extend(
            live.themes
                .iter()
                .filter(|(_, d)| *d == dark)
                .map(|(name, _)| {
                    MenuItem::action(name.clone(), SetTheme { name: name.clone() })
                        .checked(name.eq_ignore_ascii_case(&s.theme))
                }),
        );
    }
    Menu::new("View").items([
        MenuItem::action("Split Right", SplitRight),
        MenuItem::action("Split Left", SplitLeft),
        MenuItem::action("Split Down", SplitDown),
        MenuItem::action("Split Up", SplitUp),
        MenuItem::separator(),
        MenuItem::action("Zoom Split", ZoomSplit),
        MenuItem::submenu(Menu::new("Resize Split").items([
            MenuItem::action("Equalize Splits", EqualizeSplits),
            MenuItem::separator(),
            MenuItem::action("Move Divider Up", MoveDividerUp),
            MenuItem::action("Move Divider Down", MoveDividerDown),
            MenuItem::action("Move Divider Left", MoveDividerLeft),
            MenuItem::action("Move Divider Right", MoveDividerRight),
        ])),
        MenuItem::separator(),
        MenuItem::submenu(Menu::new("Focus to Pane").items([
            MenuItem::action("Focus Pane Above", FocusPaneUp),
            MenuItem::action("Focus Pane Below", FocusPaneDown),
            MenuItem::action("Focus Pane to the Left", FocusPaneLeft),
            MenuItem::action("Focus Pane to the Right", FocusPaneRight),
            MenuItem::separator(),
            MenuItem::action("Focus Next Pane", FocusNextPane),
            MenuItem::action("Focus Previous Pane", FocusPreviousPane),
        ])),
        MenuItem::separator(),
        MenuItem::action("Command Palette", CommandPalette),
        MenuItem::action("Open Quickly", OpenQuickly),
        MenuItem::action("Jump To", JumpTo),
        MenuItem::action("Open Files Quickly", OpenFilesQuickly),
        MenuItem::action("Open Agent Session", AgentHistory),
        MenuItem::separator(),
        MenuItem::action("Search Across Files", SearchInFiles),
        MenuItem::action("Search Agent Conversations", SearchAgentConversations),
        MenuItem::separator(),
        MenuItem::action("Toggle Tabs Panel", ToggleTabsPanel),
        MenuItem::action("Toggle Details Panel", ToggleDetailsPanel),
        MenuItem::action("Inbox", ToggleInbox),
        MenuItem::separator(),
        MenuItem::action("Info", DetailsInfo),
        MenuItem::action("Outline", DetailsOutline),
        MenuItem::action("Git", DetailsGit),
        MenuItem::action("Files", DetailsFiles),
        MenuItem::separator(),
        MenuItem::submenu(Menu::new("Switch to View").items([
            MenuItem::action("Terminal", SwitchToTerminal),
            MenuItem::action("File Browser", SwitchToFileBrowser),
        ])),
        MenuItem::submenu(Menu::new("Theme").items(themes)),
        MenuItem::separator(),
        MenuItem::action("Increase Font Size", IncreaseFontSize),
        MenuItem::action("Decrease Font Size", DecreaseFontSize),
        MenuItem::action("Reset Font Size", ResetFontSize),
        MenuItem::separator(),
        MenuItem::action("Pin Window", PinWindow).checked(s.pinned),
    ])
}

/// `~/code/motty`: a path as the menus show it.
fn tilde(path: &std::path::Path, home: &std::path::Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~/".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn shell_menu(s: &Snapshot, live: &Live) -> Menu {
    let mut open_in: Vec<MenuItem> = s
        .open_with
        .iter()
        .map(|(name, bundle)| {
            MenuItem::action(
                name.clone(),
                OpenIn {
                    bundle: bundle.clone(),
                },
            )
        })
        .collect();
    if open_in.is_empty() {
        open_in.push(MenuItem::action("No Apps", CopyText::default()).disabled(true));
    }
    let mut items = vec![
        MenuItem::action("Rename Tab...", RenameTab),
        MenuItem::action("Set Tab Prefix...", SetTabPrefix),
        MenuItem::separator(),
        MenuItem::action("Clear Screen", Clear),
        MenuItem::action("Clear Scrollback Above", ClearScrollbackAbove),
        MenuItem::separator(),
        MenuItem::action("Copy Path", CopyPath),
        MenuItem::action("Reveal in Finder", RevealInFinder),
        MenuItem::submenu(Menu::new("Open in").items(open_in)),
        MenuItem::separator(),
        MenuItem::action("Vi Mode", ToggleViMode).checked(s.vi),
        MenuItem::action("Mark Mode", MarkMode),
        MenuItem::action("Open Link (Hint Mode)", HintOpen),
        MenuItem::action("Read Only", ToggleReadOnly).checked(s.read_only),
        MenuItem::separator(),
        MenuItem::submenu(ports_menu(live)),
        MenuItem::submenu(git_menu(live)),
        MenuItem::submenu(about_line(s)),
    ];
    if let Some((label, session)) = &s.agent {
        let mut agent = vec![];
        if let Some(id) = session {
            agent.push(MenuItem::action(
                "Copy Session ID",
                CopyText { text: id.clone() },
            ));
            agent.push(MenuItem::action("View Session History", ViewSessionHistory));
        }
        if let Some(fork) = s.fork {
            agent.push(MenuItem::submenu(Menu::new(format!("{fork} in")).items([
                MenuItem::action("New Tab", ForkSessionInNewTab),
                MenuItem::action("New Window", ForkSessionInNewWindow),
                MenuItem::separator(),
                MenuItem::action("Split Right", ForkSessionRight),
                MenuItem::action("Split Left", ForkSessionLeft),
                MenuItem::action("Split Down", ForkSessionDown),
                MenuItem::action("Split Up", ForkSessionUp),
            ])));
        }
        agent.push(MenuItem::separator());
        agent.push(MenuItem::action("Clear Badge", ClearBadge));
        items.push(MenuItem::submenu(Menu::new(label.clone()).items(agent)));
    }
    let privileges = match &s.privileges {
        Some((list, own)) => {
            let mut items: Vec<MenuItem> = list
                .iter()
                .enumerate()
                .map(|(index, (label, on))| {
                    MenuItem::action(*label, TogglePrivilege { index }).checked(*on)
                })
                .collect();
            items.push(MenuItem::separator());
            items.push(
                MenuItem::action("Use Settings Defaults", UseDefaultPrivileges).disabled(!own),
            );
            items
        }
        None => vec![MenuItem::action("No Active Tab", CopyText::default()).disabled(true)],
    };
    items.extend([
        MenuItem::separator(),
        MenuItem::submenu(Menu::new("Notifications & Privileges").items(privileges)),
    ]);
    Menu::new("Shell").items(items)
}

/// About This Line: the focused pane's last command, and the pane.
fn about_line(s: &Snapshot) -> Menu {
    let mut items = Vec::new();
    if let Some(last) = &s.last {
        let label = match &last.ran_at {
            Some(at) => format!("{} — {at}", truncate(&last.text, 50)),
            None => truncate(&last.text, 60),
        };
        items.push(MenuItem::action(label, RevealRow { row: last.row }));
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
    // Merge and Rebase type the command for you to check, not run it.
    if !git.branches.is_empty() {
        items.push(MenuItem::separator());
        for (label, verb) in [("Merge…", "merge"), ("Rebase…", "rebase")] {
            items.push(MenuItem::submenu(Menu::new(label).items(
                git.branches.iter().map(|b| {
                    MenuItem::action(
                        b.clone(),
                        TypeCommand::new(&format!("git {verb} {b}"), false),
                    )
                }),
            )));
        }
    }
    if !git.worktrees.is_empty() {
        items.push(MenuItem::submenu(Menu::new("Switch to Worktree").items(
            git.worktrees.iter().map(|w| {
                let line = format!("cd -- '{}'", w.display().to_string().replace('\'', "'\\''"));
                MenuItem::action(
                    tilde(w, &dirs::home_dir().unwrap_or_default()),
                    TypeCommand::new(&line, true),
                )
            }),
        )));
    }
    let clients = git_clients();
    if !clients.is_empty() {
        items.push(MenuItem::submenu(Menu::new("Open in").items(
            clients.into_iter().map(|(name, bundle)| {
                MenuItem::action(
                    name,
                    OpenIn {
                        bundle: bundle.to_owned(),
                    },
                )
            }),
        )));
    }
    if let Some(web) = &git.web {
        items.push(MenuItem::separator());
        items.push(MenuItem::action(
            "Open Remote URL",
            OpenUrl { url: web.clone() },
        ));
    }
    Menu::new("Git").items(items)
}

/// The Git clients installed, as (name, bundle id).
fn git_clients() -> Vec<(&'static str, &'static str)> {
    [
        (
            "GitHub Desktop",
            "GitHub Desktop",
            "com.github.GitHubClient",
        ),
        ("Tower", "Tower", "com.fournova.Tower3"),
        ("Fork", "Fork", "com.DanPristupov.Fork"),
        ("Sublime Merge", "Sublime Merge", "com.sublimemerge"),
        ("GitKraken", "GitKraken", "com.axosoft.gitkraken"),
        ("Sourcetree", "Sourcetree", "com.torusknot.SourceTreeNotMac"),
    ]
    .into_iter()
    .filter(|(_, app, _)| std::path::Path::new(&format!("/Applications/{app}.app")).exists())
    .map(|(name, _, bundle)| (name, bundle))
    .collect()
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
