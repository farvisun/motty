//! `motty://` links, as Otty's `otty://`: `window/<sel>`, `tab/<sel>` and
//! `pane/<id>[?line=<n>]` only move focus and never ask; `run/<command>`
//! opens a tab with the command typed at the prompt and never presses
//! Return, and asks first when the link was clicked in a pane.

use std::time::Duration;

use gpui::{App, Entity, PromptLevel, Window, WindowHandle};
use motty_term::alacritty_terminal::term::TermMode;

use crate::terminal_view::TerminalView;
use crate::workspace::Workspace;

pub use motty_ipc::link::SCHEME;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Window(String),
    Tab(String),
    Pane { pane: String, line: Option<u64> },
    Run { command: String, new_window: bool },
}

/// Reads a `motty://` URL; `None` for anything else.
pub fn parse(url: &str) -> Option<Link> {
    let rest = url.strip_prefix(SCHEME)?;
    // Everything after an unencoded `#` is a fragment, as in any URL.
    let rest = rest.split('#').next().unwrap_or_default();
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let param = |name: &str| {
        query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| crate::percent_decode(v))
    };
    let (kind, selector) = path.split_once('/').unwrap_or((path, ""));
    let selector = crate::percent_decode(selector.trim_end_matches('/'));
    Some(match kind {
        "window" => Link::Window(selector),
        "tab" => Link::Tab(selector),
        "pane" => Link::Pane {
            pane: selector,
            // A non-numeric or negative line is dropped, not clamped.
            line: param("line").and_then(|l| l.parse().ok()),
        },
        "run" => Link::Run {
            command: selector,
            new_window: param("where").as_deref() == Some("window"),
        },
        _ => return None,
    })
}

/// Why a run link's command is refused: characters that could make the
/// staged line run, or read differently than it looks. Newlines are
/// checked later, against the shell (see [`stage`]).
pub fn refuse(command: &str) -> Option<&'static str> {
    if command.trim().is_empty() {
        return Some("The link has no command.");
    }
    command.chars().find_map(|c| match c {
        '\r' => Some("The command holds a carriage return."),
        '\x1b' => Some("The command holds an escape character."),
        '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {
            Some("The command holds a text-direction override.")
        }
        '\n' => None,
        c if c.is_control() => Some("The command holds a control character."),
        _ => None,
    })
}

/// A link from the system (`open motty://…`, a browser, Raycast).
pub fn open(url: &str, cx: &mut App) {
    match parse(url) {
        Some(Link::Run {
            command,
            new_window,
        }) => run(command, new_window, cx),
        Some(link) => navigate(link, cx),
        None => {
            front_workspace(cx).map(|w| w.update(cx, |_, window, _| window.activate_window()));
        }
    }
}

/// A link clicked in a pane: terminal output isn't a trusted source, so a
/// run link always shows its command and asks, and the answer isn't kept.
pub fn open_clicked(url: &str, window: &mut Window, cx: &mut App) {
    let Some(Link::Run {
        command,
        new_window,
    }) = parse(url)
    else {
        return open(url, cx);
    };
    let answer = window.prompt(
        PromptLevel::Warning,
        "Open a tab with this command typed at the prompt?",
        Some(&command),
        &["Open", "Cancel"],
        cx,
    );
    cx.spawn(async move |cx| {
        if answer.await == Ok(0) {
            cx.update(|cx| run(command, new_window, cx));
        }
    })
    .detach();
}

/// Focuses what the link names; an unknown or stale one just brings Motty
/// to the front.
fn navigate(link: Link, cx: &mut App) {
    use crate::ipc;
    let done = match link {
        Link::Window(sel) => ipc::resolve_window(&sel, cx).and_then(|w| ipc::focus_window(w, cx)),
        Link::Tab(sel) => ipc::resolve_tab(&sel, cx).and_then(|(w, ix)| ipc::focus_tab(w, ix, cx)),
        Link::Pane { pane, line } => {
            ipc::resolve_pane(&pane, cx).and_then(|(w, view)| ipc::focus_pane(w, &view, line, cx))
        }
        Link::Run { .. } => return,
    };
    if done.is_err()
        && let Some(w) = front_workspace(cx)
    {
        let _ = w.update(cx, |_, window, _| window.activate_window());
    }
}

/// Opens a tab (or window) and types `command` at its prompt, once the
/// shell is at one. A multi-line command goes in only as a bracketed
/// paste, which the shell keeps in its line editor instead of running.
fn run(command: String, new_window: bool, cx: &mut App) {
    if let Some(reason) = refuse(&command) {
        return alert(reason.to_owned(), cx);
    }
    let opened = if new_window {
        crate::open_window_for_recipe(crate::load_config(), None, cx)
            .and_then(|w| w.update(cx, |ws, _, _| ws.focused_pane()).map(|p| (w, p)))
            .map_err(|e| e.to_string())
    } else {
        match front_workspace(cx) {
            Some(w) => w
                .update(cx, |ws, window, cx| {
                    window.activate_window();
                    ws.open_tab_with(None, None, window, cx).map(Some)
                })
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
                .map(|p| (w, p)),
            None => crate::open_window_for_recipe(crate::load_config(), None, cx)
                .and_then(|w| w.update(cx, |ws, _, _| ws.focused_pane()).map(|p| (w, p)))
                .map_err(|e| e.to_string()),
        }
    };
    let (handle, Some(view)) = (match opened {
        Ok(opened) => opened,
        Err(e) => return tracing::error!("run link: {e}"),
    }) else {
        return;
    };
    cx.spawn(async move |cx| {
        stage(handle, view, command, cx).await;
    })
    .detach();
}

async fn stage(
    handle: WindowHandle<Workspace>,
    view: Entity<TerminalView>,
    command: String,
    cx: &mut gpui::AsyncApp,
) {
    // At a prompt when shell integration says so, or when the line editor
    // turns on bracketed paste; give up waiting after a few seconds.
    for _ in 0..50 {
        let ready = cx.update(|cx| {
            let session = view.read(cx).session();
            session.shell().integrated
                || session
                    .term
                    .lock()
                    .mode()
                    .contains(TermMode::BRACKETED_PASTE)
        });
        if ready {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    let _ = handle.update(cx, |_, window, cx| {
        let bracketed = view.read(cx).session().term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if command.contains('\n') && !bracketed {
            let reason = "This shell doesn't take bracketed paste, so a multi-line command could run its first lines.";
            return alert(reason.to_owned(), cx);
        }
        view.update(cx, |v, _| v.paste_text(&command, false, window));
    });
}

fn alert(reason: String, cx: &mut App) {
    tracing::warn!("run link refused: {reason}");
    if let Some(w) = front_workspace(cx) {
        let _ = w.update(cx, |_, window, cx| {
            drop(window.prompt(
                PromptLevel::Warning,
                "Motty didn't open the run link",
                Some(&reason),
                &["OK"],
                cx,
            ));
        });
    }
}

fn front_workspace(cx: &App) -> Option<WindowHandle<Workspace>> {
    cx.window_stack()
        .into_iter()
        .flatten()
        .chain(cx.windows())
        .find_map(|w| w.downcast::<Workspace>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_navigation_links() {
        assert_eq!(
            parse("motty://window/current"),
            Some(Link::Window("current".into()))
        );
        assert_eq!(parse("motty://tab/2"), Some(Link::Tab("2".into())));
        assert_eq!(
            parse("motty://pane/7?line=1204"),
            Some(Link::Pane {
                pane: "7".into(),
                line: Some(1204)
            })
        );
        // A bad line is dropped, leaving a plain pane link.
        for bad in ["x", "-3", ""] {
            assert_eq!(
                parse(&format!("motty://pane/7?line={bad}")),
                Some(Link::Pane {
                    pane: "7".into(),
                    line: None
                })
            );
        }
        assert_eq!(parse("motty://elsewhere/1"), None);
        assert_eq!(parse("https://motty.dev"), None);
    }

    #[test]
    fn reads_run_links() {
        assert_eq!(
            parse("motty://run/brew%20update"),
            Some(Link::Run {
                command: "brew update".into(),
                new_window: false
            })
        );
        assert_eq!(
            parse("motty://run/echo%20%23%20hi%3F?where=window"),
            Some(Link::Run {
                command: "echo # hi?".into(),
                new_window: true
            })
        );
        // Unencoded, `#` ends the command.
        assert_eq!(
            parse("motty://run/echo #hi"),
            Some(Link::Run {
                command: "echo ".into(),
                new_window: false
            })
        );
    }

    #[test]
    fn refuses_commands_that_could_run_or_mislead() {
        assert_eq!(refuse("make test"), None);
        assert_eq!(refuse("echo a\necho b"), None);
        assert!(refuse("ls\r").is_some());
        assert!(refuse("\x1b[201~rm -rf ~").is_some());
        assert!(refuse("echo \u{202e}txt.exe").is_some());
        assert!(refuse("echo \u{4}").is_some());
        assert!(refuse("  ").is_some());
    }
}
