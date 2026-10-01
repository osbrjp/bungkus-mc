//! Hook events from the agents to mc over a private unix socket
//! (ARCHITECTURE §4.3, SECURITY.md "Hook / status-line socket").
//!
//! The agent runs `bungkus-mc hook`, which trims the payload to the fields
//! below and sends one JSON line to mc's socket. Only these fields ever
//! leave the hook process: no `tool_input` beyond its description, no
//! `tool_response`, no prompt text.

pub(crate) mod hook;
pub(crate) mod server;
pub(crate) mod statusline;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::usage::Usage;
use crate::ui::sanitise::truncate;

/// Longest description, message or tool field kept (SECURITY.md).
const TEXT_MAX: usize = 200;

/// Longest session title kept (SECURITY.md).
const TITLE_MAX: usize = 80;

/// Longest id kept; real ids are UUIDs or short hex.
const ID_MAX: usize = 64;

/// Most background tasks kept per event.
const TASKS_MAX: usize = 64;

/// A background task as listed on Claude's `Stop`/`SubagentStop`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct BackgroundTask {
    /// Task (agent) id.
    pub id: String,
    /// `subagent` for subagents.
    pub kind: String,
    /// Subagent type, e.g. `general-purpose`.
    pub agent_type: String,
    /// `running`, `completed`, …
    pub status: String,
    /// What it was asked to do.
    pub description: String,
}

/// One hook event, trimmed to the fields mc uses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct HookEvent {
    /// `hook_event_name`, e.g. `PreToolUse`.
    pub name: String,
    /// The agent's own session id.
    pub session_id: Option<String>,
    /// Subagent id, on subagent events and their tool calls.
    pub agent_id: Option<String>,
    /// Subagent type.
    pub agent_type: Option<String>,
    /// Tool name on tool events.
    pub tool_name: Option<String>,
    /// Tool call id on tool events.
    pub tool_use_id: Option<String>,
    /// `tool_input.description`, the only `tool_input` field kept.
    pub tool_desc: Option<String>,
    /// `notification_type` (Claude) or `permission_request` (Codex).
    pub notify: Option<String>,
    /// `last_assistant_message`, cut short.
    pub last_message: Option<String>,
    /// `session_title` (Claude).
    pub session_title: Option<String>,
    /// `background_tasks` (Claude `Stop`/`SubagentStop`); `None` when absent.
    pub background_tasks: Option<Vec<BackgroundTask>>,
    /// `transcript_path`, kept opaque (read only by the Codex usage reader).
    pub transcript: Option<String>,
}

/// One line on the socket: mc's session id and either a trimmed hook
/// event or usage figures from the status line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Wire {
    /// `BUNGKUS_MC_SESSION` of the sending process.
    pub mc_session: String,
    /// The hook event (empty name for a usage line).
    #[serde(default)]
    pub event: HookEvent,
    /// Usage from `bungkus-mc statusline`.
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// Trims a raw hook payload to a [`HookEvent`]; never fails on odd input
/// (a field of the wrong type is simply absent).
///
/// # Arguments
///
/// * `raw` - The payload the agent passed on the hook's stdin.
#[must_use]
pub(crate) fn trim(raw: &Value) -> HookEvent {
    let text = |v: &Value, key: &str, max: usize| {
        v.get(key).and_then(Value::as_str).map(|s| truncate(s, max))
    };
    let tasks = raw
        .get("background_tasks")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .take(TASKS_MAX)
                .map(|t| BackgroundTask {
                    id: text(t, "id", ID_MAX).unwrap_or_default(),
                    kind: text(t, "type", ID_MAX).unwrap_or_default(),
                    agent_type: text(t, "agent_type", ID_MAX).unwrap_or_default(),
                    status: text(t, "status", ID_MAX).unwrap_or_default(),
                    description: text(t, "description", TEXT_MAX).unwrap_or_default(),
                })
                .collect()
        });
    let notify = text(raw, "notification_type", ID_MAX);
    let name = text(raw, "hook_event_name", ID_MAX).unwrap_or_default();
    HookEvent {
        notify: notify
            .or_else(|| (name == "PermissionRequest").then(|| "permission_request".into())),
        name,
        session_id: text(raw, "session_id", ID_MAX),
        agent_id: text(raw, "agent_id", ID_MAX),
        agent_type: text(raw, "agent_type", ID_MAX),
        tool_name: text(raw, "tool_name", ID_MAX),
        tool_use_id: text(raw, "tool_use_id", ID_MAX),
        tool_desc: raw.get("tool_input").and_then(|i| {
            text(i, "description", TEXT_MAX).or_else(|| text(i, "task_name", TEXT_MAX))
        }),
        last_message: text(raw, "last_assistant_message", TEXT_MAX),
        session_title: text(raw, "session_title", TITLE_MAX),
        background_tasks: tasks,
        transcript: text(raw, "transcript_path", 4096),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The recorded Claude Code 2.1.285 session (scrubbed).
    const SESSION: &str = include_str!("../agent/testdata/claude/session.jsonl");

    #[test]
    fn every_recorded_payload_trims_to_a_named_event() {
        for line in SESSION.lines() {
            let raw: Value = serde_json::from_str(line).unwrap();
            let event = trim(&raw);
            assert!(!event.name.is_empty(), "{line}");
            assert_eq!(
                event.session_id.as_deref(),
                Some("14184218-b8b3-4142-a27f-5f857334c378")
            );
            let wire = serde_json::to_string(&event).unwrap();
            assert!(
                !wire.contains("tool_response"),
                "tool_response must never be kept"
            );
            assert!(
                !wire.contains("Reply with the word hi"),
                "prompts must never be kept"
            );
        }
    }

    #[test]
    fn keeps_only_the_listed_fields() {
        let raw = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Agent",
            "tool_input": {"description": "say hi", "prompt": "secret prompt", "command": "rm -rf /"},
            "tool_response": {"stdout": "secret"},
            "notification_type": 7,
            "last_assistant_message": "x".repeat(500),
            "background_tasks": [{"id": "a", "type": "subagent", "status": "running", "extra": 1}],
        });
        let event = trim(&raw);
        assert_eq!(event.tool_desc.as_deref(), Some("say hi"));
        assert_eq!(event.notify, None, "wrong type is absent, not an error");
        assert_eq!(event.last_message.unwrap().chars().count(), TEXT_MAX);
        assert_eq!(event.background_tasks.unwrap()[0].kind, "subagent");
    }

    #[test]
    fn codex_permission_requests_count_as_notifications() {
        let event = trim(&serde_json::json!({"hook_event_name": "PermissionRequest"}));
        assert_eq!(event.notify.as_deref(), Some("permission_request"));
    }
}
