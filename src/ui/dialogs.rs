//! The `n` picker and the stop/quit confirm dialogs (DESIGN §5.5).

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Position, Rect};
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
            "enter start · j/k tab next · h/l change · esc  ",
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
            "space keep/stop · y/enter stop · n/esc keep  ",
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
                        format!("{} ", theme.icons.agent(card.kind)),
                        theme.agent_style(card.kind),
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
        Line::styled("y/enter forget · n/esc keep  ", theme.fg(Token::FgMuted))
            .alignment(Alignment::Right),
    ];
    let rect = centred(area, 50, 7);
    frame.render_widget(Clear, rect);
    let block = dialog_block("forget?", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draws the "remove unused worktrees?" confirm: only worktrees no session
/// runs in or can resume into, never one with uncommitted files, and the
/// branches stay.
pub(super) fn draw_clean_worktrees(
    frame: &mut Frame,
    area: Rect,
    project: &crate::workspace::Project,
    theme: Theme,
) {
    let lines = vec![
        Line::from(""),
        Line::styled(
            format!(
                "  Remove unused worktrees of {}?",
                truncate(&project.name, 20)
            ),
            theme.fg(Token::Fg),
        ),
        Line::styled(
            "  Ones with uncommitted files stay.",
            theme.fg(Token::FgMuted),
        ),
        Line::styled("  Branches are kept.", theme.fg(Token::FgMuted)),
        Line::from(""),
        Line::styled("y/enter remove · n/esc keep  ", theme.fg(Token::FgMuted))
            .alignment(Alignment::Right),
    ];
    let rect = centred(area, 54, 8);
    frame.render_widget(Clear, rect);
    let block = dialog_block("clean worktrees?", theme);
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
            "h/l ← → agent · enter open · esc cancel  ",
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

/// Draws the quick-session move dialog (issue #46): the mascot, the
/// bordered field, [`SLOTS`] project rows (recent while empty, the best
/// matches while typing) and the create row. Its height never changes.
///
/// [`SLOTS`]: crate::app::quick::SLOTS
pub(super) fn draw_move(
    frame: &mut Frame,
    area: Rect,
    dialog: &crate::app::quick::MoveDialog,
    model: &Model,
    theme: Theme,
) {
    use crate::ui::mascot::{self, Mascot, Mood};

    let list = move_rows(dialog, model, theme);
    let error = dialog.error.as_deref().map_or_else(
        || Line::from(""),
        |e| Line::styled(format!("  {e}"), theme.fg(Token::Err)),
    );
    let list_rows = u16::try_from(list.len()).unwrap_or(u16::MAX);
    let with_mascot = area.height >= list_rows + 22;
    let sprite_rows = if with_mascot { mascot::HEIGHT + 1 } else { 0 };
    let height = 2 + sprite_rows + 1 + 3 + list_rows + 2;
    let rect = centred(area, 62, height);
    frame.render_widget(Clear, rect);
    let block = dialog_block("move to project", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let [sprite, title, field, items, err, hint] = Layout::vertical([
        Constraint::Length(sprite_rows),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(list_rows),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    if with_mascot {
        let [spot] = Layout::horizontal([Constraint::Length(mascot::WIDTH)])
            .flex(Flex::Center)
            .areas(Rect {
                y: sprite.y + 1,
                height: mascot::HEIGHT,
                ..sprite
            });
        frame.render_widget(
            Mascot {
                theme,
                pose: Mood::YourTurn.pose(model.frame, theme.animated()),
                mini: false,
            },
            spot,
        );
    }
    frame.render_widget(
        Line::styled(
            "  Move into a project, or name a new one:",
            theme.fg(Token::Fg),
        ),
        title,
    );
    draw_field(frame, field, &dialog.query, theme);
    frame.render_widget(Paragraph::new(list), items);
    frame.render_widget(error, err);
    frame.render_widget(
        Line::styled(
            "↑↓ tab ctrl-j/k pick · enter move/create · esc cancel  ",
            theme.fg(Token::FgMuted),
        )
        .alignment(Alignment::Right),
        hint,
    );
}

/// Returns the move dialog's list: the label, [`SLOTS`] project rows
/// (blank when fewer) and the create row.
///
/// [`SLOTS`]: crate::app::quick::SLOTS
fn move_rows(
    dialog: &crate::app::quick::MoveDialog,
    model: &Model,
    theme: Theme,
) -> Vec<Line<'static>> {
    use crate::app::quick::{MoveOption, SLOTS};

    let options = model.move_options(&dialog.query);
    let row = |i: usize, text: String| {
        let chosen = i == dialog.selected;
        let style = if chosen {
            bold_if(theme.fg(Token::Accent), true)
        } else {
            theme.fg(Token::Fg)
        };
        Line::styled(
            format!("  {}{text}", if chosen { "> " } else { "  " }),
            style,
        )
    };
    let typed = !dialog.query.trim().is_empty();
    let mut list = vec![Line::styled(
        if typed { "  best matches" } else { "  recent" },
        theme.fg(Token::FgMuted),
    )];
    let projects = options
        .iter()
        .take_while(|o| matches!(o, MoveOption::Project { .. }))
        .count();
    for i in 0..SLOTS {
        list.push(match options.get(i) {
            Some(MoveOption::Project { name, .. }) => row(i, truncate(name, 48)),
            _ => Line::from(""),
        });
    }
    list.push(match options.get(projects) {
        Some(MoveOption::Create(name)) => row(
            projects,
            format!("+ enter to create {}", truncate(name, 36)),
        ),
        _ => Line::styled(
            "    type a new name to create a project",
            theme.fg(Token::FgMuted),
        ),
    });
    list
}

/// Draws a bordered text field in `area` (2 columns in on each side)
/// holding the end of `text`, with the cursor after it.
fn draw_field(frame: &mut Frame, area: Rect, text: &str, theme: Theme) {
    let field = Rect {
        x: area.x + 2,
        width: area.width.saturating_sub(4),
        ..area
    };
    let field_block = super::field_block(theme);
    let inner = field_block.inner(field);
    frame.render_widget(field_block, field);
    let room = usize::from(inner.width).saturating_sub(2);
    let count = text.chars().count();
    let shown: String = text.chars().skip(count.saturating_sub(room)).collect();
    frame.render_widget(
        Line::styled(format!(" {shown}"), theme.fg(Token::Fg)),
        inner,
    );
    let cursor = u16::try_from(shown.chars().count()).unwrap_or(0);
    frame.set_cursor_position(Position::new(inner.x + 1 + cursor, inner.y));
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
        Line::styled("y/enter stop · n/esc keep  ", theme.fg(Token::FgMuted))
            .alignment(Alignment::Right),
    ];
    let rect = centred(area, 64, 7);
    frame.render_widget(Clear, rect);
    let block = dialog_block("stop?", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draws "move this project to the Trash?" (`dd` in the projects pane).
pub(super) fn draw_trash_project(
    frame: &mut Frame,
    area: Rect,
    projects: &[crate::workspace::Project],
    model: &Model,
    theme: Theme,
) {
    let (question, detail) = match projects {
        [one] => (
            format!("  Move {} to the Trash?", truncate(&one.name, 40)),
            crate::store::config::tilde(&one.path, model.home.as_deref()),
        ),
        many => (
            format!("  Move {} projects to the Trash?", many.len()),
            many.iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ),
    };
    let lines = vec![
        Line::from(""),
        Line::styled(question, theme.fg(Token::Fg)),
        Line::styled(
            format!("  {}", truncate(&detail, 56)),
            theme.fg(Token::FgMuted),
        ),
        Line::styled(
            "  Whole folders go; u puts them back, or use the Trash.",
            theme.fg(Token::FgMuted),
        ),
        Line::from(""),
        Line::styled(
            "y/enter move to Trash · n/esc keep  ",
            theme.fg(Token::FgMuted),
        )
        .alignment(Alignment::Right),
    ];
    let rect = centred(area, 64, 8);
    frame.render_widget(Clear, rect);
    let block = dialog_block("delete project?", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draws the `a` new-project dialog: the bordered name field and the two
/// kinds, fresh repository or with agent files.
pub(super) fn draw_new_project(
    frame: &mut Frame,
    area: Rect,
    dialog: &crate::app::quick::NewProject,
    theme: Theme,
) {
    let kind = |chosen: bool, text: &str| {
        let style = if chosen {
            bold_if(theme.fg(Token::Accent), true)
        } else {
            theme.fg(Token::Fg)
        };
        Line::styled(
            format!("  {}{text}", if chosen { "> " } else { "  " }),
            style,
        )
    };
    let mut lines = vec![
        kind(
            dialog.agent_files,
            "with agent files: .git + AGENTS.md + CLAUDE.md (@AGENTS.md)",
        ),
        kind(!dialog.agent_files, "fresh: .git only"),
    ];
    lines.push(dialog.error.as_deref().map_or_else(
        || Line::from(""),
        |e| Line::styled(format!("  {e}"), theme.fg(Token::Err)),
    ));
    lines.push(
        Line::styled(
            "tab switch · enter create · esc cancel  ",
            theme.fg(Token::FgMuted),
        )
        .alignment(Alignment::Right),
    );
    let rect = centred(area, 66, 11);
    frame.render_widget(Clear, rect);
    let block = dialog_block("new project", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let [title, field, kinds] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Fill(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::styled("  Folder name in the workspace:", theme.fg(Token::Fg)),
        ]),
        title,
    );
    draw_field(frame, field, &dialog.name, theme);
    frame.render_widget(Paragraph::new(lines), kinds);
}

/// Returns `text` cut to `max` characters from the front (`…/OSBR`), so
/// the end of a long path, which tells workspaces apart, stays visible.
fn keep_end(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(n - max.saturating_sub(1)).collect();
    format!("…{tail}")
}

/// Draws the finder (`fp`, `ff`, `fg`): the bordered query field, the rows
/// scrolled to keep the highlighted one in view, and a hint line with how
/// many of the matches show. A long file path keeps its end (the file
/// name); a long grep line keeps its start.
pub(super) fn draw_finder(
    frame: &mut Frame,
    area: Rect,
    finder: &crate::app::finder::Finder,
    theme: Theme,
) {
    use crate::app::finder::Source;

    let rect = centred(
        area,
        area.width.saturating_sub(4).min(110),
        area.height.saturating_sub(2).min(30),
    );
    frame.render_widget(Clear, rect);
    let block = dialog_block(finder.source.title(), theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let [field, list, hint] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    draw_field(frame, field, &finder.query, theme);
    let shown = usize::from(list.height);
    let room = usize::from(list.width).saturating_sub(6);
    let lines: Vec<Line> = finder
        .rows
        .iter()
        .enumerate()
        .skip((finder.selected + 1).saturating_sub(shown))
        .take(shown)
        .map(|(i, row)| {
            let text = match finder.source {
                Source::Grep => crate::ui::sanitise::sanitise(row, room),
                Source::Projects | Source::Files => {
                    keep_end(&crate::ui::sanitise::sanitise(row, 4096), room)
                }
            };
            if i == finder.selected {
                let style = bold_if(theme.fg(Token::Accent), true);
                Line::styled(format!("  > {text}"), style)
            } else {
                Line::styled(format!("    {text}"), theme.fg(Token::Fg))
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
    let searching = if finder.searching {
        "searching… · "
    } else {
        ""
    };
    frame.render_widget(
        Line::styled(
            format!(
                "{searching}{}/{} · enter open · esc close  ",
                finder.rows.len(),
                finder.total
            ),
            theme.fg(Token::FgMuted),
        )
        .alignment(Alignment::Right),
        hint,
    );
}

/// Draws the `w` workspace switcher: the bordered filter field, the saved
/// workspaces (number, path, project count, `●` on the current one) and
/// the add row.
pub(super) fn draw_switcher(
    frame: &mut Frame,
    area: Rect,
    switcher: &crate::app::workspaces::Switcher,
    model: &Model,
    theme: Theme,
) {
    use crate::app::workspaces::SwitcherRow;

    let rows = model.switcher_rows(switcher);
    let width: u16 = 64;
    let path_room = usize::from(width).saturating_sub(26);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let chosen = i == switcher.selected;
            let style = if chosen {
                bold_if(theme.fg(Token::Accent), true)
            } else {
                theme.fg(Token::Fg)
            };
            let marker = if chosen { "> " } else { "  " };
            match row {
                SwitcherRow::Workspace(path, n) => {
                    let label = crate::store::config::tilde(path, model.home.as_deref());
                    let current = if model.root() == Some(path.as_path()) {
                        " ●"
                    } else {
                        ""
                    };
                    let number = if switcher.query.is_empty() && i < 9 {
                        format!("{} ", i + 1)
                    } else {
                        "  ".to_owned()
                    };
                    Line::from(vec![
                        Span::styled(format!("  {marker}"), style),
                        Span::styled(number, theme.fg(Token::Ok)),
                        Span::styled(
                            format!("{:<path_room$}", keep_end(&label, path_room)),
                            style,
                        ),
                        Span::styled(
                            format!("{n:>3} projects{current}"),
                            theme.fg(Token::FgMuted),
                        ),
                    ])
                }
                SwitcherRow::Add => Line::styled(format!("  {marker}+ add a folder…"), style),
            }
        })
        .collect();
    let list_rows = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let rect = centred(area, width, list_rows + 8);
    frame.render_widget(Clear, rect);
    let block = dialog_block("workspaces", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let [_, field, list, _, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(list_rows),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    draw_field(frame, field, &switcher.query, theme);
    frame.render_widget(Paragraph::new(lines), list);
    frame.render_widget(
        Line::styled(
            "1-9 or enter switch · ctrl-d remove from list · esc close  ",
            theme.fg(Token::FgMuted),
        )
        .alignment(Alignment::Right),
        hint,
    );
}
