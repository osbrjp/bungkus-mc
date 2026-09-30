//! Taking over a session started outside mc (ARCHITECTURE §3.4): mc waits
//! until the user has closed it in its own terminal, then resumes it here
//! with `claude --resume <id>`, so it gets a card and a live output pane.
//!
//! mc never stops the outside process itself: it only checks with signal 0
//! whether the pid still exists, which delivers nothing.

use std::time::{Duration, Instant};

use rustix::process::{Pid, test_kill_process};

use crate::agent::{Kind, Launch};
use crate::app::model::{Cmd, LaunchRequest, Model, Overlay};
use crate::external::External;
use crate::term::SessionId;

/// How often a waiting take-over checks whether the outside process has
/// closed.
pub(crate) const TAKEOVER_POLL: Duration = Duration::from_millis(300);

/// Returns whether process `pid` still exists (signal 0; nothing is sent).
fn alive(pid: i32) -> bool {
    Pid::from_raw(pid).is_some_and(
        |pid| !matches!(test_kill_process(pid), Err(e) if e == rustix::io::Errno::SRCH),
    )
}

impl Model {
    /// Starts taking over the selected outside session: refuses what cannot
    /// be resumed, launches at once when it has already closed, and
    /// otherwise opens the dialog that waits for it to close.
    pub(crate) fn take_over(&mut self, ext: External) -> Option<Cmd> {
        let project = self.selected_project()?.path.clone();
        if ext.kind == Kind::Codex {
            self.message = Some(
                "Only Claude sessions can be taken over: Codex doesn't say which session it is."
                    .into(),
            );
            return None;
        }
        let id = ext
            .session_id
            .clone()
            .filter(|id| uuid::Uuid::parse_str(id).is_ok());
        if id.is_none() {
            self.message = Some("This session has no id to resume.".into());
            return None;
        }
        if project.as_os_str().is_empty() {
            self.message = Some(
                "It runs outside the workspace's projects; resume it there with claude --resume."
                    .into(),
            );
            return None;
        }
        if !crate::workspace::same_dir(&ext.cwd, &project) {
            self.message = Some(format!(
                "It runs in a subfolder ({}); resume it there with claude --resume.",
                crate::store::config::tilde(&ext.cwd, self.home.as_deref())
            ));
            return None;
        }
        if alive(ext.pid) {
            self.overlay = Some(Overlay::TakeOver(ext, self.now));
            return None;
        }
        Some(resume(&ext, project))
    }

    /// Checks a waiting take-over on each wake-up: once the outside
    /// process has closed, the dialog closes and the session is resumed.
    pub(crate) fn take_over_due(&mut self) -> Option<Cmd> {
        let Some(Overlay::TakeOver(ext, _)) = &self.overlay else {
            return None;
        };
        if alive(ext.pid) {
            return None;
        }
        let Some(Overlay::TakeOver(ext, _)) = self.overlay.take() else {
            return None;
        };
        let project = self.selected_project()?.path.clone();
        Some(resume(&ext, project))
    }

    /// Returns when a waiting take-over checks again, if one is waiting.
    #[must_use]
    pub(crate) fn take_over_deadline(&self) -> Option<Instant> {
        matches!(self.overlay, Some(Overlay::TakeOver(..))).then(|| self.now + TAKEOVER_POLL)
    }
}

/// Returns the launch that resumes `ext` in `project`.
fn resume(ext: &External, project: std::path::PathBuf) -> Cmd {
    Cmd::Launch(LaunchRequest {
        project,
        kind: ext.kind,
        launch: Launch {
            id: SessionId::new(),
            model: None,
            name: Some(ext.name.clone()),
            prompt: None,
            settings: None,
            hook_args: Vec::new(),
            resume: ext.session_id.clone(),
        },
        replaces: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_pid_is_not_alive() {
        assert!(alive(i32::try_from(std::process::id()).unwrap()));
        assert!(!alive(i32::MAX - 7));
    }

    #[test]
    fn takes_over_a_claude_session_once_it_closes() {
        use ratatui::crossterm::event::KeyCode;

        use crate::app::AppEvent;
        use crate::app::model::Focus;
        use crate::app::model::tests::{press, sample};

        let mut m = sample(&["app"]);
        let project = m.selected_project().unwrap().path.clone();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let id = "a8e34add-0962-4129-bd62-670f74813675";
        let ext = |kind, session: Option<&str>| External {
            kind,
            pid: i32::try_from(child.id()).unwrap(),
            cwd: project.clone(),
            name: "outside".into(),
            status: Some("idle".into()),
            session_id: session.map(Into::into),
            started_ms: None,
        };
        m.update(AppEvent::External(vec![
            ext(Kind::Codex, None),
            ext(Kind::Claude, Some(id)),
        ]));
        m.focus = Focus::Sessions;
        m.update(press(KeyCode::Enter));
        assert!(
            m.message.unwrap_or_default().contains("Only Claude"),
            "codex is refused"
        );
        m.message = None;
        m.update(press(KeyCode::Char('j')));
        assert!(m.update(press(KeyCode::Enter)).is_none());
        assert!(
            matches!(m.overlay, Some(Overlay::TakeOver(..))),
            "waits while it runs"
        );
        assert!(m.deadline(m.now + Duration::from_secs(9)).is_some());
        assert!(m.update(AppEvent::Tick).is_none());
        child.kill().unwrap();
        child.wait().unwrap();
        let Some(Cmd::Launch(req)) = m.update(AppEvent::Tick) else {
            panic!("resumes once closed");
        };
        assert_eq!(req.launch.resume.as_deref(), Some(id));
        assert_eq!(req.project, project);
        assert!(m.overlay.is_none());
    }
}
