//! The workspace switcher (`w`): the saved workspaces, most recent first,
//! a field to filter them, digits to switch, and a row to add a folder.
//!
//! Switching never stops a session: cards of other workspaces keep running
//! (the header still counts them) and `!` follows a session that needs you
//! into its workspace. The list lives in `config.json` as `workspaces`;
//! removing an entry never touches the folder.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::form::{Field, FormKind};
use crate::app::model::{Cmd, Model, Overlay};
use crate::store::config::tilde;

/// Most workspaces remembered.
pub(crate) const WORKSPACES_MAX: usize = 20;

/// The switcher's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Switcher {
    /// What the user typed.
    pub query: String,
    /// Highlighted row of [`Model::switcher_rows`] (the add row is last).
    pub selected: usize,
    /// Projects per saved workspace, counted when the switcher opened.
    pub counts: Vec<(PathBuf, usize)>,
}

/// One row of the switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SwitcherRow {
    /// A saved workspace and how many projects it has.
    Workspace(PathBuf, usize),
    /// Add a folder through the folder browser.
    Add,
}

impl Model {
    /// Opens the switcher, counting each saved workspace's projects.
    pub(crate) fn open_switcher(&mut self) {
        let counts = self
            .workspaces
            .iter()
            .map(|w| (w.clone(), crate::workspace::scan(w).map_or(0, |p| p.len())))
            .collect();
        self.overlay = Some(Overlay::Switcher(Switcher {
            query: String::new(),
            selected: 0,
            counts,
        }));
    }

    /// Returns the switcher's rows for its query: the saved workspaces whose
    /// path contains it (most recent first), then the add row.
    #[must_use]
    pub(crate) fn switcher_rows(&self, switcher: &Switcher) -> Vec<SwitcherRow> {
        let needle = switcher.query.trim().to_lowercase();
        let mut rows: Vec<SwitcherRow> = switcher
            .counts
            .iter()
            .filter(|(path, _)| {
                tilde(path, self.home.as_deref())
                    .to_lowercase()
                    .contains(&needle)
            })
            .map(|(path, n)| SwitcherRow::Workspace(path.clone(), *n))
            .collect();
        rows.push(SwitcherRow::Add);
        rows
    }

    /// Applies a key to the switcher: typing filters, `↑`/`↓` move,
    /// `1`–`9` (with nothing typed) or `enter` switch, `ctrl-d` removes the
    /// highlighted workspace from the list, `esc` closes.
    pub(crate) fn switcher_key(&mut self, mut switcher: Switcher, key: KeyEvent) -> Option<Cmd> {
        let rows = self.switcher_rows(&switcher);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let pick = match key.code {
            KeyCode::Esc => return None,
            KeyCode::Enter => rows.get(switcher.selected).cloned(),
            KeyCode::Char(ch @ '1'..='9') if switcher.query.is_empty() && !ctrl => {
                let n = ch.to_digit(10).map_or(0, |d| d as usize);
                rows.get(n - 1)
                    .filter(|r| matches!(r, SwitcherRow::Workspace(..)))
                    .cloned()
            }
            KeyCode::Char('d') if ctrl => {
                if let Some(SwitcherRow::Workspace(path, _)) = rows.get(switcher.selected) {
                    if self.root() == Some(path.as_path()) {
                        self.message = Some("That is the current workspace.".into());
                    } else {
                        let path = path.clone();
                        self.workspaces.retain(|w| *w != path);
                        switcher.counts.retain(|(w, _)| *w != path);
                        switcher.selected = switcher.selected.saturating_sub(1);
                        self.overlay = Some(Overlay::Switcher(switcher));
                        return Some(Cmd::SaveWorkspaces);
                    }
                }
                None
            }
            KeyCode::Up => {
                switcher.selected = switcher.selected.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                switcher.selected = (switcher.selected + 1).min(rows.len().saturating_sub(1));
                None
            }
            KeyCode::Backspace => {
                switcher.query.pop();
                switcher.selected = 0;
                None
            }
            KeyCode::Char(ch) if !ctrl => {
                switcher.query.push(ch);
                switcher.selected = 0;
                None
            }
            _ => None,
        };
        match pick {
            Some(SwitcherRow::Workspace(path, _)) => Some(Cmd::SwitchWorkspace(path, None)),
            Some(SwitcherRow::Add) => {
                self.open_form(FormKind::Settings, Field::Workspace);
                None
            }
            None => {
                self.overlay = Some(Overlay::Switcher(switcher));
                None
            }
        }
    }

    /// Puts `workspace` first in the saved list (adding it), keeping at
    /// most [`WORKSPACES_MAX`].
    pub(crate) fn remember_workspace(&mut self, workspace: &Path) {
        self.workspaces.retain(|w| w != workspace);
        self.workspaces.insert(0, workspace.to_path_buf());
        self.workspaces.truncate(WORKSPACES_MAX);
    }

    /// Returns the saved workspace a folder belongs to: the longest saved
    /// path it is in, else its parent folder.
    #[must_use]
    pub(crate) fn workspace_of(&self, folder: &Path) -> Option<PathBuf> {
        self.workspaces
            .iter()
            .filter(|w| folder.starts_with(w) && folder != w.as_path())
            .max_by_key(|w| w.as_os_str().len())
            .cloned()
            .or_else(|| folder.parent().map(Path::to_path_buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppEvent;
    use crate::app::model::tests::{press, sample};

    #[test]
    fn w_lists_saved_workspaces_filters_and_switches() {
        let mut m = sample(&["a"]);
        let home = m.home.clone().unwrap();
        m.workspaces = vec![
            m.root().unwrap().to_path_buf(),
            home.join("personal"),
            home.join("code/oss"),
        ];
        m.update(press(KeyCode::Char('w')));
        let Some(Overlay::Switcher(s)) = &m.overlay else {
            panic!("w opens the switcher");
        };
        assert_eq!(m.switcher_rows(s).len(), 4, "three saved and the add row");
        assert_eq!(
            m.update(press(KeyCode::Char('2'))),
            Some(Cmd::SwitchWorkspace(home.join("personal"), None))
        );
        m.update(press(KeyCode::Char('w')));
        for ch in "oss".chars() {
            m.update(press(KeyCode::Char(ch)));
        }
        assert_eq!(
            m.update(press(KeyCode::Enter)),
            Some(Cmd::SwitchWorkspace(home.join("code/oss"), None))
        );
        m.update(press(KeyCode::Char('w')));
        m.update(press(KeyCode::Down));
        let ctrl_d = AppEvent::Input(ratatui::crossterm::event::Event::Key(KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
        )));
        assert_eq!(m.update(ctrl_d), Some(Cmd::SaveWorkspaces));
        assert_eq!(m.workspaces.len(), 2, "removed from the list only");
    }

    #[test]
    fn bang_follows_a_session_that_needs_you_into_its_workspace() {
        use crate::app::model::tests::with_session;
        use crate::app::sessions::State;

        let mut m = sample(&["a"]);
        let (id, _w) = with_session(&mut m, "elsewhere");
        let other = PathBuf::from("/Users/me/code/oss");
        m.cards[0].project = other.join("tool");
        m.cards[0].state = State::NeedsYou;
        m.workspaces.push(other.clone());
        m.focus = crate::app::model::Focus::Projects;
        assert_eq!(
            m.update(press(KeyCode::Char('!'))),
            Some(Cmd::SwitchWorkspace(other, Some(id)))
        );
    }

    #[test]
    fn remembers_recent_first_and_finds_a_folders_workspace() {
        let mut m = sample(&["a"]);
        let a = PathBuf::from("/w/a");
        let b = PathBuf::from("/w/b");
        m.workspaces.clear();
        m.remember_workspace(&a);
        m.remember_workspace(&b);
        m.remember_workspace(&a);
        assert_eq!(m.workspaces, [a.clone(), b]);
        assert_eq!(m.workspace_of(Path::new("/w/a/app")), Some(a));
        assert_eq!(
            m.workspace_of(Path::new("/x/y/app")),
            Some(PathBuf::from("/x/y"))
        );
    }
}
