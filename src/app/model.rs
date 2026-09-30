//! The model the UI thread owns, and `update`: one input event in, the
//! model changed, maybe a command out.
//!
//! `update` never does I/O beyond the form's folder check; anything else
//! (saving, scanning, redrawing) is returned as a [`Cmd`] for the loop.

use std::path::PathBuf;

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::form::{Field, Form, FormKind, Outcome};
use crate::store::config::Settings;
use crate::ui::keymap::{self, Action, Lookup, Scope};
use crate::ui::theme::Theme;
use crate::workspace::Project;

/// Which list pane has focus in NORMAL mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    /// The projects pane (left).
    Projects,
    /// The sessions pane (middle).
    Sessions,
}

impl Focus {
    /// Returns the keymap scope for this pane.
    #[must_use]
    pub(crate) const fn scope(self) -> Scope {
        match self {
            Self::Projects => Scope::Projects,
            Self::Sessions => Scope::Sessions,
        }
    }
}

/// A screen drawn over the panes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Overlay {
    /// The generated key help.
    Help,
    /// The first-run wizard or the settings screen.
    Form(Form),
}

/// Work `update` hands back to the event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Leave mc.
    Quit,
    /// Clear the terminal and draw everything again.
    Redraw,
    /// Save these settings and rescan the workspace.
    Apply(Settings),
}

/// Everything the screen shows.
#[derive(Debug)]
pub(crate) struct Model {
    /// Colours for the applied settings.
    pub theme: Theme,
    /// Whether the host terminal's background is light (OSC 11), if known.
    pub host_light: Option<bool>,
    /// Home directory, for `~` in paths.
    pub home: Option<PathBuf>,
    /// Applied settings; `None` until the first-run wizard finishes.
    pub settings: Option<Settings>,
    /// Projects in the workspace.
    pub projects: Vec<Project>,
    /// Why the workspace could not be listed, if it could not.
    pub scan_error: Option<String>,
    /// Index into [`Model::visible`] of the selected project.
    pub selected: usize,
    /// Focused pane.
    pub focus: Focus,
    /// Projects filter text.
    pub filter: String,
    /// Whether the filter is being typed (FILTER mode).
    pub filtering: bool,
    /// First key of a pending double press.
    pub pending: Option<char>,
    /// Help or a form on top of the panes.
    pub overlay: Option<Overlay>,
    /// One-line message shown in place of the key hints until the next key.
    pub message: Option<String>,
    /// Where each agent was found on `PATH`, in `Kind::ALL` order.
    pub found: [Option<String>; 2],
    /// Workspace used when the wizard is skipped with an empty field.
    pub fallback_workspace: PathBuf,
    /// Rows the projects list shows, for half-page moves.
    pub list_rows: usize,
}

impl Model {
    /// Creates a model with no settings applied yet.
    ///
    /// # Arguments
    ///
    /// * `theme`    - Colours for this terminal.
    /// * `home`     - Home directory, for `~` in paths.
    /// * `found`    - Where each agent was found on `PATH`.
    /// * `fallback` - Workspace used when the wizard is skipped empty.
    #[must_use]
    pub(crate) fn new(
        theme: Theme,
        home: Option<PathBuf>,
        found: [Option<String>; 2],
        fallback: PathBuf,
    ) -> Self {
        Self {
            theme,
            host_light: None,
            home,
            settings: None,
            projects: Vec::new(),
            scan_error: None,
            selected: 0,
            focus: Focus::Projects,
            filter: String::new(),
            filtering: false,
            pending: None,
            overlay: None,
            message: None,
            found,
            fallback_workspace: fallback,
            list_rows: 10,
        }
    }

    /// Returns the theme to draw with: the form's choice while a form is
    /// open (live preview), the applied one otherwise.
    #[must_use]
    pub(crate) fn view_theme(&self) -> Theme {
        match &self.overlay {
            Some(Overlay::Form(form)) => self.theme.with_name(form.theme.resolve(self.host_light)),
            Some(Overlay::Help) | None => self.theme,
        }
    }

    /// Returns the projects matching the filter, in display order.
    #[must_use]
    pub(crate) fn visible(&self) -> Vec<&Project> {
        let needle = self.filter.to_lowercase();
        self.projects
            .iter()
            .filter(|p| p.name.to_lowercase().contains(&needle))
            .collect()
    }

    /// Returns the selected project, if any is visible.
    #[must_use]
    pub(crate) fn selected_project(&self) -> Option<&Project> {
        self.visible().get(self.selected).copied()
    }

    /// Opens the settings screen (or, before first run, the wizard).
    fn open_form(&mut self, kind: FormKind, field: Field) {
        let current = self.settings.clone().unwrap_or_else(|| Settings {
            workspace: PathBuf::new(),
            theme: crate::ui::theme::ThemeChoice::Auto,
            default_agent: crate::agent::Kind::Claude,
        });
        let mut form = Form::new(
            kind,
            &current,
            self.found.clone(),
            self.fallback_workspace.clone(),
            self.home.clone(),
        );
        form.field = field;
        self.overlay = Some(Overlay::Form(form));
    }

    /// Opens the first-run wizard with `prefill` in the workspace field.
    pub(crate) fn start_wizard(&mut self, prefill: &str) {
        self.open_form(FormKind::Wizard, Field::Workspace);
        if let Some(Overlay::Form(form)) = &mut self.overlay {
            prefill.clone_into(&mut form.workspace);
        }
    }

    /// Handles one terminal event.
    ///
    /// # Returns
    ///
    /// The command the loop must run, if any.
    pub(crate) fn update(&mut self, event: &Event) -> Option<Cmd> {
        let Event::Key(key) = event else { return None };
        if key.kind != KeyEventKind::Press {
            return None;
        }
        self.message = None;
        match &mut self.overlay {
            Some(Overlay::Help) => {
                self.overlay = None;
                return None;
            }
            Some(Overlay::Form(form)) => {
                return match form.key(*key) {
                    Outcome::Continue => None,
                    Outcome::Cancel => {
                        self.overlay = None;
                        None
                    }
                    Outcome::Submit(settings) => {
                        self.overlay = None;
                        Some(Cmd::Apply(settings))
                    }
                    Outcome::Quit => Some(Cmd::Quit),
                };
            }
            None => {}
        }
        if self.filtering {
            self.filter_key(*key);
            return None;
        }
        match keymap::lookup(self.focus.scope(), *key, self.pending.take()) {
            Lookup::Pending(ch) => {
                self.pending = Some(ch);
                None
            }
            Lookup::Unbound => None,
            Lookup::Action(action) => self.act(action),
        }
    }

    /// Applies a FILTER-mode key: typing edits the filter, `enter` keeps
    /// it, `esc` clears it.
    fn filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.filtering = false,
            KeyCode::Esc => {
                self.filtering = false;
                self.filter.clear();
            }
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.push(ch);
            }
            _ => {}
        }
        self.selected = 0;
    }

    /// Runs a keymap action.
    fn act(&mut self, action: Action) -> Option<Cmd> {
        let last = self.visible().len().saturating_sub(1);
        let half = (self.list_rows / 2).max(1);
        let on_projects = self.focus == Focus::Projects;
        match action {
            Action::PrevPane => self.focus = Focus::Projects,
            Action::NextPane | Action::OpenProject => self.focus = Focus::Sessions,
            Action::Down if on_projects => self.selected = (self.selected + 1).min(last),
            Action::Up if on_projects => self.selected = self.selected.saturating_sub(1),
            Action::First if on_projects => self.selected = 0,
            Action::Last if on_projects => self.selected = last,
            Action::HalfDown if on_projects => self.selected = (self.selected + half).min(last),
            Action::HalfUp if on_projects => self.selected = self.selected.saturating_sub(half),
            Action::Filter if on_projects => {
                self.filtering = true;
                self.filter.clear();
                self.selected = 0;
            }
            Action::Down
            | Action::Up
            | Action::First
            | Action::Last
            | Action::HalfDown
            | Action::HalfUp
            | Action::Filter => {}
            Action::Workspace => self.open_form(FormKind::Settings, Field::Workspace),
            Action::Settings => self.open_form(FormKind::Settings, Field::Agent),
            Action::Help => self.overlay = Some(Overlay::Help),
            Action::Redraw => return Some(Cmd::Redraw),
            Action::Quit => return Some(Cmd::Quit),
        }
        None
    }

    /// Applies saved settings and the workspace scan that followed.
    ///
    /// # Arguments
    ///
    /// * `settings` - The settings now in effect.
    /// * `scan`     - The projects, or why the folder could not be listed.
    /// * `cwd`      - mc's working directory; a project containing it is
    ///   preselected.
    pub(crate) fn apply(
        &mut self,
        settings: Settings,
        scan: std::io::Result<Vec<Project>>,
        cwd: &std::path::Path,
    ) {
        self.theme = self
            .theme
            .with_name(settings.theme.resolve(self.host_light));
        let (projects, error) = match scan {
            Ok(projects) => (projects, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        self.projects = projects;
        self.scan_error = error;
        self.filter.clear();
        self.selected = self
            .projects
            .iter()
            .position(|p| cwd.starts_with(&p.path))
            .unwrap_or(0);
        self.settings = Some(settings);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::Kind;
    use crate::ui::theme::{Background, Profile, ThemeChoice, ThemeName};

    /// A model with `names` as projects of `/Users/me/Works/OSBR`, as the
    /// view tests and goldens use it.
    pub(crate) fn sample(names: &[&str]) -> Model {
        let home = PathBuf::from("/Users/me");
        let workspace = home.join("Works/OSBR");
        let mut model = Model::new(
            Theme::new(ThemeName::Dark, Profile::NoColor, Background::Paint),
            Some(home.clone()),
            [Some("~/.local/bin/claude".into()), None],
            home,
        );
        let projects = names
            .iter()
            .map(|n| Project {
                name: (*n).into(),
                path: workspace.join(n),
            })
            .collect();
        let settings = Settings {
            workspace,
            theme: ThemeChoice::Dark,
            default_agent: Kind::Claude,
        };
        model.apply(settings, Ok(projects), Path::new("/"));
        model
    }

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn moves_the_selection_and_focus() {
        let mut m = sample(&["a", "b", "c"]);
        m.update(&press(KeyCode::Char('j')));
        m.update(&press(KeyCode::Char('j')));
        m.update(&press(KeyCode::Char('j')));
        assert_eq!(m.selected, 2, "stops at the last project");
        m.update(&press(KeyCode::Char('g')));
        m.update(&press(KeyCode::Char('g')));
        assert_eq!(m.selected, 0);
        m.update(&press(KeyCode::Enter));
        assert_eq!(m.focus, Focus::Sessions);
        m.update(&press(KeyCode::Char('j')));
        assert_eq!(m.selected, 0, "the sessions pane does not move projects");
        m.update(&press(KeyCode::Char('h')));
        assert_eq!(m.focus, Focus::Projects);
    }

    #[test]
    fn filters_projects_and_clears_on_esc() {
        let mut m = sample(&["kedai-web", "pasar-mobile", "teh-cli"]);
        m.update(&press(KeyCode::Char('/')));
        for ch in "EH".chars() {
            m.update(&press(KeyCode::Char(ch)));
        }
        assert!(m.filtering);
        let names: Vec<&str> = m.visible().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["teh-cli"]);
        m.update(&press(KeyCode::Enter));
        assert!(!m.filtering && m.filter == "EH");
        m.update(&press(KeyCode::Char('/')));
        m.update(&press(KeyCode::Esc));
        assert_eq!(m.visible().len(), 3);
    }

    #[test]
    fn settings_screen_previews_the_theme_and_submits() {
        let mut m = sample(&["a"]);
        if let Some(settings) = &mut m.settings {
            settings.workspace = std::env::temp_dir();
        }
        m.update(&press(KeyCode::Char(',')));
        m.update(&press(KeyCode::Down));
        m.update(&press(KeyCode::Right));
        assert_eq!(m.view_theme().name, ThemeName::Light, "live preview");
        assert_eq!(m.theme.name, ThemeName::Dark, "not applied yet");
        let Some(Cmd::Apply(settings)) = m.update(&press(KeyCode::Enter)) else {
            panic!("enter saves");
        };
        assert_eq!(settings.theme, ThemeChoice::Light);
        m.update(&press(KeyCode::Char(',')));
        m.update(&press(KeyCode::Esc));
        assert!(m.overlay.is_none());
    }

    #[test]
    fn quits_and_redraws_through_commands() {
        let mut m = sample(&[]);
        assert_eq!(m.update(&press(KeyCode::Char('R'))), Some(Cmd::Redraw));
        assert_eq!(m.update(&press(KeyCode::Char('?'))), None);
        assert_eq!(m.overlay, Some(Overlay::Help));
        m.update(&press(KeyCode::Char('q')));
        assert!(m.overlay.is_none(), "any key closes help");
        assert_eq!(m.update(&press(KeyCode::Char('q'))), Some(Cmd::Quit));
    }
}
