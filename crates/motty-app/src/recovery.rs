//! Session recovery: windows, tabs, splits and each pane's folder are saved
//! every few seconds, and with scrollback on quit. The next launch reopens
//! them (`on-launch = restore_session`, Otty's default); after a crash the
//! last snapshot comes back whatever `on-launch` says.
//!
//! Only the primary instance (the one on the default socket) started
//! without `-e`, `--bounds` and the like takes part, so test and scripted
//! runs never touch the user's session.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::{App, WindowHandle};
use serde::{Deserialize, Serialize};

use crate::workspace::Workspace;

const SAVE_INTERVAL: Duration = Duration::from_secs(5);
/// Lines of scrollback kept per pane.
const SCROLLBACK_LINES: usize = 10_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    /// Written on quit; a snapshot that isn't clean comes from a crash.
    pub clean: bool,
    /// Back to front, so restoring in order leaves the front one on top.
    pub windows: Vec<WindowState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    /// x, y, width, height in points.
    pub bounds: [f32; 4],
    pub sidebar_visible: bool,
    #[serde(default)]
    pub details_visible: bool,
    /// The tabs panel's Group and Order (`project`, `date`; `updated`,
    /// `manual`); empty for the defaults.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub grouping: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub order: String,
    pub active: usize,
    pub tabs: Vec<TabState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabState {
    pub root: PaneState,
    /// Index of the focused pane among the leaves, left to right.
    pub focused: usize,
    pub zoomed: bool,
    /// A divider follows the tab in the sidebar.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub divider: bool,
    /// The divider's label (double-click it to edit).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub divider_label: String,
    /// The name given with Rename Tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The name comes before the title instead of replacing it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub name_prefix: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PaneState {
    Leaf {
        cwd: Option<PathBuf>,
        /// File in the scrollback folder (saved on quit only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scrollback: Option<String>,
        /// The agent running there and its session: `[agent, session]`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<[String; 2]>,
        /// Prompts still queued.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        queue: Vec<String>,
    },
    Split {
        /// Children stacked top and bottom (else side by side).
        vertical: bool,
        ratio: f32,
        first: Box<PaneState>,
        second: Box<PaneState>,
    },
}

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Quit and Keep Windows: the next launch reopens them even with session
/// restore off.
static KEEP: AtomicBool = AtomicBool::new(false);

pub fn keep_windows() {
    KEEP.store(true, Ordering::Relaxed);
}

fn dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Motty")
}

fn state_path() -> PathBuf {
    dir().join("session.json")
}

pub fn scrollback_dir() -> PathBuf {
    dir().join("scrollback")
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The snapshot to reopen now, if any.
pub fn to_restore(restore_session: bool) -> Option<SessionState> {
    if !enabled() {
        return None;
    }
    let text = std::fs::read_to_string(state_path()).ok()?;
    let state: SessionState = serde_json::from_str(&text)
        .inspect_err(|e| tracing::warn!("ignoring the saved session: {e}"))
        .ok()?;
    (!state.windows.is_empty() && (restore_session || !state.clean)).then_some(state)
}

/// Decides whether this instance saves the session, and starts saving.
pub fn start(cx: &mut App) {
    const SCRIPTED: [&str; 6] = [
        "-e",
        "--bounds",
        "--split",
        "--extra-tabs",
        "--background",
        "--theme",
    ];
    let scripted = std::env::args().any(|a| SCRIPTED.contains(&a.as_str()));
    let primary = crate::ipc::socket() == Some(motty_ipc::default_socket_path().as_path());
    if scripted || !primary {
        return;
    }
    ENABLED.store(true, Ordering::Relaxed);
    cx.spawn(async move |cx| {
        let mut last = None;
        loop {
            cx.background_executor().timer(SAVE_INTERVAL).await;
            let state = cx.update(|cx| capture(cx, false));
            if last.as_ref() != Some(&state) {
                write(&state);
                last = Some(state);
            }
        }
    })
    .detach();
    cx.on_app_quit(|cx| {
        let mut state = capture(cx, true);
        // Saved as if it hadn't quit cleanly, so it comes back.
        state.clean &= !KEEP.load(Ordering::Relaxed);
        write(&state);
        async {}
    })
    .detach();
}

fn capture(cx: &mut App, scrollback: bool) -> SessionState {
    let scrollback_dir = scrollback.then(|| {
        let dir = scrollback_dir();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    });
    let mut files = 0;
    let handles: Vec<WindowHandle<Workspace>> = cx
        .window_stack()
        .unwrap_or_else(|| cx.windows())
        .into_iter()
        .filter_map(|w| w.downcast::<Workspace>())
        .collect();
    let mut windows: Vec<WindowState> = handles
        .into_iter()
        .filter_map(|handle| {
            handle
                .update(cx, |workspace, window, cx| {
                    workspace.snapshot(window, scrollback_dir.as_deref(), &mut files, cx)
                })
                .ok()
        })
        .filter(|w| !w.tabs.is_empty())
        .collect();
    // The stack lists the front window first.
    windows.reverse();
    SessionState {
        clean: scrollback,
        windows,
    }
}

fn write(state: &SessionState) {
    let path = state_path();
    let result = std::fs::create_dir_all(dir()).and_then(|_| {
        let json = serde_json::to_vec_pretty(state).map_err(std::io::Error::other)?;
        // Written whole then renamed, so a crash mid-write keeps the last one.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)
    });
    if let Err(e) = result {
        tracing::warn!("couldn't save the session: {e}");
    }
}

/// Saved scrollback for a leaf, read once.
pub fn read_scrollback(name: &str) -> Option<Vec<u8>> {
    // Names are ours, but never follow one out of the folder.
    if name.contains('/') || name.contains("..") {
        return None;
    }
    std::fs::read(scrollback_dir().join(name)).ok()
}

/// How many lines of scrollback a pane saves.
pub fn scrollback_lines() -> usize {
    SCROLLBACK_LINES
}
