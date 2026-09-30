//! The folder browser under the workspace field: the current folder's
//! subfolders as a selectable list, like a file picker (setup wizard and
//! settings screen).
//!
//! It only lists directories (dot-folders hidden), marks the ones that are
//! projects, and counts them, so the user can see which folder is a good
//! workspace: one whose children are projects.

use std::path::{Path, PathBuf};

use crate::ui::sanitise::sanitise;
use crate::workspace::is_project;

/// Longest folder name shown.
const NAME_MAX: usize = 60;

/// Most folders listed; a folder with more shows the first ones by name.
const ENTRIES_MAX: usize = 500;

/// One subfolder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// Folder name, sanitised for display.
    pub name: String,
    /// Full path.
    pub path: PathBuf,
    /// Whether the folder is itself a project.
    pub project: bool,
}

/// The browser's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Browser {
    /// The folder being listed.
    pub dir: PathBuf,
    /// Its visible subfolders, sorted by name.
    pub entries: Vec<Entry>,
    /// Highlighted entry.
    pub selected: usize,
}

impl Browser {
    /// Lists `dir`; an unreadable folder lists nothing.
    #[must_use]
    pub(crate) fn open(dir: &Path) -> Self {
        let mut entries: Vec<Entry> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                    .map(|e| dir.join(e.file_name()))
                    .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_dir()))
                    .take(ENTRIES_MAX)
                    .map(|path| Entry {
                        name: sanitise(
                            &path.file_name().unwrap_or_default().to_string_lossy(),
                            NAME_MAX,
                        ),
                        project: is_project(&path),
                        path,
                    })
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by_key(|e| e.name.to_lowercase());
        Self {
            dir: dir.to_path_buf(),
            entries,
            selected: 0,
        }
    }

    /// Returns how many subfolders are projects.
    #[must_use]
    pub(crate) fn projects(&self) -> usize {
        self.entries.iter().filter(|e| e.project).count()
    }

    /// Moves the highlight by `delta`, clamped to the list.
    pub(crate) fn step(&mut self, delta: isize) {
        let last = self.entries.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// Returns the highlighted folder, if the list is not empty.
    #[must_use]
    pub(crate) fn highlighted(&self) -> Option<&Path> {
        self.entries.get(self.selected).map(|e| e.path.as_path())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn lists_visible_subfolders_marks_projects_and_moves() {
        let root = std::env::temp_dir().join(format!("mc-browse-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in ["b-app/.git", "a-notes", ".hidden", "c-api"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("c-api/AGENTS.md"), "").unwrap();
        fs::write(root.join("file.txt"), "").unwrap();
        let mut b = Browser::open(&root);
        let names: Vec<&str> = b.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a-notes", "b-app", "c-api"]);
        assert_eq!(b.projects(), 2);
        b.step(5);
        assert_eq!(b.highlighted(), Some(root.join("c-api").as_path()));
        b.step(-9);
        assert_eq!(b.selected, 0);
        assert!(Browser::open(&root.join("missing")).entries.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }
}
