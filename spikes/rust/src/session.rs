//! Agent sessions: one PTY, one emulator, one child process per session.
//!
//! This module owns the child's lifetime and the pump thread that feeds PTY
//! output into the emulator. It never renders anything and never interprets
//! keys; see [`crate::keys`] for encoding.

use std::{
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    thread,
    time::Instant,
};

use alacritty_terminal::{
    Term,
    event::{Event, EventListener},
    grid::Dimensions,
    term::{Config, TermMode},
    vte::ansi::{NamedColor, Processor, Rgb},
};
use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Scrollback cap per session, per ARCHITECTURE.md §4.1.
const SCROLLBACK_LINES: usize = 10_000;

/// Size of one PTY read, per ARCHITECTURE.md §4.1.
const READ_CHUNK: usize = 32 * 1024;

/// Default foreground reported to OSC 10 queries.
const DEFAULT_FG: Rgb = Rgb {
    r: 0xd8,
    g: 0xd8,
    b: 0xd8,
};

/// Default background reported to OSC 11 queries.
const DEFAULT_BG: Rgb = Rgb {
    r: 0x1c,
    g: 0x1c,
    b: 0x1c,
};

/// Pane size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// Width in columns.
    pub cols: u16,
    /// Height in rows.
    pub rows: u16,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.rows)
    }

    fn columns(&self) -> usize {
        usize::from(self.cols)
    }
}

impl From<Size> for PtySize {
    fn from(s: Size) -> Self {
        PtySize {
            rows: s.rows,
            cols: s.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// Shared handle to the PTY writer; the pump thread and the UI both write.
type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// Locks a mutex, recovering the data if another thread panicked while holding it.
fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Emulator event sink that writes query replies straight back to the PTY.
///
/// It runs on the pump thread while the [`Term`] lock is held, so it must never
/// touch the terminal itself.
pub struct Replier {
    writer: Writer,
}

impl EventListener for Replier {
    /// Answers DSR/DA (`PtyWrite`) and OSC 10/11/12 (`ColorRequest`); ignores the rest.
    ///
    /// Colour replies use fixed defaults rather than the host terminal's
    /// palette, because asking the host would need a round trip.
    fn send_event(&self, event: Event) {
        let reply = match event {
            Event::PtyWrite(s) => s,
            Event::ColorRequest(index, format) => {
                let rgb = if index == NamedColor::Background as usize {
                    DEFAULT_BG
                } else {
                    DEFAULT_FG
                };
                format(rgb)
            }
            _ => return,
        };
        let mut w = lock(&self.writer);
        // reason: the child may already be gone; a lost reply is harmless then.
        let _ = w.write_all(reply.as_bytes()).and_then(|()| w.flush());
    }
}

/// Emulator plus PTY state that the pump thread publishes to the UI.
pub struct Shared {
    /// The terminal emulator. Lock order: `term` before `writer`.
    pub term: Mutex<Term<Replier>>,
    /// When the pump thread hit EOF, i.e. all output after exit was consumed.
    pub drained_at: Mutex<Option<Instant>>,
}

/// One running child in a PTY with its emulator.
pub struct Session {
    /// Emulator and pump state shared with the reader thread.
    pub shared: Arc<Shared>,
    writer: Writer,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// When the child was spawned.
    pub started: Instant,
}

impl Session {
    /// Spawns `cmd` inside a new PTY and starts the pump thread.
    ///
    /// The pump reads PTY output continuously and feeds it into the
    /// emulator, so terminal queries are answered even while the caller is
    /// busy or asleep.
    ///
    /// # Arguments
    ///
    /// * `cmd`  - Program and arguments; spawned directly, not via a shell.
    /// * `cwd`  - Working directory for the child.
    /// * `env`  - Extra environment variables layered on the inherited ones.
    /// * `size` - Initial pane size.
    ///
    /// # Errors
    ///
    /// Fails if `cmd` is empty, the PTY cannot be opened, or the program
    /// cannot be spawned (e.g. it is not on `PATH`).
    pub fn spawn(cmd: &[String], cwd: &Path, env: &[(String, String)], size: Size) -> Result<Self> {
        let (prog, args) = cmd.split_first().context("empty command")?;
        let pair = native_pty_system()
            .openpty(size.into())
            .context("opening pty")?;
        let mut builder = CommandBuilder::new(prog);
        builder.args(args);
        builder.cwd(cwd);
        builder.env("TERM", "xterm-256color");
        for (k, v) in env {
            builder.env(k, v);
        }
        let started = Instant::now();
        let child = pair
            .slave
            .spawn_command(builder)
            .with_context(|| format!("spawning {prog}"))?;
        drop(pair.slave);

        let writer: Writer = Arc::new(Mutex::new(pair.master.take_writer().context("pty writer")?));
        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            ..Config::default()
        };
        let term = Term::new(
            config,
            &size,
            Replier {
                writer: Arc::clone(&writer),
            },
        );
        let shared = Arc::new(Shared {
            term: Mutex::new(term),
            drained_at: Mutex::new(None),
        });
        let reader = pair.master.try_clone_reader().context("pty reader")?;
        let pump = Arc::clone(&shared);
        thread::spawn(move || pump_output(reader, &pump));

        Ok(Self {
            shared,
            writer,
            master: pair.master,
            child,
            started,
        })
    }

    /// Writes raw bytes to the child's PTY.
    ///
    /// # Errors
    ///
    /// Fails if the PTY is closed.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut w = lock(&self.writer);
        w.write_all(bytes)
            .and_then(|()| w.flush())
            .context("writing to pty")
    }

    /// Resizes the emulator and the PTY (which sends SIGWINCH to the child).
    ///
    /// # Errors
    ///
    /// Fails if the kernel rejects the new PTY size.
    pub fn resize(&self, size: Size) -> Result<()> {
        lock(&self.shared.term).resize(size);
        self.master.resize(size.into()).context("resizing pty")
    }

    /// Returns the emulator's current mode flags.
    #[must_use]
    pub fn mode(&self) -> TermMode {
        *lock(&self.shared.term).mode()
    }

    /// Locks and returns the emulator for reading or scrolling.
    pub fn term(&self) -> MutexGuard<'_, Term<Replier>> {
        lock(&self.shared.term)
    }

    /// Reports whether the child has exited, without blocking.
    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Kills the child; a no-op if it has already exited.
    pub fn kill(&mut self) {
        // reason: fails only when the child is already dead, which is the goal.
        let _ = self.child.kill();
    }
}

/// Feeds PTY output into the emulator until EOF, then records the drain time.
///
/// # Arguments
///
/// * `reader` - Read side of the PTY master.
/// * `shared` - Emulator to feed and the slot for the drain timestamp.
fn pump_output(mut reader: Box<dyn Read + Send>, shared: &Shared) {
    let mut parser: Processor = Processor::new();
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => parser.advance(&mut *lock(&shared.term), &buf[..n]),
        }
    }
    *lock(&shared.drained_at) = Some(Instant::now());
}
