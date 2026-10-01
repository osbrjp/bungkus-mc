//! Views of the settings [`Form`]: the full-screen first-run wizard and
//! the settings dialog (DESIGN §5.8).

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::agent::Kind;
use crate::app::form::{Field, Form};
use crate::ui::mascot::{self, Mascot};
use crate::ui::theme::{Theme, ThemeChoice, ThemeName, Token};
use crate::ui::{bold_if, centred, dialog};

/// Width of the wizard's content column.
const WIZARD_WIDTH: u16 = 70;
/// Height of the wizard's content column.
const WIZARD_HEIGHT: u16 = 22;
/// Column where field values start, after the label.
const LABEL_WIDTH: u16 = 12;

/// Draws the wizard step for `form.field`, centred on the screen.
///
/// # Arguments
///
/// * `frame`      - Frame to draw into.
/// * `area`       - The whole screen.
/// * `form`       - The wizard's state.
/// * `theme`      - Colours, already previewing the chosen theme.
/// * `host_light` - The terminal's own background, for `auto`.
pub(super) fn draw_wizard(
    frame: &mut Frame,
    area: Rect,
    form: &Form,
    theme: Theme,
    host_light: Option<bool>,
) {
    let box_area = centred(area, WIZARD_WIDTH.min(area.width), WIZARD_HEIGHT);
    let [top, _, body, error, hint] = Layout::vertical([
        Constraint::Length(mascot::HEIGHT),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(box_area);
    let [sprite, _, title] = Layout::horizontal([
        Constraint::Length(mascot::WIDTH),
        Constraint::Length(4),
        Constraint::Fill(1),
    ])
    .areas(top);
    frame.render_widget(Mascot::idle(theme), sprite);
    frame.render_widget(Paragraph::new(title_lines(form.field, theme)), title);

    if form.field == Field::Workspace {
        text_row(frame, body, form, theme, true);
        let list = Rect {
            y: body.y + 1,
            height: body.height.saturating_sub(1),
            ..body
        };
        draw_browser(frame, list, form, theme);
    } else {
        frame.render_widget(Paragraph::new(step_rows(form, theme, host_light)), body);
    }
    draw_error(frame, error, form, theme);
    let hints = match form.field {
        Field::Workspace if form.typing => "type a path · enter choose · esc stop typing",
        Field::Workspace => "j/k pick · l open · h up · / type a path · enter choose · esc skip",
        Field::Agent | Field::Theme => "← → choose · enter next · esc back",
        Field::Done => "enter start · esc back",
    };
    frame.render_widget(
        Line::styled(hints, theme.fg(Token::FgMuted)).alignment(Alignment::Right),
        hint,
    );
}

/// Returns the wizard's title block: name, version, tagline and step.
fn title_lines(field: Field, theme: Theme) -> Vec<Line<'static>> {
    let (step, name) = match field {
        Field::Workspace => (1, "workspace"),
        Field::Agent => (2, "default agent"),
        Field::Theme => (3, "theme"),
        Field::Done => (4, "all set"),
    };
    vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("bungkus-mc", bold_if(theme.fg(Token::Accent), true)),
            Span::styled(
                format!("  v{}", env!("CARGO_PKG_VERSION")),
                theme.fg(Token::FgMuted),
            ),
        ]),
        Line::styled("mission control for AI agents", theme.fg(Token::Fg)),
        Line::from(""),
        Line::styled(format!("step {step} of 4 · {name}"), theme.fg(Token::Info)),
    ]
}

/// Returns the body rows of the agent, theme and summary steps.
fn step_rows(form: &Form, theme: Theme, host_light: Option<bool>) -> Vec<Line<'static>> {
    let muted = theme.fg(Token::FgMuted);
    let value = |text: &str| vec![Span::styled(text.to_owned(), theme.fg(Token::Fg))];
    match form.field {
        Field::Workspace => Vec::new(),
        Field::Agent => vec![
            label_line("agent", agent_spans(form, theme, true)),
            Line::from(""),
            indent(&found_text(form), muted),
            indent("n starts this agent; the n picker can still switch", muted),
        ],
        Field::Theme => vec![
            label_line("theme", theme_spans(form, theme, true)),
            Line::from(""),
            indent(&theme_note(form.theme, host_light), muted),
            preview_line(theme),
        ],
        Field::Done => vec![
            label_line("workspace", value(&form.workspace)),
            label_line("agent", value(form.agent.command())),
            label_line("theme", value(form.theme.label())),
            Line::from(""),
            indent("change these any time with , (settings)", muted),
        ],
    }
}

/// Draws the settings dialog over the panes (DESIGN §5.5 dialog rules).
///
/// # Arguments
///
/// * `frame`      - Frame to draw into.
/// * `area`       - The whole screen.
/// * `form`       - The settings screen's state.
/// * `theme`      - Colours, already previewing the chosen theme.
/// * `host_light` - The terminal's own background, for `auto`.
pub(super) fn draw_settings(
    frame: &mut Frame,
    area: Rect,
    form: &Form,
    theme: Theme,
    host_light: Option<bool>,
) {
    let browsing = form.field == Field::Workspace;
    let list_rows = if browsing { 9 } else { 0 };
    let rect = centred(area, 66, 12 + list_rows);
    frame.render_widget(Clear, rect);
    let block = dialog("settings", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let [_, workspace, list, agent, theme_row, note, error, _, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(list_rows),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let marker = |field| if form.field == field { ">" } else { " " };
    let focused_row = |row: Rect, field| frame_marker(row, marker(field), theme);
    frame.render_widget(focused_row(workspace, Field::Workspace), workspace);
    text_row(
        frame,
        shift(workspace),
        form,
        theme,
        form.field == Field::Workspace,
    );
    if browsing {
        draw_browser(frame, shift(list), form, theme);
    }
    frame.render_widget(focused_row(agent, Field::Agent), agent);
    frame.render_widget(
        label_line(
            "agent",
            agent_spans(form, theme, form.field == Field::Agent),
        ),
        shift(agent),
    );
    frame.render_widget(focused_row(theme_row, Field::Theme), theme_row);
    frame.render_widget(
        label_line(
            "theme",
            theme_spans(form, theme, form.field == Field::Theme),
        ),
        shift(theme_row),
    );
    let note_text = match form.field {
        Field::Agent => found_text(form),
        Field::Theme => theme_note(form.theme, host_light),
        Field::Workspace | Field::Done => {
            "projects = folders with CLAUDE.md, AGENTS.md or .git".into()
        }
    };
    frame.render_widget(indent(&note_text, theme.fg(Token::FgMuted)), shift(note));
    draw_error(frame, shift(error), form, theme);
    frame.render_widget(
        Line::styled(
            if browsing && form.typing {
                "type a path · enter choose · esc stop typing · tab field "
            } else if browsing {
                "j/k pick · l open · h up · / type a path · enter choose · tab field "
            } else {
                "j/k ↑↓ field · h/l ← → change · enter save · esc cancel "
            },
            theme.fg(Token::FgMuted),
        )
        .alignment(Alignment::Right),
        hint,
    );
}

/// Draws the folder browser under the workspace field: a line saying how
/// many projects the folder holds, then its subfolders with the highlight
/// marked and projects labelled, scrolled to keep the highlight in view.
fn draw_browser(frame: &mut Frame, area: Rect, form: &Form, theme: Theme) {
    let b = &form.browser;
    let here = match b.projects() {
        0 => "no projects directly in this folder".to_owned(),
        1 => "1 project in this folder".to_owned(),
        n => format!("{n} projects in this folder"),
    };
    let token = if b.projects() > 0 {
        Token::Ok
    } else {
        Token::FgMuted
    };
    let mut lines = vec![indent(&here, theme.fg(token))];
    let rows = usize::from(area.height.saturating_sub(1));
    let start = (b.selected + 1).saturating_sub(rows);
    let width = usize::from(area.width).saturating_sub(usize::from(LABEL_WIDTH) + 12);
    let here_row = (".".to_owned(), "  this folder", false);
    let rows_iter = std::iter::once(here_row).chain(b.entries.iter().map(|e| {
        let note = if e.project { "  project" } else { "" };
        (crate::ui::sanitise::truncate(&e.name, width), note, true)
    }));
    for (i, (name, note, folder)) in rows_iter.enumerate().skip(start).take(rows) {
        let chosen = i == b.selected;
        let marker = if chosen { "> " } else { "  " };
        let style = match (chosen, folder) {
            (true, _) => bold_if(theme.fg(Token::Accent), true),
            (false, true) => theme.fg(Token::Fg),
            (false, false) => theme.fg(Token::FgMuted),
        };
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(usize::from(LABEL_WIDTH))),
            Span::styled(marker, theme.fg(Token::Ok)),
            Span::styled(format!("{name}/"), style),
            Span::styled(note, theme.fg(Token::FgMuted)),
        ]));
    }
    if b.entries.is_empty() && rows > 1 {
        lines.push(indent(
            "(no folders here — ← to go up)",
            theme.fg(Token::FgMuted),
        ));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Returns the one-cell focus marker line for a settings row.
fn frame_marker(_row: Rect, marker: &'static str, theme: Theme) -> Line<'static> {
    Line::styled(format!(" {marker}"), theme.fg(Token::Ok))
}

/// Returns `row` moved right past the focus marker column.
fn shift(row: Rect) -> Rect {
    Rect {
        x: row.x + 3,
        width: row.width.saturating_sub(4),
        ..row
    }
}

/// Draws the workspace text input on `row` and, when `focused`, puts the
/// terminal cursor at its end.
fn text_row(frame: &mut Frame, row: Rect, form: &Form, theme: Theme, focused: bool) {
    let field_width = row.width.saturating_sub(LABEL_WIDTH + 2);
    let chars: Vec<char> = form.workspace.chars().collect();
    let visible = usize::from(field_width.saturating_sub(1));
    let shown: String = chars[chars.len().saturating_sub(visible)..]
        .iter()
        .collect();
    let pad = usize::from(field_width).saturating_sub(shown.chars().count());
    let bar = theme.fg(if focused { Token::Ok } else { Token::Border });
    let line = Line::from(vec![
        Span::styled(
            format!("{:<width$}", "workspace", width = usize::from(LABEL_WIDTH)),
            label_style(theme),
        ),
        Span::styled("┃", bar),
        Span::styled(shown.clone(), theme.fg(Token::Fg)),
        Span::raw(" ".repeat(pad)),
        Span::styled("┃", bar),
    ]);
    frame.render_widget(line, row);
    if focused && form.typing {
        let x = row.x + LABEL_WIDTH + 1 + u16::try_from(shown.chars().count()).unwrap_or(0);
        frame.set_cursor_position(Position::new(x, row.y));
    }
}

/// Returns the agent choice: the chosen one marked `>`, missing ones noted.
fn agent_spans(form: &Form, theme: Theme, focused: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for kind in Kind::ALL {
        let chosen = form.agent == kind;
        let missing = form.found[kind as usize].is_none();
        spans.push(choice(kind.command(), chosen, focused, theme));
        if missing {
            spans.push(Span::styled(" (not on PATH)", theme.fg(Token::FgMuted)));
        }
        spans.push(Span::raw("  "));
    }
    spans
}

/// Returns the theme choice row.
fn theme_spans(form: &Form, theme: Theme, focused: bool) -> Vec<Span<'static>> {
    ThemeChoice::ALL
        .iter()
        .flat_map(|t| {
            [
                choice(t.label(), form.theme == *t, focused, theme),
                Span::raw("  "),
            ]
        })
        .collect()
}

/// Returns one option of a choice row: `> name` when chosen.
fn choice(label: &str, chosen: bool, focused: bool, theme: Theme) -> Span<'static> {
    if chosen {
        let style = if focused {
            theme.fg(Token::Accent)
        } else {
            theme.fg(Token::Fg)
        };
        Span::styled(format!("> {label}"), bold_if(style, focused))
    } else {
        Span::styled(format!("  {label}"), theme.fg(Token::FgMuted))
    }
}

/// Returns where each agent was found, as one line.
fn found_text(form: &Form) -> String {
    Kind::ALL
        .iter()
        .map(|k| match &form.found[*k as usize] {
            Some(path) => format!("+ {path}"),
            None => format!("- {} not on PATH", k.command()),
        })
        .collect::<Vec<_>>()
        .join("   ")
}

/// Returns the explanation under the theme choice.
fn theme_note(choice: ThemeChoice, host_light: Option<bool>) -> String {
    match choice {
        ThemeChoice::Auto => {
            let now = match choice.resolve(host_light) {
                ThemeName::Dark => "dark",
                ThemeName::Light => "light",
            };
            format!("follows your terminal's background (now {now})")
        }
        ThemeChoice::Dark => "Daun Teduh: shaded banana leaf".into(),
        ThemeChoice::Light => "Santan: coconut cream".into(),
    }
}

/// Returns a sample of the state colours in the previewed theme.
fn preview_line(theme: Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(usize::from(LABEL_WIDTH)))];
    for (word, token) in [
        ("/ working", Token::Ok),
        ("! needs you", Token::Warn),
        ("x failed", Token::Err),
        ("~ your turn", Token::Info),
    ] {
        spans.push(Span::styled(word, theme.fg(token)));
        spans.push(Span::raw("  "));
    }
    Line::from(spans)
}

/// Returns `label` padded to the value column, then `value`.
fn label_line(label: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::raw(format!(
        "{label:<width$}",
        width = usize::from(LABEL_WIDTH)
    ))];
    spans.extend(value);
    Line::from(spans)
}

/// Returns `text` starting at the value column.
fn indent(text: &str, style: ratatui::style::Style) -> Line<'static> {
    Line::styled(
        format!("{:width$}{text}", "", width = usize::from(LABEL_WIDTH)),
        style,
    )
}

/// Returns the style of field labels.
fn label_style(theme: Theme) -> ratatui::style::Style {
    theme.fg(Token::Fg)
}

/// Draws the form's error, if any, in `err`.
fn draw_error(frame: &mut Frame, row: Rect, form: &Form, theme: Theme) {
    if let Some(error) = &form.error {
        frame.render_widget(indent(error, theme.fg(Token::Err)), row);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;

    use crate::app::model::tests::sample;
    use crate::ui::tests::{assert_golden, key, render};

    #[test]
    fn wizard_steps_match_goldens() {
        let mut model = sample(&["kedai-web"]);
        model.settings = None;
        model.start_wizard("~/Works/OSBR");
        let fixture = std::env::temp_dir().join(format!("mc-wizard-{}", std::process::id()));
        for dir in ["Works/OSBR", "kedai-web/.git", "notes", "roti-docs"] {
            std::fs::create_dir_all(fixture.join(dir)).unwrap();
        }
        if let Some(crate::app::model::Overlay::Form(form)) = &mut model.overlay {
            "/tmp".clone_into(&mut form.workspace);
            form.browser = crate::app::browser::Browser::open(&fixture);
            form.browser.step(1);
        }
        for (step, name) in ["workspace", "agent", "theme", "done"].iter().enumerate() {
            assert_golden(
                &format!("wizard-{}-{name}-80x24.txt", step + 1),
                &render(&mut model, 80, 24),
            );
            if step == 0 {
                assert_golden(
                    "wizard-1-workspace-120x40.txt",
                    &render(&mut model, 120, 40),
                );
                if let Some(crate::app::model::Overlay::Form(form)) = &mut model.overlay {
                    form.browser.step(-1);
                }
            }
            key(&mut model, KeyCode::Enter);
        }
        assert!(model.overlay.is_none(), "enter on the summary submits");
        std::fs::remove_dir_all(&fixture).unwrap();
    }

    #[test]
    fn the_browser_walks_folders_and_fills_the_field() {
        let root = std::env::temp_dir().join(format!("mc-walk-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Works/OSBR/kedai-web/.git")).unwrap();
        let mut model = sample(&[]);
        model.settings = None;
        model.start_wizard(&root.to_string_lossy());
        let form = |m: &crate::app::model::Model| match &m.overlay {
            Some(crate::app::model::Overlay::Form(f)) => f.clone(),
            _ => panic!("wizard"),
        };
        assert_eq!(form(&model).browser.dir, root);
        key(&mut model, KeyCode::Right);
        assert_eq!(form(&model).browser.dir, root, "→ on ./ stays");
        key(&mut model, KeyCode::Down);
        key(&mut model, KeyCode::Right);
        assert_eq!(form(&model).browser.dir, root.join("Works"));
        key(&mut model, KeyCode::Down);
        key(&mut model, KeyCode::Right);
        let f = form(&model);
        assert_eq!(f.workspace, root.join("Works/OSBR").to_string_lossy());
        assert_eq!(f.browser.projects(), 1, "kedai-web is a project");
        key(&mut model, KeyCode::Left);
        assert_eq!(form(&model).browser.dir, root.join("Works"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn settings_dialog_matches_goldens() {
        for (width, height) in [(120, 40), (80, 24)] {
            let mut model = sample(&["kedai-web", "teh-cli"]);
            key(&mut model, KeyCode::Char(','));
            assert_golden(
                &format!("settings-{width}x{height}.txt"),
                &render(&mut model, width, height),
            );
        }
    }

    #[test]
    fn wizard_shows_why_a_workspace_is_refused() {
        let mut model = sample(&[]);
        model.settings = None;
        model.start_wizard("nope");
        key(&mut model, KeyCode::Enter);
        let screen = render(&mut model, 80, 24);
        assert!(screen.contains("nope is not a full path"), "{screen}");
    }
}
