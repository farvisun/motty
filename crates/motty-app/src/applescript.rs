//! AppleScript, with Terminal.app's dictionary (as Otty has it): `do
//! script`, and windows whose tabs have `contents`, `history`, `busy`,
//! `processes`, `tty`, `custom title` and so on, so Terminal scripts work
//! with the application name changed. The dictionary is
//! `resources/Motty.sdef`, which Cocoa Scripting reads from the bundle; the
//! keys it names are implemented in [`mac`].
//!
//! A scripting tab is a Motty tab, but Motty tabs can be split: a tab's
//! properties read its focused pane (its first terminal while a file or web
//! page has focus), and `do script … in` a tab types into that pane. Tabs
//! without a terminal (search results, files alone) aren't scripting tabs.
//!
//! As in Otty, `do script` needs no setting, but SSH and sudo sessions are
//! protected: scripts can't type into them and read their text as empty,
//! unless `ipc-allow-sensitive-sessions`.

// Elsewhere than macOS nothing calls the model.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

#[cfg(target_os = "macos")]
mod mac;

use gpui::{App, WindowHandle};

use crate::terminal_view::TerminalView;
use crate::workspace::{ScriptTab, Workspace};

/// Registers the scripting classes; before the app runs.
pub fn register() {
    #[cfg(target_os = "macos")]
    mac::register();
}

/// Lets scripts reach the app, once it runs.
pub fn install(cx: &mut App) {
    #[cfg(target_os = "macos")]
    mac::install(cx);
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

/// `errAENoSuchObject`.
const NO_SUCH_OBJECT: isize = -1728;
/// `errAEEventFailed`.
const EVENT_FAILED: isize = -10000;
/// `errAEPrivilegeError`.
const PRIVILEGE: isize = -10004;

/// Why a script's request failed: an Apple Event error number and message.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScriptError {
    pub number: isize,
    pub message: String,
}

impl ScriptError {
    fn new(number: isize, message: impl Into<String>) -> Self {
        Self {
            number,
            message: message.into(),
        }
    }

    fn no_tab() -> Self {
        Self::new(NO_SUCH_OBJECT, "That tab no longer exists.")
    }

    fn no_window() -> Self {
        Self::new(NO_SUCH_OBJECT, "That window no longer exists.")
    }

    /// An Apple Event came in while Motty was busy (from a nested run loop:
    /// a menu or a modal panel open).
    fn busy() -> Self {
        Self::new(EVENT_FAILED, "Motty is busy; try again.")
    }
}

pub(crate) type Result<T> = std::result::Result<T, ScriptError>;

/// What makes a pane sensitive to scripts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sensitive {
    pub ssh: bool,
    pub sudo: bool,
    /// Echo is off for a password.
    pub password_prompt: bool,
}

impl Sensitive {
    pub(crate) fn of(view: &TerminalView) -> Self {
        let session = view.session();
        // Read now rather than at the last process poll: a script reads
        // right after it types `ssh`.
        let system = session.system_state();
        Self {
            ssh: system.ssh.is_some(),
            sudo: system.sudo,
            password_prompt: session.reading_secret(),
        }
    }

    /// `contents` and `history` read as empty in SSH and sudo sessions,
    /// unless `ipc-allow-sensitive-sessions`.
    pub(crate) fn hides_text(self, allow_sensitive: bool) -> bool {
        !allow_sensitive && (self.ssh || self.sudo)
    }

    /// Why `do script` won't type here: an SSH or sudo session, or a
    /// password prompt (which `motty send` refuses too), unless
    /// `ipc-allow-sensitive-sessions`.
    pub(crate) fn refuses_typing(self, allow_sensitive: bool) -> Option<&'static str> {
        if allow_sensitive {
            None
        } else if self.ssh {
            Some("in an SSH session")
        } else if self.sudo {
            Some("in a sudo session")
        } else if self.password_prompt {
            Some("at a password prompt")
        } else {
            None
        }
    }
}

/// What `do script` types: the command, its lines and the last ended with
/// Return, as Terminal runs it.
pub(crate) fn typed(command: &str) -> String {
    let mut out = command
        .trim_end_matches(['\n', '\r'])
        .replace("\r\n", "\r")
        .replace('\n', "\r");
    out.push('\r');
    out
}

/// Motty's windows, frontmost first.
pub(crate) fn windows(cx: &App) -> Vec<WindowHandle<Workspace>> {
    let mut out: Vec<WindowHandle<Workspace>> = cx
        .window_stack()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|w| w.downcast::<Workspace>())
        .collect();
    // The stack can leave out windows that are off screen.
    for window in cx.windows() {
        if let Some(w) = window.downcast::<Workspace>()
            && !out.contains(&w)
        {
            out.push(w);
        }
    }
    out
}

/// The tab holding pane `pane`, and its window.
pub(crate) fn find_tab(pane: u64, cx: &App) -> Result<(WindowHandle<Workspace>, ScriptTab)> {
    windows(cx)
        .into_iter()
        .find_map(|handle| {
            let tab = handle.read(cx).ok()?.script_tab_holding(pane, cx)?;
            Some((handle, tab))
        })
        .ok_or_else(ScriptError::no_tab)
}

/// The tabs of a window, by the pane each is known by.
pub(crate) fn tabs(handle: WindowHandle<Workspace>, cx: &App) -> Result<Vec<u64>> {
    let workspace = handle.read(cx).map_err(|_| ScriptError::no_window())?;
    Ok(workspace
        .script_tabs()
        .iter()
        .map(|t| t.pane.read(cx).id)
        .collect())
}

/// Whether `ipc-allow-sensitive-sessions` is on for the window.
fn allow_sensitive(handle: WindowHandle<Workspace>, cx: &App) -> bool {
    handle
        .read(cx)
        .is_ok_and(|workspace| workspace.badges().ipc_sensitive)
}

/// A tab's `contents` (or its `history`, with `scrollback`).
pub(crate) fn text(pane: u64, scrollback: bool, cx: &App) -> Result<String> {
    let (handle, tab) = find_tab(pane, cx)?;
    let view = tab.pane.read(cx);
    if Sensitive::of(view).hides_text(allow_sensitive(handle, cx)) {
        return Ok(String::new());
    }
    let session = view.session();
    Ok(if scrollback {
        session.last_lines(usize::MAX)
    } else {
        session.screen_text()
    })
}

/// Where `do script` runs its command.
pub(crate) enum Target {
    /// A new window, as Terminal does without `in`.
    NewWindow,
    /// The window's selected tab.
    Window(WindowHandle<Workspace>),
    /// The tab holding this pane.
    Tab(u64),
}

/// Types `command` and Return into the target tab's pane (or a new
/// window's); returns the pane it went to.
pub(crate) fn do_script(command: &str, target: Target, cx: &mut App) -> Result<u64> {
    let (handle, tab) = match target {
        Target::NewWindow => {
            let handle = crate::open_window_inner(crate::load_config(), None, None, None, None, cx)
                .map_err(|e| ScriptError::new(EVENT_FAILED, format!("{e:#}")))?;
            let tab = handle
                .read(cx)
                .ok()
                .and_then(Workspace::script_selected_tab)
                .ok_or_else(|| ScriptError::new(EVENT_FAILED, "The new window has no shell."))?;
            (handle, tab)
        }
        Target::Window(handle) => {
            let workspace = handle.read(cx).map_err(|_| ScriptError::no_window())?;
            let tab = workspace.script_selected_tab().ok_or_else(|| {
                ScriptError::new(EVENT_FAILED, "The window's selected tab has no terminal.")
            })?;
            (handle, tab)
        }
        Target::Tab(pane) => find_tab(pane, cx)?,
    };
    let view = tab.pane.read(cx);
    let id = view.id;
    if command.is_empty() {
        return Ok(id);
    }
    if view.is_read_only() {
        return Err(ScriptError::new(
            EVENT_FAILED,
            format!("Pane {id} is in Read Only mode."),
        ));
    }
    if let Some(why) = Sensitive::of(view).refuses_typing(allow_sensitive(handle, cx)) {
        return Err(ScriptError::new(
            PRIVILEGE,
            format!(
                "Pane {id} is {why}; Motty doesn't run scripts there \
                 (ipc-allow-sensitive-sessions)."
            ),
        ));
    }
    let line = typed(command);
    tab.pane.update(cx, |view, cx| view.write_input(&line, cx));
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_commands_end_with_one_return() {
        assert_eq!(typed("ls"), "ls\r");
        assert_eq!(typed("ls\n"), "ls\r");
        assert_eq!(typed("ls\r\n\n"), "ls\r");
        assert_eq!(typed("cd /tmp\nls"), "cd /tmp\rls\r");
        assert_eq!(typed("a\r\nb"), "a\rb\r");
    }

    #[test]
    fn sensitive_sessions() {
        let plain = Sensitive::default();
        let ssh = Sensitive { ssh: true, ..plain };
        let sudo = Sensitive {
            sudo: true,
            ..plain
        };
        let password = Sensitive {
            password_prompt: true,
            ..plain
        };
        assert!(!plain.hides_text(false));
        assert!(ssh.hides_text(false));
        assert!(sudo.hides_text(false));
        // A password prompt shows nothing secret.
        assert!(!password.hides_text(false));
        assert_eq!(plain.refuses_typing(false), None);
        assert_eq!(ssh.refuses_typing(false), Some("in an SSH session"));
        assert_eq!(sudo.refuses_typing(false), Some("in a sudo session"));
        assert_eq!(password.refuses_typing(false), Some("at a password prompt"));
        // `ipc-allow-sensitive-sessions` lifts both.
        for s in [ssh, sudo, password] {
            assert!(!s.hides_text(true));
            assert_eq!(s.refuses_typing(true), None);
        }
    }
}
