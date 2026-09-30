//! INTERACT: keys, paste and mouse routed to the selected agent
//! (DESIGN §8.5, §8.6).
//!
//! While the output pane has focus every key goes to the agent except the
//! exit chord and `ctrl-z`. The mouse focuses panes, scrolls mc's
//! scrollback, and is forwarded when the agent turned mouse reporting on.

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::app::model::{Focus, Model};
use crate::term::keys;
use crate::ui;

/// Lines scrolled per mouse wheel notch (ARCHITECTURE §4.1).
const WHEEL_LINES: i32 = 3;

impl Model {
    /// Handles a key in INTERACT: the exit chord returns to the sessions
    /// pane, `ctrl-z` is swallowed, everything else is encoded for the
    /// agent (and snaps its view back to the bottom).
    pub(super) fn interact_key(&mut self, key: KeyEvent) {
        if self.exit_chord.matches(&key) {
            self.focus = Focus::Sessions;
            return;
        }
        let ctrl_z =
            key.code == KeyCode::Char('z') && key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl_z {
            return;
        }
        if let Some(pty) = self
            .selected_card()
            .and_then(|i| self.cards[i].pty.as_mut())
        {
            pty.scroll(Scroll::Bottom);
            let bytes = keys::encode(&key, pty.mode());
            pty.send(bytes);
        }
    }

    /// Sends pasted text to the agent while in INTERACT.
    pub(super) fn paste(&self, text: &str) {
        if self.focus != Focus::Output {
            return;
        }
        if let Some(pty) = self
            .selected_card()
            .and_then(|i| self.cards[i].pty.as_ref())
        {
            pty.send(keys::paste(text, pty.mode()));
        }
    }

    /// Handles a mouse event: clicks focus panes, the wheel scrolls, and
    /// events inside the output pane go to an agent that asked for them.
    pub(super) fn mouse(&mut self, event: MouseEvent) {
        if self.overlay.is_some() {
            return;
        }
        let at = Position::new(event.column, event.row);
        let panes = ui::panes(self.screen, self.focus, self.zoom);
        let inside = |r: Option<Rect>| r.is_some_and(|r| r.contains(at));
        if inside(panes.output) && self.forward_mouse(event, panes.output) {
            return;
        }
        match event.kind {
            MouseEventKind::Down(_) if inside(panes.projects) => self.focus = Focus::Projects,
            MouseEventKind::Down(_) if inside(panes.sessions) => self.focus = Focus::Sessions,
            MouseEventKind::Down(_) if inside(panes.output) => {
                self.focus = Focus::Sessions;
                self.interact();
            }
            MouseEventKind::ScrollUp if inside(panes.output) => self.scroll(WHEEL_LINES),
            MouseEventKind::ScrollDown if inside(panes.output) => self.scroll(-WHEEL_LINES),
            _ => {}
        }
    }

    /// Forwards a mouse event inside the output pane to the agent when it
    /// enabled mouse reporting and INTERACT is on; returns whether it did.
    fn forward_mouse(&self, event: MouseEvent, pane: Option<Rect>) -> bool {
        let (Some(pane), Focus::Output) = (pane, self.focus) else {
            return false;
        };
        let Some(pty) = self
            .selected_card()
            .and_then(|i| self.cards[i].pty.as_ref())
        else {
            return false;
        };
        let col = event.column.saturating_sub(pane.x + 1);
        let row = event.row.saturating_sub(pane.y + 1);
        match keys::mouse(event.kind, col, row, event.modifiers, pty.mode()) {
            Some(bytes) => {
                pty.send(bytes);
                true
            }
            None => false,
        }
    }

    /// Scrolls the selected session's view; on the alternate screen there
    /// is no history, so it says where to scroll instead.
    fn scroll(&mut self, lines: i32) {
        let Some(i) = self.selected_card() else {
            return;
        };
        let Some(pty) = self.cards[i].pty.as_mut() else {
            return;
        };
        if pty.mode().contains(TermMode::ALT_SCREEN) {
            self.message = Some("scroll inside the agent".into());
        } else {
            pty.scroll(Scroll::Delta(lines));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;

    use ratatui::crossterm::event::{Event, MouseButton};

    use crate::app::AppEvent;
    use crate::app::model::tests::{sample, with_session};
    use crate::app::model::{Focus, Model};

    use super::*;

    fn drain(writes: &Receiver<Vec<u8>>) -> Vec<u8> {
        writes.try_iter().flatten().collect()
    }

    fn send(m: &mut Model, code: KeyCode, mods: KeyModifiers) {
        m.update(AppEvent::Input(Event::Key(KeyEvent::new(code, mods))));
    }

    #[test]
    fn every_key_reaches_the_agent_except_the_exit_chord_and_ctrl_z() {
        let mut m = sample(&["a"]);
        let (_, writes) = with_session(&mut m, "s");
        assert_eq!(m.focus, Focus::Output);
        let mut keys: Vec<(KeyCode, KeyModifiers)> = vec![
            (KeyCode::Char('q'), KeyModifiers::NONE),
            (KeyCode::Char('?'), KeyModifiers::NONE),
            (KeyCode::Esc, KeyModifiers::NONE),
            (KeyCode::Tab, KeyModifiers::NONE),
            (KeyCode::BackTab, KeyModifiers::SHIFT),
            (KeyCode::Enter, KeyModifiers::NONE),
            (KeyCode::Up, KeyModifiers::NONE),
            (KeyCode::Left, KeyModifiers::NONE),
            (KeyCode::F(5), KeyModifiers::NONE),
        ];
        keys.extend(
            ('a'..='y')
                .filter(|c| *c != 'z')
                .map(|c| (KeyCode::Char(c), KeyModifiers::CONTROL)),
        );
        for (code, mods) in keys {
            send(&mut m, code, mods);
            assert!(
                !drain(&writes).is_empty(),
                "{code:?} {mods:?} did not reach the agent"
            );
            assert_eq!(m.focus, Focus::Output);
        }
        send(&mut m, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert!(drain(&writes).is_empty(), "ctrl-z is swallowed");
        send(&mut m, KeyCode::Char('\\'), KeyModifiers::CONTROL);
        assert!(drain(&writes).is_empty(), "the exit chord is not sent");
        assert_eq!(m.focus, Focus::Sessions);
    }

    #[test]
    fn every_focus_route_enters_interact_in_the_same_update() {
        for code in [
            KeyCode::Enter,
            KeyCode::Char('l'),
            KeyCode::Right,
            KeyCode::Tab,
        ] {
            let mut m = sample(&["a"]);
            let _writes = with_session(&mut m, "s");
            m.focus = Focus::Sessions;
            send(&mut m, code, KeyModifiers::NONE);
            assert_eq!(m.focus, Focus::Output, "{code:?}");
        }
        let mut m = sample(&["a"]);
        let _writes = with_session(&mut m, "s");
        m.focus = Focus::Projects;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 100,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        m.update(AppEvent::Input(Event::Mouse(click)));
        assert_eq!(m.focus, Focus::Output, "click on the output pane");
    }

    #[test]
    fn paste_is_bracketed_only_when_the_agent_asked() {
        let mut m = sample(&["a"]);
        let (id, writes) = with_session(&mut m, "s");
        m.update(AppEvent::Input(Event::Paste("a\nb".into())));
        assert_eq!(drain(&writes), b"a\rb");
        m.update(AppEvent::Pty(crate::term::PtyEvent::Output(
            id,
            b"\x1b[?2004h".to_vec(),
        )));
        m.update(AppEvent::Input(Event::Paste("x".into())));
        assert_eq!(drain(&writes), b"\x1b[200~x\x1b[201~");
    }
}
