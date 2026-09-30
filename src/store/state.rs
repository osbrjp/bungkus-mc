//! `sessions.json`: every session mc started, kept for resume
//! (ARCHITECTURE §7).
//!
//! One JSON array, written atomically (0600) whenever the cards change.
//! It holds what a card shows and the agent's own session id for resume;
//! no prompts beyond the name, no transcripts, no plan limits.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::Kind;
use crate::agent::usage::Usage;

/// One stored session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Record {
    /// mc's id.
    pub id: String,
    /// Which agent.
    pub agent: Kind,
    /// The project folder it ran in.
    pub cwd: PathBuf,
    /// The agent's own session id, used to resume.
    pub agent_session_id: Option<String>,
    /// Display name.
    pub name: String,
    /// `wrapped`, `failed` or `stopped` (a running session is saved as
    /// `stopped`: it will not survive mc).
    pub status: String,
    /// Why it failed, when it did.
    pub reason: Option<String>,
    /// Start time, unix seconds.
    pub started_at: u64,
    /// End time, unix seconds.
    pub ended_at: Option<u64>,
    /// Tool calls seen.
    pub tool_calls: u32,
    /// Subagents seen.
    pub subagents: u32,
    /// Last usage figures (no limits: those are per account).
    pub usage: Option<Usage>,
}

/// Returns `$XDG_STATE_HOME/bungkus/mc/sessions.json`, or the same under
/// `~/.local/state`.
#[must_use]
pub(crate) fn state_file(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| var("HOME").map(|h| Path::new(&h).join(".local/state")))?;
    Some(base.join("bungkus/mc/sessions.json"))
}

/// Loads the stored sessions; a missing or unreadable file is empty.
#[must_use]
pub(crate) fn load(path: &Path) -> Vec<Record> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Saves the sessions atomically with mode 0600.
///
/// # Errors
///
/// The I/O error of the write; the old file is then untouched.
pub(crate) fn save(path: &Path, records: &[Record]) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(records).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    crate::store::write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_a_bad_file() {
        let dir = std::env::temp_dir().join(format!("mc-state-{}", std::process::id()));
        let path = dir.join("sessions.json");
        assert!(load(&path).is_empty());
        let record = Record {
            id: "m-1".into(),
            agent: Kind::Codex,
            cwd: "/w/p".into(),
            agent_session_id: Some("01a0f2cb-0303-7ae2-8133-1474fd08a780".into()),
            name: "fix it".into(),
            status: "wrapped".into(),
            started_at: 10,
            ..Record::default()
        };
        save(&path, std::slice::from_ref(&record)).unwrap();
        assert_eq!(load(&path), [record]);
        std::fs::write(&path, "{broken").unwrap();
        assert!(load(&path).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolves_the_state_file() {
        let env = |pairs: &'static [(&str, &str)]| {
            move |n: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == n)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(
            state_file(env(&[("XDG_STATE_HOME", "/s"), ("HOME", "/h")])),
            Some(PathBuf::from("/s/bungkus/mc/sessions.json"))
        );
        assert_eq!(
            state_file(env(&[("HOME", "/h")])),
            Some(PathBuf::from("/h/.local/state/bungkus/mc/sessions.json"))
        );
    }
}
