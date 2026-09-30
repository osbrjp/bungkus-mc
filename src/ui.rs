//! Rendering: header, the three panes, the getah bar and overlays
//! (DESIGN §4, §5).
//!
//! Views draw the [`Model`] into a ratatui [`Frame`]; they never read the
//! environment or the terminal. The only thing a view writes back is the
//! number of list rows on screen, for half-page moves.

mod form;
mod help;
pub(crate) mod keymap;
pub(crate) mod mascot;
pub(crate) mod sanitise;
pub(crate) mod theme;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};

use crate::app::form::FormKind;
use crate::app::model::{Focus, Model, Overlay};
use crate::store::config::tilde;
use crate::ui::mascot::Mascot;
use crate::ui::sanitise::truncate;
use crate::ui::theme::{Theme, Token};

/// Smallest usable screen, per DESIGN §4.
const MIN_SIZE: (u16, u16) = (80, 24);

/// Width from which the three panes sit side by side (DESIGN §4).
const THREE_PANE_WIDTH: u16 = 100;

/// Outer widths of the projects and sessions panes; output takes the rest.
const PROJECTS_WIDTH: u16 = 22;
/// See [`PROJECTS_WIDTH`].
const SESSIONS_WIDTH: u16 = 38;

/// Draws the whole screen.
///
/// Below [`MIN_SIZE`] only a one-line notice is drawn. Before the first
/// run is finished the wizard takes the whole screen. Otherwise: header,
/// panes (three side by side from [`THREE_PANE_WIDTH`] columns, else only
/// the focused one), getah bar, and the help or settings overlay on top.
///
/// # Arguments
///
/// * `frame` - The frame to draw into; its area is the whole terminal.
/// * `model` - What to show; its `list_rows` is updated.
pub(crate) fn draw(frame: &mut Frame, model: &mut Model) {
    let theme = model.view_theme();
    let area = frame.area();
    frame.render_widget(Block::new().style(theme.base()), area);
    if area.width < MIN_SIZE.0 || area.height < MIN_SIZE.1 {
        draw_too_small(frame, area);
        return;
    }
    if let Some(Overlay::Form(f)) = &model.overlay
        && f.kind == FormKind::Wizard
    {
        form::draw_wizard(frame, area, f, theme, model.host_light);
        return;
    }
    let [header, body, getah] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    model.list_rows = usize::from(body.height.saturating_sub(2));
    let wide = area.width >= THREE_PANE_WIDTH;
    draw_header(frame, header, model, theme, wide);
    if wide {
        let [projects, sessions, output] = Layout::horizontal([
            Constraint::Length(PROJECTS_WIDTH),
            Constraint::Length(SESSIONS_WIDTH),
            Constraint::Fill(1),
        ])
        .areas(body);
        draw_projects(frame, projects, model, theme);
        draw_sessions(frame, sessions, model, theme);
        draw_output(frame, output, theme);
    } else {
        match model.focus {
            Focus::Projects => draw_projects(frame, body, model, theme),
            Focus::Sessions => draw_sessions(frame, body, model, theme),
        }
    }
    draw_getah(frame, getah, model, theme);
    match &model.overlay {
        Some(Overlay::Help) => help::draw(frame, area, model.focus.scope(), theme),
        Some(Overlay::Form(f)) => form::draw_settings(frame, area, f, theme, model.host_light),
        None => {}
    }
}

/// Draws the centred "too small" notice (DESIGN §11).
fn draw_too_small(frame: &mut Frame, area: Rect) {
    let text = format!(
        "bungkus-mc needs at least {}×{} (now {}×{}).",
        MIN_SIZE.0, MIN_SIZE.1, area.width, area.height
    );
    let [row] = Layout::vertical([Constraint::Length(1)])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), row);
}

/// Draws line 1: the app name and where you are, the version right.
///
/// Wide: `~/Works/OSBR › kedai-web`. Narrow (one pane): the breadcrumb
/// `kedai-web › sessions`.
fn draw_header(frame: &mut Frame, area: Rect, model: &Model, theme: Theme, wide: bool) {
    let project = model.selected_project().map(|p| p.name.as_str());
    let crumbs = if wide {
        let workspace = workspace_label(model);
        match project {
            Some(name) => format!("{workspace} › {name}"),
            None => workspace,
        }
    } else {
        let pane = match model.focus {
            Focus::Projects => "projects",
            Focus::Sessions => "sessions",
        };
        match project {
            Some(name) => format!("{name} › {pane}"),
            None => pane.to_owned(),
        }
    };
    let line = Line::from(vec![
        Span::styled(
            " bungkus-mc",
            theme.fg(Token::Accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {crumbs}"), theme.fg(Token::FgMuted)),
    ]);
    frame.render_widget(line, area);
    let version = format!("v{} ", env!("CARGO_PKG_VERSION"));
    frame.render_widget(
        Line::styled(version, theme.fg(Token::FgMuted)).alignment(Alignment::Right),
        area,
    );
}

/// Returns the applied workspace with the home directory as `~`.
fn workspace_label(model: &Model) -> String {
    model
        .settings
        .as_ref()
        .map_or_else(String::new, |s| tilde(&s.workspace, model.home.as_deref()))
}

/// Draws the projects pane: `<marker><spinner> <name>` per row.
///
/// The marker is `>` on the selected row while the pane is focused and
/// `:` (the ascii form of `▌`) while it is not (DESIGN §5.3).
fn draw_projects(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let focused = model.focus == Focus::Projects;
    let title = if model.filtering || !model.filter.is_empty() {
        format!("projects /{}", model.filter)
    } else {
        "projects".to_owned()
    };
    let block = pane(&title, focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = usize::from(inner.height);
    let width = usize::from(inner.width).saturating_sub(3);
    let offset = (model.selected + 1).saturating_sub(rows);
    let lines: Vec<Line> = model
        .visible()
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows)
        .map(|(i, project)| {
            let selected = i == model.selected;
            let marker = match (selected, focused) {
                (true, true) => ">",
                (true, false) => ":",
                (false, _) => " ",
            };
            let name = if selected {
                theme.fg(Token::Accent)
            } else {
                theme.fg(Token::Fg)
            };
            Line::from(vec![
                Span::styled(format!("{marker}  "), theme.fg(Token::Ok)),
                Span::styled(truncate(&project.name, width), name),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draws the sessions pane for the selected project, with the empty-state
/// copy of DESIGN §11.
fn draw_sessions(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let project = model.selected_project();
    let title = project.map_or_else(
        || "sessions".to_owned(),
        |p| format!("sessions · {}", p.name),
    );
    let block = pane(&title, model.focus == Focus::Sessions, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let workspace = workspace_label(model);
    let text = match (&model.scan_error, project) {
        (Some(error), _) => format!("Can't open {workspace}: {error}. w to pick another folder."),
        (None, Some(p)) => {
            format!(
                "No sessions in {}. n to start one. Only sessions started here show up.",
                p.name
            )
        }
        (None, None) if !model.filter.is_empty() => {
            format!("No project matches /{}.", model.filter)
        }
        (None, None) => format!(
            "No projects in {workspace}. A project is a folder with CLAUDE.md, AGENTS.md or .git \
             in it — w to pick another folder."
        ),
    };
    let [text_area] = Layout::horizontal([Constraint::Fill(1)])
        .horizontal_margin(1)
        .areas(inner);
    frame.render_widget(
        Paragraph::new(text)
            .style(theme.fg(Token::FgMuted))
            .wrap(Wrap { trim: true }),
        text_area,
    );
}

/// Draws the output pane's empty state: the mascot and two lines, centred
/// (DESIGN §5.7); the mascot is left out when the pane is too short.
fn draw_output(frame: &mut Frame, area: Rect, theme: Theme) {
    let block = pane("output", false, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let with_mascot = inner.height >= mascot::HEIGHT + 4;
    let height = if with_mascot { mascot::HEIGHT + 3 } else { 2 };
    let [content] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(inner);
    let text_area = if with_mascot {
        let [sprite] = Layout::horizontal([Constraint::Length(mascot::WIDTH)])
            .flex(Flex::Center)
            .areas(content);
        frame.render_widget(Mascot { theme }, sprite);
        Rect {
            y: content.y + mascot::HEIGHT + 1,
            height: 2,
            ..content
        }
    } else {
        content
    };
    let lines = vec![
        Line::styled("Nothing wrapped yet.", theme.fg(Token::Fg)),
        Line::styled("n to start a session", theme.fg(Token::FgMuted)),
    ];
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        text_area,
    );
}

/// Draws the last line: the mode word as a badge, then the focused pane's
/// key hints or the current message.
fn draw_getah(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let (mode, token) = if model.filtering {
        (" FILTER ", Token::Info)
    } else {
        (" NORMAL ", Token::FgMuted)
    };
    let text = if let Some(message) = &model.message {
        message.clone()
    } else if model.filtering {
        "type to filter · enter keep · esc clear".to_owned()
    } else if let Some(ch) = model.pending {
        format!("{ch}…")
    } else {
        keymap::hints(model.focus.scope()).join(" · ")
    };
    let line = Line::from(vec![
        Span::styled(mode, theme.badge(token)),
        Span::styled(format!(" {text}"), theme.fg(Token::FgMuted)),
    ]);
    frame.render_widget(line, area);
}

/// Returns an empty pane: heavy `ok` border when focused, light `border`
/// otherwise (DESIGN §3, borders).
fn pane(title: &str, focused: bool, theme: Theme) -> Block<'static> {
    let (border_type, border, text) = if focused {
        (BorderType::Thick, Token::Ok, Token::Fg)
    } else {
        (BorderType::Plain, Token::Border, Token::FgMuted)
    };
    Block::bordered()
        .border_type(border_type)
        .border_style(theme.fg(border))
        .title(Span::styled(format!(" {title} "), theme.fg(text)))
}

/// Returns a centred rectangle of at most `width`×`height` inside `area`.
fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let [row] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [cell] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);
    cell
}

/// Returns a dialog frame: double border, title in the top border
/// (DESIGN §5.5).
fn dialog(title: &str, theme: Theme) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Double)
        .border_style(theme.fg(Token::Accent))
        .style(theme.base())
        .title(Span::styled(format!(" {title} "), theme.fg(Token::Fg)))
}

/// Returns `style` in bold when `on`.
fn bold_if(style: Style, on: bool) -> Style {
    if on {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fmt::Write as _;
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::app::model::tests::sample;

    const PROJECTS: &[&str] = &[
        "kedai-web",
        "pasar-mobile",
        "roti-docs",
        "teh-cli",
        "warung-api",
    ];

    /// Renders `model` at `width`×`height` and returns the text plus cursor.
    pub(crate) fn render(model: &mut Model, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, model)).unwrap();
        let backend = terminal.backend();
        let buffer = backend.buffer();
        let mut out = String::new();
        for y in 0..height {
            let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
            writeln!(out, "{}", row.trim_end()).unwrap();
        }
        if backend.cursor_visible() {
            let pos = backend.cursor_position();
            writeln!(out, "cursor: {},{}", pos.x, pos.y).unwrap();
        } else {
            writeln!(out, "cursor: hidden").unwrap();
        }
        out
    }

    /// Compares `got` with `testdata/<name>`, rewriting it under
    /// `UPDATE_GOLDEN=1`.
    pub(crate) fn assert_golden(name: &str, got: &str) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/ui/testdata")
            .join(name);
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, got).unwrap();
        }
        let want = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_GOLDEN=1)", path.display()));
        assert_eq!(got, want, "golden {name} differs");
    }

    /// Sends one plain key press to `model`.
    pub(crate) fn key(model: &mut Model, code: KeyCode) {
        model.update(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    #[test]
    fn main_screen_matches_goldens() {
        for (width, height) in [(120, 40), (80, 24)] {
            let mut model = sample(PROJECTS);
            key(&mut model, KeyCode::Char('j'));
            assert_golden(
                &format!("main-{width}x{height}.txt"),
                &render(&mut model, width, height),
            );
        }
    }

    #[test]
    fn sessions_focus_and_empty_workspace_match_goldens() {
        let mut model = sample(PROJECTS);
        key(&mut model, KeyCode::Enter);
        assert_golden("sessions-focused-80x24.txt", &render(&mut model, 80, 24));
        let mut empty = sample(&[]);
        assert_golden("empty-workspace-120x40.txt", &render(&mut empty, 120, 40));
    }

    #[test]
    fn help_overlay_matches_golden() {
        let mut model = sample(PROJECTS);
        key(&mut model, KeyCode::Char('?'));
        assert_golden("help-120x40.txt", &render(&mut model, 120, 40));
    }

    #[test]
    fn filter_shows_in_title_and_mode_word() {
        let mut model = sample(PROJECTS);
        key(&mut model, KeyCode::Char('/'));
        key(&mut model, KeyCode::Char('t'));
        let screen = render(&mut model, 120, 40);
        assert!(screen.contains("projects /t"), "{screen}");
        assert!(screen.contains("FILTER"), "{screen}");
        assert!(!screen.contains("pasar-mobile"), "{screen}");
    }

    #[test]
    fn narrow_screen_shows_the_size_notice() {
        let screen = render(&mut sample(PROJECTS), 72, 20);
        assert!(
            screen.contains("bungkus-mc needs at least 80×24 (now 72×20)."),
            "{screen}"
        );
    }
}
