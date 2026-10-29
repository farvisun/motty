//! `motty open` (a new window, as Otty's; files and URLs as before), and
//! `motty view` / `motty edit` (a file or a page as a pane).

use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Args, ValueEnum};
use motty_ipc::{Client, Created, OpenParams, PaneRef, Place, ViewParams, method};
use serde_json::Value;

use crate::output::{Ctx, own_pane};
use crate::tree::{Dir, NewWindow};

#[derive(Args)]
pub struct Open {
    /// A folder for the new window (default: here). A file or a URL opens
    /// in an editor tab or the browser, as `motty open` always has.
    pub path: Option<String>,
    /// A command line to run in the new window.
    #[arg(long)]
    pub command: Option<String>,
    /// The window's name.
    #[arg(long)]
    pub title: Option<String>,
    // What `open` took before it made windows; still honored.
    #[arg(long, hide = true)]
    pub pane: Option<u64>,
    #[arg(long, hide = true, value_enum, conflicts_with_all = ["tab", "window"])]
    pub split: Option<Dir>,
    #[arg(long, hide = true, conflicts_with = "window")]
    pub tab: bool,
    #[arg(long, hide = true)]
    pub window: bool,
}

#[derive(Args)]
pub struct View {
    /// A file, or an http(s) URL.
    pub target: String,
    /// Read-only (view) or editable (edit), whichever command this is.
    #[arg(long, value_enum)]
    pub mode: Option<ViewMode>,
    /// In a new tab (the default).
    #[arg(long, conflicts_with = "new_window")]
    pub new_tab: bool,
    #[arg(long)]
    pub new_window: bool,
    /// Split the pane: the new one on the left.
    #[arg(long, group = "side", conflicts_with_all = ["new_tab", "new_window"])]
    pub left: bool,
    #[arg(long, group = "side", conflicts_with_all = ["new_tab", "new_window"])]
    pub right: bool,
    #[arg(long, group = "side", conflicts_with_all = ["new_tab", "new_window"])]
    pub top: bool,
    #[arg(long, group = "side", conflicts_with_all = ["new_tab", "new_window"])]
    pub bottom: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ViewMode {
    View,
    Edit,
}

fn is_url(target: &str) -> bool {
    target.contains("://") || target.starts_with("mailto:")
}

/// A connection, starting Motty first when `launch` and it isn't running.
/// Returns whether it was started.
fn connect(ctx: &Ctx, launch: bool) -> anyhow::Result<(Client, bool)> {
    match ctx.connect() {
        Ok(client) => Ok((client, false)),
        Err(e) if !launch || ctx.socket.is_some() => Err(e),
        Err(e) => {
            let started = std::process::Command::new("/usr/bin/open")
                .args(["-g", "-b", "dev.motty.Motty"])
                .status();
            if !started.is_ok_and(|s| s.success()) {
                return Err(e);
            }
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                std::thread::sleep(Duration::from_millis(200));
                if let Ok(client) = ctx.connect() {
                    return Ok((client, true));
                }
                if Instant::now() >= deadline {
                    return Err(e.context("started Motty, but it didn't answer"));
                }
            }
        }
    }
}

pub fn open(ctx: &Ctx, o: Open) -> anyhow::Result<ExitCode> {
    let legacy = o.pane.is_some() || o.split.is_some() || o.tab || o.window;
    let file_or_url = o
        .path
        .as_deref()
        .is_some_and(|p| is_url(p) || std::path::Path::new(p).is_file());
    if legacy || file_or_url {
        let target = o.path.clone().unwrap_or_else(|| ".".into());
        return open_target(ctx, target, o);
    }
    let (mut client, started) = connect(ctx, true)?;
    // A Motty just started has its own window already.
    if started && o.path.is_none() && o.command.is_none() && o.title.is_none() {
        return Ok(ExitCode::SUCCESS);
    }
    let new = NewWindow {
        cwd: o.path,
        command: o.command,
        title: o.title,
    };
    crate::tree::new_window(ctx, &mut client, new)
}

/// What `motty open <path-or-url>` did before it opened windows: a file in
/// an editor tab, a folder as a shell, a URL in the browser.
fn open_target(ctx: &Ctx, target: String, o: Open) -> anyhow::Result<ExitCode> {
    let target = if is_url(&target) {
        target
    } else {
        let path = std::path::absolute(&target)?;
        anyhow::ensure!(path.exists(), "no such file or folder: {target}");
        path.display().to_string()
    };
    let place = match (o.split, o.window) {
        (Some(dir), _) => Place::Split(dir.ipc()),
        (None, true) => Place::Window,
        (None, false) => Place::Tab,
    };
    let (mut client, _) = connect(ctx, true)?;
    let params = OpenParams {
        pane: PaneRef::id(o.pane.or_else(own_pane)),
        target,
        place,
    };
    let created: Option<Created> = client.call(method::OPEN, &params)?;
    if let Some(created) = created {
        ctx.note(created.pane);
    }
    Ok(ExitCode::SUCCESS)
}

/// `motty view` (`edit` false) and `motty edit`.
pub fn view(ctx: &Ctx, v: View, edit: bool) -> anyhow::Result<ExitCode> {
    let target = if v.target.starts_with("http://") || v.target.starts_with("https://") {
        v.target
    } else {
        anyhow::ensure!(
            !is_url(&v.target),
            "only files and http(s) URLs open as a pane"
        );
        let path = std::path::absolute(&v.target)?;
        anyhow::ensure!(path.is_file(), "no such file: {}", v.target);
        path.display().to_string()
    };
    let side = [
        (v.left, Dir::Left),
        (v.right, Dir::Right),
        (v.top, Dir::Up),
        (v.bottom, Dir::Down),
    ]
    .into_iter()
    .find_map(|(on, dir)| on.then_some(dir));
    let place = match (side, v.new_window) {
        (Some(dir), _) => Place::Split(dir.ipc()),
        (None, true) => Place::Window,
        (None, false) => Place::Tab,
    };
    let params = ViewParams {
        pane: PaneRef::id(own_pane()),
        target,
        edit: v.mode.map_or(edit, |m| m == ViewMode::Edit),
        place,
    };
    let _: Value = ctx.connect()?.call(method::VIEW, &params)?;
    Ok(ExitCode::SUCCESS)
}
