//! Rendering: header, the three panes and the getah bar (DESIGN §4).
//!
//! Views are pure functions of their inputs and draw into a ratatui
//! [`Frame`]; they never read the environment or the terminal.

pub(crate) mod theme;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

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
/// Below [`MIN_SIZE`] only a one-line notice is drawn. Below
/// [`THREE_PANE_WIDTH`] columns the projects pane fills the body alone
/// (the single-pane stack); otherwise projects, sessions and output sit
/// side by side. The projects pane is focused.
///
/// # Arguments
///
/// * `frame` - The frame to draw into; its area is the whole terminal.
/// * `theme` - Colours for this terminal.
pub(crate) fn draw(frame: &mut Frame, theme: Theme) {
    let area = frame.area();
    frame.render_widget(Block::new().style(theme.base()), area);
    if area.width < MIN_SIZE.0 || area.height < MIN_SIZE.1 {
        draw_too_small(frame, area);
        return;
    }
    let [header, body, getah] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(frame, header, theme);
    if area.width >= THREE_PANE_WIDTH {
        let [projects, sessions, output] = Layout::horizontal([
            Constraint::Length(PROJECTS_WIDTH),
            Constraint::Length(SESSIONS_WIDTH),
            Constraint::Fill(1),
        ])
        .areas(body);
        frame.render_widget(pane("projects", true, theme), projects);
        frame.render_widget(pane("sessions", false, theme), sessions);
        frame.render_widget(pane("output", false, theme), output);
    } else {
        frame.render_widget(pane("projects", true, theme), body);
    }
    draw_getah(frame, getah, theme);
}

/// Draws the centred "too small" notice (DESIGN §11).
fn draw_too_small(frame: &mut Frame, area: Rect) {
    let text = format!(
        "bungkus-mc needs at least {}×{} (now {}×{}).",
        MIN_SIZE.0, MIN_SIZE.1, area.width, area.height
    );
    let [row] = Layout::vertical([Constraint::Length(1)])
        .flex(ratatui::layout::Flex::Center)
        .areas(area);
    frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), row);
}

/// Draws line 1: the app name left, the version right.
fn draw_header(frame: &mut Frame, area: Rect, theme: Theme) {
    let name = Span::styled(
        " bungkus-mc",
        theme.fg(Token::Accent).add_modifier(Modifier::BOLD),
    );
    let version = format!("v{} ", env!("CARGO_PKG_VERSION"));
    frame.render_widget(Line::from(name), area);
    frame.render_widget(
        Line::styled(version, theme.fg(Token::FgMuted)).alignment(Alignment::Right),
        area,
    );
}

/// Draws the last line: the mode word as a badge, then the key hints.
fn draw_getah(frame: &mut Frame, area: Rect, theme: Theme) {
    let line = Line::from(vec![
        Span::styled(" NORMAL ", theme.badge(Token::FgMuted)),
        Span::styled(" q quit", theme.fg(Token::FgMuted)),
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

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::ui::theme::{Background, Profile};

    /// Renders the screen at `width`×`height` with no colour (the golden
    /// setting, `CODING_RULES.md` §2) and returns the text plus cursor.
    fn render(width: u16, height: u16) -> String {
        let theme = Theme::new(Profile::NoColor, Background::Paint);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, theme)).unwrap();
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
    fn assert_golden(name: &str, got: &str) {
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

    #[test]
    fn empty_layout_matches_goldens() {
        for (width, height) in [(120, 40), (80, 24)] {
            assert_golden(
                &format!("empty-{width}x{height}.txt"),
                &render(width, height),
            );
        }
    }

    #[test]
    fn narrow_screen_shows_the_size_notice() {
        let screen = render(72, 20);
        assert!(
            screen.contains("bungkus-mc needs at least 80×24 (now 72×20)."),
            "{screen}"
        );
    }
}
