//! The event loop: owns the terminal and the [`Model`], receives every
//! [`AppEvent`] on one channel, renders, and runs the [`Cmd`]s `update`
//! returns (ARCHITECTURE §3, §8).
//!
//! The terminal is restored by [`TerminalGuard`] on every exit path and by
//! the panic hook on a panic. The loop sleeps until an event or the next
//! deadline (a synchronized update, a stop grace, the 350 ms animation
//! tick); it never renders on a fixed timer.

pub(crate) mod browser;
pub(crate) mod form;
mod interact;
pub(crate) mod model;
pub(crate) mod picker;
pub(crate) mod quick;
pub(crate) mod sessions;
pub(crate) mod stop;
mod takeover;
pub(crate) mod workspaces;

use std::ffi::OsStr;
use std::io::{self, IsTerminal, Write};
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

/// How often sessions outside mc are listed (ARCHITECTURE §3.4).
const EXTERNAL_EVERY: Duration = Duration::from_secs(5);

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
    /// Agent sessions running outside mc (every [`EXTERNAL_EVERY`]).
    External(Vec<crate::external::External>),
    /// The host terminal went away (input closed).
    HostGone,
    /// A newer release exists (the daily check).
    UpdateAvailable(String),
    /// The `U` update finished: the installed tag, `None` when already
    /// latest, or why it failed.
    Updated(Result<Option<String>, String>),
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
        if std::env::var_os("KITTY_WINDOW_ID").is_some() {
            write_host(KITTY_PASS_KEYS_OFF);
        }
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
pub(crate) fn run(mut model: Model, env: &Env) -> io::Result<bool> {
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
    let (hooks, _listener) = start_background(&mut model, &tx, uid);
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
                Err(RecvTimeoutError::Disconnected) => return Ok(false),
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
                    return Ok(model.restart);
                }
                Next::Redraw => terminal.clear()?,
                Next::Continue => {}
            }
        }
    }
}

/// Starts the hook socket and the daily update check; returns what
/// launches need for hooks and the listener that must live as long as mc.
fn start_background(
    model: &mut Model,
    tx: &SyncSender<AppEvent>,
    uid: u32,
) -> (Option<Hooks>, Option<server::Server>) {
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
    if model.kitty {
        write_host(KITTY_PASS_KEYS_ON);
    }
    if std::io::stderr().is_terminal() {
        let tx = tx.clone();
        thread::spawn(move || {
            if let Some(tag) = crate::update::available(|n| std::env::var(n).ok()) {
                // reason: mc may have quit meanwhile.
                let _ = tx.send(AppEvent::UpdateAvailable(tag));
            }
        });
    }
    let claude = find_on_path(
        Kind::Claude.command(),
        &std::env::var_os("PATH").unwrap_or_default(),
    );
    let tx = tx.clone();
    thread::spawn(move || {
        while tx
            .send(AppEvent::External(crate::external::scan(
                claude.as_deref(),
                uid,
            )))
            .is_ok()
        {
            thread::sleep(EXTERNAL_EVERY);
        }
    });
    (hooks, listener.ok())
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
        Cmd::SwitchWorkspace(workspace, session) => {
            if let Some(mut settings) = model.settings.clone() {
                crate::debug_log!("switch workspace to {}", workspace.display());
                settings.workspace = workspace;
                apply(model, env, settings);
                if let Some(id) = session {
                    model.select_session(id);
                }
            }
        }
        Cmd::SaveWorkspaces => save_workspaces(model, env),
        Cmd::Update => {
            let tx = tx.clone();
            thread::spawn(move || {
                let result = crate::update::check_and_install().map_err(|e| e.to_string());
                // reason: mc may have quit meanwhile.
                let _ = tx.send(AppEvent::Updated(result));
            });
        }
        Cmd::KittyFocus(side) => kitty_focus(side),
        Cmd::NewProject(path, agent_files) => new_project(model, &path, agent_files),
        Cmd::TrashProject(paths) => trash_projects(model, &paths),
        Cmd::RestoreProject(moved) => restore_projects(model, &moved),
        Cmd::StopOutside(pid) => {
            let text = stop_outside(pid, uid);
            crate::debug_log!("stop outside {pid}: {text}");
            model.message = Some(text);
        }
        Cmd::CreateProject(id, path) => match create_project(&path) {
            Ok(()) => {
                if let Some(root) = model.root().map(Path::to_path_buf)
                    && let Ok(projects) = workspace::scan(&root)
                {
                    model.projects = projects;
                }
                if let Some(cmd) = model.move_quick(id, path) {
                    return run_cmd(model, env, hooks, tx, uid, cmd);
                }
            }
            Err(e) => {
                model.message = Some(format!("Could not create {}: {e}", path.display()));
            }
        },
        Cmd::SaveWidths(widths) => {
            if let Some(path) = &env.config_path
                && let Err(e) = config::save_widths(path, widths)
            {
                model.message = Some(format!("Pane widths not saved: {e}"));
            }
        }
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
    use crate::store::state;
    model.state_dirty = false;
    let Some(path) = &env.state_path else { return };
    let ours: Vec<_> = model
        .cards
        .iter()
        .map(|c| c.to_record(model.now, model.unix_now))
        .collect();
    let known = model
        .known
        .iter()
        .map(|id| id.0.hyphenated().to_string())
        .collect();
    let records = state::merge_records(state::load(path), ours, &known);
    if let Err(e) = state::save(path, &records) {
        model.message = Some(format!("Sessions not saved: {e}"));
    }
    let unix = |at: Option<std::time::Instant>| {
        at.map_or(0, |at| {
            model
                .unix_now
                .saturating_sub(model.now.saturating_duration_since(at).as_secs())
        })
    };
    let ours = state::Limits {
        vendors: [0, 1].map(|v| (model.limits[v].clone(), unix(model.limits_at[v]))),
    };
    let file = state::limits_file(path);
    let limits = state::merge_limits(state::load_limits(&file), ours.clone());
    for (v, (windows, at)) in limits.vendors.iter().enumerate() {
        if *at > ours.vendors[v].1 {
            model.limits[v].clone_from(windows);
            model.limits_at[v] = model.now.checked_sub(std::time::Duration::from_secs(
                model.unix_now.saturating_sub(*at),
            ));
        }
    }
    // reason: limits are a convenience; the next report brings them back.
    let _ = state::save_limits(&file, &limits);
}

/// Asks kitty to focus its neighbouring window on `side` with
/// `kitten @ focus-window --match neighbor:<side>` (fixed argv, kitty's
/// remote control over `KITTY_LISTEN_ON`); a missing `kitten` or a refusal
/// changes nothing.
fn kitty_focus(side: &str) {
    let spawned = std::process::Command::new("kitten")
        .args(["@", "focus-window", "--match"])
        .arg(format!("neighbor:{side}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut child) = spawned {
        thread::spawn(move || {
            // reason: reaps the child; its exit status does not matter.
            let _ = child.wait();
        });
    }
}

/// The kitty user variable kitty's `map --when-focus-on var:IS_VIM=true`
/// rules test, so `ctrl-h/j/k/l` reach mc instead of moving kitty windows.
const KITTY_PASS_KEYS_ON: &[u8] = b"\x1b]1337;SetUserVar=IS_VIM=dHJ1ZQ==\x07";
/// Clears [`KITTY_PASS_KEYS_ON`] when mc exits.
const KITTY_PASS_KEYS_OFF: &[u8] = b"\x1b]1337;SetUserVar=IS_VIM\x07";

/// Writes the saved workspaces to `config.json`, keeping every other key.
fn save_workspaces(model: &mut Model, env: &Env) {
    if let Some(path) = &env.config_path
        && let Err(e) = config::save_workspaces(path, &model.workspaces)
    {
        model.message = Some(format!("Workspaces not saved: {e}"));
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
    model.remember_workspace(&settings.workspace);
    save_workspaces(model, env);
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
    card.prompted = launch.resume.is_some();
    if let Some(old) = replaces {
        model.cards.retain(|c| c.id != old);
    }
    crate::debug_log!(
        "launch {} {} in {} (resume: {}, pick: {}, fork: {}, replaces: {})",
        kind.command(),
        launch.id.short(),
        project.display(),
        launch.resume.is_some(),
        launch.pick,
        launch.fork,
        replaces.map_or_else(String::new, SessionId::short),
    );
    let size = if model.root() == Some(project.as_path()) {
        ui::popup_size(model.screen)
    } else {
        ui::output_size(model.screen, model.zoom, model.widths)
    };
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
            crate::debug_log!("could not start {command}: {e}");
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

/// Keeps every session's emulator and PTY at its pane's size: the output
/// pane, or the popup for quick sessions.
fn resize_sessions(model: &mut Model) {
    let size = ui::output_size(model.screen, model.zoom, model.widths);
    let quick = ui::popup_size(model.screen);
    let root = model.root().map(Path::to_path_buf);
    for card in model.cards.iter_mut().filter(|c| c.running()) {
        let want = if root.as_deref() == Some(card.project.as_path()) {
            quick
        } else {
            size
        };
        if let Some(pty) = card.pty.as_mut() {
            pty.resize(want);
        }
    }
}

/// Moves project folders `paths` to the Trash (`dd`, confirmed), rescans,
/// and remembers what went where for `u`; stops at the first failure.
fn trash_projects(model: &mut Model, paths: &[PathBuf]) {
    let root = model.root().map(Path::to_path_buf).unwrap_or_default();
    let home = model.home.clone().unwrap_or_default();
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    let name = |p: &Path| {
        p.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    let mut moved = Vec::new();
    let mut failure = None;
    for path in paths {
        match workspace::trash(path, &root, &home, data.as_deref()) {
            Ok(went) => {
                crate::debug_log!("trashed {} to {}", path.display(), went.display());
                moved.push((path.clone(), went));
            }
            Err(e) => {
                failure = Some(format!("Could not move {} to the Trash: {e}", name(path)));
                break;
            }
        }
    }
    if let Ok(projects) = workspace::scan(&root) {
        model.projects = projects;
    }
    model.selected = model.selected.min(model.visible().len().saturating_sub(1));
    model.card = 0;
    model.message = failure.or_else(|| {
        Some(match moved.as_slice() {
            [(one, _)] => format!("Moved {} to the Trash · u undoes it", name(one)),
            many => format!("Moved {} projects to the Trash · u undoes it", many.len()),
        })
    });
    model.last_trash = moved;
}

/// Puts the projects last moved to the Trash back (`u`) and selects the
/// first.
fn restore_projects(model: &mut Model, moved: &[(PathBuf, PathBuf)]) {
    let mut failure = None;
    for (folder, trashed) in moved {
        match workspace::restore(trashed, folder) {
            Ok(()) => crate::debug_log!("restored {} from {}", folder.display(), trashed.display()),
            Err(e) => {
                let name = folder
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                failure = Some(format!("Could not put {name} back: {e}"));
            }
        }
    }
    if let Some(root) = model.root().map(Path::to_path_buf)
        && let Ok(projects) = workspace::scan(&root)
    {
        model.projects = projects;
    }
    if let Some(row) = moved
        .first()
        .and_then(|(folder, _)| model.visible().iter().position(|p| p.path == *folder))
    {
        model.selected = row;
    }
    model.message = failure.or_else(|| Some("Put back.".to_owned()));
}

/// Sends SIGTERM to outside session `pid` after the user confirmed it
/// (SECURITY.md "Sessions outside mc"): only when a fresh snapshot of this
/// user's processes still shows it as `claude`, `codex` or `node`, and with
/// that snapshot's start time checked again right before the signal.
/// Returns the line to show.
fn stop_outside(pid: i32, uid: u32) -> String {
    let snapshot = crate::proc::snapshot(uid);
    let Some(proc) = snapshot.iter().find(|p| p.pid == pid) else {
        return format!("pid {pid} has already gone.");
    };
    if !matches!(proc.name().as_str(), "claude" | "codex" | "node") {
        return format!(
            "pid {pid} is no longer an agent ({}); left alone.",
            proc.name()
        );
    }
    match crate::proc::kill::signal(proc, rustix::process::Signal::TERM) {
        Ok(()) => format!("Stopping pid {pid}."),
        Err(crate::proc::kill::KillError::Gone) => format!("pid {pid} has already gone."),
        Err(crate::proc::kill::KillError::Denied) => format!("Could not stop pid {pid}."),
    }
}

/// Creates project `path` (`a`): `git init`, plus `AGENTS.md` and a
/// `CLAUDE.md` that imports it when `agent_files`; then rescans and selects
/// it.
fn new_project(model: &mut Model, path: &Path, agent_files: bool) {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let made = create_project(path).and_then(|()| {
        if agent_files {
            std::fs::write(
                path.join("AGENTS.md"),
                format!("# {name}\n\nInstructions for AI coding agents working in this project.\n"),
            )?;
            std::fs::write(path.join("CLAUDE.md"), "@AGENTS.md\n")?;
        }
        Ok(())
    });
    match made {
        Ok(()) => {
            crate::debug_log!(
                "new project {} (agent files: {agent_files})",
                path.display()
            );
            if let Some(root) = model.root().map(Path::to_path_buf)
                && let Ok(projects) = workspace::scan(&root)
            {
                model.projects = projects;
            }
            if let Some(row) = model.visible().iter().position(|p| p.path == path) {
                model.selected = row;
            }
            model.focus = crate::app::model::Focus::Projects;
            model.message = Some(format!("Created {name} · n starts a session in it"));
        }
        Err(e) => model.message = Some(format!("Could not create {name}: {e}")),
    }
}

/// Creates project folder `path` in the workspace and runs `git init` in
/// it (fixed argv, no shell), so the workspace scan lists it.
///
/// # Errors
///
/// The folder exists or cannot be made, or `git init` failed or is missing.
fn create_project(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)?;
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(path)
        .stdin(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("git init failed"))
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
    fn stops_an_outside_session_only_while_it_is_still_an_agent() {
        let uid = rustix::process::getuid().as_raw();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        assert!(
            stop_outside(pid, uid).contains("no longer an agent"),
            "sleep is left alone"
        );
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(stop_outside(pid, uid).contains("already gone"));
    }

    #[test]
    fn a_new_project_with_agent_files_gets_git_agents_and_claude() {
        let ws = std::env::temp_dir().join(format!("mc-a-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).unwrap();
        let mut model = crate::app::model::tests::sample(&["x"]);
        new_project(&mut model, &ws.join("full"), true);
        new_project(&mut model, &ws.join("bare"), false);
        assert!(ws.join("full/.git").is_dir() && ws.join("bare/.git").is_dir());
        assert_eq!(
            std::fs::read_to_string(ws.join("full/CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
        assert!(
            std::fs::read_to_string(ws.join("full/AGENTS.md"))
                .unwrap()
                .starts_with("# full")
        );
        assert!(!ws.join("bare/AGENTS.md").exists());
        std::fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn creates_a_project_folder_the_scan_lists() {
        let ws = std::env::temp_dir().join(format!("mc-newproj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).unwrap();
        create_project(&ws.join("fresh")).unwrap();
        let names: Vec<String> = workspace::scan(&ws)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["fresh"]);
        assert!(
            create_project(&ws.join("fresh")).is_err(),
            "an existing folder is refused"
        );
        std::fs::remove_dir_all(&ws).unwrap();
    }

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
