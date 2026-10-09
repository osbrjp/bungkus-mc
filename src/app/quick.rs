//! Quick sessions (DESIGN §5.10, issue #46): an agent started at the
//! workspace root that runs in a fixed popup over the panes, and later
//! moves into a project, or into a new one, with its conversation.
//!
//! A card is quick when its folder is the workspace root, so nothing extra
//! is stored. Moving stops the quick process, then resumes the session in
//! the project: Claude with `--resume <id> --fork-session` (a new session
//! saved under the project; the original stays at the root), Codex with
//! `resume <id>`. mc passes only those flags and reads no transcript.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rustix::process::Signal;

use crate::agent::{Kind, Launch};
use crate::app::model::{Cmd, LaunchRequest, Model, Overlay};
use crate::app::sessions::Card;
use crate::term::SessionId;
use crate::term::keys;

/// Longest new project name accepted.
const NAME_MAX: usize = 64;

/// The move dialog: one field that both finds a project to move into and
/// names a new one (issue #46).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MoveDialog {
    /// The quick session.
    pub id: SessionId,
    /// What the user typed.
    pub query: String,
    /// Highlighted row of [`Model::move_options`].
    pub selected: usize,
    /// Why the last `enter` could not be carried out.
    pub error: Option<String>,
}

/// The `a` dialog: a new project folder in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewProject {
    /// The folder name typed so far.
    pub name: String,
    /// Whether to add `AGENTS.md` and a `CLAUDE.md` that imports it, besides
    /// `git init`; otherwise the project is a fresh repository.
    pub agent_files: bool,
    /// Why the last `enter` could not be carried out.
    pub error: Option<String>,
}

/// One row of the move dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveOption {
    /// Move into this existing project.
    Project {
        /// Its display name.
        name: String,
        /// Its folder.
        path: PathBuf,
    },
    /// Create a project with this name and move into it.
    Create(String),
}

/// Projects the move dialog lists: the most recently used while the field
/// is empty, the best matches while typing. The dialog keeps this many
/// rows plus the create row, so it never changes height.
pub(crate) const SLOTS: usize = 3;

impl Model {
    /// Returns the workspace root, once settings are applied.
    #[must_use]
    pub(crate) fn root(&self) -> Option<&Path> {
        self.settings.as_ref().map(|s| s.workspace.as_path())
    }

    /// Returns whether `card` is a quick session (it runs at the root).
    #[must_use]
    pub(crate) fn is_quick(&self, card: &Card) -> bool {
        self.root() == Some(card.project.as_path())
    }

    /// Starts a quick session with the default agent at the workspace root.
    pub(crate) fn quick_session(&mut self) -> Option<Cmd> {
        let project = self.root()?.to_path_buf();
        let kind = self.default_agent();
        Some(Cmd::Launch(LaunchRequest {
            project,
            kind,
            launch: Launch {
                id: SessionId::new(),
                model: None,
                name: Some("quick".into()),
                prompt: None,
                settings: None,
                hook_args: Vec::new(),
                resume: None,
                pick: false,
                fork: false,
                cloud: None,
            },
            replaces: None,
        }))
    }

    /// Shows the popup of quick session `id` when it runs.
    pub(crate) fn open_popup(&mut self, id: SessionId) {
        if self.cards.iter().any(|c| c.id == id && c.running()) {
            self.popup = Some(id);
        }
    }

    /// Handles a key while the popup shows. The exit chord opens the popup
    /// menu, whose keys are `h` (or the chord again) hide, `m` move to a
    /// project, `p` new project, anything else back to the agent; `ctrl-h`
    /// hides at once; `ctrl-m` opens the move dialog; `ctrl-z` is
    /// swallowed; every other key goes to the agent, so typing never
    /// triggers mc.
    pub(crate) fn popup_key(&mut self, id: SessionId, key: KeyEvent) {
        let ctrl = key.modifiers == KeyModifiers::CONTROL;
        if self.popup_menu {
            self.popup_menu = false;
            match key.code {
                KeyCode::Char('h') => self.hide_popup(id),
                KeyCode::Char('m') => self.start_move_of(id, false),
                KeyCode::Char('p') => self.start_move_of(id, true),
                _ if self.exit_chord.matches(&key) => self.hide_popup(id),
                _ => {}
            }
            return;
        }
        if self.exit_chord.matches(&key) {
            self.popup_menu = true;
            return;
        }
        if ctrl && key.code == KeyCode::Char('h') {
            self.hide_popup(id);
            return;
        }
        // ctrl-m is only told apart from enter under the kitty keyboard
        // protocol; elsewhere it arrives as enter and goes to the agent.
        if ctrl && key.code == KeyCode::Char('m') {
            self.start_move_of(id, false);
            return;
        }
        if ctrl && key.code == KeyCode::Char('z') {
            return;
        }
        if let Some(pty) = self
            .cards
            .iter_mut()
            .find(|c| c.id == id)
            .and_then(|c| c.pty.as_mut())
        {
            pty.scroll(alacritty_terminal::grid::Scroll::Bottom);
            pty.send(keys::encode(&key, pty.mode()));
        }
    }

    /// Hides the popup of quick session `id` (it keeps running) and selects
    /// it under the `quick` row, so `enter` brings it back.
    fn hide_popup(&mut self, id: SessionId) {
        self.popup = None;
        self.popup_menu = false;
        self.focus = crate::app::model::Focus::Sessions;
        let root = self.root().map(Path::to_path_buf);
        if let Some(row) = self
            .visible()
            .iter()
            .position(|p| Some(&p.path) == root.as_ref())
        {
            self.selected = row;
        }
        if let Some(pos) = self
            .project_cards()
            .iter()
            .position(|&i| self.cards[i].id == id)
        {
            self.card = pos;
        }
    }

    /// Opens the move (`m`) or new-project (`p`) dialog for the selected
    /// quick session.
    pub(crate) fn start_move(&mut self, create: bool) {
        let Some(card) = self.selected_card().map(|i| &self.cards[i]) else {
            return;
        };
        if !self.is_quick(card) {
            self.message = Some("Only quick sessions move (N starts one).".into());
            return;
        }
        let id = card.id;
        self.start_move_of(id, create);
    }

    /// Opens the move dialog for quick session `id`.
    fn start_move_of(&mut self, id: SessionId, _create: bool) {
        self.overlay = Some(Overlay::Move(MoveDialog {
            id,
            query: String::new(),
            selected: 0,
            error: None,
        }));
    }

    /// Returns the move dialog's rows for `query`: with nothing typed, the
    /// [`SLOTS`] most recently used projects; otherwise the [`SLOTS`] best
    /// matches (exact, then prefix, then contains; shorter names first),
    /// then "create" unless one matches it exactly.
    #[must_use]
    pub(crate) fn move_options(&self, query: &str) -> Vec<MoveOption> {
        let query = query.trim();
        let project = |p: &crate::workspace::Project| MoveOption::Project {
            name: p.name.clone(),
            path: p.path.clone(),
        };
        if query.is_empty() {
            let mut recent: Vec<(&crate::workspace::Project, Option<std::time::Instant>)> = self
                .projects
                .iter()
                .map(|p| {
                    let last = self
                        .cards
                        .iter()
                        .filter(|c| c.project == p.path)
                        .map(|c| c.started)
                        .max();
                    (p, last)
                })
                .collect();
            recent.sort_by_key(|(_, last)| std::cmp::Reverse(*last));
            return recent
                .into_iter()
                .take(SLOTS)
                .map(|(p, _)| project(p))
                .collect();
        }
        let needle = query.to_lowercase();
        let mut ranked: Vec<(u8, &crate::workspace::Project)> = self
            .projects
            .iter()
            .filter_map(|p| {
                let name = p.name.to_lowercase();
                let rank = if name == needle {
                    0
                } else if name.starts_with(&needle) {
                    1
                } else if name.contains(&needle) {
                    2
                } else {
                    return None;
                };
                Some((rank, p))
            })
            .collect();
        ranked.sort_by(|a, b| {
            (a.0, a.1.name.len(), &a.1.name).cmp(&(b.0, b.1.name.len(), &b.1.name))
        });
        let mut rows: Vec<MoveOption> = ranked
            .into_iter()
            .take(SLOTS)
            .map(|(_, p)| project(p))
            .collect();
        if !self.projects.iter().any(|p| p.name == query) {
            rows.push(MoveOption::Create(query.to_owned()));
        }
        rows
    }

    /// Applies a key to the move dialog: typing edits the field; `↑`/`↓`,
    /// `ctrl-k`/`ctrl-j`, `ctrl-p`/`ctrl-n` or `shift-tab`/`tab` move the
    /// highlight (whichever the terminal passes on); `enter` moves into the
    /// highlighted project or creates the new one; `esc` cancels.
    pub(crate) fn move_key(&mut self, mut dialog: MoveDialog, key: KeyEvent) -> Option<Cmd> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let rows = self.move_options(&dialog.query).len();
        match key.code {
            KeyCode::Esc => return None,
            KeyCode::Enter => {
                match self
                    .move_options(&dialog.query)
                    .get(dialog.selected)
                    .cloned()
                {
                    Some(MoveOption::Project { path, .. }) => {
                        return self.move_quick(dialog.id, path);
                    }
                    Some(MoveOption::Create(name)) => match self.new_project_path(&name) {
                        Ok(path) => return Some(Cmd::CreateProject(dialog.id, path)),
                        Err(e) => dialog.error = Some(e),
                    },
                    None => dialog.error = Some("Type a project name.".into()),
                }
            }
            KeyCode::Up | KeyCode::BackTab => dialog.selected = dialog.selected.saturating_sub(1),
            KeyCode::Char('k' | 'p') if ctrl => {
                dialog.selected = dialog.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Tab => {
                dialog.selected = (dialog.selected + 1).min(rows.saturating_sub(1));
            }
            KeyCode::Char('j' | 'n') if ctrl => {
                dialog.selected = (dialog.selected + 1).min(rows.saturating_sub(1));
            }
            KeyCode::Backspace => {
                dialog.query.pop();
                dialog.selected = 0;
                dialog.error = None;
            }
            KeyCode::Char(ch) if !ctrl && dialog.query.chars().count() < NAME_MAX => {
                dialog.query.push(ch);
                dialog.selected = 0;
                dialog.error = None;
            }
            _ => {}
        }
        self.overlay = Some(Overlay::Move(dialog));
        None
    }

    /// Opens the `a` new-project dialog.
    pub(crate) fn start_new_project(&mut self) {
        if self.root().is_some() {
            self.overlay = Some(Overlay::NewProject(NewProject {
                name: String::new(),
                agent_files: true,
                error: None,
            }));
        }
    }

    /// Applies a key to the new-project dialog: typing edits the name,
    /// `tab`/`↑`/`↓`/`←`/`→` switch fresh / with agent files, `enter`
    /// creates it, `esc` cancels.
    pub(crate) fn new_project_key(&mut self, mut dialog: NewProject, key: KeyEvent) -> Option<Cmd> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return None,
            KeyCode::Enter => match self.new_project_path(&dialog.name) {
                Ok(path) => return Some(Cmd::NewProject(path, dialog.agent_files)),
                Err(e) => dialog.error = Some(e),
            },
            KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right => dialog.agent_files = !dialog.agent_files,
            KeyCode::Backspace => {
                dialog.name.pop();
                dialog.error = None;
            }
            KeyCode::Char(ch) if !ctrl && dialog.name.chars().count() < NAME_MAX => {
                dialog.name.push(ch);
                dialog.error = None;
            }
            _ => {}
        }
        self.overlay = Some(Overlay::NewProject(dialog));
        None
    }

    /// Returns the folder for a new project called `name` in the
    /// workspace, or why it cannot be one: empty, a path, hidden, or taken.
    ///
    /// # Errors
    ///
    /// The reason, shown under the field.
    pub(crate) fn new_project_path(&self, name: &str) -> Result<PathBuf, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Type a folder name.".into());
        }
        if name.contains('/') || name.starts_with('.') || name.chars().any(char::is_control) {
            return Err("Use a plain folder name (no / and not starting with .).".into());
        }
        let root = self.root().ok_or("No workspace yet.")?;
        let path = root.join(name);
        if std::fs::symlink_metadata(&path).is_ok() || self.projects.iter().any(|p| p.name == name)
        {
            return Err(format!("{name} already exists in the workspace."));
        }
        Ok(path)
    }

    /// Moves quick session `id` into `dest`: when it runs, it is stopped
    /// first and resumed once it has exited ([`Model::moved_after_exit`]);
    /// otherwise it is resumed at once.
    pub(crate) fn move_quick(&mut self, id: SessionId, dest: PathBuf) -> Option<Cmd> {
        let now = self.now;
        let card = self.cards.iter_mut().find(|c| c.id == id)?;
        if card.resume_id().is_none() && card.prompted {
            self.message = Some("This session cannot be resumed (hooks were off).".into());
            return None;
        }
        crate::debug_log!("move {} to {}", id.short(), dest.display());
        card.move_to = Some(dest);
        if self.popup == Some(id) {
            self.popup = None;
        }
        if card.running() {
            card.stop_requested = Some(now);
            if let Some(pty) = &card.pty {
                // reason: ESRCH means it already exited; the waiter reports it.
                let _ = pty.signal(Signal::TERM);
            }
            self.message = Some("Moving: closing the quick session…".into());
            return None;
        }
        self.moved_after_exit(id)
    }

    /// Returns the resume into the card's `move_to` folder once a moving
    /// quick session has exited (or right away for a finished one).
    ///
    /// A hooked session that never got a prompt has no saved conversation
    /// (`claude --resume` would answer "No conversation found"), so it
    /// starts fresh in the project instead.
    pub(crate) fn moved_after_exit(&mut self, id: SessionId) -> Option<Cmd> {
        let card = self.cards.iter_mut().find(|c| c.id == id)?;
        let dest = card.move_to.take()?;
        let resume = if card.hooked && !card.prompted {
            None
        } else {
            Some(card.resume_id()?)
        };
        crate::debug_log!(
            "{} moves to {} ({})",
            card.id.short(),
            dest.display(),
            if resume.is_some() {
                "resume"
            } else {
                "fresh: no prompt yet"
            }
        );
        Some(Cmd::Launch(LaunchRequest {
            project: dest,
            kind: card.kind,
            launch: Launch {
                id: SessionId::new(),
                model: None,
                name: Some(card.name.clone()),
                prompt: None,
                settings: None,
                hook_args: Vec::new(),
                fork: resume.is_some() && card.kind == Kind::Claude,
                cloud: None,
                resume,
                pick: false,
            },
            replaces: Some(id),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppEvent;
    use crate::app::model::tests::{press, sample, with_session};
    use crate::term::PtyEvent;

    #[test]
    fn quick_sessions_start_at_the_root_and_move_into_a_project() {
        let mut m = sample(&["app", "web"]);
        let root = m.root().unwrap().to_path_buf();
        let Some(Cmd::Launch(req)) = m.update(press(KeyCode::Char('N'))) else {
            panic!("N starts a quick session");
        };
        assert_eq!(req.project, root);

        let (id, writes) = with_session(&mut m, "quick");
        m.cards.iter_mut().for_each(|c| c.project.clone_from(&root));
        m.popup = None;
        m.open_popup(id);
        assert_eq!(m.popup, Some(id), "a running quick session pops up");
        let menu = || {
            AppEvent::Input(ratatui::crossterm::event::Event::Key(KeyEvent::new(
                KeyCode::Char('\\'),
                KeyModifiers::CONTROL,
            )))
        };
        m.update(press(KeyCode::Char('m')));
        assert!(
            writes.try_recv().is_ok(),
            "a plain m is typed into the agent"
        );
        assert!(m.overlay.is_none());
        m.update(AppEvent::Input(ratatui::crossterm::event::Event::Key(
            KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL),
        )));
        assert!(matches!(m.overlay, Some(Overlay::Move(_))), "ctrl-m moves");
        m.update(press(KeyCode::Esc));
        m.update(menu());
        assert!(m.popup_menu, "the exit chord opens the popup menu");
        m.update(press(KeyCode::Char('m')));
        assert!(matches!(m.overlay, Some(Overlay::Move(_))));
        m.update(press(KeyCode::Esc));
        m.update(menu());
        m.update(press(KeyCode::Char('h')));
        assert_eq!(m.popup, None, "menu h hides it");

        assert_eq!(m.visible()[0].name, "quick", "a quick row leads the list");
        m.selected = 2;
        m.update(press(KeyCode::Char('0')));
        assert_eq!(m.selected, 0, "0 jumps to it");
        m.now += crate::app::model::JUMP_WINDOW;
        m.update(press(KeyCode::Char('2')));
        assert_eq!(
            m.visible()[m.selected].name,
            "web",
            "1.. number the projects below it"
        );
        m.now += crate::app::model::JUMP_WINDOW;
        m.update(press(KeyCode::Char('0')));
        m.focus = crate::app::model::Focus::Sessions;
        m.card = 0;
        m.update(press(KeyCode::Char('m')));
        m.update(press(KeyCode::Down));
        assert!(
            m.update(press(KeyCode::Enter)).is_none(),
            "a running one stops first"
        );
        assert!(m.cards[0].stop_requested.is_some());
        let Some(Cmd::Launch(req)) = m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(143))))
        else {
            panic!("resumes in the project once it has exited");
        };
        assert_eq!(req.project, m.projects[1].path);
        assert_eq!(req.replaces, Some(id));
        assert!(req.launch.fork, "claude forks into the project");
        assert_eq!(req.launch.resume, Some(id.0.hyphenated().to_string()));
    }

    #[test]
    fn the_move_field_finds_projects_or_offers_to_create_one() {
        let m = sample(&["kedai-web", "pasar-mobile", "roti-docs", "teh-cli"]);
        let names = |q: &str| -> Vec<String> {
            m.move_options(q)
                .into_iter()
                .map(|o| match o {
                    MoveOption::Project { name, .. } => name,
                    MoveOption::Create(name) => format!("+{name}"),
                })
                .collect()
        };
        assert_eq!(names("").len(), 3, "recent projects while empty");
        assert_eq!(
            names("e"),
            ["teh-cli", "kedai-web", "pasar-mobile", "+e"],
            "the best three: contains, shorter names first"
        );
        assert_eq!(names("pa"), ["pasar-mobile", "+pa"], "a prefix ranks first");
        assert_eq!(
            names("teh-cli"),
            ["teh-cli"],
            "an exact name moves, no create"
        );
        assert_eq!(names("fresh"), ["+fresh"]);
    }

    #[test]
    fn typing_and_ctrl_j_pick_then_enter_creates() {
        let mut m = sample(&["app", "web"]);
        let root = m.root().unwrap().to_path_buf();
        let (id, _w) = with_session(&mut m, "quick");
        m.cards[0].project.clone_from(&root);
        m.popup = None;
        m.overlay = Some(Overlay::Move(MoveDialog {
            id,
            query: String::new(),
            selected: 0,
            error: None,
        }));
        for ch in "ap".chars() {
            m.update(press(KeyCode::Char(ch)));
        }
        m.update(AppEvent::Input(ratatui::crossterm::event::Event::Key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        )));
        let Some(Cmd::CreateProject(got, path)) = m.update(press(KeyCode::Enter)) else {
            panic!("the create row comes after the match");
        };
        assert_eq!((got, path), (id, root.join("ap")));
    }

    #[test]
    fn enter_on_an_ended_quick_session_resumes_it() {
        let mut m = sample(&["app"]);
        let root = m.root().unwrap().to_path_buf();
        let (id, _w) = with_session(&mut m, "quick");
        m.cards[0].project.clone_from(&root);
        m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(0))));
        m.popup = None;
        m.selected = 0;
        m.focus = crate::app::model::Focus::Sessions;
        m.card = 0;
        let Some(Cmd::Launch(req)) = m.update(press(KeyCode::Enter)) else {
            panic!("enter resumes an ended quick session");
        };
        assert_eq!((req.project, req.replaces), (root, Some(id)));
    }

    #[test]
    fn a_quick_session_with_no_prompt_yet_starts_fresh_in_the_project() {
        let mut m = sample(&["app"]);
        let root = m.root().unwrap().to_path_buf();
        let (id, _w) = with_session(&mut m, "quick");
        m.cards[0].project.clone_from(&root);
        m.cards[0].expect_hooks();
        m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(0))));
        let dest = m.projects[0].path.clone();
        let Some(Cmd::Launch(req)) = m.move_quick(id, dest) else {
            panic!("an ended one moves at once");
        };
        assert_eq!(req.launch.resume, None, "nothing saved to resume");
        assert!(!req.launch.fork);
    }

    #[test]
    fn tab_and_ctrl_n_move_the_highlight_too() {
        let mut m = sample(&["app", "apt", "apx"]);
        let (id, _w) = with_session(&mut m, "quick");
        m.popup = None;
        m.overlay = Some(Overlay::Move(MoveDialog {
            id,
            query: "ap".into(),
            selected: 0,
            error: None,
        }));
        let ctrl = |ch| {
            AppEvent::Input(ratatui::crossterm::event::Event::Key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::CONTROL,
            )))
        };
        m.update(press(KeyCode::Tab));
        m.update(ctrl('n'));
        m.update(ctrl('j'));
        let Some(Overlay::Move(d)) = &m.overlay else {
            panic!("still open");
        };
        assert_eq!(d.selected, 3, "the create row after three matches");
        m.update(ctrl('p'));
        m.update(press(KeyCode::BackTab));
        let Some(Overlay::Move(d)) = &m.overlay else {
            panic!("still open");
        };
        assert_eq!(d.selected, 1);
    }

    #[test]
    fn a_names_a_new_project_and_picks_its_kind() {
        let mut m = sample(&["app"]);
        m.update(press(KeyCode::Char('a')));
        for ch in "kedai".chars() {
            m.update(press(KeyCode::Char(ch)));
        }
        m.update(press(KeyCode::Tab));
        let Some(Cmd::NewProject(path, agent_files)) = m.update(press(KeyCode::Enter)) else {
            panic!("enter creates");
        };
        assert_eq!(path, m.root().unwrap().join("kedai"));
        assert!(!agent_files, "tab switched to a fresh repository");
        m.update(press(KeyCode::Char('a')));
        m.update(press(KeyCode::Char('a')));
        m.update(press(KeyCode::Char('p')));
        m.update(press(KeyCode::Char('p')));
        assert!(m.update(press(KeyCode::Enter)).is_none(), "app exists");
        let Some(Overlay::NewProject(d)) = &m.overlay else {
            panic!("stays open with the reason");
        };
        assert!(
            d.error
                .as_deref()
                .unwrap_or_default()
                .contains("already exists")
        );
    }

    #[test]
    fn new_project_names_are_checked() {
        let m = sample(&["app"]);
        for bad in ["", "a/b", ".hidden"] {
            assert!(m.new_project_path(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            m.new_project_path(" fresh ").unwrap(),
            m.root().unwrap().join("fresh")
        );
    }
}
