//! The changes popup (`D`): the files a session changed in the repository
//! it works in, committed on its branch or not, and the diff of the
//! highlighted one, as lazygit's files panel shows them.
//!
//! Read with `git` (fixed argv, no shell, `--no-optional-locks`) on a
//! background thread. This module never writes to a repository: nothing is
//! staged, discarded or committed from here. Diff lines go through
//! `sanitise()`; a path `git status` named goes back to `git` unchanged,
//! after `--` and with `--literal-pathspecs`, and is sanitised where drawn.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::app::model::{Cmd, Model, Overlay};
use crate::ui::sanitise::sanitise;

/// Most lines kept of one diff; an editor has the rest.
const LINES_MAX: usize = 5000;

/// Longest diff line kept, in characters.
const LINE_MAX: usize = 400;

/// Lines `d`/`u` scroll the diff by, and the lines that stay in view at
/// its end.
const PAGE: usize = 10;

/// One changed file, as `git status --porcelain` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct File {
    /// The status letter of the staged change (` ` for none, `?` for an
    /// untracked file).
    pub staged: char,
    /// The status letter of the change not staged yet.
    pub unstaged: char,
    /// The path from the repository's root as `git` printed it; not
    /// sanitised, because it goes back to `git`.
    pub path: String,
    /// Whether the working tree has it as the branch's last commit does:
    /// its change is in the branch's commits only, and `staged` is its
    /// letter there.
    pub committed: bool,
}

impl File {
    /// Returns whether `git` does not track the file yet.
    const fn untracked(&self) -> bool {
        self.staged == '?'
    }
}

/// What a line of a diff is, which decides its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    /// The header before the first hunk, or a note of mc's.
    Meta,
    /// A hunk header (`@@ -1,4 +1,6 @@`).
    Hunk,
    /// An added line.
    Added,
    /// A removed line.
    Removed,
    /// An unchanged line.
    Context,
}

/// One line of a diff, sanitised.
pub(crate) type Line = (Mark, String);

/// Parses `git status --porcelain -z` output: `XY path` entries ended by
/// NUL, a renamed or copied one followed by the path it came from (which
/// is skipped). Anything else is no file.
fn parse_status(output: &str) -> Vec<File> {
    let mut entries = output.split('\0');
    let mut files = Vec::new();
    while let Some(entry) = entries.next() {
        let mut chars = entry.chars();
        let (Some(staged), Some(unstaged), Some(' ')) = (chars.next(), chars.next(), chars.next())
        else {
            continue;
        };
        if matches!(staged, 'R' | 'C') || matches!(unstaged, 'R' | 'C') {
            entries.next();
        }
        files.push(File {
            staged,
            unstaged,
            path: chars.as_str().to_owned(),
            committed: false,
        });
    }
    files
}

/// Parses `git diff --name-status -z --no-renames` output (`X`, NUL,
/// `path`, NUL, …) into files changed in commits.
fn parse_committed(output: &str) -> Vec<File> {
    let mut fields = output.split('\0');
    let mut files = Vec::new();
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        if let Some(staged) = status.chars().next() {
            files.push(File {
                staged,
                unstaged: ' ',
                path: path.to_owned(),
                committed: true,
            });
        }
    }
    files
}

/// Returns the commit the branch checked out in `dir` started from: where
/// it left the remote's default branch (`origin/HEAD`), else `HEAD`, which
/// leaves the uncommitted changes only.
// ponytail: `origin/HEAD` only. A repository without that ref (no remote,
// or one added after `git init`) shows uncommitted changes only; ask for
// the base branch when that is reported.
fn base(dir: &Path) -> String {
    let args = ["merge-base", "HEAD", "origin/HEAD"].map(OsStr::new);
    super::git(dir, &args).map_or_else(|| "HEAD".to_owned(), |sha| sha.trim().to_owned())
}

/// Lists the changed files of the repository `dir` is in: what `git
/// status` names (untracked ones file by file), then the files changed
/// only in the commits since [`base`].
///
/// # Returns
///
/// The repository's root, where [`read`] runs, and its changed files; `dir`
/// and no files when it is in no repository or `git` is missing.
#[must_use]
pub(crate) fn list(dir: &Path) -> (PathBuf, Vec<File>) {
    let root = ["rev-parse", "--show-toplevel"].map(OsStr::new);
    let status = [
        "--no-optional-locks",
        "status",
        "--porcelain",
        "-z",
        "--untracked-files=all",
    ]
    .map(OsStr::new);
    let read = || {
        let root = super::git(dir, &root)?;
        let mut files = parse_status(&super::git(dir, &status)?);
        let base = base(dir);
        let commits =
            ["diff", "--name-status", "-z", "--no-renames", &base, "HEAD"].map(OsStr::new);
        let committed = parse_committed(&super::git(dir, &commits).unwrap_or_default());
        let dirty = files.len();
        for file in committed {
            if !files[..dirty].iter().any(|f| f.path == file.path) {
                files.push(file);
            }
        }
        Some((PathBuf::from(root.trim_end_matches('\n')), files))
    };
    read().unwrap_or_else(|| (dir.to_path_buf(), Vec::new()))
}

/// Parses what `git diff` printed for one file into marked lines: tabs
/// become four spaces, every line is sanitised and cut at [`LINE_MAX`], and
/// a diff longer than [`LINES_MAX`] ends in a note.
fn lines(diff: &str) -> Vec<Line> {
    let mut hunks = false;
    let mark = |raw: &str| {
        hunks |= raw.starts_with("@@");
        let mark = match raw.chars().next() {
            _ if !hunks => Mark::Meta,
            Some('@') => Mark::Hunk,
            Some('+') => Mark::Added,
            Some('-') => Mark::Removed,
            _ => Mark::Context,
        };
        (mark, sanitise(&raw.replace('\t', "    "), LINE_MAX))
    };
    let mut out: Vec<Line> = diff.lines().take(LINES_MAX).map(mark).collect();
    if diff.lines().nth(LINES_MAX).is_some() {
        out.push((Mark::Meta, format!("… (cut at {LINES_MAX} lines)")));
    }
    out
}

/// Reads the diff of `file` in the repository at `root`: its committed,
/// staged and unstaged changes against [`base`] together, or the whole
/// file as added when `git` does not track it. Empty when `git` failed.
// ponytail: one diff against the base. A staged file in a repository
// without a commit shows no diff, and a renamed file shows as a new one.
// Diff the commits, the index and the working tree apart when that is
// reported.
// ponytail: the whole diff is read before it is cut at LINES_MAX, and a
// read is not cancelled when another file is highlighted, so holding `j`
// over slow diffs runs several `git` at once. Stream it and keep one read
// at a time when a huge generated file makes that slow.
// ponytail: a file name that is not UTF-8 is listed with U+FFFD and has no
// diff. Carry paths as `OsString` when that is reported.
#[must_use]
pub(crate) fn read(root: &Path, file: &File) -> Vec<Line> {
    let fixed = [
        "--no-optional-locks",
        "--literal-pathspecs",
        "diff",
        "--no-color",
        "--no-ext-diff",
    ];
    let base = base(root);
    let tail: &[&str] = if file.untracked() {
        &["--no-index", "--", "/dev/null", &file.path]
    } else {
        &[&base, "--", &file.path]
    };
    let args: Vec<&OsStr> = fixed.iter().chain(tail).map(OsStr::new).collect();
    // `--no-index` exits with 1 when the two differ, which they always do.
    super::capture("git", root, &args, &[])
        .filter(|output| matches!(output.status.code(), Some(0 | 1)))
        .map_or_else(Vec::new, |output| {
            lines(&String::from_utf8_lossy(&output.stdout))
        })
}

/// The changes popup (`D`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Viewer {
    /// The folder it was opened on.
    pub folder: PathBuf,
    /// The root of the repository `folder` is in (`folder` until `git`
    /// answered), where the diffs are read.
    pub root: PathBuf,
    /// The changed files; `None` while `git` runs.
    pub files: Option<Vec<File>>,
    /// Highlighted file.
    pub selected: usize,
    /// The diff of the highlighted file; `None` while `git` runs.
    pub lines: Option<Vec<Line>>,
    /// The first diff line shown.
    pub scroll: usize,
}

impl Viewer {
    /// Returns the highlighted file, if there is one.
    fn file(&self) -> Option<&File> {
        self.files.as_ref()?.get(self.selected)
    }

    /// Returns the command that reads the highlighted file's diff.
    fn read_cmd(&self) -> Option<Cmd> {
        self.file()
            .map(|file| Cmd::ReadDiff(self.root.clone(), file.clone()))
    }

    /// Returns the furthest the diff scrolls.
    fn last(&self) -> usize {
        let lines = self.lines.as_ref().map_or(0, Vec::len);
        lines.saturating_sub(PAGE)
    }
}

impl Model {
    /// Opens the changes popup (`D`) for the folder the selected session
    /// works in (else the selected project) and asks the loop to list its
    /// changed files.
    pub(super) fn open_diff(&mut self) -> Option<Cmd> {
        let (_, folder) = self.shell_owner()?;
        self.overlay = Some(Overlay::Diff(Viewer {
            folder: folder.clone(),
            root: folder.clone(),
            files: None,
            selected: 0,
            lines: None,
            scroll: 0,
        }));
        Some(Cmd::ListChanges(folder))
    }

    /// Fills the open popup with the files `git` listed for `folder` and
    /// asks for the highlighted one's diff. After a reload (`r`) the file
    /// that was highlighted stays so, with its diff in place until the new
    /// one arrives.
    pub(super) fn set_changes(
        &mut self,
        folder: &Path,
        root: PathBuf,
        files: Vec<File>,
    ) -> Option<Cmd> {
        let Some(Overlay::Diff(viewer)) = &mut self.overlay else {
            return None;
        };
        if viewer.folder != folder {
            return None;
        }
        let was = viewer.file();
        let kept = files.iter().position(|file| Some(file) == was);
        if kept.is_none() {
            viewer.lines = None;
            viewer.scroll = 0;
        }
        let end = files.len().saturating_sub(1);
        viewer.selected = kept.unwrap_or_else(|| viewer.selected.min(end));
        viewer.root = root;
        viewer.files = Some(files);
        viewer.read_cmd()
    }

    /// Gives the open popup the diff `git` read for `file` in the
    /// repository at `root`, unless another file is highlighted by now or
    /// the popup is on another repository.
    pub(super) fn set_diff(&mut self, root: &Path, file: &File, lines: Vec<Line>) {
        let Some(Overlay::Diff(viewer)) = &mut self.overlay else {
            return;
        };
        if viewer.root == root && viewer.file() == Some(file) {
            viewer.lines = Some(lines);
            viewer.scroll = viewer.scroll.min(viewer.last());
        }
    }

    /// Applies a key to the popup.
    ///
    /// `j`/`k` or `↑`/`↓` highlight another file and ask for its diff;
    /// `d`/`u` or `pgdn`/`pgup` scroll the diff by [`PAGE`], `g`/`G` go to
    /// its ends; `r` lists the files again; `esc`, `q` or `D` closes.
    pub(super) fn diff_key(&mut self, mut viewer: Viewer, key: KeyEvent) -> Option<Cmd> {
        let was = viewer.selected;
        let end = viewer.files.as_ref().map_or(0, Vec::len).saturating_sub(1);
        let mut cmd = None;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'D') => return None,
            KeyCode::Up | KeyCode::Char('k') => viewer.selected = was.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => viewer.selected = (was + 1).min(end),
            KeyCode::PageUp | KeyCode::Char('u') => {
                viewer.scroll = viewer.scroll.saturating_sub(PAGE);
            }
            KeyCode::PageDown | KeyCode::Char('d') => {
                viewer.scroll = (viewer.scroll + PAGE).min(viewer.last());
            }
            KeyCode::Home | KeyCode::Char('g') => viewer.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => viewer.scroll = viewer.last(),
            KeyCode::Char('r') => cmd = Some(Cmd::ListChanges(viewer.folder.clone())),
            _ => {}
        }
        if viewer.selected != was {
            viewer.lines = None;
            viewer.scroll = 0;
            cmd = viewer.read_cmd();
        }
        self.overlay = Some(Overlay::Diff(viewer));
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppEvent;
    use crate::app::model::tests::{press, sample, with_session};

    fn file(status: &str, path: &str) -> File {
        let mut letters = status.chars();
        File {
            staged: letters.next().unwrap(),
            unstaged: letters.next().unwrap(),
            path: path.into(),
            committed: false,
        }
    }

    fn viewer(model: &Model) -> &Viewer {
        match &model.overlay {
            Some(Overlay::Diff(viewer)) => viewer,
            other => panic!("no changes popup: {other:?}"),
        }
    }

    #[test]
    fn parses_status_entries_and_skips_the_old_name_of_a_rename() {
        let output = " M src/app.rs\0R  new name.rs\0old.rs\0?? a/[id].tsx\0MM b\0\0x\0";
        assert_eq!(
            parse_status(output),
            [
                file(" M", "src/app.rs"),
                file("R ", "new name.rs"),
                file("??", "a/[id].tsx"),
                file("MM", "b"),
            ]
        );
        assert!(parse_status("fatal: not a git repository").is_empty());
    }

    #[test]
    fn marks_diff_lines_and_sanitises_them() {
        let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@ fn x\n ctx\n\
                    --- gone\n+\tnew \u{1b}[31mred\n\\ No newline at end of file\n";
        let parsed = lines(diff);
        let marks: Vec<(Mark, &str)> = parsed.iter().map(|(m, s)| (*m, s.as_str())).collect();
        assert_eq!(
            marks,
            [
                (Mark::Meta, "diff --git a/x b/x"),
                (Mark::Meta, "--- a/x"),
                (Mark::Meta, "+++ b/x"),
                (Mark::Hunk, "@@ -1,3 +1,3 @@ fn x"),
                (Mark::Context, " ctx"),
                (Mark::Removed, "--- gone"),
                (Mark::Added, "+    new red"),
                (Mark::Context, "\\ No newline at end of file"),
            ]
        );
        let long = "+x\n".repeat(LINES_MAX + 1);
        let cut = lines(&long);
        assert_eq!(cut.len(), LINES_MAX + 1);
        assert_eq!(cut.last().unwrap().0, Mark::Meta);
    }

    #[test]
    fn reads_tracked_and_untracked_changes_from_a_real_repository() {
        let repo = std::env::temp_dir().join(format!("mc-diff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
            crate::app::git(&repo, &args).unwrap_or_else(|| panic!("git {args:?}"));
        };
        // `sub/i` is what `sub/[id]` names when read as a pattern.
        for (dir, text) in [("sub/[id]", "one\n"), ("sub/i", "uno\n")] {
            std::fs::create_dir_all(repo.join(dir)).unwrap();
            std::fs::write(repo.join(dir).join("a.txt"), text).unwrap();
        }
        run(&["init", "-q"]);
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "init",
        ]);
        run(&["update-ref", "refs/remotes/origin/HEAD", "HEAD"]);
        std::fs::write(repo.join("done.txt"), "shipped\n").unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "work",
        ]);
        std::fs::write(repo.join("sub/[id]/a.txt"), "two\n").unwrap();
        std::fs::write(repo.join("sub/i/a.txt"), "dos\n").unwrap();
        std::fs::write(repo.join("sub/new.txt"), "fresh\n").unwrap();

        let (root, files) = list(&repo.join("sub"));
        assert_eq!(root.canonicalize().unwrap(), repo.canonicalize().unwrap());
        assert_eq!(
            files,
            [
                file(" M", "sub/[id]/a.txt"),
                file(" M", "sub/i/a.txt"),
                file("??", "sub/new.txt"),
                File {
                    committed: true,
                    ..file("A ", "done.txt")
                },
            ]
        );
        let texts = |file: &File| -> Vec<String> {
            let marked = read(&root, file).into_iter();
            marked
                .filter(|(mark, _)| matches!(mark, Mark::Added | Mark::Removed))
                .map(|(_, text)| text)
                .collect()
        };
        assert_eq!(texts(&files[0]), ["-one", "+two"]);
        assert_eq!(texts(&files[2]), ["+fresh"]);
        assert_eq!(texts(&files[3]), ["+shipped"]);
        assert_eq!(list(Path::new("/")), (PathBuf::from("/"), Vec::new()));
        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn the_popup_lists_files_reads_the_highlighted_diff_and_drops_a_stale_one() {
        let mut m = sample(&["a"]);
        with_session(&mut m, "s");
        m.focus = crate::app::model::Focus::Sessions;
        let folder = m.cards[0].folder();
        let cmd = m.update(press(KeyCode::Char('D')));
        assert_eq!(cmd, Some(Cmd::ListChanges(folder.clone())));
        assert_eq!(viewer(&m).files, None);

        let root = PathBuf::from("/repo");
        let files = vec![file(" M", "a.rs"), file("??", "b.rs")];
        let other = AppEvent::Changes(PathBuf::from("/other"), root.clone(), files.clone());
        assert_eq!(m.update(other), None);
        assert_eq!(viewer(&m).files, None, "another folder's answer is dropped");
        let cmd = m.update(AppEvent::Changes(
            folder.clone(),
            root.clone(),
            files.clone(),
        ));
        assert_eq!(cmd, Some(Cmd::ReadDiff(root.clone(), files[0].clone())));

        let cmd = m.update(press(KeyCode::Char('j')));
        assert_eq!(cmd, Some(Cmd::ReadDiff(root.clone(), files[1].clone())));
        let stale = vec![(Mark::Added, "+a".to_owned())];
        m.update(AppEvent::Diff(root.clone(), files[0].clone(), stale));
        assert_eq!(viewer(&m).lines, None, "a.rs is not highlighted any more");
        let long: Vec<Line> = (0..25).map(|n| (Mark::Added, format!("+{n}"))).collect();
        m.update(AppEvent::Diff(
            "/other".into(),
            files[1].clone(),
            long.clone(),
        ));
        assert_eq!(viewer(&m).lines, None, "another repository's b.rs");
        m.update(AppEvent::Diff(root.clone(), files[1].clone(), long));
        assert_eq!(m.update(press(KeyCode::Char('j'))), None, "last file");

        m.update(press(KeyCode::Char('d')));
        m.update(press(KeyCode::Char('d')));
        assert_eq!(viewer(&m).scroll, 15, "the last page stays in view");
        m.update(press(KeyCode::Char('u')));
        assert_eq!(viewer(&m).scroll, 5);

        let cmd = m.update(press(KeyCode::Char('r')));
        assert_eq!(cmd, Some(Cmd::ListChanges(folder.clone())));
        let cmd = m.update(AppEvent::Changes(
            folder,
            root.clone(),
            vec![files[1].clone()],
        ));
        assert_eq!(cmd, Some(Cmd::ReadDiff(root, files[1].clone())));
        assert_eq!((viewer(&m).selected, viewer(&m).scroll), (0, 5));
        assert!(viewer(&m).lines.is_some(), "the diff stays over a reload");

        m.update(press(KeyCode::Esc));
        assert_eq!(m.overlay, None);
    }
}
