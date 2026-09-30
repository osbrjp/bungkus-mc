//! The sessions pane: one card per session of the selected project
//! (DESIGN §5.2, §5.3), or the empty-state copy (§11).

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::app::model::{Focus, Model};
use crate::app::sessions::{Card, State};
use crate::ui::sanitise::truncate;
use crate::ui::theme::{Theme, Token};
use crate::ui::{pane, workspace_label};

/// Rows one card takes, including the blank line after it.
const CARD_ROWS: usize = 4;

/// The ascii spinner (DESIGN §3); one frame per animation tick.
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

/// Draws the sessions pane.
pub(super) fn draw(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let project = model.selected_project();
    let title = project.map_or_else(
        || "sessions".to_owned(),
        |p| format!("sessions · {}", p.name),
    );
    let focused = model.focus == Focus::Sessions;
    let block = pane(&title, focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let indices = model.project_cards();
    if indices.is_empty() {
        draw_empty(frame, inner, model, theme);
        return;
    }
    let per_page = (usize::from(inner.height) / CARD_ROWS).max(1);
    let offset = (model.card + 1).saturating_sub(per_page);
    let width = usize::from(inner.width);
    let spin = if theme.no_color() {
        SPINNER[0]
    } else {
        SPINNER[model.frame % SPINNER.len()]
    };
    let mut lines = Vec::new();
    for (pos, &i) in indices.iter().enumerate().skip(offset).take(per_page) {
        let marker = if pos == model.card && focused {
            ">"
        } else {
            " "
        };
        lines.extend(card_lines(
            &model.cards[i],
            marker,
            width,
            spin,
            model.now,
            theme,
        ));
        lines.push(Line::from(""));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draws the no-sessions / no-projects copy.
fn draw_empty(frame: &mut Frame, inner: Rect, model: &Model, theme: Theme) {
    let workspace = workspace_label(model);
    let text = match (&model.scan_error, model.selected_project()) {
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

/// Returns a card's three lines (DESIGN §5.2):
/// `<marker><gutter><glyph> <badge> #id <name> … <right>`, the state line,
/// and the compact usage line (`-` until usage arrives in M5).
fn card_lines(
    card: &Card,
    marker: &str,
    width: usize,
    spin: char,
    now: Instant,
    theme: Theme,
) -> Vec<Line<'static>> {
    let (glyph, glyph_token, word) = match &card.state {
        State::Running => (spin, Token::Ok, "working"),
        State::Failed(_) => ('x', Token::Err, "failed"),
        State::Stopped => ('#', Token::FgMuted, "stopped"),
        State::Wrapped => ('+', Token::Ok, "wrapped"),
    };
    let gutter = if matches!(card.state, State::Failed(_)) {
        "┃"
    } else {
        " "
    };
    let muted = matches!(card.state, State::Wrapped | State::Stopped);
    let text = if muted {
        theme.fg(Token::FgMuted)
    } else {
        theme.fg(Token::Fg)
    };
    let minutes = card
        .ended
        .unwrap_or(now)
        .saturating_duration_since(card.started)
        .as_secs()
        / 60;
    let right = if card.running() {
        format!("{minutes}m")
    } else {
        word.to_owned()
    };
    let id = card.id.short();
    let fixed = 2 + 2 + 2 + id.len() + 1;
    let room = width.saturating_sub(fixed + right.len() + 1);
    let name = truncate(&card.name, room);
    let pad = width.saturating_sub(fixed + name.chars().count() + right.len());
    let title = Line::from(vec![
        Span::styled(marker.to_owned(), theme.fg(Token::Ok)),
        Span::styled(gutter, theme.fg(Token::Err)),
        Span::styled(glyph.to_string(), theme.fg(glyph_token)),
        Span::raw(" "),
        Span::styled(card.kind.badge().to_string(), theme.fg(Token::Accent)),
        Span::raw(" "),
        Span::styled(id, theme.fg(Token::Info)),
        Span::raw(" "),
        Span::styled(name, text.add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, theme.fg(Token::FgMuted)),
    ]);
    let detail = match &card.state {
        State::Running if card.pty.as_ref().and_then(|p| p.last_output).is_none() => {
            "working · warming up".to_owned()
        }
        State::Running => format!("working · {}", card.kind.command()),
        State::Failed(reason) => reason.clone(),
        State::Stopped => "stopped".to_owned(),
        State::Wrapped => format!("wrapped in {minutes}m"),
    };
    let body = |s: String| {
        Line::from(vec![
            Span::raw(" "),
            Span::styled(gutter, theme.fg(Token::Err)),
            Span::styled(format!("  {}", truncate(&s, width.saturating_sub(4))), text),
        ])
    };
    vec![title, body(detail), body("- tok · - · ctx -".to_owned())]
}
