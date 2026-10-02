//! Which MCP servers a session has: the ones in the agents' own config
//! files, and the ones its tool calls name (ARCHITECTURE §5.5).
//!
//! Only server names leave this module, sanitised and capped: a server's
//! command, arguments and environment (which may hold API keys) are never
//! kept, shown or logged. The files are never written. They are parsed as
//! a stream that skips everything but the names, off the UI thread, and
//! only when [`stamp`] says a file changed.

use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::agent::Kind;
use crate::ui::sanitise::sanitise;

/// 8 MiB, the hook payload cap (SECURITY.md "MCP config reader"); a larger
/// config file is cut there and so does not parse.
const FILE_MAX: u64 = 8 * 1024 * 1024;

/// Longest server name shown, in characters.
const NAME_MAX: usize = 24;

/// Most servers kept per session.
pub(crate) const SERVERS_MAX: usize = 12;

/// The modification time and size of each config file of a session, as
/// [`stamp`] found them; `None` for a file that is absent.
pub(crate) type Stamp = [Option<(SystemTime, u64)>; 2];

/// The parts of Claude's `.mcp.json` and `.claude.json` that name servers;
/// every other key is skipped while parsing.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Scope {
    /// The servers' names.
    mcp_servers: Keys,
    /// `.mcp.json` servers the user turned off for the project.
    #[serde(deserialize_with = "lenient")]
    disabled_mcpjson_servers: Vec<String>,
    /// Other servers the user turned off for the project.
    #[serde(deserialize_with = "lenient")]
    disabled_mcp_servers: Vec<String>,
    /// `.claude.json` only: project folder to its own scope.
    projects: HashMap<String, Self>,
}

/// The keys of a JSON object, or none for `null`. The values (a server's
/// command, arguments and environment) are skipped without being parsed.
#[derive(Default)]
struct Keys(Vec<String>);

impl<'de> Deserialize<'de> for Keys {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeysVisitor;
        impl<'de> serde::de::Visitor<'de> for KeysVisitor {
            type Value = Keys;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object or null")
            }

            fn visit_unit<E>(self) -> Result<Keys, E> {
                Ok(Keys::default())
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Keys, A::Error> {
                let mut keys = Vec::new();
                while let Some((key, IgnoredAny)) = map.next_entry()? {
                    keys.push(key);
                }
                Ok(Keys(keys))
            }
        }
        deserializer.deserialize_any(KeysVisitor)
    }
}

/// Deserialises a `T`, or its default when the value has another shape, so
/// one odd value in a config file does not lose the rest of it.
fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either<T> {
        Fits(T),
        Other(IgnoredAny),
    }
    Ok(match Either::deserialize(deserializer)? {
        Either::Fits(value) => value,
        Either::Other(_) => T::default(),
    })
}

/// Returns whether `a` and `b` name one server: Claude writes a server's
/// name in tool names with every other character than letters, digits and
/// `-` as `_`.
#[must_use]
pub(crate) fn same(a: &str, b: &str) -> bool {
    let norm = |c: char| {
        if c.is_ascii_alphanumeric() || c == '-' {
            c
        } else {
            '_'
        }
    };
    a.chars().map(norm).eq(b.chars().map(norm))
}

/// Returns the config files of a `kind` session in `cwd`. Claude:
/// `<cwd>/.mcp.json` and `.claude.json` (in `$CLAUDE_CONFIG_DIR`, else the
/// home directory). Codex: `config.toml` in `$CODEX_HOME`, else `~/.codex`.
fn files(kind: Kind, cwd: &Path, var: impl Fn(&str) -> Option<String>) -> [Option<PathBuf>; 2] {
    let var = |name: &str| var(name).filter(|v| !v.is_empty());
    match kind {
        Kind::Claude => [
            Some(cwd.join(".mcp.json")),
            var("CLAUDE_CONFIG_DIR")
                .or_else(|| var("HOME"))
                .map(|dir| Path::new(&dir).join(".claude.json")),
        ],
        Kind::Codex => [
            crate::agent::codex_usage::codex_home(var).map(|dir| dir.join("config.toml")),
            None,
        ],
    }
}

/// Returns the [`Stamp`] of the config files of a `kind` session in `cwd`:
/// two `stat` calls, so a caller can skip [`servers`] while nothing
/// changed.
#[must_use]
pub(crate) fn stamp(kind: Kind, cwd: &Path, var: impl Fn(&str) -> Option<String>) -> Stamp {
    files(kind, cwd, var).map(|file| {
        let meta = std::fs::metadata(file?).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    })
}

/// Returns the names of the MCP servers configured for a `kind` session
/// in `cwd`, sorted, without duplicates.
///
/// Claude: the servers of `.mcp.json`, of the user, and of the project's
/// entry in `.claude.json`, less the ones that entry disabled. Codex: the
/// `[mcp_servers.<name>]` tables of `config.toml`.
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
    let [first, second] = files(kind, cwd, var);
    let mut names = match kind {
        Kind::Claude => claude(cwd, first.as_deref(), second.as_deref()),
        Kind::Codex => first.as_deref().map(codex).unwrap_or_default(),
    };
    names.sort_unstable();
    names.dedup();
    let mut shown: Vec<String> = names.iter().filter_map(|n| clean(n)).collect();
    shown.sort_unstable();
    shown.truncate(SERVERS_MAX);
    shown
}

/// Returns the MCP server a hook's `tool_name` belongs to
/// (`mcp__<server>__<tool>`), without the `claude_ai_` or `plugin_` prefix
/// Claude gives connector and plugin servers.
#[must_use]
pub(crate) fn used(tool_name: &str) -> Option<String> {
    let (server, _) = tool_name.strip_prefix("mcp__")?.split_once("__")?;
    let server = ["claude_ai_", "plugin_"]
        .iter()
        .find_map(|prefix| server.strip_prefix(prefix))
        .unwrap_or(server);
    clean(server)
}

/// Returns `name` as shown: sanitised and cut to [`NAME_MAX`]; `None` when
/// nothing is left.
fn clean(name: &str) -> Option<String> {
    Some(sanitise(name.trim(), NAME_MAX)).filter(|n| !n.is_empty())
}

/// Opens `path` for reading; `None` when it is larger than [`FILE_MAX`]
/// (and cut there should it grow meanwhile).
fn open(path: &Path) -> Option<impl Read> {
    let file = std::fs::File::open(path).ok()?;
    let small = file.metadata().ok()?.len() <= FILE_MAX;
    small.then(|| BufReader::new(file.take(FILE_MAX)))
}

/// Returns Claude's server names for a session in `cwd` from `.mcp.json`
/// (`project`) and `.claude.json` (`user`).
fn claude(cwd: &Path, project: Option<&Path>, user: Option<&Path>) -> Vec<String> {
    let scope = |path: Option<&Path>| -> Scope {
        path.and_then(open)
            .and_then(|file| serde_json::from_reader(file).ok())
            .unwrap_or_default()
    };
    let (project, mut user) = (scope(project), scope(user));
    let local = cwd
        .to_str()
        .and_then(|cwd| user.projects.remove(cwd))
        .unwrap_or_default();
    let off = [local.disabled_mcpjson_servers, local.disabled_mcp_servers].concat();
    [project.mcp_servers, local.mcp_servers, user.mcp_servers]
        .into_iter()
        .flat_map(|keys| keys.0)
        .filter(|name| !off.contains(name))
        .collect()
}

/// Returns the server names in Codex's `config.toml`.
fn codex(path: &Path) -> Vec<String> {
    let mut bytes = Vec::new();
    if open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes).ok())
        .is_none()
    {
        return Vec::new();
    }
    let toml = String::from_utf8_lossy(&bytes);
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
        let var = |name: &str| match name {
            "HOME" => Some(home.to_string_lossy().into_owned()),
            "CLAUDE_CONFIG_DIR" => Some(String::new()),
            _ => None,
        };
        assert!(servers(Kind::Claude, &cwd, var).is_empty(), "no files");
        assert!(servers(Kind::Codex, &cwd, var).is_empty(), "no files");
        assert_eq!(stamp(Kind::Claude, &cwd, var), [None, None]);

        std::fs::write(
            cwd.join(".mcp.json"),
            r#"{"mcpServers":{"slack":{"env":{"TOKEN":"xoxb-secret"}},"off":{},"\u001b[31mred":{}}}"#,
        )
        .unwrap();
        let user = serde_json::json!({
            "history": [{"big": "ignored"}],
            "mcpServers": {"github": {"command": "gh-mcp"}, "slack": {}},
            "disabledMcpServers": [1, null],
            "projects": {
                "/odd": {"mcpServers": null, "disabledMcpjsonServers": "all"},
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
        let before = stamp(Kind::Claude, &cwd, var);
        assert!(before.iter().all(Option::is_some));
        std::fs::write(home.join(".claude.json"), "{broken").unwrap();
        assert_ne!(stamp(Kind::Claude, &cwd, var), before, "a change shows");
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

    #[test]
    fn names_the_server_of_an_mcp_tool_call() {
        let cases = [
            ("mcp__blender__get_scene_info", Some("blender")),
            ("mcp__claude_ai_Figma__get_screenshot", Some("Figma")),
            (
                "mcp__plugin_slack_slack__slack_send_message",
                Some("slack_slack"),
            ),
            ("mcp__cut_short…", None),
            ("Bash", None),
            ("mcp____x", None),
        ];
        for (tool, want) in cases {
            assert_eq!(used(tool).as_deref(), want, "{tool}");
        }
    }
}
