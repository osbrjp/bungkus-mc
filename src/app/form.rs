//! The settings form behind both the first-run wizard and the settings
//! screen: workspace, default agent and theme.
//!
//! The wizard shows one field per step (then a summary) and can be skipped
//! with defaults; the settings screen shows all three at once. Either way
//! the result is a [`Settings`] the caller saves to `config.json`.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::agent::Kind;
use crate::app::browser::Browser;
use crate::store::config::{AgentScope, Settings, expand, tilde};
use crate::ui::theme::ThemeChoice;

/// Which of the two screens the form is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormKind {
    /// First run: one step per field, then a summary.
    Wizard,
    /// The `,` screen: every field at once.
    Settings,
}

/// The field (or wizard step) in focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    /// The workspace folder text input.
    Workspace,
    /// The default agent choice.
    Agent,
    /// The theme choice.
    Theme,
    /// The wizard's summary step.
    Done,
}

/// What a key did to the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Still editing.
    Continue,
    /// Closed without saving (settings screen only).
    Cancel,
    /// Finished; save these.
    Submit(Settings),
    /// The user asked to quit mc.
    Quit,
}

/// The form's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Form {
    /// Wizard or settings screen.
    pub kind: FormKind,
    /// Field in focus.
    pub field: Field,
    /// Workspace as typed.
    pub workspace: String,
    /// Chosen default agent.
    pub agent: Kind,
    /// Where the default agent is saved (`w` on the agent row of the
    /// settings screen switches it).
    pub scope: AgentScope,
    /// Chosen theme.
    pub theme: ThemeChoice,
    /// Where each agent was found on `PATH`, in [`Kind::ALL`] order.
    pub found: [Option<String>; 2],
    /// Why the last submit was refused.
    pub error: Option<String>,
    /// Workspace used when the wizard is skipped with an empty field.
    fallback: PathBuf,
    /// For `~` expansion.
    home: Option<PathBuf>,
    /// The folder list under the workspace field.
    pub browser: Browser,
    /// Whether the workspace field takes typing (after `/`, `~` or `i`);
    /// otherwise `j`/`k`/`h`/`l` drive the folder browser.
    pub typing: bool,
}

impl Form {
    /// Creates a form starting from `settings`.
    ///
    /// # Arguments
    ///
    /// * `kind`     - Wizard or settings screen.
    /// * `settings` - Initial values; the wizard's workspace may be empty.
    /// * `found`    - Where each agent was found, in [`Kind::ALL`] order.
    /// * `fallback` - Workspace used when the wizard is skipped empty.
    /// * `home`     - Home directory for `~` expansion.
    #[must_use]
    pub(crate) fn new(
        kind: FormKind,
        settings: &Settings,
        found: [Option<String>; 2],
        fallback: PathBuf,
        home: Option<PathBuf>,
    ) -> Self {
        let workspace = if settings.workspace.as_os_str().is_empty() {
            String::new()
        } else {
            tilde(&settings.workspace, home.as_deref())
        };
        let mut form = Self {
            kind,
            field: Field::Workspace,
            workspace,
            agent: settings.default_agent,
            scope: AgentScope::Global,
            theme: settings.theme,
            found,
            error: None,
            fallback,
            home,
            browser: Browser::open(Path::new("/")),
            typing: false,
        };
        form.sync_browser();
        if !form.selectable(form.agent) {
            form.agent = form.cycle_agent();
        }
        form
    }

    /// Handles one key press.
    ///
    /// `ctrl-c` quits from anywhere. The workspace field browses folders
    /// and types a path on request ([`Form::workspace_key`]); the choices
    /// move with `←`/`→` (and `h`/`l`). In the wizard
    /// `enter` goes to the next step and `esc` back, and `esc` on the first
    /// step skips the wizard with defaults; on the settings screen
    /// `tab`/`shift-tab` (and `↑`/`↓` off the workspace field) move between
    /// fields, `w` on the agent row switches where the agent is saved
    /// ([`AgentScope`]), `enter` saves and `esc` cancels.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Outcome::Quit;
        }
        if self.field == Field::Workspace
            && let Some(outcome) = self.workspace_key(key)
        {
            return outcome;
        }
        match (self.kind, key.code) {
            (_, KeyCode::Left | KeyCode::Char('h')) => self.change(false),
            (_, KeyCode::Right | KeyCode::Char('l')) => self.change(true),
            (FormKind::Wizard, KeyCode::Enter) => return self.wizard_next(),
            (FormKind::Wizard, KeyCode::Esc) => return self.wizard_back(),
            (FormKind::Settings, KeyCode::Char('w')) if self.field == Field::Agent => {
                self.scope = match self.scope {
                    AgentScope::Global => AgentScope::Workspace,
                    AgentScope::Workspace => AgentScope::Global,
                };
            }
            (FormKind::Settings, KeyCode::Enter) => return self.submit(),
            (FormKind::Settings, KeyCode::Esc) => return Outcome::Cancel,
            (FormKind::Settings, KeyCode::Down | KeyCode::Tab | KeyCode::Char('j')) => {
                self.move_field(true);
            }
            (FormKind::Settings, KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k')) => {
                self.move_field(false);
            }
            _ => {}
        }
        Outcome::Continue
    }

    /// Chooses the highlighted folder (or, on the `./` row, the one in the
    /// field) as the workspace and moves on to the next field; the panel
    /// stays open, and a folder that cannot be used shows why.
    fn choose_workspace(&mut self) -> Outcome {
        if let Some(dir) = self.browser.highlighted().map(Path::to_path_buf) {
            self.set_workspace(&tilde(&dir, self.home.as_deref()));
        }
        match self.kind {
            FormKind::Wizard => self.wizard_next(),
            FormKind::Settings => {
                match self.validated() {
                    Ok(_) => self.field = Field::Agent,
                    Err(e) => self.error = Some(e),
                }
                Outcome::Continue
            }
        }
    }

    /// Handles a key on the workspace field; `None` lets the form handle it
    /// (`tab`, `esc` while browsing, …).
    ///
    /// Browsing (the default): `j`/`k` or `↓`/`↑` move the highlight, `l`/`→`
    /// opens the folder, `h`/`←` goes up, `enter` chooses; `/` or `~` start
    /// typing a new path, `i` (or `backspace`, `ctrl-u`) edits this one.
    /// Typing: letters edit the path and the list follows it; `esc` stops
    /// typing, `enter` chooses, the arrows still browse.
    fn workspace_key(&mut self, key: KeyEvent) -> Option<Outcome> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Enter {
            self.typing = false;
            return Some(self.choose_workspace());
        }
        if self.typing {
            if key.code == KeyCode::Esc {
                self.typing = false;
                return Some(Outcome::Continue);
            }
            if self.edit_text(key) {
                self.error = None;
                self.sync_browser();
                return Some(Outcome::Continue);
            }
        } else {
            let arrow = match key.code {
                KeyCode::Char('j') if !ctrl => Some(KeyCode::Down),
                KeyCode::Char('k') if !ctrl => Some(KeyCode::Up),
                KeyCode::Char('l') if !ctrl => Some(KeyCode::Right),
                KeyCode::Char('h') if !ctrl => Some(KeyCode::Left),
                _ => None,
            };
            if let Some(code) = arrow {
                self.browse_key(KeyEvent::new(code, KeyModifiers::NONE));
                self.error = None;
                return Some(Outcome::Continue);
            }
            match key.code {
                KeyCode::Char(start @ ('/' | '~')) if !ctrl => {
                    self.typing = true;
                    self.workspace = start.to_string();
                    self.sync_browser();
                    return Some(Outcome::Continue);
                }
                KeyCode::Char('i') if !ctrl => {
                    self.typing = true;
                    return Some(Outcome::Continue);
                }
                KeyCode::Backspace | KeyCode::Char('u')
                    if key.code == KeyCode::Backspace || ctrl =>
                {
                    self.typing = true;
                    self.edit_text(key);
                    self.sync_browser();
                    return Some(Outcome::Continue);
                }
                _ => {}
            }
        }
        if self.browse_key(key) {
            self.error = None;
            return Some(Outcome::Continue);
        }
        None
    }

    /// Returns the folder the field points at: the typed folder when it
    /// exists, else the nearest existing parent, else the home folder.
    fn browsed_dir(&self) -> PathBuf {
        let typed = expand(self.workspace.trim(), self.home.as_deref());
        let home = self.home.clone().unwrap_or_else(|| PathBuf::from("/"));
        typed
            .as_deref()
            .and_then(|p| {
                p.ancestors()
                    .find(|a| std::fs::metadata(a).is_ok_and(|m| m.is_dir()))
            })
            .map_or(home, Path::to_path_buf)
    }

    /// Sets the workspace text and lists the folder it points at.
    pub(crate) fn set_workspace(&mut self, text: &str) {
        text.clone_into(&mut self.workspace);
        self.sync_browser();
    }

    /// Re-lists the browser when the field points at another folder.
    fn sync_browser(&mut self) {
        let dir = self.browsed_dir();
        if self.browser.dir != dir {
            self.browser = Browser::open(&dir);
        }
    }

    /// Applies a browser key: `↑`/`↓` move the highlight, `→` opens the
    /// highlighted folder, `←` goes to the parent (the field follows).
    /// Returns whether the key was one.
    fn browse_key(&mut self, key: KeyEvent) -> bool {
        let target = match key.code {
            KeyCode::Up => {
                self.browser.step(-1);
                return true;
            }
            KeyCode::Down => {
                self.browser.step(1);
                return true;
            }
            KeyCode::Right => self.browser.highlighted().map(Path::to_path_buf),
            KeyCode::Left => self.browser.dir.parent().map(Path::to_path_buf),
            _ => return false,
        };
        if let Some(dir) = target {
            self.workspace = tilde(&dir, self.home.as_deref());
            self.sync_browser();
        }
        true
    }

    /// Applies a text-editing key to the workspace field; returns whether
    /// the key was one.
    fn edit_text(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => self.workspace.clear(),
            KeyCode::Char(ch) if !ctrl => self.workspace.push(ch),
            KeyCode::Backspace => {
                self.workspace.pop();
            }
            _ => return false,
        }
        true
    }

    /// Moves the focused choice to the next (or previous) value.
    fn change(&mut self, forward: bool) {
        match self.field {
            Field::Agent => self.agent = self.cycle_agent(),
            Field::Theme => {
                let all = ThemeChoice::ALL;
                let i = all.iter().position(|t| *t == self.theme).unwrap_or(0);
                let step = if forward { 1 } else { all.len() - 1 };
                self.theme = all[(i + step) % all.len()];
            }
            Field::Workspace | Field::Done => {}
        }
    }

    /// Returns whether `kind` may be chosen: installed, or nothing is.
    fn selectable(&self, kind: Kind) -> bool {
        let installed = |k: Kind| self.found[k as usize].is_some();
        installed(kind) || !Kind::ALL.into_iter().any(installed)
    }

    /// Returns the other agent when it is selectable, else the current one.
    fn cycle_agent(&self) -> Kind {
        let other = match self.agent {
            Kind::Claude => Kind::Codex,
            Kind::Codex => Kind::Claude,
        };
        if self.selectable(other) {
            other
        } else {
            self.agent
        }
    }

    /// Moves focus between the settings screen's fields, wrapping round.
    fn move_field(&mut self, forward: bool) {
        self.field = match (self.field, forward) {
            (Field::Workspace, true) | (Field::Theme, false) => Field::Agent,
            (Field::Agent, true) | (Field::Workspace, false) => Field::Theme,
            (Field::Theme | Field::Done, true) | (Field::Agent | Field::Done, false) => {
                Field::Workspace
            }
        };
    }

    /// Advances the wizard, validating the workspace when leaving it.
    fn wizard_next(&mut self) -> Outcome {
        self.field = match self.field {
            Field::Workspace => match self.validated() {
                Ok(_) => Field::Agent,
                Err(e) => {
                    self.error = Some(e);
                    Field::Workspace
                }
            },
            Field::Agent => Field::Theme,
            Field::Theme => Field::Done,
            Field::Done => return self.submit(),
        };
        Outcome::Continue
    }

    /// Steps the wizard back; on the first step, skips it with defaults.
    fn wizard_back(&mut self) -> Outcome {
        self.field = match self.field {
            Field::Workspace => {
                let workspace = self.validated().unwrap_or_else(|_| self.fallback.clone());
                return Outcome::Submit(self.settings(workspace));
            }
            Field::Agent => Field::Workspace,
            Field::Theme => Field::Agent,
            Field::Done => Field::Theme,
        };
        Outcome::Continue
    }

    /// Submits when the workspace is valid; otherwise shows why and moves
    /// focus to it.
    fn submit(&mut self) -> Outcome {
        match self.validated() {
            Ok(workspace) => Outcome::Submit(self.settings(workspace)),
            Err(e) => {
                self.error = Some(e);
                self.field = Field::Workspace;
                Outcome::Continue
            }
        }
    }

    /// Returns the form's values with `workspace`.
    fn settings(&self, workspace: PathBuf) -> Settings {
        Settings {
            workspace,
            theme: self.theme,
            default_agent: self.agent,
        }
    }

    /// Returns the typed workspace as an absolute existing folder.
    ///
    /// # Errors
    ///
    /// A one-line message in the product voice when the field is empty,
    /// relative, or not a folder.
    fn validated(&self) -> Result<PathBuf, String> {
        let typed = self.workspace.trim();
        if typed.is_empty() {
            return Err("Type a folder, e.g. ~/Works.".into());
        }
        let path = expand(typed, self.home.as_deref())
            .ok_or_else(|| format!("{typed} is not a full path; start with / or ~."))?;
        if std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
            Ok(path)
        } else {
            Err(format!("{typed} is not a folder."))
        }
    }
}

/// Returns the default workspace for the wizard: the parent of the git
/// repository `cwd` is in, if any (DESIGN §5.7 first run).
#[must_use]
pub(crate) fn git_parent(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| std::fs::metadata(dir.join(".git")).is_ok())
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(form: &mut Form, text: &str) {
        for ch in text.chars() {
            assert_eq!(form.key(press(KeyCode::Char(ch))), Outcome::Continue);
        }
    }

    fn wizard(found: [Option<String>; 2]) -> Form {
        let empty = Settings {
            workspace: PathBuf::new(),
            theme: ThemeChoice::Auto,
            default_agent: Kind::Claude,
        };
        Form::new(
            FormKind::Wizard,
            &empty,
            found,
            PathBuf::from("/fallback"),
            None,
        )
    }

    #[test]
    fn wizard_walks_every_step_and_submits() {
        let tmp = std::env::temp_dir();
        let mut form = wizard([Some("claude".into()), Some("codex".into())]);
        typed(&mut form, &tmp.to_string_lossy());
        assert_eq!(form.key(press(KeyCode::Enter)), Outcome::Continue);
        assert_eq!(form.field, Field::Agent);
        form.key(press(KeyCode::Right));
        assert_eq!(form.agent, Kind::Codex);
        form.key(press(KeyCode::Enter));
        form.key(press(KeyCode::Char('l')));
        assert_eq!((form.field, form.theme), (Field::Theme, ThemeChoice::Dark));
        form.key(press(KeyCode::Enter));
        assert_eq!(form.field, Field::Done);
        let want = Settings {
            workspace: tmp,
            theme: ThemeChoice::Dark,
            default_agent: Kind::Codex,
        };
        assert_eq!(form.key(press(KeyCode::Enter)), Outcome::Submit(want));
    }

    #[test]
    fn wizard_refuses_a_bad_workspace_and_skips_to_the_fallback() {
        let mut form = wizard([None, None]);
        assert_eq!(form.key(press(KeyCode::Enter)), Outcome::Continue);
        assert_eq!(form.field, Field::Workspace);
        assert!(form.error.is_some());
        typed(&mut form, "relative/dir");
        form.key(press(KeyCode::Enter));
        assert!(
            form.error
                .as_deref()
                .unwrap_or_default()
                .contains("not a full path")
        );
        let Outcome::Submit(settings) = form.key(press(KeyCode::Esc)) else {
            panic!("esc on the first step skips");
        };
        assert_eq!(settings.workspace, PathBuf::from("/fallback"));
    }

    #[test]
    fn only_installed_agents_are_selectable() {
        let mut form = wizard([None, Some("codex".into())]);
        assert_eq!(form.agent, Kind::Codex, "starts on the installed one");
        form.field = Field::Agent;
        form.key(press(KeyCode::Left));
        assert_eq!(form.agent, Kind::Codex);
        let mut none = wizard([None, None]);
        none.field = Field::Agent;
        none.key(press(KeyCode::Right));
        assert_eq!(
            none.agent,
            Kind::Codex,
            "nothing installed: both selectable"
        );
    }

    #[test]
    fn enter_on_the_workspace_chooses_the_highlighted_folder_and_stays_open() {
        let root = std::env::temp_dir().join(format!("mc-choose-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("works/app/.git")).unwrap();
        let current = Settings {
            workspace: root.clone(),
            theme: ThemeChoice::Auto,
            default_agent: Kind::Claude,
        };
        let mut form = Form::new(
            FormKind::Settings,
            &current,
            [None, None],
            PathBuf::new(),
            None,
        );
        form.set_workspace(&root.to_string_lossy());
        assert_eq!(form.browser.highlighted(), None, "./ comes first");
        form.key(press(KeyCode::Down));
        assert_eq!(form.key(press(KeyCode::Enter)), Outcome::Continue);
        assert_eq!(form.field, Field::Agent, "on to the next field");
        assert_eq!(PathBuf::from(&form.workspace), root.join("works"));
        form.key(press(KeyCode::Up));
        form.key(press(KeyCode::Up));
        assert_eq!(form.field, Field::Workspace);
        form.key(press(KeyCode::Enter));
        assert_eq!(
            PathBuf::from(&form.workspace),
            root.join("works"),
            "enter on ./ keeps the folder in the field"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn j_and_k_move_between_settings_rows_outside_the_text_box() {
        let current = Settings {
            workspace: std::env::temp_dir(),
            theme: ThemeChoice::Auto,
            default_agent: Kind::Claude,
        };
        let mut form = Form::new(
            FormKind::Settings,
            &current,
            [None, None],
            PathBuf::new(),
            None,
        );
        form.field = Field::Agent;
        form.key(press(KeyCode::Char('j')));
        assert_eq!(form.field, Field::Theme);
        form.key(press(KeyCode::Char('k')));
        form.key(press(KeyCode::Char('k')));
        assert_eq!(form.field, Field::Workspace);
        let before = form.workspace.clone();
        form.key(press(KeyCode::Char('j')));
        assert_eq!(
            (form.field, form.workspace.clone(), form.browser.selected),
            (Field::Workspace, before.clone(), 1),
            "on the workspace field j moves the folder list"
        );
        form.key(press(KeyCode::Char('i')));
        form.key(press(KeyCode::Char('j')));
        assert_eq!(form.workspace, format!("{before}j"), "after i, j is typed");
        form.key(press(KeyCode::Esc));
        assert!(!form.typing, "esc stops typing, not the dialog");
    }

    #[test]
    fn w_on_the_agent_row_switches_where_the_agent_is_saved() {
        let settings = Settings {
            workspace: std::env::temp_dir(),
            theme: ThemeChoice::Dark,
            default_agent: Kind::Claude,
        };
        let found = [Some("claude".into()), Some("codex".into())];
        let mut form = Form::new(
            FormKind::Settings,
            &settings,
            found.clone(),
            "/".into(),
            None,
        );
        form.field = Field::Agent;
        assert_eq!(form.scope, AgentScope::Global);
        form.key(press(KeyCode::Char('w')));
        assert_eq!(form.scope, AgentScope::Workspace);
        form.key(press(KeyCode::Char('w')));
        assert_eq!(form.scope, AgentScope::Global);
        form.field = Field::Theme;
        form.key(press(KeyCode::Char('w')));
        assert_eq!(form.scope, AgentScope::Global, "only on the agent row");
        let mut wizard = Form::new(FormKind::Wizard, &settings, found, "/".into(), None);
        wizard.field = Field::Agent;
        wizard.key(press(KeyCode::Char('w')));
        assert_eq!(wizard.scope, AgentScope::Global, "not in the wizard");
    }

    #[test]
    fn settings_screen_moves_between_fields_and_cancels() {
        let current = Settings {
            workspace: std::env::temp_dir(),
            theme: ThemeChoice::Light,
            default_agent: Kind::Claude,
        };
        let mut form = Form::new(
            FormKind::Settings,
            &current,
            [None, None],
            PathBuf::new(),
            None,
        );
        form.key(press(KeyCode::Tab));
        form.key(press(KeyCode::Tab));
        assert_eq!(form.field, Field::Theme);
        form.key(press(KeyCode::Right));
        assert_eq!(form.theme, ThemeChoice::Auto);
        typed(&mut form, "q");
        assert_eq!(
            form.field,
            Field::Theme,
            "q is not typed outside the text field"
        );
        assert_eq!(form.key(press(KeyCode::Esc)), Outcome::Cancel);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(form.key(ctrl_c), Outcome::Quit);
    }

    #[test]
    fn finds_the_parent_of_the_enclosing_git_repository() {
        let root = std::env::temp_dir().join(format!("mc-git-{}", std::process::id()));
        std::fs::create_dir_all(root.join("ws/repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("ws/repo/src/deep")).unwrap();
        assert_eq!(
            git_parent(&root.join("ws/repo/src/deep")),
            Some(root.join("ws"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
