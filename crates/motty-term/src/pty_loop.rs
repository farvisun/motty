//! The PTY reader thread: reads the child's output, parses it into the
//! terminal through [`Graphemes`], writes input and replies back, and
//! reports the child's exit. Modeled on alacritty's event loop, which only
//! parses into a bare `Term`.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, OnResize, WindowSize};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::tty::{self, EventedPty, EventedReadWrite};
use alacritty_terminal::vte::ansi;
use polling::{Event as PollEvent, Events, PollMode, Poller};

use crate::graphemes::{Cluster, Graphemes};
use crate::{EventProxy, Terminal};

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
    /// Input waiting for the PTY to accept it, and how much of the front
    /// chunk was written.
    pending: VecDeque<Cow<'static, [u8]>>,
    written: usize,
}

impl PtyLoop {
    pub fn new(
        pty: tty::Pty,
        terminal: Arc<FairMutex<Terminal>>,
        proxy: EventProxy,
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
            pending: VecDeque::new(),
            written: 0,
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
                    Msg::Resize(size) => self.pty.on_resize(size),
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
            let term = terminal.get_or_insert_with(|| self.terminal.lock_unfair());
            let mut handler = Graphemes {
                term: &mut **term,
                cluster: &mut self.cluster,
            };
            self.parser.advance(&mut handler, &buf[..n]);
            processed += n;
            if processed >= MAX_LOCKED_READ {
                break;
            }
        }
        drop(terminal);
        // Output held back by a synchronized update shows when it ends.
        if processed > 0 && self.parser.sync_bytes_count() < processed {
            self.proxy.send_event(Event::Wakeup);
        }
        Ok(())
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
