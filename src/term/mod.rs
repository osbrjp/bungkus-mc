//! The live output pane: agent processes in PTYs, the embedded emulator,
//! and input encoding toward the agent (ARCHITECTURE §4.1).
//!
//! Every byte an agent writes goes into an `alacritty_terminal` grid; only
//! printable cells and SGR attributes ever reach the screen. Nothing here
//! reads agent transcripts.

pub(crate) mod keys;
pub(crate) mod screen;
pub(crate) mod session;

use uuid::Uuid;

/// mc's own id for one session; also Claude's `--session-id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SessionId(pub Uuid);

impl SessionId {
    /// Returns a new random id.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Returns the `#a3f1` short form shown on cards (DESIGN §5.2).
    #[must_use]
    pub(crate) fn short(self) -> String {
        let hex = self.0.simple().to_string();
        format!("#{}", &hex[..4])
    }
}

/// What a session's background threads report to the event loop.
#[derive(Debug)]
pub(crate) enum PtyEvent {
    /// Bytes the agent wrote.
    Output(SessionId, Vec<u8>),
    /// The agent process ended with this exit code (`None`: killed by a
    /// signal), after its output was drained or 500 ms passed.
    Exited(SessionId, Option<u32>),
}
