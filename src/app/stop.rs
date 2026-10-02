//! Stopping sessions and what they started: the quit/`x` dialog and the
//! signal sequence (ARCHITECTURE §3.2, DESIGN §5.5).
//!
//! The dialog lists the sessions first, then every tracked descendant with
//! a `[stop]`/`[keep]` toggle; exactly the listed `[stop]` set is
//! signalled. Sessions get SIGTERM to their process group (SIGKILL after
//! 3 s); a session's `[stop]` descendants get an identity-checked SIGTERM
//! once it has exited, and SIGKILL 3 s later. Nothing is signalled that is
//! not in the list.

use std::collections::HashMap;
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use rustix::process::Signal;

use crate::app::model::{Cmd, Model, Overlay};
use crate::app::sessions::{Card, STOP_GRACE};
use crate::proc::Proc;
use crate::term::SessionId;

/// What the dialog stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopKind {
    /// Quit mc: every running session and every tracked descendant.
    Quit,
    /// One session and its descendants (`x`).
    Session(SessionId),
}

/// One row of the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// A running session.
    Session(SessionId),
    /// A tracked descendant of `owner`, with its listening ports.
    Process {
        /// The session it descends from.
        owner: SessionId,
        /// The process.
        proc: Proc,
        /// Listening TCP ports (annotation only).
        ports: Vec<u16>,
    },
}

/// A row and whether it will be stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    /// What the row is.
    pub target: Target,
    /// `[stop]` when true, `[keep]` when false.
    pub stop: bool,
}

/// The open quit/stop dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StopDialog {
    /// Quit or one session.
    pub kind: StopKind,
    /// Sessions first, then processes.
    pub rows: Vec<Row>,
    /// Highlighted row.
    pub cursor: usize,
}

impl Model {
    /// Opens the dialog for `kind` from a fresh snapshot and port list,
    /// or quits at once when there is nothing to stop.
    pub(crate) fn open_stop(
        &mut self,
        kind: StopKind,
        snapshot: &[Proc],
        ports: &HashMap<i32, Vec<u16>>,
    ) -> Option<Cmd> {
        self.track(snapshot);
        let rows = self.stop_rows(kind, ports);
        if rows.is_empty() {
            return (kind == StopKind::Quit).then_some(Cmd::Quit);
        }
        self.overlay = Some(Overlay::Stop(StopDialog {
            kind,
            rows,
            cursor: 0,
        }));
        None
    }

    /// Folds a process snapshot into every card's tracked descendants.
    pub(crate) fn track(&mut self, snapshot: &[Proc]) {
        for card in &mut self.cards {
            if let Some(pid) = card.pid {
                card.descendants.update(pid, snapshot);
            }
        }
    }

    /// Builds the dialog rows for `kind`; process rows start as `[keep]`
    /// for the default-keep set (ARCHITECTURE §3.2).
    fn stop_rows(&self, kind: StopKind, ports: &HashMap<i32, Vec<u16>>) -> Vec<Row> {
        let chosen = |c: &Card| match kind {
            StopKind::Quit => true,
            StopKind::Session(id) => c.id == id,
        };
        let sessions = self
            .cards
            .iter()
            .filter(|c| chosen(c) && c.running())
            .map(|c| Row {
                target: Target::Session(c.id),
                stop: true,
            });
        let processes = self.cards.iter().filter(|c| chosen(c)).flat_map(|c| {
            c.descendants.procs.iter().map(|p| Row {
                target: Target::Process {
                    owner: c.id,
                    proc: p.clone(),
                    ports: ports.get(&p.pid).cloned().unwrap_or_default(),
                },
                stop: !p.keep_by_default(&self.keep),
            })
        });
        sessions.chain(processes).collect()
    }

    /// Handles a key in the dialog: `j`/`k` move, `space` toggles a
    /// process row, `y` or `enter` carries it out, anything else closes it.
    pub(crate) fn stop_key(&mut self, mut dialog: StopDialog, key: KeyEvent) -> Option<Cmd> {
        let last = dialog.rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => dialog.cursor = (dialog.cursor + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => dialog.cursor = dialog.cursor.saturating_sub(1),
            KeyCode::Char(' ') => {
                if let Some(row) = dialog.rows.get_mut(dialog.cursor)
                    && matches!(row.target, Target::Process { .. })
                {
                    row.stop = !row.stop;
                }
            }
            _ if crate::app::model::confirms(key) => return self.carry_out(&dialog),
            _ => return None,
        }
        self.overlay = Some(Overlay::Stop(dialog));
        None
    }

    /// Stops what `dialog` lists as `[stop]`: SIGTERM to the sessions now;
    /// their descendants wait for the session to exit, descendants of
    /// sessions that already ended get SIGTERM at once.
    pub(crate) fn carry_out(&mut self, dialog: &StopDialog) -> Option<Cmd> {
        let now = self.now;
        if dialog.kind == StopKind::Quit {
            self.quitting = Some(now);
            self.message = Some("stopping…".into());
        }
        for row in dialog.rows.iter().filter(|r| r.stop) {
            match &row.target {
                Target::Session(id) => {
                    if let Some(card) = self.card_mut(*id).filter(|c| c.running()) {
                        card.stop_requested = Some(now);
                        // An empty hooked session has no conversation to resume.
                        card.auto_resume =
                            dialog.kind == StopKind::Quit && (card.prompted || !card.hooked);
                        if let Some(pty) = &card.pty {
                            // reason: ESRCH means it already exited; the waiter reports it.
                            let _ = pty.signal(Signal::TERM);
                        }
                    }
                }
                Target::Process { owner, proc, .. } => {
                    if let Some(card) = self.card_mut(*owner) {
                        card.stop_plan.push(proc.clone());
                    }
                }
            }
        }
        let ended: Vec<Proc> = self
            .cards
            .iter_mut()
            .filter(|c| !c.running() && !c.stop_plan.is_empty() && c.plan_termed.is_none())
            .flat_map(|c| {
                c.plan_termed = Some(now);
                c.stop_plan.clone()
            })
            .collect();
        if ended.is_empty() {
            self.quit_when_stopped()
        } else {
            Some(Cmd::Signal(ended, Signal::TERM))
        }
    }

    /// Quits without asking when the terminal went away: every session is
    /// stopped and the default-keep rule decides the descendants.
    pub(crate) fn host_gone(&mut self) -> Option<Cmd> {
        let rows = self.stop_rows(StopKind::Quit, &HashMap::new());
        let dialog = StopDialog {
            kind: StopKind::Quit,
            rows,
            cursor: 0,
        };
        self.carry_out(&dialog)
    }

    /// Returns the SIGTERM for a stopped session's descendants once it has
    /// exited.
    pub(crate) fn plan_after_exit(&mut self, id: SessionId) -> Option<Cmd> {
        let now = self.now;
        let card = self.card_mut(id)?;
        if card.stop_requested.is_none() || card.stop_plan.is_empty() || card.plan_termed.is_some()
        {
            return None;
        }
        card.plan_termed = Some(now);
        Some(Cmd::Signal(card.stop_plan.clone(), Signal::TERM))
    }

    /// Returns the SIGKILL for one card whose descendants' grace has run
    /// out, and forgets its plan.
    pub(crate) fn plan_kill_due(&mut self) -> Option<Cmd> {
        let now = self.now;
        let card = self
            .cards
            .iter_mut()
            .find(|c| c.plan_termed.is_some_and(|at| now >= at + STOP_GRACE))?;
        card.plan_termed = None;
        Some(Cmd::Signal(
            std::mem::take(&mut card.stop_plan),
            Signal::KILL,
        ))
    }

    /// Returns when a descendant's grace ends next, if any.
    pub(crate) fn plan_deadline(&self) -> Option<Instant> {
        self.cards
            .iter()
            .filter_map(|c| c.plan_termed.map(|at| at + STOP_GRACE))
            .min()
    }

    /// Returns whether any stop is still in progress.
    pub(crate) fn stopping(&self) -> bool {
        self.cards
            .iter()
            .any(|c| c.running() || !c.stop_plan.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;
    use crate::app::AppEvent;
    use crate::app::model::tests::{sample, with_session};
    use crate::term::PtyEvent;

    #[expect(
        clippy::similar_names,
        reason = "pid and ppid are the process field names"
    )]
    fn proc(pid: i32, ppid: i32, comm: &str) -> Proc {
        Proc {
            pid,
            ppid,
            uid: 501,
            start: format!("s{pid}"),
            comm: comm.into(),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn the_dialog_lists_sessions_then_processes_with_keep_defaults() {
        let mut m = sample(&["a"]);
        let (id, _w) = with_session(&mut m, "s");
        m.cards[0].pid = Some(100);
        let snap = [
            proc(100, 1, "claude"),
            proc(101, 100, "node"),
            proc(102, 101, "ssh-agent"),
        ];
        let ports = HashMap::from([(101, vec![5173])]);
        assert_eq!(m.open_stop(StopKind::Quit, &snap, &ports), None);
        let Some(Overlay::Stop(d)) = &m.overlay else {
            panic!("dialog")
        };
        assert_eq!(d.rows[0].target, Target::Session(id));
        assert!(
            matches!(&d.rows[1].target, Target::Process { proc, ports, .. } if proc.pid == 101 && ports == &[5173])
        );
        assert_eq!(
            d.rows.iter().map(|r| r.stop).collect::<Vec<_>>(),
            [true, true, false]
        );
    }

    #[test]
    fn exactly_the_stop_set_is_signalled_after_the_session_exits() {
        let mut m = sample(&["a"]);
        let (id, _w) = with_session(&mut m, "s");
        m.cards[0].pid = Some(100);
        let snap = [proc(101, 100, "node"), proc(102, 100, "vite")];
        m.open_stop(StopKind::Session(id), &snap, &HashMap::new());
        let Some(Overlay::Stop(d)) = m.overlay.take() else {
            panic!("dialog")
        };
        let mut d = d;
        d.cursor = 2;
        let d = {
            m.stop_key(d.clone(), key(KeyCode::Char(' ')));
            match m.overlay.take() {
                Some(Overlay::Stop(d)) => d,
                _ => panic!("still open"),
            }
        };
        assert!(!d.rows[2].stop, "vite toggled to keep");
        assert_eq!(
            m.stop_key(d, key(KeyCode::Enter)),
            None,
            "enter is yes: nothing until the session exits"
        );
        let cmd = m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(143))));
        assert_eq!(
            cmd,
            Some(Cmd::Signal(vec![proc(101, 100, "node")], Signal::TERM))
        );
        m.now += STOP_GRACE;
        assert_eq!(
            m.update(AppEvent::Tick),
            Some(Cmd::Signal(vec![proc(101, 100, "node")], Signal::KILL))
        );
        assert!(m.cards[0].stop_plan.is_empty());
    }

    #[test]
    fn x_an_empty_session_and_another_workspace_do_not_resume() {
        let mut m = sample(&["a"]);
        let (by_x, _w1) = with_session(&mut m, "one");
        let (empty, _w2) = with_session(&mut m, "two");
        let (away, _w3) = with_session(&mut m, "three");
        let stop = |m: &mut Model, kind| {
            m.open_stop(kind, &[], &HashMap::new());
            let Some(Overlay::Stop(d)) = m.overlay.take() else {
                panic!("dialog")
            };
            m.carry_out(&d);
        };
        stop(&mut m, StopKind::Session(by_x));
        m.update(AppEvent::Pty(PtyEvent::Exited(by_x, Some(0))));
        let card = m.card_mut(empty).unwrap();
        (card.hooked, card.prompted) = (true, false);
        m.card_mut(away).unwrap().project = "/elsewhere/p".into();
        stop(&mut m, StopKind::Quit);
        let marked = [by_x, empty, away].map(|id| m.card_mut(id).unwrap().auto_resume);
        assert_eq!(marked, [false, false, true], "x, never prompted, quit");
        assert!(m.auto_resume().is_empty(), "its project is not open here");
    }

    #[test]
    fn quit_waits_for_descendants_and_esc_stops_nothing() {
        let mut m = sample(&["a"]);
        let (id, _w) = with_session(&mut m, "s");
        m.cards[0].pid = Some(100);
        m.open_stop(StopKind::Quit, &[proc(101, 100, "node")], &HashMap::new());
        let Some(Overlay::Stop(d)) = m.overlay.take() else {
            panic!("dialog")
        };
        assert_eq!(m.stop_key(d.clone(), key(KeyCode::Esc)), None);
        assert!(
            m.overlay.is_none() && m.quitting.is_none(),
            "esc stops nothing"
        );
        m.carry_out(&d);
        assert!(m.quitting.is_some());
        assert!(m.cards[0].auto_resume, "quit marks it for the next start");
        let Some(Cmd::Launch(req)) = m.auto_resume().pop() else {
            panic!("resumes at the next start");
        };
        assert_eq!(req.replaces, Some(id));
        assert!(req.launch.resume.is_some());
        m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(0))));
        assert!(m.stopping(), "descendants still pending");
        m.now += STOP_GRACE;
        assert!(matches!(m.update(AppEvent::Tick), Some(Cmd::Signal(_, _))));
        assert_eq!(m.update(AppEvent::Tick), Some(Cmd::Quit));
    }
}
