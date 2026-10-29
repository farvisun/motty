//! Motty's control protocol, shared by the app (server), the `motty` CLI
//! and the MCP server.
//!
//! Each line on the socket is one JSON [`Request`]; the app answers each
//! with one [`Response`] line carrying the same `id`. Shells started by
//! Motty find the socket in `$MOTTY_SOCKET` and their own pane in
//! `$MOTTY_PANE_ID`.
//!
//! Ids last as long as the app runs and never move: a pane's is a number
//! (`$MOTTY_PANE_ID`, also written `p_<n>`), a tab's `t_<n>` and a
//! window's `w_<n>`, each counted from 1 in the order they were made. A
//! tab keeps its id when it moves to another window. Selectors (ids,
//! indexes, `current`, ...) are in [`select`]; `motty://` links in
//! [`link`].

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
pub use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod link;
pub mod select;
pub mod skills;

pub const SOCKET_ENV: &str = "MOTTY_SOCKET";
pub const PANE_ENV: &str = "MOTTY_PANE_ID";
/// Bumped when a method changes in a way an older client can't follow.
pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The exit status the CLI should use for `error`, e.g. [`NO_MATCH`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
}

/// A selector named no pane, tab or window (Otty's exit status 4).
pub const NO_MATCH: i32 = 4;

/// An error the app answered with, and the exit status it asks for.
#[derive(Debug)]
pub struct IpcError {
    pub message: String,
    pub code: Option<i32>,
}

impl IpcError {
    /// `No pane matched selector`, as Otty words it.
    pub fn no_match(kind: &str, selector: impl std::fmt::Display) -> Self {
        Self {
            message: format!("No {kind} matched selector {selector}"),
            code: Some(NO_MATCH),
        }
    }
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for IpcError {}

/// Methods, with their parameter and result types below.
pub mod method {
    pub const LIST_PANES: &str = "list-panes";
    pub const READ: &str = "read";
    pub const SEND_TEXT: &str = "send-text";
    pub const SPLIT: &str = "split";
    pub const NEW_TAB: &str = "new-tab";
    pub const RUN: &str = "run";
    pub const FOCUS: &str = "focus";
    pub const CLOSE: &str = "close";
    pub const FIND: &str = "find";
    pub const SET_MODE: &str = "set-mode";
    pub const QUIT: &str = "quit";
    pub const ACTION: &str = "action";
    pub const LIST_ACTIONS: &str = "list-actions";
    pub const AGENT_STATE: &str = "agent-state";
    pub const OPEN: &str = "open";
    /// Re-read the config files now.
    pub const CONFIG_RELOAD: &str = "config-reload";
    /// Apply config lines to the running app only ([`super::ConfigSetParams`]).
    pub const CONFIG_SET: &str = "config-set";
    pub const APP_INFO: &str = "app-info";
    pub const LIST_WINDOWS: &str = "list-windows";
    pub const LIST_TABS: &str = "list-tabs";
    pub const NEW_WINDOW: &str = "new-window";
    pub const CLOSE_WINDOW: &str = "close-window";
    pub const CLOSE_TAB: &str = "close-tab";
    pub const CLOSE_PANE: &str = "close-pane";
    pub const FOCUS_WINDOW: &str = "focus-window";
    pub const FOCUS_TAB: &str = "focus-tab";
    pub const RENAME_WINDOW: &str = "rename-window";
    pub const RENAME_TAB: &str = "rename-tab";
    pub const MOVE_TAB: &str = "move-tab";
    pub const ZOOM_PANE: &str = "zoom-pane";
    pub const RESIZE_PANE: &str = "resize-pane";
    pub const TAB_BADGE: &str = "tab-badge";
    pub const VIEW: &str = "view";
    pub const PANE_LINK: &str = "pane-link";
    pub const AGENT_INTEGRATION: &str = "agent-integration";
    pub const JUMP: &str = "jump";
    pub const LEARN: &str = "learn";
    pub const IGNORE: &str = "ignore";
    pub const KEYBIND_LIST: &str = "keybind-list";
    pub const RECIPE_LIST: &str = "recipe-list";
    pub const RECIPE_APPLY: &str = "recipe-apply";
}

/// `motty config set --transient`: config.toml lines that take the place
/// of the files' values for the same keys until Motty quits.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigSetParams {
    pub lines: String,
}

/// What a coding agent is doing, as its hooks report it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentState {
    /// A turn started.
    Processing,
    /// Waiting for the user's approval or answer.
    Awaiting,
    /// The turn finished.
    Idle,
    /// The turn failed.
    Error,
    /// The session ended; the agent is gone.
    Ended,
}

impl AgentState {
    /// Otty's spellings (`state=processing`), plus a few aliases.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "processing" | "working" | "running" | "busy" => Self::Processing,
            "awaiting" | "awaiting-input" | "blocked" | "waiting" => Self::Awaiting,
            "idle" | "done" | "finished" | "complete" => Self::Idle,
            "error" | "failed" => Self::Error,
            "ended" | "exit" | "exited" => Self::Ended,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentStateParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    /// The agent's id: `claude`, `codex`, or a custom one.
    pub agent: String,
    pub state: AgentState,
    /// The agent's process; finds the pane when `pane` isn't known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Where the agent logs the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// Display name; defaults to the agent's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// What it's doing or needs, in a few words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: u64,
    /// Index of the window among Motty's windows, frontmost first (from 0).
    pub window: usize,
    /// Index of the tab in its window (from 0).
    pub tab: usize,
    /// The window's id, `w_<n>`.
    #[serde(default)]
    pub window_id: String,
    /// The tab's id, `t_<n>`.
    #[serde(default)]
    pub tab_id: String,
    /// Its place among the tab's panes, in reading order (from 1).
    #[serde(default)]
    pub index: usize,
    pub title: String,
    pub cwd: Option<String>,
    pub process: Option<String>,
    /// A command is running.
    pub busy: bool,
    /// The focused pane of the focused tab of its window.
    pub focused: bool,
    pub cols: u16,
    pub rows: u16,
    /// The shell reports prompts and commands (OSC 133).
    pub shell_integration: bool,
    /// The coding agent that reported from this pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Its id, as `motty state:<agent>` and `watch:<agent>` name it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The tab badge: running, awaiting, done, finished or error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    /// What the agent is doing or needs, or how the last command went.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The agent is in the middle of a turn.
    #[serde(default)]
    pub in_turn: bool,
    /// The agent's session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Prompts waiting in the pane's queue, plus one just sent that the
    /// pane hasn't finished with.
    #[serde(default)]
    pub queued: usize,
}

impl PaneInfo {
    /// Done with its work: an agent between turns, else a shell at its
    /// prompt, with nothing left in the queue. `None` when the pane can't
    /// tell (no agent and no shell integration).
    pub fn idle(&self) -> Option<bool> {
        let quiet = self.queued == 0;
        if self.agent.is_some() {
            Some(!self.in_turn && quiet)
        } else if self.shell_integration {
            Some(!self.busy && quiet)
        } else {
            None
        }
    }
}

/// Which pane a request is about: by id, else by selector, else the
/// focused pane of the tab or window named, else of the front window.
/// Requests about a tab or a window take the one holding the pane named.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PaneRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
    /// A pane selector (see [`select`]), when `pane` isn't given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<String>,
    /// A tab selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    /// A window selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
}

impl PaneRef {
    pub fn id(pane: Option<u64>) -> Self {
        Self {
            pane,
            ..Default::default()
        }
    }

    /// Names a pane itself, not just a tab or window.
    pub fn names_pane(&self) -> bool {
        self.pane.is_some() || self.select.is_some()
    }

    /// Names nothing: the focused pane (or tab, or window).
    pub fn is_empty(&self) -> bool {
        !self.names_pane() && self.tab.is_none() && self.window.is_none()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowInfo {
    /// `w_<n>`.
    pub id: String,
    /// Frontmost first, from 1.
    pub index: usize,
    pub title: String,
    /// Set with `window rename`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The frontmost window.
    pub focused: bool,
    pub tabs: usize,
    /// The active tab's id.
    pub active_tab: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TabInfo {
    /// `t_<n>`.
    pub id: String,
    pub window_id: String,
    /// In its window, from 1.
    pub index: usize,
    pub title: String,
    /// Set with `tab rename` (or Rename Tab).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The window's active tab.
    pub active: bool,
    /// Terminal panes, by id.
    pub panes: Vec<u64>,
    /// Its badge: running, awaiting, done, finished or error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    pub zoomed: bool,
}

/// Closes a pane, tab or window. Without `force`, one whose close would
/// ask first (`confirm-close-*`) is refused instead.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CloseParams {
    #[serde(flatten)]
    pub target: PaneRef,
    #[serde(default)]
    pub force: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RenameParams {
    #[serde(flatten)]
    pub target: PaneRef,
    /// Empty goes back to the title Motty makes.
    pub name: String,
}

/// Moves a tab: to `index` (from 1) in its window or in `to_window`, or
/// into a new window.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MoveTabParams {
    #[serde(flatten)]
    pub target: PaneRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_window: Option<String>,
    #[serde(default)]
    pub new_window: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ZoomParams {
    #[serde(flatten)]
    pub target: PaneRef,
    /// Zoom in or out; toggles when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<bool>,
}

/// Moves the dividers beside a pane, by cells: `right` moves the divider
/// on its right (else on its left) to the right, and so on.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResizeParams {
    #[serde(flatten)]
    pub target: PaneRef,
    #[serde(default)]
    pub left: u16,
    #[serde(default)]
    pub right: u16,
    #[serde(default)]
    pub up: u16,
    #[serde(default)]
    pub down: u16,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FocusParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    /// Scroll this stable row (see [`link`]) into view, briefly lit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
}

/// Sets (or with no `kind`, clears) a tab's badge: one of [`BADGE_KINDS`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TabBadgeParams {
    #[serde(flatten)]
    pub target: PaneRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

pub const BADGE_KINDS: [&str; 6] = [
    "running",
    "completed",
    "finished",
    "unread",
    "error",
    "awaiting-input",
];

/// Opens a file or an http(s) URL as a pane: read-only unless `edit`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ViewParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    /// An absolute path or a URL.
    pub target: String,
    #[serde(default)]
    pub edit: bool,
    #[serde(default)]
    pub place: Place,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LinkParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    /// The row where the pane's last command starts.
    #[serde(default)]
    pub last_command: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkInfo {
    pub pane: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentIntegration {
    /// Motty knows how to set this agent up (custom agents it doesn't).
    pub known: bool,
    /// Its hooks are installed.
    pub installed: bool,
}

/// `motty jump`: the best folder for `query`, or with none, home and back.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct JumpParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Where the jump starts: the caller's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Also type `cd <path>` into the pane.
    #[serde(default)]
    pub cd: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JumpResult {
    pub path: String,
}

/// `learn` and `ignore`: a folder, else the pane's.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FolderParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    /// An absolute path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeybindInfo {
    /// As `keybind` lines spell it: `cmd+shift+t`.
    pub keys: String,
    /// As `keybind` lines name it: `new_tab`.
    pub action: String,
    /// `default` or `config`.
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecipeInfo {
    pub id: String,
    pub name: String,
    /// tab, window, commands or snippet.
    pub scope: String,
    /// Commands it replays.
    pub commands: usize,
    /// Unix seconds.
    pub updated: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RecipeApplyParams {
    /// Its id, or its name ignoring case.
    pub recipe: String,
    #[serde(default)]
    pub new_window: bool,
    #[serde(default)]
    pub new_tab: bool,
    /// For its Current Folder paths; the front tab's folder when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppInfo {
    pub version: String,
    pub protocol: u32,
    pub pid: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextRange {
    /// The visible screen.
    Screen,
    /// The last N lines, scrollback included.
    Lines(usize),
    /// The output of the last finished command (needs shell integration).
    LastCommand,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    pub range: TextRange,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendTextParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    pub text: String,
    /// Press Return too: when it's the pane's turn, or at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<Submit>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Submit {
    /// Through the pane's prompt queue: once the agent finishes its turn
    /// or the shell is back at an empty prompt.
    Queue,
    Now,
}

/// Where `open` puts what it opens.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Place {
    #[default]
    Tab,
    Window,
    Split(Direction),
}

/// Opens a file (editor tab), a folder (a shell there) or a URL (the
/// browser).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    /// An absolute path or a URL.
    pub target: String,
    #[serde(default)]
    pub place: Place,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SplitParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    pub direction: Direction,
    /// Program and arguments to run instead of the shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Keep focus where it is.
    #[serde(default)]
    pub no_focus: bool,
    /// The new pane's share of the space, in percent (10–90).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u8>,
    /// A command line typed into the new shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NewTabParams {
    /// Window index; the frontmost window when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Bring back the last closed tab instead (as ⇧⌘T does).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reopen: bool,
    /// A window selector, instead of `window`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select_window: Option<String>,
    /// A command line typed into the new shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// The tab's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NewWindowParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// A command line typed into its shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// The window's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowCreated {
    pub window: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    /// A command line for the pane's shell.
    pub command: String,
    /// Give up after this many milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunResult {
    pub exit: Option<i32>,
    pub output: String,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FindParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    pub query: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    /// `query` is a regular expression (Rust syntax).
    #[serde(default)]
    pub regex: bool,
    /// Matches to return (default 100); `total` counts them all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Also open the pane's find bar with this query.
    #[serde(default)]
    pub show: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FoundMatch {
    /// Screen line: 0 is the top row of the screen, negative lines are
    /// scrollback.
    pub line: i32,
    pub column: usize,
    /// The matched text.
    pub text: String,
    /// The whole line it is on.
    pub line_text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FindResult {
    /// All matches, up to 10000.
    pub total: usize,
    pub matches: Vec<FoundMatch>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaneMode {
    /// Input goes to the program.
    Normal,
    /// Keys move a cursor through the scrollback.
    Vi,
    /// Input is refused; output still streams.
    ReadOnly,
    /// Labels on every link on screen; typing one opens it.
    Hints,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetModeParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    pub mode: PaneMode,
}

/// Runs one of Motty's actions (what keys and the command palette run),
/// by its snake_case name, e.g. `split_right`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionParams {
    #[serde(flatten)]
    pub pane: PaneRef,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Created {
    pub pane: u64,
}

/// The socket a client should use: `$MOTTY_SOCKET`, else the app's default.
pub fn socket_path() -> PathBuf {
    std::env::var_os(SOCKET_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path)
}

/// Unix socket paths must fit in `sockaddr_un` (104 bytes on macOS), with
/// room for a second instance's `motty-<pid>.sock`.
const MAX_SOCKET_PATH: usize = 90;

/// Where the first running Motty listens: in Application Support, or the
/// per-user temp folder when that path is too long for a socket.
pub fn default_socket_path() -> PathBuf {
    let preferred = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Motty/motty.sock");
    if preferred.as_os_str().len() <= MAX_SOCKET_PATH {
        preferred
    } else {
        std::env::temp_dir().join("Motty/motty.sock")
    }
}

/// A blocking client, one request at a time.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

impl Client {
    pub fn connect() -> anyhow::Result<Self> {
        Self::connect_at(&socket_path())
    }

    pub fn connect_at(path: &std::path::Path) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(path)
            .with_context(|| format!("can't reach Motty at {} (is it running?)", path.display()))?;
        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            next_id: 1,
        })
    }

    pub fn set_timeout(&mut self, timeout: Option<Duration>) -> anyhow::Result<()> {
        self.writer.set_read_timeout(timeout)?;
        Ok(())
    }

    pub fn call<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: &P,
    ) -> anyhow::Result<R> {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request {
            id,
            method: method.to_owned(),
            params: serde_json::to_value(params)?,
        };
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        let mut answer = String::new();
        if self.reader.read_line(&mut answer)? == 0 {
            bail!("Motty closed the connection");
        }
        let response: Response = serde_json::from_str(&answer)?;
        if let Some(message) = response.error {
            return Err(IpcError {
                message,
                code: response.code,
            }
            .into());
        }
        Ok(serde_json::from_value(
            response.result.unwrap_or(Value::Null),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_follows_the_agent_else_the_shell() {
        let mut p: PaneInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "window": 0, "tab": 0, "title": "", "cwd": null, "process": null,
            "busy": false, "focused": false, "cols": 80, "rows": 24,
            "shell_integration": false,
        }))
        .unwrap();
        assert_eq!(p.idle(), None);
        p.shell_integration = true;
        assert_eq!(p.idle(), Some(true));
        p.queued = 1;
        assert_eq!(p.idle(), Some(false));
        // A running agent counts as busy to the shell; its turn decides.
        p.queued = 0;
        p.busy = true;
        p.agent = Some("Claude Code".into());
        assert_eq!(p.idle(), Some(true));
        p.in_turn = true;
        assert_eq!(p.idle(), Some(false));
    }

    #[test]
    fn params_are_flat_and_kebab_case() {
        let p = ReadParams {
            pane: PaneRef::id(Some(3)),
            range: TextRange::Lines(20),
        };
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"pane":3,"range":{"lines":20}}"#
        );
        let p: ReadParams = serde_json::from_str(r#"{"range":"last-command"}"#).unwrap();
        assert!(p.pane.pane.is_none() && matches!(p.range, TextRange::LastCommand));
    }
}
