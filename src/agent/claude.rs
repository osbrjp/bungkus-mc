//! Claude Code specifics: the `--settings` hooks block (ARCHITECTURE §5.1).
//!
//! `--settings` hooks merge with the user's own (verified), so mc adds its
//! hook to every event without touching `~/.claude/settings.json`.

use std::path::Path;

use serde_json::{Value, json};

/// The hook events mc listens to (ARCHITECTURE §5.1).
pub(crate) const EVENTS: [&str; 9] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "SubagentStart",
    "SubagentStop",
    "Notification",
    "Stop",
    "SessionEnd",
];

/// Returns `exe` quoted for `sh -c`, followed by the subcommand, e.g.
/// `'/opt/my apps/bungkus-mc' hook`.
///
/// Agents run hook commands through a shell, so the path is single-quoted
/// with every `'` written as `'\''` (SECURITY.md "Hook subcommands").
#[must_use]
pub(crate) fn command(exe: &Path, subcommand: &str) -> String {
    let path = exe.to_string_lossy().replace('\'', r"'\''");
    format!("'{path}' {subcommand}")
}

/// Returns the `--settings` JSON that runs `bungkus-mc hook` on every
/// event in [`EVENTS`].
///
/// # Arguments
///
/// * `exe` - mc's own canonical executable path.
#[must_use]
pub(crate) fn settings(exe: &Path) -> String {
    let hook = json!([{ "hooks": [{ "type": "command", "command": command(exe, "hook") }] }]);
    let hooks: serde_json::Map<String, Value> = EVENTS
        .iter()
        .map(|e| ((*e).to_owned(), hook.clone()))
        .collect();
    json!({ "hooks": hooks }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_with_spaces_and_quotes() {
        let cases = [
            (
                "/usr/local/bin/bungkus-mc",
                "'/usr/local/bin/bungkus-mc' hook",
            ),
            (
                "/Users/me/my apps/bungkus-mc",
                "'/Users/me/my apps/bungkus-mc' hook",
            ),
            (
                "/Users/o'neil/bin/bungkus-mc",
                r"'/Users/o'\''neil/bin/bungkus-mc' hook",
            ),
        ];
        for (path, want) in cases {
            assert_eq!(command(Path::new(path), "hook"), want);
        }
    }

    #[test]
    fn quoted_command_survives_sh() {
        let tricky = "/tmp/it's a 'test'/bin";
        let line = command(Path::new(tricky), "hook");
        let out = std::process::Command::new("sh")
            .args(["-c", &format!("printf '%s|' {line}")])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("{tricky}|hook|")
        );
    }

    #[test]
    fn settings_parse_and_cover_exactly_the_events() {
        let json: Value = serde_json::from_str(&settings(Path::new("/o'k/mc"))).unwrap();
        let hooks = json["hooks"].as_object().unwrap();
        let mut names: Vec<&str> = hooks.keys().map(String::as_str).collect();
        names.sort_unstable();
        let mut want = EVENTS.to_vec();
        want.sort_unstable();
        assert_eq!(names, want);
        let cmd = &hooks["Stop"][0]["hooks"][0]["command"];
        assert_eq!(cmd, r"'/o'\''k/mc' hook");
    }
}
