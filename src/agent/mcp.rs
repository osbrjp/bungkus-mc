//! Which MCP servers a session is configured with, read from the agents'
//! own config files when the session starts (ARCHITECTURE §5.5).
//!
//! Only server names leave this module, sanitised and capped: a server's
//! command, arguments and environment (which may hold API keys) are never
//! kept, shown or logged. The files are read once and never written. The
//! list is what is configured, not what connected: servers from plugins,
//! claude.ai connectors and managed settings are not in these files.

use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::agent::Kind;
use crate::ui::sanitise::sanitise;

/// 8 MiB, the hook payload cap (SECURITY.md "MCP config reader"); a larger
/// config file is not read.
const FILE_MAX: u64 = 8 * 1024 * 1024;

/// Longest server name shown, in characters.
const NAME_MAX: usize = 24;

/// Most servers kept per session.
const SERVERS_MAX: usize = 12;

/// Returns the names of the MCP servers configured for a `kind` session
/// in `cwd`, sorted, without duplicates.
///
/// Claude: `<cwd>/.mcp.json`, then the user and per-project entries of
/// `.claude.json` (in `$CLAUDE_CONFIG_DIR`, else the home directory), less
/// the servers that project disabled. Codex: the `[mcp_servers.<name>]`
/// tables of `config.toml` in `$CODEX_HOME`, else `~/.codex`.
///
/// # Arguments
///
/// * `kind` - Which agent the session runs.
/// * `cwd`  - The project folder the session starts in.
/// * `var`  - Looks up an environment variable by name.
///
/// # Returns
///
/// An empty list when no file exists, is readable, or parses.
#[must_use]
pub(crate) fn servers(kind: Kind, cwd: &Path, var: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let var = |name: &str| var(name).filter(|v| !v.is_empty());
    let home = var("HOME");
    let mut names = match kind {
        Kind::Claude => {
            let dir = var("CLAUDE_CONFIG_DIR").or(home);
            claude(cwd, dir.as_deref().map(Path::new))
        }
        Kind::Codex => crate::agent::codex_usage::codex_home(var)
            .and_then(|dir| read(&dir.join("config.toml")))
            .map(|bytes| codex(&String::from_utf8_lossy(&bytes)))
            .unwrap_or_default(),
    };
    names.sort_unstable();
    names.dedup();
    let mut shown: Vec<String> = names
        .iter()
        .map(|n| sanitise(n.trim(), NAME_MAX))
        .filter(|n| !n.is_empty())
        .collect();
    shown.sort_unstable();
    shown.truncate(SERVERS_MAX);
    shown
}

/// Returns `path`'s bytes, or `None` when it cannot be read or is larger
/// than [`FILE_MAX`].
fn read(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(FILE_MAX + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= FILE_MAX).then_some(bytes)
}

/// Returns the keys of the object at `key` in `value`.
fn keys(value: Option<&Value>, key: &str) -> Vec<String> {
    value
        .and_then(|v| v.get(key))
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

/// Returns Claude's server names for a session in `cwd`; `dir` holds
/// `.claude.json`.
fn claude(cwd: &Path, dir: Option<&Path>) -> Vec<String> {
    let json = |path: &Path| serde_json::from_slice::<Value>(&read(path)?).ok();
    let project = json(&cwd.join(".mcp.json"));
    let user = dir.and_then(|d| json(&d.join(".claude.json")));
    let local = user
        .as_ref()
        .and_then(|u| u.get("projects")?.get(cwd.to_str()?));
    let off: Vec<&str> = ["disabledMcpjsonServers", "disabledMcpServers"]
        .iter()
        .filter_map(|key| local?.get(key)?.as_array())
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut names = keys(project.as_ref(), "mcpServers");
    names.extend(keys(local, "mcpServers"));
    names.extend(keys(user.as_ref(), "mcpServers"));
    names.retain(|n| !off.contains(&n.as_str()));
    names
}

/// Returns the server names in Codex's `config.toml` text.
fn codex(toml: &str) -> Vec<String> {
    // ponytail: table headers only, so inline `mcp_servers = { … }` and
    // `enabled = false` are missed; a TOML crate if that ever matters.
    toml.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("[mcp_servers.")?;
            let name = match rest.strip_prefix(['"', '\'']) {
                Some(quoted) => quoted.split(['"', '\'']).next(),
                None => rest.split(['.', ']']).next(),
            }?;
            Some(name.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_claude_and_codex_server_names_only() {
        let root = std::env::temp_dir().join(format!("mc-mcp-{}", std::process::id()));
        let (cwd, home) = (root.join("proj"), root.join("home"));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let var = |name: &str| (name == "HOME").then(|| home.to_string_lossy().into_owned());
        assert!(servers(Kind::Claude, &cwd, var).is_empty(), "no files");
        assert!(servers(Kind::Codex, &cwd, var).is_empty(), "no files");

        std::fs::write(
            cwd.join(".mcp.json"),
            r#"{"mcpServers":{"slack":{"env":{"TOKEN":"xoxb-secret"}},"off":{},"\u001b[31mred":{}}}"#,
        )
        .unwrap();
        let user = serde_json::json!({
            "mcpServers": {"github": {"command": "gh-mcp"}, "slack": {}},
            "projects": {
                cwd.to_str().unwrap(): {
                    "mcpServers": {"playwright": {}},
                    "disabledMcpjsonServers": ["off"],
                },
                "/elsewhere": {"mcpServers": {"other": {}}},
            },
        });
        std::fs::write(home.join(".claude.json"), user.to_string()).unwrap();
        assert_eq!(
            servers(Kind::Claude, &cwd, var),
            ["github", "playwright", "red", "slack"]
        );
        std::fs::write(home.join(".claude.json"), "{broken").unwrap();
        assert_eq!(
            servers(Kind::Claude, &cwd, var),
            ["off", "red", "slack"],
            "a malformed user file gives nothing, the disabled list included"
        );

        std::fs::write(
            home.join(".codex/config.toml"),
            "model = \"x\"\n[mcp_servers.blender]\ncommand = \"b\"\n[mcp_servers.blender.env]\n\
             KEY = \"secret\"\n  [mcp_servers.\"my server\".tools.run]\n[other.mcp_servers.no]\n",
        )
        .unwrap();
        assert_eq!(servers(Kind::Codex, &cwd, var), ["blender", "my server"]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
