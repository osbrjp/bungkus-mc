//! The model the UI thread owns, and `update`: one event in, the model
//! changed, maybe a command out (ARCHITECTURE §8).
//!
//! `update` does no blocking I/O. Sending bytes to a session's writer
//! thread and signalling its process group are non-blocking and happen
//! here; spawning, saving and scanning are returned as a [`Cmd`].

use std::path::{Path, PathBuf};
use std::time::Instant;

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use rustix::process::Signal;

use crate::agent::usage::{Usage, Window};
use crate::agent::{Kind, Launch};
use crate::app::AppEvent;
use crate::app::form::{Field, Form, FormKind, Outcome};
use crate::app::picker::{self, Picker};
use crate::app::sessions::{self, Card, HOOK_GRACE, STOP_GRACE, State};
use crate::app::stop::{StopDialog, StopKind};
use crate::external::External;
use crate::ipc::Wire;
use crate::proc::Proc;
use crate::store::config::Settings;
use crate::term::keys::Chord;
use crate::term::{PtyEvent, SessionId};
use crate::ui::keymap::{self, Action, Lookup, Scope};
use crate::ui::theme::{Theme, ThemeChoice};
use crate::workspace::Project;

/// How often tracked processes are rescanned while a session runs
/// (ARCHITECTURE §3.3).
const SCAN_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// How long after a project-number digit a next digit extends the number
/// instead of starting a new one (DESIGN §8; bungkus-cli's `jumpWindow`).
pub(crate) const JUMP_WINDOW: std::time::Duration = std::time::Duration::from_millis(700);

/// Which pane has focus. The output pane's focus is INTERACT (DESIGN §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    /// The projects pane (left).
    Projects,
    /// The sessions pane (middle).
    Sessions,
    /// The output pane: every key goes to the agent.
    Output,
}

impl Focus {
    /// Returns the keymap scope for this pane (INTERACT uses no keymap).
    #[must_use]
    pub(crate) const fn scope(self) -> Scope {
        match self {
            Self::Projects => Scope::Projects,
            Self::Sessions | Self::Output => Scope::Sessions,
        }
    }
}

/// A screen drawn over the panes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Overlay {
    /// The generated key help.
    Help,
    /// The first-run wizard or the settings screen.
    Form(Form),
    /// The `n` picker.
    Picker(Picker),
    /// The quit / stop dialog.
    Stop(StopDialog),
    /// "Forget this session?"
    Forget(SessionId),
    /// Waiting for an outside session (since the instant) to close so mc
    /// can resume it.
    TakeOver(External, Instant),
    /// Which agent's list of past sessions `r` opens.
    ResumeAgent(Kind),
    /// Moving a quick session into a project, or a new one.
    Move(crate::app::quick::MoveDialog),
    /// "Stop this session started outside mc?"
    StopOutside(External),
}

/// A session the loop must start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchRequest {
    /// The project folder it runs in.
    pub project: PathBuf,
    /// Which agent.
    pub kind: Kind,
    /// The picker's choices.
    pub launch: Launch,
    /// The finished card a resume replaces.
    pub replaces: Option<SessionId>,
}

/// Something the host terminal should announce (DESIGN §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Alert {
    /// A session now needs the user.
    NeedsYou(String),
    /// A session failed.
    Failed(String),
}

/// Work `update` hands back to the event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Leave mc.
    Quit,
    /// Clear the terminal and draw everything again.
    Redraw,
    /// Save these settings and rescan the workspace.
    Apply(Settings),
    /// Start a session.
    Launch(LaunchRequest),
    /// Start the Codex usage reader on this session's rollout file.
    WatchRollout(SessionId, PathBuf),
    /// Take a fresh snapshot and open the quit / stop dialog.
    OpenStop(StopKind),
    /// Signal these tracked processes (identity-checked).
    Signal(Vec<Proc>, Signal),
    /// Take a background process snapshot.
    Scan,
    /// Write the dragged pane widths to `config.json`.
    SaveWidths(crate::ui::Widths),
    /// Stop outside session `pid` (SIGTERM, after the user confirmed):
    /// still ours, still an agent, same process.
    StopOutside(i32),
    /// Create this project folder (`git init`), then move quick session
    /// `SessionId` into it.
    CreateProject(SessionId, PathBuf),
}

/// Everything the screen shows.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent view flags of the one app model, not a state machine"
)]
pub(crate) struct Model {
    /// Colours for the applied settings.
    pub theme: Theme,
    /// Whether the host terminal's background is light (OSC 11), if known.
    pub host_light: Option<bool>,
    /// Home directory, for `~` in paths.
    pub home: Option<PathBuf>,
    /// Applied settings; `None` until the first-run wizard finishes.
    pub settings: Option<Settings>,
    /// Projects in the workspace.
    pub projects: Vec<Project>,
    /// Why the workspace could not be listed, if it could not.
    pub scan_error: Option<String>,
    /// Index into [`Model::visible`] of the selected project.
    pub selected: usize,
    /// Every session started in this run, all projects.
    pub cards: Vec<Card>,
    /// Index into the selected project's ordered cards.
    pub card: usize,
    /// Focused pane.
    pub focus: Focus,
    /// Whether the output pane fills the body (`z`).
    pub zoom: bool,
    /// Projects filter text.
    pub filter: String,
    /// Whether the filter is being typed (FILTER mode).
    pub filtering: bool,
    /// First key of a pending double press.
    pub pending: Option<char>,
    /// The project number typed so far and when its last digit came.
    pub jump: Option<(usize, Instant)>,
    /// Help, a form, the picker or a confirm on top of the panes.
    pub overlay: Option<Overlay>,
    /// One-line message shown in place of the key hints until the next key.
    pub message: Option<String>,
    /// Where each agent was found on `PATH`, in `Kind::ALL` order.
    pub found: [Option<String>; 2],
    /// Workspace used when the wizard is skipped with an empty field.
    pub fallback_workspace: PathBuf,
    /// Rows the projects list shows, for half-page moves.
    pub list_rows: usize,
    /// The chord that leaves INTERACT.
    pub exit_chord: Chord,
    /// Pane widths (dragged by the mouse, from `config.json`).
    pub widths: crate::ui::Widths,
    /// The pane border being dragged, if any.
    pub drag: Option<crate::app::interact::Divider>,
    /// The whole terminal, for layout and mouse hit tests.
    pub screen: Rect,
    /// The time the view renders at.
    pub now: Instant,
    /// Animation frame counter, advanced by the 350 ms tick.
    pub frame: usize,
    /// When quitting started, while sessions are being stopped.
    pub quitting: Option<Instant>,
    /// Announcements for the loop to send (bell, desktop, title).
    pub alerts: Vec<Alert>,
    /// The latest plan limits per vendor (`Kind::ALL` order); per
    /// account, not per session (DESIGN §6.1).
    pub limits: [Vec<Window>; 2],
    /// When each vendor's limits were last reported; they only refresh when
    /// a session in mc reports, so the status bar shows their age once old.
    pub limits_at: [Option<Instant>; 2],
    /// Wall-clock time in unix seconds, for stale limit windows.
    pub unix_now: u64,
    /// `cleanup.keep`: extra process names that start as `[keep]`.
    pub keep: Vec<String>,
    /// When the next background process scan is due.
    pub next_scan: Option<Instant>,
    /// Whether `sessions.json` must be written.
    pub state_dirty: bool,
    /// Agent sessions running outside mc, read-only (ARCHITECTURE §3.4).
    pub external: Vec<External>,
    /// Shows every key mc receives in the hint line (`BUNGKUS_MC_DEBUG_KEYS`),
    /// to find chords a terminal keeps for itself.
    pub debug_keys: bool,
    /// The quick session whose popup shows (keys go to it).
    pub popup: Option<SessionId>,
    /// Whether the popup's menu (`ctrl-\`: hide, move, new project) is open.
    pub popup_menu: bool,
    /// The `quick` row that leads the projects list while quick sessions
    /// exist; its path is the workspace root.
    pub quick_row: Project,
    /// When the band mascot was last clicked and which quote it says.
    pub poke: Option<(Instant, usize)>,
    /// The "elsewhere" row that ends the projects list while an outside
    /// session runs in no project folder; its path is empty.
    pub elsewhere: Project,
}

impl Model {
    /// Creates a model with no settings applied yet.
    ///
    /// # Arguments
    ///
    /// * `theme`    - Colours for this terminal.
    /// * `home`     - Home directory, for `~` in paths.
    /// * `found`    - Where each agent was found on `PATH`.
    /// * `fallback` - Workspace used when the wizard is skipped empty.
    #[must_use]
    pub(crate) fn new(
        theme: Theme,
        home: Option<PathBuf>,
        found: [Option<String>; 2],
        fallback: PathBuf,
    ) -> Self {
        Self {
            theme,
            host_light: None,
            home,
            settings: None,
            projects: Vec::new(),
            scan_error: None,
            selected: 0,
            cards: Vec::new(),
            card: 0,
            focus: Focus::Projects,
            zoom: false,
            filter: String::new(),
            filtering: false,
            pending: None,
            jump: None,
            overlay: None,
            message: None,
            found,
            fallback_workspace: fallback,
            list_rows: 10,
            widths: crate::ui::Widths::default(),
            drag: None,
            exit_chord: Chord::DEFAULT,
            screen: Rect::new(0, 0, 120, 40),
            now: Instant::now(),
            frame: 0,
            quitting: None,
            alerts: Vec::new(),
            limits: [Vec::new(), Vec::new()],
            limits_at: [None, None],
            unix_now: 0,
            keep: Vec::new(),
            next_scan: None,
            state_dirty: false,
            external: Vec::new(),
            poke: None,
            popup: None,
            popup_menu: false,
            debug_keys: false,
            quick_row: Project {
                name: "quick".into(),
                path: PathBuf::new(),
                worktree_of: None,
            },
            elsewhere: Project {
                name: "elsewhere".into(),
                path: PathBuf::new(),
                worktree_of: None,
            },
        }
    }

    /// Returns the theme to draw with: the form's choice while a form is
    /// open (live preview), the applied one otherwise.
    #[must_use]
    pub(crate) fn view_theme(&self) -> Theme {
        match &self.overlay {
            Some(Overlay::Form(form)) => self.theme.with_name(form.theme.resolve(self.host_light)),
            _ => self.theme,
        }
    }

    /// Returns the projects matching the filter, in display order, then
    /// the [`Model::elsewhere`] row while an outside session runs in no
    /// project folder, then the `quick` row (number 0) while quick
    /// sessions exist.
    #[must_use]
    pub(crate) fn visible(&self) -> Vec<&Project> {
        let needle = self.filter.to_lowercase();
        let elsewhere = (!self.external_in(Path::new("")).is_empty()).then_some(&self.elsewhere);
        let quick = self
            .cards
            .iter()
            .any(|c| self.is_quick(c))
            .then_some(&self.quick_row);
        self.projects
            .iter()
            .chain(elsewhere)
            .chain(quick)
            .filter(|p| p.name.to_lowercase().contains(&needle))
            .collect()
    }

    /// Returns the selected project, if any is visible.
    #[must_use]
    pub(crate) fn selected_project(&self) -> Option<&Project> {
        self.visible().get(self.selected).copied()
    }

    /// Returns the indices into [`Model::cards`] of the selected project's
    /// sessions, in display order.
    #[must_use]
    pub(crate) fn project_cards(&self) -> Vec<usize> {
        self.selected_project()
            .map_or_else(Vec::new, |p| sessions::order(&self.cards, &p.path))
    }

    /// Returns the sessions outside mc running in `project` (or below it),
    /// or, for the empty path of [`Model::elsewhere`], in no project folder;
    /// every session mc started is left out: by pid, by a tracked
    /// descendant's pid, or by the agent's session id.
    #[must_use]
    pub(crate) fn external_in(&self, project: &Path) -> Vec<&External> {
        if self.root() == Some(project) {
            return Vec::new();
        }
        let inside = |e: &External| {
            if project.as_os_str().is_empty() {
                !self.projects.iter().any(|p| e.cwd.starts_with(&p.path))
            } else {
                e.cwd.starts_with(project)
            }
        };
        self.external
            .iter()
            .filter(|e| inside(e))
            .filter(|e| {
                !self.cards.iter().any(|c| {
                    c.pid == Some(e.pid)
                        || c.descendants.procs.iter().any(|p| p.pid == e.pid)
                        || e.session_id.as_ref().is_some_and(|id| {
                            c.agent_session.as_ref() == Some(id) || c.id.0.to_string() == *id
                        })
                })
            })
            .collect()
    }

    /// Returns the selected outside session: the selection runs past the
    /// project's cards into the outside sessions listed below them.
    #[must_use]
    pub(crate) fn selected_external(&self) -> Option<&External> {
        let project = self.selected_project()?;
        let index = self.card.checked_sub(self.project_cards().len())?;
        self.external_in(&project.path).get(index).copied()
    }

    /// Returns how many rows the sessions pane can select: the project's
    /// cards, then its outside sessions.
    fn session_rows(&self) -> usize {
        let outside = self
            .selected_project()
            .map_or(0, |p| self.external_in(&p.path).len());
        self.project_cards().len() + outside
    }

    /// Returns the index of the selected card, if the project has any.
    #[must_use]
    pub(crate) fn selected_card(&self) -> Option<usize> {
        self.project_cards().get(self.card).copied()
    }

    /// Returns the card with `id`.
    pub(crate) fn card_mut(&mut self, id: SessionId) -> Option<&mut Card> {
        self.cards.iter_mut().find(|c| c.id == id)
    }

    /// Returns the installed agents, in `Kind::ALL` order.
    #[must_use]
    pub(crate) fn installed(&self) -> [bool; 2] {
        [self.found[0].is_some(), self.found[1].is_some()]
    }

    /// Returns whether anything on screen animates (a running card of the
    /// selected project, or the empty-state mascot), so the 350 ms tick
    /// must run.
    #[must_use]
    pub(crate) fn animating(&self) -> bool {
        let empty_output = self.selected_card().is_none();
        self.theme.animated()
            && (empty_output
                || self.poke.is_some()
                || self
                    .project_cards()
                    .iter()
                    .any(|&i| self.cards[i].running()))
    }

    /// Returns when the loop must wake without input: a synchronized update
    /// to flush, a stop grace period ending, or the animation tick.
    #[must_use]
    pub(crate) fn deadline(&self, tick: Instant) -> Option<Instant> {
        let syncs = self
            .cards
            .iter()
            .filter_map(|c| c.pty.as_ref()?.sync_deadline());
        let stops = self
            .cards
            .iter()
            .filter(|c| c.running() && !c.killed)
            .filter_map(|c| c.stop_requested.map(|at| at + STOP_GRACE));
        let silent = self
            .cards
            .iter()
            .filter(|c| c.hooked && c.running() && c.events == 0)
            .map(|c| c.started + HOOK_GRACE)
            .filter(|at| *at > self.now);
        let tick = self.animating().then_some(tick);
        let scan = self
            .next_scan
            .filter(|_| self.cards.iter().any(Card::running));
        let plans = self.plan_deadline();
        let takeover = self.take_over_deadline();
        let poke = self.poke.map(|(at, _)| at + crate::ui::mascot::POKE);
        syncs
            .chain(takeover)
            .chain(poke)
            .chain(stops)
            .chain(silent)
            .chain(tick)
            .chain(scan)
            .chain(plans)
            .min()
    }

    /// Opens the settings screen (or, before first run, the wizard).
    fn open_form(&mut self, kind: FormKind, field: Field) {
        let current = self.settings.clone().unwrap_or_else(|| Settings {
            workspace: PathBuf::new(),
            theme: ThemeChoice::Auto,
            default_agent: Kind::Claude,
        });
        let mut form = Form::new(
            kind,
            &current,
            self.found.clone(),
            self.fallback_workspace.clone(),
            self.home.clone(),
        );
        form.field = field;
        self.overlay = Some(Overlay::Form(form));
    }

    /// Opens the first-run wizard with `prefill` in the workspace field.
    pub(crate) fn start_wizard(&mut self, prefill: &str) {
        self.open_form(FormKind::Wizard, Field::Workspace);
        if let Some(Overlay::Form(form)) = &mut self.overlay {
            form.set_workspace(prefill);
        }
    }

    /// Handles one event from the loop's channel.
    ///
    /// # Returns
    ///
    /// The command the loop must run, if any.
    pub(crate) fn update(&mut self, event: AppEvent) -> Option<Cmd> {
        match event {
            AppEvent::Tick => {
                self.frame = self.frame.wrapping_add(1);
                if self
                    .poke
                    .is_some_and(|(at, _)| self.now >= at + crate::ui::mascot::POKE)
                {
                    self.poke = None;
                }
                if let Some(cmd) = self.take_over_due() {
                    return Some(cmd);
                }
                self.enforce_stops();
                if let Some(cmd) = self.plan_kill_due() {
                    return Some(cmd);
                }
                let running = self.cards.iter().any(Card::running);
                match self.next_scan {
                    Some(at) if running && self.now >= at => {
                        self.next_scan = Some(self.now + SCAN_EVERY);
                        return Some(Cmd::Scan);
                    }
                    None if running => self.next_scan = Some(self.now + SCAN_EVERY),
                    _ => {}
                }
            }
            AppEvent::Procs(snapshot) => self.track(&snapshot),
            AppEvent::External(list) => {
                self.external = list;
                self.selected = self.selected.min(self.visible().len().saturating_sub(1));
                self.card = self.card.min(self.session_rows().saturating_sub(1));
            }
            AppEvent::HostGone => return self.host_gone(),
            AppEvent::UpdateAvailable(tag) => {
                self.message = Some(format!("bungkus-mc {tag} is out — bungkus-mc update"));
            }
            AppEvent::Pty(PtyEvent::Output(id, bytes)) => {
                if let Some(pty) = self.card_mut(id).and_then(|c| c.pty.as_mut()) {
                    pty.advance(&bytes);
                }
            }
            AppEvent::Hook(line) => {
                if let Some(cmd) = self.hook(&line) {
                    return Some(cmd);
                }
            }
            AppEvent::Usage(id, usage) => self.codex_usage(id, usage),
            AppEvent::Pty(PtyEvent::Exited(id, code)) => {
                let now = self.now;
                self.state_dirty = true;
                if let Some(card) = self.card_mut(id) {
                    card.exited(code, now);
                    if let State::Failed(reason) = &card.state {
                        let text = format!("{} failed: {reason}", card.id.short());
                        self.alerts.push(Alert::Failed(text));
                    }
                }
                if self.popup == Some(id) {
                    self.popup = None;
                }
                if let Some(cmd) = self.moved_after_exit(id) {
                    return Some(cmd);
                }
                if let Some(cmd) = self.plan_after_exit(id) {
                    return Some(cmd);
                }
                if self.focus == Focus::Output
                    && self.selected_card().map(|i| self.cards[i].id) == Some(id)
                {
                    self.focus = Focus::Sessions;
                }
            }
            AppEvent::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                let cmd = self.key(key);
                if self.debug_keys {
                    self.message = Some(format!("key: {:?} + {:?}", key.code, key.modifiers));
                }
                return cmd;
            }
            AppEvent::Input(Event::Paste(text)) => self.paste(&text),
            AppEvent::Input(Event::Mouse(mouse)) => return self.mouse(mouse),
            AppEvent::Input(Event::Resize(w, h)) => self.screen = Rect::new(0, 0, w, h),
            AppEvent::Input(_) => {}
        }
        self.quit_when_stopped()
    }

    /// Applies one socket line to the card of its mc session; malformed
    /// lines and unknown sessions are dropped.
    fn hook(&mut self, line: &[u8]) -> Option<Cmd> {
        let wire = serde_json::from_slice::<Wire>(line).ok()?;
        let now = self.now;
        let card = self
            .cards
            .iter_mut()
            .find(|c| c.id.0.hyphenated().to_string() == wire.mc_session)?;
        if let Some(usage) = wire.usage {
            if !usage.limits.is_empty() {
                self.limits[card.kind as usize].clone_from(&usage.limits);
                self.limits_at[card.kind as usize] = Some(now);
            }
            card.report(usage);
            return None;
        }
        let before = card.state.clone();
        let bound = card.agent_session.is_some();
        card.reduce(&wire.event, now);
        if card.state == State::NeedsYou && before != State::NeedsYou {
            let text = format!("{} needs you: {}", card.id.short(), card.name);
            self.alerts.push(Alert::NeedsYou(text));
        }
        let first_bind = !bound && card.agent_session.is_some();
        let watch = card.kind == Kind::Codex && card.running() && card.rollout_stop.is_none();
        match (
            &wire.event.transcript,
            first_bind || wire.event.name == "SessionStart",
            watch,
        ) {
            (Some(path), true, true) => Some(Cmd::WatchRollout(card.id, PathBuf::from(path))),
            _ => None,
        }
    }

    /// Applies usage from the Codex rollout reader.
    fn codex_usage(&mut self, id: SessionId, usage: Usage) {
        if !usage.limits.is_empty() {
            self.limits[Kind::Codex as usize].clone_from(&usage.limits);
            self.limits_at[Kind::Codex as usize] = Some(self.now);
        }
        if let Some(card) = self.card_mut(id) {
            card.report(usage);
        }
    }

    /// Jumps to the next session that needs you, across projects: selects
    /// its project and card and enters INTERACT (DESIGN §8.1).
    fn next_needs_you(&mut self) {
        let current = self.selected_card().map(|i| self.cards[i].id);
        let projects: Vec<PathBuf> = self.visible().iter().map(|p| p.path.clone()).collect();
        let mut found = Vec::new();
        for (p, path) in projects.iter().enumerate() {
            for (c, &i) in sessions::order(&self.cards, path).iter().enumerate() {
                if self.cards[i].state == State::NeedsYou {
                    found.push((p, c, self.cards[i].id));
                }
            }
        }
        let after = found
            .iter()
            .position(|f| Some(f.2) == current)
            .map_or(0, |i| i + 1);
        let Some(&(p, c, _)) = found.get(after % found.len().max(1)) else {
            self.message = Some("nobody needs you right now".into());
            return;
        };
        self.selected = p;
        self.card = c;
        self.focus = Focus::Output;
    }

    /// Sends SIGKILL to sessions still running after their stop grace.
    fn enforce_stops(&mut self) {
        let now = self.now;
        for card in self.cards.iter_mut().filter(|c| c.running() && !c.killed) {
            if card.stop_requested.is_some_and(|at| now >= at + STOP_GRACE) {
                card.killed = true;
                if let Some(pty) = &card.pty {
                    // reason: ESRCH means it exited meanwhile, which is the goal.
                    let _ = pty.signal(Signal::KILL);
                }
            }
        }
    }

    /// Returns [`Cmd::Quit`] once quitting and every session has ended (or
    /// the grace period plus one second has passed).
    pub(crate) fn quit_when_stopped(&self) -> Option<Cmd> {
        let started = self.quitting?;
        let done = !self.stopping()
            || self.now >= started + STOP_GRACE * 2 + std::time::Duration::from_secs(1);
        done.then_some(Cmd::Quit)
    }

    /// Handles a key press.
    fn key(&mut self, key: KeyEvent) -> Option<Cmd> {
        if self.quitting.is_some() {
            return None;
        }
        if let (Some(id), None) = (self.popup, &self.overlay) {
            self.popup_key(id, key);
            return None;
        }
        if self.focus == Focus::Output && self.overlay.is_none() {
            self.interact_key(key);
            return None;
        }
        self.message = None;
        if let Some(overlay) = self.overlay.take() {
            return self.overlay_key(overlay, key);
        }
        if self.filtering {
            self.filter_key(key);
            return None;
        }
        let lookup = keymap::lookup(self.focus.scope(), key, self.pending.take());
        let jump = self.jump.take();
        match lookup {
            Lookup::Pending(ch) => {
                self.pending = Some(ch);
                None
            }
            Lookup::Unbound => None,
            Lookup::Action(Action::Jump) => {
                if let KeyCode::Char(ch) = key.code
                    && let Some(d) = ch.to_digit(10)
                {
                    self.jump_digit(jump, d as usize);
                }
                None
            }
            Lookup::Action(action) => self.act(action),
        }
    }

    /// Handles a project-number digit, as bungkus-cli's wizard does: the
    /// selection moves on every digit, never after a wait. Within
    /// [`JUMP_WINDOW`] of the previous digit it extends the number (`1`
    /// then `6` is 16), otherwise it starts a new one. A number with no
    /// project leaves the last jump in place; any other key ends the
    /// number, and `0` alone selects the `quick` row.
    fn jump_digit(&mut self, previous: Option<(usize, Instant)>, d: usize) {
        let n = match previous {
            Some((n, at)) if self.now < at + JUMP_WINDOW => n * 10 + d,
            _ => d,
        };
        let visible = self.visible();
        let quick = visible
            .last()
            .is_some_and(|p| self.root() == Some(p.path.as_path()));
        let numbered = visible.len() - usize::from(quick);
        if n == 0 {
            if quick && previous.is_none_or(|(_, at)| self.now >= at + JUMP_WINDOW) {
                self.selected = numbered;
                self.card = 0;
            }
            return;
        }
        self.jump = Some((n, self.now));
        if n <= numbered {
            self.selected = n - 1;
            self.card = 0;
        }
    }

    /// Passes a key to an overlay; the overlay stays unless it closed.
    fn overlay_key(&mut self, overlay: Overlay, key: KeyEvent) -> Option<Cmd> {
        match overlay {
            Overlay::Help => None,
            Overlay::Form(mut form) => match form.key(key) {
                Outcome::Continue => {
                    self.overlay = Some(Overlay::Form(form));
                    None
                }
                Outcome::Cancel => None,
                Outcome::Submit(settings) => Some(Cmd::Apply(settings)),
                Outcome::Quit => Some(self.request_quit()),
            },
            Overlay::Picker(mut p) => match p.key(key) {
                picker::Outcome::Continue => {
                    self.overlay = Some(Overlay::Picker(p));
                    None
                }
                picker::Outcome::Cancel => None,
                picker::Outcome::Start => self.launch(&p),
            },
            Overlay::Stop(dialog) => self.stop_key(dialog, key),
            Overlay::Move(dialog) => self.move_key(dialog, key),
            Overlay::StopOutside(ext) => {
                (key.code == KeyCode::Char('y')).then_some(Cmd::StopOutside(ext.pid))
            }
            Overlay::ResumeAgent(kind) => match key.code {
                KeyCode::Enter => self.pick_past(kind),
                KeyCode::Esc => None,
                KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l') | KeyCode::Tab => {
                    let other = match kind {
                        Kind::Claude => Kind::Codex,
                        Kind::Codex => Kind::Claude,
                    };
                    self.overlay = Some(Overlay::ResumeAgent(other));
                    None
                }
                _ => {
                    self.overlay = Some(Overlay::ResumeAgent(kind));
                    None
                }
            },
            Overlay::TakeOver(ext, since) => {
                if key.code != KeyCode::Esc {
                    self.overlay = Some(Overlay::TakeOver(ext, since));
                }
                None
            }
            Overlay::Forget(id) => {
                if key.code == KeyCode::Char('y') {
                    self.cards.retain(|c| c.id != id);
                    self.card = self.card.min(self.project_cards().len().saturating_sub(1));
                    self.state_dirty = true;
                }
                None
            }
        }
    }

    /// Turns the picker's choices into a launch, or says why not.
    fn launch(&mut self, p: &Picker) -> Option<Cmd> {
        if !p.installed[p.agent as usize] {
            let (cmd, product) = (p.agent.command(), p.agent.product());
            self.message = Some(format!(
                "{cmd} not found on PATH. Install {product}, then n again."
            ));
            return None;
        }
        let project = self.selected_project()?.path.clone();
        let text = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
        let launch = Launch {
            id: SessionId::new(),
            model: p.model_id(),
            name: text(&p.name),
            prompt: text(&p.prompt),
            settings: None,
            hook_args: Vec::new(),
            resume: None,
            pick: false,
            fork: false,
        };
        Some(Cmd::Launch(LaunchRequest {
            project,
            kind: p.agent,
            launch,
            replaces: None,
        }))
    }

    /// Resumes the selected finished session in its folder (ARCHITECTURE
    /// §5); a Codex session that never reported through hooks has no id to
    /// resume.
    fn resume(&mut self) -> Option<Cmd> {
        let card = &self.cards[self.selected_card()?];
        if card.running() {
            return None;
        }
        let Some(id) = card.resume_id() else {
            self.message = Some("not resumable — hooks off".into());
            return None;
        };
        let launch = Launch {
            id: SessionId::new(),
            model: None,
            name: Some(card.name.clone()),
            prompt: None,
            settings: None,
            hook_args: Vec::new(),
            resume: Some(id),
            pick: false,
            fork: false,
        };
        Some(Cmd::Launch(LaunchRequest {
            project: card.project.clone(),
            kind: card.kind,
            launch,
            replaces: Some(card.id),
        }))
    }

    /// Starts the band mascot's click animation with a random quote (never
    /// the one it just said).
    pub(crate) fn poke(&mut self) {
        let quotes = crate::ui::mascot::QUOTES.len();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let mut pick = usize::try_from(nanos).unwrap_or(0) % quotes;
        if self.poke.is_some_and(|(_, last)| last == pick) {
            pick = (pick + 1) % quotes;
        }
        self.poke = Some((self.now, pick));
    }

    /// Enters the selected row of the sessions pane: takes over an outside
    /// session, opens a quick session's popup, or enters INTERACT.
    fn enter_session(&mut self) -> Option<Cmd> {
        if let Some(ext) = self.selected_external().cloned() {
            return self.take_over(ext);
        }
        if let Some(card) = self.selected_card().map(|i| &self.cards[i])
            && self.is_quick(card)
        {
            let id = card.id;
            self.open_popup(id);
            return None;
        }
        self.interact();
        None
    }

    /// Opens the `n` picker for the selected project, preselecting the
    /// default agent.
    fn open_picker(&mut self) {
        let agent = self
            .settings
            .as_ref()
            .map_or(Kind::Claude, |s| s.default_agent);
        let project = self
            .selected_project()
            .map(|p| p.name.clone())
            .unwrap_or_default();
        self.overlay = Some(Overlay::Picker(Picker::new(
            agent,
            self.installed(),
            project,
        )));
    }

    /// Focuses pane `n`: 1 projects, 2 sessions, 3 output (INTERACT when
    /// the selected session runs).
    pub(crate) fn focus_pane(&mut self, n: u8) {
        match n {
            1 => self.focus = Focus::Projects,
            2 => self.focus = Focus::Sessions,
            _ => {
                self.focus = Focus::Sessions;
                self.interact();
            }
        }
    }

    /// Opens a past session of the selected project that mc did not start:
    /// asks which agent when both are installed, then launches that agent's
    /// own list of past sessions (`claude --resume`, `codex resume`) in the
    /// output pane, where the user picks one.
    fn resume_pick(&mut self) -> Option<Cmd> {
        if self
            .selected_project()
            .is_none_or(|p| p.path.as_os_str().is_empty())
        {
            return None;
        }
        let default = self
            .settings
            .as_ref()
            .map_or(Kind::Claude, |s| s.default_agent);
        match self.installed() {
            [true, true] => {
                self.overlay = Some(Overlay::ResumeAgent(default));
                None
            }
            [true, false] => self.pick_past(Kind::Claude),
            [false, true] => self.pick_past(Kind::Codex),
            [false, false] => {
                self.message = Some("Neither claude nor codex is on PATH.".into());
                None
            }
        }
    }

    /// Launches `kind`'s own list of past sessions in the selected project.
    fn pick_past(&mut self, kind: Kind) -> Option<Cmd> {
        let project = self.selected_project()?.path.clone();
        Some(Cmd::Launch(LaunchRequest {
            project,
            kind,
            launch: Launch {
                id: SessionId::new(),
                model: None,
                name: Some("past session".into()),
                prompt: None,
                settings: None,
                hook_args: Vec::new(),
                resume: None,
                pick: true,
                fork: false,
            },
            replaces: None,
        }))
    }

    /// Adds a started (or failed-to-start) session and enters INTERACT on
    /// it when it runs.
    pub(crate) fn add_card(&mut self, card: Card) {
        self.state_dirty = true;
        let running = card.running();
        let quick = self.is_quick(&card);
        let id = card.id;
        let project = card.project.clone();
        self.cards.push(card);
        if let Some(row) = self.visible().iter().position(|p| p.path == project) {
            self.selected = row;
        }
        self.card = self
            .project_cards()
            .iter()
            .position(|&i| self.cards[i].id == id)
            .unwrap_or(0);
        if running && quick {
            self.popup = Some(id);
            return;
        }
        self.focus = if running {
            Focus::Output
        } else {
            Focus::Sessions
        };
    }

    /// Quits at once, or asks first (after a fresh scan) when sessions
    /// are running or processes they started are tracked.
    fn request_quit(&mut self) -> Cmd {
        let busy = self
            .cards
            .iter()
            .any(|c| c.running() || !c.descendants.procs.is_empty());
        if busy {
            Cmd::OpenStop(StopKind::Quit)
        } else {
            Cmd::Quit
        }
    }

    /// Applies a FILTER-mode key: typing edits the search, `↑`/`↓` move
    /// through the matches, `enter` keeps the search and opens the
    /// highlighted project's sessions, `esc` clears it.
    fn filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.filtering = false;
                if self.selected_project().is_some() {
                    self.focus = Focus::Sessions;
                }
                return;
            }
            KeyCode::Up | KeyCode::Down => {
                let delta = if key.code == KeyCode::Up { -1 } else { 1 };
                self.selected = Self::step(self.selected, delta, self.visible().len());
                self.card = 0;
                return;
            }
            KeyCode::Esc => {
                self.filtering = false;
                self.filter.clear();
            }
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.push(ch);
            }
            _ => {}
        }
        self.selected = 0;
        self.card = 0;
    }

    /// Moves a list selection by `delta` rows, clamped to `len`.
    fn step(index: usize, delta: isize, len: usize) -> usize {
        index
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1))
    }

    /// Runs a keymap action.
    fn act(&mut self, action: Action) -> Option<Cmd> {
        let half = isize::try_from((self.list_rows / 2).max(1)).unwrap_or(1);
        let moved = match action {
            Action::Down => Some(1),
            Action::Up => Some(-1),
            Action::HalfDown => Some(half),
            Action::HalfUp => Some(-half),
            Action::First => Some(isize::MIN),
            Action::Last => Some(isize::MAX),
            _ => None,
        };
        if let Some(delta) = moved {
            match self.focus {
                Focus::Projects => {
                    self.selected = Self::step(self.selected, delta, self.visible().len());
                    self.card = 0;
                }
                Focus::Sessions | Focus::Output => {
                    self.card = Self::step(self.card, delta, self.session_rows());
                }
            }
            return None;
        }
        match action {
            Action::PrevPane => {
                self.focus = match self.focus {
                    Focus::Sessions | Focus::Projects => Focus::Projects,
                    Focus::Output => Focus::Sessions,
                };
            }
            Action::NextPane | Action::Interact if self.focus == Focus::Sessions => {
                return self.enter_session();
            }
            Action::Interact => self.interact(),
            Action::NextPane | Action::OpenProject => self.focus = Focus::Sessions,
            Action::Filter => {
                self.focus = Focus::Projects;
                self.filtering = true;
                self.filter.clear();
                self.selected = 0;
            }
            Action::NewSession
                if self
                    .selected_project()
                    .is_some_and(|p| !p.path.as_os_str().is_empty()) =>
            {
                self.open_picker();
            }
            Action::Stop => {
                if let Some(ext) = self.selected_external().cloned() {
                    self.overlay = Some(Overlay::StopOutside(ext));
                    return None;
                }
                if let Some(i) = self.selected_card().filter(|&i| self.cards[i].running()) {
                    return Some(Cmd::OpenStop(StopKind::Session(self.cards[i].id)));
                }
            }
            Action::Pane(n) => self.focus_pane(n),
            Action::QuickSession => return self.quick_session(),
            Action::MoveQuick => self.start_move(false),
            Action::MakeProject => self.start_move(true),
            Action::Zoom => self.zoom = !self.zoom,
            Action::Resume => {
                if self
                    .selected_card()
                    .is_some_and(|i| !self.cards[i].running())
                    && self.focus == Focus::Sessions
                {
                    return self.resume();
                }
                return self.resume_pick();
            }
            Action::Forget => {
                if let Some(i) = self.selected_card().filter(|&i| !self.cards[i].running()) {
                    self.overlay = Some(Overlay::Forget(self.cards[i].id));
                }
            }
            Action::NextNeedsYou => self.next_needs_you(),
            Action::Workspace => self.open_form(FormKind::Settings, Field::Workspace),
            Action::Settings => self.open_form(FormKind::Settings, Field::Agent),
            Action::Help => self.overlay = Some(Overlay::Help),
            Action::Redraw => return Some(Cmd::Redraw),
            Action::Quit => return Some(self.request_quit()),
            Action::Jump
            | Action::NewSession
            | Action::Down
            | Action::Up
            | Action::First
            | Action::Last
            | Action::HalfDown
            | Action::HalfUp => {}
        }
        None
    }

    /// Enters INTERACT on the selected session when it is running.
    pub(crate) fn interact(&mut self) {
        if self
            .selected_card()
            .is_some_and(|i| self.cards[i].running())
        {
            self.focus = Focus::Output;
        }
    }

    /// Applies saved settings and the workspace scan that followed.
    ///
    /// # Arguments
    ///
    /// * `settings` - The settings now in effect.
    /// * `scan`     - The projects, or why the folder could not be listed.
    /// * `cwd`      - mc's working directory; a project containing it is
    ///   preselected.
    pub(crate) fn apply(
        &mut self,
        settings: Settings,
        scan: std::io::Result<Vec<Project>>,
        cwd: &std::path::Path,
    ) {
        self.theme = self
            .theme
            .with_name(settings.theme.resolve(self.host_light));
        let (projects, error) = match scan {
            Ok(projects) => (projects, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        self.projects = projects;
        self.scan_error = error;
        self.filter.clear();
        self.selected = self
            .projects
            .iter()
            .position(|p| cwd.starts_with(&p.path))
            .unwrap_or(0);
        self.card = 0;
        self.quick_row.path.clone_from(&settings.workspace);
        self.settings = Some(settings);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use super::*;
    use crate::term::session::{Colors, Session, Size};
    use crate::ui::theme::{Background, Profile, ThemeName};

    /// A model with `names` as projects of `/Users/me/Works/OSBR`, as the
    /// view tests and goldens use it.
    pub(crate) fn sample(names: &[&str]) -> Model {
        let home = PathBuf::from("/Users/me");
        let workspace = home.join("Works/OSBR");
        let mut model = Model::new(
            Theme::new(ThemeName::Dark, Profile::NoColor, Background::Paint),
            Some(home.clone()),
            [Some("~/.local/bin/claude".into()), None],
            home,
        );
        let projects = names
            .iter()
            .map(|n| Project {
                worktree_of: None,
                name: (*n).into(),
                path: workspace.join(n),
            })
            .collect();
        let settings = Settings {
            workspace,
            theme: ThemeChoice::Dark,
            default_agent: Kind::Claude,
        };
        model.apply(settings, Ok(projects), Path::new("/"));
        model
    }

    /// Adds a running session with a detached emulator to the selected
    /// project; returns its id and what it would write to the PTY.
    pub(crate) fn with_session(
        model: &mut Model,
        name: &str,
    ) -> (SessionId, std::sync::mpsc::Receiver<Vec<u8>>) {
        let project = model.selected_project().unwrap().path.clone();
        let n = u128::try_from(model.cards.len()).unwrap();
        let id = SessionId(uuid::Uuid::from_u128((0xa3f1 + n) << 112));
        let mut card = Card::new(id, Kind::Claude, project, Some(name), None, model.now);
        let black = alacritty_terminal::vte::ansi::Rgb::default();
        let colors = Colors {
            fg: black,
            bg: black,
        };
        let (session, writes) = Session::detached(Size { cols: 56, rows: 36 }, colors);
        card.pty = Some(session);
        model.add_card(card);
        (id, writes)
    }

    pub(crate) fn press(code: KeyCode) -> AppEvent {
        AppEvent::Input(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    #[test]
    fn moves_the_selection_and_focus() {
        let mut m = sample(&["a", "b", "c"]);
        m.update(press(KeyCode::Char('j')));
        m.update(press(KeyCode::Char('j')));
        m.update(press(KeyCode::Char('j')));
        assert_eq!(m.selected, 2, "stops at the last project");
        m.update(press(KeyCode::Char('g')));
        m.update(press(KeyCode::Char('g')));
        assert_eq!(m.selected, 0);
        m.update(press(KeyCode::Enter));
        assert_eq!(m.focus, Focus::Sessions);
        m.update(press(KeyCode::Char('j')));
        assert_eq!(m.selected, 0, "the sessions pane does not move projects");
        m.update(press(KeyCode::Char('l')));
        assert_eq!(m.focus, Focus::Sessions, "no session: no INTERACT");
        m.update(press(KeyCode::Char('h')));
        assert_eq!(m.focus, Focus::Projects);
    }

    #[test]
    fn digits_move_at_once_and_extend_within_the_window() {
        let names: Vec<String> = (1..=20).map(|i| format!("p{i:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut m = sample(&names);
        let type_keys = |m: &mut Model, keys: &str| {
            for ch in keys.chars() {
                m.update(press(KeyCode::Char(ch)));
            }
        };
        let cases = [
            ("1", 0, "the first digit moves at once"),
            ("6", 15, "a digit within the window extends: 16"),
            ("9", 15, "169 is out of range; 16 stays"),
            ("j3", 2, "another key ends the number"),
            ("0", 2, "30 is out of range; 3 stays"),
        ];
        for (keys, selected, why) in cases {
            type_keys(&mut m, keys);
            assert_eq!(m.selected, selected, "{why}");
        }
        m.now += JUMP_WINDOW;
        type_keys(&mut m, "2");
        assert_eq!(
            m.selected, 1,
            "after the window a digit starts a new number"
        );
        m.now += JUMP_WINDOW;
        type_keys(&mut m, "0");
        assert_eq!(m.selected, 1, "0 alone does nothing");
        assert!(m.message.is_none());
    }

    #[test]
    fn ctrl_h_and_ctrl_l_switch_panes_even_in_interact() {
        let ctrl = |ch| {
            AppEvent::Input(Event::Key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::CONTROL,
            )))
        };
        let mut m = sample(&["a"]);
        let (_id, writes) = with_session(&mut m, "s");
        assert_eq!(m.focus, Focus::Output);
        m.update(ctrl('l'));
        assert_eq!(m.focus, Focus::Output, "already rightmost");
        m.update(ctrl('h'));
        assert_eq!(m.focus, Focus::Sessions, "leaves INTERACT");
        m.update(ctrl('h'));
        assert_eq!(m.focus, Focus::Projects);
        m.update(ctrl('l'));
        m.update(ctrl('l'));
        assert_eq!(m.focus, Focus::Output, "back into INTERACT");
        assert!(writes.try_recv().is_err(), "nothing reached the agent");
    }

    #[test]
    fn r_opens_an_agents_list_of_past_sessions() {
        let mut m = sample(&["a"]);
        let Some(Cmd::Launch(req)) = m.update(press(KeyCode::Char('r'))) else {
            panic!("only claude is installed: launch at once");
        };
        assert!(req.launch.pick && req.launch.resume.is_none());
        assert_eq!(req.kind, Kind::Claude);
        m.found[1] = Some("/bin/codex".into());
        assert!(m.update(press(KeyCode::Char('r'))).is_none());
        assert_eq!(m.overlay, Some(Overlay::ResumeAgent(Kind::Claude)));
        m.update(press(KeyCode::Right));
        let Some(Cmd::Launch(req)) = m.update(press(KeyCode::Enter)) else {
            panic!("enter opens the chosen agent's list");
        };
        assert_eq!(req.kind, Kind::Codex);
        assert!(m.overlay.is_none());
    }

    #[test]
    fn cmd_alt_or_ctrl_digits_focus_a_pane_even_in_interact() {
        let chord = |ch, mods| AppEvent::Input(Event::Key(KeyEvent::new(KeyCode::Char(ch), mods)));
        let mut m = sample(&["a"]);
        let (_id, writes) = with_session(&mut m, "s");
        assert_eq!(m.focus, Focus::Output);
        m.update(chord('1', KeyModifiers::SUPER));
        assert_eq!(m.focus, Focus::Projects, "cmd-1 leaves INTERACT");
        m.update(chord('2', KeyModifiers::ALT));
        assert_eq!(m.focus, Focus::Sessions);
        m.update(chord('3', KeyModifiers::CONTROL));
        assert_eq!(m.focus, Focus::Output);
        assert!(writes.try_recv().is_err(), "nothing reached the agent");
        assert_eq!(
            m.selected, 0,
            "plain digits still jump projects, chords do not"
        );
    }

    #[test]
    fn x_on_an_outside_session_asks_before_stopping_it() {
        let mut m = sample(&["a"]);
        let project = m.selected_project().unwrap().path.clone();
        let ext = External {
            kind: Kind::Claude,
            pid: 4242,
            cwd: project,
            name: "outside".into(),
            status: Some("idle".into()),
            session_id: None,
            started_ms: None,
        };
        m.update(AppEvent::External(vec![ext]));
        m.focus = Focus::Sessions;
        assert!(m.update(press(KeyCode::Char('x'))).is_none());
        assert!(matches!(m.overlay, Some(Overlay::StopOutside(_))));
        assert!(m.update(press(KeyCode::Char('n'))).is_none(), "n keeps it");
        assert!(m.overlay.is_none());
        m.update(press(KeyCode::Char('x')));
        assert_eq!(
            m.update(press(KeyCode::Char('y'))),
            Some(Cmd::StopOutside(4242))
        );
    }

    #[test]
    fn filters_projects_and_clears_on_esc() {
        let mut m = sample(&["kedai-web", "pasar-mobile", "teh-cli"]);
        m.update(press(KeyCode::Char('/')));
        for ch in "EH".chars() {
            m.update(press(KeyCode::Char(ch)));
        }
        assert!(m.filtering);
        let names: Vec<&str> = m.visible().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["teh-cli"]);
        m.update(press(KeyCode::Enter));
        assert!(!m.filtering && m.filter == "EH");
        m.update(press(KeyCode::Char('/')));
        m.update(press(KeyCode::Esc));
        assert_eq!(m.visible().len(), 3);
    }

    #[test]
    fn settings_screen_previews_the_theme_and_submits() {
        let mut m = sample(&["a"]);
        if let Some(settings) = &mut m.settings {
            settings.workspace = std::env::temp_dir();
        }
        m.update(press(KeyCode::Char(',')));
        m.update(press(KeyCode::Down));
        m.update(press(KeyCode::Right));
        assert_eq!(m.view_theme().name, ThemeName::Light, "live preview");
        assert_eq!(m.theme.name, ThemeName::Dark, "not applied yet");
        let Some(Cmd::Apply(settings)) = m.update(press(KeyCode::Enter)) else {
            panic!("enter saves");
        };
        assert_eq!(settings.theme, ThemeChoice::Light);
        m.update(press(KeyCode::Char(',')));
        m.update(press(KeyCode::Esc));
        assert!(m.overlay.is_none());
    }

    #[test]
    fn quits_and_redraws_through_commands() {
        let mut m = sample(&[]);
        assert_eq!(m.update(press(KeyCode::Char('R'))), Some(Cmd::Redraw));
        assert_eq!(m.update(press(KeyCode::Char('?'))), None);
        assert_eq!(m.overlay, Some(Overlay::Help));
        m.update(press(KeyCode::Char('q')));
        assert!(m.overlay.is_none(), "any key closes help");
        assert_eq!(m.update(press(KeyCode::Char('q'))), Some(Cmd::Quit));
    }

    #[test]
    fn picker_launches_with_the_default_agent() {
        let mut m = sample(&["kedai-web"]);
        m.update(press(KeyCode::Char('n')));
        for c in "-fix it".chars() {
            m.update(press(KeyCode::Char(c)));
        }
        let Some(Cmd::Launch(req)) = m.update(press(KeyCode::Enter)) else {
            panic!("enter launches");
        };
        assert_eq!(req.kind, Kind::Claude);
        assert_eq!(req.project, PathBuf::from("/Users/me/Works/OSBR/kedai-web"));
        assert_eq!(req.launch.prompt.as_deref(), Some("-fix it"));
        assert_eq!(req.launch.name.as_deref(), Some("-fix it"));
    }

    #[test]
    fn picker_refuses_an_agent_that_is_not_installed() {
        let mut m = sample(&["kedai-web"]);
        m.found = [None, None];
        m.update(press(KeyCode::Char('n')));
        assert_eq!(m.update(press(KeyCode::Enter)), None);
        assert_eq!(
            m.message.as_deref(),
            Some("claude not found on PATH. Install Claude Code, then n again.")
        );
    }

    #[test]
    fn quit_with_running_sessions_asks_and_waits_for_them() {
        let mut m = sample(&["a"]);
        let (id, _writes) = with_session(&mut m, "s");
        m.focus = Focus::Sessions;
        assert_eq!(
            m.update(press(KeyCode::Char('q'))),
            Some(Cmd::OpenStop(StopKind::Quit)),
            "asks after a fresh scan"
        );
        assert_eq!(
            m.open_stop(StopKind::Quit, &[], &std::collections::HashMap::new()),
            None
        );
        assert!(matches!(m.overlay, Some(Overlay::Stop(_))));
        assert_eq!(
            m.update(press(KeyCode::Char('y'))),
            None,
            "waits for the session"
        );
        assert!(m.quitting.is_some());
        assert_eq!(
            m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(143)))),
            Some(Cmd::Quit)
        );
        assert_eq!(m.cards[0].state, sessions::State::Stopped);
    }

    /// Returns a socket line for `id` with the given hook payload.
    pub(crate) fn hook_line(id: SessionId, payload: &str) -> AppEvent {
        let raw = serde_json::from_str(payload).unwrap();
        let wire = Wire {
            mc_session: id.0.hyphenated().to_string(),
            event: crate::ipc::trim(&raw),
            usage: None,
        };
        AppEvent::Hook(serde_json::to_vec(&wire).unwrap())
    }

    #[test]
    fn hook_lines_move_states_and_raise_alerts() {
        let mut m = sample(&["a"]);
        let (id, _w) = with_session(&mut m, "s");
        m.cards[0].expect_hooks();
        m.update(hook_line(id, r#"{"hook_event_name":"UserPromptSubmit"}"#));
        assert_eq!(m.cards[0].state, State::Working);
        assert_eq!(crate::ui::title(&m), "bungkus-mc · 1 working");
        m.update(hook_line(
            id,
            r#"{"hook_event_name":"Notification","notification_type":"permission_prompt"}"#,
        ));
        assert_eq!(m.cards[0].state, State::NeedsYou);
        assert_eq!(m.alerts.len(), 1);
        assert_eq!(crate::ui::title(&m), "bungkus-mc · 1 needs you");
        m.update(hook_line(
            id,
            r#"{"hook_event_name":"Notification","notification_type":"permission_prompt"}"#,
        ));
        assert_eq!(m.alerts.len(), 1, "no second alert while still needing you");
        m.update(AppEvent::Hook(b"not json".to_vec()));
        m.update(hook_line(SessionId::new(), r#"{"hook_event_name":"Stop"}"#));
        assert_eq!(
            m.cards[0].state,
            State::NeedsYou,
            "garbage and unknown sessions are dropped"
        );
    }

    /// Returns a socket line for `id` carrying the status-line fixture.
    pub(crate) fn usage_line(id: SessionId) -> AppEvent {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/ipc/testdata/statusline.json");
        let raw = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let wire = Wire {
            mc_session: id.0.hyphenated().to_string(),
            event: crate::ipc::HookEvent::default(),
            usage: Some(crate::agent::usage::from_statusline(&raw)),
        };
        AppEvent::Hook(serde_json::to_vec(&wire).unwrap())
    }

    #[test]
    fn usage_lines_update_the_card_name_and_limits() {
        let mut m = sample(&["a"]);
        let (id, _w) = with_session(&mut m, "picker name");
        m.cards[0].expect_hooks();
        m.update(usage_line(id));
        assert_eq!(m.cards[0].name, "sl-test", "session_name wins");
        assert_eq!(m.cards[0].usage.as_ref().unwrap().ctx_pct, Some(23.0));
        assert_eq!(m.limits[Kind::Claude as usize].len(), 2);
        assert!(
            !m.cards[0].output_only(m.now + HOOK_GRACE),
            "a usage line counts as a sign of life"
        );
    }

    #[test]
    fn codex_session_start_asks_to_watch_the_rollout_once() {
        let mut m = sample(&["a"]);
        let (id, _w) = with_session(&mut m, "cx");
        m.cards[0].kind = Kind::Codex;
        m.cards[0].expect_hooks();
        let start = r#"{"hook_event_name":"SessionStart","session_id":"s1","transcript_path":"/c/r.jsonl"}"#;
        let cmd = m.update(hook_line(id, start));
        assert_eq!(
            cmd,
            Some(Cmd::WatchRollout(id, PathBuf::from("/c/r.jsonl")))
        );
        m.cards[0].rollout_stop = Some(std::sync::mpsc::channel().0);
        assert_eq!(m.update(hook_line(id, start)), None, "already watching");
        let window = Window {
            label: "5h".into(),
            used_pct: 1.0,
            resets_at: None,
        };
        let usage = crate::agent::usage::Usage {
            limits: vec![window],
            ..Default::default()
        };
        m.update(AppEvent::Usage(id, usage));
        assert_eq!(m.limits[Kind::Codex as usize].len(), 1);
    }

    #[test]
    fn resume_and_forget_act_on_finished_sessions() {
        let mut m = sample(&["a"]);
        let (id, _w) = with_session(&mut m, "s");
        m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(0))));
        assert!(m.state_dirty);
        m.focus = Focus::Sessions;
        let Some(Cmd::Launch(req)) = m.update(press(KeyCode::Char('r'))) else {
            panic!("r resumes");
        };
        assert_eq!(req.replaces, Some(id));
        assert_eq!(req.launch.resume, Some(id.0.hyphenated().to_string()));
        m.update(press(KeyCode::Char('d')));
        assert_eq!(m.overlay, Some(Overlay::Forget(id)));
        m.update(press(KeyCode::Char('y')));
        assert!(m.cards.is_empty());
    }

    #[test]
    fn bang_jumps_to_the_next_needs_you_across_projects() {
        let mut m = sample(&["a", "b"]);
        let (first, _w1) = with_session(&mut m, "one");
        m.update(press(KeyCode::Char('\x1c')));
        m.focus = Focus::Projects;
        m.update(press(KeyCode::Char('j')));
        let (second, _w2) = with_session(&mut m, "two");
        for id in [first, second] {
            m.update(hook_line(
                id,
                r#"{"hook_event_name":"Notification","notification_type":"permission_prompt"}"#,
            ));
        }
        m.focus = Focus::Projects;
        m.update(press(KeyCode::Char('!')));
        assert_eq!(
            (m.selected, m.focus),
            (0, Focus::Output),
            "wraps to project a"
        );
        m.focus = Focus::Sessions;
        m.update(press(KeyCode::Char('!')));
        assert_eq!(m.selected, 1, "then project b");
        m.cards.iter_mut().for_each(|c| c.state = State::YourTurn);
        m.focus = Focus::Sessions;
        m.update(press(KeyCode::Char('!')));
        assert_eq!(m.message.as_deref(), Some("nobody needs you right now"));
    }

    #[test]
    fn a_session_exit_leaves_interact() {
        let mut m = sample(&["a"]);
        let (id, _writes) = with_session(&mut m, "s");
        assert_eq!(m.focus, Focus::Output);
        m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(0))));
        assert_eq!(m.focus, Focus::Sessions);
        assert_eq!(m.cards[0].state, sessions::State::Wrapped);
    }
}
