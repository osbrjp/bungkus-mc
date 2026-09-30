//! The single source of key bindings (DESIGN §8).
//!
//! [`BINDINGS`] drives key dispatch, the help overlay and the getah bar's
//! hints, so a key cannot work without being documented. Dialogs and the
//! first-run wizard handle their own few keys and list them in their hint
//! line; INTERACT passes every key to the agent.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Where a binding applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// NORMAL mode, whichever list pane has focus.
    Global,
    /// The projects pane has focus.
    Projects,
    /// The sessions pane has focus.
    Sessions,
}

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Focus the pane to the left.
    PrevPane,
    /// Focus the pane to the right.
    NextPane,
    /// Move the selection down one row.
    Down,
    /// Move the selection up one row.
    Up,
    /// Select the first row.
    First,
    /// Select the last row.
    Last,
    /// Move down half a page.
    HalfDown,
    /// Move up half a page.
    HalfUp,
    /// Start filtering the focused list.
    Filter,
    /// Open the settings screen on the workspace field.
    Workspace,
    /// Open the settings screen.
    Settings,
    /// Show the help overlay.
    Help,
    /// Clear and redraw the whole screen.
    Redraw,
    /// Quit mc.
    Quit,
    /// Focus the sessions pane of the selected project.
    OpenProject,
    /// Focus the output pane on the selected session (INTERACT).
    Interact,
    /// Open the `n` picker for the selected project.
    NewSession,
    /// Stop the selected session (with a confirm).
    Stop,
    /// Toggle the zoomed output pane.
    Zoom,
    /// Jump to the next session that needs you, across projects.
    NextNeedsYou,
}

/// One key, or `gg`-style double press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    /// A single key with exact modifiers.
    Press(KeyCode, KeyModifiers),
    /// The same character pressed twice (only `gg`).
    Twice(char),
}

/// A key binding with its documentation.
#[derive(Debug)]
pub(crate) struct Binding {
    /// Every key that triggers the action.
    pub keys: &'static [Key],
    /// How the keys read in the help overlay.
    pub label: &'static str,
    /// The action.
    pub action: Action,
    /// Help overlay text.
    pub help: &'static str,
    /// Short getah-bar hint for the focused pane, if it has one.
    pub hint: Option<&'static str>,
    /// Where the binding applies.
    pub scope: Scope,
}

/// Shorthand for a key without modifiers.
const fn k(code: KeyCode) -> Key {
    Key::Press(code, KeyModifiers::NONE)
}

/// Shorthand for a plain character key.
const fn c(ch: char) -> Key {
    k(KeyCode::Char(ch))
}

/// Shorthand for a ctrl chord.
const fn ctrl(ch: char) -> Key {
    Key::Press(KeyCode::Char(ch), KeyModifiers::CONTROL)
}

/// Every binding, grouped by scope in help order.
pub(crate) const BINDINGS: &[Binding] = &[
    Binding {
        keys: &[k(KeyCode::Enter)],
        label: "enter",
        action: Action::OpenProject,
        help: "its sessions",
        hint: Some("enter → sessions"),
        scope: Scope::Projects,
    },
    Binding {
        keys: &[c('n')],
        label: "n",
        action: Action::NewSession,
        help: "new session",
        hint: Some("n new"),
        scope: Scope::Projects,
    },
    Binding {
        keys: &[k(KeyCode::Enter)],
        label: "enter l → tab",
        action: Action::Interact,
        help: "→ agent",
        hint: Some("enter → agent (ctrl-\\ back)"),
        scope: Scope::Sessions,
    },
    Binding {
        keys: &[c('n')],
        label: "n",
        action: Action::NewSession,
        help: "new session",
        hint: Some("n new"),
        scope: Scope::Sessions,
    },
    Binding {
        keys: &[c('x')],
        label: "x",
        action: Action::Stop,
        help: "stop (confirm)",
        hint: Some("x stop"),
        scope: Scope::Sessions,
    },
    Binding {
        keys: &[
            c('h'),
            k(KeyCode::Left),
            Key::Press(KeyCode::BackTab, KeyModifiers::SHIFT),
        ],
        label: "h ← shift-tab",
        action: Action::PrevPane,
        help: "pane left",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('l'), k(KeyCode::Right), k(KeyCode::Tab)],
        label: "l → tab",
        action: Action::NextPane,
        help: "pane right",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('j'), k(KeyCode::Down)],
        label: "j ↓",
        action: Action::Down,
        help: "down",
        hint: Some("j/k"),
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('k'), k(KeyCode::Up)],
        label: "k ↑",
        action: Action::Up,
        help: "up",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[Key::Twice('g'), k(KeyCode::Home)],
        label: "gg home",
        action: Action::First,
        help: "first",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[
            Key::Press(KeyCode::Char('G'), KeyModifiers::SHIFT),
            c('G'),
            k(KeyCode::End),
        ],
        label: "G end",
        action: Action::Last,
        help: "last",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[ctrl('d'), k(KeyCode::PageDown)],
        label: "ctrl-d pgdn",
        action: Action::HalfDown,
        help: "half page down",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[ctrl('u'), k(KeyCode::PageUp)],
        label: "ctrl-u pgup",
        action: Action::HalfUp,
        help: "half page up",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[
            c('!'),
            Key::Press(KeyCode::Char('!'), KeyModifiers::SHIFT),
            ctrl(']'),
        ],
        label: "! ctrl-]",
        action: Action::NextNeedsYou,
        help: "next needs you",
        hint: Some("! next"),
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('z')],
        label: "z",
        action: Action::Zoom,
        help: "zoom output",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('/')],
        label: "/",
        action: Action::Filter,
        help: "filter",
        hint: Some("/ filter"),
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('w')],
        label: "w",
        action: Action::Workspace,
        help: "workspace",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[c(',')],
        label: ",",
        action: Action::Settings,
        help: "settings",
        hint: Some(", settings"),
        scope: Scope::Global,
    },
    Binding {
        keys: &[
            ctrl('l'),
            Key::Press(KeyCode::Char('R'), KeyModifiers::SHIFT),
            c('R'),
        ],
        label: "ctrl-l R",
        action: Action::Redraw,
        help: "redraw",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('?'), Key::Press(KeyCode::Char('?'), KeyModifiers::SHIFT)],
        label: "?",
        action: Action::Help,
        help: "this help",
        hint: Some("? help"),
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('q'), ctrl('c')],
        label: "q ctrl-c",
        action: Action::Quit,
        help: "quit",
        hint: None,
        scope: Scope::Global,
    },
];

/// Result of looking up a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lookup {
    /// The key triggers this action.
    Action(Action),
    /// The key is the first half of a double press; wait for the next.
    Pending(char),
    /// Nothing is bound to the key here.
    Unbound,
}

/// Looks up `key` for the focused pane's `scope`, then the global scope.
///
/// # Arguments
///
/// * `scope`   - The focused pane's scope.
/// * `key`     - The key event.
/// * `pending` - The first half of a double press, if one is waiting.
#[must_use]
pub(crate) fn lookup(scope: Scope, key: KeyEvent, pending: Option<char>) -> Lookup {
    let pressed = Key::Press(key.code, key.modifiers);
    let in_scope = |b: &&Binding| b.scope == scope || b.scope == Scope::Global;
    for binding in BINDINGS.iter().filter(in_scope) {
        for bound in binding.keys {
            match *bound {
                Key::Twice(ch) if pending == Some(ch) && pressed == c(ch) => {
                    return Lookup::Action(binding.action);
                }
                Key::Twice(ch) if pending.is_none() && pressed == c(ch) => {
                    return Lookup::Pending(ch);
                }
                Key::Press(..) if *bound == pressed && pending.is_none() => {
                    return Lookup::Action(binding.action);
                }
                Key::Twice(_) | Key::Press(..) => {}
            }
        }
    }
    Lookup::Unbound
}

/// Returns the getah-bar hints for `scope`, pane-specific ones first.
#[must_use]
pub(crate) fn hints(scope: Scope) -> Vec<&'static str> {
    let mut out: Vec<&str> = BINDINGS
        .iter()
        .filter(|b| b.scope == scope)
        .filter_map(|b| b.hint)
        .collect();
    out.extend(
        BINDINGS
            .iter()
            .filter(|b| b.scope == Scope::Global)
            .filter_map(|b| b.hint),
    );
    out
}

/// Returns the `(label, help)` rows the help overlay shows for `scope`.
#[must_use]
pub(crate) fn help_rows(scope: Scope) -> Vec<(&'static str, &'static str)> {
    BINDINGS
        .iter()
        .filter(|b| b.scope == scope)
        .map(|b| (b.label, b.help))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_key_is_bound_twice_within_a_pane() {
        for scope in [Scope::Projects, Scope::Sessions] {
            let mut seen: Vec<Key> = Vec::new();
            let bindings = BINDINGS
                .iter()
                .filter(|b| b.scope == scope || b.scope == Scope::Global);
            for key in bindings.flat_map(|b| b.keys) {
                assert!(!seen.contains(key), "{scope:?}: {key:?} bound twice");
                seen.push(*key);
            }
        }
    }

    #[test]
    fn every_binding_is_documented() {
        for b in BINDINGS {
            assert!(!b.help.is_empty() && !b.label.is_empty(), "{:?}", b.action);
            assert!(!b.keys.is_empty(), "{:?}", b.action);
        }
    }

    #[test]
    fn every_vim_motion_has_an_arrow_twin() {
        let twins = [
            (Action::PrevPane, KeyCode::Left),
            (Action::NextPane, KeyCode::Right),
            (Action::Down, KeyCode::Down),
            (Action::Up, KeyCode::Up),
            (Action::First, KeyCode::Home),
            (Action::Last, KeyCode::End),
            (Action::HalfDown, KeyCode::PageDown),
            (Action::HalfUp, KeyCode::PageUp),
        ];
        for (action, arrow) in twins {
            let b = BINDINGS.iter().find(|b| b.action == action).unwrap();
            assert!(b.keys.contains(&k(arrow)), "{action:?} lacks {arrow:?}");
        }
    }

    #[test]
    fn looks_up_keys_and_double_presses() {
        let key = |code, mods| KeyEvent::new(code, mods);
        let cases = [
            (
                Scope::Projects,
                key(KeyCode::Char('j'), KeyModifiers::NONE),
                None,
                Lookup::Action(Action::Down),
            ),
            (
                Scope::Projects,
                key(KeyCode::Enter, KeyModifiers::NONE),
                None,
                Lookup::Action(Action::OpenProject),
            ),
            (
                Scope::Sessions,
                key(KeyCode::Enter, KeyModifiers::NONE),
                None,
                Lookup::Action(Action::Interact),
            ),
            (
                Scope::Sessions,
                key(KeyCode::Char('g'), KeyModifiers::NONE),
                None,
                Lookup::Pending('g'),
            ),
            (
                Scope::Sessions,
                key(KeyCode::Char('g'), KeyModifiers::NONE),
                Some('g'),
                Lookup::Action(Action::First),
            ),
            (
                Scope::Sessions,
                key(KeyCode::Char('j'), KeyModifiers::NONE),
                Some('g'),
                Lookup::Unbound,
            ),
            (
                Scope::Projects,
                key(KeyCode::Char('G'), KeyModifiers::SHIFT),
                None,
                Lookup::Action(Action::Last),
            ),
            (
                Scope::Projects,
                key(KeyCode::Char('c'), KeyModifiers::CONTROL),
                None,
                Lookup::Action(Action::Quit),
            ),
            (
                Scope::Projects,
                key(KeyCode::Char(','), KeyModifiers::NONE),
                None,
                Lookup::Action(Action::Settings),
            ),
        ];
        for (scope, event, pending, want) in cases {
            assert_eq!(
                lookup(scope, event, pending),
                want,
                "{scope:?} {event:?} {pending:?}"
            );
        }
    }
}
