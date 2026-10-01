//! The git branch and status shown on a session card (issue #110).
//!
//! Read with `git --no-optional-locks status --porcelain=v2 --branch`
//! (fixed argv) in the folder a session works in, on a background thread;
//! `--no-optional-locks` keeps mc from taking the index lock an agent's
//! own git command may need.

use std::ffi::OsStr;
use std::path::Path;

use crate::ui::sanitise::{sanitise, truncate};

/// Longest branch name kept, in characters.
const BRANCH_MAX: usize = 64;

/// What a session card says about its folder's repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    /// The branch, sanitised; `detached` without one.
    pub branch: String,
    /// Files with staged, unstaged, unmerged or untracked changes.
    pub changed: usize,
    /// Commits the branch has that its upstream lacks.
    pub ahead: u32,
    /// Commits the upstream has that the branch lacks.
    pub behind: u32,
}

impl Status {
    /// Parses `git status --porcelain=v2 --branch` output; `None` when it
    /// names no branch line.
    #[must_use]
    pub(crate) fn parse(output: &str) -> Option<Self> {
        let mut status = Self {
            branch: String::new(),
            changed: 0,
            ahead: 0,
            behind: 0,
        };
        let mut seen = false;
        for line in output.lines() {
            if let Some(head) = line.strip_prefix("# branch.head ") {
                seen = true;
                status.branch = if head == "(detached)" {
                    "detached".to_owned()
                } else {
                    sanitise(head, BRANCH_MAX)
                };
            } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
                let mut counts = ab.split_whitespace().map(|n| n[1..].parse().unwrap_or(0));
                status.ahead = counts.next().unwrap_or(0);
                status.behind = counts.next().unwrap_or(0);
            } else if !line.starts_with('#') && !line.is_empty() {
                status.changed += 1;
            }
        }
        seen.then_some(status)
    }

    /// Reads the status of the repository `dir` is in; `None` when it is
    /// not in one or `git` is missing.
    #[must_use]
    pub(crate) fn read(dir: &Path) -> Option<Self> {
        let args = [
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
        ]
        .map(OsStr::new);
        Self::parse(&super::git(dir, &args)?)
    }

    /// Returns the card line within `width` columns: the branch (cut short
    /// first), then `clean` or the number of changed files, then `↑n` /
    /// `↓n` against the upstream.
    #[must_use]
    pub(crate) fn label(&self, width: usize) -> String {
        let changes = match self.changed {
            0 => "clean".to_owned(),
            n => format!("{n} changed"),
        };
        let count = |arrow: char, n: u32| match n {
            0 => String::new(),
            n => format!(" {arrow}{n}"),
        };
        let tail = format!(
            " · {changes}{}{}",
            count('↑', self.ahead),
            count('↓', self.behind)
        );
        let room = width.saturating_sub(tail.chars().count());
        format!("{}{tail}", truncate(&self.branch, room))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_counts_and_changes_and_fits_the_label() {
        let dirty = "# branch.oid abc\n# branch.head i110-20261001-1600\n\
                     # branch.upstream origin/x\n# branch.ab +2 -1\n\
                     1 .M N... 100644 100644 100644 a b src/app.rs\n? new.txt\n";
        let cases = [
            (dirty, 40, "i110-20261001-1600 · 2 changed ↑2 ↓1"),
            (dirty, 30, "i110-202610… · 2 changed ↑2 ↓1"),
            ("# branch.head main\n", 40, "main · clean"),
            (
                "# branch.head (detached)\n? x\n",
                40,
                "detached · 1 changed",
            ),
            ("# branch.head a\u{1b}]0;x\u{7}b\n", 40, "ab · clean"),
        ];
        for (output, width, want) in cases {
            let status = Status::parse(output).unwrap();
            assert_eq!(status.label(width), want, "{output:?}");
        }
        assert_eq!(Status::parse("fatal: not a git repository\n"), None);
    }
}
