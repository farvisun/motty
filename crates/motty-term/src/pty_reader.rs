//! Drains the PTY on a thread of its own. macOS hands out about 1 KiB per
//! read whatever the buffer size, so reading and parsing on one thread
//! leaves the program blocked on its writes while output is parsed. Here it
//! keeps writing while [`crate::pty_loop`] parses what came before, and the
//! parser takes everything that piled up meanwhile in one go.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use polling::{Event, Events, PollMode, Poller};

/// Most bytes read before handing them over.
const CHUNK: usize = 0x1_0000;
/// Bytes queued before the reader waits for the parser, which in turn makes
/// the program wait on its writes.
const QUEUE: usize = 0x10_0000;
/// Most bytes read once asked to stop, for output that never ends (a
/// background job still printing).
const FINAL: usize = QUEUE;

pub struct Reader {
    shared: Arc<Shared>,
    poller: Arc<Poller>,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    /// Signalled when the parser takes bytes or the reader ends.
    changed: Condvar,
    stop: AtomicBool,
}

#[derive(Default)]
struct Queue {
    bytes: Vec<u8>,
    ended: bool,
}

impl Shared {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Reader {
    /// Starts reading `pty` (a copy of its descriptor) and wakes `wake`
    /// when bytes arrive with none queued.
    pub fn spawn(pty: &File, wake: Arc<Poller>) -> std::io::Result<Self> {
        let file = pty.try_clone()?;
        let poller = Arc::new(Poller::new()?);
        // SAFETY: `file` stays registered with this poller only, and is
        // deregistered before it is dropped.
        unsafe { poller.add_with_mode(&file, Event::readable(0), PollMode::Level)? };
        let shared = Arc::new(Shared::default());
        let (thread_shared, thread_poller) = (shared.clone(), poller.clone());
        std::thread::Builder::new()
            .name("PTY drain".into())
            .spawn(move || {
                drain(&file, &thread_poller, &thread_shared, &wake);
                let _ = thread_poller.delete(&file);
                thread_shared.queue().ended = true;
                thread_shared.changed.notify_all();
                let _ = wake.notify();
            })?;
        Ok(Self { shared, poller })
    }

    /// Moves the queued bytes to the end of `out`.
    pub fn take(&self, out: &mut Vec<u8>) {
        let mut queue = self.shared.queue();
        if out.is_empty() {
            std::mem::swap(out, &mut queue.bytes);
        } else {
            out.append(&mut queue.bytes);
        }
        drop(queue);
        self.shared.changed.notify_all();
    }

    /// Whether bytes are queued.
    pub fn pending(&self) -> bool {
        !self.shared.queue().bytes.is_empty()
    }

    /// Stops the reader and moves everything it read to the end of `out`.
    pub fn finish(&self, out: &mut Vec<u8>) {
        self.stop();
        let mut queue = self.shared.queue();
        while !queue.ended {
            out.append(&mut queue.bytes);
            self.shared.changed.notify_all();
            queue = self
                .shared
                .changed
                .wait(queue)
                .unwrap_or_else(|e| e.into_inner());
        }
        out.append(&mut queue.bytes);
    }

    /// Has the reader read what's left (up to [`FINAL`]) and end.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.changed.notify_all();
        let _ = self.poller.notify();
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop();
    }
}

fn drain(mut file: &File, poller: &Poller, shared: &Shared, wake: &Poller) {
    let mut events = Events::new();
    let mut buf = Vec::new();
    loop {
        events.clear();
        match poller.wait(&mut events, None) {
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => {
                tracing::error!("pty drain: {e}");
                return;
            }
            Ok(_) => {}
        }
        let last = shared.stop.load(Ordering::Acquire);
        let ended = read_available(&mut file, &mut buf, if last { FINAL } else { CHUNK });
        if !buf.is_empty() {
            let mut queue = shared.queue();
            while queue.bytes.len() >= QUEUE && !shared.stop.load(Ordering::Acquire) {
                queue = shared
                    .changed
                    .wait(queue)
                    .unwrap_or_else(|e| e.into_inner());
            }
            // Bytes already queued mean a wakeup is on its way.
            let idle = queue.bytes.is_empty();
            queue.bytes.append(&mut buf);
            drop(queue);
            if idle {
                let _ = wake.notify();
            }
        }
        if ended || last {
            return;
        }
    }
}

/// Reads into `out` until the PTY would block or `limit` bytes; `true` once
/// it has ended (the child and everything holding its terminal are gone).
fn read_available(file: &mut &File, out: &mut Vec<u8>, limit: usize) -> bool {
    while out.len() < limit {
        let len = out.len();
        out.resize((len + CHUNK).min(limit), 0);
        match file.read(&mut out[len..]) {
            Ok(0) => {
                out.truncate(len);
                return true;
            }
            Ok(n) => out.truncate(len + n),
            Err(e) if e.kind() == ErrorKind::Interrupted => out.truncate(len),
            Err(e) => {
                out.truncate(len);
                return e.kind() != ErrorKind::WouldBlock;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use super::*;

    /// A socket stands in for the PTY: same non-blocking reads.
    fn pair() -> (UnixStream, File) {
        let (a, b) = UnixStream::pair().unwrap();
        b.set_nonblocking(true).unwrap();
        (a, File::from(std::os::fd::OwnedFd::from(b)))
    }

    #[test]
    fn delivers_everything_in_order() {
        let (mut writer, file) = pair();
        let reader = Reader::spawn(&file, Arc::new(Poller::new().unwrap())).unwrap();
        // More than the queue holds, so the reader has to wait for us.
        let data: Vec<u8> = (0..QUEUE * 3).map(|i| (i % 251) as u8).collect();
        let sent = data.clone();
        let feeder = std::thread::spawn(move || writer.write_all(&sent).unwrap());
        let mut got = Vec::new();
        while got.len() < data.len() {
            reader.take(&mut got);
            std::thread::sleep(Duration::from_millis(1));
        }
        feeder.join().unwrap();
        assert!(got == data);
    }

    #[test]
    fn finish_reads_what_is_left() {
        let (mut writer, file) = pair();
        let reader = Reader::spawn(&file, Arc::new(Poller::new().unwrap())).unwrap();
        writer.write_all(b"last words").unwrap();
        // Let the reader take it, or leave it for the final read.
        std::thread::sleep(Duration::from_millis(20));
        // `writer` stays open: finishing alone ends the stream.
        let mut got = Vec::new();
        reader.finish(&mut got);
        assert_eq!(got, b"last words");
    }

    #[test]
    fn finish_after_the_end() {
        let (mut writer, file) = pair();
        let reader = Reader::spawn(&file, Arc::new(Poller::new().unwrap())).unwrap();
        writer.write_all(b"bye").unwrap();
        drop(writer);
        let mut got = Vec::new();
        reader.finish(&mut got);
        assert_eq!(got, b"bye");
    }
}
