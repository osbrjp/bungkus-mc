//! The event loop: owns the terminal and the [`Model`], reads input,
//! renders, and runs the [`Cmd`]s `update` returns.
//!
//! The terminal is restored by [`TerminalGuard`] on every exit path and by
//! ratatui's panic hook on a panic. Rendering happens only after an input
//! event, never on a timer.

pub(crate) mod form;
pub(crate) mod model;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::crossterm::event;
use rustix::event::{PollFd, PollFlags, Timespec, poll};

use crate::app::model::{Cmd, Model};
use crate::store::config;
use crate::ui::{self, theme};
use crate::workspace;

/// How long to wait for the terminal's OSC 11 reply (ARCHITECTURE §3.1).
const OSC_REPLY_BUDGET: Duration = Duration::from_millis(200);

/// Restores the host terminal (raw mode off, main screen) when dropped.
#[derive(Debug)]
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// What the loop needs besides the model.
#[derive(Debug)]
pub(crate) struct Env {
    /// Where settings are saved; `None` when no home directory is known.
    pub config_path: Option<PathBuf>,
    /// mc's working directory.
    pub cwd: PathBuf,
    /// Workspace field prefill for the wizard, when it runs.
    pub wizard_prefill: Option<String>,
}

/// Runs the TUI until the user quits.
///
/// Enters raw mode and the alternate screen (ratatui also installs a panic
/// hook that restores both), asks the terminal for its background colour,
/// then draws and waits for input in turn.
///
/// # Arguments
///
/// * `model` - The initial model; its settings are `None` for first run.
/// * `env`   - Paths the loop saves to and scans from.
///
/// # Errors
///
/// Returns the I/O error if the terminal cannot be set up, drawn to or read
/// from. Saving or scanning errors are shown in the UI instead.
pub(crate) fn run(mut model: Model, env: &Env) -> io::Result<()> {
    let _guard = TerminalGuard;
    let mut terminal = ratatui::try_init()?;
    model.host_light = query_host_background();
    if let Some(settings) = &model.settings {
        model.theme = model
            .theme
            .with_name(settings.theme.resolve(model.host_light));
    }
    if let Some(prefill) = &env.wizard_prefill {
        model.start_wizard(prefill);
    }
    // ponytail: input is read on the UI thread; the AppEvent channel of
    // ARCHITECTURE §8 replaces this once a second event source exists.
    loop {
        terminal.draw(|frame| ui::draw(frame, &mut model))?;
        let event = event::read()?;
        match model.update(&event) {
            None => {}
            Some(Cmd::Quit) => return Ok(()),
            Some(Cmd::Redraw) => terminal.clear()?,
            Some(Cmd::Apply(settings)) => {
                if let Some(path) = &env.config_path
                    && let Err(e) = config::save(path, &settings)
                {
                    model.message = Some(format!("Settings not saved: {e}"));
                }
                let scan = workspace::scan(&settings.workspace);
                model.apply(settings, scan, &env.cwd);
            }
        }
    }
}

/// Asks the terminal for its background colour (OSC 11) and returns
/// whether it is light; `None` when it does not answer in time.
///
/// Must run in raw mode and before the first `event::read`, so the reply
/// is not mistaken for key presses (ARCHITECTURE §3.1). Reads the tty with
/// `rustix` directly because std's buffered stdin would keep bytes that
/// crossterm then never sees.
fn query_host_background() -> Option<bool> {
    let mut out = io::stdout();
    out.write_all(b"\x1b]11;?\x1b\\").ok()?;
    out.flush().ok()?;
    let stdin = io::stdin();
    let deadline = Instant::now() + OSC_REPLY_BUDGET;
    let mut reply = Vec::new();
    let mut chunk = [0_u8; 64];
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: i64::from(u32::try_from(left.as_nanos()).unwrap_or(u32::MAX)),
        };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        if poll(&mut fds, Some(&timeout)).ok()? == 0 {
            break;
        }
        let n = rustix::io::read(&stdin, &mut chunk).ok()?;
        reply.extend_from_slice(&chunk[..n]);
        if n == 0 || reply.ends_with(b"\x07") || reply.ends_with(b"\x1b\\") {
            break;
        }
    }
    theme::is_light_reply(&String::from_utf8_lossy(&reply))
}

/// Returns `path` as an absolute workspace: relative paths are taken from
/// `cwd`, `~` from `home`.
#[must_use]
pub(crate) fn absolute(path: &str, cwd: &Path, home: Option<&Path>) -> PathBuf {
    config::expand(path, home).unwrap_or_else(|| cwd.join(path))
}
