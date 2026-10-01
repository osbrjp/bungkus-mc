//! The workspace: one folder whose direct children are the projects.
//!
//! A project is a direct child directory holding `CLAUDE.md`, `AGENTS.md`
//! or `.git` (PROPOSAL §6 item 2). Dot-directories are skipped, symlinked
//! directories and markers are followed, names are sanitised, and nothing
//! below the first level is read, except git's worktree records: a linked
//! worktree's `.git` *file* names its main repository in its `gitdir:`
//! line, and a repository's `.git/worktrees/*/gitdir` files name its
//! worktrees. A worktree whose main repository is a project is listed
//! right below it, wherever on disk it lives (an agent's own worktrees
//! are usually inside the repository or in a temporary folder).

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
    /// `workspace.join(<raw folder name>)`, or the folder of a linked
    /// worktree outside the workspace; the agent's working directory.
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
    let elsewhere: Vec<Project> = projects
        .iter()
        .flat_map(|p| linked_worktrees(&p.path))
        .filter(|folder| !projects.iter().any(|p| same_dir(&p.path, folder)))
        .map(|path| Project {
            name: sanitise(
                &path.file_name().unwrap_or_default().to_string_lossy(),
                NAME_MAX,
            ),
            path,
            worktree_of: None,
        })
        .collect();
    projects.extend(elsewhere);
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(nest_worktrees(&projects))
}

/// Returns the folders of the linked worktrees of repository `repo`, which
/// may be anywhere on disk: each `<repo>/.git/worktrees/<name>/gitdir`
/// names a worktree's `.git` file, and only a folder whose `.git` file
/// points back at `repo` counts (as git itself requires), so a stale or
/// edited record lists nothing.
fn linked_worktrees(repo: &Path) -> Vec<PathBuf> {
    let Ok(records) = std::fs::read_dir(repo.join(".git/worktrees")) else {
        return Vec::new();
    };
    records
        .filter_map(Result::ok)
        .filter_map(|record| {
            let text = std::fs::read_to_string(record.path().join("gitdir")).ok()?;
            let dot_git = record.path().join(text.lines().next()?.trim());
            let folder = dot_git.parent()?.to_path_buf();
            main_repo(&folder)
                .is_some_and(|main| same_dir(&main, repo))
                .then_some(folder)
        })
        .collect()
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

/// Moves project folder `path` into the Trash and returns where it went.
///
/// Only a direct child of `root` (symlinks resolved for the check; a
/// symlinked project moves as the link) is accepted. macOS uses `~/.Trash`;
/// elsewhere the freedesktop trash (`$XDG_DATA_HOME/Trash`, with its
/// `.trashinfo` file). A name already in the Trash gets the time appended.
/// The move is a `rename`, so nothing is copied or deleted.
///
/// # Errors
///
/// `InvalidInput` when `path` is not a direct child of `root`; the error
/// of the rename (`CrossesDevices` when the Trash is on another disk) or
/// of creating the Trash folders.
pub(crate) fn trash(
    path: &Path,
    root: &Path,
    home: &Path,
    data_home: Option<&Path>,
) -> std::io::Result<PathBuf> {
    use std::io::{Error, ErrorKind};
    let parent = path.parent().map(Path::canonicalize).transpose()?;
    if parent.as_deref() != Some(root.canonicalize()?.as_path()) || path.file_name().is_none() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "not a project folder of the workspace",
        ));
    }
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let (files, info) = if cfg!(target_os = "macos") {
        (home.join(".Trash"), None)
    } else {
        let base = data_home
            .map_or_else(|| home.join(".local/share"), Path::to_path_buf)
            .join("Trash");
        (base.join("files"), Some(base.join("info")))
    };
    std::fs::create_dir_all(&files)?;
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut target = files.join(&name);
    if std::fs::symlink_metadata(&target).is_ok() {
        target = files.join(format!("{name} {unix}"));
    }
    if let Some(info) = info {
        std::fs::create_dir_all(&info)?;
        let stem = target
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let entry = format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            path.display(),
            rfc3339_local(unix)
        );
        std::fs::write(info.join(format!("{stem}.trashinfo")), entry)?;
    }
    std::fs::rename(path, &target)?;
    Ok(target)
}

/// Moves a trashed project (`trashed`, as [`trash`] returned it) back to
/// `folder`, and drops its freedesktop `.trashinfo` when there is one.
///
/// # Errors
///
/// `AlreadyExists` when `folder` exists again; the rename's error.
pub(crate) fn restore(trashed: &Path, folder: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(folder).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "a folder with that name exists again",
        ));
    }
    std::fs::rename(trashed, folder)?;
    if let (Some(files), Some(name)) = (trashed.parent(), trashed.file_name())
        && let Some(base) = files.parent()
    {
        let info = base
            .join("info")
            .join(format!("{}.trashinfo", name.to_string_lossy()));
        // reason: macOS has no info file, and a stale one is harmless.
        let _ = std::fs::remove_file(info);
    }
    Ok(())
}

/// Formats unix seconds as `YYYY-MM-DDThh:mm:ss` (UTC; the trash spec asks
/// for local time, which only affects the date the trash UI shows).
fn rfc3339_local(unix: u64) -> String {
    let days = unix / 86_400;
    let secs = unix % 86_400;
    // Civil-from-days (Howard Hinnant), valid for every date mc will see.
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
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
        // Worktrees outside the workspace: one recorded both ways, one
        // whose folder is gone, one whose folder does not point back.
        let far = ws.join("app/.claude/worktrees/far");
        for dir in ["far", "gone", "liar"] {
            fs::create_dir_all(ws.join("app/.git/worktrees").join(dir)).unwrap();
        }
        fs::create_dir_all(&far).unwrap();
        fs::write(far.join(".git"), gitdir("app/.git/worktrees/far")).unwrap();
        let record = |name: &str, folder: &Path| {
            fs::write(
                ws.join("app/.git/worktrees").join(name).join("gitdir"),
                format!("{}\n", folder.join(".git").display()),
            )
            .unwrap();
        };
        record("far", &far);
        record("gone", &ws.join("app/.claude/worktrees/gone"));
        record("liar", &ws.join("lib"));
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
                row("far", Some("app")),
                row("zz-wt", Some("app")),
                row("lib", None),
                row("orphan", None),
                row("sub", None),
            ]
        );
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn trashes_only_a_project_folder_of_the_workspace() {
        let root = std::env::temp_dir().join(format!("mc-trash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let ws = root.join("ws");
        let home = root.join("home");
        fs::create_dir_all(ws.join("old-app/.git")).unwrap();
        fs::create_dir_all(ws.join("keep/inner")).unwrap();
        fs::create_dir_all(&home).unwrap();
        let data = root.join("data");
        assert!(
            trash(&ws.join("keep/inner"), &ws, &home, Some(&data)).is_err(),
            "not a direct child"
        );
        assert!(
            trash(&ws, &ws, &home, Some(&data)).is_err(),
            "not the workspace itself"
        );
        let went = trash(&ws.join("old-app"), &ws, &home, Some(&data)).unwrap();
        assert!(!ws.join("old-app").exists());
        assert!(
            went.join(".git").exists(),
            "moved, not deleted: {}",
            went.display()
        );
        fs::create_dir_all(ws.join("old-app")).unwrap();
        let again = trash(&ws.join("old-app"), &ws, &home, Some(&data)).unwrap();
        assert_ne!(
            again, went,
            "a second one with the same name does not clash"
        );
        restore(&again, &ws.join("old-app")).unwrap();
        assert!(ws.join("old-app").exists(), "u puts it back");
        assert!(
            restore(&went, &ws.join("old-app")).is_err(),
            "never over a folder"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn formats_trash_dates() {
        assert_eq!(rfc3339_local(0), "1970-01-01T00:00:00");
        assert_eq!(rfc3339_local(1_790_823_759), "2026-10-01T03:02:39");
    }

    #[test]
    fn a_missing_workspace_is_an_error() {
        assert!(scan(Path::new("/nonexistent/mc-workspace")).is_err());
    }
}
