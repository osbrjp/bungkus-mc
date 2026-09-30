//! The workspace: one folder whose direct children are the projects.
//!
//! A project is a direct child directory holding `CLAUDE.md`, `AGENTS.md`
//! or `.git` (PROPOSAL §6 item 2). Dot-directories are skipped, symlinked
//! directories and markers are followed, names are sanitised, and nothing
//! below the first level is read, except a `.git` *file*: a linked git
//! worktree's `gitdir:` line names its main repository, and a worktree
//! whose main repository is also a project is listed right below it.

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
    /// The name of the project this folder is a linked git worktree of,
    /// when that repository is a project in the same workspace.
    pub worktree_of: Option<String>,
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
                worktree_of: None,
            })
        })
        .collect();
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(nest_worktrees(&projects))
}

/// Moves every linked worktree right below its main repository's project
/// (worktrees sorted by name) and names that project in `worktree_of`; a
/// worktree whose repository is not in the list stays where it is.
fn nest_worktrees(projects: &[Project]) -> Vec<Project> {
    let mains: Vec<Option<PathBuf>> = projects.iter().map(|p| main_repo(&p.path)).collect();
    let parent_of = |i: usize| {
        let main = mains[i].as_ref()?;
        projects
            .iter()
            .position(|p| same_dir(&p.path, main))
            .filter(|&j| j != i && mains[j].is_none())
    };
    let parents: Vec<Option<usize>> = (0..projects.len()).map(parent_of).collect();
    let mut out = Vec::with_capacity(projects.len());
    for (i, project) in projects.iter().enumerate() {
        if parents[i].is_some() {
            continue;
        }
        out.push(project.clone());
        for (j, child) in projects.iter().enumerate() {
            if parents[j] == Some(i) {
                out.push(Project {
                    worktree_of: Some(project.name.clone()),
                    ..child.clone()
                });
            }
        }
    }
    out
}

/// Returns whether two paths name the same folder (symlinks resolved).
pub(crate) fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Returns the main repository folder of a linked git worktree: `.git` is
/// a file whose `gitdir: <repo>/.git/worktrees/<name>` line points into
/// the main repository's `.git`. A submodule (`.git/modules/…`) or a plain
/// repository returns `None`.
fn main_repo(path: &Path) -> Option<PathBuf> {
    let dot_git = path.join(".git");
    if !std::fs::symlink_metadata(&dot_git).is_ok_and(|m| m.is_file()) {
        return None;
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = path.join(text.lines().next()?.strip_prefix("gitdir:")?.trim());
    let worktrees = gitdir.parent()?;
    let git = worktrees.parent()?;
    (worktrees.file_name()? == "worktrees" && git.file_name()? == ".git")
        .then(|| git.parent().map(Path::to_path_buf))
        .flatten()
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
    fn nests_linked_worktrees_below_their_repository() {
        let ws = std::env::temp_dir().join(format!("mc-wt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        for dir in [
            "app/.git/worktrees/app-fix",
            "app-fix",
            "zz-wt",
            "lib/.git/modules/sub",
            "sub",
            "orphan",
        ] {
            fs::create_dir_all(ws.join(dir)).unwrap();
        }
        let gitdir = |to: &str| format!("gitdir: {}\n", ws.join(to).display());
        fs::write(
            ws.join("app-fix/.git"),
            gitdir("app/.git/worktrees/app-fix"),
        )
        .unwrap();
        fs::write(ws.join("zz-wt/.git"), gitdir("app/.git/worktrees/zz-wt")).unwrap();
        fs::write(ws.join("sub/.git"), gitdir("lib/.git/modules/sub")).unwrap();
        fs::write(
            ws.join("orphan/.git"),
            "gitdir: /elsewhere/.git/worktrees/o\n",
        )
        .unwrap();
        let rows: Vec<(String, Option<String>)> = scan(&ws)
            .unwrap()
            .into_iter()
            .map(|p| (p.name, p.worktree_of))
            .collect();
        let row = |n: &str, of: Option<&str>| (n.to_owned(), of.map(str::to_owned));
        assert_eq!(
            rows,
            [
                row("app", None),
                row("app-fix", Some("app")),
                row("zz-wt", Some("app")),
                row("lib", None),
                row("orphan", None),
                row("sub", None),
            ]
        );
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn a_missing_workspace_is_an_error() {
        assert!(scan(Path::new("/nonexistent/mc-workspace")).is_err());
    }
}
