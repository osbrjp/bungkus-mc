//! Session cards: what mc knows about each session it started.
//!
//! A card outlives its process: a wrapped, failed or stopped session stays
//! listed with its last screen. The state here comes from the process in
//! M3; hook events refine it from M4 on.

use std::cmp::Reverse;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::agent::Kind;
use crate::term::SessionId;
use crate::term::session::Session;
use crate::ui::sanitise::sanitise;

/// Longest session name shown, in characters (DESIGN §5.2, ARCHITECTURE §5.3).
const NAME_MAX: usize = 80;

/// Grace between SIGTERM and SIGKILL when stopping (ARCHITECTURE §3.2).
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(3);

/// A session's state (DESIGN §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum State {
    /// The agent process is running.
    Running,
    /// The process exited non-zero, or could not start; the reason line.
    Failed(String),
    /// The user stopped it (`x` or quit), whatever its exit code.
    Stopped,
    /// The process exited 0.
    Wrapped,
}

impl State {
    /// Returns the card order rank: failed, working, stopped, wrapped
    /// (DESIGN §5.2).
    const fn rank(&self) -> u8 {
        match self {
            Self::Failed(_) => 1,
            Self::Running => 2,
            Self::Stopped => 4,
            Self::Wrapped => 5,
        }
    }
}

/// One session mc started.
#[derive(Debug)]
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
}

impl Card {
    /// Creates a card for a new session.
    ///
    /// The name falls back to the prompt's first line, then `untitled`
    /// (ARCHITECTURE §5.3).
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
            state: State::Running,
            started,
            ended: None,
            stop_requested: None,
            killed: false,
            pty: None,
        }
    }

    /// Records the process exit: stopped when the user asked, else wrapped
    /// on 0 and failed otherwise, with the screen's last line as reason.
    pub(crate) fn exited(&mut self, code: Option<u32>, now: Instant) {
        self.ended = Some(now);
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

    /// Returns whether the process is still running.
    #[must_use]
    pub(crate) fn running(&self) -> bool {
        self.state == State::Running
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
        assert_eq!(names, ["1", "3", "2", "0"]);
    }
}
