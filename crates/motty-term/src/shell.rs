//! Shell integration: the OSC sequences alacritty's handler drops.
//!
//! - OSC 7 `file://host/path`: the shell's working directory.
//! - OSC 133 A/B/C/D: prompt start, input start, output start and command
//!   end (with its exit status), recorded as command [`Block`]s.
//! - OSC 9 / OSC 777 `notify`: desktop notifications; OSC 9;4 progress.
//! - OSC 5522: the kitty clipboard protocol, for [`crate::clipboard`].
//! - OSC 7501: the program status protocol, in which a program (a coding
//!   agent, a script) says what it's doing; see [`ProgramStatus`].
//!
//! A raw `vte` parser runs beside the VT parser over the same bytes and
//! stops at each of these, so a mark lands where the cursor is at that point
//! in the stream.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use alacritty_terminal::vte::Perform;

/// Command blocks kept per terminal.
const MAX_BLOCKS: usize = 1000;

#[derive(Clone, Debug, PartialEq)]
pub enum ShellOsc {
    Cwd(PathBuf),
    PromptStart,
    InputStart,
    OutputStart,
    CommandEnd(Option<i32>),
    Notify { title: Option<String>, body: String },
    Progress(Progress),
    Status(ProgramStatus),
}

/// An OSC 7501 report about the program as a whole:
/// `ESC ] 7501 ; state=working:app=claude-code:msg=<base64> ST`. Reports
/// about parts of the work (`id=…`) are dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramStatus {
    pub state: ProgramState,
    /// The program's name (`claude-code`, `cargo`); empty when not given.
    pub app: String,
    /// What it's doing or needs, in a few words.
    pub message: Option<String>,
    /// For `blocked`: `permission`, `question` or `auth`.
    pub kind: Option<String>,
    /// From 0 through 100, when the program says.
    pub progress: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
    /// Forget the last report.
    Clear,
}

impl ProgramStatus {
    /// Reads the body after `7501;`: `key=value` pairs joined by `:`, with
    /// `title` and `msg` in base64.
    pub fn parse(body: &str) -> Option<Self> {
        use base64::Engine as _;
        let mut state = None;
        let mut status = Self {
            state: ProgramState::Clear,
            app: String::new(),
            message: None,
            kind: None,
            progress: None,
        };
        for pair in body.split(':') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            match key {
                "state" => {
                    state = Some(match value {
                        "idle" => ProgramState::Idle,
                        "working" => ProgramState::Working,
                        "done" => ProgramState::Done,
                        "blocked" => ProgramState::Blocked,
                        "error" => ProgramState::Error,
                        "clear" => ProgramState::Clear,
                        _ => return None,
                    })
                }
                "id" if !value.is_empty() => return None,
                "app" => status.app = value.chars().take(32).collect(),
                "msg" => {
                    status.message = base64::engine::general_purpose::STANDARD
                        .decode(value)
                        .ok()
                        .map(|b| String::from_utf8_lossy(&b).trim().to_owned())
                        .filter(|m| !m.is_empty())
                        .map(|m| m.chars().take(2048).collect())
                }
                "kind" => status.kind = Some(value.to_owned()).filter(|k| !k.is_empty()),
                "progress" => status.progress = value.parse::<u8>().ok().map(|p| p.min(100)),
                _ => {}
            }
        }
        status.state = state?;
        Some(status)
    }
}

/// OSC 9;4 progress, as ConEmu defined it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Progress {
    Clear,
    Normal(u8),
    Error(u8),
    Indeterminate,
    Paused(u8),
}

/// Collects one shell-integration OSC at a time; `terminated` stops the
/// parser right after it so the caller can catch the VT parser up first.
/// It also catches the queries [`crate::vt`] answers.
#[derive(Default)]
pub struct OscSniffer {
    pending: Option<ShellOsc>,
    clipboard: Option<crate::clipboard::Packet>,
    queries: Vec<crate::vt::Query>,
}

impl OscSniffer {
    pub fn take(&mut self) -> Option<ShellOsc> {
        self.pending.take()
    }

    pub fn take_clipboard(&mut self) -> Option<crate::clipboard::Packet> {
        self.clipboard.take()
    }

    pub fn take_queries(&mut self) -> Vec<crate::vt::Query> {
        std::mem::take(&mut self.queries)
    }
}

impl Perform for OscSniffer {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.first() == Some(&b"5522".as_slice()) {
            self.clipboard = Some(crate::clipboard::Packet::parse(&params[1..]));
        } else {
            self.pending = parse_osc(params);
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &alacritty_terminal::vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        self.queries
            .extend(crate::vt::csi_query(params, intermediates, action));
    }

    fn execute(&mut self, byte: u8) {
        if byte == 0x05 {
            self.queries.push(crate::vt::Query::Enquiry);
        }
    }

    fn terminated(&self) -> bool {
        self.pending.is_some() || self.clipboard.is_some() || !self.queries.is_empty()
    }
}

fn parse_osc(params: &[&[u8]]) -> Option<ShellOsc> {
    let text = |i: usize| {
        params
            .get(i)
            .map(|p| String::from_utf8_lossy(p).into_owned())
    };
    match *params.first()? {
        b"7" => parse_file_url(&text(1)?).map(ShellOsc::Cwd),
        b"133" => match params.get(1)?.first()? {
            b'A' => Some(ShellOsc::PromptStart),
            b'B' => Some(ShellOsc::InputStart),
            b'C' => Some(ShellOsc::OutputStart),
            b'D' => Some(ShellOsc::CommandEnd(
                text(2).and_then(|s| s.trim().parse().ok()),
            )),
            _ => None,
        },
        b"9" if params.get(1) == Some(&b"4".as_slice()) => {
            let state = text(2).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            let pct = text(3)
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(0)
                .min(100);
            Some(ShellOsc::Progress(match state {
                1 => Progress::Normal(pct),
                2 => Progress::Error(pct),
                3 => Progress::Indeterminate,
                4 => Progress::Paused(pct),
                _ => Progress::Clear,
            }))
        }
        b"7501" => ProgramStatus::parse(
            &(1..params.len())
                .filter_map(text)
                .collect::<Vec<_>>()
                .join(";"),
        )
        .map(ShellOsc::Status),
        // The message may itself contain `;`.
        b"9" => Some(ShellOsc::Notify {
            title: None,
            body: (1..params.len())
                .filter_map(text)
                .collect::<Vec<_>>()
                .join(";"),
        }),
        b"777" if params.get(1) == Some(&b"notify".as_slice()) => Some(ShellOsc::Notify {
            title: text(2).filter(|t| !t.is_empty()),
            body: (3..params.len())
                .filter_map(text)
                .collect::<Vec<_>>()
                .join(";"),
        }),
        _ => None,
    }
}

/// `file://host/path` (percent-encoded) to a path; the host is not checked,
/// so remote shells report remote paths.
fn parse_file_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|&b| hex(b)),
                bytes.get(i + 2).and_then(|&b| hex(b)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

/// One prompt and the command run from it. Rows are `Grid::lines_pushed +
/// line` at the time, which stay valid as output scrolls.
#[derive(Clone, Debug)]
pub struct Block {
    pub prompt_row: u64,
    /// Where typing starts (end of the prompt).
    pub input: Option<(u64, usize)>,
    pub output_row: Option<u64>,
    pub end_row: Option<u64>,
    pub exit: Option<i32>,
    pub started: Option<Instant>,
    pub finished: Option<Instant>,
}

impl Block {
    pub fn duration(&self) -> Option<Duration> {
        Some(self.finished?.duration_since(self.started?))
    }
}

/// What shell integration has reported for one terminal.
#[derive(Default)]
pub struct ShellState {
    pub cwd: Option<PathBuf>,
    pub blocks: VecDeque<Block>,
    /// Whether the shell has sent any OSC 133 mark.
    pub integrated: bool,
    pub progress: Option<Progress>,
    /// The pointer shape a program set (OSC 22); a new prompt clears it.
    pub pointer: Option<alacritty_terminal::vte::ansi::cursor_icon::CursorIcon>,
}

impl ShellState {
    /// Records `osc` with the cursor at (`row`, `col`); returns a finished
    /// command, if this ended one.
    pub fn apply(&mut self, osc: &ShellOsc, row: u64, col: usize) -> Option<Block> {
        match osc {
            ShellOsc::Cwd(path) => self.cwd = Some(path.clone()),
            ShellOsc::PromptStart => {
                self.integrated = true;
                self.pointer = None;
                if self.blocks.len() == MAX_BLOCKS {
                    self.blocks.pop_front();
                }
                self.blocks.push_back(Block {
                    prompt_row: row,
                    input: None,
                    output_row: None,
                    end_row: None,
                    exit: None,
                    started: None,
                    finished: None,
                });
            }
            ShellOsc::InputStart => {
                if let Some(b) = self.blocks.back_mut() {
                    b.input = Some((row, col));
                }
            }
            ShellOsc::OutputStart => {
                if let Some(b) = self.blocks.back_mut() {
                    b.output_row = Some(row);
                    b.started = Some(Instant::now());
                }
            }
            ShellOsc::CommandEnd(exit) => {
                // Progress belongs to the command; a crash mustn't leave it.
                self.progress = None;
                let b = self.blocks.back_mut()?;
                // A bare prompt (empty line, ^C) ends without running anything.
                if b.started.is_none() || b.finished.is_some() {
                    return None;
                }
                b.end_row = Some(row);
                b.exit = *exit;
                b.finished = Some(Instant::now());
                return Some(b.clone());
            }
            ShellOsc::Progress(p) => self.progress = (*p != Progress::Clear).then_some(*p),
            ShellOsc::Notify { .. } | ShellOsc::Status(_) => {}
        }
        None
    }

    /// Whether `osc` (before it's applied) is the first sign that a command
    /// that ran is over: its end, or the next prompt when the shell sent no
    /// end. A prompt the shell merely redraws isn't, nor is an empty one.
    pub fn ends_command(&self, osc: &ShellOsc) -> bool {
        matches!(osc, ShellOsc::CommandEnd(_) | ShellOsc::PromptStart)
            && self
                .blocks
                .back()
                .is_some_and(|b| b.started.is_some() && b.finished.is_none())
    }

    /// The nearest prompt above (`back`) or below the stable row `top`,
    /// skipping prompts older than `oldest` (gone from the scrollback).
    pub fn prompt_from(&self, top: u64, oldest: u64, back: bool) -> Option<u64> {
        let mut rows = self.blocks.iter().map(|b| b.prompt_row);
        if back {
            rows.rev().find(|&row| row < top && row >= oldest)
        } else {
            rows.find(|&row| row > top)
        }
    }

    /// The last command that ran to completion.
    pub fn last_finished(&self) -> Option<&Block> {
        self.blocks.iter().rev().find(|b| b.finished.is_some())
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::vte::Parser;

    use super::*;

    fn sniff(bytes: &[u8]) -> Vec<(usize, ShellOsc)> {
        let (mut parser, mut sniffer, mut out, mut pos) =
            (Parser::new(), OscSniffer::default(), Vec::new(), 0);
        while pos < bytes.len() {
            pos += parser.advance_until_terminated(&mut sniffer, &bytes[pos..]);
            if let Some(osc) = sniffer.take() {
                out.push((pos, osc));
            }
        }
        out
    }

    #[test]
    fn stops_right_after_each_sequence() {
        let input = b"a\x1b]133;A\x07$ \x1b]133;B\x1b\\ls\n\x1b]133;D;2\x07";
        let found = sniff(input);
        assert_eq!(found[0], (9, ShellOsc::PromptStart));
        assert_eq!(found[1].1, ShellOsc::InputStart);
        assert_eq!(found[2], (input.len(), ShellOsc::CommandEnd(Some(2))));
    }

    #[test]
    fn parses_program_status() {
        // As `muxy status blocked --msg Apply? --kind permission --app terraform`.
        let found = sniff(
            b"\x1b]7501;state=blocked:app=terraform:progress=40:kind=permission:msg=QXBwbHk/\x1b\\",
        );
        assert_eq!(
            found[0].1,
            ShellOsc::Status(ProgramStatus {
                state: ProgramState::Blocked,
                app: "terraform".into(),
                message: Some("Apply?".into()),
                kind: Some("permission".into()),
                progress: Some(40),
            })
        );
        let ShellOsc::Status(idle) = &sniff(b"\x1b]7501;state=idle\x07")[0].1 else {
            panic!("not a status");
        };
        assert_eq!((idle.state, idle.app.as_str()), (ProgramState::Idle, ""));
        // Parts of the work, unknown states and reports without a state.
        assert!(sniff(b"\x1b]7501;state=working:id=a\x07").is_empty());
        assert!(sniff(b"\x1b]7501;state=sleeping\x07").is_empty());
        assert!(sniff(b"\x1b]7501;app=x\x07").is_empty());
    }

    #[test]
    fn parses_cwd_notifications_and_progress() {
        assert_eq!(
            sniff(b"\x1b]7;file://mac.local/Users/a%20b\x07")[0].1,
            ShellOsc::Cwd("/Users/a b".into())
        );
        assert_eq!(
            sniff(b"\x1b]777;notify;Build;done; ok\x07")[0].1,
            ShellOsc::Notify {
                title: Some("Build".into()),
                body: "done; ok".into()
            }
        );
        assert_eq!(
            sniff(b"\x1b]9;hello\x07")[0].1,
            ShellOsc::Notify {
                title: None,
                body: "hello".into()
            }
        );
        assert_eq!(
            sniff(b"\x1b]9;4;1;40\x07")[0].1,
            ShellOsc::Progress(Progress::Normal(40))
        );
        assert!(sniff(b"\x1b]0;title\x07").is_empty());
    }

    #[test]
    fn prompts_above_and_below_a_row() {
        let mut state = ShellState::default();
        for row in [5, 20, 40] {
            state.apply(&ShellOsc::PromptStart, row, 0);
        }
        assert_eq!(state.prompt_from(30, 0, true), Some(20));
        assert_eq!(state.prompt_from(20, 0, true), Some(5));
        assert_eq!(state.prompt_from(5, 0, true), None);
        // Trimmed from the scrollback.
        assert_eq!(state.prompt_from(20, 10, true), None);
        assert_eq!(state.prompt_from(5, 0, false), Some(20));
        assert_eq!(state.prompt_from(30, 0, false), Some(40));
        assert_eq!(state.prompt_from(40, 0, false), None);
    }

    #[test]
    fn blocks_record_commands_but_not_bare_prompts() {
        let mut state = ShellState::default();
        state.apply(&ShellOsc::PromptStart, 0, 0);
        assert!(state.apply(&ShellOsc::CommandEnd(Some(0)), 1, 0).is_none());
        state.apply(&ShellOsc::PromptStart, 1, 0);
        state.apply(&ShellOsc::InputStart, 1, 2);
        state.apply(&ShellOsc::OutputStart, 2, 0);
        let done = state.apply(&ShellOsc::CommandEnd(Some(1)), 5, 0).unwrap();
        assert_eq!(
            (done.prompt_row, done.output_row, done.exit),
            (1, Some(2), Some(1))
        );
        assert_eq!(state.last_finished().unwrap().end_row, Some(5));
    }

    #[test]
    fn a_command_ends_once() {
        let mut state = ShellState::default();
        let end = ShellOsc::CommandEnd(Some(0));
        // A bare prompt, and its redraws, end nothing.
        state.apply(&ShellOsc::PromptStart, 0, 0);
        assert!(!state.ends_command(&ShellOsc::PromptStart));
        assert!(!state.ends_command(&end));
        state.apply(&ShellOsc::InputStart, 0, 2);
        state.apply(&ShellOsc::OutputStart, 1, 0);
        assert!(state.ends_command(&end));
        // A shell that sends no end: the next prompt ends it.
        assert!(state.ends_command(&ShellOsc::PromptStart));
        assert!(!state.ends_command(&ShellOsc::InputStart));
        state.apply(&end, 2, 0);
        assert!(!state.ends_command(&ShellOsc::PromptStart));
    }

    #[test]
    fn stops_after_each_query() {
        use crate::vt::Query;
        let input = b"ab\x1b[>q\x1b[c\x05x";
        let (mut parser, mut sniffer) = (Parser::new(), OscSniffer::default());
        let mut pos = 0;
        let mut found = Vec::new();
        while pos < input.len() {
            pos += parser.advance_until_terminated(&mut sniffer, &input[pos..]);
            found.extend(sniffer.take_queries().into_iter().map(|q| (pos, q)));
        }
        assert_eq!(
            found,
            [
                (6, Query::Version),
                (9, Query::PrimaryAttributes),
                (11, Query::Enquiry)
            ]
        );
    }
}
