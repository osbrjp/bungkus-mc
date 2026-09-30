//! The workspace: one folder whose direct children are the projects.
//!
//! A project is a direct child directory holding `CLAUDE.md`, `AGENTS.md`
//! or `.git` (PROPOSAL §6 item 2). Dot-directories are skipped, symlinked
//! directories and markers are followed, names are sanitised, and nothing
//! below the first level is read.

use std::path::{Path, PathBuf};

use crate::ui::sanitise::sanitise;

/// Files or directories whose presence makes a folder a project.
const MARKERS: [&str; 3] = ["CLAUDE.md", "AGENTS.md", ".git"];

/// Longest project name shown, in characters.
const NAME_MAX: usize = 64;

/// One project folder in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Project {
    /// Folder name, sanitised for display.
    pub name: String,
    /// `workspace.join(<raw folder name>)`; the agent's working directory.
    pub path: PathBuf,
}

/// Lists the projects in `workspace`, sorted by name.
///
/// # Errors
///
/// Returns the I/O error if `workspace` cannot be listed. Entries that
/// cannot be inspected are skipped.
pub(crate) fn scan(workspace: &Path) -> std::io::Result<Vec<Project>> {
    let mut projects: Vec<Project> = std::fs::read_dir(workspace)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let raw = entry.file_name();
            let path = workspace.join(&raw);
            let visible = !raw.to_string_lossy().starts_with('.');
            let is_project = visible && is_project(&path);
            is_project.then(|| Project {
                name: sanitise(&raw.to_string_lossy(), NAME_MAX),
                path,
            })
        })
        .collect();
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(projects)
}

/// Returns whether `path` is a project folder: a directory holding
/// `CLAUDE.md`, `AGENTS.md` or `.git` (symlinks followed).
#[must_use]
pub(crate) fn is_project(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_dir())
        && MARKERS
            .iter()
            .any(|m| std::fs::metadata(path.join(m)).is_ok())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn lists_marked_children_and_skips_the_rest() {
        let root = std::env::temp_dir().join(format!("mc-ws-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let outside = root.join("outside");
        let ws = root.join("ws");
        for dir in [
            "b-agents",
            "a-claude",
            "c-git/.git",
            "plain",
            ".hidden",
            "esc\u{1b}]0;x\u{7}",
        ] {
            fs::create_dir_all(ws.join(dir)).unwrap();
        }
        fs::create_dir_all(&outside).unwrap();
        fs::write(ws.join("a-claude/CLAUDE.md"), "").unwrap();
        fs::write(ws.join("b-agents/AGENTS.md"), "").unwrap();
        fs::write(ws.join(".hidden/CLAUDE.md"), "").unwrap();
        fs::write(ws.join("esc\u{1b}]0;x\u{7}/CLAUDE.md"), "").unwrap();
        fs::write(ws.join("file.txt"), "").unwrap();
        fs::write(outside.join("AGENTS.md"), "").unwrap();
        symlink(&outside, ws.join("linked")).unwrap();

        let names: Vec<String> = scan(&ws).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["a-claude", "b-agents", "c-git", "esc", "linked"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_workspace_is_an_error() {
        assert!(scan(Path::new("/nonexistent/mc-workspace")).is_err());
    }
}
