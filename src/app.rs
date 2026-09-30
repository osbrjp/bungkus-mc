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
pub(crate) mod stop;

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

use crate::agent::{self, Kind, find_on_path};
use crate::app::model::{Alert, Cmd, LaunchRequest, Model};
use crate::app::sessions::{Card, State};
use crate::ipc::server;
use crate::store::config::{self, Config, Notify};
use crate::term::session::{Colors, Session, child_env};
use crate::term::{PtyEvent, SessionId};
use crate::ui::sanitise::sanitise;
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
    /// One line from the hook socket, parsed on the UI thread.
    Hook(Vec<u8>),
    /// Usage from a session's Codex rollout reader.
    Usage(crate::term::SessionId, crate::agent::usage::Usage),
    /// A background process snapshot.
    Procs(Vec<crate::proc::Proc>),
    /// The host terminal went away (input closed).
    HostGone,
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
        // XTWINOPS: restore the title saved at start (DESIGN §9).
        write_host(b"\x1b[23;0t");
    }
}

/// Writes mc's own control bytes (bell, title, notifications) to the host
/// terminal; never agent bytes.
fn write_host(bytes: &[u8]) {
    let mut out = io::stdout();
    // reason: a terminal that is gone cannot be told anything.
    let _ = out.write_all(bytes).and_then(|()| out.flush());
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
    /// The config as loaded (agent commands, mouse, notify).
    pub config: Config,
    /// Where `sessions.json` lives; `None` when no home directory is known.
    pub state_path: Option<PathBuf>,
}

/// What launches need besides the model: the socket for hooks and mc's
/// own path for the hook command.
#[derive(Debug)]
struct Hooks {
    socket: PathBuf,
    exe: PathBuf,
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
    let uid = rustix::process::getuid().as_raw();
    let dir = server::socket_dir(|n| std::env::var(n).ok(), uid);
    let listener = server::start(&dir, uid, tx.clone(), AppEvent::Hook);
    let exe = std::env::current_exe().and_then(|p| p.canonicalize());
    let hooks = match (&listener, exe) {
        (Ok(server), Ok(exe)) => Some(Hooks {
            socket: server.path.clone(),
            exe,
        }),
        (Err(e), _) => {
            model.message = Some(format!("No live tree this run ({e}); output still works."));
            None
        }
        (_, Err(_)) => None,
    };
    write_host(b"\x1b[22;0t");
    let mut title = String::new();
    let input = tx.clone();
    thread::spawn(move || {
        while let Ok(event) = event::read() {
            if input.send(AppEvent::Input(event)).is_err() {
                return;
            }
        }
        // reason: the loop may already be gone; nothing else to tell.
        let _ = input.send(AppEvent::HostGone);
    });

    let mut next_tick = Instant::now() + TICK;
    loop {
        model.now = Instant::now();
        model.unix_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        resize_sessions(&mut model);
        terminal.draw(|frame| ui::draw(frame, &mut model))?;
        announce(&mut model, env.config.notify, &mut title);
        if model.state_dirty {
            save_state(&mut model, env);
        }
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
            match run_cmd(&mut model, env, hooks.as_ref(), &tx, uid, cmd) {
                Next::Quit => {
                    save_state(&mut model, env);
                    return Ok(());
                }
                Next::Redraw => terminal.clear()?,
                Next::Continue => {}
            }
        }
    }
}

/// What the loop does after a command.
enum Next {
    /// Keep going.
    Continue,
    /// Clear the terminal first.
    Redraw,
    /// Leave mc.
    Quit,
}

/// Runs one command `update` returned.
fn run_cmd(
    model: &mut Model,
    env: &Env,
    hooks: Option<&Hooks>,
    tx: &SyncSender<AppEvent>,
    uid: u32,
    cmd: Cmd,
) -> Next {
    match cmd {
        Cmd::Quit => return Next::Quit,
        Cmd::Redraw => return Next::Redraw,
        Cmd::Apply(settings) => apply(model, env, settings),
        Cmd::Launch(request) => launch(model, env, hooks, request, tx),
        Cmd::WatchRollout(id, path) => watch_rollout(model, id, &path, tx),
        Cmd::Scan => {
            let tx = tx.clone();
            thread::spawn(move || {
                // reason: a closed loop just loses this snapshot.
                let _ = tx.send(AppEvent::Procs(crate::proc::snapshot(uid)));
            });
        }
        Cmd::OpenStop(kind) => {
            let snapshot = crate::proc::snapshot(uid);
            model.track(&snapshot);
            let pids: Vec<i32> = model
                .cards
                .iter()
                .flat_map(|c| c.descendants.procs.iter().map(|p| p.pid))
                .collect();
            let ports = crate::proc::ports::listening(&pids);
            if let Some(Cmd::Quit) = model.open_stop(kind, &snapshot, &ports) {
                return Next::Quit;
            }
        }
        Cmd::Signal(procs, signal) => {
            for proc in &procs {
                if let Err(crate::proc::kill::KillError::Denied) =
                    crate::proc::kill::signal(proc, signal)
                {
                    model.message =
                        Some(format!("could not stop {} pid {}", proc.name(), proc.pid));
                }
            }
            if let Some(Cmd::Quit) = model.update(AppEvent::Tick) {
                return Next::Quit;
            }
        }
    }
    Next::Continue
}

/// Writes every card to `sessions.json`; a failure is shown once and
/// retried on the next change.
fn save_state(model: &mut Model, env: &Env) {
    model.state_dirty = false;
    let Some(path) = &env.state_path else { return };
    let records: Vec<_> = model
        .cards
        .iter()
        .map(|c| c.to_record(model.now, model.unix_now))
        .collect();
    if let Err(e) = crate::store::state::save(path, &records) {
        model.message = Some(format!("Sessions not saved: {e}"));
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
fn launch(
    model: &mut Model,
    env: &Env,
    hooks: Option<&Hooks>,
    request: LaunchRequest,
    tx: &SyncSender<AppEvent>,
) {
    let LaunchRequest {
        project,
        kind,
        mut launch,
        replaces,
    } = request;
    if let (Some(hooks), Kind::Codex) = (hooks, kind) {
        launch.hook_args = agent::codex::hook_args(&hooks.exe);
    }
    let mut user_line = None;
    if let (Some(hooks), Kind::Claude) = (hooks, kind) {
        let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map_or_else(
            || model.home.clone().unwrap_or_default().join(".claude"),
            PathBuf::from,
        );
        user_line = agent::claude::user_statusline(&project, &config_dir);
        launch.settings = Some(agent::claude::settings(&hooks.exe, user_line.as_ref()));
    }
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
    let mut extra = vec![("BUNGKUS_MC_SESSION", OsStr::new(&id))];
    if let Some(hooks) = hooks {
        extra.push(("BUNGKUS_MC_SOCK", hooks.socket.as_os_str()));
    }
    if let Some(line) = &user_line {
        extra.push(("BUNGKUS_MC_USER_STATUSLINE", OsStr::new(&line.command)));
    }
    let child = child_env(std::env::vars_os(), &extra);
    if launch.settings.is_some() || !launch.hook_args.is_empty() {
        card.expect_hooks();
    }
    card.agent_session.clone_from(&launch.resume);
    if let Some(old) = replaces {
        model.cards.retain(|c| c.id != old);
    }
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
        Ok(session) => {
            card.pid = session.pid();
            card.pty = Some(session);
        }
        Err(e) => {
            card.state = State::Failed(format!("could not start {command}: {e}"));
            card.ended = Some(model.now);
        }
    }
    model.add_card(card);
}

/// Sends pending alerts and keeps the terminal title on the tally
/// (DESIGN §9): BEL for `bell`, plus OSC 9 / 99 / 777 for `desktop`
/// depending on the terminal; inside a multiplexer only the bell.
fn announce(model: &mut Model, notify: Notify, title: &mut String) {
    let tally = ui::title(model);
    if *title != tally {
        write_host(format!("\x1b]2;{tally}\x07").as_bytes());
        *title = tally;
    }
    for alert in model.alerts.drain(..) {
        let text = match &alert {
            Alert::NeedsYou(t) | Alert::Failed(t) => sanitise(t, 120),
        };
        match notify {
            Notify::Off => {}
            Notify::Bell => write_host(b"\x07"),
            Notify::Desktop => {
                write_host(b"\x07");
                write_host(desktop_notification(&text, |n| std::env::var(n).ok()).as_bytes());
            }
        }
    }
}

/// Returns the desktop-notification escape for this terminal, or nothing
/// inside a multiplexer (DESIGN §9).
fn desktop_notification(text: &str, var: impl Fn(&str) -> Option<String>) -> String {
    if var("TMUX").is_some() || var("ZELLIJ").is_some() {
        return String::new();
    }
    let program = var("TERM_PROGRAM").unwrap_or_default();
    let term = var("TERM").unwrap_or_default();
    if term.contains("kitty") {
        format!("\x1b]99;;{text}\x1b\\")
    } else if ["iTerm.app", "WezTerm", "ghostty", "vscode"].contains(&program.as_str()) {
        format!("\x1b]9;{text}\x07")
    } else {
        format!("\x1b]777;notify;bungkus-mc;{text}\x07")
    }
}

/// Starts the Codex usage reader for a session, if its rollout path passes
/// validation (ARCHITECTURE §6.3); otherwise the card keeps showing `-`.
fn watch_rollout(model: &mut Model, id: SessionId, path: &Path, tx: &SyncSender<AppEvent>) {
    let Some(home) = agent::codex_usage::codex_home(|n| std::env::var(n).ok()) else {
        return;
    };
    let Ok(file) = agent::codex_usage::open(path, &home) else {
        return;
    };
    let (stop, stopped) = mpsc::channel();
    agent::codex_usage::watch(file, stopped, tx.clone(), move |usage| {
        AppEvent::Usage(id, usage)
    });
    if let Some(card) = model.card_mut(id) {
        card.rollout_stop = Some(stop);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_desktop_notification_per_terminal() {
        let env = |pairs: &'static [(&str, &str)]| {
            move |n: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == n)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let cases: [(&'static [(&str, &str)], &str); 4] = [
            (&[("TERM", "xterm-kitty")], "\x1b]99;;hi\x1b\\"),
            (&[("TERM_PROGRAM", "ghostty")], "\x1b]9;hi\x07"),
            (&[("TERM", "foot")], "\x1b]777;notify;bungkus-mc;hi\x07"),
            (&[("TMUX", "/tmp/x"), ("TERM_PROGRAM", "iTerm.app")], ""),
        ];
        for (vars, want) in cases {
            assert_eq!(desktop_notification("hi", env(vars)), want, "{vars:?}");
        }
    }
}
