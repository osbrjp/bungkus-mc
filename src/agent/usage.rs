//! Usage figures: tokens, cost, context fill and plan limits
//! (ARCHITECTURE §6, DESIGN §6).
//!
//! Claude's come from its status-line payload, Codex's from the rollout
//! reader (M6). Every figure is optional: unknown renders as `-`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ui::sanitise::truncate;

/// Longest session name kept from a status line (SECURITY.md).
const NAME_MAX: usize = 80;

/// Most added folders kept from a status line, and the longest path.
const ADDED_DIRS_MAX: (usize, usize) = (32, 4096);

/// One plan-limit window, e.g. the 5-hour one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Window {
    /// `5h` or `7d`.
    pub label: String,
    /// Share of the window used, 0–100.
    pub used_pct: f64,
    /// When the window resets, in unix seconds.
    pub resets_at: Option<u64>,
}

/// Usage figures for one session as last reported.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Usage {
    /// The agent's own session name (Claude `session_name`).
    pub session_name: Option<String>,
    /// Input tokens, including cache reads and writes.
    pub input: Option<u64>,
    /// Output tokens.
    pub output: Option<u64>,
    /// Cache-read tokens of the last request.
    pub cache_read: Option<u64>,
    /// Cache-write tokens of the last request.
    pub cache_write: Option<u64>,
    /// Cost in USD at list price; `None` for Codex.
    pub cost_usd: Option<f64>,
    /// Context window fill, 0–100.
    pub ctx_pct: Option<f64>,
    /// Context window size in tokens.
    pub ctx_size: Option<u64>,
    /// Plan-limit windows; empty when the account has none.
    pub limits: Vec<Window>,
    /// Folders the session works in besides its own (Claude
    /// `workspace.added_dirs`: `/add-dir`, `--add-dir`); absolute paths,
    /// only ever compared with project folders, never shown.
    pub added_dirs: Vec<PathBuf>,
}

impl Usage {
    /// Returns the total tokens (input incl. cache + output), if known.
    #[must_use]
    pub(crate) fn tokens(&self) -> Option<u64> {
        match (self.input, self.output) {
            (None, None) => None,
            (i, o) => Some(i.unwrap_or(0) + o.unwrap_or(0)),
        }
    }
}

/// Extracts [`Usage`] from a Claude status-line payload; fields of the
/// wrong type or missing are simply `None` (ARCHITECTURE §6.2).
#[must_use]
pub(crate) fn from_statusline(raw: &Value) -> Usage {
    let num = |v: &Value, path: &[&str]| path.iter().try_fold(v, |v, k| v.get(k))?.as_f64();
    let int = |v: &Value, path: &[&str]| path.iter().try_fold(v, |v, k| v.get(k))?.as_u64();
    let cw = raw.get("context_window").unwrap_or(&Value::Null);
    let limits = [("five_hour", "5h"), ("seven_day", "7d")]
        .into_iter()
        .filter_map(|(key, label)| {
            let w = raw.get("rate_limits")?.get(key)?;
            Some(Window {
                label: label.to_owned(),
                used_pct: w.get("used_percentage")?.as_f64()?,
                resets_at: w.get("resets_at").and_then(Value::as_u64),
            })
        })
        .collect();
    Usage {
        session_name: raw
            .get("session_name")
            .and_then(Value::as_str)
            .map(|s| truncate(s, NAME_MAX)),
        input: int(cw, &["total_input_tokens"]),
        output: int(cw, &["total_output_tokens"]),
        cache_read: int(cw, &["current_usage", "cache_read_input_tokens"]),
        cache_write: int(cw, &["current_usage", "cache_creation_input_tokens"]),
        cost_usd: num(raw, &["cost", "total_cost_usd"]),
        ctx_pct: num(cw, &["used_percentage"]),
        ctx_size: int(cw, &["context_window_size"]),
        limits,
        added_dirs: added_dirs(raw),
    }
}

/// Returns `workspace.added_dirs` of a status-line payload: its absolute
/// path strings, [`ADDED_DIRS_MAX`] at most and none longer than that
/// limit's path length; anything else in the list is skipped.
fn added_dirs(raw: &Value) -> Vec<PathBuf> {
    let list = raw.get("workspace").and_then(|w| w.get("added_dirs"));
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|dir| dir.len() <= ADDED_DIRS_MAX.1)
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .take(ADDED_DIRS_MAX.0)
        .collect()
}

/// Formats a token count as `950`, `486k` or `1.2M`.
#[must_use]
pub(crate) fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{}k", n / 1_000),
        _ => {
            let tenths = n / 100_000;
            format!("{}.{}M", tenths / 10, tenths % 10)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/ipc/testdata")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn reads_the_recorded_status_line() {
        let u = from_statusline(&fixture("statusline.json"));
        assert_eq!(u.session_name.as_deref(), Some("sl-test"));
        assert_eq!((u.input, u.output), (Some(45_769), Some(140)));
        assert_eq!((u.cache_read, u.cache_write), (Some(30_007), Some(15_752)));
        assert_eq!((u.ctx_pct, u.ctx_size), (Some(23.0), Some(200_000)));
        assert!((u.cost_usd.unwrap() - 0.0352).abs() < 0.001);
        assert_eq!(u.limits.len(), 2);
        assert_eq!(
            (u.limits[0].label.as_str(), u.limits[0].used_pct),
            ("5h", 6.0)
        );
        assert_eq!(u.tokens(), Some(45_909));
    }

    #[test]
    fn before_the_first_reply_context_is_unknown() {
        let u = from_statusline(&fixture("statusline-before-reply.json"));
        assert_eq!(u.ctx_pct, None);
        assert_eq!(
            from_statusline(&serde_json::json!({"cost": "x"})),
            Usage::default()
        );
    }

    #[test]
    fn reads_added_folders_and_skips_what_is_not_an_absolute_path() {
        assert!(
            from_statusline(&fixture("statusline.json"))
                .added_dirs
                .is_empty()
        );
        let long = "/".repeat(ADDED_DIRS_MAX.1 + 1);
        let raw = serde_json::json!({"workspace": {"added_dirs":
            ["/w/api", "relative/web", 7, null, long, "/w/docs"]}});
        let dirs = from_statusline(&raw).added_dirs;
        assert_eq!(dirs, [PathBuf::from("/w/api"), PathBuf::from("/w/docs")]);
        for hostile in [
            serde_json::json!({"workspace": {"added_dirs": "/w/api"}}),
            serde_json::json!({"workspace": 3}),
            serde_json::json!({}),
        ] {
            assert!(from_statusline(&hostile).added_dirs.is_empty());
        }
        let many: Vec<String> = (0..100).map(|n| format!("/w/{n}")).collect();
        let raw = serde_json::json!({"workspace": {"added_dirs": many}});
        assert_eq!(from_statusline(&raw).added_dirs.len(), ADDED_DIRS_MAX.0);
    }

    #[test]
    fn formats_token_counts() {
        let cases = [
            (950, "950"),
            (486_000, "486k"),
            (1_234_567, "1.2M"),
            (0, "0"),
        ];
        for (n, want) in cases {
            assert_eq!(tokens(n), want);
        }
    }
}
