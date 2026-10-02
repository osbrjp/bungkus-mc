//! The `n` picker: agent, model, name and prompt for a new session
//! (DESIGN §5.5).
//!
//! The agent starts on the default agent from settings. The name stays
//! empty unless the user types one, so the agent names the session itself.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::agent::Kind;

/// The picker row in focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    /// Agent choice.
    Agent,
    /// Model choice.
    Model,
    /// Session name.
    Name,
    /// Start prompt.
    Prompt,
}

/// What a key did to the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Still choosing.
    Continue,
    /// Closed without starting.
    Cancel,
    /// Start a session with these choices.
    Start,
}

/// The picker's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Picker {
    /// Row in focus.
    pub row: Row,
    /// Chosen agent.
    pub agent: Kind,
    /// Index into `agent.models()`.
    pub model: usize,
    /// Name as typed; empty leaves the naming to the agent.
    pub name: String,
    /// Prompt as typed.
    pub prompt: String,
    /// Which agents are installed, in `Kind::ALL` order.
    pub installed: [bool; 2],
    /// The project the session starts in, for the title.
    pub project: String,
}

impl Picker {
    /// Opens the picker on the prompt row with `agent` preselected.
    #[must_use]
    pub(crate) fn new(agent: Kind, installed: [bool; 2], project: String) -> Self {
        Self {
            row: Row::Prompt,
            agent,
            model: 0,
            name: String::new(),
            prompt: String::new(),
            installed,
            project,
        }
    }

    /// Returns the chosen model id, `None` for the agent's default.
    #[must_use]
    pub(crate) fn model_id(&self) -> Option<String> {
        let models = self.agent.models();
        models
            .get(self.model)
            .filter(|m| **m != "default")
            .map(|m| (*m).to_owned())
    }

    /// Handles one key press: `tab`/`↓` and `shift-tab`/`↑` move between
    /// rows, `←`/`→` change agent and model, typing edits name and prompt,
    /// `enter` starts, `esc` cancels. On the agent and model rows, where
    /// nothing is typed, `j`/`k` move between rows and `h`/`l` change.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Enter => return Outcome::Start,
            KeyCode::Tab | KeyCode::Down => self.row = self.step(true),
            KeyCode::BackTab | KeyCode::Up => self.row = self.step(false),
            KeyCode::Left | KeyCode::Right => self.change(key.code == KeyCode::Right),
            KeyCode::Char(c @ ('j' | 'k' | 'h' | 'l'))
                if !ctrl && matches!(self.row, Row::Agent | Row::Model) =>
            {
                match c {
                    'j' => self.row = self.step(true),
                    'k' => self.row = self.step(false),
                    other => self.change(other == 'l'),
                }
            }
            KeyCode::Char('u') if ctrl => self.edit(String::clear),
            KeyCode::Char(c) if !ctrl => self.edit(|s| s.push(c)),
            KeyCode::Backspace => self.edit(|s| {
                s.pop();
            }),
            _ => {}
        }
        Outcome::Continue
    }

    /// Returns the next (or previous) row, wrapping round.
    const fn step(&self, forward: bool) -> Row {
        match (self.row, forward) {
            (Row::Agent, true) | (Row::Name, false) => Row::Model,
            (Row::Model, true) | (Row::Prompt, false) => Row::Name,
            (Row::Name, true) | (Row::Agent, false) => Row::Prompt,
            (Row::Prompt, true) | (Row::Model, false) => Row::Agent,
        }
    }

    /// Changes the agent (to an installed one) or the model.
    fn change(&mut self, forward: bool) {
        match self.row {
            Row::Agent => {
                let other = match self.agent {
                    Kind::Claude => Kind::Codex,
                    Kind::Codex => Kind::Claude,
                };
                let none_installed = !self.installed.contains(&true);
                if self.installed[other as usize] || none_installed {
                    self.agent = other;
                    self.model = 0;
                }
            }
            Row::Model => {
                let n = self.agent.models().len();
                self.model = (self.model + if forward { 1 } else { n - 1 }) % n;
            }
            Row::Name | Row::Prompt => {}
        }
    }

    /// Applies a text edit to the focused text row.
    fn edit(&mut self, f: impl FnOnce(&mut String)) {
        match self.row {
            Row::Name => f(&mut self.name),
            Row::Prompt => f(&mut self.prompt),
            Row::Agent | Row::Model => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(p: &mut Picker, code: KeyCode) -> Outcome {
        p.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(p: &mut Picker, text: &str) {
        for c in text.chars() {
            press(p, KeyCode::Char(c));
        }
    }

    #[test]
    fn the_name_stays_empty_until_the_user_types_one() {
        let mut p = Picker::new(Kind::Claude, [true, true], "kedai-web".into());
        typed(&mut p, "fix the flaky date test");
        assert_eq!(p.name, "", "the agent names the session");
        press(&mut p, KeyCode::BackTab);
        assert_eq!(p.row, Row::Name);
        typed(&mut p, "dates");
        press(&mut p, KeyCode::Tab);
        typed(&mut p, " more");
        assert_eq!((p.name.as_str(), p.prompt.len()), ("dates", 28));
        assert_eq!(press(&mut p, KeyCode::Enter), Outcome::Start);
    }

    #[test]
    fn changes_agent_only_to_installed_ones_and_resets_the_model() {
        let mut p = Picker::new(Kind::Claude, [true, false], String::new());
        p.row = Row::Model;
        press(&mut p, KeyCode::Right);
        assert_eq!(p.model_id().as_deref(), Some("haiku"));
        p.row = Row::Agent;
        press(&mut p, KeyCode::Right);
        assert_eq!(p.agent, Kind::Claude, "codex is not installed");
        p.installed = [true, true];
        press(&mut p, KeyCode::Right);
        assert_eq!((p.agent, p.model_id()), (Kind::Codex, None));
        assert_eq!(press(&mut p, KeyCode::Esc), Outcome::Cancel);
    }
}
