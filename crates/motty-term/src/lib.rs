//! A terminal session: PTY + child process + VT state, with parsing done on
//! alacritty's PTY reader thread. UI layers hold a [`Session`] and receive
//! [`SessionEvent`]s; content wakeups are coalesced so a flood of output
//! produces at most one pending notification.

use std::collections::HashMap;
use std::os::fd::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::tty;

pub mod input;

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
    ColorRequest(usize, Arc<dyn Fn(alacritty_terminal::vte::ansi::Rgb) -> String + Send + Sync>),
    TextAreaSizeRequest(Arc<dyn Fn(WindowSize) -> String + Send + Sync>),
    CursorBlinkingChange,
    Exited(Option<i32>),
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
        }
    }
}

#[derive(Clone)]
pub struct EventProxy {
    tx: async_channel::Sender<SessionEvent>,
    wakeup_pending: Arc<AtomicBool>,
    pty_tx: Arc<std::sync::OnceLock<EventLoopSender>>,
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
                    let _ = tx.send(Msg::Input(text.into_bytes().into()));
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
}

pub struct Session {
    pub term: Arc<FairMutex<Terminal>>,
    pty_tx: EventLoopSender,
    wakeup_pending: Arc<AtomicBool>,
    size: GridSize,
    pid: u32,
    /// PTY master, kept for foreground-process queries. Owned by the event loop.
    master_fd: RawFd,
}

impl Session {
    pub fn spawn(config: SessionConfig, size: GridSize) -> anyhow::Result<(Self, async_channel::Receiver<SessionEvent>)> {
        let (tx, rx) = async_channel::unbounded();
        let wakeup_pending = Arc::new(AtomicBool::new(false));
        let pty_tx_cell = Arc::new(std::sync::OnceLock::new());
        let proxy = EventProxy { tx, wakeup_pending: wakeup_pending.clone(), pty_tx: pty_tx_cell.clone() };

        let mut env = config.env.clone();
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "Motty".into());
        env.insert("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into());

        let options = tty::Options {
            shell: config.command.clone().map(|(program, args)| tty::Shell::new(program, args)),
            working_directory: config.working_directory.clone(),
            drain_on_exit: true,
            env,
        };
        let pty = tty::new(&options, size.window_size(), 0)?;
        let pid = pty.child().id();
        let master_fd = pty.file().as_raw_fd();

        let term_config = term::Config {
            scrolling_history: if config.scrollback == 0 { 10_000 } else { config.scrollback },
            ..Default::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(term_config, &size, proxy.clone())));
        let event_loop = EventLoop::new(term.clone(), proxy, pty, false, false)?;
        let pty_tx = event_loop.channel();
        let _ = pty_tx_cell.set(pty_tx.clone());
        event_loop.spawn();

        Ok((Self { term, pty_tx, wakeup_pending, size, pid, master_fd }, rx))
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
            let _ = self.pty_tx.send(Msg::Input(bytes));
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
        let _ = self.pty_tx.send(Msg::Resize(size.window_size()));
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

    /// Working directory of the foreground process.
    pub fn foreground_cwd(&self) -> Option<PathBuf> {
        process_cwd(self.foreground_pid()?)
    }
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
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&raw mut info).cast(), size) };
    if n != size {
        return None;
    }
    // SAFETY: the kernel NUL-terminates vip_path.
    let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
    Some(PathBuf::from(path.to_string_lossy().into_owned()))
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
        let _ = self.pty_tx.send(Msg::Shutdown);
    }
}
