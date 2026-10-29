//! Motty's control protocol, shared by the app (server), the `motty` CLI
//! and the MCP server.
//!
//! Each line on the socket is one JSON [`Request`]; the app answers each
//! with one [`Response`] line carrying the same `id`. Shells started by
//! Motty find the socket in `$MOTTY_SOCKET` and their own pane in
//! `$MOTTY_PANE_ID`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
pub use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod skills;

pub const SOCKET_ENV: &str = "MOTTY_SOCKET";
pub const PANE_ENV: &str = "MOTTY_PANE_ID";

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
}

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
    /// Index of the window among Motty's windows.
    pub window: usize,
    /// Index of the tab in its window.
    pub tab: usize,
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

/// Which pane a request is about; `None` means the focused one.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PaneRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
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
        let path = socket_path();
        let stream = UnixStream::connect(&path)
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
        if let Some(error) = response.error {
            bail!("{error}");
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
            pane: PaneRef { pane: Some(3) },
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
