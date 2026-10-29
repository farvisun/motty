//! `motty window|tab|pane <verb>`: Otty's noun-verb commands for the
//! running app. Selectors are `motty_ipc::select`'s; one that names nothing
//! exits 4.

use std::io::Read as _;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use clap::{Args, Subcommand, ValueEnum};
use motty_ipc::link::{self, Link};
use motty_ipc::{
    ActionParams, CloseParams, Created, Direction, FindParams, FindResult, FocusParams, LinkInfo,
    LinkParams, MoveTabParams, NewTabParams, NewWindowParams, PaneInfo, PaneMode, PaneRef,
    ReadParams, RenameParams, ResizeParams, RunParams, RunResult, SendTextParams, SetModeParams,
    SplitParams, Submit, TabBadgeParams, TabInfo, TextRange, WindowCreated, WindowInfo, ZoomParams,
    method,
};
use serde_json::{Value, json};

use crate::keys;
use crate::output::{Ctx, own_pane, target};

/// `--window`.
#[derive(Args, Clone, Default)]
pub struct WindowSel {
    /// w_<id>, an index (frontmost first), current, last or title:<pattern>.
    #[arg(long, value_name = "SEL")]
    pub window: Option<String>,
}

/// `--tab` and `--window`.
#[derive(Args, Clone, Default)]
pub struct TabSel {
    /// t_<id>, an index in its window, or current.
    #[arg(long, value_name = "SEL")]
    pub tab: Option<String>,
    #[command(flatten)]
    pub window: WindowSel,
}

/// `--pane`, `--tab` and `--window`; none means the pane this runs in.
#[derive(Args, Clone, Default)]
pub struct PaneSel {
    /// A pane id ($MOTTY_PANE_ID, or p_<id>), index:<n> in its tab, or current.
    #[arg(long, value_name = "SEL")]
    pub pane: Option<String>,
    #[command(flatten)]
    pub tab: TabSel,
}

impl WindowSel {
    fn target(&self) -> PaneRef {
        target(None, None, self.window.as_deref())
    }
}

impl TabSel {
    fn target(&self) -> PaneRef {
        target(None, self.tab.as_deref(), self.window.window.as_deref())
    }
}

impl PaneSel {
    fn target(&self) -> PaneRef {
        target(
            self.pane.as_deref(),
            self.tab.tab.as_deref(),
            self.tab.window.window.as_deref(),
        )
    }

    /// Only the filters given, for lists (no default pane).
    fn filter(&self) -> PaneRef {
        PaneRef {
            pane: None,
            select: self.pane.clone(),
            tab: self.tab.tab.clone(),
            window: self.tab.window.window.clone(),
        }
    }
}

#[derive(Subcommand)]
pub enum WindowCmd {
    /// Every window, frontmost first.
    List,
    /// One window (the one this runs in, else the frontmost).
    Show(WindowSel),
    /// A new window (as `motty open`).
    New(NewWindow),
    /// Close a window. Refused when Motty would ask first, unless --force.
    Close {
        #[command(flatten)]
        sel: WindowSel,
        #[arg(long)]
        force: bool,
    },
    /// Bring a window to the front.
    Focus(WindowSel),
    /// Name a window; an empty name gives back the title Motty makes.
    Rename {
        #[command(flatten)]
        sel: WindowSel,
        name: String,
    },
}

#[derive(Args)]
pub struct NewWindow {
    /// Where its shell starts.
    #[arg(long)]
    pub cwd: Option<String>,
    /// A command line to run in it.
    #[arg(long)]
    pub command: Option<String>,
    #[arg(long)]
    pub title: Option<String>,
}

#[derive(Subcommand)]
pub enum TabCmd {
    /// Tabs: every window's, or one window's with --window.
    List(WindowSel),
    /// One tab (the one this runs in, else the frontmost window's).
    Show(TabSel),
    /// A new tab; prints its pane's id.
    New {
        #[command(flatten)]
        sel: WindowSel,
        #[arg(long)]
        cwd: Option<String>,
        /// A command line to run in it.
        #[arg(long)]
        command: Option<String>,
        /// The tab's name.
        #[arg(long)]
        title: Option<String>,
    },
    /// Close a tab. Refused when Motty would ask first, unless --force.
    Close {
        #[command(flatten)]
        sel: TabSel,
        #[arg(long)]
        force: bool,
    },
    /// Switch to a tab (and bring its window forward).
    Focus(TabSel),
    /// Name a tab; an empty name gives back its title.
    Rename {
        #[command(flatten)]
        sel: TabSel,
        name: String,
    },
    /// Move a tab: to a place in its window, to another window, or into a
    /// new one.
    Move {
        #[command(flatten)]
        sel: TabSel,
        /// Its place (from 1), in its window or in --to-window.
        #[arg(long, value_name = "N")]
        index: Option<usize>,
        /// Into this window.
        #[arg(long, value_name = "SEL", conflicts_with = "new_window")]
        to_window: Option<String>,
        /// Into a window of its own.
        #[arg(long)]
        new_window: bool,
    },
    /// Set a tab's status badge, or clear it.
    Badge {
        #[command(flatten)]
        sel: TabSel,
        #[arg(long, value_enum, required_unless_present = "clear")]
        kind: Option<BadgeKind>,
        #[arg(long, conflicts_with = "kind")]
        clear: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum BadgeKind {
    Running,
    Completed,
    Finished,
    Unread,
    Error,
    AwaitingInput,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    pub fn ipc(self) -> Direction {
        match self {
            Dir::Left => Direction::Left,
            Dir::Right => Direction::Right,
            Dir::Up => Direction::Up,
            Dir::Down => Direction::Down,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Mode {
    Normal,
    Vi,
    ReadOnly,
    Hints,
}

impl Mode {
    pub fn ipc(self) -> PaneMode {
        match self {
            Mode::Normal => PaneMode::Normal,
            Mode::Vi => PaneMode::Vi,
            Mode::ReadOnly => PaneMode::ReadOnly,
            Mode::Hints => PaneMode::Hints,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum SubmitWhen {
    Queue,
    Now,
}

impl SubmitWhen {
    pub fn ipc(self) -> Submit {
        match self {
            SubmitWhen::Queue => Submit::Queue,
            SubmitWhen::Now => Submit::Now,
        }
    }
}

#[derive(Args)]
pub struct Split {
    #[command(flatten)]
    sel: PaneSel,
    #[arg(long, short, value_enum, default_value = "right")]
    direction: Dir,
    /// The new pane's share of the space, in percent.
    #[arg(long, value_parser = clap::value_parser!(u8).range(10..=90))]
    size: Option<u8>,
    #[arg(long)]
    cwd: Option<String>,
    /// A command line to run in it.
    #[arg(long)]
    command: Option<String>,
    /// Keep focus where it is.
    #[arg(long)]
    no_focus: bool,
}

#[derive(Subcommand)]
pub enum PaneCmd {
    /// Panes: all of them, or a tab's or window's.
    List {
        #[command(flatten)]
        sel: TabSel,
    },
    /// One pane (the one this runs in, else the focused one).
    Show(PaneSel),
    /// A new pane beside one (as `pane split`).
    New(Split),
    /// Split a pane; prints the new pane's id.
    Split(Split),
    /// Close a pane. Refused when Motty would ask first, unless --force.
    Close {
        #[command(flatten)]
        sel: PaneSel,
        #[arg(long)]
        force: bool,
    },
    /// Focus a pane: by selector, or a motty:// link; --line scrolls that
    /// line into view and lights it.
    Focus {
        /// A pane selector or a motty:// link.
        target: Option<String>,
        #[command(flatten)]
        sel: PaneSel,
        #[arg(long, value_name = "N")]
        line: Option<u64>,
    },
    /// Show only this pane in its tab, or all of them again.
    Zoom {
        #[command(flatten)]
        sel: PaneSel,
        #[arg(long, conflicts_with = "off")]
        on: bool,
        #[arg(long)]
        off: bool,
    },
    /// Move the dividers beside a pane, by cells.
    Resize {
        #[command(flatten)]
        sel: PaneSel,
        #[arg(long, value_name = "N", default_value_t = 0)]
        left: u16,
        #[arg(long, value_name = "N", default_value_t = 0)]
        right: u16,
        #[arg(long, value_name = "N", default_value_t = 0)]
        up: u16,
        #[arg(long, value_name = "N", default_value_t = 0)]
        down: u16,
    },
    /// Type text and keys: `-- "echo hi" key:Enter`, `key:C-c`. Needs
    /// ipc-allow-send-keys.
    SendKeys {
        #[command(flatten)]
        sel: PaneSel,
        #[arg(required = true, allow_hyphen_values = true)]
        keys: Vec<String>,
    },
    /// Type text, C escapes and all (`'git status\n'`, `'\x03'`). Needs
    /// ipc-allow-send-keys.
    SendText {
        #[command(flatten)]
        sel: PaneSel,
        #[arg(required_unless_present_any = ["from_file", "stdin"], allow_hyphen_values = true)]
        text: Option<String>,
        /// Send this file's bytes as they are.
        #[arg(long, value_name = "PATH", conflicts_with_all = ["text", "stdin"])]
        from_file: Option<String>,
        /// Send standard input as it is.
        #[arg(long, conflicts_with = "text")]
        stdin: bool,
        /// Press Return too: when it's the pane's turn, or `=now`.
        #[arg(long, value_enum, num_args = 0..=1, require_equals = true, default_missing_value = "queue")]
        submit: Option<SubmitWhen>,
    },
    /// Run a command line in a pane's shell and report how it went (not
    /// its output); exits with its status. Needs shell integration.
    Run {
        #[command(flatten)]
        sel: PaneSel,
        #[arg(trailing_var_arg = true, required = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// As `run`, and hand back its stdout and stderr apart.
    Exec {
        #[command(flatten)]
        sel: PaneSel,
        #[arg(trailing_var_arg = true, required = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Block until work already running is done: a pane, every pane of a
    /// tab (--tab) or window, or an agent session. Exits 6 when a pane
    /// can't tell, 9 on timeout.
    Wait {
        #[arg(long, value_name = "SEL")]
        pane: Option<String>,
        /// Every pane of this tab (bare: the one this runs in).
        #[arg(long, value_name = "SEL", num_args = 0..=1, default_missing_value = "")]
        tab: Option<String>,
        /// Every pane of this window.
        #[arg(long, value_name = "SEL")]
        window: Option<String>,
        /// The pane running this agent session.
        #[arg(long, conflicts_with_all = ["pane", "tab", "window"])]
        session: Option<String>,
        #[arg(long)]
        timeout_secs: Option<f64>,
    },
    /// Print what's on a pane: the screen, its last N lines, all of its
    /// scrollback, or its last command's output.
    Capture {
        #[command(flatten)]
        sel: PaneSel,
        #[arg(long, conflicts_with_all = ["scrollback", "last_command"])]
        lines: Option<usize>,
        #[arg(long, conflicts_with = "last_command")]
        scrollback: bool,
        #[arg(long)]
        last_command: bool,
    },
    /// Print a motty:// link to a pane, or to a line of it.
    Link {
        /// A pane selector.
        target: Option<String>,
        #[command(flatten)]
        sel: PaneSel,
        #[arg(long, value_name = "N", conflicts_with = "last_command")]
        line: Option<u64>,
        /// The line where its last command starts.
        #[arg(long)]
        last_command: bool,
    },
    /// Search a pane's screen and scrollback (exits 1 when nothing matches).
    Find {
        #[command(flatten)]
        sel: PaneSel,
        #[arg(long, short = 'c')]
        case_sensitive: bool,
        #[arg(long, short = 'w')]
        word: bool,
        #[arg(long, short = 'e')]
        regex: bool,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Also open the pane's find bar.
        #[arg(long)]
        show: bool,
        query: String,
    },
    /// Put a pane in vi, read-only or hints mode, or back to normal.
    Mode {
        #[command(flatten)]
        sel: PaneSel,
        #[arg(value_enum)]
        mode: Mode,
    },
    /// Run one of Motty's actions in a pane (--list names them).
    Action {
        #[command(flatten)]
        sel: PaneSel,
        #[arg(long, conflicts_with = "name")]
        list: bool,
        #[arg(required_unless_present = "list")]
        name: Option<String>,
    },
}

pub fn window(ctx: &Ctx, cmd: WindowCmd) -> anyhow::Result<ExitCode> {
    let mut client = ctx.connect()?;
    match cmd {
        WindowCmd::List => {
            let windows: Vec<WindowInfo> = client.call(method::LIST_WINDOWS, &Value::Null)?;
            print_windows(ctx, &windows);
        }
        WindowCmd::Show(sel) => {
            let windows: Vec<WindowInfo> = client.call(method::LIST_WINDOWS, &sel.target())?;
            let w = windows.first().context("no Motty window")?;
            ctx.print(w, || {
                show(&[
                    ("id", w.id.clone()),
                    ("index", w.index.to_string()),
                    ("title", w.title.clone()),
                    ("name", w.name.clone().unwrap_or_default()),
                    ("focused", w.focused.to_string()),
                    ("tabs", w.tabs.to_string()),
                    ("active tab", w.active_tab.clone().unwrap_or_default()),
                ])
            });
        }
        WindowCmd::New(new) => return new_window(ctx, &mut client, new),
        WindowCmd::Close { sel, force } => {
            let params = CloseParams {
                target: sel.target(),
                force: force || ctx.yes,
            };
            let _: Value = client.call(method::CLOSE_WINDOW, &params)?;
        }
        WindowCmd::Focus(sel) => {
            let _: Value = client.call(method::FOCUS_WINDOW, &sel.target())?;
        }
        WindowCmd::Rename { sel, name } => {
            let params = RenameParams {
                target: sel.target(),
                name,
            };
            let _: Value = client.call(method::RENAME_WINDOW, &params)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn new_window(
    ctx: &Ctx,
    client: &mut motty_ipc::Client,
    new: NewWindow,
) -> anyhow::Result<ExitCode> {
    let cwd = match new.cwd {
        Some(dir) => std::path::absolute(dir)?,
        None => std::env::current_dir()?,
    };
    let params = NewWindowParams {
        cwd: Some(cwd.display().to_string()),
        line: new.command,
        title: new.title,
    };
    let created: WindowCreated = client.call(method::NEW_WINDOW, &params)?;
    ctx.print(&created, || ctx.note(&created.window));
    Ok(ExitCode::SUCCESS)
}

pub fn print_windows(ctx: &Ctx, windows: &[WindowInfo]) {
    ctx.print(&windows, || {
        let rows: Vec<Vec<String>> = windows
            .iter()
            .map(|w| {
                vec![
                    format!("{}{}", if w.focused { "*" } else { " " }, w.id),
                    w.index.to_string(),
                    w.tabs.to_string(),
                    w.title.clone(),
                ]
            })
            .collect();
        ctx.table(&[" ID", "INDEX", "TABS", "TITLE"], &rows);
    });
}

pub fn tab(ctx: &Ctx, cmd: TabCmd) -> anyhow::Result<ExitCode> {
    let mut client = ctx.connect()?;
    match cmd {
        TabCmd::List(sel) => {
            let filter = PaneRef {
                window: sel.window,
                ..Default::default()
            };
            let tabs: Vec<TabInfo> = client.call(method::LIST_TABS, &filter)?;
            print_tabs(ctx, &tabs);
        }
        TabCmd::Show(sel) => {
            let tabs: Vec<TabInfo> = client.call(method::LIST_TABS, &sel.target())?;
            let t = tabs.first().context("no tab")?;
            ctx.print(t, || {
                let panes: Vec<String> = t.panes.iter().map(u64::to_string).collect();
                show(&[
                    ("id", t.id.clone()),
                    ("window", t.window_id.clone()),
                    ("index", t.index.to_string()),
                    ("title", t.title.clone()),
                    ("name", t.name.clone().unwrap_or_default()),
                    ("active", t.active.to_string()),
                    ("panes", panes.join(" ")),
                    ("badge", t.badge.clone().unwrap_or_default()),
                    ("zoomed", t.zoomed.to_string()),
                ])
            });
        }
        TabCmd::New {
            sel,
            cwd,
            command,
            title,
        } => {
            let cwd = cwd
                .map(std::path::absolute)
                .transpose()?
                .map(|p| p.display().to_string());
            // The window this runs in, unless one is named.
            let select_window = match sel.window {
                Some(w) => Some(w),
                None => own_window(&mut client)?,
            };
            let params = NewTabParams {
                cwd,
                select_window,
                line: command,
                title,
                ..Default::default()
            };
            let created: Created = client.call(method::NEW_TAB, &params)?;
            ctx.print(&created, || ctx.note(created.pane));
        }
        TabCmd::Close { sel, force } => {
            let params = CloseParams {
                target: sel.target(),
                force: force || ctx.yes,
            };
            let _: Value = client.call(method::CLOSE_TAB, &params)?;
        }
        TabCmd::Focus(sel) => {
            let _: Value = client.call(method::FOCUS_TAB, &sel.target())?;
        }
        TabCmd::Rename { sel, name } => {
            let params = RenameParams {
                target: sel.target(),
                name,
            };
            let _: Value = client.call(method::RENAME_TAB, &params)?;
        }
        TabCmd::Move {
            sel,
            index,
            to_window,
            new_window,
        } => {
            anyhow::ensure!(
                index.is_some() || to_window.is_some() || new_window,
                "say where: --index, --to-window or --new-window"
            );
            let params = MoveTabParams {
                target: sel.target(),
                index,
                to_window,
                new_window,
            };
            let _: Value = client.call(method::MOVE_TAB, &params)?;
        }
        TabCmd::Badge { sel, kind, clear } => {
            let kind = kind.filter(|_| !clear).map(|k| {
                k.to_possible_value()
                    .map(|v| v.get_name().to_owned())
                    .unwrap_or_default()
            });
            let params = TabBadgeParams {
                target: sel.target(),
                kind,
            };
            let _: Value = client.call(method::TAB_BADGE, &params)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The window id of the pane this runs in, if it's in Motty.
fn own_window(client: &mut motty_ipc::Client) -> anyhow::Result<Option<String>> {
    let Some(pane) = own_pane() else {
        return Ok(None);
    };
    let panes: Vec<PaneInfo> = client.call(method::LIST_PANES, &PaneRef::id(Some(pane)))?;
    Ok(panes.first().map(|p| p.window_id.clone()))
}

pub fn print_tabs(ctx: &Ctx, tabs: &[TabInfo]) {
    ctx.print(&tabs, || {
        let rows: Vec<Vec<String>> = tabs
            .iter()
            .map(|t| {
                vec![
                    format!("{}{}", if t.active { "*" } else { " " }, t.id),
                    t.window_id.clone(),
                    t.index.to_string(),
                    t.panes.len().to_string(),
                    t.badge.clone().unwrap_or_else(|| "-".into()),
                    t.title.clone(),
                ]
            })
            .collect();
        ctx.table(
            &[" ID", "WINDOW", "INDEX", "PANES", "BADGE", "TITLE"],
            &rows,
        );
    });
}

pub fn print_panes(ctx: &Ctx, panes: &[PaneInfo]) {
    ctx.print(&panes, || {
        let rows: Vec<Vec<String>> = panes
            .iter()
            .map(|p| {
                let state = match (&p.agent, &p.badge) {
                    (Some(a), Some(b)) => format!("{a}: {b}"),
                    (Some(a), None) => a.clone(),
                    (None, Some(b)) => b.clone(),
                    (None, None) => if p.busy { "busy" } else { "-" }.into(),
                };
                vec![
                    format!("{}{}", if p.focused { "*" } else { " " }, p.id),
                    p.window_id.clone(),
                    p.tab_id.clone(),
                    p.index.to_string(),
                    p.process.clone().unwrap_or_else(|| "-".into()),
                    state,
                    p.cwd.clone().unwrap_or_else(|| p.title.clone()),
                ]
            })
            .collect();
        ctx.table(
            &[" ID", "WINDOW", "TAB", "INDEX", "PROCESS", "STATE", "CWD"],
            &rows,
        );
    });
}

/// `key: value` lines.
fn show(fields: &[(&str, String)]) {
    for (key, value) in fields {
        println!("{key}: {value}");
    }
}

pub fn pane(ctx: &Ctx, cmd: PaneCmd) -> anyhow::Result<ExitCode> {
    let mut client = ctx.connect()?;
    match cmd {
        PaneCmd::List { sel } => {
            let filter = PaneSel {
                pane: None,
                tab: sel,
            }
            .filter();
            let panes: Vec<PaneInfo> = client.call(method::LIST_PANES, &filter)?;
            print_panes(ctx, &panes);
        }
        PaneCmd::Show(sel) => {
            let panes: Vec<PaneInfo> = client.call(method::LIST_PANES, &sel.target())?;
            let p = panes.first().context("no pane")?;
            ctx.print(p, || {
                show(&[
                    ("id", p.id.to_string()),
                    ("window", p.window_id.clone()),
                    ("tab", p.tab_id.clone()),
                    ("index", p.index.to_string()),
                    ("title", p.title.clone()),
                    ("cwd", p.cwd.clone().unwrap_or_default()),
                    ("process", p.process.clone().unwrap_or_default()),
                    ("busy", p.busy.to_string()),
                    ("focused", p.focused.to_string()),
                    ("size", format!("{}x{}", p.cols, p.rows)),
                    ("shell integration", p.shell_integration.to_string()),
                    ("agent", p.agent.clone().unwrap_or_default()),
                    ("session", p.session.clone().unwrap_or_default()),
                    ("badge", p.badge.clone().unwrap_or_default()),
                    ("status", p.status.clone().unwrap_or_default()),
                ])
            });
        }
        PaneCmd::New(split) | PaneCmd::Split(split) => {
            let cwd = split
                .cwd
                .map(std::path::absolute)
                .transpose()?
                .map(|p| p.display().to_string());
            let params = SplitParams {
                pane: split.sel.target(),
                direction: split.direction.ipc(),
                command: None,
                cwd,
                no_focus: split.no_focus,
                size: split.size,
                line: split.command,
            };
            let created: Created = client.call(method::SPLIT, &params)?;
            ctx.print(&created, || ctx.note(created.pane));
        }
        PaneCmd::Close { sel, force } => {
            let params = CloseParams {
                target: sel.target(),
                force: force || ctx.yes,
            };
            let _: Value = client.call(method::CLOSE_PANE, &params)?;
        }
        PaneCmd::Focus { target, sel, line } => {
            focus(&mut client, target, sel, line)?;
        }
        PaneCmd::Zoom { sel, on, off } => {
            let params = ZoomParams {
                target: sel.target(),
                zoom: (on || off).then_some(on),
            };
            let out: Value = client.call(method::ZOOM_PANE, &params)?;
            ctx.print(&out, || {});
        }
        PaneCmd::Resize {
            sel,
            left,
            right,
            up,
            down,
        } => {
            anyhow::ensure!(
                left | right | up | down != 0,
                "say how far: --left, --right, --up or --down N"
            );
            let params = ResizeParams {
                target: sel.target(),
                left,
                right,
                up,
                down,
            };
            let _: Value = client.call(method::RESIZE_PANE, &params)?;
        }
        PaneCmd::SendKeys { sel, keys } => {
            let text = keys::send_keys(&keys).map_err(anyhow::Error::msg)?;
            send(&mut client, sel.target(), text, None)?;
        }
        PaneCmd::SendText {
            sel,
            text,
            from_file,
            stdin,
            submit,
        } => {
            let text = match (text, from_file) {
                (Some(text), _) => keys::unescape(&text),
                (None, Some(path)) => {
                    std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?
                }
                (None, None) => {
                    debug_assert!(stdin);
                    let mut text = String::new();
                    std::io::stdin().read_to_string(&mut text)?;
                    text
                }
            };
            send(&mut client, sel.target(), text, submit.map(SubmitWhen::ipc))?;
        }
        PaneCmd::Run { sel, command } => {
            return run(ctx, sel.target(), command.join(" "));
        }
        PaneCmd::Exec { sel, command } => {
            return exec(ctx, &mut client, sel.target(), command.join(" "));
        }
        PaneCmd::Wait {
            pane,
            tab,
            window,
            session,
            timeout_secs,
        } => {
            let deadline = timeout_secs.map(|s| Instant::now() + Duration::from_secs_f64(s));
            let what = match (session, tab, window) {
                (Some(session), ..) => Waited::Session(session),
                (None, Some(tab), window) => {
                    // A bare --tab: the tab of the pane named, or this one.
                    let tab = if tab.is_empty() {
                        let of: Vec<PaneInfo> = client
                            .call(method::LIST_PANES, &target(pane.as_deref(), None, None))?;
                        of.first().context("no pane")?.tab_id.clone()
                    } else {
                        tab
                    };
                    Waited::Group(PaneRef {
                        tab: Some(tab),
                        window,
                        ..Default::default()
                    })
                }
                (None, None, Some(window)) => Waited::Group(PaneRef {
                    window: Some(window),
                    ..Default::default()
                }),
                (None, None, None) => Waited::Pane(target(pane.as_deref(), None, None)),
            };
            return wait(&mut ctx.connect_patient()?, what, deadline);
        }
        PaneCmd::Capture {
            sel,
            lines,
            scrollback,
            last_command,
        } => {
            let range = match (lines, scrollback, last_command) {
                (_, _, true) => TextRange::LastCommand,
                // As far back as a pane keeps.
                (_, true, _) => TextRange::Lines(1_000_000_000),
                (Some(n), ..) => TextRange::Lines(n),
                _ => TextRange::Screen,
            };
            let params = ReadParams {
                pane: sel.target(),
                range,
            };
            let out: Value = client.call(method::READ, &params)?;
            ctx.print(&out, || {
                println!("{}", out["text"].as_str().unwrap_or_default())
            });
        }
        PaneCmd::Link {
            target: positional,
            sel,
            line,
            last_command,
        } => {
            let mut target = sel.target();
            if let Some(pane) = positional {
                target = crate::output::target(Some(&pane), None, None);
            }
            let params = LinkParams {
                pane: target,
                last_command,
            };
            let info: LinkInfo = client.call(method::PANE_LINK, &params)?;
            let url = link::pane(info.pane, line.or(info.line));
            ctx.print(
                &json!({ "url": url, "pane": info.pane, "line": line.or(info.line) }),
                || println!("{url}"),
            );
        }
        PaneCmd::Find {
            sel,
            case_sensitive,
            word,
            regex,
            limit,
            show,
            query,
        } => {
            let params = FindParams {
                pane: sel.target(),
                query,
                case_sensitive,
                whole_word: word,
                regex,
                limit: Some(limit),
                show,
            };
            return find(ctx, &mut client, &params);
        }
        PaneCmd::Mode { sel, mode } => {
            let params = SetModeParams {
                pane: sel.target(),
                mode: mode.ipc(),
            };
            let _: Value = client.call(method::SET_MODE, &params)?;
        }
        PaneCmd::Action { list: true, .. } => {
            let names: Vec<String> = client.call(method::LIST_ACTIONS, &Value::Null)?;
            ctx.print(&names, || names.iter().for_each(|n| println!("{n}")));
        }
        PaneCmd::Action { sel, name, .. } => {
            let params = ActionParams {
                pane: sel.target(),
                name: name.unwrap_or_default(),
            };
            let _: Value = client.call(method::ACTION, &params)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn send(
    client: &mut motty_ipc::Client,
    pane: PaneRef,
    text: String,
    submit: Option<Submit>,
) -> anyhow::Result<()> {
    let _: Value = client.call(method::SEND_TEXT, &SendTextParams { pane, text, submit })?;
    Ok(())
}

/// `pane focus`: a selector or a motty:// link, maybe with a line.
fn focus(
    client: &mut motty_ipc::Client,
    positional: Option<String>,
    sel: PaneSel,
    line: Option<u64>,
) -> anyhow::Result<()> {
    let mut params = FocusParams {
        pane: sel.target(),
        line,
    };
    match positional {
        Some(url) if url.starts_with(link::SCHEME) => {
            match link::parse(&url).map_err(anyhow::Error::msg)? {
                Link::Window(w) => {
                    let _: Value =
                        client.call(method::FOCUS_WINDOW, &target(None, None, Some(&w)))?;
                    return Ok(());
                }
                Link::Tab(t) => {
                    let _: Value = client.call(method::FOCUS_TAB, &target(None, Some(&t), None))?;
                    return Ok(());
                }
                Link::Pane { pane, line: at } => {
                    params.pane = target(Some(&pane), None, None);
                    params.line = line.or(at);
                }
            }
        }
        Some(pane) => params.pane = target(Some(&pane), None, None),
        None => {}
    }
    let _: Value = client.call(method::FOCUS, &params)?;
    Ok(())
}

/// Shared with the hidden `motty find`.
pub fn find(
    ctx: &Ctx,
    client: &mut motty_ipc::Client,
    params: &FindParams,
) -> anyhow::Result<ExitCode> {
    let result: FindResult = client.call(method::FIND, params)?;
    ctx.print(&result, || {
        for m in &result.matches {
            println!("{}:{}: {}", m.line, m.column + 1, m.line_text);
        }
        if result.total > result.matches.len() {
            eprintln!("({} more)", result.total - result.matches.len());
        }
    });
    Ok(if result.total == 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// A command's exit status as this process's.
pub fn exit_code(exit: Option<i32>) -> ExitCode {
    ExitCode::from(exit.unwrap_or(0).clamp(0, 255) as u8)
}

/// `pane run`: how it went, not what it printed (Otty's shape).
fn run(ctx: &Ctx, pane: PaneRef, command: String) -> anyhow::Result<ExitCode> {
    let params = RunParams {
        pane,
        command: command.clone(),
        timeout_ms: None,
    };
    let result: RunResult = ctx.connect_patient()?.call(method::RUN, &params)?;
    let state = if result.exit.unwrap_or(0) == 0 {
        "succeed"
    } else {
        "error"
    };
    let report = json!({
        "state": state,
        "exit_code": result.exit,
        "command": command,
        "duration_ms": result.duration_ms,
    });
    ctx.print(&report, || match result.exit {
        Some(code) if code != 0 => ctx.note(format!("{state} (exit {code})")),
        _ => ctx.note(state),
    });
    Ok(exit_code(result.exit))
}

/// `pane exec`: as `run`, with stderr sent to a file so the two streams
/// come back apart (stderr is no tty for the command then).
fn exec(
    ctx: &Ctx,
    client: &mut motty_ipc::Client,
    pane: PaneRef,
    command: String,
) -> anyhow::Result<ExitCode> {
    let panes: Vec<PaneInfo> = client.call(method::LIST_PANES, &pane)?;
    let info = panes.first().context("no pane")?;
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let err_file = std::env::temp_dir().join(format!(
        "motty-exec-{}-{}.err",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let line = exec_line(
        &command,
        &err_file.display().to_string(),
        info.process.as_deref(),
    );
    let params = RunParams {
        pane: PaneRef::id(Some(info.id)),
        command: line,
        timeout_ms: None,
    };
    let result = ctx
        .connect_patient()?
        .call::<_, RunResult>(method::RUN, &params);
    let stderr = std::fs::read_to_string(&err_file).unwrap_or_default();
    let _ = std::fs::remove_file(&err_file);
    let result = result?;
    let report = json!({
        "state": if result.exit.unwrap_or(0) == 0 { "succeed" } else { "error" },
        "exit_code": result.exit,
        "command": command,
        "duration_ms": result.duration_ms,
        "stdout": result.output,
        "stderr": stderr,
        "stderr_is_tty": false,
    });
    ctx.print(&report, || {
        if !result.output.is_empty() {
            println!("{}", result.output);
        }
        eprint!("{stderr}");
    });
    Ok(exit_code(result.exit))
}

/// The line `exec` types: the command with its stderr in `file`, in the
/// pane's shell's syntax.
pub fn exec_line(command: &str, file: &str, shell: Option<&str>) -> String {
    let quoted = format!("'{}'", file.replace('\'', r"'\''"));
    if shell == Some("fish") {
        format!("begin; {command}; end 2>{quoted}")
    } else {
        format!("( {command} ) 2>{quoted}")
    }
}

/// What `pane wait` waits on.
pub enum Waited {
    /// One pane, by id or selector.
    Pane(PaneRef),
    /// Every pane of a tab or window but this one.
    Group(PaneRef),
    /// The pane running an agent session.
    Session(String),
}

/// Polls the panes until every one waited on is done (or gone). Exits 6
/// when a pane can't tell, 9 on timeout (Otty's statuses).
pub fn wait(
    client: &mut motty_ipc::Client,
    what: Waited,
    deadline: Option<Instant>,
) -> anyhow::Result<ExitCode> {
    let me = own_pane();
    let mut first = true;
    loop {
        let panes: Vec<PaneInfo> = match &what {
            Waited::Pane(target) | Waited::Group(target) => {
                match client.call(method::LIST_PANES, target) {
                    Ok(panes) => panes,
                    // It closed since: nothing left to wait for.
                    Err(_) if !first => return Ok(ExitCode::SUCCESS),
                    Err(e) => return Err(e),
                }
            }
            Waited::Session(session) => {
                let all: Vec<PaneInfo> = client.call(method::LIST_PANES, &Value::Null)?;
                let found: Vec<PaneInfo> = all
                    .into_iter()
                    .filter(|p| p.session.as_deref() == Some(session))
                    .collect();
                anyhow::ensure!(!(first && found.is_empty()), "no such session");
                found
            }
        };
        let waited: Vec<&PaneInfo> = panes
            .iter()
            .filter(|p| !matches!(what, Waited::Group(_)) || Some(p.id) != me)
            .collect();
        let mut busy = Vec::new();
        for p in &waited {
            match p.idle() {
                Some(true) => {}
                Some(false) => busy.push(p.id.to_string()),
                None => {
                    eprintln!(
                        "motty: pane {} can't report when it's done: it runs no coding agent \
                         Motty knows of and its shell has no shell integration",
                        p.id
                    );
                    return Ok(ExitCode::from(6));
                }
            }
        }
        if busy.is_empty() {
            return Ok(ExitCode::SUCCESS);
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            eprintln!("motty: timed out; still busy: pane {}", busy.join(", "));
            return Ok(ExitCode::from(9));
        }
        first = false;
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_lines_follow_the_shell() {
        assert_eq!(
            exec_line("cargo build", "/tmp/e.err", Some("zsh")),
            "( cargo build ) 2>'/tmp/e.err'"
        );
        assert_eq!(
            exec_line("make", "/tmp/it's.err", Some("fish")),
            "begin; make; end 2>'/tmp/it'\\''s.err'"
        );
    }
}
