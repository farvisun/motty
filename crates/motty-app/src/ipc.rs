//! The control socket: JSON-lines requests from the `motty` CLI (and the MCP
//! server), answered on the main thread. See `motty-ipc` for the protocol.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use gpui::{App, AppContext as _, Entity, Focusable as _, Global, WindowHandle};
use motty_ipc::{
    ActionParams, Created, Direction as IpcDirection, FindParams, FindResult, FoundMatch,
    NewTabParams, PaneInfo, PaneRef, ReadParams, Request, Response, RunParams, RunResult,
    SendTextParams, SetModeParams, SplitParams, TextRange, method,
};
use motty_term::alacritty_terminal::grid::Dimensions;
use motty_term::alacritty_terminal::index::{Column, Point};
use motty_term::search::{self, SearchOptions};
use serde_json::{Value, json};

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
        method::LIST_PANES => Ok(json!(list_panes(cx))),
        method::READ => parse(request.params).and_then(|p| read(p, cx)),
        method::SEND_TEXT => parse(request.params).and_then(|p| send_text(p, cx)),
        method::SPLIT => parse(request.params).and_then(|p| split(p, cx)),
        method::NEW_TAB => parse(request.params).and_then(|p| new_tab(p, cx)),
        method::FOCUS => parse(request.params).and_then(|p| focus(p, cx)),
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
        },
        Err(e) => Response {
            id,
            result: None,
            error: Some(format!("{e:#}")),
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

/// The pane `pane` names, or the focused pane of the frontmost window.
fn resolve(
    pane: &PaneRef,
    cx: &App,
) -> anyhow::Result<(WindowHandle<Workspace>, Entity<TerminalView>)> {
    for handle in workspaces(cx) {
        let Ok(workspace) = handle.read(cx) else {
            continue;
        };
        let found = match pane.pane {
            Some(id) => workspace.find_pane(id, cx),
            None => workspace.focused_pane(),
        };
        if let Some(view) = found {
            return Ok((handle, view));
        }
    }
    match pane.pane {
        Some(id) => bail!("no pane {id}"),
        None => bail!("no Motty window"),
    }
}

fn list_panes(cx: &App) -> Vec<PaneInfo> {
    let mut out = Vec::new();
    for (window, handle) in workspaces(cx).into_iter().enumerate() {
        let Ok(workspace) = handle.read(cx) else {
            continue;
        };
        for (tab, view) in workspace.panes() {
            let v = view.read(cx);
            let session = v.session();
            let size = session.size();
            out.push(PaneInfo {
                id: v.id,
                window,
                tab,
                title: v.display_title(),
                cwd: v.cwd.as_ref().map(|c| c.display().to_string()),
                process: v.process.clone(),
                busy: v.busy,
                focused: window == 0 && workspace.is_focused_pane(view),
                cols: size.cols,
                rows: size.rows,
                shell_integration: session.shell().integrated,
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
    let (_, view) = resolve(&p.pane, cx)?;
    view.update(cx, |v, cx| v.write_input(&p.text, cx));
    Ok(Value::Null)
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
    let direction = match p.direction {
        IpcDirection::Left => Direction::Left,
        IpcDirection::Right => Direction::Right,
        IpcDirection::Up => Direction::Up,
        IpcDirection::Down => Direction::Down,
    };
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
        if p.no_focus {
            workspace.reveal_pane(&view, window, cx);
        }
        new
    })??;
    Ok(json!(Created {
        pane: created.read(cx).id
    }))
}

fn new_tab(p: NewTabParams, cx: &mut App) -> anyhow::Result<Value> {
    let windows = workspaces(cx);
    let handle = *windows
        .get(p.window.unwrap_or(0))
        .context("no such window")?;
    let command = command_of(p.command)?;
    let created = handle.update(cx, |workspace, window, cx| {
        if p.reopen {
            workspace.reopen_tab(None, window, cx)
        } else {
            workspace.open_tab_with(command, p.cwd.map(PathBuf::from), window, cx)
        }
    })??;
    Ok(json!(Created {
        pane: created.read(cx).id
    }))
}

fn focus(p: PaneRef, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, view) = resolve(&p, cx)?;
    handle.update(cx, |workspace, window, cx| {
        workspace.reveal_pane(&view, window, cx);
        window.activate_window();
    })?;
    Ok(Value::Null)
}

fn close(p: PaneRef, cx: &mut App) -> anyhow::Result<Value> {
    let (handle, view) = resolve(&p, cx)?;
    handle.update(cx, |workspace, window, cx| {
        workspace.close_pane_entity(&view, window, cx)
    })?;
    Ok(Value::Null)
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
        });
    }
}
