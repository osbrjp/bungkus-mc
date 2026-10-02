//! Choosing several projects at once and grouping related ones.
//!
//! `v` marks single rows and `V` selects a range; together they are the
//! chosen rows that `d` (Trash) and `g` (group) act on. A group is a list
//! of project folder names kept in the workspace's own
//! `.bungkus-mc/config.json`: its members are shown with the link bar, and
//! a session started in one of them is given the others (`--add-dir` and
//! a line in its instructions; see [`crate::agent::instruction_args`]).
//!
//! A group can only name projects directly inside the open workspace: a
//! name that is no project here is ignored, so a config file from
//! elsewhere cannot hand an agent any other folder.

use std::path::{Path, PathBuf};

use crate::app::model::{Cmd, Model};
use crate::workspace::Project;

impl Model {
    /// Returns whether visible row `row`, the project at `path`, is one of
    /// the chosen rows: inside the `V` range or marked with `v`.
    #[must_use]
    pub(crate) fn is_chosen(&self, row: usize, path: &Path) -> bool {
        self.in_visual(row) || self.marks.iter().any(|mark| mark == path)
    }

    /// Returns whether any row is chosen (the mode word is then `VISUAL`).
    #[must_use]
    pub(crate) fn choosing(&self) -> bool {
        self.visual.is_some() || !self.marks.is_empty()
    }

    /// Returns the chosen projects in list order: the `V` range and the
    /// `v` marks, without the `quick` and elsewhere rows. Empty when
    /// nothing is chosen.
    #[must_use]
    pub(crate) fn chosen(&self) -> Vec<&Project> {
        let visible = self.visible();
        let ranged = |project: &Project| {
            let row = visible.iter().position(|p| p.path == project.path);
            row.is_some_and(|row| self.in_visual(row))
        };
        self.projects
            .iter()
            .filter(|p| ranged(p) || self.marks.contains(&p.path))
            .collect()
    }

    /// Marks the selected project, or unmarks it (`v`); the `quick` and
    /// elsewhere rows cannot be marked.
    pub(super) fn toggle_mark(&mut self) {
        let Some(path) = self.selected_project().map(|p| p.path.clone()) else {
            return;
        };
        if !self.projects.iter().any(|p| p.path == path) {
            return;
        }
        match self.marks.iter().position(|mark| *mark == path) {
            Some(at) => {
                self.marks.remove(at);
            }
            None => self.marks.push(path),
        }
    }

    /// Forgets the `V` range and the `v` marks.
    pub(super) fn clear_chosen(&mut self) {
        self.visual = None;
        self.marks.clear();
    }

    /// Groups the chosen projects (`g`), and says what happened.
    ///
    /// Two or more that are exactly one existing group are ungrouped;
    /// otherwise they become a new group, each leaving the group it was
    /// in. A single chosen project only leaves its group. A group left
    /// with fewer than two members is dropped.
    ///
    /// # Returns
    ///
    /// The command that saves the groups, when they changed.
    pub(super) fn group_chosen(&mut self) -> Option<Cmd> {
        let names: Vec<String> = self.chosen().iter().map(|p| p.name.clone()).collect();
        self.clear_chosen();
        let before = self.overrides.groups.clone();
        let same = |group: &Vec<String>| {
            group.len() == names.len() && names.iter().all(|name| group.contains(name))
        };
        let exact = before.iter().any(same);
        let mut groups = before.clone();
        for group in &mut groups {
            group.retain(|name| !names.contains(name));
        }
        groups.retain(|group| group.len() > 1);
        let list = names.join(", ");
        self.message = Some(match (names.len(), exact) {
            (0, _) => return None,
            (1, _) if groups == before => "Mark two or more projects to group (v marks).".into(),
            (1, _) => format!("{list} left its group."),
            (_, true) => format!("Ungrouped {list}."),
            (_, false) => {
                groups.push(names);
                format!("Grouped {list}: new and resumed sessions get the others.")
            }
        });
        if groups == before {
            return None;
        }
        self.overrides.groups.clone_from(&groups);
        Some(Cmd::SaveGroups(groups))
    }

    /// Returns the folders grouped with `project`: the other members of
    /// its group that are projects of the open workspace.
    #[must_use]
    pub(crate) fn group_of(&self, project: &Path) -> Vec<PathBuf> {
        let named = |path: &Path| self.projects.iter().find(|p| p.path == path);
        let Some(name) = named(project).map(|p| &p.name) else {
            return Vec::new();
        };
        let groups = self.overrides.groups.iter();
        let members: Vec<&String> = groups
            .filter(|group| group.contains(name))
            .flatten()
            .collect();
        let others = self
            .projects
            .iter()
            .filter(|p| p.path != project && members.contains(&&p.name));
        others.map(|p| p.path.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;

    use crate::app::model::tests::{press, sample};
    use crate::app::model::{Cmd, Model, Overlay};

    fn keys(m: &mut Model, text: &str) -> Option<Cmd> {
        text.chars()
            .fold(None, |_, ch| m.update(press(KeyCode::Char(ch))))
    }

    fn groups(lists: &[&[&str]]) -> Vec<Vec<String>> {
        let owned = |list: &&[&str]| list.iter().map(|name| (*name).to_owned()).collect();
        lists.iter().map(owned).collect()
    }

    #[test]
    fn v_marks_separate_rows_and_g_groups_and_ungroups_them() {
        let mut m = sample(&["a", "b", "c", "d"]);
        let path = |m: &Model, i: usize| m.projects[i].path.clone();
        keys(&mut m, "vjjv");
        let chosen: Vec<&str> = m.chosen().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(chosen, ["a", "c"], "not the row between them");
        assert!(m.is_chosen(0, &path(&m, 0)) && !m.is_chosen(1, &path(&m, 1)));
        assert_eq!(
            keys(&mut m, "g"),
            Some(Cmd::SaveGroups(groups(&[&["a", "c"]])))
        );
        assert!(!m.choosing(), "the marks are used up");
        assert_eq!(m.group_of(&path(&m, 0)), [path(&m, 2)]);
        assert!(m.related(&path(&m, 0), &path(&m, 2)) && m.linked(&path(&m, 2)));
        assert!(!m.linked(&path(&m, 1)), "b is in no group");

        let regroup = keys(&mut m, "vjvg");
        assert_eq!(
            regroup,
            Some(Cmd::SaveGroups(groups(&[&["c", "d"]]))),
            "c leaves a's group, which is dropped with one member"
        );
        assert_eq!(
            keys(&mut m, "vkvg"),
            Some(Cmd::SaveGroups(Vec::new())),
            "ungroup"
        );
        assert_eq!(keys(&mut m, "vg"), None, "one project that is in no group");
        assert!(
            m.message
                .as_deref()
                .unwrap()
                .starts_with("Mark two or more")
        );

        keys(&mut m, "v");
        m.update(press(KeyCode::Esc));
        assert!(!m.choosing(), "esc clears the marks");
    }

    #[test]
    fn a_group_only_names_projects_of_the_workspace() {
        let mut m = sample(&["a", "b"]);
        m.overrides.groups = groups(&[&["a", "../outside", "/etc", "nope"]]);
        let a = m.projects[0].path.clone();
        assert!(m.group_of(&a).is_empty() && !m.linked(&a));
        m.overrides.groups = groups(&[&["a", "b", "/etc"]]);
        assert_eq!(m.group_of(&a), [m.projects[1].path.clone()]);
    }

    #[test]
    fn d_moves_the_marked_rows_to_the_trash_dialog() {
        let mut m = sample(&["a", "b", "c"]);
        keys(&mut m, "vjjvd");
        let Some(Overlay::TrashProject(projects)) = &m.overlay else {
            panic!("the Trash dialog");
        };
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a", "c"]);
        assert!(!m.choosing());
    }
}
