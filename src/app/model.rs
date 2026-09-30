//! The model the UI thread owns, and `update`: one event in, the model
//! changed, maybe a command out (ARCHITECTURE §8).
//!
//! `update` does no blocking I/O. Sending bytes to a session's writer
//! thread and signalling its process group are non-blocking and happen
//! here; spawning, saving and scanning are returned as a [`Cmd`].

use std::path::PathBuf;
use std::time::Instant;

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use rustix::process::Signal;

use crate::agent::{Kind, Launch};
use crate::app::AppEvent;
use crate::app::form::{Field, Form, FormKind, Outcome};
use crate::app::picker::{self, Picker};
use crate::app::sessions::{self, Card, STOP_GRACE};
use crate::store::config::Settings;
use crate::term::keys::Chord;
use crate::term::{PtyEvent, SessionId};
use crate::ui::keymap::{self, Action, Lookup, Scope};
use crate::ui::theme::{Theme, ThemeChoice};
use crate::workspace::Project;

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

/// What a confirm dialog asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Confirm {
    /// Quit mc, stopping every running session.
    Quit,
    /// Stop one session.
    Stop(SessionId),
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
    /// A yes/no question.
    Confirm(Confirm),
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
}

/// Everything the screen shows.
#[derive(Debug)]
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
    /// The whole terminal, for layout and mouse hit tests.
    pub screen: Rect,
    /// The time the view renders at.
    pub now: Instant,
    /// Animation frame counter, advanced by the 350 ms tick.
    pub frame: usize,
    /// When quitting started, while sessions are being stopped.
    pub quitting: Option<Instant>,
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
            overlay: None,
            message: None,
            found,
            fallback_workspace: fallback,
            list_rows: 10,
            exit_chord: Chord::DEFAULT,
            screen: Rect::new(0, 0, 120, 40),
            now: Instant::now(),
            frame: 0,
            quitting: None,
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

    /// Returns the projects matching the filter, in display order.
    #[must_use]
    pub(crate) fn visible(&self) -> Vec<&Project> {
        let needle = self.filter.to_lowercase();
        self.projects
            .iter()
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

    /// Returns whether anything on screen animates (a working card of the
    /// selected project), so the 350 ms tick must run.
    #[must_use]
    pub(crate) fn animating(&self) -> bool {
        !self.theme.no_color()
            && self
                .project_cards()
                .iter()
                .any(|&i| self.cards[i].running())
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
        let tick = self.animating().then_some(tick);
        syncs.chain(stops).chain(tick).min()
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
            prefill.clone_into(&mut form.workspace);
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
                self.enforce_stops();
            }
            AppEvent::Pty(PtyEvent::Output(id, bytes)) => {
                if let Some(pty) = self.card_mut(id).and_then(|c| c.pty.as_mut()) {
                    pty.advance(&bytes);
                }
            }
            AppEvent::Pty(PtyEvent::Exited(id, code)) => {
                let now = self.now;
                if let Some(card) = self.card_mut(id) {
                    card.exited(code, now);
                }
                if self.focus == Focus::Output
                    && self.selected_card().map(|i| self.cards[i].id) == Some(id)
                {
                    self.focus = Focus::Sessions;
                }
            }
            AppEvent::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                return self.key(key);
            }
            AppEvent::Input(Event::Paste(text)) => self.paste(&text),
            AppEvent::Input(Event::Mouse(mouse)) => self.mouse(mouse),
            AppEvent::Input(Event::Resize(w, h)) => self.screen = Rect::new(0, 0, w, h),
            AppEvent::Input(_) => {}
        }
        self.quit_when_stopped()
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
    fn quit_when_stopped(&self) -> Option<Cmd> {
        let started = self.quitting?;
        let done = !self.cards.iter().any(Card::running)
            || self.now >= started + STOP_GRACE + std::time::Duration::from_secs(1);
        done.then_some(Cmd::Quit)
    }

    /// Handles a key press.
    fn key(&mut self, key: KeyEvent) -> Option<Cmd> {
        if self.quitting.is_some() {
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
        match keymap::lookup(self.focus.scope(), key, self.pending.take()) {
            Lookup::Pending(ch) => {
                self.pending = Some(ch);
                None
            }
            Lookup::Unbound => None,
            Lookup::Action(action) => self.act(action),
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
                Outcome::Quit => self.request_quit(),
            },
            Overlay::Picker(mut p) => match p.key(key) {
                picker::Outcome::Continue => {
                    self.overlay = Some(Overlay::Picker(p));
                    None
                }
                picker::Outcome::Cancel => None,
                picker::Outcome::Start => self.launch(&p),
            },
            Overlay::Confirm(confirm) => {
                if key.code == KeyCode::Char('y') {
                    self.confirmed(confirm);
                }
                self.quit_when_stopped()
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
        };
        Some(Cmd::Launch(LaunchRequest {
            project,
            kind: p.agent,
            launch,
        }))
    }

    /// Adds a started (or failed-to-start) session and enters INTERACT on
    /// it when it runs.
    pub(crate) fn add_card(&mut self, card: Card) {
        let running = card.running();
        let id = card.id;
        self.cards.push(card);
        self.card = self
            .project_cards()
            .iter()
            .position(|&i| self.cards[i].id == id)
            .unwrap_or(0);
        self.focus = if running {
            Focus::Output
        } else {
            Focus::Sessions
        };
    }

    /// Carries out a confirmed stop or quit: SIGTERM to each session's
    /// process group; SIGKILL follows after the grace period.
    fn confirmed(&mut self, confirm: Confirm) {
        let now = self.now;
        let targets: Vec<SessionId> = match confirm {
            Confirm::Quit => {
                self.quitting = Some(now);
                self.message = Some("stopping…".into());
                self.cards
                    .iter()
                    .filter(|c| c.running())
                    .map(|c| c.id)
                    .collect()
            }
            Confirm::Stop(id) => vec![id],
        };
        for id in targets {
            if let Some(card) = self.card_mut(id).filter(|c| c.running()) {
                card.stop_requested = Some(now);
                if let Some(pty) = &card.pty {
                    // reason: ESRCH means it already exited; the waiter reports it.
                    let _ = pty.signal(Signal::TERM);
                }
            }
        }
    }

    /// Quits at once, or asks first when sessions are running.
    fn request_quit(&mut self) -> Option<Cmd> {
        if self.cards.iter().any(Card::running) {
            self.overlay = Some(Overlay::Confirm(Confirm::Quit));
            None
        } else {
            Some(Cmd::Quit)
        }
    }

    /// Applies a FILTER-mode key: typing edits the filter, `enter` keeps
    /// it, `esc` clears it.
    fn filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.filtering = false,
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
                    self.card = Self::step(self.card, delta, self.project_cards().len());
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
            Action::NextPane if self.focus == Focus::Sessions => self.interact(),
            Action::Interact => self.interact(),
            Action::NextPane | Action::OpenProject => self.focus = Focus::Sessions,
            Action::Filter if self.focus == Focus::Projects => {
                self.filtering = true;
                self.filter.clear();
                self.selected = 0;
            }
            Action::NewSession if self.selected_project().is_some() => {
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
            Action::Stop => {
                if let Some(i) = self.selected_card().filter(|&i| self.cards[i].running()) {
                    self.overlay = Some(Overlay::Confirm(Confirm::Stop(self.cards[i].id)));
                }
            }
            Action::Zoom => self.zoom = !self.zoom,
            Action::Workspace => self.open_form(FormKind::Settings, Field::Workspace),
            Action::Settings => self.open_form(FormKind::Settings, Field::Agent),
            Action::Help => self.overlay = Some(Overlay::Help),
            Action::Redraw => return Some(Cmd::Redraw),
            Action::Quit => return self.request_quit(),
            Action::Filter
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
        m.update(press(KeyCode::Char('\\')));
        m.focus = Focus::Sessions;
        assert_eq!(m.update(press(KeyCode::Char('q'))), None);
        assert_eq!(m.overlay, Some(Overlay::Confirm(Confirm::Quit)));
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
