//! Session cards: what mc knows about each session it started, and the
//! hook-event state machine (ARCHITECTURE §4.3, DESIGN §5.3).
//!
//! A card outlives its process: a wrapped, failed or stopped session stays
//! listed with its last screen. While the process runs, hook events move
//! it between working, your turn and needs you; the process exit decides
//! wrapped, failed or stopped.

use std::cmp::Reverse;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::agent::Kind;
use crate::agent::usage::Usage;
use crate::ipc::HookEvent;
use crate::proc::{Descendants, Proc};
use crate::store::state::Record;
use crate::term::SessionId;
use crate::term::session::Session;
use crate::ui::sanitise::sanitise;

/// Longest session name shown, in characters (DESIGN §5.2, ARCHITECTURE §5.3).
const NAME_MAX: usize = 80;

/// Grace between SIGTERM and SIGKILL when stopping (ARCHITECTURE §3.2).
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(3);

/// How long a hooked session may stay silent before its card says
/// "output only" (ARCHITECTURE §4.3).
pub(crate) const HOOK_GRACE: Duration = Duration::from_secs(10);

/// Notification types that mean the agent waits for the user
/// (ARCHITECTURE §4.3); `idle_prompt` is deliberately not one.
const NEEDS_YOU: [&str; 4] = [
    "permission_prompt",
    "agent_needs_input",
    "elicitation_dialog",
    "permission_request",
];

/// Returns whether a tool call starts a subagent: Claude's `Agent`, or a
/// Codex tool ending in `spawn_agent` (0.159.2 names it
/// `collaborationspawn_agent`).
fn spawns(tool: &str) -> bool {
    tool == "Agent" || tool.ends_with("spawn_agent")
}

/// A session's state (DESIGN §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum State {
    /// The agent is working (a turn or a subagent is active).
    Working,
    /// The agent finished its turn and waits for the next prompt.
    YourTurn,
    /// The agent is blocked on the user (permission, question).
    NeedsYou,
    /// The process exited non-zero, or could not start; the reason line.
    Failed(String),
    /// The user stopped it (`x` or quit), whatever its exit code.
    Stopped,
    /// The process exited 0.
    Wrapped,
}

impl State {
    /// Returns the card order rank (DESIGN §5.2): needs you, failed,
    /// working, your turn, stopped, wrapped.
    pub(crate) const fn rank(&self) -> u8 {
        match self {
            Self::NeedsYou => 0,
            Self::Failed(_) => 1,
            Self::Working => 2,
            Self::YourTurn => 3,
            Self::Stopped => 4,
            Self::Wrapped => 5,
        }
    }
}

/// One subagent of a session (a flat list, DESIGN §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Subagent {
    /// The agent's id.
    pub id: String,
    /// What it was asked to do (the spawning tool's description).
    pub description: String,
    /// When it started.
    pub started: Instant,
    /// When it finished; `None` while running.
    pub ended: Option<Instant>,
}

/// One session mc started.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts about one session (killed, hooked, prompted, turn done)"
)]
pub(crate) struct Card {
    /// mc's id (Claude's `--session-id`).
    pub id: SessionId,
    /// Which agent.
    pub kind: Kind,
    /// The project folder it runs in.
    pub project: PathBuf,
    /// Display name, sanitised.
    pub name: String,
    /// Current state.
    pub state: State,
    /// When it was started.
    pub started: Instant,
    /// When its process ended.
    pub ended: Option<Instant>,
    /// When the user asked it to stop (SIGTERM sent).
    pub stop_requested: Option<Instant>,
    /// Whether SIGKILL was sent after the grace period.
    pub killed: bool,
    /// The PTY and emulator; `None` when the process could not start.
    pub pty: Option<Session>,
    /// Whether this session was started with mc's hooks.
    pub hooked: bool,
    /// How many hook events arrived.
    pub events: u32,
    /// The agent's own session id, once bound.
    pub agent_session: Option<String>,
    /// The main agent's current tool.
    pub tool: Option<String>,
    /// The main agent's last reply, cut short.
    pub last_message: Option<String>,
    /// Subagents, oldest first.
    pub subagents: Vec<Subagent>,
    /// Descriptions of spawn calls waiting for their `SubagentStart`.
    pending: VecDeque<String>,
    /// Whether the main agent's turn ended (`Stop` seen since the last prompt).
    main_done: bool,
    /// Tool calls seen, for the wrapped line.
    pub tool_calls: u32,
    /// The last usage report.
    pub usage: Option<Usage>,
    /// Stops the Codex usage reader when dropped.
    pub rollout_stop: Option<std::sync::mpsc::Sender<()>>,
    /// The agent's pid, the root of its descendant tree.
    pub pid: Option<i32>,
    /// Processes observed under the agent (ARCHITECTURE §3.3).
    pub descendants: Descendants,
    /// Descendants the user chose to stop, signalled after the session exits.
    pub stop_plan: Vec<Proc>,
    /// When the plan's SIGTERM went out; SIGKILL follows [`STOP_GRACE`] later.
    pub plan_termed: Option<Instant>,
    /// Subagents counted in an earlier run (restored from `sessions.json`).
    pub restored_subagents: u32,
    /// Where a quick session moves once it has exited (issue #46).
    pub move_to: Option<PathBuf>,
    /// Whether the conversation has a prompt (so the agent saved it and it
    /// can be resumed); restored and resumed sessions count as prompted.
    pub prompted: bool,
    /// The git worktree it runs in (`claude --worktree <name>`, under the
    /// project's `.claude/worktrees/`); a resume goes back into it.
    pub worktree: Option<String>,
    /// Names of the MCP servers configured for it when it started
    /// ([`crate::agent::mcp::servers`]).
    pub mcp: Vec<String>,
}

impl Card {
    /// Returns the folder the session works in: its worktree under the
    /// project's `.claude/worktrees/` when it has one, else the project.
    #[must_use]
    pub(crate) fn folder(&self) -> PathBuf {
        self.worktree.as_ref().map_or_else(
            || self.project.clone(),
            |name| self.project.join(".claude/worktrees").join(name),
        )
    }

    /// Creates a card for a new session.
    ///
    /// The name falls back to the prompt's first line, then `untitled`
    /// (ARCHITECTURE §5.3). A hooked Claude session starts as "your turn";
    /// one without hooks as "working", since nothing will say otherwise.
    #[must_use]
    pub(crate) fn new(
        id: SessionId,
        kind: Kind,
        project: PathBuf,
        name: Option<&str>,
        prompt: Option<&str>,
        started: Instant,
    ) -> Self {
        let name = name
            .filter(|n| !n.trim().is_empty())
            .or_else(|| {
                prompt
                    .and_then(|p| p.lines().next())
                    .filter(|l| !l.trim().is_empty())
            })
            .map_or_else(|| "untitled".to_owned(), |n| sanitise(n.trim(), NAME_MAX));
        Self {
            id,
            kind,
            project,
            name,
            state: State::Working,
            started,
            ended: None,
            stop_requested: None,
            killed: false,
            pty: None,
            hooked: false,
            events: 0,
            agent_session: None,
            tool: None,
            last_message: None,
            subagents: Vec::new(),
            pending: VecDeque::new(),
            main_done: false,
            tool_calls: 0,
            usage: None,
            rollout_stop: None,
            pid: None,
            descendants: Descendants::default(),
            stop_plan: Vec::new(),
            plan_termed: None,
            restored_subagents: 0,
            move_to: None,
            prompted: false,
            worktree: None,
            mcp: Vec::new(),
        }
    }

    /// Returns the stored form of this card (ARCHITECTURE §7). A running
    /// session is stored as `stopped`: it does not outlive mc.
    #[must_use]
    pub(crate) fn to_record(&self, now: Instant, unix_now: u64) -> Record {
        let unix =
            |at: Instant| unix_now.saturating_sub(now.saturating_duration_since(at).as_secs());
        let (status, reason) = match &self.state {
            State::Failed(reason) => ("failed", Some(reason.clone())),
            State::Wrapped => ("wrapped", None),
            State::Working | State::YourTurn | State::NeedsYou | State::Stopped => {
                ("stopped", None)
            }
        };
        let usage = self.usage.clone().map(|u| Usage {
            limits: Vec::new(),
            ..u
        });
        Record {
            id: self.id.0.hyphenated().to_string(),
            agent: self.kind,
            cwd: self.project.clone(),
            agent_session_id: self.agent_session.clone(),
            name: self.name.clone(),
            status: status.to_owned(),
            reason,
            started_at: unix(self.started),
            ended_at: Some(unix(self.ended.unwrap_or(now))),
            tool_calls: self.tool_calls,
            subagents: u32::try_from(self.subagents.len()).unwrap_or(u32::MAX)
                + self.restored_subagents,
            usage,
            worktree: self.worktree.clone(),
        }
    }

    /// Rebuilds a finished card from a stored record; `None` when its id is
    /// not a UUID.
    #[must_use]
    pub(crate) fn from_record(record: &Record, now: Instant, unix_now: u64) -> Option<Self> {
        let id = SessionId(uuid::Uuid::parse_str(&record.id).ok()?);
        let at = |unix: u64| {
            now.checked_sub(Duration::from_secs(unix_now.saturating_sub(unix)))
                .unwrap_or(now)
        };
        let mut card = Self::new(
            id,
            record.agent,
            record.cwd.clone(),
            Some(&record.name),
            None,
            at(record.started_at),
        );
        card.state = match record.status.as_str() {
            "failed" => State::Failed(record.reason.clone().unwrap_or_else(|| "failed".into())),
            "wrapped" => State::Wrapped,
            _ => State::Stopped,
        };
        card.ended = Some(at(record.ended_at.unwrap_or(record.started_at)));
        card.agent_session.clone_from(&record.agent_session_id);
        card.tool_calls = record.tool_calls;
        card.restored_subagents = record.subagents;
        card.usage.clone_from(&record.usage);
        card.prompted = true;
        card.worktree.clone_from(&record.worktree);
        Some(card)
    }

    /// Returns the agent session id to resume, when it is a valid UUID
    /// (SECURITY.md: anything else would be read as a name or a picker).
    /// A Claude session without hooks still has mc's own id, which it was
    /// started with; a Codex session without hooks cannot be resumed.
    #[must_use]
    pub(crate) fn resume_id(&self) -> Option<String> {
        let own = self.id.0.hyphenated().to_string();
        let id = match (self.kind, &self.agent_session) {
            (_, Some(bound)) => bound.as_str(),
            (Kind::Claude, None) => own.as_str(),
            (Kind::Codex, None) => return None,
        };
        uuid::Uuid::parse_str(id)
            .ok()
            .map(|u| u.hyphenated().to_string())
    }

    /// Marks the session as started with hooks: it now waits for them.
    pub(crate) fn expect_hooks(&mut self) {
        self.hooked = true;
        self.state = State::YourTurn;
    }

    /// Returns whether the process is still running.
    #[must_use]
    pub(crate) fn running(&self) -> bool {
        matches!(
            self.state,
            State::Working | State::YourTurn | State::NeedsYou
        )
    }

    /// Returns whether the card should say "output only": no hooks, or a
    /// hooked session that stayed silent past [`HOOK_GRACE`].
    #[must_use]
    pub(crate) fn output_only(&self, now: Instant) -> bool {
        self.running()
            && self.events == 0
            && (!self.hooked || now.saturating_duration_since(self.started) >= HOOK_GRACE)
    }

    /// Returns the number of running subagents.
    #[must_use]
    pub(crate) fn running_subagents(&self) -> usize {
        self.subagents.iter().filter(|s| s.ended.is_none()).count()
    }

    /// Records the process exit: stopped when the user asked, else wrapped
    /// on 0 and failed otherwise, with the screen's last line as reason.
    pub(crate) fn exited(&mut self, code: Option<u32>, now: Instant) {
        self.ended = Some(now);
        self.rollout_stop = None;
        for sub in self.subagents.iter_mut().filter(|s| s.ended.is_none()) {
            sub.ended = Some(now);
        }
        self.state = if self.stop_requested.is_some() {
            State::Stopped
        } else if code == Some(0) {
            State::Wrapped
        } else {
            let code = code.map_or_else(|| "signal".to_owned(), |c| format!("exit {c}"));
            let last = self
                .pty
                .as_ref()
                .map(Session::last_line)
                .unwrap_or_default();
            let reason = if last.is_empty() {
                code
            } else {
                format!("{code} · {last}")
            };
            State::Failed(sanitise(&reason, NAME_MAX))
        };
    }

    /// Applies a usage report: its figures, and the session's own name
    /// when it has one (ARCHITECTURE §5.3). Reports after the process
    /// ended are ignored.
    pub(crate) fn report(&mut self, usage: Usage) {
        if !self.running() {
            return;
        }
        if let Some(name) = usage
            .session_name
            .as_deref()
            .filter(|n| !n.trim().is_empty())
        {
            self.name = sanitise(name.trim(), NAME_MAX);
        }
        self.events = self.events.saturating_add(1);
        self.usage = Some(usage);
    }

    /// Applies one hook event (ARCHITECTURE §4.3).
    ///
    /// Events for another agent session (a nested `claude` inheriting the
    /// environment) are ignored once this card is bound to its session id;
    /// events after the process ended change nothing.
    pub(crate) fn reduce(&mut self, event: &HookEvent, now: Instant) {
        if !self.running() {
            return;
        }
        if let Some(sid) = &event.session_id {
            match &self.agent_session {
                Some(bound) if bound != sid => return,
                Some(_) => {}
                None => self.agent_session = Some(sid.clone()),
            }
        }
        self.events = self.events.saturating_add(1);
        if let Some(title) = event
            .session_title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
        {
            self.name = sanitise(title.trim(), NAME_MAX);
        }
        let main = event.agent_id.is_none();
        match event.name.as_str() {
            "SessionStart" => self.state = State::YourTurn,
            "UserPromptSubmit" => {
                self.prompted = true;
                self.main_done = false;
                self.tool = None;
                self.state = State::Working;
            }
            "PreToolUse" | "PostToolUse" => {
                if main {
                    self.main_done = false;
                    self.tool.clone_from(&event.tool_name);
                }
                if event.name == "PreToolUse" {
                    self.tool_calls = self.tool_calls.saturating_add(1);
                    let spawn = event.tool_name.as_deref().is_some_and(spawns);
                    if spawn {
                        self.pending
                            .push_back(event.tool_desc.clone().unwrap_or_default());
                    }
                }
                self.state = State::Working;
            }
            "SubagentStart" => {
                if let Some(id) = &event.agent_id {
                    let description = self
                        .pending
                        .pop_front()
                        .filter(|d| !d.is_empty())
                        .or_else(|| event.agent_type.clone())
                        .unwrap_or_default();
                    self.subagents.push(Subagent {
                        id: id.clone(),
                        description: sanitise(&description, NAME_MAX),
                        started: now,
                        ended: None,
                    });
                }
                self.state = State::Working;
            }
            "SubagentStop" => {
                self.reconcile(event, now);
                if let Some(sub) = self
                    .subagents
                    .iter_mut()
                    .find(|s| Some(&s.id) == event.agent_id.as_ref())
                {
                    sub.ended.get_or_insert(now);
                }
                if self.main_done && self.running_subagents() == 0 {
                    self.state = State::YourTurn;
                }
            }
            "Notification" | "PermissionRequest" => {
                if event
                    .notify
                    .as_deref()
                    .is_some_and(|n| NEEDS_YOU.contains(&n))
                {
                    self.state = State::NeedsYou;
                }
            }
            "Stop" => {
                self.main_done = true;
                self.tool = None;
                self.last_message.clone_from(&event.last_message);
                self.reconcile(event, now);
                self.state = if self.running_subagents() > 0 {
                    State::Working
                } else {
                    State::YourTurn
                };
            }
            _ => {}
        }
    }

    /// Reconciles the subagent list with `background_tasks`, which is
    /// authoritative when present: listed running tasks are running (and
    /// added when unknown), every other known subagent has finished.
    fn reconcile(&mut self, event: &HookEvent, now: Instant) {
        let Some(tasks) = &event.background_tasks else {
            return;
        };
        let running: Vec<_> = tasks
            .iter()
            .filter(|t| t.kind == "subagent" && t.status == "running")
            .collect();
        for sub in &mut self.subagents {
            let listed = running.iter().any(|t| t.id == sub.id);
            if listed {
                sub.ended = None;
            } else {
                sub.ended.get_or_insert(now);
            }
        }
        for task in running {
            if !self.subagents.iter().any(|s| s.id == task.id) {
                self.subagents.push(Subagent {
                    id: task.id.clone(),
                    description: sanitise(&task.description, NAME_MAX),
                    started: now,
                    ended: None,
                });
            }
        }
    }
}

/// Returns the indices of `cards` in `project`, in display order: by state
/// (DESIGN §5.2), then most recently started first.
#[must_use]
pub(crate) fn order(cards: &[Card], project: &std::path::Path) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..cards.len())
        .filter(|&i| cards[i].project.as_path() == project)
        .collect();
    idx.sort_by_key(|&i| (cards[i].state.rank(), Reverse(cards[i].started)));
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: Option<&str>, prompt: Option<&str>) -> Card {
        Card::new(
            SessionId::new(),
            Kind::Claude,
            "/p".into(),
            name,
            prompt,
            Instant::now(),
        )
    }

    fn event(line: &str) -> HookEvent {
        crate::ipc::trim(&serde_json::from_str(line).unwrap())
    }

    #[test]
    fn names_fall_back_from_picker_to_prompt_to_untitled() {
        let cases = [
            (Some("fix it"), Some("prompt"), "fix it"),
            (Some("  "), Some("first line\nsecond"), "first line"),
            (None, None, "untitled"),
            (None, Some("\u{1b}]0;x\u{7}evil"), "evil"),
        ];
        for (name, prompt, want) in cases {
            assert_eq!(card(name, prompt).name, want, "{name:?} {prompt:?}");
        }
    }

    #[test]
    fn exit_codes_map_to_states_and_user_stops_win() {
        let now = Instant::now();
        let mut ok = card(None, None);
        ok.exited(Some(0), now);
        assert_eq!(ok.state, State::Wrapped);
        let mut bad = card(None, None);
        bad.exited(Some(1), now);
        assert_eq!(bad.state, State::Failed("exit 1".into()));
        let mut stopped = card(None, None);
        stopped.stop_requested = Some(now);
        stopped.exited(Some(143), now);
        assert_eq!(stopped.state, State::Stopped);
    }

    #[test]
    fn the_recorded_claude_session_walks_the_decided_states() {
        use State::{NeedsYou, Working, YourTurn};
        let lines: Vec<&str> = include_str!("../agent/testdata/claude/session.jsonl")
            .lines()
            .collect();
        let want = [
            YourTurn, Working, Working, Working, Working, Working, Working, Working, YourTurn,
            Working, YourTurn, YourTurn, YourTurn, Working, Working, NeedsYou, Working, YourTurn,
            YourTurn, YourTurn,
        ];
        assert_eq!(lines.len(), want.len());
        let mut c = card(Some("picker name"), None);
        c.expect_hooks();
        let now = Instant::now();
        for (i, (line, want)) in lines.iter().zip(want).enumerate() {
            c.reduce(&event(line), now);
            assert_eq!(c.state, want, "after line {i}: {line:.80}");
            if i == 5 {
                assert_eq!(c.running_subagents(), 1);
                assert_eq!(c.subagents[0].description, "say hi");
            }
        }
        assert_eq!(
            c.name, "rec-test",
            "session_title wins over the picker name"
        );
        assert_eq!(
            c.subagents.len(),
            1,
            "internal helpers with no type are not listed"
        );
        assert_eq!(c.running_subagents(), 0);
        assert_eq!(c.tool_calls, 3);
    }

    #[test]
    fn the_recorded_codex_session_walks_the_decided_states() {
        use State::{Working, YourTurn};
        let lines: Vec<&str> = include_str!("../agent/testdata/codex/session.jsonl")
            .lines()
            .collect();
        let want = [
            YourTurn, Working, Working, Working, Working, Working, Working, Working, YourTurn,
            YourTurn, YourTurn,
        ];
        assert_eq!(lines.len(), want.len());
        let mut c = Card::new(
            SessionId::new(),
            Kind::Codex,
            "/p".into(),
            None,
            None,
            Instant::now(),
        );
        c.expect_hooks();
        let now = Instant::now();
        for (i, (line, want)) in lines.iter().zip(want).enumerate() {
            c.reduce(&event(line), now);
            assert_eq!(c.state, want, "after line {i}: {line:.80}");
        }
        assert_eq!(c.subagents.len(), 1);
        assert_eq!(
            c.subagents[0].description, "say_hi",
            "Codex task_name as description"
        );
        assert_eq!(
            c.agent_session.as_deref(),
            Some("01a0f2cb-0303-7ae2-8133-1474fd08a780")
        );
    }

    #[test]
    fn idle_prompts_foreign_sessions_and_unknown_events_change_nothing() {
        let mut c = card(None, None);
        c.expect_hooks();
        let now = Instant::now();
        c.reduce(
            &event(r#"{"hook_event_name":"UserPromptSubmit","session_id":"a"}"#),
            now,
        );
        c.reduce(
            &event(r#"{"hook_event_name":"Stop","session_id":"a","background_tasks":[]}"#),
            now,
        );
        assert_eq!(c.state, State::YourTurn);
        let ignored = [
            r#"{"hook_event_name":"Notification","session_id":"a","notification_type":"idle_prompt"}"#,
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"b"}"#,
            r#"{"hook_event_name":"SomethingNew","session_id":"a"}"#,
        ];
        for line in ignored {
            c.reduce(&event(line), now);
            assert_eq!(c.state, State::YourTurn, "{line}");
        }
    }

    #[test]
    fn stop_with_running_background_tasks_keeps_working() {
        let mut c = card(None, None);
        c.expect_hooks();
        let now = Instant::now();
        let stop = r#"{"hook_event_name":"Stop","background_tasks":[
            {"id":"x1","type":"subagent","status":"running","description":"audit"}]}"#;
        c.reduce(&event(stop), now);
        assert_eq!(
            (c.state.clone(), c.running_subagents()),
            (State::Working, 1)
        );
        c.reduce(
            &event(r#"{"hook_event_name":"SubagentStop","agent_id":"x1"}"#),
            now,
        );
        assert_eq!(
            c.state,
            State::YourTurn,
            "last child done after main stopped"
        );
    }

    #[test]
    fn records_round_trip_and_running_sessions_are_stored_as_stopped() {
        let now = Instant::now();
        let mut c = card(Some("fix it"), None);
        c.started = now.checked_sub(Duration::from_mins(10)).unwrap();
        c.exited(Some(2), now);
        let record = c.to_record(now, 10_000);
        assert_eq!(
            (record.status.as_str(), record.started_at),
            ("failed", 9_400)
        );
        let back = Card::from_record(&record, now, 10_000).unwrap();
        assert_eq!(
            (back.name.as_str(), back.state.clone()),
            ("fix it", State::Failed("exit 2".into()))
        );
        let running = card(None, None);
        assert_eq!(running.to_record(now, 1).status, "stopped");
        let bad = crate::store::state::Record {
            id: "not-a-uuid".into(),
            ..record
        };
        assert!(Card::from_record(&bad, now, 1).is_none());
    }

    #[test]
    fn resume_ids_must_be_uuids() {
        let mut claude = card(None, None);
        assert_eq!(
            claude.resume_id(),
            Some(claude.id.0.hyphenated().to_string()),
            "its own --session-id"
        );
        claude.agent_session = Some("../etc".into());
        assert_eq!(claude.resume_id(), None, "never a name or a path");
        let mut codex = Card::new(
            SessionId::new(),
            Kind::Codex,
            "/p".into(),
            None,
            None,
            Instant::now(),
        );
        assert_eq!(codex.resume_id(), None, "no hooks, nothing to resume");
        codex.agent_session = Some("01a0f2cb-0303-7ae2-8133-1474fd08a780".into());
        assert!(codex.resume_id().is_some());
    }

    #[test]
    fn orders_by_state_then_newest_first() {
        let t0 = Instant::now();
        let mut cards: Vec<Card> = (0..4)
            .map(|i| {
                let mut c = card(Some(&i.to_string()), None);
                c.started = t0 + Duration::from_secs(i);
                c
            })
            .collect();
        cards[0].exited(Some(0), t0);
        cards[1].exited(Some(2), t0);
        cards[3].state = State::NeedsYou;
        cards.push(Card::new(
            SessionId::new(),
            Kind::Codex,
            "/other".into(),
            None,
            None,
            t0,
        ));
        let names: Vec<&str> = order(&cards, std::path::Path::new("/p"))
            .iter()
            .map(|&i| cards[i].name.as_str())
            .collect();
        assert_eq!(names, ["3", "1", "2", "0"]);
    }
}
