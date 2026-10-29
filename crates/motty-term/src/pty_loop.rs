//! The PTY reader thread: reads the child's output, parses it into the
//! terminal through [`Graphemes`], writes input and replies back, and
//! reports the child's exit. Modeled on alacritty's event loop, which only
//! parses into a bare `Term`.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, OnResize, WindowSize};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::tty::{self, EventedPty, EventedReadWrite};
use alacritty_terminal::vte::ansi;
use parking_lot::MutexGuard as FairMutexGuard;
use polling::{Event as PollEvent, Events, PollMode, Poller};

use alacritty_terminal::term::TermMode;

use crate::graphemes::{Cluster, Graphemes};
use crate::graphics::{Graphics, Scanner, Sequence, Step, Work};
use crate::repaint::Repaint;
use crate::shell::{OscSniffer, ShellOsc, ShellState};
use crate::{EventProxy, SessionEvent, Terminal};

/// Poller keys alacritty's `Pty::register` uses (crate-private there).
const PTY_READ_WRITE_TOKEN: usize = 0;
const PTY_CHILD_EVENT_TOKEN: usize = 1;

/// Bytes read per `read` call.
const READ_BUFFER: usize = 0x1_0000;
/// Parse at most this much per terminal lock, so the UI can take it.
const MAX_LOCKED_READ: usize = u16::MAX as usize;

pub enum Msg {
    Input(Cow<'static, [u8]>),
    Resize(WindowSize),
    Shutdown,
}

/// Sends to the loop and wakes its poller.
#[derive(Clone)]
pub struct LoopSender {
    tx: Sender<Msg>,
    poller: Arc<Poller>,
}

impl LoopSender {
    pub fn send(&self, msg: Msg) {
        if self.tx.send(msg).is_ok() {
            let _ = self.poller.notify();
        }
    }
}

pub struct PtyLoop {
    pty: tty::Pty,
    poller: Arc<Poller>,
    rx: Receiver<Msg>,
    terminal: Arc<FairMutex<Terminal>>,
    proxy: EventProxy,
    parser: ansi::Processor,
    cluster: Cluster,
    /// OSC 22's pointer shape, on its way to `shell`.
    pointer: Option<alacritty_terminal::vte::ansi::cursor_icon::CursorIcon>,
    /// Runs beside `parser` to catch the shell-integration OSCs it drops.
    osc_parser: alacritty_terminal::vte::Parser,
    sniffer: OscSniffer,
    repaint: Repaint,
    shell: Arc<std::sync::Mutex<ShellState>>,
    /// Cuts image sequences out of the output for `graphics`.
    scanner: Scanner,
    graphics: Arc<Mutex<Graphics>>,
    /// Input waiting for the PTY to accept it, and how much of the front
    /// chunk was written.
    pending: VecDeque<Cow<'static, [u8]>>,
    written: usize,
    /// The grid and cell size, for XTWINOPS replies.
    window: WindowSize,
}

impl PtyLoop {
    pub fn new(
        pty: tty::Pty,
        terminal: Arc<FairMutex<Terminal>>,
        proxy: EventProxy,
        shell: Arc<std::sync::Mutex<ShellState>>,
        window: WindowSize,
        repaint: Repaint,
        graphics: Arc<Mutex<Graphics>>,
    ) -> std::io::Result<(Self, LoopSender)> {
        let poller = Arc::new(Poller::new()?);
        let (tx, rx) = mpsc::channel();
        let sender = LoopSender {
            tx,
            poller: poller.clone(),
        };
        let this = Self {
            pty,
            poller,
            rx,
            terminal,
            proxy,
            parser: ansi::Processor::new(),
            cluster: Cluster::default(),
            pointer: None,
            osc_parser: alacritty_terminal::vte::Parser::new(),
            sniffer: OscSniffer::default(),
            repaint,
            shell,
            scanner: Scanner::default(),
            graphics,
            pending: VecDeque::new(),
            written: 0,
            window,
        };
        Ok((this, sender))
    }

    pub fn spawn(mut self) {
        let spawned = std::thread::Builder::new()
            .name("PTY reader".into())
            .spawn(move || {
                if let Err(e) = self.run() {
                    tracing::error!("pty loop: {e}");
                }
                let _ = self.pty.deregister(&self.poller);
            });
        if let Err(e) = spawned {
            tracing::error!("failed to start the pty thread: {e}");
        }
    }

    fn run(&mut self) -> std::io::Result<()> {
        let mut buf = vec![0u8; READ_BUFFER];
        let mut interest = PollEvent::readable(0);
        // SAFETY: the PTY stays registered with this poller only, and is
        // deregistered before it is dropped.
        unsafe { self.pty.register(&self.poller, interest, PollMode::Level)? };
        let mut events = Events::with_capacity(NonZeroUsize::new(64).unwrap());
        loop {
            // Synchronized updates (DEC 2026) end by themselves after a timeout.
            let timeout = self
                .parser
                .sync_timeout()
                .sync_timeout()
                .map(|t| t.saturating_duration_since(Instant::now()));
            events.clear();
            match self.poller.wait(&mut events, timeout) {
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
                Ok(_) => {}
            }
            let mut messages = false;
            while let Ok(msg) = self.rx.try_recv() {
                messages = true;
                match msg {
                    Msg::Input(bytes) => self.pending.push_back(bytes),
                    Msg::Resize(size) => {
                        self.window = size;
                        self.pty.on_resize(size);
                        // Images are sized with the cell programs are told
                        // about (TIOCGWINSZ, CSI 16 t).
                        lock(&self.graphics).set_cell_size(size.cell_width, size.cell_height);
                    }
                    Msg::Shutdown => return Ok(()),
                }
            }
            if events.is_empty() && !messages && timeout.is_some() {
                self.parser.stop_sync(&mut *self.terminal.lock());
                self.proxy.send_event(Event::Wakeup);
                continue;
            }
            for event in events.iter() {
                match event.key {
                    PTY_CHILD_EVENT_TOKEN => {
                        if let Some(tty::ChildEvent::Exited(status)) = self.pty.next_child_event() {
                            // Show whatever the child printed last.
                            let _ = self.read(&mut buf);
                            if let Some(status) = status {
                                self.proxy.send_event(Event::ChildExit(status));
                            }
                            self.terminal.lock().exit();
                            self.proxy.send_event(Event::Wakeup);
                            return Ok(());
                        }
                    }
                    PTY_READ_WRITE_TOKEN => {
                        if event.is_interrupt() {
                            continue;
                        }
                        if event.readable {
                            self.read(&mut buf)?;
                        }
                        if event.writable {
                            self.write()?;
                        }
                    }
                    _ => {}
                }
            }
            // Writes go out right away when the PTY takes them; otherwise
            // wait until it is writable.
            if !self.pending.is_empty() {
                self.write()?;
            }
            let wants_write = !self.pending.is_empty();
            if wants_write != interest.writable {
                interest.writable = wants_write;
                self.pty
                    .reregister(&self.poller, interest, PollMode::Level)?;
            }
        }
    }

    /// Reads and parses until the PTY would block (or a lock's worth).
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        let mut processed = 0;
        let shared = self.terminal.clone();
        let mut terminal = None;
        loop {
            let n = match self.pty.reader().read(buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {
                    break;
                }
                Err(e) => return Err(e),
            };
            // Image sequences are cut out and handled where they are in the
            // stream; everything else goes to the VT parser.
            let mut pos = 0;
            while pos < n {
                let (used, step) = self.scanner.next(&buf[pos..n]);
                let bytes = &buf[pos..pos + used];
                pos += used;
                match step {
                    Step::Pass => {
                        let term = terminal.get_or_insert_with(|| shared.lock_unfair());
                        self.feed(term, bytes);
                    }
                    Step::PassHeld(held) => {
                        let term = terminal.get_or_insert_with(|| shared.lock_unfair());
                        self.feed(term, &held);
                    }
                    Step::Sequence(seq) => self.image(&mut terminal, &shared, seq),
                    Step::Pending => {}
                }
            }
            processed += n;
            if processed >= MAX_LOCKED_READ {
                break;
            }
        }
        if let Some(term) = terminal.as_mut() {
            let mut graphics = lock(&self.graphics);
            if let Some((first, count)) = self.repaint.settle(term) {
                graphics.rows_removed(false, first, count);
            }
            graphics.prune(term);
        }
        drop(terminal);
        // Output held back by a synchronized update shows when it ends.
        if processed > 0 && self.parser.sync_bytes_count() < processed {
            self.proxy.send_event(Event::Wakeup);
        }
        Ok(())
    }

    /// Feeds plain output to the VT parser, stopping at each
    /// shell-integration OSC so it's recorded where the cursor is then.
    fn feed(&mut self, term: &mut Terminal, bytes: &[u8]) {
        let mut graphics = lock(&self.graphics);
        let mut pos = 0;
        while pos < bytes.len() {
            let end = pos
                + self
                    .osc_parser
                    .advance_until_terminated(&mut self.sniffer, &bytes[pos..]);
            let mut handler = Graphemes {
                term,
                cluster: &mut self.cluster,
                pointer: &mut self.pointer,
                repaint: &mut self.repaint,
                graphics: &mut graphics,
            };
            self.parser.advance(&mut handler, &bytes[pos..end]);
            pos = end;
            if let Some(icon) = self.pointer.take()
                && let Ok(mut shell) = self.shell.lock()
            {
                shell.pointer = Some(icon);
            }
            if let Some(osc) = self.sniffer.take() {
                record_osc(osc, term, &self.shell, &self.proxy);
            }
            for query in self.sniffer.take_queries() {
                let vt = self.proxy.vt();
                if let Some(reply) = crate::vt::reply(query, &vt, self.window, term.title()) {
                    self.proxy.send_event(Event::PtyWrite(reply));
                }
            }
        }
    }

    /// Handles an image sequence. Decoding runs with the terminal unlocked
    /// so a big image doesn't stall drawing; nothing else writes to the
    /// terminal meanwhile, so the cursor stays put.
    fn image<'a>(
        &mut self,
        terminal: &mut Option<FairMutexGuard<'a, Terminal>>,
        shared: &'a FairMutex<Terminal>,
        seq: Sequence,
    ) {
        let term = terminal.get_or_insert_with(|| shared.lock_unfair());
        // Text held back by a synchronized update (DEC 2026) goes first, so
        // the image lands where the program put it; the update resumes after.
        let synced = self.parser.sync_timeout().sync_timeout().is_some();
        if synced {
            let mut graphics = lock(&self.graphics);
            self.parser.stop_sync(&mut Graphemes {
                term: &mut **term,
                cluster: &mut self.cluster,
                pointer: &mut self.pointer,
                repaint: &mut self.repaint,
                graphics: &mut graphics,
            });
        }
        let work = lock(&self.graphics).begin(seq, term);
        self.send_replies();
        if let Work::Decode(job) = work {
            *terminal = None;
            let decoded = job.run();
            let term = terminal.get_or_insert_with(|| shared.lock_unfair());
            lock(&self.graphics).finish(decoded, term);
            self.send_replies();
        }
        if synced {
            let term = terminal.get_or_insert_with(|| shared.lock_unfair());
            self.feed(term, b"\x1b[?2026h");
        }
    }

    fn send_replies(&self) {
        for reply in lock(&self.graphics).take_replies() {
            self.proxy.send_event(Event::PtyWrite(reply));
        }
    }

    fn write(&mut self) -> std::io::Result<()> {
        while let Some(front) = self.pending.front() {
            match self.pty.writer().write(&front[self.written..]) {
                Ok(0) => break,
                Ok(n) => {
                    self.written += n;
                    if self.written == front.len() {
                        self.pending.pop_front();
                        self.written = 0;
                    }
                }
                Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

fn lock(graphics: &Mutex<Graphics>) -> MutexGuard<'_, Graphics> {
    graphics.lock().unwrap_or_else(|e| e.into_inner())
}

/// Applies a shell-integration OSC at the cursor and tells the UI.
fn record_osc(
    osc: ShellOsc,
    term: &mut Terminal,
    shell: &std::sync::Mutex<ShellState>,
    proxy: &EventProxy,
) {
    // Whatever turned on the kitty keyboard protocol or modifyOtherKeys is
    // gone, cleaned up or not (a crash): the shell gets classic keys back.
    if shell
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .ends_command(&osc)
    {
        term.reset_key_encodings();
    }
    let event = match &osc {
        ShellOsc::Notify { title, body } => Some(SessionEvent::Notification {
            title: title.clone(),
            body: body.clone(),
        }),
        ShellOsc::Progress(_) | ShellOsc::Cwd(_) | ShellOsc::OutputStart => {
            Some(SessionEvent::ShellChanged)
        }
        ShellOsc::InputStart => Some(SessionEvent::PromptReady),
        _ => None,
    };
    // Full-screen programs own the alternate screen; marks there would
    // point into the wrong grid.
    if matches!(osc, ShellOsc::Cwd(_) | ShellOsc::Progress(_))
        || !term.mode().contains(TermMode::ALT_SCREEN)
    {
        let grid = term.grid();
        let cursor = grid.cursor.point;
        let row = grid.lines_pushed() + cursor.line.0.max(0) as u64;
        let finished =
            shell
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .apply(&osc, row, cursor.column.0);
        if let Some(block) = finished {
            proxy.send_session_event(SessionEvent::CommandFinished {
                exit: block.exit,
                duration: block.duration(),
            });
        }
    }
    if let Some(event) = event {
        proxy.send_session_event(event);
    }
}
