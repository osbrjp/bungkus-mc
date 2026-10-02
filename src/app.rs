//! The event loop: owns the terminal and the [`Model`], receives every
//! [`AppEvent`] on one channel, renders, and runs the [`Cmd`]s `update`
//! returns (ARCHITECTURE §3, §8).
//!
//! The terminal is restored by [`TerminalGuard`] on every exit path and by
//! the panic hook on a panic. The loop sleeps until an event or the next
//! deadline (a synchronized update, a stop grace, the 350 ms animation
//! tick); it never renders on a fixed timer.

pub(crate) mod activity;
pub(crate) mod browser;
pub(crate) mod form;
pub(crate) mod groups;
mod interact;
pub(crate) mod links;
pub(crate) mod model;
pub(crate) mod picker;
pub(crate) mod quick;
pub(crate) mod repo;
pub(crate) mod sessions;
pub(crate) mod stop;
mod takeover;
pub(crate) mod tools;
pub(crate) mod workspaces;

use std::ffi::{OsStr, OsString};
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
use crate::app::tools::{Opener, TermView, Tool};
use crate::ipc::server;
use crate::store::config::{self, Config, Notify};
use crate::term::session::{Colors, Session, Size, child_env};
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

/// How long `O` waits before the desktop's opener runs. The opener takes
/// the focus, and a terminal that loses it while Shift is still down never
/// sees the release: every later key then arrives shifted, until a click.
const FOLDER_DELAY: Duration = Duration::from_millis(300);

/// How often the pull request and issue of a running session's branch are
/// read again: two `gh` calls per folder, well inside GitHub's hourly
/// limit of 5000 for a handful of sessions.
const LINKS_EVERY: Duration = Duration::from_mins(1);

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
    /// A resource reading for the activity overlay.
    Activity(crate::proc::usage::Sample),
    /// Agent sessions running outside mc (every [`EXTERNAL_EVERY`]).
    External(Vec<crate::external::External>),
    /// The git branch and status of the session folders in a repository.
    Repos(Vec<(PathBuf, repo::Status)>),
    /// The pull request and issue of the session folders that were read.
    Links(Vec<(PathBuf, links::Links)>),
    /// The open pull requests and issues `gh` listed for the popup opened
    /// on this folder.
    LinkList(PathBuf, Vec<links::Link>),
    /// The text `gh` read of the issue or pull request at this URL, for
    /// the popup's text view; `None` when the read failed.
    LinkBody(String, Option<String>),
    /// The host terminal went away (input closed).
    HostGone,
    /// A newer release exists (the hourly check).
    UpdateAvailable(String),
    /// The `U` update finished: the installed tag, `None` when already
    /// latest, or why it failed.
    Updated(Result<Option<String>, String>),
    /// A worktree removal finished; what to tell the user.
    Worktrees(String),
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
    /// Whether to start a quick session popup at once (`-q`).
    pub quick: bool,
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
    load_overrides(&mut model, env);
    resume_sessions(&mut model, env, hooks.as_ref(), &tx, uid);
    if env.quick
        && let Some(cmd) = model.quick_session()
    {
        run_cmd(&mut model, env, hooks.as_ref(), &tx, uid, cmd);
    }
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
        let notify = model.overrides.notify.unwrap_or(env.config.notify);
        announce(&mut model, notify, &mut title);
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
            before_event(&mut model, &event);
            if matches!(event, AppEvent::External(_) | AppEvent::Worktrees(_)) {
                rescan(&mut model);
                scan_repos(&mut model, &tx);
                scan_links(&mut model, &env.config, &tx);
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

/// Runs before each event is applied: ends overdue synchronized updates in
/// every emulator and, in kitty, sets [`KITTY_PASS_KEYS_ON`] again after a
/// PTY event.
fn before_event(model: &mut Model, event: &AppEvent) {
    for card in &mut model.cards {
        if let Some(pty) = card.pty.as_mut() {
            pty.flush_sync();
        }
    }
    model.tools_mut().for_each(Session::flush_sync);
    if model.kitty && matches!(event, AppEvent::Pty(_)) {
        write_host(KITTY_PASS_KEYS_ON);
    }
}

/// Resumes the sessions the last quit stopped ([`Model::auto_resume`]).
/// Launching selects and focuses each session, so the project and focus
/// mc started with are put back: the project by its folder, since the
/// resumed sessions reorder the recent group of the list.
fn resume_sessions(
    model: &mut Model,
    env: &Env,
    hooks: Option<&Hooks>,
    tx: &SyncSender<AppEvent>,
    uid: u32,
) {
    let resumes = model.auto_resume();
    if resumes.is_empty() {
        return;
    }
    let selected = model.selected_project().map(|p| p.path.clone());
    let focus = model.focus;
    for cmd in resumes {
        run_cmd(model, env, hooks, tx, uid, cmd);
    }
    if let Some(path) = selected {
        model.select_project(&path);
    }
    (model.card, model.focus) = (0, focus);
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
            let mut told = None;
            loop {
                let tag = crate::update::available(|n| std::env::var(n).ok());
                if tag.is_some() && tag != told {
                    told.clone_from(&tag);
                    if tx
                        .send(AppEvent::UpdateAvailable(tag.unwrap_or_default()))
                        .is_err()
                    {
                        return;
                    }
                }
                thread::sleep(crate::update::CHECK_TTL);
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
        Cmd::Apply(settings, scope) => apply(model, env, settings, Some(scope)),
        Cmd::Launch(request) => launch(model, env, hooks, request, tx),
        Cmd::WatchRollout(id, path) => watch_rollout(model, id, &path, tx),
        Cmd::SwitchWorkspace(workspace, session) => {
            if let Some(mut settings) = model.settings.clone() {
                crate::debug_log!("switch workspace to {}", workspace.display());
                settings.workspace = workspace;
                apply(model, env, settings, None);
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
        Cmd::RemoveWorktree(project, name) => forget_worktree(model, &project, &name, tx),
        Cmd::CleanWorktrees(project) => clean_worktrees(model, &project, tx),
        Cmd::OpenEditor(project) => open_editor(model, &env.config, &project, tx),
        Cmd::OpenFolder(project) => open_folder(model, project),
        Cmd::OpenUrl(url) => open_folder(model, PathBuf::from(url)),
        Cmd::ListLinks(folder) => list_links(gh_dir(model, env, &folder), folder, tx.clone()),
        Cmd::ReadLink(folder, link) => {
            read_link(gh_dir(model, env, &folder), folder, link, tx.clone());
        }
        Cmd::OpenTerminal(owner, dir) => open_terminal(model, &env.config, owner, &dir, tx),
        Cmd::SaveGroups(groups) => save_groups(model, &groups),
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
        Cmd::Sample => sample_activity(tx),
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

/// Takes a resource reading for the activity overlay on a background
/// thread and reports it as [`AppEvent::Activity`].
fn sample_activity(tx: &SyncSender<AppEvent>) {
    let tx = tx.clone();
    thread::spawn(move || {
        // reason: a closed loop just loses this sample.
        let _ = tx.send(AppEvent::Activity(crate::proc::usage::sample()));
    });
}

/// Reads the git branch and status of every session folder on a
/// background thread (every [`EXTERNAL_EVERY`], one read at a time) and
/// reports them as [`AppEvent::Repos`].
fn scan_repos(model: &mut Model, tx: &SyncSender<AppEvent>) {
    if model.repo_scan {
        return;
    }
    let mut folders: Vec<PathBuf> = model.cards.iter().map(Card::folder).collect();
    folders.sort();
    folders.dedup();
    if folders.is_empty() {
        return;
    }
    model.repo_scan = true;
    let tx = tx.clone();
    thread::spawn(move || {
        let repos = folders
            .into_iter()
            .filter_map(|folder| Some((repo::Status::read(&folder)?, folder)))
            .map(|(status, folder)| (folder, status))
            .collect();
        // reason: mc may have quit meanwhile.
        let _ = tx.send(AppEvent::Repos(repos));
    });
}

/// Returns the `gh` config folder of the workspace `folder` is in
/// ([`Config::gh_config_dir`]), for mc's own `gh` reads.
fn gh_dir(model: &Model, env: &Env, folder: &Path) -> Option<PathBuf> {
    env.config.gh_config_dir(folder, model.home.as_deref())
}

/// Lists the open pull requests and issues of the repository `folder` is
/// in on a background thread, for the `i` popup ([`AppEvent::LinkList`]).
/// `gh` is the workspace's `gh` config folder ([`gh_dir`]).
fn list_links(gh: Option<PathBuf>, folder: PathBuf, tx: SyncSender<AppEvent>) {
    thread::spawn(move || {
        let list = links::list(&folder, gh.as_deref());
        // reason: mc may have quit meanwhile.
        let _ = tx.send(AppEvent::LinkList(folder, list));
    });
}

/// Reads the text of `link` in `folder` on a background thread, for the
/// popup's text view ([`AppEvent::LinkBody`]). `gh` is the workspace's `gh`
/// config folder ([`gh_dir`]).
fn read_link(gh: Option<PathBuf>, folder: PathBuf, link: links::Link, tx: SyncSender<AppEvent>) {
    thread::spawn(move || {
        let text = links::body(&folder, gh.as_deref(), &link);
        // reason: mc may have quit meanwhile.
        let _ = tx.send(AppEvent::LinkBody(link.url, text));
    });
}

/// Reads the pull request and issue of session folders' branches on a
/// background thread (one read at a time) and reports them as
/// [`AppEvent::Links`].
///
/// A folder is read when its branch is not the one its links were read
/// for, and the folders of running sessions again every [`LINKS_EVERY`],
/// so a pull request opened meanwhile shows up. Each folder is read with
/// its workspace's `gh` config folder ([`Config::gh_config_dir`]).
fn scan_links(model: &mut Model, config: &Config, tx: &SyncSender<AppEvent>) {
    if model.links_scan {
        return;
    }
    let stale = model
        .links_at
        .is_none_or(|at| model.now >= at + LINKS_EVERY);
    let mut due: Vec<(PathBuf, String)> = model
        .cards
        .iter()
        .filter_map(|card| {
            let folder = card.folder();
            let branch = &model.repos.get(&folder)?.branch;
            let read = model.links.get(&folder).map(|links| &links.branch) == Some(branch);
            (!read || (stale && card.running())).then(|| (folder, branch.clone()))
        })
        .collect();
    due.sort();
    due.dedup();
    if due.is_empty() {
        return;
    }
    model.links_scan = true;
    if stale {
        model.links_at = Some(model.now);
    }
    let home = model.home.as_deref();
    let due: Vec<_> = due
        .into_iter()
        .map(|(folder, branch)| (config.gh_config_dir(&folder, home), folder, branch))
        .collect();
    let tx = tx.clone();
    thread::spawn(move || {
        let read = due
            .into_iter()
            .map(|(gh, folder, branch)| {
                let links = links::read(&folder, gh.as_deref(), &branch);
                (folder, links)
            })
            .collect();
        // reason: mc may have quit meanwhile.
        let _ = tx.send(AppEvent::Links(read));
    });
}

/// Lists the workspace again (every [`EXTERNAL_EVERY`]), so a folder made
/// outside mc (a new git worktree, a clone) shows up in the projects pane;
/// the selection stays on its project.
fn rescan(model: &mut Model) {
    let Some(Ok(projects)) = model.root().map(workspace::scan) else {
        return;
    };
    if projects == model.projects {
        return;
    }
    let selected = model.selected_project().map(|p| p.path.clone());
    model.projects = projects;
    if let Some(row) = selected.and_then(|path| model.visible().iter().position(|p| p.path == path))
    {
        model.selected = row;
    }
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
///
/// Written at start and again after every PTY event: a program inside mc
/// (vim-kitty-navigator, over `KITTY_LISTEN_ON`) sets the variable to
/// `false` on mc's window when it quits, and its last output comes after.
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
/// A `scope` (a submit of the settings screen) first puts the default
/// agent where it says ([`save_agent_scope`]); a workspace switch has none
/// and leaves the workspace's own choice alone.
fn apply(
    model: &mut Model,
    env: &Env,
    settings: config::Settings,
    scope: Option<config::AgentScope>,
) {
    let settings = match scope {
        Some(scope) => save_agent_scope(model, settings, scope),
        None => settings,
    };
    if let Some(path) = &env.config_path
        && let Err(e) = config::save(path, &settings)
    {
        model.message = Some(format!("Settings not saved: {e}"));
    }
    let scan = workspace::scan(&settings.workspace);
    model.remember_workspace(&settings.workspace);
    save_workspaces(model, env);
    model.apply(settings, scan, &env.cwd);
    load_overrides(model, env);
    recolour(model);
}

/// Saves the default agent of submitted `settings` where `scope` says and
/// returns the settings to save globally.
///
/// For the workspace only: it goes into the workspace's
/// `.bungkus-mc/config.json` and the global default agent stays what it
/// was. For all workspaces: the workspace's own `defaultAgent` is removed,
/// so the global one applies there again. A file that cannot be written
/// leaves the settings as submitted and says why.
fn save_agent_scope(
    model: &mut Model,
    settings: config::Settings,
    scope: config::AgentScope,
) -> config::Settings {
    let (here, global) = match scope {
        config::AgentScope::Workspace => {
            let before = model.settings.as_ref().map(|s| s.default_agent);
            (
                Some(settings.default_agent),
                before.unwrap_or(settings.default_agent),
            )
        }
        config::AgentScope::Global => (None, settings.default_agent),
    };
    match config::save_workspace_agent(&settings.workspace, here) {
        Ok(()) => config::Settings {
            default_agent: global,
            ..settings
        },
        Err(e) => {
            model.message = Some(format!("{}/{e}; not changed.", config::WORKSPACE_DIR));
            settings
        }
    }
}

/// Reads the open workspace's own `.bungkus-mc/config.json` over the
/// global config: an unset key keeps the global value, and a file that
/// cannot be used sets nothing and says why.
fn load_overrides(model: &mut Model, env: &Env) {
    let loaded = model.root().map(config::load_overrides).transpose();
    model.overrides = match loaded {
        Ok(overrides) => overrides.unwrap_or_default(),
        Err(e) => {
            model.message = Some(format!("{}/{e}; ignored.", config::WORKSPACE_DIR));
            config::Overrides::default()
        }
    };
    let keep = model
        .overrides
        .cleanup
        .as_ref()
        .unwrap_or(&env.config.cleanup);
    model.keep.clone_from(&keep.keep);
}

/// Points running sessions at the theme now in effect: the OSC 10/11
/// answers, and for Claude a colour-scheme report (`CSI ? 997 ; 1 n` dark,
/// `2 n` light) so its `auto` theme follows without a restart. The
/// emulator does not track mode 2031, so the report is sent unasked;
/// Claude parses it either way.
fn recolour(model: &mut Model) {
    let colors = colors(model.theme);
    let report: &[u8] = match model.theme.name {
        theme::ThemeName::Dark => b"\x1b[?997;1n",
        theme::ThemeName::Light => b"\x1b[?997;2n",
    };
    for card in &model.cards {
        if let Some(pty) = &card.pty {
            pty.set_colors(colors);
            if card.kind == Kind::Claude {
                pty.send(report.to_vec());
            }
        }
    }
}

/// Returns `$CLAUDE_CONFIG_DIR`, else `.claude` under `home`.
fn claude_config_dir(home: Option<&Path>) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map_or_else(
        || home.unwrap_or_else(|| Path::new("")).join(".claude"),
        PathBuf::from,
    )
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
    let worktree = worktree_for(model, env, &request, has_commit);
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
        let config_dir = claude_config_dir(model.home.as_deref());
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
    let extra_args = extra_args(model, &env.config, (kind, &project), worktree.as_deref());
    card.worktree = worktree;
    let argv = agent::argv(kind, &program, &extra_args, &launch);
    let id = launch.id.0.hyphenated().to_string();
    let gh = env.config.gh_config_dir(&project, model.home.as_deref());
    let mut extra = vec![("BUNGKUS_MC_SESSION", OsStr::new(&id))];
    extra.extend(gh.iter().map(|dir| ("GH_CONFIG_DIR", dir.as_os_str())));
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
        model.output_size()
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

/// Starts `command` (program first, looked up on `PATH`) in a PTY of
/// `size` in `dir`, with the same scrubbed environment as an agent and the
/// workspace's `GH_CONFIG_DIR` ([`Config::gh_config_dir`]).
///
/// # Returns
///
/// The running tool, or `None` with the reason in the message line.
fn spawn_tool(
    model: &mut Model,
    config: &Config,
    command: &[String],
    dir: &Path,
    size: Size,
    tx: &SyncSender<AppEvent>,
) -> Option<Tool> {
    let name = command.first()?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let program = if name.contains('/') {
        Some(PathBuf::from(name))
    } else {
        find_on_path(name, &path)
    };
    let Some(program) = program else {
        model.message = Some(format!("{name} not found on PATH."));
        return None;
    };
    let argv: Vec<OsString> = std::iter::once(program.into_os_string())
        .chain(command[1..].iter().map(OsString::from))
        .collect();
    let id = SessionId::new();
    let gh = config.gh_config_dir(dir, model.home.as_deref());
    let extra: Vec<_> = gh
        .iter()
        .map(|dir| ("GH_CONFIG_DIR", dir.as_os_str()))
        .collect();
    let env = child_env(std::env::vars_os(), &extra);
    match Session::spawn(id, &argv, dir, &env, size, colors(model.theme), tx) {
        Ok(pty) => Some(Tool { id, pty }),
        Err(e) => {
            model.message = Some(format!("Could not start {name}: {e}"));
            None
        }
    }
}

/// Starts the terminal pane's shell (`$SHELL`, else `/bin/sh`) for `owner`
/// in `dir` and gives it the keys.
fn open_terminal(
    model: &mut Model,
    config: &Config,
    owner: crate::app::tools::Owner,
    dir: &Path,
    tx: &SyncSender<AppEvent>,
) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let size = ui::terminal_size(model.screen, model.zoom, model.widths);
    if let Some(tool) = spawn_tool(model, config, &[shell], dir, size, tx) {
        model.shells.push((owner, tool));
        model.term_view = TermView::Focused;
    }
}

/// Returns the user's own editor: `$VISUAL`, else `$EDITOR`, when set.
pub(crate) fn user_editor() -> Option<String> {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
}

/// Opens `project` in the user's editor (the `editor` setting, else
/// `$VISUAL`, else `$EDITOR`; see [`tools::opener`]): vim in the popup with
/// the folder as its argument (also when none is set and one is on
/// `PATH`), anything else started on its own and left alone.
fn open_editor(model: &mut Model, config: &Config, project: &Path, tx: &SyncSender<AppEvent>) {
    let editor = model
        .settings
        .as_ref()
        .and_then(|s| s.editor.clone())
        .or_else(user_editor);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let installed = |name: &str| find_on_path(name, &path).is_some();
    match tools::opener(editor.as_deref(), installed) {
        Opener::Popup(mut command) => {
            command.push(".".into());
            let size = ui::popup_size(model.screen);
            model.editor = spawn_tool(model, config, &command, project, size, tx);
        }
        Opener::Detached(command) => open_detached(model, &command, project),
    }
}

/// Opens `project` (a folder, or an `https://` address) with the desktop's
/// opener ([`tools::desktop_opener`]) after [`FOLDER_DELAY`], off the UI
/// thread; a missing opener goes to the message line.
fn open_folder(model: &mut Model, project: PathBuf) {
    use std::os::unix::process::CommandExt;
    let opener = tools::desktop_opener();
    let path = std::env::var_os("PATH").unwrap_or_default();
    if find_on_path(opener, &path).is_none() {
        model.message = Some(format!("{opener} not found on PATH."));
        return;
    }
    model.message = Some(format!("Opened in {opener}."));
    thread::spawn(move || {
        thread::sleep(FOLDER_DELAY);
        // reason: the opener shows its own errors; its status does not matter.
        let _ = std::process::Command::new(opener)
            .arg(&project)
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// Starts `command` (argv, no shell) with `project` as its last argument
/// and leaves it alone: its own process group, no terminal; the outcome
/// goes to the message line.
fn open_detached(model: &mut Model, command: &[String], project: &Path) {
    use std::os::unix::process::CommandExt;
    let Some((program, args)) = command.split_first() else {
        return;
    };
    let spawned = std::process::Command::new(program)
        .args(args)
        .arg(project)
        .current_dir(project)
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            model.message = Some(format!("Opened in {program}."));
            thread::spawn(move || {
                // reason: reaps the child; its exit status does not matter.
                let _ = child.wait();
            });
        }
        Err(e) => model.message = Some(format!("Could not start {program}: {e}")),
    }
}

/// Returns the git worktree a Claude session runs in, if any: the one of
/// the card it resumes, or a new one when another session already runs in
/// the project (`replaces` aside), so the two do not edit one checkout.
///
/// A new one needs `worktrees` on in the config, a fresh session (not a
/// resume, not the picker of past sessions), a project other than the
/// workspace root, and a repository with a commit to branch from.
fn worktree_for(
    model: &Model,
    env: &Env,
    request: &LaunchRequest,
    has_commit: impl Fn(&Path) -> bool,
) -> Option<String> {
    if request.kind != Kind::Claude {
        return None;
    }
    let project = request.project.as_path();
    let resumed = request
        .replaces
        .and_then(|id| model.cards.iter().find(|c| c.id == id))
        .filter(|c| c.project == project)
        .and_then(|c| c.worktree.clone());
    if resumed.is_some() {
        return resumed;
    }
    let shared = model.root() != Some(project)
        && model
            .cards
            .iter()
            .any(|c| c.running() && c.project == project && Some(c.id) != request.replaces);
    let fresh = request.launch.resume.is_none() && !request.launch.pick;
    let worktrees = model.overrides.worktrees.unwrap_or(env.config.worktrees);
    (worktrees && shared && fresh && has_commit(project)).then(|| {
        let name = project.file_name().unwrap_or_default().to_string_lossy();
        worktree_name(&name, request.launch.id)
    })
}

/// Returns what goes between the agent's program and mc's own arguments,
/// for a new session and a resumed one alike: the configured `args`,
/// `--worktree <name>` if any, `--add-dir <folder>` for every project
/// grouped with `project` ([`Model::group_of`]), and the instructions (the
/// built-in rules unless `instructions` is off in the config, the related
/// folders, then the open workspace's `.bungkus-mc/CLAUDE.md` /
/// `AGENTS.md`; see [`agent::instruction_args`]).
fn extra_args(
    model: &Model,
    config: &config::Config,
    (kind, project): (Kind, &Path),
    worktree: Option<&str>,
) -> Vec<String> {
    let dir = model.root().map(|root| root.join(config::WORKSPACE_DIR));
    let related = model.group_of(project);
    let shared = related
        .iter()
        .filter_map(|folder| folder.to_str())
        .flat_map(|folder| ["--add-dir".to_owned(), folder.to_owned()])
        .collect();
    let instructions = agent::instruction_args(kind, dir.as_deref(), config.instructions, &related);
    [
        worktree_args(&config.agents.get(kind).args, worktree),
        shared,
        instructions,
    ]
    .concat()
}

/// Writes the workspace's project groups to its `.bungkus-mc/config.json`
/// (`g`); a failure is said, and the groups stay in effect for this run.
fn save_groups(model: &mut Model, groups: &[Vec<String>]) {
    let saved = model
        .root()
        .map(|root| config::save_workspace_groups(root, groups));
    if let Some(Err(e)) = saved {
        model.message = Some(format!("{}/{e}; groups not saved.", config::WORKSPACE_DIR));
    }
}

/// Returns the agent's extra arguments plus `--worktree <name>`, if any.
fn worktree_args(args: &[String], worktree: Option<&str>) -> Vec<String> {
    let flag = worktree.map(|name| ["--worktree".to_owned(), name.to_owned()]);
    args.iter()
        .cloned()
        .chain(flag.into_iter().flatten())
        .collect()
}

/// Returns the worktree (and branch) name of a session in project folder
/// `name`, as `<project-name>-<session-id>` (`bungkus-mc-3ec9`): the
/// folder's name in lowercase ASCII letters, digits and `-` (24 at most),
/// then the session's short id without its `#`. The workspace's
/// instructions give agents the same format for worktrees of their own.
fn worktree_name(name: &str, id: SessionId) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    let slug: String = slug
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(24)
        .collect();
    let short = id.short();
    format!("{slug}-{}", short.trim_start_matches('#'))
        .trim_matches('-')
        .to_owned()
}

/// Runs `git` in `dir` with fixed arguments (no shell) and returns what it
/// printed, or `None` when it failed or is missing. It gets its own process
/// group, so a signal that ends mc (a closed terminal) does not cut a
/// worktree removal off halfway.
fn git(dir: &Path, args: &[&OsStr]) -> Option<String> {
    output("git", dir, args, &[])
}

/// Runs `program` in `dir` with fixed arguments (no shell, no input, its
/// own process group) and `env` added to mc's environment, and returns
/// what it printed, or `None` when it failed or is missing.
fn output(program: &str, dir: &Path, args: &[&OsStr], env: &[(&str, &OsStr)]) -> Option<String> {
    use std::os::unix::process::CommandExt;
    let output = std::process::Command::new(program)
        .args(args)
        .envs(env.iter().copied())
        .current_dir(dir)
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Returns whether `project` is a git repository with a commit, which a
/// new worktree needs as its starting point.
fn has_commit(project: &Path) -> bool {
    let args = ["rev-parse", "-q", "--verify", "HEAD"].map(OsStr::new);
    git(project, &args).is_some()
}

/// Removes worktree `path` of repository `repo` and returns whether it
/// went: unlocked first (Claude locks the ones it makes), then
/// `git worktree remove` without force, which refuses a worktree holding
/// modified or untracked files. Its branch stays.
fn remove_worktree(repo: &Path, path: &Path) -> bool {
    let args = |verb| [OsStr::new("worktree"), OsStr::new(verb), path.as_os_str()];
    // reason: an unlocked worktree makes this fail, which changes nothing.
    let _ = git(repo, &args("unlock"));
    git(repo, &args("remove")).is_some()
}

/// Removes the worktree a forgotten session ran in, off the UI thread (a
/// worktree with dependencies takes seconds to delete), and reports what
/// happened as [`AppEvent::Worktrees`].
fn forget_worktree(model: &mut Model, project: &Path, name: &str, tx: &SyncSender<AppEvent>) {
    let path = project.join(".claude/worktrees").join(name);
    if !path.exists() {
        return;
    }
    model.message = Some(format!("Removing worktree {name}…"));
    let (project, name, tx) = (project.to_path_buf(), name.to_owned(), tx.clone());
    thread::spawn(move || {
        let text = if remove_worktree(&project, &path) {
            format!("Removed worktree {name} (its branch stays).")
        } else {
            format!("Worktree {name} has uncommitted files; kept.")
        };
        // reason: mc may have quit meanwhile.
        let _ = tx.send(AppEvent::Worktrees(text));
    });
}

/// Removes the unused worktrees of `project` (`c`, confirmed): every
/// linked worktree git lists, except one a session of mc runs in or can
/// resume into, one an outside agent session runs in, and (git refuses)
/// one holding modified or untracked files. Branches stay. The removal
/// runs off the UI thread and reports as [`AppEvent::Worktrees`].
fn clean_worktrees(model: &mut Model, project: &Path, tx: &SyncSender<AppEvent>) {
    // reason: pruning only drops records of folders that are gone.
    let _ = git(project, &["worktree", "prune"].map(OsStr::new));
    let args = ["worktree", "list", "--porcelain"].map(OsStr::new);
    let Some(list) = git(project, &args) else {
        model.message = Some("Not a git repository; no worktrees to clean.".into());
        return;
    };
    let used = |path: &Path| {
        model.cards.iter().any(|c| {
            (c.running() && workspace::same_dir(&c.project, path))
                || c.worktree.as_ref().is_some_and(|name| {
                    workspace::same_dir(&c.project.join(".claude/worktrees").join(name), path)
                })
        }) || model.external.iter().any(|e| {
            let cwd = e.cwd.canonicalize().unwrap_or_else(|_| e.cwd.clone());
            cwd.starts_with(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
        })
    };
    // The first entry is the main checkout.
    let unused: Vec<PathBuf> = list
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .skip(1)
        .map(PathBuf::from)
        .filter(|path| !used(path))
        .collect();
    if unused.is_empty() {
        model.message = Some("No unused worktrees.".into());
        return;
    }
    model.message = Some(format!("Removing {} unused worktrees…", unused.len()));
    let (project, tx) = (project.to_path_buf(), tx.clone());
    thread::spawn(move || {
        let removed = unused
            .iter()
            .filter(|path| remove_worktree(&project, path))
            .count();
        let text = match (removed, unused.len() - removed) {
            (n, 0) => format!("Removed {n} unused worktrees (branches stay)."),
            (n, k) => format!("Removed {n} unused worktrees · kept {k} with uncommitted files."),
        };
        // reason: mc may have quit meanwhile.
        let _ = tx.send(AppEvent::Worktrees(text));
    });
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
/// pane, or the popup for quick sessions; the editor has the popup's size
/// and every shell the terminal pane's.
fn resize_sessions(model: &mut Model) {
    let size = model.output_size();
    let quick = ui::popup_size(model.screen);
    if let Some(editor) = model.editor.as_mut() {
        editor.pty.resize(quick);
    }
    let pane = ui::terminal_size(model.screen, model.zoom, model.widths);
    for (_, shell) in &mut model.shells {
        shell.pty.resize(pane);
    }
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
    if let Some((folder, _)) = moved.first() {
        model.select_project(folder);
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
            model.select_project(path);
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
    fn a_session_in_a_grouped_project_is_given_the_other_folders() {
        let mut model = crate::app::model::tests::sample(&["api", "web", "docs"]);
        let config = config::Config {
            instructions: false,
            ..config::Config::default()
        };
        let path = |i: usize| model.projects[i].path.clone();
        let (api, web, docs) = (path(0), path(1), path(2));
        for kind in Kind::ALL {
            assert!(extra_args(&model, &config, (kind, &api), None).is_empty());
        }
        model.overrides.groups = vec![vec!["web".into(), "api".into()]];
        for kind in Kind::ALL {
            let args = extra_args(&model, &config, (kind, &api), None);
            assert_eq!(args[..2], ["--add-dir", web.to_str().unwrap()], "{kind:?}");
            assert!(args[3].contains("Related folders") && args[3].contains(web.to_str().unwrap()));
            assert!(extra_args(&model, &config, (kind, &docs), None).is_empty());
        }
    }

    #[test]
    fn the_settings_screen_saves_the_agent_for_one_workspace_or_all() {
        use crate::store::config::{AgentScope, Settings, load_overrides};

        let root = std::env::temp_dir().join(format!("mc-scope-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut model = crate::app::model::tests::sample(&["a"]);
        let submitted = Settings {
            workspace: root.clone(),
            default_agent: Kind::Codex,
            ..model.settings.clone().unwrap()
        };
        let global = save_agent_scope(&mut model, submitted.clone(), AgentScope::Workspace);
        assert_eq!(global.default_agent, Kind::Claude, "the global one stays");
        assert_eq!(
            load_overrides(&root).unwrap().default_agent,
            Some(Kind::Codex)
        );
        let global = save_agent_scope(&mut model, submitted, AgentScope::Global);
        assert_eq!(global.default_agent, Kind::Codex);
        assert_eq!(load_overrides(&root).unwrap().default_agent, None);
        model.overrides.default_agent = Some(Kind::Codex);
        model.open_form(
            crate::app::form::FormKind::Settings,
            crate::app::form::Field::Agent,
        );
        let Some(crate::app::model::Overlay::Form(form)) = &model.overlay else {
            panic!("the settings screen");
        };
        assert_eq!(
            (form.agent, form.scope),
            (Kind::Codex, AgentScope::Workspace)
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn only_a_fresh_claude_session_joining_a_busy_repository_gets_a_worktree() {
        use crate::app::model::tests::{sample, with_session};
        let mut model = sample(&["a", "b"]);
        let (id, _writes) = with_session(&mut model, "first");
        let (a, b) = (
            model.projects[0].path.clone(),
            model.projects[1].path.clone(),
        );
        let env = |worktrees| Env {
            config_path: None,
            cwd: PathBuf::new(),
            wizard_prefill: None,
            quick: false,
            config: Config {
                worktrees,
                ..Config::default()
            },
            state_path: None,
        };
        let request = |kind, project: &Path, replaces, resume: Option<&str>| LaunchRequest {
            project: project.to_path_buf(),
            kind,
            launch: agent::Launch {
                id: SessionId(uuid::Uuid::from_u128(0xbeef << 112)),
                model: None,
                name: Some("Fix: the login!".into()),
                prompt: None,
                settings: None,
                hook_args: Vec::new(),
                resume: resume.map(str::to_owned),
                pick: false,
                fork: false,
            },
            replaces,
        };
        let name = Some("a-beef".to_owned());
        assert_eq!(
            worktree_name(
                "Kedai Web_v2",
                SessionId(uuid::Uuid::from_u128(0xbeef << 112))
            ),
            "kedai-web-v2-beef",
            "<project-name>-<session-id>"
        );
        let cases = [
            (Kind::Claude, &a, None, None, true, true, name.clone()),
            (Kind::Codex, &a, None, None, true, true, None),
            (Kind::Claude, &b, None, None, true, true, None),
            (Kind::Claude, &a, Some(id), None, true, true, None),
            (Kind::Claude, &a, None, Some("x"), true, true, None),
            (Kind::Claude, &a, None, None, false, true, None),
            (Kind::Claude, &a, None, None, true, false, None),
        ];
        for (kind, project, replaces, resume, on, commit, want) in cases {
            let got = worktree_for(
                &model,
                &env(on),
                &request(kind, project, replaces, resume),
                |_| commit,
            );
            assert_eq!(got, want, "{kind:?} {project:?} {replaces:?} {resume:?}");
        }
        model.cards[0].worktree = Some("old".into());
        let got = worktree_for(
            &model,
            &env(false),
            &request(Kind::Claude, &a, Some(id), Some("x")),
            |_| false,
        );
        assert_eq!(
            got,
            Some("old".into()),
            "a resume goes back into its worktree"
        );
    }

    #[test]
    fn cleaning_removes_clean_unused_worktrees_and_keeps_dirty_ones() {
        let repo = std::env::temp_dir().join(format!("mc-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
            git(&repo, &args).unwrap_or_else(|| panic!("git {args:?}"))
        };
        run(&["init", "-q"]);
        assert!(!has_commit(&repo));
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
        assert!(has_commit(&repo));
        for name in ["clean", "dirty", "mine"] {
            let path = format!(".claude/worktrees/{name}");
            run(&["worktree", "add", "-q", "-b", name, &path]);
            run(&["worktree", "lock", &path]);
        }
        std::fs::write(repo.join(".claude/worktrees/dirty/new.txt"), "x").unwrap();
        let mut model = crate::app::model::tests::sample(&[]);
        let mut card = Card::new(
            SessionId::new(),
            Kind::Claude,
            repo.clone(),
            None,
            None,
            model.now,
        );
        card.worktree = Some("mine".into());
        model.cards.push(card);
        let (tx, rx) = mpsc::sync_channel(1);
        let done = || match rx.recv().unwrap() {
            AppEvent::Worktrees(text) => text,
            other => panic!("{other:?}"),
        };
        clean_worktrees(&mut model, &repo, &tx);
        let text = done();
        let left = |name: &str| repo.join(".claude/worktrees").join(name).exists();
        assert!(!left("clean"), "{text}");
        assert!(left("dirty") && left("mine"));
        assert!(
            run(&["branch", "--list", "clean"]).contains("clean"),
            "the branch stays"
        );
        assert!(text.contains("kept 1"));
        forget_worktree(&mut model, &repo, "mine", &tx);
        done();
        assert!(!left("mine"));
        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn a_theme_switch_reports_the_colour_scheme_to_claude() {
        let mut model = crate::app::model::tests::sample(&["a"]);
        let (_, writes) = crate::app::model::tests::with_session(&mut model, "s");
        for (name, want) in [
            (theme::ThemeName::Light, &b"\x1b[?997;2n"[..]),
            (theme::ThemeName::Dark, &b"\x1b[?997;1n"[..]),
        ] {
            model.theme = model.theme.with_name(name);
            recolour(&mut model);
            assert_eq!(writes.try_recv().unwrap(), want);
        }
    }

    #[test]
    fn a_rescan_lists_a_new_folder_and_keeps_the_selected_project() {
        let ws = std::env::temp_dir().join(format!("mc-rescan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        for name in ["a", "c"] {
            std::fs::create_dir_all(ws.join(name).join(".git")).unwrap();
        }
        let mut model = crate::app::model::tests::sample(&[]);
        let mut settings = model.settings.clone().unwrap();
        settings.workspace.clone_from(&ws);
        model.apply(settings, workspace::scan(&ws), Path::new("/"));
        model.selected = 1;
        std::fs::create_dir_all(ws.join("b").join(".git")).unwrap();
        rescan(&mut model);
        let names: Vec<&str> = model.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert_eq!(model.selected_project().unwrap().name, "c");
        std::fs::remove_dir_all(&ws).unwrap();
    }

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
