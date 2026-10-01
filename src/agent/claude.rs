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

/// The user's own Claude status line, as their settings define it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserStatusLine {
    /// The shell command.
    pub command: String,
    /// Their `padding`, kept as-is.
    pub padding: Option<Value>,
    /// Their `refreshInterval`, kept as-is.
    pub refresh: Option<Value>,
}

/// Finds the user's effective status line for a session in `cwd`
/// (ARCHITECTURE §6.2): the first of `<cwd>/.claude/settings.local.json`,
/// `<cwd>/.claude/settings.json` and `<config_dir>/settings.json` that
/// defines one. A malformed file defines none; a command that is mc's own
/// `statusline` is dropped (recursion guard).
///
/// # Arguments
///
/// * `cwd`        - The project the session starts in.
/// * `config_dir` - `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
#[must_use]
pub(crate) fn user_statusline(cwd: &Path, config_dir: &Path) -> Option<UserStatusLine> {
    let files = [
        cwd.join(".claude/settings.local.json"),
        cwd.join(".claude/settings.json"),
        config_dir.join("settings.json"),
    ];
    let line = files.iter().find_map(|file| {
        let bytes = std::fs::read(file).ok()?;
        let settings: Value = serde_json::from_slice(&bytes).ok()?;
        let line = settings.get("statusLine")?;
        Some(UserStatusLine {
            command: line.get("command")?.as_str()?.to_owned(),
            padding: line.get("padding").cloned(),
            refresh: line.get("refreshInterval").cloned(),
        })
    })?;
    let ours =
        line.command.contains("bungkus-mc") && line.command.trim_end().ends_with("statusline");
    (!ours).then_some(line)
}

/// Returns the `--settings` JSON: `bungkus-mc hook` on every event in
/// [`EVENTS`], and `bungkus-mc statusline` as the status line with the
/// user's padding and refresh interval (30 s when they set none).
///
/// # Arguments
///
/// * `exe`  - mc's own canonical executable path.
/// * `user` - The user's own status line, if any.
#[must_use]
pub(crate) fn settings(exe: &Path, user: Option<&UserStatusLine>) -> String {
    let hook = json!([{ "hooks": [{ "type": "command", "command": command(exe, "hook") }] }]);
    let hooks: serde_json::Map<String, Value> = EVENTS
        .iter()
        .map(|e| ((*e).to_owned(), hook.clone()))
        .collect();
    let mut line = json!({
        "type": "command",
        "command": command(exe, "statusline"),
        "refreshInterval": user.and_then(|u| u.refresh.clone()).unwrap_or_else(|| json!(30)),
    });
    if let Some(padding) = user.and_then(|u| u.padding.clone()) {
        line["padding"] = padding;
    }
    json!({ "hooks": hooks, "statusLine": line }).to_string()
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
        let json: Value = serde_json::from_str(&settings(Path::new("/o'k/mc"), None)).unwrap();
        let hooks = json["hooks"].as_object().unwrap();
        let mut names: Vec<&str> = hooks.keys().map(String::as_str).collect();
        names.sort_unstable();
        let mut want = EVENTS.to_vec();
        want.sort_unstable();
        assert_eq!(names, want);
        assert_eq!(json["statusLine"]["refreshInterval"], 30);
        assert_eq!(json["statusLine"]["command"], r"'/o'\''k/mc' statusline");
        let cmd = &hooks["Stop"][0]["hooks"][0]["command"];
        assert_eq!(cmd, r"'/o'\''k/mc' hook");
    }

    #[test]
    fn resolves_the_user_status_line_in_order() {
        let root = std::env::temp_dir().join(format!("mc-sl-res-{}", std::process::id()));
        let (cwd, home) = (root.join("proj"), root.join("home"));
        std::fs::create_dir_all(cwd.join(".claude")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let write = |p: std::path::PathBuf, v: &str| std::fs::write(p, v).unwrap();
        assert_eq!(user_statusline(&cwd, &home), None);
        write(
            home.join("settings.json"),
            r#"{"statusLine":{"command":"home.sh","padding":2}}"#,
        );
        assert_eq!(user_statusline(&cwd, &home).unwrap().command, "home.sh");
        write(cwd.join(".claude/settings.json"), "{broken");
        assert_eq!(
            user_statusline(&cwd, &home).unwrap().command,
            "home.sh",
            "malformed = none there"
        );
        write(
            cwd.join(".claude/settings.local.json"),
            r#"{"statusLine":{"command":"local.sh","refreshInterval":0}}"#,
        );
        let local = user_statusline(&cwd, &home).unwrap();
        assert_eq!(
            (local.command.as_str(), local.refresh.clone()),
            ("local.sh", Some(json!(0)))
        );
        write(
            cwd.join(".claude/settings.local.json"),
            r#"{"statusLine":{"command":"'/x/bungkus-mc' statusline"}}"#,
        );
        assert_eq!(
            user_statusline(&cwd, &home),
            None,
            "our own command is dropped"
        );
        let with_user = settings(Path::new("/mc"), Some(&local));
        let json: Value = serde_json::from_str(&with_user).unwrap();
        assert_eq!(
            json["statusLine"]["refreshInterval"], 0,
            "the user's refresh is kept"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
