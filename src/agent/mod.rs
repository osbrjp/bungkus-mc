//! The agents mc can run: Claude Code and Codex.
//!
//! This module names them and finds them on `PATH`. It never reads agent
//! transcripts; structure arrives through hook events.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Which agent CLI a session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    /// Claude Code (`claude`).
    #[default]
    Claude,
    /// Codex CLI (`codex`).
    Codex,
}

impl Kind {
    /// Both agents, in picker order.
    pub(crate) const ALL: [Self; 2] = [Self::Claude, Self::Codex];

    /// Returns the command name, which is also the word shown in the UI.
    #[must_use]
    pub(crate) const fn command(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// Returns the first executable file named `name` in the `PATH` list.
///
/// # Arguments
///
/// * `name` - Command name without a directory.
/// * `path` - The value of `PATH` (colon-separated directories).
#[must_use]
pub(crate) fn find_on_path(name: &str, path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// Returns whether `path` is a file with an execute bit (symlinks followed).
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn finds_only_executable_files_on_path() {
        let root = std::env::temp_dir().join(format!("mc-path-{}", std::process::id()));
        let (a, b) = (root.join("a"), root.join("b"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(b.join("claude")).unwrap();
        fs::write(a.join("codex"), "").unwrap();
        fs::write(b.join("codex"), "").unwrap();
        fs::set_permissions(b.join("codex"), fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([&a, &b]).unwrap();

        assert_eq!(find_on_path("codex", &path), Some(b.join("codex")));
        assert_eq!(
            find_on_path("claude", &path),
            None,
            "a directory is not a command"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
