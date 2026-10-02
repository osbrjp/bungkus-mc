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
    /// Search the projects (FILTER mode).
    Filter,
    /// Jump to a project by its number (digits; two when there are more
    /// than nine projects).
    Jump,
    /// Create a new project in the workspace.
    NewProject,
    /// Move the selected project's folder to the Trash (with a confirm).
    TrashProject,
    /// Put the project last moved to the Trash back.
    UndoTrash,
    /// Start or end a line selection of projects (`V`), for `d`/`dd`.
    Visual,
    /// Remove the selected project's unused git worktrees (with a confirm).
    CleanWorktrees,
    /// Install a newer release and restart mc on it (`U`).
    Update,
    /// Show or hide the projects that are not recent.
    ToggleRest,
    /// Open the selected project in the user's editor (vim in a popup).
    Editor,
    /// Show or hide the terminal pane below the output pane.
    Terminal,
    /// Start a quick session at the workspace root (a popup).
    QuickSession,
    /// Move the selected quick session into a project.
    MoveQuick,
    /// Make a new project for the selected quick session.
    MakeProject,
    /// Focus pane 1, 2 or 3 (projects, sessions, output) with cmd, alt or
    /// ctrl plus the digit, whichever the terminal passes on.
    Pane(u8),
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
    /// Resume the selected finished session.
    Resume,
    /// Forget the selected finished session (with a confirm).
    Forget,
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

/// The cmd / alt / ctrl chords of digit `ch`.
const fn pane_keys(ch: char) -> [Key; 3] {
    [
        Key::Press(KeyCode::Char(ch), KeyModifiers::SUPER),
        Key::Press(KeyCode::Char(ch), KeyModifiers::ALT),
        Key::Press(KeyCode::Char(ch), KeyModifiers::CONTROL),
    ]
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
        hint: Some("enter → agent"),
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
        keys: &[c('a')],
        label: "a",
        action: Action::NewProject,
        help: "new project",
        hint: None,
        scope: Scope::Projects,
    },
    Binding {
        keys: &[Key::Press(KeyCode::Char('V'), KeyModifiers::SHIFT), c('V')],
        label: "V",
        action: Action::Visual,
        help: "select lines",
        hint: None,
        scope: Scope::Projects,
    },
    Binding {
        keys: &[c('u')],
        label: "u",
        action: Action::UndoTrash,
        help: "undo dd",
        hint: None,
        scope: Scope::Projects,
    },
    Binding {
        keys: &[Key::Twice('d')],
        label: "dd",
        action: Action::TrashProject,
        help: "to Trash",
        hint: None,
        scope: Scope::Projects,
    },
    Binding {
        keys: &[c('c')],
        label: "c",
        action: Action::CleanWorktrees,
        help: "clean worktrees",
        hint: Some("c clean"),
        scope: Scope::Projects,
    },
    Binding {
        keys: &[c('c')],
        label: "c",
        action: Action::CleanWorktrees,
        help: "clean worktrees",
        hint: None,
        scope: Scope::Sessions,
    },
    Binding {
        keys: &[c('e')],
        label: "e",
        action: Action::ToggleRest,
        help: "more projects",
        hint: Some("e more"),
        scope: Scope::Projects,
    },
    Binding {
        keys: &[c('m')],
        label: "m",
        action: Action::MoveQuick,
        help: "quick → project",
        hint: None,
        scope: Scope::Sessions,
    },
    Binding {
        keys: &[c('p')],
        label: "p",
        action: Action::MakeProject,
        help: "quick → new project",
        hint: None,
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
        keys: &[c('r')],
        label: "r",
        action: Action::Resume,
        help: "resume · past",
        hint: None,
        scope: Scope::Sessions,
    },
    Binding {
        keys: &[c('r')],
        label: "r",
        action: Action::Resume,
        help: "past sessions",
        hint: None,
        scope: Scope::Projects,
    },
    Binding {
        keys: &[c('d')],
        label: "d",
        action: Action::Forget,
        help: "forget (confirm)",
        hint: None,
        scope: Scope::Sessions,
    },
    Binding {
        keys: &[
            c('h'),
            k(KeyCode::Left),
            Key::Press(KeyCode::BackTab, KeyModifiers::SHIFT),
            ctrl('h'),
        ],
        label: "h ← shift-tab ctrl-h",
        action: Action::PrevPane,
        help: "pane left",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('l'), k(KeyCode::Right), k(KeyCode::Tab), ctrl('l')],
        label: "l → tab ctrl-l",
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
        hint: None,
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
        hint: None,
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
        help: "search projects",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[
            c('1'),
            c('2'),
            c('3'),
            c('4'),
            c('5'),
            c('6'),
            c('7'),
            c('8'),
            c('9'),
            c('0'),
        ],
        label: "1-9",
        action: Action::Jump,
        help: "jump to project",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[Key::Press(KeyCode::Char('U'), KeyModifiers::SHIFT), c('U')],
        label: "U",
        action: Action::Update,
        help: "update+restart",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[Key::Press(KeyCode::Char('N'), KeyModifiers::SHIFT), c('N')],
        label: "N",
        action: Action::QuickSession,
        help: "quick session",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &pane_keys('1'),
        label: "cmd/alt-1",
        action: Action::Pane(1),
        help: "projects pane",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &pane_keys('2'),
        label: "cmd/alt-2",
        action: Action::Pane(2),
        help: "sessions pane",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &pane_keys('3'),
        label: "cmd/alt-3",
        action: Action::Pane(3),
        help: "output pane",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('o')],
        label: "o",
        action: Action::Editor,
        help: "open in editor",
        hint: Some("o edit"),
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('t')],
        label: "t",
        action: Action::Terminal,
        help: "terminal on/off",
        hint: Some("t term"),
        scope: Scope::Global,
    },
    Binding {
        keys: &[c('w')],
        label: "w",
        action: Action::Workspace,
        help: "workspaces",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[c(',')],
        label: ",",
        action: Action::Settings,
        help: "settings",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[Key::Press(KeyCode::Char('R'), KeyModifiers::SHIFT), c('R')],
        label: "R",
        action: Action::Redraw,
        help: "redraw",
        hint: None,
        scope: Scope::Global,
    },
    Binding {
        keys: &[
            c('?'),
            Key::Press(KeyCode::Char('?'), KeyModifiers::SHIFT),
            c(' '),
        ],
        label: "? space",
        action: Action::Help,
        help: "this help",
        hint: Some("? keys"),
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

/// A heading of the key menu (`?`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    /// Moving between panes, rows and projects.
    Navigate,
    /// Starting, entering, stopping and resuming sessions.
    Sessions,
    /// Creating and removing projects.
    Projects,
    /// Workspaces, settings and mc itself.
    App,
}

impl Group {
    /// Every group, in key-menu order.
    pub(crate) const ALL: [Self; 4] = [Self::Navigate, Self::Sessions, Self::Projects, Self::App];

    /// Returns the heading the key menu shows.
    #[must_use]
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Navigate => "navigate",
            Self::Sessions => "sessions",
            Self::Projects => "projects",
            Self::App => "workspace & app",
        }
    }
}

impl Action {
    /// Returns the key-menu group the action belongs to.
    #[must_use]
    pub(crate) const fn group(self) -> Group {
        match self {
            Self::PrevPane
            | Self::NextPane
            | Self::Down
            | Self::Up
            | Self::First
            | Self::Last
            | Self::HalfDown
            | Self::HalfUp
            | Self::Filter
            | Self::Jump
            | Self::Pane(_)
            | Self::NextNeedsYou
            | Self::Zoom
            | Self::OpenProject => Group::Navigate,
            Self::Interact
            | Self::NewSession
            | Self::Stop
            | Self::Resume
            | Self::Forget
            | Self::QuickSession
            | Self::MoveQuick
            | Self::MakeProject => Group::Sessions,
            Self::NewProject
            | Self::TrashProject
            | Self::UndoTrash
            | Self::Visual
            | Self::Editor
            | Self::ToggleRest
            | Self::CleanWorktrees => Group::Projects,
            Self::Workspace
            | Self::Settings
            | Self::Help
            | Self::Redraw
            | Self::Update
            | Self::Terminal
            | Self::Quit => Group::App,
        }
    }
}

/// Returns the key menu for `scope`: per [`Group`], the `(label, help)`
/// rows of that pane's and the global bindings, skipping empty groups.
#[must_use]
pub(crate) fn help_groups(scope: Scope) -> Vec<(Group, Vec<(&'static str, &'static str)>)> {
    Group::ALL
        .into_iter()
        .map(|group| {
            let rows = BINDINGS
                .iter()
                .filter(|b| {
                    (b.scope == scope || b.scope == Scope::Global) && b.action.group() == group
                })
                .map(|b| (b.label, b.help))
                .collect();
            (group, rows)
        })
        .filter(|(_, rows): &(Group, Vec<_>)| !rows.is_empty())
        .collect()
}

/// Keys that work in the agent pane (INTERACT) and the quick popup, which
/// pass everything else to the agent; listed in the key menu.
pub(crate) const AGENT_KEYS: [(&str, &str); 4] = [
    ("ctrl-\\", "leave · menu"),
    ("ctrl-h", "to sessions"),
    ("ctrl-m", "move (popup)"),
    ("cmd/alt-1..3", "focus a pane"),
];

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
