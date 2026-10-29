//! The control socket: JSON-lines requests from the `motty` CLI (and the MCP
//! server), answered on the main thread. See `motty-ipc` for the protocol.
//!
//! Selectors (`--window`, `--tab`, `--pane`, `motty://` links) resolve in
//! one place, here: [`resolve_window`], [`resolve_tab`] and
//! [`resolve_pane`] take a selector as typed; [`focus_pane`] (with a line,
//! scrolled into view and lit), [`focus_tab`] and [`focus_window`] go
//! there.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use gpui::{App, AppContext as _, Entity, Focusable as _, Global, WindowHandle};
use motty_ipc::select::{PaneSel, TabSel, WindowSel, title_matches};
use motty_ipc::{
    ActionParams, AgentIntegration, AppInfo, CloseParams, Created, Direction as IpcDirection,
    FindParams, FindResult, FocusParams, FolderParams, FoundMatch, IpcError, JumpParams,
    JumpResult, KeybindInfo, LinkInfo, LinkParams, MoveTabParams, NewTabParams, NewWindowParams,
    OpenParams, PaneInfo, PaneRef, Place, ReadParams, RecipeApplyParams, RecipeInfo, RenameParams,
    Request, ResizeParams, Response, RunParams, RunResult, SendTextParams, SetModeParams,
    SplitParams, Submit, TabBadgeParams, TabInfo, TextRange, ViewParams, WindowCreated, WindowInfo,
    ZoomParams, method,
};
use motty_term::alacritty_terminal::grid::Dimensions;
use motty_term::alacritty_terminal::index::{Column, Point};
use motty_term::search::{self, SearchOptions};
use serde_json::{Value, json};

use crate::agents::Badge;
use crate::panes::Direction;
use crate::terminal_view::TerminalView;
use crate::workspace::Workspace;

static SOCKET: OnceLock<PathBuf> = OnceLock::new();

/// Where this app listens, once [`start`] has run.
pub fn socket() -> Option<&'static Path> {
    SOCKET.get().map(PathBuf::as_path)
}

type Reply = async_channel::Sender<Response>;

/// `run` requests waiting for their pane's command to finish.
#[derive(Default)]
struct Waiters(HashMap<u64, Vec<(u64, Reply)>>);

impl Global for Waiters {}

/// Listens on the default socket, or a per-process one when another Motty
/// already owns it.
pub fn start(cx: &mut App) {
    let listener = match bind() {
        Ok(listener) => listener,
        Err(e) => return tracing::warn!("control socket unavailable: {e:#}"),
    };
    let (tx, rx) = async_channel::unbounded::<(Request, Reply)>();
    std::thread::Builder::new()
        .name("ipc accept".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let tx = tx.clone();
                let _ = std::thread::Builder::new()
                    .name("ipc client".into())
                    .spawn(move || serve(stream, tx));
            }
        })
        .ok();
    cx.set_global(Waiters::default());
    cx.spawn(async move |cx| {
        while let Ok((request, reply)) = rx.recv().await {
            cx.update(|cx| dispatch(request, reply, cx));
        }
    })
    .detach();
}

fn bind() -> anyhow::Result<UnixListener> {
    let default = motty_ipc::default_socket_path();
    let path = if UnixStream::connect(&default).is_ok() {
        default.with_file_name(format!("motty-{}.sock", std::process::id()))
    } else {
        default
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // A socket file left by a Motty that didn't exit cleanly.
    let _ = std::fs::remove_file(&path);
    let listener =
        UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    let _ = SOCKET.set(path);
    Ok(listener)
}

/// One connection: a request per line, answered in order.
fn serve(stream: UnixStream, tx: async_channel::Sender<(Request, Reply)>) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                let (reply, answer) = async_channel::bounded(1);
                if tx.send_blocking((request, reply)).is_err() {
                    break;
                }
                match answer.recv_blocking() {
                    Ok(response) => response,
                    Err(_) => break,
                }
            }
            Err(e) => Response {
                id: 0,
                result: None,
                error: Some(format!("bad request: {e}")),
                code: None,
            },
        };
        let Ok(mut out) = serde_json::to_string(&response) else {
            break;
        };
        out.push('\n');
        if writer.write_all(out.as_bytes()).is_err() {
            break;
        }
    }
}

fn dispatch(request: Request, reply: Reply, cx: &mut App) {
    let id = request.id;
    let result = match request.method.as_str() {
        method::LIST_PANES => parse(request.params).and_then(|p| list_panes(&p, cx)),
        method::LIST_TABS => parse(request.params).and_then(|p| list_tabs(&p, cx)),
        method::LIST_WINDOWS => parse(request.params).and_then(|p| list_windows(&p, cx)),
        method::APP_INFO => Ok(json!(AppInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: motty_ipc::PROTOCOL_VERSION,
            pid: std::process::id(),
        })),
        method::READ => parse(request.params).and_then(|p| read(p, cx)),
        method::SEND_TEXT => parse(request.params).and_then(|p| send_text(p, cx)),
        method::SPLIT => parse(request.params).and_then(|p| split(p, cx)),
        method::NEW_TAB => parse(request.params).and_then(|p| new_tab(p, cx)),
        method::NEW_WINDOW => parse(request.params).and_then(|p| new_window(p, cx)),
        method::FOCUS => parse(request.params).and_then(|p| focus(p, cx)),
        method::FOCUS_TAB => parse(request.params).and_then(|p| {
            let (handle, ix) = target_tab(&p, cx)?;
            focus_tab(handle, ix, cx)
        }),
        method::FOCUS_WINDOW => parse(request.params).and_then(|p| {
            let handle = target_window(&p, cx)?;
            focus_window(handle, cx)
        }),
        method::CLOSE_PANE => parse(request.params).and_then(|p| close_pane(p, cx)),
        method::CLOSE_TAB => parse(request.params).and_then(|p| close_tab(p, cx)),
        method::CLOSE_WINDOW => parse(request.params).and_then(|p| close_window(p, cx)),
        method::RENAME_TAB => parse(request.params).and_then(|p| rename_tab(p, cx)),
        method::RENAME_WINDOW => parse(request.params).and_then(|p| rename_window(p, cx)),
        method::MOVE_TAB => parse(request.params).and_then(|p| move_tab(p, cx)),
        method::ZOOM_PANE => parse(request.params).and_then(|p| zoom_pane(p, cx)),
        method::RESIZE_PANE => parse(request.params).and_then(|p| resize_pane(p, cx)),
        method::TAB_BADGE => parse(request.params).and_then(|p| tab_badge(p, cx)),
        method::VIEW => parse(request.params).and_then(|p| view(p, cx)),
        method::PANE_LINK => parse(request.params).and_then(|p| pane_link(p, cx)),
        method::AGENT_INTEGRATION => parse(request.params).and_then(agent_integration),
        method::JUMP => parse(request.params).and_then(|p| jump(p, cx)),
        method::LEARN => parse(request.params).and_then(|p| learn(p, false, cx)),
        method::IGNORE => parse(request.params).and_then(|p| learn(p, true, cx)),
        method::KEYBIND_LIST => Ok(json!(keybind_list(cx))),
        method::RECIPE_LIST => Ok(json!(recipe_list())),
        method::RECIPE_APPLY => parse(request.params).and_then(|p| recipe_apply(p, cx)),
        method::CLOSE => parse(request.params).and_then(|p| close(p, cx)),
        method::FIND => parse(request.params).and_then(|p| find(p, cx)),
        method::QUIT => {
            // Answer first; quitting closes the socket.
            cx.defer(|cx| cx.quit());
            Ok(Value::Null)
        }
        method::SET_MODE => parse(request.params).and_then(|p| set_mode(p, cx)),
        method::ACTION => parse(request.params).and_then(|p| action(p, cx)),
        method::LIST_ACTIONS => Ok(json!(action_names(cx))),
        method::AGENT_STATE => parse(request.params).and_then(|p| agent_state(p, cx)),
        method::OPEN => parse(request.params).and_then(|p| open(p, cx)),
        method::CONFIG_RELOAD => {
            crate::workspace::reload_all(cx);
            Ok(Value::Null)
        }
        method::CONFIG_SET => parse(request.params).and_then(|p: motty_ipc::ConfigSetParams| {
            crate::set_transient(&p.lines)?;
            crate::workspace::reload_all(cx);
            Ok(Value::Null)
        }),
        method::RUN => match parse(request.params).and_then(|p| run(p, id, &reply, cx)) {
            // Answered when the command finishes.
            Ok(None) => return,
            Ok(Some(value)) => Ok(value),
            Err(e) => Err(e),
        },
        other => Err(anyhow!("unknown method `{other}`")),
    };
    let response = match result {
        Ok(result) => Response {
            id,
            result: Some(result),
            error: None,
            code: None,
        },
        Err(e) => Response {
            id,
            result: None,
            error: Some(format!("{e:#}")),
            code: e.downcast_ref::<IpcError>().and_then(|e| e.code),
        },
    };
    let _ = reply.try_send(response);
}

fn parse<T: motty_ipc::DeserializeOwned>(params: Value) -> anyhow::Result<T> {
    // Methods without required fields accept no params at all.
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).context("bad params")
}

/// Motty's windows, frontmost first.
fn workspaces(cx: &App) -> Vec<WindowHandle<Workspace>> {
    let windows = cx.window_stack().unwrap_or_else(|| cx.windows());
    windows
        .into_iter()
        .filter_map(|w| w.downcast::<Workspace>())
        .collect()
}

type Pane = (WindowHandle<Workspace>, Entity<TerminalView>);

fn front(cx: &App) -> anyhow::Result<WindowHandle<Workspace>> {
    workspaces(cx).into_iter().next().context("no Motty window")
}

/// A window by selector: `w_<id>`, a 1-based index (frontmost first),
/// `current`, `last` (the one used before it) or `title:<pattern>`.
pub fn resolve_window(selector: &str, cx: &App) -> anyhow::Result<WindowHandle<Workspace>> {
    let sel = WindowSel::parse(selector)?;
    let windows = workspaces(cx);
    let found = match &sel {
        WindowSel::Id(id) => windows
            .iter()
            .find(|h| h.read(cx).is_ok_and(|w| w.id() == *id)),
        WindowSel::Index(n) => windows.get(n - 1),
        WindowSel::Current => windows.first(),
        WindowSel::Last => windows.get(1),
        WindowSel::Title(pattern) => windows.iter().find(|h| {
            h.read(cx)
                .is_ok_and(|w| title_matches(pattern, &w.title(cx)))
        }),
    };
    found
        .copied()
        .ok_or_else(|| IpcError::no_match("window", selector).into())
}

/// A tab by selector: `t_<id>`, a 1-based index in the current window, or
/// `current`.
#[allow(dead_code)] // For `motty://tab/…` links.
pub fn resolve_tab(selector: &str, cx: &App) -> anyhow::Result<(WindowHandle<Workspace>, usize)> {
    resolve_tab_in(selector, None, cx)
}

/// [`resolve_tab`], with indexes and `current` in `window` when given.
fn resolve_tab_in(
    selector: &str,
    window: Option<WindowHandle<Workspace>>,
    cx: &App,
) -> anyhow::Result<(WindowHandle<Workspace>, usize)> {
    let sel = TabSel::parse(selector)?;
    let no_match = || IpcError::no_match("tab", selector);
    if let TabSel::Id(id) = sel {
        return workspaces(cx)
            .into_iter()
            .find_map(|h| {
                let ix = h.read(cx).ok()?.tab_ids().iter().position(|t| *t == id)?;
                Some((h, ix))
            })
            .ok_or_else(|| no_match().into());
    }
    let handle = match window {
        Some(handle) => handle,
        None => front(cx)?,
    };
    let workspace = handle.read(cx)?;
    let ix = match sel {
        TabSel::Index(n) => n - 1,
        _ => workspace.active_tab(),
    };
    if ix >= workspace.tab_count() {
        return Err(no_match().into());
    }
    Ok((handle, ix))
}

/// A pane by selector: its id (`$MOTTY_PANE_ID`), `p_<id>`, `index:<n>`
/// (in the current tab) or `current`.
#[allow(dead_code)] // For `motty://pane/…` links.
pub fn resolve_pane(selector: &str, cx: &App) -> anyhow::Result<Pane> {
    resolve(
        &PaneRef {
            select: Some(selector.to_owned()),
            ..Default::default()
        },
        cx,
    )
}

/// Goes to a pane, its tab and window; with `line` (a stable row, see
/// `motty_ipc::link`), scrolls it into view and lights it for a moment.
pub fn focus_pane(
    handle: WindowHandle<Workspace>,
    view: &Entity<TerminalView>,
    line: Option<u64>,
    cx: &mut App,
) -> anyhow::Result<Value> {
    handle.update(cx, |workspace, window, cx| {
        workspace.reveal_pane(view, window, cx);
        window.activate_window();
        if let Some(line) = line {
            view.update(cx, |v, cx| v.flash_row(line, cx));
        }
    })?;
    Ok(Value::Null)
}

pub fn focus_tab(
    handle: WindowHandle<Workspace>,
    ix: usize,
    cx: &mut App,
) -> anyhow::Result<Value> {
    handle.update(cx, |workspace, window, cx| {
        workspace.focus_tab(ix, window, cx)
    })?;
    Ok(Value::Null)
}

pub fn focus_window(handle: WindowHandle<Workspace>, cx: &mut App) -> anyhow::Result<Value> {
    handle.update(cx, |_, window, _| window.activate_window())?;
    Ok(Value::Null)
}

fn find_pane(id: u64, cx: &App) -> Option<Pane> {
    workspaces(cx).into_iter().find_map(|handle| {
        let view = handle.read(cx).ok()?.find_pane(id, cx)?;
        Some((handle, view))
    })
}

/// The pane a request names (see [`PaneRef`]): by id or selector, else the
/// focused pane of the tab or window named, else of the front window.
fn resolve(target: &PaneRef, cx: &App) -> anyhow::Result<Pane> {
    if let Some(id) = target.pane {
        return find_pane(id, cx).ok_or_else(|| IpcError::no_match("pane", id).into());
    }
    let sel = target.select.as_deref().map(PaneSel::parse).transpose()?;
    if let Some(PaneSel::Id(id)) = sel {
        let selector = target.select.as_deref().unwrap_or_default();
        return find_pane(id, cx).ok_or_else(|| IpcError::no_match("pane", selector).into());
    }
    // Indexes count in the tab named, else the window's active one.
    let scope = PaneRef {
        tab: target.tab.clone(),
        window: target.window.clone(),
        ..Default::default()
    };
    let (handle, ix) = target_tab(&scope, cx)?;
    let workspace = handle.read(cx)?;
    let view = match sel {
        Some(PaneSel::Index(n)) => {
            workspace.tab_panes(ix).get(n - 1).cloned().ok_or_else(|| {
                IpcError::no_match("pane", target.select.as_deref().unwrap_or_default())
            })?
        }
        _ => workspace
            .tab_focused_pane(ix)
            .context("the focused pane there isn't a terminal")?,
    };
    Ok((handle, view))
}

/// The tab a request names: by selector (indexes in the window named), else
/// the one holding the pane named, else the active tab of the window named
/// or the front one.
fn target_tab(target: &PaneRef, cx: &App) -> anyhow::Result<(WindowHandle<Workspace>, usize)> {
    let window = target
        .window
        .as_deref()
        .map(|w| resolve_window(w, cx))
        .transpose()?;
    if let Some(tab) = &target.tab {
        return resolve_tab_in(tab, window, cx);
    }
    if target.names_pane() {
        let (handle, view) = resolve(target, cx)?;
        let ix = handle
            .read(cx)?
            .tab_of_pane(&view)
            .context("that pane is in no tab")?;
        return Ok((handle, ix));
    }
    let handle = match window {
        Some(handle) => handle,
        None => front(cx)?,
    };
    let ix = handle.read(cx)?.active_tab();
    Ok((handle, ix))
}

/// The window a request names, else the one holding the tab or pane named,
/// else the front one.
fn target_window(target: &PaneRef, cx: &App) -> anyhow::Result<WindowHandle<Workspace>> {
    if let Some(window) = &target.window {
        return resolve_window(window, cx);
    }
    if target.tab.is_some() || target.names_pane() {
        return Ok(target_tab(target, cx)?.0);
    }
    front(cx)
}

fn window_id(handle: WindowHandle<Workspace>, cx: &App) -> String {
    handle
        .read(cx)
        .map(|w| format!("w_{}", w.id()))
        .unwrap_or_default()
}

/// Panes, all of them or those of the pane, tab or window named.
fn list_panes(target: &PaneRef, cx: &App) -> anyhow::Result<Value> {
    let panes = all_panes(cx);
    let keep: Option<Vec<u64>> = if target.names_pane() {
        Some(vec![resolve(target, cx)?.1.read(cx).id])
    } else if target.tab.is_some() {
        let (handle, ix) = target_tab(target, cx)?;
        Some(
            handle
                .read(cx)?
                .tab_panes(ix)
                .iter()
                .map(|v| v.read(cx).id)
                .collect(),
        )
    } else if target.window.is_some() {
        let id = window_id(target_window(target, cx)?, cx);
        Some(
            panes
                .iter()
                .filter(|p| p.window_id == id)
                .map(|p| p.id)
                .collect(),
        )
    } else {
        None
    };
    let panes: Vec<PaneInfo> = match keep {
        Some(keep) => panes.into_iter().filter(|p| keep.contains(&p.id)).collect(),
        None => panes,
    };
    Ok(json!(panes))
}

/// Tabs, all of them or those of the tab, pane or window named.
fn list_tabs(target: &PaneRef, cx: &App) -> anyhow::Result<Value> {
    let mut out: Vec<TabInfo> = Vec::new();
    if target.tab.is_some() || target.names_pane() {
        let (handle, ix) = target_tab(target, cx)?;
        let id = window_id(handle, cx);
        out.extend(handle.read(cx)?.tab_info(ix, &id, cx));
    } else {
        let windows = match &target.window {
            Some(_) => vec![target_window(target, cx)?],
            None => workspaces(cx),
        };
        for handle in windows {
            let id = window_id(handle, cx);
            let workspace = handle.read(cx)?;
            out.extend((0..workspace.tab_count()).filter_map(|ix| workspace.tab_info(ix, &id, cx)));
        }
    }
    Ok(json!(out))
}

fn list_windows(target: &PaneRef, cx: &App) -> anyhow::Result<Value> {
    let only = (!target.is_empty())
        .then(|| target_window(target, cx))
        .transpose()?;
    let mut out = Vec::new();
    for (ix, handle) in workspaces(cx).into_iter().enumerate() {
        if only.is_some_and(|o| o != handle) {
            continue;
        }
        let workspace = handle.read(cx)?;
        let active = workspace.active_tab();
        out.push(WindowInfo {
            id: format!("w_{}", workspace.id()),
            index: ix + 1,
            title: workspace.title(cx),
            name: workspace.name().map(str::to_owned),
            focused: ix == 0,
            tabs: workspace.tab_count(),
            active_tab: workspace.tab_ids().get(active).map(|id| format!("t_{id}")),
        });
    }
    Ok(json!(out))
}

fn all_panes(cx: &App) -> Vec<PaneInfo> {
    let mut out = Vec::new();
    for (window, handle) in workspaces(cx).into_iter().enumerate() {
        let Ok(workspace) = handle.read(cx) else {
            continue;
        };
        let window_id = format!("w_{}", workspace.id());
        let tab_ids = workspace.tab_ids();
        for (tab, view) in workspace.panes() {
            let v = view.read(cx);
            let session = v.session();
            let size = session.size();
            let index = workspace
                .tab_panes(tab)
                .iter()
                .position(|p| p == view)
                .map_or(0, |i| i + 1);
            out.push(PaneInfo {
                id: v.id,
                window,
                tab,
                window_id: window_id.clone(),
                tab_id: tab_ids
                    .get(tab)
                    .map(|t| format!("t_{t}"))
                    .unwrap_or_default(),
                index,
                title: v.display_title(),
                cwd: v.cwd.as_ref().map(|c| c.display().to_string()),
                process: v.process.clone(),
                busy: v.busy,
                focused: window == 0 && workspace.is_focused_pane(view),
                cols: size.cols,
                rows: size.rows,
                shell_integration: session.shell().integrated,
                agent: v.status.as_ref().and_then(|s| s.agent.clone()),
                agent_id: v.status.as_ref().and_then(|s| s.agent_id.clone()),
                badge: v
                    .status
                    .as_ref()
                    .and_then(|s| s.badge)
                    .map(|b| b.name().to_owned()),
                status: v.status.as_ref().and_then(|s| s.message.clone()),
                in_turn: v.status.as_ref().is_some_and(|s| s.in_turn),
                session: v.status.as_ref().and_then(|s| s.session.clone()),
                queued: v.pending(),
            });
        }
    }
    out
}

fn read(p: ReadParams, cx: &App) -> anyhow::Result<Value> {
    let (_, view) = resolve(&p.pane, cx)?;
    let session = view.read(cx).session();
    let text = match p.range {
        TextRange::Screen => session.screen_text(),
        TextRange::Lines(n) => session.last_lines(n),
        TextRange::LastCommand => {
            let block = session
                .shell()
                .last_finished()
                .cloned()
                .context("no finished command (shell integration off?)")?;
            session
                .block_output(&block)
                .context("that output has scrolled away")?
        }
    };
    Ok(json!({ "text": text }))
}

fn send_text(p: SendTextParams, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, view) = resolve(&p.pane, cx)?;
    let badges = handle.read(cx)?.badges();
    if !badges.ipc_send_keys {
        bail!(
            "typing into panes is off; turn on Settings › Advanced › IPC Allow Send Keys \
             (ipc-allow-send-keys = true)"
        );
    }
    let v = view.read(cx);
    anyhow::ensure!(!v.is_read_only(), "pane {} is in Read Only mode", v.id);
    let ssh = v.system.ssh.is_some();
    if !badges.ipc_sensitive && (ssh || v.system.sudo || v.session().reading_secret()) {
        bail!(
            "pane {} is {}; Motty doesn't type into it (ipc-allow-sensitive-sessions)",
            v.id,
            if ssh {
                "in an SSH session"
            } else if v.system.sudo {
                "in a sudo session"
            } else {
                "at a password prompt"
            }
        );
    }
    let text = p.text;
    handle.update(cx, |_, window, cx| {
        view.update(cx, |v, cx| match p.submit {
            None => v.write_input(&text, cx),
            Some(Submit::Now) => v.send_tracked(&text, window, cx),
            Some(Submit::Queue) => v.enqueue(vec![text], window, cx),
        })
    })?;
    Ok(Value::Null)
}

/// A URL goes to the browser, a folder gets a shell, a file an editor tab.
fn open(p: OpenParams, cx: &mut App) -> anyhow::Result<Value> {
    // Web pages open in a web pane, like Otty's built-in browser.
    if p.target.starts_with("http://") || p.target.starts_with("https://") {
        let handle = workspaces(cx)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no Motty window"))?;
        handle.update(cx, |workspace, window, cx| {
            workspace.open_web(Some(p.target.clone()), window, cx)
        })?;
        return Ok(Value::Null);
    }
    if p.target.contains("://") || p.target.starts_with("mailto:") {
        cx.open_url(&p.target);
        return Ok(Value::Null);
    }
    let path = PathBuf::from(&p.target);
    anyhow::ensure!(path.is_absolute(), "{} isn't an absolute path", p.target);
    let meta = std::fs::metadata(&path).with_context(|| format!("can't open {}", p.target))?;
    // Over a file pane there's no terminal to start from: the front window.
    let (handle, view) = match resolve(&p.pane, cx) {
        Ok((handle, view)) => (handle, Some(view)),
        Err(e) if p.pane.pane.is_none() => (workspaces(cx).into_iter().next().ok_or(e)?, None),
        Err(e) => return Err(e),
    };
    if meta.is_dir() {
        let place = match p.place {
            Place::Tab => crate::workspace::Place::Tab,
            Place::Window => crate::workspace::Place::Window,
            Place::Split(d) => crate::workspace::Place::Split(direction(d)),
        };
        let pane = handle.update(cx, |workspace, window, cx| {
            if let Some(view) = &view {
                workspace.reveal_pane(view, window, cx);
            }
            workspace.start_in("", Some(path), place, window, cx)
        })?;
        return Ok(match pane {
            Some(pane) => json!(Created {
                pane: pane.read(cx).id
            }),
            None => Value::Null,
        });
    }
    // Recipe files open their recipe, asking before their commands run.
    if path
        .extension()
        .is_some_and(|x| x == crate::recipes::EXTENSION || x == crate::recipes::OTTY_EXTENSION)
    {
        handle.update(cx, |workspace, window, cx| {
            window.activate_window();
            workspace.open_recipe_path(&path, window, cx);
        })?;
        return Ok(Value::Null);
    }
    // Theme files are offered for import, with a preview.
    if path.extension().is_some_and(|x| x == "ottytheme") {
        handle.update(cx, |workspace, window, cx| {
            window.activate_window();
            workspace.preview_import(&path, None, window, cx);
        })?;
        return Ok(Value::Null);
    }
    // Files always get an editor tab, wherever they were asked for.
    handle.update(cx, |workspace, window, cx| {
        workspace.open_editor(Some(path), window, cx);
        window.activate_window();
    })?;
    Ok(Value::Null)
}

fn direction(d: IpcDirection) -> Direction {
    match d {
        IpcDirection::Left => Direction::Left,
        IpcDirection::Right => Direction::Right,
        IpcDirection::Up => Direction::Up,
        IpcDirection::Down => Direction::Down,
    }
}

fn command_of(command: Option<Vec<String>>) -> anyhow::Result<Option<(String, Vec<String>)>> {
    match command {
        None => Ok(None),
        Some(mut argv) if !argv.is_empty() => {
            let program = argv.remove(0);
            Ok(Some((program, argv)))
        }
        Some(_) => bail!("empty command"),
    }
}

fn split(p: SplitParams, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, view) = resolve(&p.pane, cx)?;
    let direction = direction(p.direction);
    let command = command_of(p.command)?;
    let created = handle.update(cx, |workspace, window, cx| {
        workspace.reveal_pane(&view, window, cx);
        let new = workspace.split_pane(
            direction,
            command,
            p.cwd.map(PathBuf::from),
            !p.no_focus,
            window,
            cx,
        );
        if let (Ok(new), Some(size)) = (&new, p.size) {
            let share = f32::from(size.clamp(10, 90)) / 100.;
            workspace.set_split_share(new, direction, share, cx);
        }
        if p.no_focus {
            workspace.reveal_pane(&view, window, cx);
        }
        new
    })??;
    type_line(&created, p.line.as_deref(), cx);
    Ok(json!(Created {
        pane: created.read(cx).id
    }))
}

/// Types `line` at a new shell's prompt, as a recipe's commands are.
fn type_line(view: &Entity<TerminalView>, line: Option<&str>, cx: &mut App) {
    if let Some(line) = line.filter(|l| !l.trim().is_empty()) {
        let line = format!("{}\r", line.trim_end_matches(['\n', '\r']));
        view.update(cx, |v, cx| v.write_input(&line, cx));
    }
}

fn new_tab(p: NewTabParams, cx: &mut App) -> anyhow::Result<Value> {
    let handle = match (&p.select_window, p.window) {
        (Some(sel), _) => resolve_window(sel, cx)?,
        (None, index) => *workspaces(cx)
            .get(index.unwrap_or(0))
            .ok_or_else(|| IpcError::no_match("window", index.unwrap_or(0) + 1))?,
    };
    let command = command_of(p.command)?;
    let title = p.title.filter(|t| !t.is_empty());
    let created = handle.update(cx, |workspace, window, cx| {
        let view = if p.reopen {
            workspace.reopen_tab(None, window, cx)
        } else {
            workspace.open_tab_with(command, p.cwd.map(PathBuf::from), window, cx)
        };
        if title.is_some() {
            let ix = workspace.active_tab();
            workspace.rename_tab(ix, title, cx);
        }
        view
    })??;
    type_line(&created, p.line.as_deref(), cx);
    Ok(json!(Created {
        pane: created.read(cx).id
    }))
}

/// `motty open`: a new window, in `cwd`, maybe running a command.
fn new_window(p: NewWindowParams, cx: &mut App) -> anyhow::Result<Value> {
    let cwd = p.cwd.map(PathBuf::from);
    if let Some(dir) = &cwd {
        anyhow::ensure!(dir.is_dir(), "no such folder: {}", dir.display());
    }
    let handle = crate::open_window_for_recipe(crate::load_config(), cwd, cx)?;
    let title = p.title.filter(|t| !t.is_empty());
    let (id, view) = handle.update(cx, |workspace, window, cx| {
        if title.is_some() {
            workspace.set_name(title, cx);
        }
        window.activate_window();
        (workspace.id(), workspace.focused_pane())
    })?;
    if let Some(view) = &view {
        type_line(view, p.line.as_deref(), cx);
    }
    Ok(json!(WindowCreated {
        window: format!("w_{id}"),
        pane: view.map(|v| v.read(cx).id),
    }))
}

fn focus(p: FocusParams, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, view) = resolve(&p.pane, cx)?;
    focus_pane(handle, &view, p.line, cx)
}

fn close_pane(p: CloseParams, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, view) = resolve(&p.target, cx)?;
    handle.update(cx, |workspace, window, cx| {
        workspace.close_pane_from_cli(&view, p.force, window, cx)
    })??;
    Ok(Value::Null)
}

fn close_tab(p: CloseParams, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, ix) = target_tab(&p.target, cx)?;
    handle.update(cx, |workspace, window, cx| {
        workspace.close_tab_from_cli(ix, p.force, window, cx)
    })??;
    Ok(Value::Null)
}

fn close_window(p: CloseParams, cx: &mut App) -> anyhow::Result<Value> {
    let handle = target_window(&p.target, cx)?;
    handle.update(cx, |workspace, window, cx| {
        workspace.close_window_from_cli(p.force, window, cx)
    })??;
    Ok(Value::Null)
}

fn rename_tab(p: RenameParams, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, ix) = target_tab(&p.target, cx)?;
    let name = Some(p.name.trim().to_owned()).filter(|n| !n.is_empty());
    handle.update(cx, |workspace, _, cx| workspace.rename_tab(ix, name, cx))?;
    Ok(Value::Null)
}

fn rename_window(p: RenameParams, cx: &mut App) -> anyhow::Result<Value> {
    let handle = target_window(&p.target, cx)?;
    let name = Some(p.name.trim().to_owned()).filter(|n| !n.is_empty());
    handle.update(cx, |workspace, _, cx| workspace.set_name(name, cx))?;
    Ok(Value::Null)
}

/// A tab to another place in its window, to another window, or to a new
/// one.
fn move_tab(p: MoveTabParams, cx: &mut App) -> anyhow::Result<Value> {
    let (from, ix) = target_tab(&p.target, cx)?;
    let index = p.index.map(|n| n.saturating_sub(1));
    if p.new_window {
        from.update(cx, |workspace, window, cx| {
            anyhow::ensure!(workspace.tab_count() > 1, "it's the window's only tab");
            workspace.tab_to_new_window(ix, window, cx);
            Ok(())
        })??;
        return Ok(Value::Null);
    }
    let to = match &p.to_window {
        Some(sel) => resolve_window(sel, cx)?,
        None => from,
    };
    if to == from {
        let index = index.context("say where: --index, --window or --new-window")?;
        from.update(cx, |workspace, window, cx| {
            workspace.move_tab_within(ix, index, window, cx)
        })?;
        return Ok(Value::Null);
    }
    let moved = from.update(cx, |workspace, window, cx| {
        workspace.take_tab(ix, window, cx)
    })??;
    to.update(cx, |workspace, window, cx| {
        workspace.put_tab(moved, index, window, cx);
        window.activate_window();
    })?;
    Ok(Value::Null)
}

fn zoom_pane(p: ZoomParams, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, view) = resolve(&p.target, cx)?;
    let zoomed = handle.update(cx, |workspace, window, cx| {
        workspace.zoom_pane(&view, p.zoom, window, cx)
    })??;
    Ok(json!({ "zoomed": zoomed }))
}

fn resize_pane(p: ResizeParams, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, view) = resolve(&p.target, cx)?;
    handle.update(cx, |workspace, _, cx| {
        workspace.resize_pane(&view, [p.left, p.right, p.up, p.down], cx)
    })??;
    Ok(Value::Null)
}

/// Otty's badge kinds, as Motty's badges.
fn badge_kind(kind: &str) -> anyhow::Result<Badge> {
    Ok(match kind {
        "running" => Badge::Running,
        "completed" => Badge::Done,
        "finished" | "unread" => Badge::Finished,
        "error" => Badge::Error,
        "awaiting-input" | "awaiting" => Badge::Awaiting,
        other => bail!(
            "no badge kind {other:?} (there's {})",
            motty_ipc::BADGE_KINDS.join(", ")
        ),
    })
}

fn tab_badge(p: TabBadgeParams, cx: &mut App) -> anyhow::Result<Value> {
    let badge = p.kind.as_deref().map(badge_kind).transpose()?;
    let (handle, ix) = target_tab(&p.target, cx)?;
    handle.update(cx, |workspace, _, cx| {
        workspace.set_tab_badge(ix, badge, cx)
    })?;
    Ok(Value::Null)
}

/// `motty view` / `motty edit`: beside the pane named (or the focused one),
/// in a new tab of its window, or in a new window.
fn view(p: ViewParams, cx: &mut App) -> anyhow::Result<Value> {
    if p.place == Place::Window {
        let handle = crate::open_window_for_recipe(crate::load_config(), None, cx)?;
        handle.update(cx, |workspace, window, cx| {
            workspace.view_target(
                &p.target,
                p.edit,
                crate::workspace::Place::Tab,
                true,
                window,
                cx,
            )
        })??;
        return Ok(Value::Null);
    }
    let place = match p.place {
        Place::Split(d) => crate::workspace::Place::Split(direction(d)),
        _ => crate::workspace::Place::Tab,
    };
    // Over an editor tab there's no terminal to split from: the front
    // window's focused pane, whatever it is.
    let (handle, view) = match resolve(&p.pane, cx) {
        Ok((handle, view)) => (handle, Some(view)),
        Err(e) if p.pane.is_empty() => (front(cx).map_err(|_| e)?, None),
        Err(e) => return Err(e),
    };
    handle.update(cx, |workspace, window, cx| {
        if let Some(view) = &view {
            workspace.reveal_pane(view, window, cx);
        }
        workspace.view_target(&p.target, p.edit, place, false, window, cx)
    })??;
    Ok(Value::Null)
}

/// A pane's id and, for its last command, the row it starts on.
fn pane_link(p: LinkParams, cx: &App) -> anyhow::Result<Value> {
    let (_, view) = resolve(&p.pane, cx)?;
    let v = view.read(cx);
    let line = if p.last_command {
        let shell = v.session().shell();
        let block = shell
            .blocks
            .iter()
            .rev()
            .find(|b| b.started.is_some())
            .context("no command in that pane (shell integration off?)")?;
        Some(block.input.map_or(block.prompt_row, |(row, _)| row))
    } else {
        None
    };
    Ok(json!(LinkInfo { pane: v.id, line }))
}

fn agent_integration(p: Value) -> anyhow::Result<Value> {
    let agent = p["agent"].as_str().context("`agent` is required")?;
    let env = crate::agents::hooks::Env::current();
    Ok(json!(AgentIntegration {
        known: crate::agents::hooks::target(agent, &env).is_some(),
        installed: crate::agents::hooks::installed(agent, &env),
    }))
}

/// The pane's folder, as `cwd` falls back to.
fn pane_folder(target: &PaneRef, cx: &App) -> Option<PathBuf> {
    let (_, view) = resolve(target, cx).ok()?;
    view.read(cx).cwd.clone()
}

fn jump(p: JumpParams, cx: &mut App) -> anyhow::Result<Value> {
    let cwd = p
        .cwd
        .map(PathBuf::from)
        .or_else(|| pane_folder(&p.pane, cx));
    let path =
        crate::folders::jump(p.query.as_deref(), cwd.as_deref()).with_context(|| {
            match p.query.as_deref().filter(|q| !q.trim().is_empty()) {
                Some(q) => format!("no folder matches {q:?}"),
                None => "no jump to go back to".to_owned(),
            }
        })?;
    if p.cd {
        let (_, view) = resolve(&p.pane, cx)?;
        anyhow::ensure!(
            !view.read(cx).is_read_only(),
            "pane {} is in Read Only mode",
            view.read(cx).id
        );
        let line = format!(
            "cd {}\r",
            crate::terminal_view::shell_escape(&path.to_string_lossy())
        );
        view.update(cx, |v, cx| v.write_input(&line, cx));
    }
    Ok(json!(JumpResult {
        path: path.display().to_string()
    }))
}

/// `motty learn` (or with `ignore`, `motty ignore`) a folder.
fn learn(p: FolderParams, ignore: bool, cx: &mut App) -> anyhow::Result<Value> {
    let path = match p.path {
        Some(path) => PathBuf::from(path),
        None => pane_folder(&p.pane, cx).context("that pane's folder isn't known")?,
    };
    anyhow::ensure!(
        path.is_absolute(),
        "{} isn't an absolute path",
        path.display()
    );
    if ignore {
        crate::folders::ignore(&path);
    } else {
        anyhow::ensure!(path.is_dir(), "no such folder: {}", path.display());
        crate::folders::learn(&path);
    }
    Ok(json!({ "path": path.display().to_string() }))
}

fn keybind_list(cx: &App) -> Vec<KeybindInfo> {
    let config;
    let keybinds = match front(cx).ok().and_then(|h| h.read(cx).ok()) {
        Some(workspace) => workspace.keybinds(),
        None => {
            config = crate::load_config();
            &config.keybinds
        }
    };
    crate::actions::keybind_list(keybinds)
        .into_iter()
        .map(|(keys, action, source)| KeybindInfo {
            keys,
            action,
            source: source.to_owned(),
        })
        .collect()
}

fn recipe_list() -> Vec<RecipeInfo> {
    use crate::recipes::Scope;
    crate::recipes::list()
        .into_iter()
        .map(|s| RecipeInfo {
            commands: match s.recipe.recipe.scope {
                Scope::Snippet => 0,
                _ => s.recipe.all_commands().len(),
            },
            scope: s.recipe.recipe.scope.label().to_lowercase(),
            name: s.recipe.recipe.name,
            id: s.id,
            updated: s
                .updated
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        })
        .collect()
}

fn recipe_apply(p: RecipeApplyParams, cx: &mut App) -> anyhow::Result<Value> {
    use crate::recipes::Scope;
    anyhow::ensure!(
        !(p.new_tab && p.new_window),
        "--new-tab and --new-window don't go together"
    );
    let dest = match (p.new_tab, p.new_window) {
        (true, _) => Some(Scope::Tab),
        (_, true) => Some(Scope::Window),
        _ => None,
    };
    let cwd = p.cwd.map(PathBuf::from);
    if let Some(dir) = &cwd {
        anyhow::ensure!(dir.is_dir(), "no such folder: {}", dir.display());
    }
    let handle = front(cx)?;
    let id = handle.update(cx, |workspace, window, cx| {
        window.activate_window();
        workspace.apply_recipe_from_cli(&p.recipe, dest, cwd, window, cx)
    })??;
    Ok(json!({ "id": id }))
}

fn close(p: PaneRef, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, view) = resolve(&p, cx)?;
    handle.update(cx, |workspace, window, cx| {
        workspace.close_pane_entity(&view, window, cx)
    })?;
    Ok(Value::Null)
}

/// A coding agent's hook reported its state.
fn agent_state(p: motty_ipc::AgentStateParams, cx: &mut App) -> anyhow::Result<Value> {
    for handle in workspaces(cx) {
        let Ok(workspace) = handle.read(cx) else {
            continue;
        };
        let Some(view) = workspace.pane_for_agent(p.pane.pane, p.agent_pid, cx) else {
            continue;
        };
        handle.update(cx, |workspace, window, cx| {
            workspace.agent_state(&view, p, window, cx)
        })?;
        let working = workspaces(cx)
            .iter()
            .any(|h| h.read(cx).is_ok_and(|w| w.agent_working(cx)));
        crate::agents::keep_awake(working);
        return Ok(Value::Null);
    }
    bail!("no tab found for that agent")
}

fn set_mode(p: SetModeParams, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, view) = resolve(&p.pane, cx)?;
    handle.update(cx, |_, window, cx| {
        view.update(cx, |view, cx| view.set_mode(p.mode, window, cx))
    })?;
    Ok(Value::Null)
}

/// Action names as scripts use them: `motty::SplitRight` is `split_right`.
fn action_names(cx: &App) -> Vec<String> {
    let mut names: Vec<String> = cx
        .all_action_names()
        .iter()
        .filter(|name| cx.build_action(name, None).is_ok())
        .filter_map(|name| name.strip_prefix("motty::"))
        .map(|name| {
            let mut out = String::new();
            for (i, c) in name.chars().enumerate() {
                if c.is_uppercase() && i > 0 {
                    out.push('_');
                }
                out.extend(c.to_lowercase());
            }
            out
        })
        .collect();
    names.sort();
    names
}

/// Runs an action in a pane, as if its key were pressed there; a pane in
/// a hidden tab is brought forward first.
fn action(p: ActionParams, cx: &mut App) -> anyhow::Result<Value> {
    let pascal: String = p
        .name
        .split(['_', '-'])
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect();
    let action = cx
        .build_action(&format!("motty::{pascal}"), None)
        .map_err(|_| anyhow::anyhow!("no action {} (see `motty action --list`)", p.name))?;
    // Over an editor tab, the action goes to whatever has focus.
    if p.pane.pane.is_none()
        && let Some(handle) = workspaces(cx).into_iter().next()
        && handle.read(cx).is_ok_and(|w| w.focused_pane().is_none())
    {
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_action(action, cx)
        })?;
        return Ok(Value::Null);
    }
    let (handle, view) = resolve(&p.pane, cx)?;
    let focus = view.read(cx).focus_handle(cx);
    let shown = handle.update(cx, |workspace, window, cx| {
        let shown = workspace.is_pane_shown(&view);
        if !shown {
            workspace.reveal_pane(&view, window, cx);
        }
        shown
    })?;
    let window = handle.into();
    if shown {
        cx.update_window(window, |_, window, cx| {
            focus.dispatch_action(&*action, window, cx)
        })?;
    } else {
        // It's in the window's dispatch tree once a frame has drawn it.
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(50))
                .await;
            let _ = cx.update_window(window, |_, window, cx| {
                focus.dispatch_action(&*action, window, cx)
            });
        })
        .detach();
    }
    Ok(Value::Null)
}

fn find(p: FindParams, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, view) = resolve(&p.pane, cx)?;
    let options = SearchOptions {
        case_sensitive: p.case_sensitive,
        whole_word: p.whole_word,
        regex: p.regex,
    };
    if p.show {
        handle.update(cx, |workspace, window, cx| {
            workspace.reveal_pane(&view, window, cx);
            view.update(cx, |view, cx| view.show_find(&p.query, options, window, cx));
        })?;
    }
    let term = view.read(cx).session().term.lock();
    let all = search::find_all(&*term, &p.query, options, 10_000)
        .map_err(|e| anyhow!("invalid pattern: {e}"))?;
    let matches = all
        .iter()
        .take(p.limit.unwrap_or(100))
        .filter_map(|m| {
            let (start, end) = m.to_points(&*term)?;
            let line_end = Point::new(start.line, term.grid().last_column());
            Some(FoundMatch {
                line: start.line.0,
                column: start.column.0,
                text: term.bounds_to_string(start, end),
                line_text: term
                    .bounds_to_string(Point::new(start.line, Column(0)), line_end)
                    .trim_end()
                    .to_owned(),
            })
        })
        .collect();
    Ok(json!(FindResult {
        total: all.len(),
        matches,
    }))
}

/// Types `command` into the pane's shell and answers once the shell reports
/// it finished (OSC 133 D). Returns `None` when the answer comes later.
fn run(p: RunParams, id: u64, reply: &Reply, cx: &mut App) -> anyhow::Result<Option<Value>> {
    let (_, view) = resolve(&p.pane, cx)?;
    let (pane, integrated, busy) = {
        let v = view.read(cx);
        // One at a time: `is_busy` takes the shell state's lock too.
        let integrated = v.session().shell().integrated;
        (v.id, integrated, v.session().is_busy())
    };
    if !integrated {
        bail!("pane {pane} has no shell integration, so Motty can't tell when a command ends");
    }
    if busy {
        bail!("pane {pane} is busy running another command");
    }
    cx.global_mut::<Waiters>()
        .0
        .entry(pane)
        .or_default()
        .push((id, reply.clone()));
    if let Some(ms) = p.timeout_ms {
        let reply = reply.clone();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(ms))
                .await;
            cx.update(|cx| {
                let waiters = cx.global_mut::<Waiters>();
                if let Some(list) = waiters.0.get_mut(&pane) {
                    list.retain(|(waiting, _)| *waiting != id);
                }
            });
            let _ = reply.try_send(Response {
                id,
                result: None,
                error: Some(format!("timed out after {ms} ms")),
                code: None,
            });
        })
        .detach();
    }
    let line = format!("{}\r", p.command.trim_end_matches(['\n', '\r']));
    view.update(cx, |v, cx| v.write_input(&line, cx));
    Ok(None)
}

/// Answers `run` requests waiting on this pane.
pub fn command_finished(
    view: &Entity<TerminalView>,
    exit: Option<i32>,
    duration: Option<Duration>,
    cx: &mut App,
) {
    let pane = view.read(cx).id;
    if cx
        .try_global::<Waiters>()
        .and_then(|w| w.0.get(&pane))
        .is_none_or(Vec::is_empty)
    {
        return;
    }
    let session = view.read(cx).session();
    let output = session
        .shell()
        .last_finished()
        .cloned()
        .and_then(|b| session.block_output(&b))
        .unwrap_or_default();
    let result = RunResult {
        exit,
        output,
        duration_ms: duration.map(|d| d.as_millis() as u64),
    };
    for (id, reply) in cx
        .global_mut::<Waiters>()
        .0
        .remove(&pane)
        .unwrap_or_default()
    {
        let _ = reply.try_send(Response {
            id,
            result: Some(json!(result)),
            error: None,
            code: None,
        });
    }
}
