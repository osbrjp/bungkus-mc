//! `config.json`: reading the user's settings and writing back the three
//! the settings screen changes and the dragged pane widths.
//!
//! mc writes only the keys it owns (`workspace`, `theme`, `defaultAgent`,
//! `panes`) and keeps every other key, and the key order, exactly as the
//! user wrote it (ARCHITECTURE §7).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::agent::Kind;
use crate::ui::theme::{Background, ThemeChoice};

/// The settings mc reads from `config.json`; every key is optional.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent on/off keys of the config file, not a state machine"
)]
pub(crate) struct Config {
    /// The folder whose children are the projects; may start with `~`.
    pub workspace: Option<String>,
    /// Colour theme.
    pub theme: ThemeChoice,
    /// Whether mc paints its own background at TrueColor.
    pub background: Background,
    /// The agent the `n` picker preselects.
    pub default_agent: Kind,
    /// Commands and extra arguments per agent.
    pub agents: Agents,
    /// The chord that leaves INTERACT, e.g. `ctrl-^` (DESIGN §8.5).
    pub interact_exit: Option<String>,
    /// Whether mc captures the mouse.
    pub mouse: bool,
    /// How mc announces needs-you and failed sessions (DESIGN §9).
    pub notify: Notify,
    /// Process cleanup settings.
    pub cleanup: Cleanup,
    /// State glyph set; `auto` picks by the installed fonts.
    pub icons: crate::ui::icons::IconChoice,
    /// Whether spinners and the mascot move.
    pub motion: bool,
    /// Outer widths of the projects and sessions panes.
    pub panes: crate::ui::Widths,
    /// Saved workspaces for the `w` switcher, most recent first.
    pub workspaces: Vec<String>,
    /// Whether a Claude session joining a project where another session
    /// runs gets its own git worktree.
    pub worktrees: bool,
    /// Whether every session mc starts gets mc's built-in rules (session
    /// ids, branching, worktrees); a workspace's own instruction files
    /// apply either way.
    pub instructions: bool,
}

/// The `cleanup` config block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct Cleanup {
    /// Process names that start as `[keep]` in the quit dialog.
    pub keep: Vec<String>,
}

/// The `notify` setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Notify {
    /// Ring the terminal bell.
    #[default]
    Bell,
    /// Bell plus a desktop notification where the terminal supports one.
    Desktop,
    /// Stay quiet (the title and tallies still update).
    Off,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workspace: None,
            theme: ThemeChoice::default(),
            background: Background::default(),
            default_agent: Kind::default(),
            agents: Agents::default(),
            interact_exit: None,
            mouse: true,
            notify: Notify::default(),
            cleanup: Cleanup::default(),
            icons: crate::ui::icons::IconChoice::default(),
            motion: true,
            panes: crate::ui::Widths::default(),
            workspaces: Vec::new(),
            worktrees: true,
            instructions: true,
        }
    }
}

/// The `agents` config block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct Agents {
    /// Claude Code.
    pub claude: AgentCommand,
    /// Codex.
    pub codex: AgentCommand,
}

impl Agents {
    /// Returns the entry for `kind`.
    #[must_use]
    pub(crate) const fn get(&self, kind: Kind) -> &AgentCommand {
        match kind {
            Kind::Claude => &self.claude,
            Kind::Codex => &self.codex,
        }
    }
}

/// How to start one agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct AgentCommand {
    /// Command name or path; the agent's own name when unset.
    pub command: Option<String>,
    /// Extra arguments placed before mc's own.
    pub args: Vec<String>,
}

/// The values the wizard and the settings screen edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Settings {
    /// Absolute workspace folder.
    pub workspace: PathBuf,
    /// Colour theme.
    pub theme: ThemeChoice,
    /// The agent the `n` picker preselects.
    pub default_agent: Kind,
}

/// Why `config.json` could not be used.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    /// The file exists but could not be read or written.
    #[error("config.json: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not valid JSON or has a value of the wrong type.
    #[error("config.json is not valid: {0}")]
    Invalid(#[from] serde_json::Error),
    /// The file is valid JSON but not an object.
    #[error("config.json is not a JSON object")]
    NotObject,
}

/// The folder at a workspace's root that holds what is specific to that
/// workspace: `config.json` ([`Overrides`]) and the agents' instruction
/// files (ARCHITECTURE §7).
pub(crate) const WORKSPACE_DIR: &str = ".bungkus-mc";

/// What a workspace's own `.bungkus-mc/config.json` may set instead of the
/// global `config.json`; an unset key keeps the global value.
///
/// Only settings about the work in the workspace are here. What mc runs
/// (agent commands and arguments) is never read from a workspace folder,
/// which can come from elsewhere (SECURITY.md).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Overrides {
    /// The agent the `n` picker preselects.
    pub default_agent: Option<Kind>,
    /// Whether a Claude session joining a project where another session
    /// runs gets its own git worktree.
    pub worktrees: Option<bool>,
    /// How mc announces needs-you and failed sessions.
    pub notify: Option<Notify>,
    /// Process cleanup settings.
    pub cleanup: Option<Cleanup>,
}

/// Loads the overrides of the workspace at `root`; a missing file sets
/// nothing.
///
/// # Errors
///
/// * [`ConfigError::Io`] - the file exists but cannot be read.
/// * [`ConfigError::Invalid`] - the JSON is malformed or a value has the
///   wrong type; the caller uses the global values and says so.
pub(crate) fn load_overrides(root: &Path) -> Result<Overrides, ConfigError> {
    match std::fs::read(root.join(WORKSPACE_DIR).join("config.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Overrides::default()),
        Err(e) => Err(e.into()),
    }
}

/// Loads `config.json`; a missing file is the default config.
///
/// # Errors
///
/// * [`ConfigError::Io`] - the file exists but cannot be read.
/// * [`ConfigError::Invalid`] - the JSON is malformed or a value has the
///   wrong type; the caller uses defaults and shows a banner.
pub(crate) fn load(path: &Path) -> Result<Config, ConfigError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e.into()),
    }
}

/// Writes `settings` into `config.json`, keeping every other key and the
/// existing key order.
///
/// # Errors
///
/// * [`ConfigError::Invalid`] / [`ConfigError::NotObject`] - the existing
///   file is not a JSON object; it is left untouched rather than replaced.
/// * [`ConfigError::Io`] - reading or the atomic write failed.
pub(crate) fn save(path: &Path, settings: &Settings) -> Result<(), ConfigError> {
    edit(path, |root| {
        root.insert(
            "workspace".into(),
            settings.workspace.to_string_lossy().into(),
        );
        root.insert("theme".into(), serde_json::to_value(settings.theme)?);
        root.insert(
            "defaultAgent".into(),
            serde_json::to_value(settings.default_agent)?,
        );
        Ok(())
    })
}

/// Writes the dragged pane widths into `config.json` as `panes`, keeping
/// every other key and the key order.
///
/// # Errors
///
/// As [`save`].
pub(crate) fn save_widths(path: &Path, widths: crate::ui::Widths) -> Result<(), ConfigError> {
    edit(path, |root| {
        root.insert("panes".into(), serde_json::to_value(widths)?);
        Ok(())
    })
}

/// Writes the `w` switcher's saved workspaces into `config.json` as
/// `workspaces`, keeping every other key and the key order.
///
/// # Errors
///
/// As [`save`].
pub(crate) fn save_workspaces(path: &Path, workspaces: &[PathBuf]) -> Result<(), ConfigError> {
    edit(path, |root| {
        let list: Vec<Value> = workspaces
            .iter()
            .map(|w| Value::from(w.to_string_lossy().into_owned()))
            .collect();
        root.insert("workspaces".into(), Value::Array(list));
        Ok(())
    })
}

/// Reads `config.json` as an object (a missing file is empty), lets `f`
/// change it, and writes it back atomically.
///
/// # Errors
///
/// As [`save`], plus whatever `f` returns.
fn edit(
    path: &Path,
    f: impl FnOnce(&mut Map<String, Value>) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    let mut root = match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes)? {
            Value::Object(map) => map,
            _ => return Err(ConfigError::NotObject),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(e.into()),
    };
    f(&mut root)?;
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(root))?;
    bytes.push(b'\n');
    crate::store::write_atomic(path, &bytes)?;
    Ok(())
}

/// Expands a leading `~` and returns the path when it is absolute.
///
/// # Arguments
///
/// * `input` - A path as typed or stored (`~/Works`, `/abs/dir`).
/// * `home`  - The user's home directory, if known.
#[must_use]
pub(crate) fn expand(input: &str, home: Option<&Path>) -> Option<PathBuf> {
    let path = match (input.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(input),
    };
    path.is_absolute().then_some(path)
}

/// Returns `path` with the home directory shown as `~`.
#[must_use]
pub(crate) fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mc-config-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.json")
    }

    #[test]
    fn workspace_overrides_set_only_what_they_name() {
        let root = temp("overrides").parent().unwrap().to_path_buf();
        assert_eq!(load_overrides(&root).unwrap(), Overrides::default());
        let dir = root.join(WORKSPACE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        std::fs::write(
            &file,
            r#"{"defaultAgent":"codex","worktrees":false,"cleanup":{"keep":["vite"]},
               "agents":{"claude":{"command":"/tmp/evil"}}}"#,
        )
        .unwrap();
        let got = load_overrides(&root).unwrap();
        assert_eq!(got.default_agent, Some(Kind::Codex));
        assert_eq!(got.worktrees, Some(false));
        assert_eq!(got.notify, None, "unset: the global value stays");
        assert_eq!(got.cleanup.unwrap().keep, ["vite"]);
        std::fs::write(&file, r#"{"notify": 3}"#).unwrap();
        assert!(matches!(
            load_overrides(&root),
            Err(ConfigError::Invalid(_))
        ));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn loads_defaults_for_a_missing_file_and_rejects_bad_values() {
        let path = temp("load");
        assert_eq!(load(&path).unwrap(), Config::default());
        std::fs::write(
            &path,
            r#"{"theme":"light","defaultAgent":"codex","future":1}"#,
        )
        .unwrap();
        let config = load(&path).unwrap();
        assert_eq!(
            (config.theme, config.default_agent),
            (ThemeChoice::Light, Kind::Codex)
        );
        std::fs::write(&path, r#"{"theme":"purple"}"#).unwrap();
        assert!(matches!(load(&path), Err(ConfigError::Invalid(_))));
        std::fs::write(&path, "{not json").unwrap();
        assert!(matches!(load(&path), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn save_keeps_unknown_keys_and_their_order() {
        let path = temp("order");
        let original =
            r#"{"zeta":1,"theme":"dark","agents":{"codex":{"command":"codex"}},"alpha":[1,2]}"#;
        std::fs::write(&path, original).unwrap();
        let settings = Settings {
            workspace: PathBuf::from("/w"),
            theme: ThemeChoice::Light,
            default_agent: Kind::Codex,
        };
        save(&path, &settings).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let keys: Vec<String> = match serde_json::from_str::<Value>(&text).unwrap() {
            Value::Object(map) => map.keys().cloned().collect(),
            other => panic!("not an object: {other}"),
        };
        assert_eq!(
            keys,
            [
                "zeta",
                "theme",
                "agents",
                "alpha",
                "workspace",
                "defaultAgent"
            ]
        );
        let config = load(&path).unwrap();
        assert_eq!(config.workspace.as_deref(), Some("/w"));
        assert_eq!(
            (config.theme, config.default_agent),
            (ThemeChoice::Light, Kind::Codex)
        );
        assert!(
            text.contains(r#""command": "codex""#),
            "nested unknown keys survive"
        );
    }

    #[test]
    fn saves_pane_widths_next_to_the_other_keys() {
        let path = temp("panes");
        std::fs::write(&path, r#"{"zeta":1,"panes":{"projects":20}}"#).unwrap();
        let widths = crate::ui::Widths {
            projects: 30,
            sessions: 50,
        };
        save_widths(&path, widths).unwrap();
        let config = load(&path).unwrap();
        assert_eq!(config.panes, widths);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.find("zeta") < text.find("panes"), "{text}");
        std::fs::write(&path, r#"{"panes":{"sessions":60}}"#).unwrap();
        assert_eq!(
            load(&path).unwrap().panes,
            crate::ui::Widths {
                projects: 22,
                sessions: 60
            },
            "a missing width is the default"
        );
    }

    #[test]
    fn save_refuses_to_replace_a_malformed_file() {
        let path = temp("malformed");
        std::fs::write(&path, "{broken").unwrap();
        let settings = Settings {
            workspace: "/w".into(),
            theme: ThemeChoice::Auto,
            default_agent: Kind::Claude,
        };
        assert!(save(&path, &settings).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
        std::fs::write(&path, "[1]").unwrap();
        assert!(matches!(
            save(&path, &settings),
            Err(ConfigError::NotObject)
        ));
    }

    #[test]
    fn expands_and_abbreviates_the_home_directory() {
        let home = Path::new("/Users/me");
        let cases = [
            ("~", Some("/Users/me")),
            ("~/Works", Some("/Users/me/Works")),
            ("/abs", Some("/abs")),
            ("relative", None),
            ("~other/x", None),
        ];
        for (input, want) in cases {
            assert_eq!(
                expand(input, Some(home)),
                want.map(PathBuf::from),
                "{input}"
            );
        }
        assert_eq!(tilde(Path::new("/Users/me/Works"), Some(home)), "~/Works");
        assert_eq!(tilde(Path::new("/Users/me"), Some(home)), "~");
        assert_eq!(tilde(Path::new("/srv/w"), Some(home)), "/srv/w");
    }
}
