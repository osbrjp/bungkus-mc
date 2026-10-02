//! The user's own tools next to the agents: the editor popup (`o`) and the
//! terminal pane (`t`, one shell per project), each a program in its own
//! PTY and mc's own emulator; no other program (tmux, a terminal app) is
//! involved.
//!
//! A tool is not a session: it has no card, no hooks and no usage, is
//! never saved to `sessions.json`, and ends with mc (its PTY closes). This
//! module owns their keys and visibility; the event loop spawns them.

use std::ops::ControlFlow;
use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::model::{Cmd, Model};
use crate::term::SessionId;
use crate::term::keys;
use crate::term::session::Session;

/// The editors that run in mc's popup; any other `$VISUAL`/`$EDITOR` is
/// started on its own, as a window of the desktop.
const VIM: [&str; 3] = ["vi", "vim", "nvim"];

/// A program of the user's (not an agent) that mc runs in a PTY.
#[derive(Debug)]
pub(crate) struct Tool {
    /// The id its PTY events carry.
    pub id: SessionId,
    /// Its PTY and emulator.
    pub pty: Session,
}

/// How the terminal pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TermView {
    /// Not drawn; the shells keep running.
    #[default]
    Hidden,
    /// The selected project's shell, if it has one, is drawn below the
    /// output pane; keys go to mc.
    Shown,
    /// Drawn, and every key goes to the shell.
    Focused,
}

/// How `o` opens a project folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Opener {
    /// Run this command with the folder in the popup (vim).
    Popup(Vec<String>),
    /// Start this command with the folder and leave it alone (an IDE, or
    /// the desktop's own opener).
    Detached(Vec<String>),
}

/// The editors the wizard and the settings screen look for on `PATH`, in
/// the order they are offered: the popup ones, then desktop ones that take
/// a folder as their argument.
const KNOWN: [&str; 6] = ["nvim", "vim", "vi", "code", "cursor", "zed"];

/// Returns the desktop's opener: `open` on macOS, `xdg-open` elsewhere.
#[must_use]
pub(crate) const fn desktop_opener() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Returns the editors to offer as the `editor` setting: the user's own
/// `$VISUAL`/`$EDITOR` (`env`) first, then every known editor that
/// `installed` finds.
#[must_use]
pub(crate) fn editors(env: Option<&str>, installed: impl Fn(&str) -> bool) -> Vec<String> {
    let env = env.map(str::trim).filter(|e| !e.is_empty());
    let known = KNOWN
        .into_iter()
        .filter(|name| Some(*name) != env && installed(name));
    env.into_iter().chain(known).map(str::to_owned).collect()
}

/// Returns how to open a project for a user whose editor (the `editor`
/// setting, else `$VISUAL`/`$EDITOR`) is `editor`: vim in the popup,
/// another editor on its own, and with none set the first of
/// `nvim`/`vim`/`vi` that `installed` finds, else the desktop's opener
/// ([`desktop_opener`]).
///
/// The value is split on whitespace into an argument vector; it never
/// goes through a shell.
// ponytail: only vi/vim/nvim get the popup; another terminal editor
// (nano, hx) starts without a terminal. Widen `VIM` when one is asked for.
#[must_use]
pub(crate) fn opener(editor: Option<&str>, installed: impl Fn(&str) -> bool) -> Opener {
    let argv: Vec<String> = editor
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let Some(program) = argv.first() else {
        if let Some(vim) = VIM.iter().rev().find(|name| installed(name)) {
            return Opener::Popup(vec![(*vim).to_owned()]);
        }
        return Opener::Detached(vec![desktop_opener().to_owned()]);
    };
    if VIM.contains(&program.rsplit('/').next().unwrap_or_default()) {
        Opener::Popup(argv)
    } else {
        Opener::Detached(argv)
    }
}

impl Model {
    /// Returns the emulators of the running tools.
    pub(crate) fn tools(&self) -> impl Iterator<Item = &Session> {
        let shells = self.shells.iter().map(|(_, t)| t);
        self.editor.iter().chain(shells).map(|t| &t.pty)
    }

    /// Returns the running tools, to advance or resize their emulators.
    fn all_tools_mut(&mut self) -> impl Iterator<Item = &mut Tool> {
        let shells = self.shells.iter_mut().map(|(_, t)| t);
        self.editor.iter_mut().chain(shells)
    }

    /// Returns the emulators of the running tools, to flush them.
    pub(crate) fn tools_mut(&mut self) -> impl Iterator<Item = &mut Session> {
        self.all_tools_mut().map(|t| &mut t.pty)
    }

    /// Returns the tool with PTY id `id`, if it is one.
    pub(super) fn tool_mut(&mut self, id: SessionId) -> Option<&mut Session> {
        self.all_tools_mut()
            .find(|t| t.id == id)
            .map(|t| &mut t.pty)
    }

    /// Returns the folder the terminal pane belongs to now: the selected
    /// project, or the workspace root on a row that is no folder.
    fn shell_dir(&self) -> Option<&std::path::Path> {
        self.selected_project()
            .map(|p| p.path.as_path())
            .filter(|p| !p.as_os_str().is_empty())
            .or(self.root())
    }

    /// Returns the selected project's shell, if it has one running.
    #[must_use]
    pub(crate) fn shell(&self) -> Option<&Tool> {
        let dir = self.shell_dir()?;
        self.shells.iter().find(|(d, _)| d == dir).map(|(_, t)| t)
    }

    /// Returns whether the terminal pane is drawn: it is not hidden and
    /// the selected project has a shell.
    #[must_use]
    pub(crate) fn terminal_shown(&self) -> bool {
        self.term_view != TermView::Hidden && self.shell().is_some()
    }

    /// Forgets the tool with PTY id `id` when it exited: the popup closes,
    /// the project's terminal pane goes and the keys return to mc. A
    /// failed editor says so.
    ///
    /// # Returns
    ///
    /// Whether `id` was a tool (and not a session).
    pub(super) fn tool_exited(&mut self, id: SessionId, code: Option<u32>) -> bool {
        if self.editor.as_ref().is_some_and(|t| t.id == id) {
            self.editor = None;
            if code != Some(0) {
                self.message = Some("The editor ended with an error.".into());
            }
            true
        } else if let Some(at) = self.shells.iter().position(|(_, t)| t.id == id) {
            self.shells.remove(at);
            if self.term_view == TermView::Focused {
                self.term_view = TermView::Shown;
            }
            true
        } else {
            false
        }
    }

    /// Returns the tool that has the keys: the editor popup, else the
    /// selected project's shell while the terminal is focused.
    pub(super) fn keyed_tool(&mut self) -> Option<&mut Session> {
        if self.editor.is_some() {
            return self.editor.as_mut().map(|t| &mut t.pty);
        }
        if self.term_view != TermView::Focused {
            return None;
        }
        let dir = self.shell_dir()?.to_path_buf();
        let shell = self.shells.iter_mut().find(|(d, _)| *d == dir);
        shell.map(|(_, t)| &mut t.pty)
    }

    /// Handles a key while a tool has the keys. The editor gets every key
    /// (it closes when the editor quits). In the terminal the exit chord
    /// gives the keys back to mc, `ctrl-h` goes left to the sessions pane
    /// and `ctrl-k` up to the output pane (INTERACT); `ctrl-j` and `ctrl-l`
    /// have no pane to go to and stay the shell's (enter, clear screen).
    /// `ctrl-z` is swallowed: nothing could resume a suspended tool.
    ///
    /// # Returns
    ///
    /// Breaks with the command to run when a tool had the keys.
    pub(super) fn tool_key(&mut self, key: KeyEvent) -> ControlFlow<Option<Cmd>> {
        let terminal = self.editor.is_none();
        let ctrl = |ch| key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char(ch);
        let leave = terminal && self.exit_chord.matches(&key);
        let Some(pty) = self.keyed_tool() else {
            return ControlFlow::Continue(());
        };
        if leave {
            self.term_view = TermView::Shown;
        } else if terminal && (ctrl('h') || ctrl('k')) {
            self.term_view = TermView::Shown;
            self.focus_pane(if ctrl('h') { 2 } else { 3 });
        } else if !ctrl('z') {
            pty.scroll(alacritty_terminal::grid::Scroll::Bottom);
            pty.send(keys::encode(&key, pty.mode()));
        }
        ControlFlow::Break(None)
    }

    /// Returns the selected project's folder, when the row is one.
    fn project_dir(&self) -> Option<PathBuf> {
        self.selected_project()
            .filter(|p| !p.path.as_os_str().is_empty())
            .map(|p| p.path.clone())
    }

    /// Opens the selected project in the user's editor (`o`).
    pub(super) fn open_editor(&self) -> Option<Cmd> {
        self.project_dir().map(Cmd::OpenEditor)
    }

    /// Opens the selected project's folder with the desktop's opener (`O`).
    pub(super) fn open_folder(&self) -> Option<Cmd> {
        self.project_dir().map(Cmd::OpenFolder)
    }

    /// Shows or hides the terminal pane (`t`). Every project has its own
    /// shell: showing the pane gives the selected project's shell the keys,
    /// and starts one there (the workspace root on a row that is no
    /// folder) the first time and after it ended.
    pub(super) fn toggle_terminal(&mut self) -> Option<Cmd> {
        if self.terminal_shown() {
            self.term_view = TermView::Hidden;
            return None;
        }
        if self.shell().is_some() {
            self.term_view = TermView::Focused;
            return None;
        }
        self.shell_dir().map(|p| Cmd::OpenTerminal(p.to_path_buf()))
    }

    /// Returns the output pane's inner size now, which every session's PTY
    /// has: smaller while the terminal pane shows.
    #[must_use]
    pub(crate) fn output_size(&self) -> crate::term::session::Size {
        crate::ui::output_size(self.screen, self.zoom, self.widths, self.terminal_shown())
    }

    /// Returns the pane rectangles as drawn now on a screen of `area`: the
    /// terminal pane takes the lower third of the output pane while it
    /// shows.
    #[must_use]
    pub(crate) fn panes(&self, area: ratatui::layout::Rect) -> crate::ui::Panes {
        crate::ui::panes(area, self.zoom, self.widths).with_terminal(self.terminal_shown())
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{Event, MouseButton, MouseEvent, MouseEventKind};

    use crate::app::AppEvent;
    use crate::app::model::tests::{press, sample};
    use crate::term::PtyEvent;
    use crate::term::session::{Colors, Size};

    use super::*;

    fn tool(n: u128) -> (Tool, std::sync::mpsc::Receiver<Vec<u8>>) {
        let black = alacritty_terminal::vte::ansi::Rgb::default();
        let colors = Colors {
            fg: black,
            bg: black,
        };
        let (pty, writes) = Session::detached(Size { cols: 40, rows: 8 }, colors);
        let id = SessionId(uuid::Uuid::from_u128(n));
        (Tool { id, pty }, writes)
    }

    fn drain(writes: &std::sync::mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
        writes.try_iter().flatten().collect()
    }

    #[test]
    fn vim_opens_in_the_popup_and_anything_else_on_its_own() {
        let argv = |s: &str| s.split(' ').map(str::to_owned).collect::<Vec<_>>();
        let open = desktop_opener();
        for (editor, want) in [
            (Some("nvim"), Opener::Popup(argv("nvim"))),
            (
                Some("/usr/bin/vim -p"),
                Opener::Popup(argv("/usr/bin/vim -p")),
            ),
            (Some("vi"), Opener::Popup(argv("vi"))),
            (Some("code --wait"), Opener::Detached(argv("code --wait"))),
            (Some("gvim"), Opener::Detached(argv("gvim"))),
            (Some("  "), Opener::Detached(argv(open))),
            (None, Opener::Detached(argv(open))),
        ] {
            assert_eq!(opener(editor, |_| false), want, "{editor:?}");
        }
    }

    #[test]
    fn with_no_editor_set_an_installed_vim_opens_before_the_desktop_opener() {
        for (installed, want) in [
            (&["vi", "vim", "nvim"][..], "nvim"),
            (&["vi", "vim"][..], "vim"),
            (&["vi"][..], "vi"),
        ] {
            for editor in [None, Some("  ")] {
                assert_eq!(
                    opener(editor, |name| installed.contains(&name)),
                    Opener::Popup(vec![want.to_owned()]),
                    "{installed:?}"
                );
            }
        }
        assert_eq!(
            opener(Some("code"), |_| true),
            Opener::Detached(vec!["code".to_owned()])
        );
    }

    #[test]
    fn offers_the_users_own_editor_first_then_the_installed_known_ones() {
        let installed = |name: &str| ["vim", "code", "hx"].contains(&name);
        for (env, want) in [
            (None, &["vim", "code"][..]),
            (Some(" "), &["vim", "code"][..]),
            (Some("hx"), &["hx", "vim", "code"][..]),
            (Some("code"), &["code", "vim"][..]),
        ] {
            assert_eq!(editors(env, installed), want, "{env:?}");
        }
        assert!(editors(None, |_| false).is_empty());
    }

    #[test]
    fn capital_o_asks_for_the_desktop_opener_on_the_project_folder() {
        let mut m = sample(&["a"]);
        let path = m.selected_project().unwrap().path.clone();
        assert_eq!(
            m.update(press(KeyCode::Char('O'))),
            Some(Cmd::OpenFolder(path))
        );
    }

    #[test]
    fn o_asks_for_the_editor_and_its_popup_takes_every_key_until_it_quits() {
        let mut m = sample(&["a", "b"]);
        let path = m.selected_project().unwrap().path.clone();
        assert_eq!(
            m.update(press(KeyCode::Char('o'))),
            Some(Cmd::OpenEditor(path))
        );
        let (editor, writes) = tool(1);
        let id = editor.id;
        m.editor = Some(editor);
        assert!(crate::ui::tests::render(&mut m, 120, 40).contains(" EDITOR "));
        m.update(press(KeyCode::Char('q')));
        m.update(AppEvent::Input(Event::Key(KeyEvent::new(
            KeyCode::Char('\\'),
            KeyModifiers::CONTROL,
        ))));
        assert_eq!(drain(&writes), b"q\x1c", "the exit chord is vim's too");
        m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(0))));
        assert!(m.editor.is_none() && m.message.is_none());
    }

    #[test]
    fn t_shows_and_hides_the_terminal_and_the_exit_chord_returns_the_keys() {
        let mut m = sample(&["a", "b"]);
        let path = m.selected_project().unwrap().path.clone();
        let t = || press(KeyCode::Char('t'));
        assert_eq!(m.update(t()), Some(Cmd::OpenTerminal(path.clone())));
        let (shell, writes) = tool(2);
        let id = shell.id;
        m.shells.push((path.clone(), shell));
        m.term_view = TermView::Focused;
        let output = |m: &Model| m.panes(m.screen).output.unwrap();
        let whole = crate::ui::panes(m.screen, m.zoom, m.widths).output.unwrap();
        let pane = m.panes(m.screen).terminal.unwrap();
        let screen = crate::ui::tests::render(&mut m, 120, 40);
        assert!(screen.contains("terminal · ") && screen.contains(" TERMINAL "));
        assert_eq!(pane.height, whole.height / 3, "a third of the output pane");
        assert_eq!(output(&m).height + pane.height, whole.height);

        m.update(t());
        assert_eq!(drain(&writes), b"t", "typing goes to the shell");
        m.update(AppEvent::Input(Event::Key(KeyEvent::new(
            KeyCode::Char('\\'),
            KeyModifiers::CONTROL,
        ))));
        assert_eq!(m.term_view, TermView::Shown);
        m.update(t());
        assert_eq!(m.term_view, TermView::Hidden);
        assert!(m.panes(m.screen).terminal.is_none() && output(&m) == whole);
        assert!(m.update(t()).is_none(), "the same shell comes back");
        assert_eq!(m.term_view, TermView::Focused);

        let click = |m: &mut Model, at: ratatui::layout::Rect| {
            m.update(AppEvent::Input(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: at.x + 1,
                row: at.y + 1,
                modifiers: KeyModifiers::NONE,
            })));
        };
        let ctrl = |ch| {
            AppEvent::Input(Event::Key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::CONTROL,
            )))
        };
        m.update(ctrl('l'));
        m.update(ctrl('j'));
        assert_eq!(
            drain(&writes),
            b"\x0c\n",
            "ctrl-l and ctrl-j are the shell's"
        );
        m.update(ctrl('h'));
        assert_eq!(
            (m.term_view, m.focus),
            (TermView::Shown, crate::app::model::Focus::Sessions),
            "ctrl-h goes left"
        );
        m.term_view = TermView::Focused;
        let projects = m.panes(m.screen).projects.unwrap();
        click(&mut m, projects);
        assert_eq!(m.term_view, TermView::Shown, "a click elsewhere");
        click(&mut m, pane);
        assert_eq!(m.term_view, TermView::Focused, "a click on it");

        m.update(AppEvent::Input(Event::Key(KeyEvent::new(
            KeyCode::Char('\\'),
            KeyModifiers::CONTROL,
        ))));
        m.update(press(KeyCode::Char('j')));
        assert!(!m.terminal_shown(), "project b has no shell yet");
        let other = m.selected_project().unwrap().path.clone();
        assert_eq!(m.update(t()), Some(Cmd::OpenTerminal(other)), "its own");
        m.update(press(KeyCode::Char('k')));
        assert!(m.terminal_shown(), "back on a: its shell shows again");

        m.update(AppEvent::Pty(PtyEvent::Exited(id, Some(0))));
        assert!(m.shells.is_empty() && !m.terminal_shown());
    }
}
