//! The event loop: owns the terminal and the [`Model`], receives every
//! [`AppEvent`] on one channel, renders, and runs the [`Cmd`]s `update`
//! returns (ARCHITECTURE §3, §8).
//!
//! The terminal is restored by [`TerminalGuard`] on every exit path and by
//! the panic hook on a panic. The loop sleeps until an event or the next
//! deadline (a synchronized update, a stop grace, the 350 ms animation
//! tick); it never renders on a fixed timer.

pub(crate) mod form;
mod interact;
pub(crate) mod model;
pub(crate) mod picker;
pub(crate) mod sessions;

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use rustix::event::{PollFd, PollFlags, Timespec, poll};

use crate::agent::{self, find_on_path};
use crate::app::model::{Cmd, LaunchRequest, Model};
use crate::app::sessions::{Card, State};
use crate::store::config::{self, Config};
use crate::term::PtyEvent;
use crate::term::session::{Colors, Session, child_env};
use crate::ui::{self, theme};
use crate::workspace;

/// How long to wait for the terminal's OSC 11 reply (ARCHITECTURE §3.1).
const OSC_REPLY_BUDGET: Duration = Duration::from_millis(200);

/// The animation tick (DESIGN §7).
const TICK: Duration = Duration::from_millis(350);

/// Capacity of the event channel: 64 PTY reads of 32 KiB (ARCHITECTURE §3).
const CHANNEL_CAPACITY: usize = 64;

/// Everything the loop wakes up for.
#[derive(Debug)]
pub(crate) enum AppEvent {
    /// A key, paste, mouse or resize event from the host terminal.
    Input(Event),
    /// Output or exit from a session.
    Pty(PtyEvent),
    /// A deadline passed: animation frame, sync flush or stop grace.
    Tick,
}

impl From<PtyEvent> for AppEvent {
    fn from(event: PtyEvent) -> Self {
        Self::Pty(event)
    }
}

/// Restores the host terminal when dropped: kitty flags, mouse, paste,
/// raw mode and the alternate screen.
#[derive(Debug)]
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        release_host_modes();
        ratatui::restore();
    }
}

/// Undoes the host-terminal modes mc turned on besides raw mode and the
/// alternate screen; harmless when they were never on.
fn release_host_modes() {
    // reason: nothing useful can be done if the terminal is already gone.
    let _ = execute!(
        io::stdout(),
        PopKeyboardEnhancementFlags,
        DisableMouseCapture,
        DisableBracketedPaste
    );
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
    /// The config as loaded (agent commands, mouse).
    pub config: Config,
}

/// Runs the TUI until the user quits.
///
/// Enters raw mode and the alternate screen, turns on mouse capture,
/// bracketed paste and (where supported) kitty key disambiguation, asks
/// the terminal for its background colour, starts the input thread, then
/// loops: wait for an event or deadline, update, render.
///
/// # Errors
///
/// Returns the I/O error if the terminal cannot be set up, drawn to or read
/// from. Saving, scanning or spawn errors are shown in the UI instead.
pub(crate) fn run(mut model: Model, env: &Env) -> io::Result<()> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        release_host_modes();
        previous(info);
    }));
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
    if env.config.mouse {
        execute!(io::stdout(), EnableMouseCapture)?;
    }
    execute!(io::stdout(), EnableBracketedPaste)?;
    if supports_keyboard_enhancement().unwrap_or(false) {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        execute!(io::stdout(), PushKeyboardEnhancementFlags(flags))?;
    }
    let size = terminal.size()?;
    model.screen = ratatui::layout::Rect::new(0, 0, size.width, size.height);

    let (tx, rx) = mpsc::sync_channel::<AppEvent>(CHANNEL_CAPACITY);
    let input = tx.clone();
    thread::spawn(move || {
        while let Ok(event) = event::read() {
            if input.send(AppEvent::Input(event)).is_err() {
                break;
            }
        }
    });

    let mut next_tick = Instant::now() + TICK;
    loop {
        model.now = Instant::now();
        resize_sessions(&mut model);
        terminal.draw(|frame| ui::draw(frame, &mut model))?;
        let first = match model.deadline(next_tick) {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => AppEvent::Tick,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            },
            None => rx
                .recv()
                .map_err(|_| io::Error::other("event channel closed"))?,
        };
        if Instant::now() >= next_tick {
            next_tick = Instant::now() + TICK;
        }
        for event in std::iter::once(first).chain(rx.try_iter()) {
            model.now = Instant::now();
            for card in &mut model.cards {
                if let Some(pty) = card.pty.as_mut() {
                    pty.flush_sync();
                }
            }
            let Some(cmd) = model.update(event) else {
                continue;
            };
            match cmd {
                Cmd::Quit => return Ok(()),
                Cmd::Redraw => terminal.clear()?,
                Cmd::Apply(settings) => apply(&mut model, env, settings),
                Cmd::Launch(request) => launch(&mut model, env, request, &tx),
            }
        }
    }
}

/// Saves settings, rescans the workspace and recolours running sessions.
fn apply(model: &mut Model, env: &Env, settings: config::Settings) {
    if let Some(path) = &env.config_path
        && let Err(e) = config::save(path, &settings)
    {
        model.message = Some(format!("Settings not saved: {e}"));
    }
    let scan = workspace::scan(&settings.workspace);
    model.apply(settings, scan, &env.cwd);
    let colors = colors(model.theme);
    for pty in model.cards.iter_mut().filter_map(|c| c.pty.as_mut()) {
        pty.set_colors(colors);
    }
}

/// Starts a session for `request` and adds its card; a failure to start
/// becomes a failed card with the reason.
fn launch(model: &mut Model, env: &Env, request: LaunchRequest, tx: &SyncSender<AppEvent>) {
    let LaunchRequest {
        project,
        kind,
        launch,
    } = request;
    let mut card = Card::new(
        launch.id,
        kind,
        project.clone(),
        launch.name.as_deref(),
        launch.prompt.as_deref(),
        model.now,
    );
    let agent = env.config.agents.get(kind);
    let command = agent
        .command
        .clone()
        .unwrap_or_else(|| kind.command().to_owned());
    let path = std::env::var_os("PATH").unwrap_or_default();
    let program = if command.contains('/') {
        Some(PathBuf::from(&command))
    } else {
        find_on_path(&command, &path)
    };
    let Some(program) = program else {
        model.message = Some(format!(
            "{command} not found on PATH. Install {}, then n again.",
            kind.product()
        ));
        return;
    };
    let argv = agent::argv(kind, &program, &agent.args, &launch);
    let id = launch.id.0.hyphenated().to_string();
    let child = child_env(
        std::env::vars_os(),
        &[("BUNGKUS_MC_SESSION", OsStr::new(&id))],
    );
    let size = ui::output_size(model.screen, model.zoom);
    match Session::spawn(
        launch.id,
        &argv,
        &project,
        &child,
        size,
        colors(model.theme),
        tx,
    ) {
        Ok(session) => card.pty = Some(session),
        Err(e) => {
            card.state = State::Failed(format!("could not start {command}: {e}"));
            card.ended = Some(model.now);
        }
    }
    model.add_card(card);
}

/// Keeps every session's emulator and PTY at the output pane's size.
fn resize_sessions(model: &mut Model) {
    let size = ui::output_size(model.screen, model.zoom);
    for pty in model
        .cards
        .iter_mut()
        .filter(|c| c.running())
        .filter_map(|c| c.pty.as_mut())
    {
        pty.resize(size);
    }
}

/// Returns the colours the agent sees for OSC 10/11: the theme's painted
/// foreground and background (ARCHITECTURE §3.1).
fn colors(theme: theme::Theme) -> Colors {
    let rgb = |hex: u32| {
        let [_, r, g, b] = hex.to_be_bytes();
        alacritty_terminal::vte::ansi::Rgb { r, g, b }
    };
    Colors {
        fg: rgb(theme::spec(theme.name, theme::Token::Fg).painted),
        bg: rgb(theme::bg(theme.name)),
    }
}

/// Asks the terminal for its background colour (OSC 11) and returns
/// whether it is light; `None` when it does not answer in time.
///
/// Must run in raw mode and before the input thread starts, so the reply
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
