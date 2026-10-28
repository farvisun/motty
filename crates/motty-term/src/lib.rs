//! A terminal session: PTY + child process + VT state, with parsing done on
//! our PTY reader thread ([`pty_loop`]). UI layers hold a [`Session`] and receive
//! [`SessionEvent`]s; content wakeups are coalesced so a flood of output
//! produces at most one pending notification.

use std::collections::HashMap;
use std::os::fd::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::CursorStyle;

mod graphemes;
pub mod input;
mod pty_loop;
pub mod shell;

use pty_loop::{LoopSender, Msg, PtyLoop};

pub use alacritty_terminal;

pub type Terminal = Term<EventProxy>;

/// Events delivered to the UI thread.
pub enum SessionEvent {
    /// Terminal content changed. Re-arm with [`Session::take_wakeup`].
    Wakeup,
    Title(Option<String>),
    Bell,
    ClipboardStore(String),
    /// The application asked to read the clipboard; reply via `Session::write`
    /// with `format(contents)`.
    ClipboardLoad(Arc<dyn Fn(&str) -> String + Send + Sync>),
    /// Reply needed: the formatter turns an RGB color into the response sequence.
    ColorRequest(
        usize,
        Arc<dyn Fn(alacritty_terminal::vte::ansi::Rgb) -> String + Send + Sync>,
    ),
    TextAreaSizeRequest(Arc<dyn Fn(WindowSize) -> String + Send + Sync>),
    CursorBlinkingChange,
    Exited(Option<i32>),
    /// A command run from an integrated shell finished.
    CommandFinished {
        exit: Option<i32>,
        duration: Option<std::time::Duration>,
    },
    /// OSC 9 / OSC 777 desktop notification.
    Notification {
        title: Option<String>,
        body: String,
    },
    /// The shell reported a new cwd or progress; see [`Session::shell`].
    ShellChanged,
}

impl std::fmt::Debug for SessionEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wakeup => f.write_str("Wakeup"),
            Self::Title(t) => write!(f, "Title({t:?})"),
            Self::Bell => f.write_str("Bell"),
            Self::ClipboardStore(_) => f.write_str("ClipboardStore"),
            Self::ClipboardLoad(_) => f.write_str("ClipboardLoad"),
            Self::ColorRequest(i, _) => write!(f, "ColorRequest({i})"),
            Self::TextAreaSizeRequest(_) => f.write_str("TextAreaSizeRequest"),
            Self::CursorBlinkingChange => f.write_str("CursorBlinkingChange"),
            Self::Exited(code) => write!(f, "Exited({code:?})"),
            Self::CommandFinished { exit, duration } => {
                write!(f, "CommandFinished({exit:?}, {duration:?})")
            }
            Self::Notification { title, body } => write!(f, "Notification({title:?}, {body:?})"),
            Self::ShellChanged => f.write_str("ShellChanged"),
        }
    }
}

#[derive(Clone)]
pub struct EventProxy {
    tx: async_channel::Sender<SessionEvent>,
    wakeup_pending: Arc<AtomicBool>,
    pty_tx: Arc<std::sync::OnceLock<LoopSender>>,
}

impl EventProxy {
    fn send_session_event(&self, event: SessionEvent) {
        let _ = self.tx.try_send(event);
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let ev = match event {
            Event::Wakeup => {
                if self.wakeup_pending.swap(true, Ordering::AcqRel) {
                    return;
                }
                SessionEvent::Wakeup
            }
            // Replies the terminal generates itself (DSR, DA) go straight back.
            Event::PtyWrite(text) => {
                tracing::trace!(?text, "pty reply");
                if let Some(tx) = self.pty_tx.get() {
                    tx.send(Msg::Input(text.into_bytes().into()));
                }
                return;
            }
            Event::Title(t) => SessionEvent::Title(Some(t)),
            Event::ResetTitle => SessionEvent::Title(None),
            Event::Bell => SessionEvent::Bell,
            Event::ClipboardStore(_, s) => SessionEvent::ClipboardStore(s),
            Event::ClipboardLoad(_, f) => SessionEvent::ClipboardLoad(f),
            Event::ColorRequest(i, f) => SessionEvent::ColorRequest(i, f),
            Event::TextAreaSizeRequest(f) => SessionEvent::TextAreaSizeRequest(f),
            Event::CursorBlinkingChange => SessionEvent::CursorBlinkingChange,
            Event::ChildExit(status) => SessionEvent::Exited(status.code()),
            Event::Exit => SessionEvent::Exited(None),
            Event::MouseCursorDirty => return,
        };
        let _ = self.tx.try_send(ev);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl GridSize {
    fn window_size(self) -> WindowSize {
        WindowSize {
            num_cols: self.cols,
            num_lines: self.rows,
            cell_width: self.cell_width.round() as u16,
            cell_height: self.cell_height.round() as u16,
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

#[derive(Clone, Debug, Default)]
pub struct SessionConfig {
    /// Program and arguments; `None` runs the user's login shell.
    pub command: Option<(String, Vec<String>)>,
    pub working_directory: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub scrollback: usize,
    /// Shape and blinking until the program asks for another (DECSCUSR).
    pub cursor_style: CursorStyle,
}

pub struct Session {
    pub term: Arc<FairMutex<Terminal>>,
    pty_tx: LoopSender,
    wakeup_pending: Arc<AtomicBool>,
    size: GridSize,
    pid: u32,
    /// PTY master, kept for foreground-process queries. Owned by the event loop.
    master_fd: RawFd,
    shell: Arc<std::sync::Mutex<shell::ShellState>>,
}

impl Session {
    pub fn spawn(
        config: SessionConfig,
        size: GridSize,
    ) -> anyhow::Result<(Self, async_channel::Receiver<SessionEvent>)> {
        let (tx, rx) = async_channel::unbounded();
        let wakeup_pending = Arc::new(AtomicBool::new(false));
        let pty_tx_cell = Arc::new(std::sync::OnceLock::new());
        let proxy = EventProxy {
            tx,
            wakeup_pending: wakeup_pending.clone(),
            pty_tx: pty_tx_cell.clone(),
        };

        let mut env = config.env.clone();
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "Motty".into());
        env.insert(
            "TERM_PROGRAM_VERSION".into(),
            env!("CARGO_PKG_VERSION").into(),
        );

        let options = tty::Options {
            shell: config
                .command
                .clone()
                .map(|(program, args)| tty::Shell::new(program, args)),
            working_directory: config.working_directory.clone(),
            drain_on_exit: true,
            env,
        };
        let pty = tty::new(&options, size.window_size(), 0)?;
        let pid = pty.child().id();
        let master_fd = pty.file().as_raw_fd();

        let term_config = term::Config {
            scrolling_history: if config.scrollback == 0 {
                10_000
            } else {
                config.scrollback
            },
            default_cursor_style: config.cursor_style,
            ..Default::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(term_config, &size, proxy.clone())));
        let shell = Arc::new(std::sync::Mutex::new(shell::ShellState::default()));
        let (pty_loop, pty_tx) = PtyLoop::new(pty, term.clone(), proxy, shell.clone())?;
        let _ = pty_tx_cell.set(pty_tx.clone());
        pty_loop.spawn();

        Ok((
            Self {
                term,
                pty_tx,
                wakeup_pending,
                size,
                pid,
                master_fd,
                shell,
            },
            rx,
        ))
    }

    /// Applies changed config to a running terminal; the cursor style only
    /// matters until a program sets its own.
    pub fn set_options(&self, cursor_style: CursorStyle, scrollback: usize) {
        let options = term::Config {
            scrolling_history: if scrollback == 0 { 10_000 } else { scrollback },
            default_cursor_style: cursor_style,
            ..Default::default()
        };
        self.term.lock().set_options(options);
    }

    /// Re-arms content wakeups. Call before reading the terminal for a frame,
    /// so output arriving during the frame schedules another one.
    pub fn take_wakeup(&self) {
        self.wakeup_pending.store(false, Ordering::Release);
    }

    pub fn write(&self, bytes: impl Into<std::borrow::Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        tracing::trace!(bytes = ?String::from_utf8_lossy(&bytes), "pty write");
        if !bytes.is_empty() {
            self.pty_tx.send(Msg::Input(bytes));
        }
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    pub fn resize(&mut self, size: GridSize) {
        if size == self.size || size.cols == 0 || size.rows == 0 {
            return;
        }
        self.size = size;
        self.term.lock().resize(size);
        self.pty_tx.send(Msg::Resize(size.window_size()));
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Process group currently in the foreground of the PTY (the running
    /// command, or the shell when idle).
    pub fn foreground_pid(&self) -> Option<i32> {
        // SAFETY: tcgetpgrp only reads from the descriptor.
        let pgid = unsafe { libc::tcgetpgrp(self.master_fd) };
        (pgid > 0).then_some(pgid)
    }

    /// Short name of the foreground process, e.g. `zsh` or `vim`.
    pub fn foreground_process_name(&self) -> Option<String> {
        process_name(self.foreground_pid()?)
    }

    /// The shell's pid. On macOS the default shell is started through
    /// `/usr/bin/login`, so the spawned process is `login` (root-owned, so
    /// its name is unreadable) and the shell is its child.
    fn shell_pid(&self) -> i32 {
        let pid = self.pid as i32;
        match process_name(pid).as_deref() {
            None | Some("login") => first_child(pid).unwrap_or(pid),
            _ => pid,
        }
    }

    /// Short name of the shell, e.g. `zsh`.
    pub fn shell_name(&self) -> Option<String> {
        process_name(self.shell_pid())
    }

    /// Whether the shell is running a command rather than waiting at a
    /// prompt. Non-interactive shells keep children in their own process
    /// group, so child processes are checked as well as the foreground group.
    pub fn is_busy(&self) -> bool {
        let shell = self.shell_pid();
        self.foreground_pid().is_some_and(|pgid| pgid != shell) || first_child(shell).is_some()
    }

    /// What shell integration has reported (cwd, command blocks).
    pub fn shell(&self) -> std::sync::MutexGuard<'_, shell::ShellState> {
        self.shell.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The visible screen as text, without trailing blank lines.
    pub fn screen_text(&self) -> String {
        let term = self.term.lock();
        let last = term.screen_lines() as i32 - 1;
        text_between(&term, Line(0), Line(last))
            .trim_end_matches('\n')
            .to_owned()
    }

    /// The last `n` lines, scrollback included, without trailing blank lines.
    pub fn last_lines(&self, n: usize) -> String {
        let term = self.term.lock();
        let bottom = term.bottommost_line();
        let top = Line((bottom.0 - n as i32 + 1).max(term.topmost_line().0));
        text_between(&term, top, bottom)
            .trim_end_matches('\n')
            .to_owned()
    }

    /// What `block`'s command printed, if those rows are still there.
    pub fn block_output(&self, block: &shell::Block) -> Option<String> {
        let (start, end) = (block.output_row?, block.end_row?);
        let term = self.term.lock();
        let pushed = term.grid().lines_pushed() as i64;
        let line = |row: u64| Line((row as i64 - pushed) as i32);
        let (first, after) = (line(start), line(end));
        if first < term.topmost_line() || after.0 <= first.0 {
            return (after.0 == first.0).then(String::new);
        }
        Some(text_between(&term, first, Line(after.0 - 1)))
    }

    /// Working directory of the foreground process.
    pub fn foreground_cwd(&self) -> Option<PathBuf> {
        process_cwd(self.foreground_pid()?)
    }
}

/// Whole lines `top..=bottom` as text, joining wrapped lines.
fn text_between(term: &Terminal, top: Line, bottom: Line) -> String {
    let end = Point::new(bottom, Column(term.columns().saturating_sub(1)));
    term.bounds_to_string(Point::new(top, Column(0)), end)
}

#[cfg(target_os = "macos")]
fn process_name(pid: i32) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its full length.
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

#[cfg(target_os = "macos")]
fn process_cwd(pid: i32) -> Option<PathBuf> {
    use std::ffi::CStr;
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
    // SAFETY: `info` is a properly sized, writable proc_vnodepathinfo.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // SAFETY: the kernel NUL-terminates vip_path.
    let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
    Some(PathBuf::from(path.to_string_lossy().into_owned()))
}

#[cfg(target_os = "macos")]
fn first_child(pid: i32) -> Option<i32> {
    let mut buf = [0 as libc::pid_t; 16];
    let size = std::mem::size_of_val(&buf) as i32;
    // SAFETY: the buffer is valid for `size` bytes.
    let n = unsafe { libc::proc_listchildpids(pid, buf.as_mut_ptr().cast(), size) };
    (n > 0 && buf[0] > 0).then_some(buf[0])
}

#[cfg(not(target_os = "macos"))]
fn first_child(_pid: i32) -> Option<i32> {
    None
}

#[cfg(not(target_os = "macos"))]
fn process_name(_pid: i32) -> Option<String> {
    None
}

#[cfg(not(target_os = "macos"))]
fn process_cwd(_pid: i32) -> Option<PathBuf> {
    None
}

impl Drop for Session {
    fn drop(&mut self) {
        self.pty_tx.send(Msg::Shutdown);
    }
}
