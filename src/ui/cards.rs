//! The sessions pane: one card per session of the selected project
//! (DESIGN §5.2, §5.3), or the empty-state copy (§11).

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::app::model::{Focus, Model};
use crate::app::sessions::{Card, State};
use crate::ui::sanitise::truncate;
use crate::ui::theme::{Theme, Token};
use crate::ui::{SPINNER, pane, workspace_label};

/// Most subagent rows shown per card; the newest are kept.
const SUBAGENT_ROWS: usize = 5;

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
    let width = usize::from(inner.width);
    let spin = spinner(model.frame, theme);
    let cards: Vec<Vec<Line>> = indices
        .iter()
        .enumerate()
        .map(|(pos, &i)| {
            let marker = if pos == model.card && focused {
                ">"
            } else {
                " "
            };
            let mut lines = card_lines(&model.cards[i], marker, width, spin, model.now, theme);
            lines.push(Line::from(""));
            lines
        })
        .collect();
    let rows = usize::from(inner.height);
    let mut start = model.card.min(cards.len() - 1);
    let mut used = cards[start].len();
    while start > 0 && used + cards[start - 1].len() <= rows {
        start -= 1;
        used += cards[start].len();
    }
    let lines: Vec<Line> = cards.into_iter().skip(start).flatten().collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Returns the spinner frame for `frame`, or its first frame without
/// colour (motion off, DESIGN §7).
pub(super) fn spinner(frame: usize, theme: Theme) -> char {
    if theme.no_color() {
        SPINNER[0]
    } else {
        SPINNER[frame % SPINNER.len()]
    }
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

/// Returns the glyph, its colour and the state word (DESIGN §5.3).
pub(super) fn glyph(state: &State, spin: char) -> (char, Token, &'static str) {
    match state {
        State::Working => (spin, Token::Ok, "working"),
        State::YourTurn => ('~', Token::Info, "your turn"),
        State::NeedsYou => ('!', Token::Warn, "needs you"),
        State::Failed(_) => ('x', Token::Err, "failed"),
        State::Stopped => ('#', Token::FgMuted, "stopped"),
        State::Wrapped => ('+', Token::Ok, "wrapped"),
    }
}

/// Returns whole minutes between `from` and `to`.
fn minutes(from: Instant, to: Instant) -> u64 {
    to.saturating_duration_since(from).as_secs() / 60
}

/// Returns a card's lines (DESIGN §5.2): the title line, the state line,
/// the compact usage line (`-` until M5), then one line per subagent.
fn card_lines(
    card: &Card,
    marker: &str,
    width: usize,
    spin: char,
    now: Instant,
    theme: Theme,
) -> Vec<Line<'static>> {
    let (glyph, glyph_token, word) = glyph(&card.state, spin);
    let (gutter, gutter_token) = match card.state {
        State::NeedsYou => ("┃", Token::Warn),
        State::Failed(_) => ("┃", Token::Err),
        _ => (" ", Token::Fg),
    };
    let muted = matches!(card.state, State::Wrapped | State::Stopped);
    let text = if muted {
        theme.fg(Token::FgMuted)
    } else {
        theme.fg(Token::Fg)
    };
    let elapsed = minutes(card.started, card.ended.unwrap_or(now));
    let right = match card.state {
        State::Working | State::NeedsYou => format!("{elapsed}m"),
        _ => word.to_owned(),
    };
    let id = card.id.short();
    let fixed = 2 + 2 + 2 + id.len() + 1;
    let name = truncate(&card.name, width.saturating_sub(fixed + right.len() + 1));
    let pad = width.saturating_sub(fixed + name.chars().count() + right.len());
    let title_style = if muted {
        text
    } else {
        text.add_modifier(Modifier::BOLD)
    };
    let title = Line::from(vec![
        Span::styled(marker.to_owned(), theme.fg(Token::Ok)),
        Span::styled(gutter, theme.fg(gutter_token)),
        Span::styled(glyph.to_string(), theme.fg(glyph_token)),
        Span::raw(" "),
        Span::styled(card.kind.badge().to_string(), theme.fg(Token::Accent)),
        Span::raw(" "),
        Span::styled(id, theme.fg(Token::Info)),
        Span::raw(" "),
        Span::styled(name, title_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, theme.fg(Token::FgMuted)),
    ]);
    let body_width = width.saturating_sub(4);
    let body = |s: &str, style: Style| {
        Line::from(vec![
            Span::raw(" "),
            Span::styled(gutter, theme.fg(gutter_token)),
            Span::styled(format!("  {}", truncate(s, body_width)), style),
        ])
    };
    let mut lines = vec![
        title,
        body(&detail(card, now), text),
        body("- tok · - · ctx -", text),
    ];
    let skip = card.subagents.len().saturating_sub(SUBAGENT_ROWS);
    for sub in card.subagents.iter().skip(skip) {
        let mark = if sub.ended.is_some() { '+' } else { spin };
        let time = format!("{}m", minutes(sub.started, sub.ended.unwrap_or(now)));
        let right_len = time.len() + 2;
        let desc = truncate(&sub.description, body_width.saturating_sub(right_len + 2));
        let pad = body_width.saturating_sub(desc.chars().count() + 2 + right_len);
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(gutter, theme.fg(gutter_token)),
            Span::styled("  * ", theme.fg(Token::FgMuted)),
            Span::styled(desc, text),
            Span::raw(" ".repeat(pad)),
            Span::styled(format!("{mark} "), theme.fg(Token::Ok)),
            Span::styled(time, theme.fg(Token::FgMuted)),
        ]));
    }
    lines
}

/// Returns the state line: state word and detail (DESIGN §5.2).
fn detail(card: &Card, now: Instant) -> String {
    if card.output_only(now) {
        return "output only · live tree unavailable".to_owned();
    }
    match &card.state {
        State::Working if card.pty.as_ref().and_then(|p| p.last_output).is_none() => {
            "working · warming up".to_owned()
        }
        State::Working => match &card.tool {
            Some(tool) => format!("working · {tool}"),
            None => "working".to_owned(),
        },
        State::NeedsYou => match &card.tool {
            Some(tool) => format!("needs you · permission: {tool}"),
            None => "needs you".to_owned(),
        },
        State::YourTurn => match card.last_message.as_deref().and_then(|m| m.lines().next()) {
            Some(first) => format!("done: \"{first}\""),
            None => "your turn".to_owned(),
        },
        State::Failed(reason) => reason.clone(),
        State::Stopped => "stopped".to_owned(),
        State::Wrapped => {
            let m = minutes(card.started, card.ended.unwrap_or(now));
            let subs = card.subagents.len();
            let plural = if subs == 1 { "" } else { "s" };
            format!("{m}m · {} tools · {subs} subagent{plural}", card.tool_calls)
        }
    }
}
