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

/// What a quick session is being moved into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveDialog {
    /// Picking one of the workspace's projects (index into `projects`).
    Pick {
        /// The quick session.
        id: SessionId,
        /// Highlighted project.
        selected: usize,
    },
    /// Typing the name of a new project folder.
    Create {
        /// The quick session.
        id: SessionId,
        /// The name typed so far.
        name: String,
        /// Why the name cannot be used, after `enter`.
        error: Option<String>,
    },
}

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
        let kind = self
            .settings
            .as_ref()
            .map_or(Kind::Claude, |s| s.default_agent);
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

    /// Handles a key while the popup shows: the exit chord or `ctrl-h`
    /// hides it (the session keeps running), `ctrl-z` is swallowed, every
    /// other key goes to the agent.
    pub(crate) fn popup_key(&mut self, id: SessionId, key: KeyEvent) {
        let ctrl = key.modifiers == KeyModifiers::CONTROL;
        if self.exit_chord.matches(&key) || (ctrl && key.code == KeyCode::Char('h')) {
            self.popup = None;
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
        self.overlay = Some(Overlay::Move(if create {
            MoveDialog::Create {
                id,
                name: String::new(),
                error: None,
            }
        } else {
            MoveDialog::Pick { id, selected: 0 }
        }));
    }

    /// Applies a key to the move dialog.
    pub(crate) fn move_key(&mut self, dialog: MoveDialog, key: KeyEvent) -> Option<Cmd> {
        match (dialog, key.code) {
            (_, KeyCode::Esc) => None,
            (MoveDialog::Pick { id, selected }, KeyCode::Enter) => {
                let dest = self.projects.get(selected)?.path.clone();
                self.move_quick(id, dest)
            }
            (MoveDialog::Pick { id, selected }, KeyCode::Up | KeyCode::Char('k')) => {
                let selected = selected.saturating_sub(1);
                self.overlay = Some(Overlay::Move(MoveDialog::Pick { id, selected }));
                None
            }
            (MoveDialog::Pick { id, selected }, KeyCode::Down | KeyCode::Char('j')) => {
                let selected = (selected + 1).min(self.projects.len().saturating_sub(1));
                self.overlay = Some(Overlay::Move(MoveDialog::Pick { id, selected }));
                None
            }
            (MoveDialog::Create { id, name, .. }, KeyCode::Enter) => {
                match self.new_project_path(&name) {
                    Ok(path) => Some(Cmd::CreateProject(id, path)),
                    Err(e) => {
                        self.overlay = Some(Overlay::Move(MoveDialog::Create {
                            id,
                            name,
                            error: Some(e),
                        }));
                        None
                    }
                }
            }
            (MoveDialog::Create { id, mut name, .. }, code) => {
                match code {
                    KeyCode::Backspace => {
                        name.pop();
                    }
                    KeyCode::Char(ch)
                        if !key.modifiers.contains(KeyModifiers::CONTROL)
                            && name.chars().count() < NAME_MAX =>
                    {
                        name.push(ch);
                    }
                    _ => {}
                }
                self.overlay = Some(Overlay::Move(MoveDialog::Create {
                    id,
                    name,
                    error: None,
                }));
                None
            }
            (dialog @ MoveDialog::Pick { .. }, _) => {
                self.overlay = Some(Overlay::Move(dialog));
                None
            }
        }
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
        if std::fs::symlink_metadata(&path).is_ok() {
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
        if card.resume_id().is_none() {
            self.message = Some("This session cannot be resumed (hooks were off).".into());
            return None;
        }
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
    pub(crate) fn moved_after_exit(&mut self, id: SessionId) -> Option<Cmd> {
        let card = self.cards.iter_mut().find(|c| c.id == id)?;
        let dest = card.move_to.take()?;
        let resume = card.resume_id()?;
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
                resume: Some(resume),
                pick: false,
                fork: card.kind == Kind::Claude,
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

        let (id, _w) = with_session(&mut m, "quick");
        m.cards.iter_mut().for_each(|c| c.project.clone_from(&root));
        m.popup = None;
        m.open_popup(id);
        assert_eq!(m.popup, Some(id), "a running quick session pops up");
        m.update(AppEvent::Input(ratatui::crossterm::event::Event::Key(
            KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL),
        )));
        assert_eq!(m.popup, None, "the exit chord hides it");

        assert_eq!(m.visible()[0].name, "quick", "a quick row leads the list");
        m.selected = 0;
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
