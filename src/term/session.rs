//! One agent session: a child in a PTY, its emulator, and the threads that
//! move bytes (ARCHITECTURE §4.1, §8).
//!
//! The emulator lives here and is advanced and read only on the UI thread.
//! A reader thread sends PTY output as [`PtyEvent::Output`] on a bounded
//! channel (backpressure), a writer thread owns the PTY writer so the UI
//! never blocks on it, and a waiter thread reports the exit. This module
//! never parses what the agent prints; it only emulates it.

use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, Osc52, TermMode};
use alacritty_terminal::vte::ansi::{NamedColor, Processor, Rgb};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use rustix::process::{Pid, Signal, kill_process_group};

use crate::term::{PtyEvent, SessionId};

/// Scrollback per session, per ARCHITECTURE §4.1.
const SCROLLBACK_LINES: usize = 10_000;

/// Size of one PTY read, per ARCHITECTURE §3 (64 × 32 KiB channel).
const READ_CHUNK: usize = 32 * 1024;

/// How long the waiter waits for the reader to drain after the child
/// exited, per ARCHITECTURE §4.1 "child exit rule".
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// Nominal cell size in pixels for CSI 14 t replies; mc cannot know the
/// host's real font size.
const CELL_PIXELS: (u16, u16) = (8, 16);

/// Environment variables never passed to an agent: host-terminal identity
/// (the child talks to mc's emulator), mc's routing key, and the Claude
/// Code / Codex session markers (ARCHITECTURE §3.1, SECURITY.md).
const ENV_DENY: &[&str] = &[
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "KITTY_WINDOW_ID",
    "TMUX",
    "TMUX_PANE",
    "TYPESAFE_API_KEY",
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_EFFORT",
    "CLAUDE_PID",
];

/// Prefixes of host-terminal identity variables that are dropped too.
const ENV_DENY_PREFIX: &[&str] = &["WEZTERM_", "ITERM_"];

/// Pane size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Size {
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
        Self {
            rows: s.rows,
            cols: s.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// The default colours the agent sees for OSC 10/11 queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Colors {
    /// Default foreground.
    pub fg: Rgb,
    /// Default background.
    pub bg: Rgb,
}

/// State the emulator's listener shares with the session.
#[derive(Debug)]
struct Shared {
    colors: Cell<Colors>,
    size: Cell<Size>,
}

/// Receives the emulator's events and answers the agent's queries through
/// the writer thread; everything else (title, clipboard, bell) is dropped
/// (SECURITY.md "Agent output rendering").
#[derive(Debug)]
pub(crate) struct Listener {
    writer: Sender<Vec<u8>>,
    shared: Rc<Shared>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let reply = match event {
            Event::PtyWrite(text) => text,
            Event::ColorRequest(index, format) => {
                let colors = self.shared.colors.get();
                if index == NamedColor::Background as usize {
                    format(colors.bg)
                } else if index == NamedColor::Foreground as usize
                    || index == NamedColor::Cursor as usize
                {
                    format(colors.fg)
                } else {
                    return;
                }
            }
            Event::TextAreaSizeRequest(format) => {
                let size = self.shared.size.get();
                format(WindowSize {
                    num_lines: size.rows,
                    num_cols: size.cols,
                    cell_width: CELL_PIXELS.0,
                    cell_height: CELL_PIXELS.1,
                })
            }
            _ => return,
        };
        // reason: the writer is gone only after the session was dropped.
        let _ = self.writer.send(reply.into_bytes());
    }
}

/// The OS side of a session: the PTY master and the child's process group.
struct Pty {
    master: Box<dyn MasterPty + Send>,
    pid: Option<Pid>,
}

impl std::fmt::Debug for Pty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pty")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

/// One running (or finished) agent in a PTY with its emulator.
pub(crate) struct Session {
    term: Term<Listener>,
    processor: Processor,
    writer: Sender<Vec<u8>>,
    shared: Rc<Shared>,
    pty: Option<Pty>,
    /// When the agent last wrote output; `None` before the first byte.
    pub last_output: Option<Instant>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("pty", &self.pty)
            .finish_non_exhaustive()
    }
}

/// Why a session could not start.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SpawnError {
    /// The argument vector was empty.
    #[error("no command to run")]
    NoCommand,
    /// Opening the PTY or spawning the program failed.
    #[error("{0}")]
    Pty(String),
}

impl Session {
    /// Spawns `argv` in a new PTY in `cwd` and starts its threads.
    ///
    /// The child gets exactly `env` (see [`child_env`]) and becomes the
    /// leader of its own session, so its process group id is its pid.
    ///
    /// # Arguments
    ///
    /// * `id`     - The session's id, echoed on every event.
    /// * `argv`   - Program (resolved path) and arguments; never a shell.
    /// * `cwd`    - Working directory; must exist.
    /// * `env`    - The complete child environment.
    /// * `size`   - Initial pane size.
    /// * `colors` - Default colours for OSC 10/11 replies.
    /// * `events` - The event loop's bounded channel.
    ///
    /// # Errors
    ///
    /// * [`SpawnError::NoCommand`] - `argv` is empty.
    /// * [`SpawnError::Pty`] - the PTY could not be opened or the program
    ///   could not be spawned.
    pub(crate) fn spawn<E: From<PtyEvent> + Send + 'static>(
        id: SessionId,
        argv: &[OsString],
        cwd: &Path,
        env: &[(OsString, OsString)],
        size: Size,
        colors: Colors,
        events: &SyncSender<E>,
    ) -> Result<Self, SpawnError> {
        let (program, rest) = argv.split_first().ok_or(SpawnError::NoCommand)?;
        let pty = |e: &dyn std::fmt::Display| SpawnError::Pty(e.to_string());
        let pair = native_pty_system()
            .openpty(size.into())
            .map_err(|e| pty(&e))?;
        let mut command = CommandBuilder::new(program);
        command.args(rest);
        command.cwd(cwd);
        command.env_clear();
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = pair.slave.spawn_command(command).map_err(|e| pty(&e))?;
        drop(pair.slave);
        let pid = child
            .process_id()
            .and_then(|p| i32::try_from(p).ok())
            .and_then(Pid::from_raw);
        let mut reader = pair.master.try_clone_reader().map_err(|e| pty(&e))?;
        let mut pty_writer = pair.master.take_writer().map_err(|e| pty(&e))?;

        let (session, writes) = Self::detached(size, colors);
        let session = Self {
            pty: Some(Pty {
                master: pair.master,
                pid,
            }),
            ..session
        };

        thread::spawn(move || {
            for bytes in writes {
                // reason: EIO after the child exited is expected (spike finding).
                let _ = pty_writer
                    .write_all(&bytes)
                    .and_then(|()| pty_writer.flush());
            }
        });
        let (drained_tx, drained_rx) = mpsc::channel::<()>();
        let output = events.clone();
        thread::spawn(move || {
            let _drained = drained_tx;
            let mut buf = vec![0_u8; READ_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if output
                            .send(PtyEvent::Output(id, buf[..n].to_vec()).into())
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
        let exits = events.clone();
        thread::spawn(move || {
            let code = child
                .wait()
                .ok()
                .filter(|s| s.signal().is_none())
                .map(|s| s.exit_code());
            // reason: a timeout just means a descendant still holds the PTY.
            let _ = drained_rx.recv_timeout(DRAIN_GRACE);
            let _ = exits.send(PtyEvent::Exited(id, code).into());
        });
        Ok(session)
    }

    /// Creates a session with an emulator but no process; the returned
    /// receiver gets everything that would be written to the PTY.
    #[must_use]
    pub(crate) fn detached(size: Size, colors: Colors) -> (Self, Receiver<Vec<u8>>) {
        let (writer, outbox) = mpsc::channel();
        let shared = Rc::new(Shared {
            colors: Cell::new(colors),
            size: Cell::new(size),
        });
        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            kitty_keyboard: true,
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        let listener = Listener {
            writer: writer.clone(),
            shared: Rc::clone(&shared),
        };
        let session = Self {
            term: Term::new(config, &size, listener),
            processor: Processor::new(),
            writer,
            shared,
            pty: None,
            last_output: None,
        };
        (session, outbox)
    }

    /// Feeds agent output into the emulator.
    pub(crate) fn advance(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
        self.last_output = Some(Instant::now());
    }

    /// Returns when a pending synchronized update must be flushed, if one
    /// is pending (DEC 2026; ARCHITECTURE §4.1).
    #[must_use]
    pub(crate) fn sync_deadline(&self) -> Option<Instant> {
        self.processor.sync_timeout().sync_timeout()
    }

    /// Ends a synchronized update whose deadline passed, so a lost ESU can
    /// never freeze the pane.
    pub(crate) fn flush_sync(&mut self) {
        if self.sync_deadline().is_some_and(|at| at <= Instant::now()) {
            self.processor.stop_sync(&mut self.term);
        }
    }

    /// Sends bytes to the agent through the writer thread; never blocks.
    pub(crate) fn send(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            // reason: the writer thread only ends when the session is dropped.
            let _ = self.writer.send(bytes);
        }
    }

    /// Returns the agent's current terminal modes.
    #[must_use]
    pub(crate) fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    /// Returns the emulator, for rendering.
    #[must_use]
    pub(crate) const fn term(&self) -> &Term<Listener> {
        &self.term
    }

    /// Scrolls the view into scrollback (`delta` > 0 is up) or back down.
    pub(crate) fn scroll(&mut self, scroll: Scroll) {
        self.term.scroll_display(scroll);
    }

    /// Resizes the emulator and the PTY (which signals the child).
    pub(crate) fn resize(&mut self, size: Size) {
        if self.shared.size.get() == size {
            return;
        }
        self.shared.size.set(size);
        self.term.resize(size);
        if let Some(pty) = &self.pty {
            // reason: fails only after the child is gone; nothing to resize then.
            let _ = pty.master.resize(size.into());
        }
    }

    /// Changes the colours reported to OSC 10/11 queries (theme switch).
    pub(crate) fn set_colors(&self, colors: Colors) {
        self.shared.colors.set(colors);
    }

    /// Sends `signal` to the agent's process group.
    ///
    /// # Errors
    ///
    /// Returns the OS error (for example `ESRCH` when it already exited).
    pub(crate) fn signal(&self, signal: Signal) -> std::io::Result<()> {
        match self.pty.as_ref().and_then(|p| p.pid) {
            Some(pid) => kill_process_group(pid, signal).map_err(std::io::Error::from),
            None => Ok(()),
        }
    }

    /// Returns the last non-empty line on screen, trimmed; the "reason"
    /// shown for a failed session.
    #[must_use]
    pub(crate) fn last_line(&self) -> String {
        let grid = self.term.grid();
        (0..grid.screen_lines())
            .rev()
            .map(|y| {
                let line = Line(i32::try_from(y).unwrap_or(i32::MAX));
                (0..grid.columns())
                    .map(|x| grid[Point::new(line, Column(x))].c)
                    .collect::<String>()
                    .trim()
                    .to_owned()
            })
            .find(|text| !text.is_empty())
            .unwrap_or_default()
    }
}

/// Builds the complete environment for an agent from mc's own.
///
/// Drops [`ENV_DENY`] and [`ENV_DENY_PREFIX`], sets `TERM` and `COLORTERM`
/// for mc's emulator, and adds `extra`. Everything else, including user
/// settings such as `CLAUDE_CONFIG_DIR` and `CODEX_HOME`, is kept.
///
/// # Arguments
///
/// * `vars`  - mc's environment (`std::env::vars_os()`).
/// * `extra` - Variables mc adds for this session.
#[must_use]
pub(crate) fn child_env(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    extra: &[(&str, &OsStr)],
) -> Vec<(OsString, OsString)> {
    let denied = |key: &OsStr| {
        let key = key.to_string_lossy();
        key == "TERM"
            || key == "COLORTERM"
            || ENV_DENY.contains(&key.as_ref())
            || ENV_DENY_PREFIX.iter().any(|p| key.starts_with(p))
            || extra.iter().any(|(k, _)| *k == key)
    };
    let mut env: Vec<(OsString, OsString)> = vars.into_iter().filter(|(k, _)| !denied(k)).collect();
    env.push(("TERM".into(), "xterm-256color".into()));
    env.push(("COLORTERM".into(), "truecolor".into()));
    env.extend(
        extra
            .iter()
            .map(|(k, v)| (OsString::from(k), (*v).to_owned())),
    );
    env
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const COLORS: Colors = Colors {
        fg: Rgb {
            r: 0xd6,
            g: 0xe2,
            b: 0xd3,
        },
        bg: Rgb {
            r: 0x1c,
            g: 0x2a,
            b: 0x21,
        },
    };
    const SIZE: Size = Size { cols: 40, rows: 6 };

    fn replies(writes: &Receiver<Vec<u8>>) -> String {
        let mut out = Vec::new();
        while let Ok(bytes) = writes.recv_timeout(Duration::from_millis(100)) {
            out.extend(bytes);
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn answers_terminal_queries_through_the_writer() {
        let cases: &[(&[u8], &str)] = &[
            (b"ab\x1b[6n", "\x1b[1;3R"),
            (b"\x1b[c", "\x1b[?6c"),
            (b"\x1b]10;?\x1b\\", "rgb:d6d6/e2e2/d3d3"),
            (b"\x1b]11;?\x07", "rgb:1c1c/2a2a/2121"),
            (b"\x1b[14t", "\x1b[4;96;320t"),
            (b"\x1b[18t", "\x1b[8;6;40t"),
            (b"\x1b[?u", "\x1b[?0u"),
        ];
        for (input, want) in cases {
            let (mut session, writes) = Session::detached(SIZE, COLORS);
            session.advance(input);
            let got = replies(&writes);
            assert!(got.contains(want), "{input:?}: got {got:?}, want {want:?}");
        }
    }

    #[test]
    fn a_missing_esu_is_flushed_after_the_deadline() {
        let (mut session, _writes) = Session::detached(SIZE, COLORS);
        session.advance(b"\x1b[?2026hhello");
        assert_eq!(session.last_line(), "", "parked inside the update");
        let deadline = session.sync_deadline().unwrap();
        assert!(deadline <= Instant::now() + Duration::from_millis(150));
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        session.flush_sync();
        assert_eq!(session.last_line(), "hello");
        assert_eq!(session.sync_deadline(), None);
    }

    #[test]
    fn wide_characters_advance_the_cursor_by_their_width() {
        let (mut session, _writes) = Session::detached(SIZE, COLORS);
        session.advance("日本語 ⏺ ok\r\n".as_bytes());
        session.advance("e\u{301}x".as_bytes());
        let cursor = session.term().grid().cursor.point;
        assert_eq!(
            (cursor.line.0, cursor.column.0),
            (1, 2),
            "combining mark takes no column"
        );
        let first: String = (0..12)
            .map(|x| session.term().grid()[Point::new(Line(0), Column(x))].c)
            .collect();
        assert!(first.starts_with('日'), "{first:?}");
    }

    #[test]
    fn child_env_scrubs_markers_and_keeps_user_settings() {
        let host = [
            ("PATH", "/bin"),
            ("TERM", "xterm-kitty"),
            ("TERM_PROGRAM", "ghostty"),
            ("WEZTERM_PANE", "1"),
            ("TYPESAFE_API_KEY", "secret"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_CHILD_SESSION", "1"),
            ("CLAUDE_CONFIG_DIR", "/c"),
            ("CLAUDE_CODE_PROJECT_DIR_NAME", "p"),
            ("CODEX_HOME", "/x"),
            ("BUNGKUS_MC_SESSION", "stale"),
        ];
        let vars = host
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let env = child_env(vars, &[("BUNGKUS_MC_SESSION", OsStr::new("new"))]);
        let get = |k: &str| {
            env.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.to_string_lossy())
        };
        for gone in ENV_DENY.iter().chain(&["WEZTERM_PANE"]) {
            assert_eq!(get(gone), None, "{gone} must be unset");
        }
        for kept in [
            "PATH",
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_PROJECT_DIR_NAME",
            "CODEX_HOME",
        ] {
            assert!(get(kept).is_some(), "{kept} must be kept");
        }
        assert_eq!(get("TERM").as_deref(), Some("xterm-256color"));
        assert_eq!(get("COLORTERM").as_deref(), Some("truecolor"));
        assert_eq!(get("BUNGKUS_MC_SESSION").as_deref(), Some("new"));
        assert_eq!(env.iter().filter(|(k, _)| k == "TERM").count(), 1);
    }

    /// Spawns `script` under `sh` and returns its events until it exits.
    fn run_script(script: &str) -> (String, Option<u32>, Duration) {
        let (tx, rx) = mpsc::sync_channel::<PtyEvent>(64);
        let id = SessionId::new();
        let argv = [OsString::from("/bin/sh"), "-c".into(), script.into()];
        let env = child_env(std::env::vars_os(), &[]);
        let started = Instant::now();
        let mut session =
            Session::spawn(id, &argv, Path::new("/"), &env, SIZE, COLORS, &tx).unwrap();
        loop {
            match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                PtyEvent::Output(_, bytes) => session.advance(&bytes),
                PtyEvent::Exited(_, code) => return (session.last_line(), code, started.elapsed()),
            }
        }
    }

    #[test]
    fn reports_the_exit_code_after_the_output() {
        let (last, code, _) = run_script("printf 'done here'; exit 3");
        assert_eq!((last.as_str(), code), ("done here", Some(3)));
    }

    #[test]
    fn reports_exit_even_when_a_descendant_holds_the_pty() {
        let (_, code, took) = run_script("(sleep 3 &); exit 0");
        assert_eq!(code, Some(0));
        assert!(
            took < Duration::from_secs(2),
            "exit reported after {took:?}"
        );
    }
}
