//! Cloud mode (`C`): a Claude session runs on this machine or in Claude's
//! cloud (claude.ai/code), and changes sides at any time.
//!
//! mc only passes Claude's own flags. To the cloud: `claude --cloud=<task>`
//! starts a new cloud session for the folder's repository; Claude's CLI
//! cannot send a local conversation there, so the task line is all it
//! knows. To this machine: `claude --teleport [id]` brings the cloud
//! session's conversation and branch. Either way the card's process is
//! stopped first and the new one takes its place.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rustix::process::Signal;

use crate::agent::{Cloud, Kind, Launch, Mode};
use crate::app::model::{Cmd, LaunchRequest, Model, Overlay};
use crate::term::SessionId;

/// The `C` dialog: where a session should run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModeDialog {
    /// The session.
    pub id: SessionId,
    /// Where it runs now.
    pub from: Mode,
    /// The chosen mode.
    pub to: Mode,
    /// The task the cloud session starts with.
    pub task: String,
}

impl Model {
    /// Opens the `C` dialog on the selected session, with the other mode
    /// chosen; cloud mode is Claude's alone.
    pub(crate) fn open_mode(&mut self) {
        let Some(card) = self.selected_card().map(|i| &self.cards[i]) else {
            return;
        };
        if card.kind != Kind::Claude {
            self.message = Some("Cloud mode is Claude Code only.".into());
            return;
        }
        let to = match card.mode {
            Mode::Local => Mode::Cloud,
            Mode::Cloud => Mode::Local,
        };
        self.overlay = Some(Overlay::Mode(ModeDialog {
            id: card.id,
            from: card.mode,
            to,
            task: format!("Continue: {}", card.name),
        }));
    }

    /// Handles a key in the `C` dialog: `←`/`→`/`tab` choose the mode,
    /// typing edits the task of a session going to the cloud, `enter`
    /// applies, `esc` closes.
    pub(crate) fn mode_key(&mut self, mut dialog: ModeDialog, key: KeyEvent) -> Option<Cmd> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let typing = dialog.from == Mode::Local && dialog.to == Mode::Cloud;
        match key.code {
            KeyCode::Esc => return None,
            KeyCode::Enter => return self.switch_mode(dialog),
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                dialog.to = match dialog.to {
                    Mode::Local => Mode::Cloud,
                    Mode::Cloud => Mode::Local,
                };
            }
            KeyCode::Char('u') if ctrl && typing => dialog.task.clear(),
            KeyCode::Char(c) if !ctrl && typing => dialog.task.push(c),
            KeyCode::Backspace if typing => {
                dialog.task.pop();
            }
            _ => {}
        }
        self.overlay = Some(Overlay::Mode(dialog));
        None
    }

    /// Applies the dialog: nothing when the mode stays, else the launch
    /// that replaces the card, after its process has stopped.
    fn switch_mode(&mut self, dialog: ModeDialog) -> Option<Cmd> {
        if dialog.to == dialog.from {
            return None;
        }
        let task = dialog.task.trim().to_owned();
        if dialog.to == Mode::Cloud && task.is_empty() {
            self.message =
                Some("A cloud session needs a task: the conversation stays here.".into());
            self.overlay = Some(Overlay::Mode(dialog));
            return None;
        }
        let now = self.now;
        let card = self.card_mut(dialog.id)?;
        card.note_cloud_id();
        let (cloud, prompt) = match dialog.to {
            Mode::Cloud => (Cloud::Start, Some(task)),
            Mode::Local => (Cloud::Pull(card.cloud_id.clone()), None),
        };
        let request = LaunchRequest {
            project: card.project.clone(),
            kind: Kind::Claude,
            launch: Launch {
                id: SessionId::new(),
                model: None,
                name: Some(card.name.clone()),
                prompt,
                settings: None,
                hook_args: Vec::new(),
                resume: None,
                pick: false,
                fork: false,
                cloud: Some(cloud),
            },
            replaces: Some(card.id),
        };
        crate::debug_log!("{} changes to {:?}", card.id.short(), dialog.to);
        if !card.running() {
            return Some(Cmd::Launch(request));
        }
        card.then = Some(request);
        card.stop_requested = Some(now);
        if let Some(pty) = &card.pty {
            // reason: ESRCH means it already exited; the waiter reports it.
            let _ = pty.signal(Signal::TERM);
        }
        self.message = Some("Changing mode: closing the session…".into());
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppEvent;
    use crate::app::model::tests::{press, sample, with_session};
    use crate::term::PtyEvent;

    #[test]
    fn a_session_goes_to_the_cloud_and_comes_back() {
        let mut m = sample(&["app"]);
        let (id, _writes) = with_session(&mut m, "fix login");
        m.focus = crate::app::model::Focus::Sessions;
        m.update(press(KeyCode::Char('C')));
        let Some(Overlay::Mode(dialog)) = &m.overlay else {
            panic!("C opens the mode dialog");
        };
        assert_eq!(
            (dialog.to, dialog.task.as_str()),
            (Mode::Cloud, "Continue: fix login")
        );
        m.update(press(KeyCode::Char('!')));
        assert!(
            m.update(press(KeyCode::Enter)).is_none(),
            "a running one stops first"
        );
        assert!(m.cards[0].stop_requested.is_some());
        let Some(Cmd::Launch(req)) = m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(143))))
        else {
            panic!("starts the cloud session once the local one has exited");
        };
        assert_eq!(req.replaces, Some(id));
        assert_eq!(req.launch.cloud, Some(Cloud::Start));
        assert_eq!(req.launch.prompt.as_deref(), Some("Continue: fix login!"));

        let card = m.card_mut(id).unwrap();
        card.mode = Mode::Cloud;
        card.cloud_id = Some("session_01DiUkqY2kzb".into());
        assert!(
            m.update(press(KeyCode::Char('r'))).is_none(),
            "nothing to resume on this machine"
        );
        m.update(press(KeyCode::Char('C')));
        m.update(press(KeyCode::Char('x')));
        let Some(Cmd::Launch(req)) = m.update(press(KeyCode::Enter)) else {
            panic!("a finished cloud card comes back at once");
        };
        assert_eq!(
            req.launch.cloud,
            Some(Cloud::Pull(Some("session_01DiUkqY2kzb".into())))
        );
        assert_eq!(req.launch.prompt, None, "typing changes no task here");
    }

    #[test]
    fn the_mode_stays_without_a_change_or_a_task_and_codex_has_none() {
        let mut m = sample(&["app"]);
        let (_id, _writes) = with_session(&mut m, "s");
        m.focus = crate::app::model::Focus::Sessions;
        m.update(press(KeyCode::Char('C')));
        m.update(press(KeyCode::Tab));
        assert!(m.update(press(KeyCode::Enter)).is_none());
        assert!(m.overlay.is_none(), "local stays local");
        assert!(m.cards[0].stop_requested.is_none());

        m.update(press(KeyCode::Char('C')));
        m.update(AppEvent::Input(ratatui::crossterm::event::Event::Key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        )));
        assert!(m.update(press(KeyCode::Enter)).is_none());
        assert!(
            matches!(m.overlay, Some(Overlay::Mode(_))),
            "no task, no cloud session"
        );
        m.update(press(KeyCode::Esc));

        m.cards[0].kind = Kind::Codex;
        m.update(press(KeyCode::Char('C')));
        assert!(m.overlay.is_none());
    }
}
