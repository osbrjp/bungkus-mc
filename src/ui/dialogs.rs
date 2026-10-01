//! The `n` picker and the stop/quit confirm dialogs (DESIGN §5.5).

use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::agent::Kind;
use crate::app::model::Model;
use crate::app::picker::{Picker, Row};
use crate::app::stop::{StopDialog, StopKind, Target};
use crate::term::SessionId;
use crate::ui::sanitise::truncate;
use crate::ui::theme::{Theme, Token};
use crate::ui::{bold_if, centred, dialog as dialog_block};

/// Inner width of the picker's text fields.
const FIELD: usize = 32;

/// Most sessions a confirm dialog lists before `… and N more`.
const CONFIRM_ROWS: usize = 8;

/// Draws the `n` picker: agent, model, name and prompt rows.
pub(super) fn draw_picker(frame: &mut Frame, area: Rect, p: &Picker, theme: Theme) {
    let rect = centred(area, 54, 11);
    frame.render_widget(Clear, rect);
    let block = dialog_block(&format!("new session · {}", p.project), theme);
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

/// Draws the quit / stop dialog (DESIGN §5.5): the sessions, then every
/// tracked process as `basename [:ports] pid n`, each with its
/// `[stop]`/`[keep]` state; after [`CONFIRM_ROWS`] rows it scrolls and
/// says `… and N more`.
pub(super) fn draw_stop(
    frame: &mut Frame,
    area: Rect,
    dialog: &StopDialog,
    model: &Model,
    theme: Theme,
) {
    let sessions = dialog
        .rows
        .iter()
        .filter(|r| matches!(r.target, Target::Session(_)))
        .count();
    let (title, heading, keys) = match dialog.kind {
        StopKind::Quit => (
            "quit?",
            match sessions {
                0 => "Stopping what your sessions started:".to_owned(),
                1 => "Stopping 1 session and what it started:".to_owned(),
                n => format!("Stopping {n} sessions and what they started:"),
            },
            "space keep/stop · y quit · n stay  ",
        ),
        StopKind::Session(_) => (
            "stop?",
            "Stopping this session and what it started:".to_owned(),
            "space keep/stop · y stop · n keep  ",
        ),
    };
    let mut lines = vec![
        Line::from(""),
        Line::styled(format!("  {heading}"), theme.fg(Token::Fg)),
    ];
    lines.push(Line::from(""));
    let start = dialog.cursor.saturating_sub(CONFIRM_ROWS - 1);
    for (i, row) in dialog
        .rows
        .iter()
        .enumerate()
        .skip(start)
        .take(CONFIRM_ROWS)
    {
        let marker = if i == dialog.cursor { ">" } else { " " };
        let (word, token) = if row.stop {
            ("[stop]", Token::Warn)
        } else {
            ("[keep]", Token::Ok)
        };
        let mut spans = vec![
            Span::styled(format!(" {marker}"), theme.fg(Token::Ok)),
            Span::styled(format!("{word} "), theme.fg(token)),
        ];
        match &row.target {
            Target::Session(id) => {
                if let Some(card) = model.cards.iter().find(|c| c.id == *id) {
                    spans.push(Span::styled(
                        format!("{} ", card.kind.badge()),
                        theme.fg(Token::Accent),
                    ));
                    spans.push(Span::styled(
                        format!("{} ", card.id.short()),
                        theme.fg(Token::Info),
                    ));
                    spans.push(Span::styled(truncate(&card.name, 28), theme.fg(Token::Fg)));
                }
            }
            Target::Process { proc, ports, .. } => {
                let ports = ports
                    .iter()
                    .map(|p| format!(" :{p}"))
                    .collect::<Vec<_>>()
                    .concat();
                let text = format!("{}{ports} pid {}", proc.name(), proc.pid);
                spans.push(Span::styled(truncate(&text, 36), theme.fg(Token::Fg)));
            }
        }
        lines.push(Line::from(spans));
    }
    let hidden = dialog.rows.len().saturating_sub(start + CONFIRM_ROWS);
    if hidden > 0 {
        lines.push(Line::styled(
            format!("  … and {hidden} more"),
            theme.fg(Token::FgMuted),
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(keys, theme.fg(Token::FgMuted)).alignment(Alignment::Right));
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let rect = centred(area, 50, height);
    frame.render_widget(Clear, rect);
    let block = dialog_block(title, theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draws the "forget this session?" confirm: mc drops it from its list;
/// the agent keeps its own history.
pub(super) fn draw_forget(
    frame: &mut Frame,
    area: Rect,
    id: SessionId,
    model: &Model,
    theme: Theme,
) {
    let name = model
        .cards
        .iter()
        .find(|c| c.id == id)
        .map_or_else(String::new, |c| c.name.clone());
    let lines = vec![
        Line::from(""),
        Line::styled(
            format!("  Forget {} {}?", id.short(), truncate(&name, 28)),
            theme.fg(Token::Fg),
        ),
        Line::styled(
            "  It stays in the agent's own history.",
            theme.fg(Token::FgMuted),
        ),
        Line::from(""),
        Line::styled("y forget · n keep  ", theme.fg(Token::FgMuted)).alignment(Alignment::Right),
    ];
    let rect = centred(area, 50, 7);
    frame.render_widget(Clear, rect);
    let block = dialog_block("forget?", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draws the take-over dialog: what happens, and what the user must do
/// in the other terminal (DESIGN §5.9).
pub(super) fn draw_take_over(
    frame: &mut Frame,
    area: Rect,
    ext: &crate::external::External,
    theme: Theme,
) {
    let lines = vec![
        Line::from(""),
        Line::styled(
            format!("  Take over {}?", truncate(&ext.name, 40)),
            theme.fg(Token::Fg),
        ),
        Line::styled(
            "  mc resumes it here (claude --resume) once it closes.",
            theme.fg(Token::FgMuted),
        ),
        Line::styled(
            format!("  Quit it in its own terminal (/exit) — pid {}.", ext.pid),
            theme.fg(Token::FgMuted),
        ),
        Line::from(""),
        Line::styled(
            "waiting for it to close · esc cancel  ",
            theme.fg(Token::FgMuted),
        )
        .alignment(Alignment::Right),
    ];
    let rect = centred(area, 60, 8);
    frame.render_widget(Clear, rect);
    let block = dialog_block("take over", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draws the `r` agent choice: whose list of past sessions to open.
pub(super) fn draw_resume_agent(frame: &mut Frame, area: Rect, kind: Kind, theme: Theme) {
    let choice = |k: Kind| {
        let (text, style) = if k == kind {
            (
                format!("> {}", k.command()),
                bold_if(theme.fg(Token::Accent), true),
            )
        } else {
            (format!("  {}", k.command()), theme.fg(Token::Fg))
        };
        Span::styled(format!("{text:<10}"), style)
    };
    let lines = vec![
        Line::from(""),
        Line::styled("  Resume a past session with", theme.fg(Token::Fg)),
        Line::from(vec![
            Span::raw("  "),
            choice(Kind::Claude),
            choice(Kind::Codex),
        ]),
        Line::styled(
            "  Its own list of this project's sessions opens.",
            theme.fg(Token::FgMuted),
        ),
        Line::from(""),
        Line::styled(
            "← → agent · enter open · esc cancel  ",
            theme.fg(Token::FgMuted),
        )
        .alignment(Alignment::Right),
    ];
    let rect = centred(area, 54, 8);
    frame.render_widget(Clear, rect);
    let block = dialog_block("resume", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draws the quick-session move dialog: the projects to move into, or the
/// new project's name (issue #46).
pub(super) fn draw_move(
    frame: &mut Frame,
    area: Rect,
    dialog: &crate::app::quick::MoveDialog,
    model: &Model,
    theme: Theme,
) {
    use crate::app::quick::MoveDialog;
    let (title, mut lines, hint) = match dialog {
        MoveDialog::Pick { selected, .. } => {
            let rows = usize::from(area.height.saturating_sub(10)).max(3);
            let start = (selected + 1).saturating_sub(rows);
            let lines: Vec<Line> = model
                .projects
                .iter()
                .enumerate()
                .skip(start)
                .take(rows)
                .map(|(i, p)| {
                    if i == *selected {
                        Line::styled(
                            format!("  > {}", truncate(&p.name, 50)),
                            bold_if(theme.fg(Token::Accent), true),
                        )
                    } else {
                        Line::styled(
                            format!("    {}", truncate(&p.name, 50)),
                            theme.fg(Token::Fg),
                        )
                    }
                })
                .collect();
            (
                "move to project",
                lines,
                "↑↓ pick · enter move · esc cancel  ",
            )
        }
        MoveDialog::Create { name, error, .. } => {
            let mut lines = vec![
                Line::styled(
                    "  New project folder in the workspace:",
                    theme.fg(Token::Fg),
                ),
                Line::styled(format!("  > {name}▏"), theme.fg(Token::Accent)),
            ];
            if let Some(e) = error {
                lines.push(Line::styled(format!("  {e}"), theme.fg(Token::Err)));
            } else {
                lines.push(Line::styled(
                    "  mc creates it, runs git init and moves the session there.",
                    theme.fg(Token::FgMuted),
                ));
            }
            ("new project", lines, "enter create · esc cancel  ")
        }
    };
    lines.insert(0, Line::from(""));
    lines.push(Line::from(""));
    lines.push(Line::styled(hint, theme.fg(Token::FgMuted)).alignment(Alignment::Right));
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let rect = centred(area, 64, height);
    frame.render_widget(Clear, rect);
    let block = dialog_block(title, theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draws "stop this outside session?": `x` on a session started outside
/// mc, which gets SIGTERM only after `y`.
pub(super) fn draw_stop_outside(
    frame: &mut Frame,
    area: Rect,
    ext: &crate::external::External,
    theme: Theme,
) {
    let lines = vec![
        Line::from(""),
        Line::styled(
            format!("  Stop {} (pid {})?", truncate(&ext.name, 36), ext.pid),
            theme.fg(Token::Fg),
        ),
        Line::styled(
            "  It was started outside mc; its terminal will show it ended.",
            theme.fg(Token::FgMuted),
        ),
        Line::from(""),
        Line::styled("y stop · n keep  ", theme.fg(Token::FgMuted)).alignment(Alignment::Right),
    ];
    let rect = centred(area, 64, 7);
    frame.render_widget(Clear, rect);
    let block = dialog_block("stop?", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}
