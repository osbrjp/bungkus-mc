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
use crate::app::tools::{TermView, Tool};
use crate::external::External;
use crate::ipc::Wire;
use crate::proc::Proc;
use crate::store::config::Settings;
use crate::term::keys::Chord;
use crate::term::session::Session;
use crate::term::{PtyEvent, SessionId};
use crate::ui::keymap::{self, Action, Lookup, Scope};
use crate::ui::mascot::NOTICE;
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
    /// The `a` new-project dialog.
    NewProject(crate::app::quick::NewProject),
    /// The `w` workspace switcher.
    Switcher(crate::app::workspaces::Switcher),
    /// "Move these projects' folders to the Trash?"
    TrashProject(Vec<Project>),
    /// "Remove this project's unused worktrees?"
    CleanWorktrees(Project),
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

/// The line of the projects list that shows or hides the projects that
/// are not recent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rest {
    /// The row it sits before: the first project that is not recent.
    pub at: usize,
    /// How many projects are not recent.
    pub count: usize,
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
    /// Open this project folder in the user's editor (`o`).
    OpenEditor(PathBuf),
    /// Start the terminal pane's shell in this folder (`t`).
    OpenTerminal(PathBuf),
    /// Write the dragged pane widths to `config.json`.
    SaveWidths(crate::ui::Widths),
    /// Switch to this workspace (saved first in the list), then select this
    /// session when given (`!` following a session into its workspace).
    SwitchWorkspace(PathBuf, Option<SessionId>),
    /// Write the saved workspaces to `config.json`.
    SaveWorkspaces,
    /// Check for a newer release and install it in the background (`U`).
    Update,
    /// Ask kitty to focus its window on that side (`left`, `right`, `top`,
    /// `bottom`): `ctrl-h/j/k/l` past mc's own edge, as vim-kitty-navigator
    /// does.
    KittyFocus(&'static str),
    /// Create this project folder: `git init`, plus `AGENTS.md` and a
    /// `CLAUDE.md` that imports it when the flag is set.
    NewProject(PathBuf, bool),
    /// Move these project folders to the Trash (after the user confirmed).
    TrashProject(Vec<PathBuf>),
    /// Move trashed projects back: each `(folder, where it went)`.
    RestoreProject(Vec<(PathBuf, PathBuf)>),
    /// Stop outside session `pid` (SIGTERM, after the user confirmed):
    /// still ours, still an agent, same process.
    StopOutside(i32),
    /// Create this project folder (`git init`), then move quick session
    /// `SessionId` into it.
    CreateProject(SessionId, PathBuf),
    /// Remove the worktree with this name of this project, left by a
    /// forgotten session, unless it has uncommitted files.
    RemoveWorktree(PathBuf, String),
    /// Remove this project's unused worktrees (after the user confirmed).
    CleanWorktrees(PathBuf),
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
    /// The git branch and status of each session folder that is in a
    /// repository, as last read.
    pub repos: std::collections::HashMap<PathBuf, crate::app::repo::Status>,
    /// Whether a background read of [`Model::repos`] is under way.
    pub repo_scan: bool,
    /// Shows every key mc receives in the hint line (`BUNGKUS_MC_DEBUG_KEYS`),
    /// to find chords a terminal keeps for itself.
    pub debug_keys: bool,
    /// Project asked for with `-p`, selected by the next [`Model::apply`]
    /// whose workspace has it; that apply clears it either way.
    pub want_project: Option<String>,
    /// Whether mc runs in kitty (`KITTY_WINDOW_ID`): `ctrl-h/j/k/l` past
    /// mc's edges then move to kitty's neighbouring window.
    pub kitty: bool,
    /// Every session id this mc loaded or started (forgotten ones too), so a
    /// save replaces only its own records in the shared `sessions.json`.
    pub known: std::collections::HashSet<SessionId>,
    /// Whether a `U` update is running.
    pub updating: bool,
    /// The newer release the check found; stays in the header until mc is
    /// updated.
    pub newer: Option<String>,
    /// Whether mc starts again (the updated binary) once it has quit.
    pub restart: bool,
    /// Saved workspaces, most recently used first (`w`).
    pub workspaces: Vec<PathBuf>,
    /// The projects last moved to the Trash this run, each `(folder, where
    /// it went)`, for `u`.
    pub last_trash: Vec<(PathBuf, PathBuf)>,
    /// The other end of the projects pane's line selection (`V`), when on.
    pub visual: Option<usize>,
    /// The quick session whose popup shows (keys go to it).
    pub popup: Option<SessionId>,
    /// Whether the popup's menu (`ctrl-\`: hide, move, new project) is open.
    pub popup_menu: bool,
    /// Whether the projects that are not recent show (`e`).
    pub show_rest: bool,
    /// The editor whose popup shows (`o`; every key goes to it).
    pub editor: Option<Tool>,
    /// The terminal pane's shells (`t`), one per folder it was opened in
    /// (a project, or the workspace root); they keep running while hidden.
    pub shells: Vec<(PathBuf, Tool)>,
    /// How the terminal pane shows.
    pub term_view: TermView,
    /// The `quick` row that leads the projects list while quick sessions
    /// exist; its path is the workspace root.
    pub quick_row: Project,
    /// When the band mascot was last clicked and which quote it says.
    pub poke: Option<(Instant, usize)>,
    /// The last notification and when it was raised; the band mascot says
    /// it for [`crate::ui::mascot::NOTICE`].
    pub notice: Option<(Instant, String)>,
    /// The "elsewhere" row that ends the projects list while an outside
    /// session runs in no project folder; its path is empty.
    pub elsewhere: Project,
}

/// Returns `key` with the list-navigation chords every dialog accepts
/// turned into arrows: `ctrl-j`/`ctrl-n` are `↓`, `ctrl-k`/`ctrl-p` are
/// `↑` (vim and readline habits), so each dialog only handles arrows.
#[must_use]
pub(crate) fn nav_alias(key: KeyEvent) -> KeyEvent {
    if key.modifiers != KeyModifiers::CONTROL {
        return key;
    }
    let code = match key.code {
        KeyCode::Char('j' | 'n') => KeyCode::Down,
        KeyCode::Char('k' | 'p') => KeyCode::Up,
        _ => return key,
    };
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        ..key
    }
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
            repos: std::collections::HashMap::new(),
            repo_scan: false,
            poke: None,
            notice: None,
            popup: None,
            last_trash: Vec::new(),
            workspaces: Vec::new(),
            updating: false,
            newer: None,
            restart: false,
            known: std::collections::HashSet::new(),
            kitty: false,
            visual: None,
            popup_menu: false,
            show_rest: false,
            editor: None,
            shells: Vec::new(),
            term_view: TermView::Hidden,
            debug_keys: false,
            want_project: None,
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

    /// Returns the rows of the projects list in display order and, when
    /// the projects are grouped, their "rest" line.
    ///
    /// The `quick` row (number 0) leads while quick sessions exist, and the
    /// [`Model::elsewhere`] row ends the list while an outside session runs
    /// in no project folder. Between them, recent projects come first: the
    /// ones with a session (of mc's, running or finished, or an outside
    /// one), a repository and its worktrees counting together. The rest
    /// hide behind the rest line until [`Model::show_rest`].
    ///
    /// Nothing is grouped while a search is typed (it finds every
    /// project), or when no project or every project is recent.
    fn grouped(&self) -> (Vec<&Project>, Option<Rest>) {
        fn family(project: &Project) -> &str {
            project.worktree_of.as_deref().unwrap_or(&project.name)
        }
        let needle = self.filter.to_lowercase();
        let elsewhere = (!self.external_in(Path::new("")).is_empty()).then_some(&self.elsewhere);
        let quick = self
            .cards
            .iter()
            .any(|c| self.is_quick(c))
            .then_some(&self.quick_row);
        let recent: std::collections::HashSet<&str> = self
            .projects
            .iter()
            .filter(|p| {
                self.cards.iter().any(|c| c.project == p.path)
                    || !self.external_in(&p.path).is_empty()
            })
            .map(family)
            .collect();
        let (first, rest): (Vec<&Project>, Vec<&Project>) = self
            .projects
            .iter()
            .partition(|p| recent.contains(family(p)));
        if !needle.is_empty() || first.is_empty() || rest.is_empty() {
            let all = quick
                .into_iter()
                .chain(&self.projects)
                .chain(elsewhere)
                .filter(|p| p.name.to_lowercase().contains(&needle))
                .collect();
            return (all, None);
        }
        let group = Rest {
            at: usize::from(quick.is_some()) + first.len(),
            count: rest.len(),
        };
        let rest = if self.show_rest { rest } else { Vec::new() };
        let rows = quick
            .into_iter()
            .chain(first)
            .chain(rest)
            .chain(elsewhere)
            .collect();
        (rows, Some(group))
    }

    /// Returns the rows of the projects list: the projects matching the
    /// search, or the recent ones and (when shown) the rest, with the
    /// `quick` and elsewhere rows around them (see [`Model::rest`]).
    #[must_use]
    pub(crate) fn visible(&self) -> Vec<&Project> {
        self.grouped().0
    }

    /// Returns the rest line of the projects list, when recent projects
    /// are grouped apart from the others.
    #[must_use]
    pub(crate) fn rest(&self) -> Option<Rest> {
        self.grouped().1
    }

    /// Shows or hides the projects that are not recent (`e`, or a click on
    /// the rest line), keeping the selected project selected while it
    /// stays in the list.
    pub(crate) fn toggle_rest(&mut self) {
        if self.rest().is_none() {
            return;
        }
        let path = self.selected_project().map(|p| p.path.clone());
        self.show_rest = !self.show_rest;
        let visible = self.visible();
        let last = visible.len().saturating_sub(1);
        let kept = path.and_then(|path| visible.iter().position(|p| p.path == path));
        if kept.is_none() {
            self.card = 0;
        }
        self.selected = kept.unwrap_or(last);
    }

    /// Selects the project at `path`, showing the rest when it hides
    /// there.
    ///
    /// # Returns
    ///
    /// Whether the project is in the list.
    pub(crate) fn select_project(&mut self, path: &Path) -> bool {
        let row = |m: &Self| m.visible().iter().position(|p| p.path == path);
        if row(self).is_none() && self.rest().is_some() {
            self.show_rest = true;
        }
        let row = row(self);
        if let Some(row) = row {
            self.selected = row;
        }
        row.is_some()
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
            .filter_map(|c| c.pty.as_ref()?.sync_deadline())
            .chain(self.tools().filter_map(Session::sync_deadline));
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
        let notice = self.notice.as_ref().map(|(at, _)| *at + NOTICE);
        syncs
            .chain(takeover)
            .chain(poke)
            .chain(notice)
            .chain(stops)
            .chain(silent)
            .chain(tick)
            .chain(scan)
            .chain(plans)
            .min()
    }

    /// Opens the settings screen (or, before first run, the wizard).
    pub(crate) fn open_form(&mut self, kind: FormKind, field: Field) {
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
    /// The selection is a row and a position, and events other than input
    /// reorder both (cards sort by state, the `quick` and elsewhere rows
    /// come and go), so after one the selection is put back on the project
    /// and session it was on: the output pane and INTERACT never switch to
    /// another session by themselves.
    ///
    /// # Returns
    ///
    /// The command the loop must run, if any.
    pub(crate) fn update(&mut self, event: AppEvent) -> Option<Cmd> {
        let pinned = (!matches!(event, AppEvent::Input(_))).then(|| {
            (
                self.selected_project().map(|p| p.path.clone()),
                self.selected_card().map(|i| self.cards[i].id),
            )
        });
        let cmd = self.handle(event);
        if let Some((project, card)) = pinned {
            if let Some(row) =
                project.and_then(|path| self.visible().iter().position(|p| p.path == path))
            {
                self.selected = row;
            }
            if let Some(pos) = card.and_then(|id| {
                self.project_cards()
                    .iter()
                    .position(|&i| self.cards[i].id == id)
            }) {
                self.card = pos;
            }
        }
        cmd
    }

    /// Applies one event; [`Model::update`] keeps the selection in place.
    fn handle(&mut self, event: AppEvent) -> Option<Cmd> {
        match event {
            AppEvent::Tick => {
                self.frame = self.frame.wrapping_add(1);
                self.poke = self
                    .poke
                    .filter(|(at, _)| self.now < *at + crate::ui::mascot::POKE);
                self.notice = self.notice.take().filter(|(at, _)| self.now < *at + NOTICE);
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
            AppEvent::Repos(repos) => {
                self.repos = repos.into_iter().collect();
                self.repo_scan = false;
            }
            AppEvent::External(list) => {
                self.external = list;
                self.selected = self.selected.min(self.visible().len().saturating_sub(1));
                self.card = self.card.min(self.session_rows().saturating_sub(1));
            }
            AppEvent::HostGone => return self.host_gone(),
            AppEvent::UpdateAvailable(tag) => {
                self.message = Some(format!("bungkus-mc {tag} is out — U updates and restarts"));
                self.newer = Some(tag);
            }
            AppEvent::Updated(result) => return self.updated(result),
            AppEvent::Worktrees(text) => self.message = Some(text),
            AppEvent::Pty(PtyEvent::Output(id, bytes)) => {
                if let Some(pty) = self.tool_mut(id) {
                    pty.advance(&bytes);
                } else if let Some(pty) = self.card_mut(id).and_then(|c| c.pty.as_mut()) {
                    pty.advance(&bytes);
                }
            }
            AppEvent::Hook(line) => {
                if let Some(cmd) = self.hook(&line) {
                    return Some(cmd);
                }
            }
            AppEvent::Usage(id, usage) => self.codex_usage(id, usage),
            AppEvent::Pty(PtyEvent::Exited(id, code)) if self.tool_exited(id, code) => {}
            AppEvent::Pty(PtyEvent::Exited(id, code)) => {
                crate::debug_log!("{} exited: {code:?}", id.short());
                let now = self.now;
                self.state_dirty = true;
                let selected = self.selected_card().map(|i| self.cards[i].id) == Some(id);
                if let Some(card) = self.card_mut(id) {
                    card.exited(code, now);
                    if let State::Failed(reason) = &card.state {
                        let text = format!("{} failed: {reason}", card.id.short());
                        self.notice = Some((now, text.clone()));
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
                if self.focus == Focus::Output && selected {
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
                self.state_dirty = true;
            }
            card.report(usage);
            return None;
        }
        let before = card.state.clone();
        let bound = card.agent_session.is_some();
        card.reduce(&wire.event, now);
        crate::debug_log!(
            "{} hook {} → {:?}",
            card.id.short(),
            wire.event.name,
            card.state
        );
        if card.state == State::NeedsYou && before != State::NeedsYou {
            let text = format!("{} needs you: {}", card.id.short(), card.name);
            self.notice = Some((now, text.clone()));
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
            self.state_dirty = true;
        }
        if let Some(card) = self.card_mut(id) {
            card.report(usage);
        }
    }

    /// Jumps to the next session that needs you, across projects: selects
    /// its project and card and enters INTERACT (DESIGN §8.1).
    fn next_needs_you(&mut self) -> Option<Cmd> {
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
            let elsewhere = self
                .cards
                .iter()
                .find(|card| card.state == State::NeedsYou)
                .and_then(|card| Some((self.workspace_of(&card.project)?, card.id)));
            if let Some((workspace, id)) = elsewhere {
                return Some(Cmd::SwitchWorkspace(workspace, Some(id)));
            }
            self.message = Some("nobody needs you right now".into());
            return None;
        };
        self.selected = p;
        self.card = c;
        self.focus = Focus::Output;
        None
    }

    /// Selects session `id` (its project row and card) and focuses its
    /// output, after a switch brought its workspace up.
    pub(crate) fn select_session(&mut self, id: SessionId) {
        let Some(project) = self
            .cards
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.project.clone())
        else {
            return;
        };
        if let Some(row) = self.visible().iter().position(|p| p.path == project) {
            self.selected = row;
        }
        if let Some(pos) = self
            .project_cards()
            .iter()
            .position(|&i| self.cards[i].id == id)
        {
            self.card = pos;
            self.focus = Focus::Output;
        }
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
        if self.overlay.is_none()
            && let std::ops::ControlFlow::Break(cmd) = self.tool_key(key)
        {
            return cmd;
        }
        if let Some(side) = self.kitty_edge(key) {
            return Some(Cmd::KittyFocus(side));
        }
        if self.focus == Focus::Output && self.overlay.is_none() {
            self.interact_key(key);
            return None;
        }
        self.message = None;
        if let Some(overlay) = self.overlay.take() {
            return self.overlay_key(overlay, nav_alias(key));
        }
        if self.filtering {
            self.filter_key(nav_alias(key));
            return None;
        }
        if self.visual.is_some() && self.focus == Focus::Projects {
            match key.code {
                KeyCode::Esc => {
                    self.visual = None;
                    return None;
                }
                KeyCode::Char('d') if key.modifiers.is_empty() => {
                    self.pending = None;
                    self.ask_trash();
                    return None;
                }
                _ => {}
            }
        }
        if self.focus != Focus::Projects {
            self.visual = None;
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
            .first()
            .is_some_and(|p| self.root() == Some(p.path.as_path()));
        let offset = usize::from(quick);
        let numbered = visible.len() - offset;
        if n == 0 {
            if quick {
                self.selected = 0;
                self.card = 0;
            }
            return;
        }
        self.jump = Some((n, self.now));
        if n <= numbered {
            self.selected = n - 1 + offset;
            self.card = 0;
        }
    }

    /// Passes a key to an overlay; the overlay stays unless it closed.
    fn overlay_key(&mut self, overlay: Overlay, key: KeyEvent) -> Option<Cmd> {
        match overlay {
            Overlay::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('?' | ' ') => None,
                _ => self.key(key),
            },
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
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Char('h' | 'l' | 'j' | 'k')
                | KeyCode::Tab => {
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
            Overlay::NewProject(dialog) => self.new_project_key(dialog, key),
            Overlay::Switcher(switcher) => self.switcher_key(switcher, key),
            Overlay::TrashProject(projects) => (key.code == KeyCode::Char('y'))
                .then(|| Cmd::TrashProject(projects.into_iter().map(|p| p.path).collect())),
            Overlay::CleanWorktrees(project) => {
                (key.code == KeyCode::Char('y')).then_some(Cmd::CleanWorktrees(project.path))
            }
            Overlay::Forget(id) => {
                if key.code != KeyCode::Char('y') {
                    return None;
                }
                let worktree = self
                    .cards
                    .iter()
                    .find(|c| c.id == id)
                    .and_then(|c| Some((c.project.clone(), c.worktree.clone()?)));
                self.cards.retain(|c| c.id != id);
                self.card = self.card.min(self.project_cards().len().saturating_sub(1));
                self.state_dirty = true;
                worktree.map(|(project, name)| Cmd::RemoveWorktree(project, name))
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

    /// Asks before moving the selected projects' folders to the Trash: the
    /// `V` selection, else the highlighted one. The `quick` and `elsewhere`
    /// rows are skipped; any project with running sessions refuses all.
    fn ask_trash(&mut self) {
        let visible = self.visible();
        let (from, to) = match self.visual {
            Some(anchor) => (anchor.min(self.selected), anchor.max(self.selected)),
            None => (self.selected, self.selected),
        };
        let projects: Vec<Project> = visible
            .iter()
            .take(to + 1)
            .skip(from)
            .filter(|p| !p.path.as_os_str().is_empty() && self.root() != Some(p.path.as_path()))
            .map(|p| (*p).clone())
            .collect();
        self.visual = None;
        if projects.is_empty() {
            return;
        }
        if let Some(busy) = projects.iter().find(|p| {
            self.cards
                .iter()
                .any(|c| c.project == p.path && c.running())
        }) {
            self.message = Some(format!(
                "{} has running sessions; stop them first (x).",
                busy.name
            ));
            return;
        }
        self.overlay = Some(Overlay::TrashProject(projects));
    }

    /// Puts the projects last moved to the Trash back (`u`), or says there
    /// is nothing to undo.
    fn undo_trash(&mut self) -> Option<Cmd> {
        if self.last_trash.is_empty() {
            self.message = Some("Nothing to undo.".into());
            return None;
        }
        Some(Cmd::RestoreProject(std::mem::take(&mut self.last_trash)))
    }

    /// Returns the kitty window to move to when `key` is `ctrl-h/j/k/l` past
    /// mc's edge: `ctrl-h` on the projects pane, `ctrl-l` in the output pane,
    /// `ctrl-j`/`ctrl-k` on the two list panes (in the output pane they
    /// belong to the agent: `ctrl-j` is Claude's newline). Only in kitty, and
    /// never while a dialog, the search or a quick popup has the keys.
    fn kitty_edge(&self, key: KeyEvent) -> Option<&'static str> {
        if !self.kitty
            || key.modifiers != KeyModifiers::CONTROL
            || self.overlay.is_some()
            || self.popup.is_some()
            || self.filtering
        {
            return None;
        }
        match (key.code, self.focus) {
            (KeyCode::Char('h'), Focus::Projects) => Some("left"),
            (KeyCode::Char('l'), Focus::Output) => Some("right"),
            (KeyCode::Char('j'), Focus::Projects | Focus::Sessions) => Some("bottom"),
            (KeyCode::Char('k'), Focus::Projects | Focus::Sessions) => Some("top"),
            _ => None,
        }
    }

    /// Returns whether visible row `row` is inside the `V` selection.
    #[must_use]
    pub(crate) fn in_visual(&self, row: usize) -> bool {
        self.visual.is_some_and(|anchor| {
            (anchor.min(self.selected)..=anchor.max(self.selected)).contains(&row)
        })
    }

    /// Enters the selected row of the sessions pane: takes over an outside
    /// session, opens a quick session's popup (resuming it into the popup
    /// when it has ended), or enters INTERACT.
    fn enter_session(&mut self) -> Option<Cmd> {
        if let Some(ext) = self.selected_external().cloned() {
            return self.take_over(ext);
        }
        if let Some(card) = self.selected_card().map(|i| &self.cards[i])
            && self.is_quick(card)
        {
            if !card.running() {
                return self.resume();
            }
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
        self.known.insert(id);
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

    /// Starts the `U` update (one at a time).
    fn start_update(&mut self) -> Option<Cmd> {
        if self.updating {
            self.message = Some("Already updating…".into());
            return None;
        }
        self.updating = true;
        self.message = Some("Checking for an update…".into());
        Some(Cmd::Update)
    }

    /// Handles the end of a `U` update: an installed release quits (asking
    /// about running sessions first, as `q` does) and restarts on it.
    fn updated(&mut self, result: Result<Option<String>, String>) -> Option<Cmd> {
        self.updating = false;
        match result {
            Ok(Some(tag)) => {
                crate::debug_log!("updated to {tag}; restarting");
                self.message = Some(format!("Updated to {tag} — restarting…"));
                self.restart = true;
                return Some(self.request_quit());
            }
            Ok(None) => {
                self.message = Some(format!(
                    "bungkus-mc {} is up to date.",
                    env!("CARGO_PKG_VERSION")
                ));
            }
            Err(e) => {
                self.message = Some(format!(
                    "Update failed: {e} · bungkus-mc update in a terminal shows why"
                ));
            }
        }
        None
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
                    // Down on the last recent project opens the rest.
                    if delta == 1 && self.rest().is_some_and(|r| r.at == self.selected + 1) {
                        self.show_rest = true;
                    }
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
            Action::Stop => return self.stop_selected(),
            Action::Pane(n) => self.focus_pane(n),
            Action::TrashProject => self.ask_trash(),
            Action::NewProject => self.start_new_project(),
            Action::CleanWorktrees => self.ask_clean_worktrees(),
            Action::UndoTrash => return self.undo_trash(),
            Action::Visual => self.visual = self.visual.xor(Some(self.selected)),
            Action::QuickSession => return self.quick_session(),
            Action::ToggleRest => self.toggle_rest(),
            Action::Editor => return self.open_editor(),
            Action::Terminal => return self.toggle_terminal(),
            Action::Update => return self.start_update(),
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
            Action::NextNeedsYou => return self.next_needs_you(),
            Action::Workspace => self.open_switcher(),
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

    /// Asks before stopping the selected session (`x`): an outside one
    /// with its own dialog, one of mc's (when it runs) with the stop dialog.
    fn stop_selected(&mut self) -> Option<Cmd> {
        if let Some(ext) = self.selected_external().cloned() {
            self.overlay = Some(Overlay::StopOutside(ext));
            return None;
        }
        let i = self.selected_card().filter(|&i| self.cards[i].running())?;
        Some(Cmd::OpenStop(StopKind::Session(self.cards[i].id)))
    }

    /// Asks whether to remove the selected project's unused worktrees
    /// (`c`); not for the `quick` or elsewhere rows.
    fn ask_clean_worktrees(&mut self) {
        if let Some(project) = self
            .selected_project()
            .filter(|p| !p.path.as_os_str().is_empty() && self.root() != Some(&p.path))
        {
            self.overlay = Some(Overlay::CleanWorktrees(project.clone()));
        }
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
    ///   preselected, unless [`Model::want_project`] names one here.
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
        let wanted = self.want_project.take();
        let wanted = self
            .projects
            .iter()
            .find(|p| Some(&p.name) == wanted.as_ref())
            .or_else(|| self.projects.iter().find(|p| cwd.starts_with(&p.path)))
            .map(|p| p.path.clone());
        self.selected = 0;
        if let Some(path) = wanted {
            self.select_project(&path);
        }
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
    fn apply_selects_the_wanted_project_once() {
        let mut m = sample(&["a", "b", "c"]);
        let settings = m.settings.clone().unwrap();
        let projects = m.projects.clone();
        let scan = || Ok(projects.clone());
        m.want_project = Some("c".into());
        m.apply(settings.clone(), scan(), Path::new("/"));
        assert_eq!(m.selected_project().map(|p| p.name.as_str()), Some("c"));
        assert_eq!(m.want_project, None);
        m.want_project = Some("nope".into());
        m.apply(settings, scan(), Path::new("/"));
        assert_eq!(m.selected, 0);
        assert_eq!(m.want_project, None, "a miss is not retried for ever");
    }

    #[test]
    fn enter_in_interact_keeps_the_output_on_the_session_typed_into() {
        let mut m = sample(&["a"]);
        let (done, _done_writes) = with_session(&mut m, "done");
        let (typing, writes) = with_session(&mut m, "typing");
        for id in [done, typing] {
            m.update(hook_line(id, r#"{"hook_event_name":"Stop"}"#));
        }
        m.select_session(typing);
        m.update(press(KeyCode::Enter));
        assert!(writes.try_recv().is_ok(), "enter goes to the agent");
        m.update(hook_line(
            typing,
            r#"{"hook_event_name":"UserPromptSubmit"}"#,
        ));
        assert_eq!(m.card_mut(typing).unwrap().state, State::Working);
        assert_eq!(
            m.selected_card().map(|i| m.cards[i].id),
            Some(typing),
            "working sorts above your turn; the output pane does not move"
        );
    }

    #[test]
    fn the_selection_follows_its_session_when_cards_reorder() {
        let mut m = sample(&["a"]);
        let (first, _first_writes) = with_session(&mut m, "first");
        let (second, writes) = with_session(&mut m, "second");
        assert_eq!(m.selected_card().map(|i| m.cards[i].id), Some(second));
        m.update(AppEvent::Pty(PtyEvent::Exited(first, Some(0))));
        assert_eq!(
            m.selected_card().map(|i| m.cards[i].id),
            Some(second),
            "the wrapped card sorts below; the output pane stays on its session"
        );
        m.update(press(KeyCode::Char('y')));
        assert!(writes.try_recv().is_ok(), "keys still reach that session");
    }

    #[test]
    fn c_asks_before_cleaning_and_forgetting_names_the_worktree() {
        let mut m = sample(&["a"]);
        let path = m.projects[0].path.clone();
        m.update(press(KeyCode::Char('c')));
        assert!(matches!(m.overlay, Some(Overlay::CleanWorktrees(_))));
        assert_eq!(m.update(press(KeyCode::Char('n'))), None);
        m.focus = Focus::Sessions;
        m.update(press(KeyCode::Char('c')));
        assert_eq!(
            m.update(press(KeyCode::Char('y'))),
            Some(Cmd::CleanWorktrees(path.clone()))
        );
        let (id, _writes) = with_session(&mut m, "s");
        m.cards[0].worktree = Some("s-a3f1".into());
        m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(0))));
        m.focus = Focus::Sessions;
        m.update(press(KeyCode::Char('d')));
        assert_eq!(
            m.update(press(KeyCode::Char('y'))),
            Some(Cmd::RemoveWorktree(path, "s-a3f1".into()))
        );
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
    fn dd_and_a_v_selection_ask_before_trashing_projects() {
        let mut m = sample(&["a", "b", "c", "d"]);
        m.focus = Focus::Projects;
        m.update(press(KeyCode::Char('d')));
        m.update(press(KeyCode::Char('d')));
        let Some(Overlay::TrashProject(one)) = &m.overlay else {
            panic!("dd asks");
        };
        assert_eq!(one.len(), 1);
        assert!(m.update(press(KeyCode::Char('n'))).is_none(), "n keeps it");
        m.update(press(KeyCode::Char('V')));
        m.update(press(KeyCode::Char('j')));
        m.update(press(KeyCode::Char('j')));
        assert!(m.in_visual(1) && !m.in_visual(3));
        m.update(press(KeyCode::Char('d')));
        let Some(Cmd::TrashProject(paths)) = m.update(press(KeyCode::Char('y'))) else {
            panic!("y trashes the selection");
        };
        let names: Vec<_> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert!(m.visual.is_none());
        m.update(press(KeyCode::Char('V')));
        m.update(press(KeyCode::Esc));
        assert!(m.visual.is_none(), "esc ends the selection");
        m.update(press(KeyCode::Char('u')));
        assert_eq!(m.message.as_deref(), Some("Nothing to undo."));
    }

    #[test]
    fn the_key_menu_runs_the_key_pressed_in_it() {
        let mut m = sample(&["a", "b"]);
        m.focus = Focus::Projects;
        m.update(press(KeyCode::Char(' ')));
        assert_eq!(m.overlay, Some(Overlay::Help), "space opens the key menu");
        m.update(press(KeyCode::Char('j')));
        assert!(m.overlay.is_none());
        assert_eq!(m.selected, 1, "j ran");
        m.update(press(KeyCode::Char('?')));
        m.update(press(KeyCode::Esc));
        assert_eq!(
            (m.overlay.clone(), m.selected),
            (None, 1),
            "esc only closes"
        );
    }

    #[test]
    fn ctrl_j_k_n_p_move_in_every_dialog_and_the_search() {
        let ctrl = |ch| {
            AppEvent::Input(Event::Key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::CONTROL,
            )))
        };
        let mut m = sample(&["kedai", "kopi", "roti"]);
        m.update(press(KeyCode::Char('n')));
        let row = |m: &Model| match &m.overlay {
            Some(Overlay::Picker(p)) => p.row,
            other => panic!("picker: {other:?}"),
        };
        let first = row(&m);
        m.update(ctrl('j'));
        assert_ne!(row(&m), first, "ctrl-j moves down in the n picker");
        m.update(ctrl('k'));
        assert_eq!(row(&m), first, "ctrl-k moves back up");
        m.update(press(KeyCode::Esc));
        m.update(press(KeyCode::Char('/')));
        m.update(press(KeyCode::Char('k')));
        m.update(ctrl('n'));
        assert_eq!(m.selected, 1, "ctrl-n moves through the search matches");
        m.update(ctrl('p'));
        assert_eq!(m.selected, 0);
    }

    #[test]
    fn in_kitty_ctrl_hjkl_past_the_edge_moves_to_kittys_window() {
        let ctrl = |ch| {
            AppEvent::Input(Event::Key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::CONTROL,
            )))
        };
        let mut m = sample(&["a", "b"]);
        m.focus = Focus::Projects;
        assert_eq!(m.update(ctrl('h')), None, "outside kitty nothing leaves mc");
        m.kitty = true;
        assert_eq!(m.update(ctrl('h')), Some(Cmd::KittyFocus("left")));
        assert_eq!(m.update(ctrl('j')), Some(Cmd::KittyFocus("bottom")));
        assert_eq!(
            m.update(ctrl('l')),
            None,
            "ctrl-l still moves inside mc first"
        );
        assert_eq!(m.focus, Focus::Sessions);
        m.update(press(KeyCode::Char('w')));
        assert_eq!(
            m.update(ctrl('j')),
            None,
            "a dialog keeps ctrl-j for its list"
        );
    }

    #[test]
    fn u_updates_then_restarts_or_says_why_not() {
        let mut m = sample(&["a"]);
        assert_eq!(m.update(press(KeyCode::Char('U'))), Some(Cmd::Update));
        assert!(
            m.update(press(KeyCode::Char('U'))).is_none(),
            "one update at a time"
        );
        m.update(AppEvent::Updated(Ok(None)));
        assert!(
            m.message
                .as_deref()
                .unwrap_or_default()
                .contains("up to date")
        );
        assert!(!m.restart);
        m.update(AppEvent::Updated(Err("the installer failed".into())));
        assert!(
            m.message
                .as_deref()
                .unwrap_or_default()
                .starts_with("Update failed")
        );
        let (_id, _w) = with_session(&mut m, "busy");
        assert_eq!(
            m.update(AppEvent::Updated(Ok(Some("v0.2.0".into())))),
            Some(Cmd::OpenStop(StopKind::Quit)),
            "running sessions are asked about first, as on quit"
        );
        assert!(m.restart, "then mc starts again on the new version");
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
        assert_eq!(req.launch.name, None, "the agent names the session");
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
        assert!(
            m.notice
                .as_ref()
                .is_some_and(|(_, text)| text.ends_with("needs you: s")),
            "the mascot says it too"
        );
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
        assert!(m.rest().is_some() && !m.show_rest, "b is folded away");
        m.update(press(KeyCode::Char('j')));
        assert!(
            m.show_rest,
            "down on the last recent project opens the rest"
        );
        assert_eq!(m.selected, 1);
        let (second, _w2) = with_session(&mut m, "two");
        assert!(m.rest().is_none(), "every project is recent now");
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
