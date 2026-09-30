//! The event loop: owns the terminal, reads input, renders, quits.
//!
//! The terminal is restored by [`TerminalGuard`] on every exit path and by
//! ratatui's panic hook on a panic. Rendering happens only after an input
//! event, never on a timer.

use std::io;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::ui::{self, theme::Theme};

/// Restores the host terminal (raw mode off, main screen) when dropped.
#[derive(Debug)]
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// Runs the TUI until the user quits.
///
/// Enters raw mode and the alternate screen (ratatui also installs a panic
/// hook that restores both), then draws and waits for input in turn.
///
/// # Arguments
///
/// * `theme` - Colours for the whole screen.
///
/// # Errors
///
/// Returns the I/O error if the terminal cannot be set up, drawn to or read
/// from.
pub(crate) fn run(theme: Theme) -> io::Result<()> {
    let _guard = TerminalGuard;
    let mut terminal = ratatui::try_init()?;
    // ponytail: input is read on the UI thread; the AppEvent channel of
    // ARCHITECTURE §8 replaces this once a second event source exists.
    loop {
        terminal.draw(|frame| ui::draw(frame, theme))?;
        if let Event::Key(key) = event::read()?
            && is_quit(key)
        {
            return Ok(());
        }
    }
}

/// Returns whether `key` is a quit key (`q` or `ctrl-c`, DESIGN §8.1).
#[must_use]
fn is_quit(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && match key.code {
            KeyCode::Char('q') => key.modifiers.is_empty(),
            KeyCode::Char('c') => key.modifiers == KeyModifiers::CONTROL,
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quits_only_on_q_and_ctrl_c_presses() {
        let press = |code, modifiers| KeyEvent::new(code, modifiers);
        let cases = [
            (press(KeyCode::Char('q'), KeyModifiers::NONE), true),
            (press(KeyCode::Char('c'), KeyModifiers::CONTROL), true),
            (press(KeyCode::Char('Q'), KeyModifiers::SHIFT), false),
            (press(KeyCode::Char('q'), KeyModifiers::CONTROL), false),
            (press(KeyCode::Char('c'), KeyModifiers::NONE), false),
            (press(KeyCode::Esc, KeyModifiers::NONE), false),
            (
                KeyEvent::new_with_kind(
                    KeyCode::Char('q'),
                    KeyModifiers::NONE,
                    KeyEventKind::Release,
                ),
                false,
            ),
        ];
        for (key, want) in cases {
            assert_eq!(is_quit(key), want, "{key:?}");
        }
    }
}
