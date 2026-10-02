//! INTERACT: keys, paste and mouse routed to the selected agent
//! (DESIGN §8.5, §8.6).
//!
//! While the output pane has focus every key goes to the agent except the
//! exit chord, a fast `jj` / `jk` and `ctrl-z`. The mouse focuses panes, scrolls mc's
//! scrollback, and is forwarded when the agent turned mouse reporting on.

use std::ops::ControlFlow;
use std::time::Duration;

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::app::model::{Cmd, Focus, Model};
use crate::app::tools::TermView;
use crate::term::keys;
use crate::ui;

/// Lines scrolled per mouse wheel notch (ARCHITECTURE §4.1).
const WHEEL_LINES: i32 = 3;

/// Longest gap between the two keys of `jj` / `jk` that still leaves
/// INTERACT (DESIGN §8.5); slower pairs are ordinary typing.
const LEAVE_WINDOW: Duration = Duration::from_millis(200);

/// A pane border the mouse can drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Divider {
    /// Between projects and sessions (the projects width).
    Projects,
    /// Between sessions and output (the sessions width).
    Sessions,
}

impl Model {
    /// Handles a key in INTERACT: the exit chord and `ctrl-h` return to
    /// the sessions pane, cmd/alt/ctrl + 1–4 focus that pane, `ctrl-j` goes
    /// down to the terminal pane while it shows (otherwise it is the
    /// agent's), `ctrl-l` (already the rightmost pane) and `ctrl-z` are
    /// swallowed, everything else is encoded for the agent (and snaps
    /// its view back to the bottom). A plain `j` followed within
    /// [`LEAVE_WINDOW`] by `j` or `k` also returns to the sessions pane.
    ///
    /// # Returns
    ///
    /// The command that starts the project's shell, for the terminal
    /// pane's chord on a project without one.
    pub(super) fn interact_key(&mut self, key: KeyEvent) -> Option<Cmd> {
        if self.exit_chord.matches(&key) {
            self.focus = Focus::Sessions;
            return None;
        }
        if let crate::ui::keymap::Lookup::Action(crate::ui::keymap::Action::Pane(n)) =
            crate::ui::keymap::lookup(crate::ui::keymap::Scope::Global, key, None)
        {
            return self.focus_pane(n);
        }
        if key.modifiers == KeyModifiers::CONTROL {
            match key.code {
                KeyCode::Char('h') => {
                    self.focus = Focus::Sessions;
                    return None;
                }
                KeyCode::Char('l') => return None,
                KeyCode::Char('j') if self.terminal_shown() => return self.focus_terminal(),
                _ => {}
            }
        }
        let ctrl_z =
            key.code == KeyCode::Char('z') && key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl_z {
            return None;
        }
        let plain = key.modifiers == KeyModifiers::NONE && key.kind == KeyEventKind::Press;
        let armed = self
            .leave_j
            .take()
            .is_some_and(|at| self.now.duration_since(at) <= LEAVE_WINDOW);
        let leave = plain && armed && matches!(key.code, KeyCode::Char('j' | 'k'));
        if plain && !leave && key.code == KeyCode::Char('j') {
            self.leave_j = Some(self.now);
        }
        if let Some(pty) = self
            .selected_card()
            .and_then(|i| self.cards[i].pty.as_mut())
        {
            pty.scroll(Scroll::Bottom);
            // ponytail: the first `j` already went to the agent, so a
            // backspace takes it back; wrong where `j` is not text (a menu,
            // vim normal mode). Hold the `j` until the window ends if that
            // matters.
            let key = if leave {
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
            } else {
                key
            };
            let bytes = keys::encode(&key, pty.mode());
            pty.send(bytes);
        }
        if leave {
            self.focus = Focus::Sessions;
        }
        None
    }

    /// Sends pasted text to the tool that has the keys, else to the agent
    /// while in INTERACT; a dialog over a tool takes nothing.
    pub(super) fn paste(&mut self, text: &str) {
        if self.editor.is_some() || self.term_view == TermView::Focused {
            let free = self.overlay.is_none();
            if let (true, Some(pty)) = (free, self.keyed_tool()) {
                pty.send(keys::paste(text, pty.mode()));
            }
            return;
        }
        let target = match self.popup {
            Some(id) => self.cards.iter().find(|c| c.id == id),
            None if self.focus == Focus::Output => self.selected_card().map(|i| &self.cards[i]),
            None => None,
        };
        if let Some(pty) = target.and_then(|c| c.pty.as_ref()) {
            pty.send(keys::paste(text, pty.mode()));
        }
    }

    /// Handles a mouse event: a pane's right border drags to resize it
    /// (saved on release), clicks focus panes (the terminal pane too) and
    /// select the project row or session under the pointer (or open and
    /// close the rest of the projects), the wheel scrolls the output or the
    /// terminal pane under the pointer, and events inside the output pane
    /// go to an agent that asked for them.
    pub(super) fn mouse(&mut self, event: MouseEvent) -> Option<Cmd> {
        if self.overlay.is_some() || self.popup.is_some() || self.editor.is_some() {
            return None;
        }
        let at = Position::new(event.column, event.row);
        let panes = self.panes(self.screen);
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            && panes
                .output
                .is_some_and(|o| ui::strip_mascot(o).contains(at))
        {
            self.poke();
            return None;
        }
        if let ControlFlow::Break(cmd) = self.drag_border(event, &panes) {
            return cmd;
        }
        let inside = |r: Option<Rect>| r.is_some_and(|r| r.contains(at));
        if matches!(event.kind, MouseEventKind::Down(_)) && self.terminal_shown() {
            self.term_view = if inside(panes.terminal) {
                TermView::Focused
            } else {
                TermView::Shown
            };
        }
        if inside(panes.output) && self.forward_mouse(event, panes.output) {
            return None;
        }
        match event.kind {
            MouseEventKind::Down(_) if inside(panes.projects) => {
                self.focus = Focus::Projects;
                let rest = self.rest().map(|rest| rest.at);
                let rows = self.visible().len();
                let row = panes
                    .projects
                    .and_then(|pane| ui::project_at(pane, self.selected, rest, rows, event.row));
                match row {
                    Some(ui::Row::Project(row)) if row != self.selected => {
                        self.selected = row;
                        self.card = 0;
                    }
                    Some(ui::Row::Rest) => self.toggle_rest(),
                    Some(ui::Row::Project(_)) | None => {}
                }
            }
            MouseEventKind::Down(_) if inside(panes.sessions) => {
                let row = panes
                    .sessions
                    .and_then(|pane| ui::session_at(pane, self, event.row));
                self.focus = Focus::Sessions;
                if row == Some(self.card) {
                    return self.enter_session();
                }
                self.card = row.unwrap_or(self.card);
            }
            MouseEventKind::Down(_) if inside(panes.output) => {
                self.focus = Focus::Sessions;
                self.interact();
            }
            MouseEventKind::ScrollUp if inside(panes.output) => self.scroll(WHEEL_LINES),
            MouseEventKind::ScrollDown if inside(panes.output) => self.scroll(-WHEEL_LINES),
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Some(pane) = panes.terminal.filter(|r| r.contains(at)) {
                    self.wheel_terminal(event, pane);
                }
            }
            _ => {}
        }
        None
    }

    /// Handles the wheel over the terminal pane `pane`: a program in it
    /// that asked for the mouse gets the event, else the shell's
    /// scrollback moves (the alternate screen has none).
    fn wheel_terminal(&mut self, event: MouseEvent, pane: Rect) {
        let Some(pty) = self.shell_mut() else {
            return;
        };
        let col = event.column.saturating_sub(pane.x + 1);
        let row = event.row.saturating_sub(pane.y + 1);
        if let Some(bytes) = keys::mouse(event.kind, col, row, event.modifiers, pty.mode()) {
            pty.send(bytes);
        } else if !pty.mode().contains(TermMode::ALT_SCREEN) {
            let up = event.kind == MouseEventKind::ScrollUp;
            pty.scroll(Scroll::Delta(if up { WHEEL_LINES } else { -WHEEL_LINES }));
        }
    }

    /// Starts, follows or ends a border drag; breaks with the command to
    /// run when the event was part of one.
    ///
    /// A press on the right border of the projects or sessions pane (the
    /// column where two panes meet) starts it; the pane's width follows the
    /// pointer within [`ui::Widths::fit`]; the release saves the widths.
    fn drag_border(&mut self, event: MouseEvent, panes: &ui::Panes) -> ControlFlow<Option<Cmd>> {
        let (Some(projects), Some(sessions)) = (panes.projects, panes.sessions) else {
            return ControlFlow::Continue(());
        };
        let body = projects.y..projects.bottom();
        let on = |pane: Rect| {
            body.contains(&event.row)
                && (event.column + 1 == pane.right() || event.column == pane.right())
        };
        match (event.kind, self.drag) {
            (MouseEventKind::Down(MouseButton::Left), _) => {
                self.drag = if on(projects) {
                    Some(Divider::Projects)
                } else if on(sessions) && panes.output.is_some_and(|o| o.x == sessions.right()) {
                    Some(Divider::Sessions)
                } else {
                    None
                };
                match self.drag {
                    Some(_) => ControlFlow::Break(None),
                    None => ControlFlow::Continue(()),
                }
            }
            (MouseEventKind::Drag(MouseButton::Left), Some(divider)) => {
                let width = self.screen.width;
                let mut widths = self.widths.fit(width);
                match divider {
                    Divider::Projects => {
                        widths.projects = (event.column + 1).saturating_sub(projects.x);
                    }
                    Divider::Sessions => {
                        widths.sessions = (event.column + 1).saturating_sub(sessions.x);
                    }
                }
                self.widths = widths.fit(width);
                ControlFlow::Break(None)
            }
            (MouseEventKind::Up(_), Some(_)) => {
                self.drag = None;
                ControlFlow::Break(Some(Cmd::SaveWidths(self.widths)))
            }
            _ => ControlFlow::Continue(()),
        }
    }

    /// Forwards a mouse event inside the output pane to the agent when it
    /// enabled mouse reporting and INTERACT is on; returns whether it did.
    ///
    /// The agent's screen starts below the border and the mascot strip
    /// ([`ui::STRIP_HEIGHT`]); an event on the strip is not forwarded.
    fn forward_mouse(&self, event: MouseEvent, pane: Option<Rect>) -> bool {
        let (Some(pane), Focus::Output) = (pane, self.focus) else {
            return false;
        };
        let top = pane.y + 1 + ui::STRIP_HEIGHT;
        if event.row < top {
            return false;
        }
        let Some(pty) = self
            .selected_card()
            .and_then(|i| self.cards[i].pty.as_ref())
        else {
            return false;
        };
        let col = event.column.saturating_sub(pane.x + 1);
        let row = event.row - top;
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
    fn every_key_reaches_the_agent_except_the_exit_chord_and_ctrl_h_l_z() {
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
                .filter(|c| !matches!(c, 'h' | 'l'))
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
    fn a_fast_jj_or_jk_leaves_interact_and_takes_the_j_back() {
        let j = KeyCode::Char('j');
        for second in [j, KeyCode::Char('k')] {
            let mut m = sample(&["a"]);
            let (_, writes) = with_session(&mut m, "s");
            send(&mut m, j, KeyModifiers::NONE);
            assert_eq!((drain(&writes), m.focus), (b"j".to_vec(), Focus::Output));
            send(&mut m, second, KeyModifiers::NONE);
            assert_eq!(
                (drain(&writes), m.focus),
                (b"\x7f".to_vec(), Focus::Sessions)
            );
        }
        let mut m = sample(&["a"]);
        let (_, writes) = with_session(&mut m, "s");
        send(&mut m, j, KeyModifiers::NONE);
        m.now += LEAVE_WINDOW + Duration::from_millis(1);
        send(&mut m, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!((drain(&writes), m.focus), (b"jk".to_vec(), Focus::Output));
        send(&mut m, j, KeyModifiers::NONE);
        send(&mut m, KeyCode::Char('a'), KeyModifiers::NONE);
        send(&mut m, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(m.focus, Focus::Output, "a key between them disarms");
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
    fn a_click_on_a_project_row_selects_it() {
        let mut m = sample(&["a", "b", "c"]);
        m.focus = Focus::Sessions;
        let pane = ui::panes(m.screen, m.zoom, m.widths).projects.unwrap();
        let click = |row| {
            AppEvent::Input(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: pane.x + 3,
                row,
                modifiers: KeyModifiers::NONE,
            }))
        };
        m.update(click(pane.y + 4));
        assert_eq!((m.focus, m.selected), (Focus::Projects, 2));
        for (row, why) in [
            (pane.y + 1, "the search row"),
            (pane.y + 9, "below the list"),
        ] {
            m.update(click(row));
            assert_eq!(m.selected, 2, "{why}");
        }
        m.update(click(pane.y + 2));
        assert_eq!(m.selected, 0);
    }

    #[test]
    fn ctrl_j_and_ctrl_k_move_between_the_agent_and_a_shown_terminal() {
        let mut m = sample(&["a"]);
        let (_, writes) = with_session(&mut m, "s");
        send(&mut m, KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(drain(&writes), b"\n", "no terminal: the agent's newline");
        let black = alacritty_terminal::vte::ansi::Rgb::default();
        let colors = crate::term::session::Colors {
            fg: black,
            bg: black,
        };
        let size = crate::term::session::Size { cols: 40, rows: 8 };
        let (pty, _shell) = crate::term::session::Session::detached(size, colors);
        let id = crate::term::SessionId::new();
        let owner = crate::app::tools::Owner::Session(m.cards[0].id);
        m.shells.push((owner, crate::app::tools::Tool { id, pty }));
        m.term_view = TermView::Shown;
        send(&mut m, KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(m.term_view, TermView::Focused, "down to the terminal");
        assert!(drain(&writes).is_empty());
        send(&mut m, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!((m.term_view, m.focus), (TermView::Shown, Focus::Output));
        send(&mut m, KeyCode::Char('j'), KeyModifiers::CONTROL);
        m.update(AppEvent::Pty(crate::term::PtyEvent::Exited(id, Some(0))));
        assert_eq!(m.focus, Focus::Output, "the shell ended: back up");
        assert!(!m.terminal_shown());
    }

    #[test]
    fn a_click_on_a_session_selects_it() {
        let mut m = sample(&["a"]);
        let _first = with_session(&mut m, "one");
        let _second = with_session(&mut m, "two");
        m.focus = Focus::Projects;
        let selected = m.card;
        let pane = ui::panes(m.screen, m.zoom, m.widths).sessions.unwrap();
        let rows: Vec<Option<usize>> = (pane.y..pane.bottom())
            .map(|row| ui::session_at(pane, &m, row))
            .collect();
        assert_eq!(rows[0], None, "the border");
        assert_eq!(rows[1], Some(0), "the first card's title line");
        let other = 1 - selected;
        let row = rows.iter().position(|r| *r == Some(other)).unwrap();
        m.update(AppEvent::Input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: pane.x + 3,
            row: pane.y + u16::try_from(row).unwrap(),
            modifiers: KeyModifiers::NONE,
        })));
        assert_eq!((m.focus, m.card), (Focus::Sessions, other));
        m.update(AppEvent::Input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: pane.x + 3,
            row: pane.bottom() - 2,
            modifiers: KeyModifiers::NONE,
        })));
        assert_eq!(m.card, other, "below the cards: only the focus");
        let again = (pane.y..pane.bottom())
            .find(|row| ui::session_at(pane, &m, *row) == Some(m.card))
            .unwrap();
        m.update(AppEvent::Input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: pane.x + 3,
            row: again,
            modifiers: KeyModifiers::NONE,
        })));
        assert_eq!(
            m.focus,
            Focus::Output,
            "a click on the selected card enters it"
        );
    }

    #[test]
    fn recent_projects_lead_and_the_rest_fold_behind_a_line() {
        let names =
            |m: &Model| -> Vec<String> { m.visible().iter().map(|p| p.name.clone()).collect() };
        let mut m = sample(&["a", "b", "c"]);
        assert!(m.rest().is_none(), "no recent project: all of them show");
        m.selected = 1;
        let _writes = with_session(&mut m, "s");
        assert_eq!((names(&m), m.selected), (vec!["b".to_owned()], 0));
        let rest = m.rest().unwrap();
        assert_eq!((rest.at, rest.count), (1, 2));
        let pane = ui::panes(m.screen, m.zoom, m.widths).projects.unwrap();
        let click = |row| {
            AppEvent::Input(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: pane.x + 3,
                row,
                modifiers: KeyModifiers::NONE,
            }))
        };
        m.update(click(pane.y + 3));
        assert_eq!(
            (names(&m), m.selected),
            (vec!["b".into(), "a".into(), "c".into()], 0)
        );
        m.update(click(pane.y + 5));
        assert_eq!(m.selected, 2, "the row below the rest line");
        send(&mut m, KeyCode::Char('e'), KeyModifiers::NONE);
        assert_eq!((names(&m), m.selected), (vec!["b".to_owned()], 0));
        m.filter = "c".into();
        assert_eq!(names(&m), ["c"], "the search finds folded projects");
        assert!(m.rest().is_none());
    }

    #[test]
    fn the_recent_group_holds_five_projects_running_ones_first() {
        let all = ["a", "b", "c", "d", "e", "f", "g"];
        let mut m = sample(&all);
        let mut writes = Vec::new();
        for (row, minutes) in (0..all.len()).zip(0u64..) {
            m.selected = row;
            m.show_rest = true;
            writes.push(with_session(&mut m, "s"));
            let card = m.cards.last_mut().unwrap();
            card.started += std::time::Duration::from_secs(60 * minutes);
            card.state = crate::app::sessions::State::Wrapped;
        }
        m.show_rest = false;
        let names =
            |m: &Model| -> Vec<String> { m.visible().iter().map(|p| p.name.clone()).collect() };
        assert_eq!(names(&m), ["c", "d", "e", "f", "g"], "the five last used");
        assert_eq!(m.rest().unwrap().count, 2);
        m.cards[0].state = crate::app::sessions::State::NeedsYou;
        assert_eq!(
            names(&m),
            ["a", "d", "e", "f", "g"],
            "a runs: it goes first"
        );
        for card in &mut m.cards {
            card.state = crate::app::sessions::State::Working;
        }
        assert!(m.rest().is_none(), "every running project stays, past five");
    }

    #[test]
    fn a_forwarded_click_is_relative_to_the_agent_screen_below_the_strip() {
        let mut m = sample(&["a"]);
        let (id, writes) = with_session(&mut m, "s");
        m.update(AppEvent::Pty(crate::term::PtyEvent::Output(
            id,
            b"\x1b[?1000h\x1b[?1006h".to_vec(),
        )));
        let pane = ui::panes(m.screen, m.zoom, m.widths).output.unwrap();
        let strip = ui::STRIP_HEIGHT;
        let click = |row| {
            AppEvent::Input(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: pane.x + 1,
                row,
                modifiers: KeyModifiers::NONE,
            }))
        };
        m.update(click(pane.y + 1 + strip));
        assert_eq!(drain(&writes), b"\x1b[<0;1;1M");
        m.update(click(pane.y + 1));
        assert!(drain(&writes).is_empty(), "the strip is not the agent's");
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
