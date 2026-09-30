//! The `n` picker and the stop/quit confirm dialogs (DESIGN §5.5).

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::agent::Kind;
use crate::app::model::{Confirm, Model};
use crate::app::picker::{Picker, Row};
use crate::ui::sanitise::truncate;
use crate::ui::theme::{Theme, Token};
use crate::ui::{bold_if, centred, dialog};

/// Inner width of the picker's text fields.
const FIELD: usize = 32;

/// Most sessions a confirm dialog lists before `… and N more`.
const CONFIRM_ROWS: usize = 8;

/// Draws the `n` picker: agent, model, name and prompt rows.
pub(super) fn draw_picker(frame: &mut Frame, area: Rect, p: &Picker, theme: Theme) {
    let rect = centred(area, 54, 11);
    frame.render_widget(Clear, rect);
    let block = dialog(&format!("new session · {}", p.project), theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let label = |row: Row, text: &str| {
        let style = bold_if(theme.fg(Token::Fg), p.row == row);
        Span::styled(format!("  {text:<8}"), style)
    };
    let choice = |text: &str, chosen: bool, missing: bool| {
        let mut word = if chosen {
            format!("> {text}")
        } else {
            format!("  {text}")
        };
        if missing {
            word.push_str(" (not on PATH)");
        }
        let token = match (chosen, missing) {
            (true, _) => Token::Accent,
            (false, true) => Token::FgDim,
            (false, false) => Token::FgMuted,
        };
        Span::styled(format!("{word}  "), theme.fg(token))
    };
    let mut agents = vec![label(Row::Agent, "agent")];
    for kind in Kind::ALL {
        agents.push(choice(
            kind.command(),
            p.agent == kind,
            !p.installed[kind as usize],
        ));
    }
    let mut models = vec![label(Row::Model, "model")];
    for (i, model) in p.agent.models().iter().enumerate() {
        models.push(choice(model, p.model == i, false));
    }
    let field = |row: Row, text: &str| {
        let shown = tail(text, FIELD - 1);
        let bar = theme.fg(if p.row == row {
            Token::Ok
        } else {
            Token::Border
        });
        let pad = FIELD.saturating_sub(shown.chars().count());
        let name = match row {
            Row::Name => "name",
            Row::Prompt | Row::Agent | Row::Model => "prompt",
        };
        Line::from(vec![
            label(row, name),
            Span::styled("┃", bar),
            Span::styled(shown, theme.fg(Token::Fg)),
            Span::raw(" ".repeat(pad)),
            Span::styled("┃", bar),
        ])
    };
    let lines = vec![
        Line::from(""),
        Line::from(agents),
        Line::from(models),
        field(Row::Name, &p.name),
        field(Row::Prompt, &p.prompt),
        Line::from(""),
        Line::styled("  prompt and name are optional", theme.fg(Token::FgMuted)),
        Line::styled(
            "enter start · tab next · esc cancel  ",
            theme.fg(Token::FgMuted),
        )
        .alignment(Alignment::Right),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
    let (row_y, text) = match p.row {
        Row::Name => (3, &p.name),
        Row::Prompt => (4, &p.prompt),
        Row::Agent | Row::Model => return,
    };
    let len = u16::try_from(tail(text, FIELD - 1).chars().count()).unwrap_or(0);
    frame.set_cursor_position(Position::new(inner.x + 11 + len, inner.y + row_y));
}

/// Returns the last `max` characters of `text`.
fn tail(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(max)..].iter().collect()
}

/// Draws a stop or quit confirm listing the sessions it stops.
pub(super) fn draw_confirm(
    frame: &mut Frame,
    area: Rect,
    confirm: Confirm,
    model: &Model,
    theme: Theme,
) {
    let (title, targets): (&str, Vec<usize>) = match confirm {
        Confirm::Quit => (
            "quit?",
            (0..model.cards.len())
                .filter(|&i| model.cards[i].running())
                .collect(),
        ),
        Confirm::Stop(id) => (
            "stop?",
            model
                .cards
                .iter()
                .position(|c| c.id == id)
                .into_iter()
                .collect(),
        ),
    };
    let count = targets.len();
    let heading = match (confirm, count) {
        (Confirm::Quit, 1) => "Stopping 1 session:".to_owned(),
        (Confirm::Quit, n) => format!("Stopping {n} sessions:"),
        (Confirm::Stop(_), _) => "Stopping this session:".to_owned(),
    };
    let mut lines = vec![
        Line::from(""),
        Line::styled(format!("  {heading}"), theme.fg(Token::Fg)),
    ];
    lines.push(Line::from(""));
    for &i in targets.iter().take(CONFIRM_ROWS) {
        let card = &model.cards[i];
        lines.push(Line::from(vec![
            Span::styled("  [stop] ", theme.fg(Token::Warn)),
            Span::styled(format!("{} ", card.kind.badge()), theme.fg(Token::Accent)),
            Span::styled(format!("{} ", card.id.short()), theme.fg(Token::Info)),
            Span::styled(truncate(&card.name, 28), theme.fg(Token::Fg)),
        ]));
    }
    if count > CONFIRM_ROWS {
        lines.push(Line::styled(
            format!("  … and {} more", count - CONFIRM_ROWS),
            theme.fg(Token::FgMuted),
        ));
    }
    lines.push(Line::from(""));
    let keys = match confirm {
        Confirm::Quit => "y quit · n stay  ",
        Confirm::Stop(_) => "y stop · n keep  ",
    };
    lines.push(Line::styled(keys, theme.fg(Token::FgMuted)).alignment(Alignment::Right));
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let rect = centred(area, 46, height);
    frame.render_widget(Clear, rect);
    let block = dialog(title, theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}
