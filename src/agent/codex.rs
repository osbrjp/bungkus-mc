//! Codex specifics: hooks injected per launch with `-c` (ARCHITECTURE §5.2).
//!
//! Codex asks the user to review new hooks once and records its trust
//! against the hook definition's hash (verified with Codex 0.159.2: the
//! approval survives a restart as long as the injected definition is
//! byte-identical), so mc keeps its hook command stable. mc never passes
//! `--dangerously-bypass-hook-trust`.

use std::path::Path;

use crate::agent::claude::command;

/// The hook events mc listens to (ARCHITECTURE §5.2).
pub(crate) const EVENTS: [&str; 9] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "SessionEnd",
];

/// Returns `text` as a TOML basic string, quotes and backslashes escaped.
fn toml_string(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Returns the `-c` arguments that run `bungkus-mc hook` on every event
/// in [`EVENTS`], e.g. `-c hooks.Stop=[{hooks=[{type="command",command="…"}]}]`.
///
/// # Arguments
///
/// * `exe` - mc's own canonical executable path.
#[must_use]
pub(crate) fn hook_args(exe: &Path) -> Vec<String> {
    let cmd = toml_string(&command(exe, "hook"));
    EVENTS
        .iter()
        .flat_map(|event| {
            [
                "-c".to_owned(),
                format!("hooks.{event}=[{{hooks=[{{type=\"command\",command={cmd}}}]}}]"),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_args_cover_every_event_with_the_quoted_command() {
        let args = hook_args(Path::new("/opt/it's/bungkus-mc"));
        assert_eq!(args.len(), EVENTS.len() * 2);
        assert!(args.iter().step_by(2).all(|a| a == "-c"));
        assert_eq!(
            args[1],
            r#"hooks.SessionStart=[{hooks=[{type="command",command="'/opt/it'\\''s/bungkus-mc' hook"}]}]"#
        );
    }

    #[test]
    fn toml_strings_escape_quotes_and_backslashes() {
        assert_eq!(toml_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
