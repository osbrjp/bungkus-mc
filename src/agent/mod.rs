//! The agents mc can run: Claude Code and Codex.
//!
//! This module names them, finds them on `PATH` and builds their launch
//! argv. It never reads agent transcripts; structure arrives through hook
//! events.

pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod codex_usage;
pub(crate) mod usage;

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::term::SessionId;

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

    /// Returns the one-letter badge (DESIGN §5.4).
    #[must_use]
    pub(crate) const fn badge(self) -> char {
        match self {
            Self::Claude => 'C',
            Self::Codex => 'X',
        }
    }

    /// Returns the product name used in user-facing messages.
    #[must_use]
    pub(crate) const fn product(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    /// Returns the models the `n` picker offers; `default` means no flag.
    #[must_use]
    pub(crate) const fn models(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["default", "haiku", "sonnet", "opus"],
            Self::Codex => &["default"],
        }
    }
}

/// What the user chose in the `n` picker for a new session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launch {
    /// mc's id; Claude also gets it as `--session-id`.
    pub id: SessionId,
    /// Model id, or `None` for the agent's default.
    pub model: Option<String>,
    /// Session name (`--name` for Claude; Codex has no such flag).
    pub name: Option<String>,
    /// Start prompt, passed as one argument after `--`.
    pub prompt: Option<String>,
    /// Claude `--settings` JSON (mc's hooks); `None` without a socket.
    pub settings: Option<String>,
    /// Codex `-c hooks.*` arguments; empty without a socket.
    pub hook_args: Vec<String>,
    /// The agent's own session id to resume; must be a UUID.
    pub resume: Option<String>,
}

/// Builds the argument vector for a new session (ARCHITECTURE §5.1, §5.2).
///
/// The prompt is always one argument after `--`, so a prompt starting with
/// `-` cannot become a flag; the name is one `--name` value. Nothing goes
/// through a shell.
///
/// # Arguments
///
/// * `kind`    - Which agent.
/// * `program` - The agent's resolved executable.
/// * `args`    - Extra arguments from config, placed before mc's.
/// * `launch`  - The picker's choices.
#[must_use]
pub(crate) fn argv(kind: Kind, program: &Path, args: &[String], launch: &Launch) -> Vec<OsString> {
    let mut out: Vec<OsString> = vec![program.into()];
    out.extend(args.iter().map(OsString::from));
    if kind == Kind::Codex {
        out.extend(launch.hook_args.iter().map(OsString::from));
    }
    let mut flag = |name: &str, value: &str| {
        out.push(name.into());
        out.push(value.into());
    };
    match (kind, &launch.resume) {
        (Kind::Claude, resume) => {
            match resume {
                Some(id) => flag("--resume", id),
                None => flag("--session-id", &launch.id.0.hyphenated().to_string()),
            }
            if let Some(settings) = &launch.settings {
                flag("--settings", settings);
            }
            if let Some(model) = &launch.model {
                flag("--model", model);
            }
            if let (Some(name), None) = (&launch.name, resume) {
                flag("--name", name);
            }
        }
        (Kind::Codex, resume) => {
            if let Some(model) = &launch.model {
                flag("-m", model);
            }
            if let Some(id) = resume {
                flag("resume", id);
            }
        }
    }
    if let Some(prompt) = &launch.prompt {
        out.push("--".into());
        out.push(prompt.into());
    }
    out
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
    fn builds_launch_argv_per_agent() {
        let id = SessionId(uuid::Uuid::nil());
        let full = Launch {
            id,
            model: Some("haiku".into()),
            name: Some("flaky test".into()),
            prompt: Some("-rf everything".into()),
            settings: Some("{}".into()),
            hook_args: vec!["-c".into(), "hooks.Stop=[]".into()],
            resume: None,
        };
        let bare = Launch {
            id,
            model: None,
            name: None,
            prompt: None,
            settings: None,
            hook_args: Vec::new(),
            resume: None,
        };
        let s = |v: Vec<OsString>| {
            v.into_iter()
                .map(|a| a.into_string().unwrap())
                .collect::<Vec<_>>()
        };
        let claude = Path::new("/bin/claude");
        assert_eq!(
            s(argv(Kind::Claude, claude, &["--verbose".into()], &full)),
            [
                "/bin/claude",
                "--verbose",
                "--session-id",
                "00000000-0000-0000-0000-000000000000",
                "--settings",
                "{}",
                "--model",
                "haiku",
                "--name",
                "flaky test",
                "--",
                "-rf everything",
            ]
        );
        assert_eq!(
            s(argv(Kind::Claude, claude, &[], &bare)),
            [
                "/bin/claude",
                "--session-id",
                "00000000-0000-0000-0000-000000000000"
            ]
        );
        assert_eq!(
            s(argv(Kind::Codex, Path::new("/bin/codex"), &[], &full)),
            [
                "/bin/codex",
                "-c",
                "hooks.Stop=[]",
                "-m",
                "haiku",
                "--",
                "-rf everything"
            ]
        );
    }

    #[test]
    fn builds_resume_argv_without_new_session_flags() {
        let id = SessionId(uuid::Uuid::nil());
        let resume = Launch {
            id,
            model: None,
            name: Some("ignored".into()),
            prompt: None,
            settings: Some("{}".into()),
            hook_args: vec!["-c".into(), "hooks.Stop=[]".into()],
            resume: Some("5f1c0000-0000-0000-0000-000000000000".into()),
        };
        let s = |v: Vec<OsString>| {
            v.into_iter()
                .map(|a| a.into_string().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            s(argv(Kind::Claude, Path::new("/bin/claude"), &[], &resume)),
            [
                "/bin/claude",
                "--resume",
                "5f1c0000-0000-0000-0000-000000000000",
                "--settings",
                "{}"
            ]
        );
        assert_eq!(
            s(argv(Kind::Codex, Path::new("/bin/codex"), &[], &resume)),
            [
                "/bin/codex",
                "-c",
                "hooks.Stop=[]",
                "resume",
                "5f1c0000-0000-0000-0000-000000000000"
            ]
        );
    }

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
