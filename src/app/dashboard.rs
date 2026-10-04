//! The dashboard mc opens on when started without a workspace or project
//! argument, and that `D` reopens: the mascot, the last active project, the saved workspaces, a
//! row to add one and a row that opens the project finder.
//!
//! Every row hands over to what already does the job: the switch of the
//! workspace switcher, the folder field of the settings, the `fp` finder.
//! `esc` closes it on the workspace `config.json` names.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::finder::Source;
use crate::app::form::{Field, FormKind};
use crate::app::model::{Cmd, Model, Overlay};
use crate::term::SessionId;

/// One row of the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DashRow {
    /// Pick up the most recently started session: its workspace, its
    /// project's name and the session.
    Last(PathBuf, String, SessionId),
    /// A saved workspace.
    Workspace(PathBuf),
    /// Add a folder through the folder field.
    Add,
    /// Open the project finder (`fp`).
    Find,
}

impl Model {
    /// Returns the dashboard's rows: the last active project when a stored
    /// session names one, the saved workspaces, then add and find.
    #[must_use]
    pub(crate) fn dashboard_rows(&self) -> Vec<DashRow> {
        let last = self.cards.iter().max_by_key(|c| c.started).and_then(|c| {
            let name = c.project.file_name()?.to_string_lossy().into_owned();
            Some(DashRow::Last(self.workspace_of(&c.project)?, name, c.id))
        });
        last.into_iter()
            .chain(self.workspaces.iter().cloned().map(DashRow::Workspace))
            .chain([DashRow::Add, DashRow::Find])
            .collect()
    }

    /// Applies a key to the dashboard: `↑`/`↓` (or `k`/`j`) move, `enter`
    /// picks, `1`–`9` switch to that workspace, `a` adds one, `f` or `/`
    /// finds a project, `esc` or `q` closes.
    pub(crate) fn dashboard_key(&mut self, mut selected: usize, key: KeyEvent) -> Option<Cmd> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            self.overlay = Some(Overlay::Dashboard(selected));
            return None;
        }
        let rows = self.dashboard_rows();
        let pick = match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return None,
            KeyCode::Enter => rows.get(selected).cloned(),
            KeyCode::Char(ch @ '1'..='9') => {
                let n = ch.to_digit(10).map_or(0, |d| d as usize);
                self.workspaces.get(n - 1).cloned().map(DashRow::Workspace)
            }
            KeyCode::Char('a') => Some(DashRow::Add),
            KeyCode::Char('f' | '/') => Some(DashRow::Find),
            KeyCode::Up | KeyCode::Char('k') => {
                selected = selected.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                selected = (selected + 1).min(rows.len().saturating_sub(1));
                None
            }
            _ => None,
        };
        match pick {
            Some(DashRow::Last(workspace, _, id)) => {
                Some(Cmd::SwitchWorkspace(workspace, Some(id)))
            }
            Some(DashRow::Workspace(path)) => Some(Cmd::SwitchWorkspace(path, None)),
            Some(DashRow::Add) => {
                self.open_form(FormKind::Settings, Field::Workspace);
                None
            }
            Some(DashRow::Find) => self.open_finder(Source::Projects),
            None => {
                self.overlay = Some(Overlay::Dashboard(selected));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::tests::{press, sample};

    #[test]
    fn rows_keys_and_hand_overs() {
        let mut m = sample(&["a", "b"]);
        let home = m.home.clone().unwrap();
        m.workspaces = vec![m.root().unwrap().to_path_buf(), home.join("personal")];
        m.overlay = Some(Overlay::Dashboard(0));
        let rows = m.dashboard_rows();
        assert_eq!(
            rows.len(),
            4,
            "no stored session: two workspaces, add, find"
        );
        assert_eq!(rows[2], DashRow::Add);

        assert_eq!(
            m.update(press(KeyCode::Char('2'))),
            Some(Cmd::SwitchWorkspace(home.join("personal"), None))
        );

        m.overlay = Some(Overlay::Dashboard(0));
        m.update(press(KeyCode::Down));
        assert_eq!(m.overlay, Some(Overlay::Dashboard(1)));
        for _ in 0..9 {
            m.update(press(KeyCode::Down));
        }
        assert_eq!(
            m.overlay,
            Some(Overlay::Dashboard(3)),
            "stops on the last row"
        );
        m.update(press(KeyCode::Enter));
        assert!(
            matches!(m.overlay, Some(Overlay::Finder(_))),
            "the last row opens the project finder"
        );

        m.overlay = None;
        m.update(press(KeyCode::Char('D')));
        assert_eq!(m.overlay, Some(Overlay::Dashboard(0)), "D reopens it");
        m.update(press(KeyCode::Esc));
        assert_eq!(m.overlay, None);
    }
}
