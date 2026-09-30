//! Rendering: header, the three panes, the getah bar and overlays
//! (DESIGN §4, §5).
//!
//! Views draw the [`Model`] into a ratatui [`Frame`]; they never read the
//! environment or the terminal. The only thing a view writes back is the
//! number of list rows on screen, for half-page moves. [`panes`] is the
//! one layout function: drawing, mouse hit tests and PTY sizes all use it.

mod cards;
mod dialogs;
mod form;
mod help;
pub(crate) mod keymap;
pub(crate) mod mascot;
mod output;
pub(crate) mod sanitise;
pub(crate) mod theme;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use crate::app::form::FormKind;
use crate::app::model::{Focus, Model, Overlay};
use crate::app::sessions::{self, State};
use crate::store::config::tilde;
use crate::term::session::Size;
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

/// The ascii spinner (DESIGN §3); one frame per animation tick.
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

/// Session counts across every project, for the header and the title.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Tally {
    needs_you: usize,
    failed: usize,
    working: usize,
}

/// Counts needs-you, failed and working sessions in every project.
fn tally(model: &Model) -> Tally {
    model.cards.iter().fold(Tally::default(), |mut t, card| {
        match card.state {
            State::NeedsYou => t.needs_you += 1,
            State::Failed(_) => t.failed += 1,
            State::Working => t.working += 1,
            State::YourTurn | State::Stopped | State::Wrapped => {}
        }
        t
    })
}

/// Returns the terminal title (DESIGN §9): `bungkus-mc · 1 needs you`,
/// else `bungkus-mc · 2 working`, else `bungkus-mc`.
#[must_use]
pub(crate) fn title(model: &Model) -> String {
    let t = tally(model);
    if t.needs_you > 0 {
        format!("bungkus-mc · {} needs you", t.needs_you)
    } else if t.working > 0 {
        format!("bungkus-mc · {} working", t.working)
    } else {
        "bungkus-mc".to_owned()
    }
}

/// Where each pane is on screen; `None` when it is not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Panes {
    /// The projects pane.
    pub projects: Option<Rect>,
    /// The sessions pane.
    pub sessions: Option<Rect>,
    /// The output pane.
    pub output: Option<Rect>,
}

/// Lays out the panes for a screen of `area` (DESIGN §4).
///
/// Three side by side from [`THREE_PANE_WIDTH`] columns, otherwise only
/// the focused pane (the single-pane stack); `zoom` gives the body to the
/// output pane. The header and getah bar take one line each.
#[must_use]
pub(crate) fn panes(area: Rect, focus: Focus, zoom: bool) -> Panes {
    let [_, body, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let only = |which: Focus| Panes {
        projects: (which == Focus::Projects).then_some(body),
        sessions: (which == Focus::Sessions).then_some(body),
        output: (which == Focus::Output).then_some(body),
    };
    if zoom {
        return only(Focus::Output);
    }
    if area.width < THREE_PANE_WIDTH {
        return only(focus);
    }
    let [projects, sessions, output] = Layout::horizontal([
        Constraint::Length(PROJECTS_WIDTH),
        Constraint::Length(SESSIONS_WIDTH),
        Constraint::Fill(1),
    ])
    .areas(body);
    Panes {
        projects: Some(projects),
        sessions: Some(sessions),
        output: Some(output),
    }
}

/// Returns the output pane's inner size, which every session's PTY has
/// (the size the pane has whenever it is shown).
#[must_use]
pub(crate) fn output_size(area: Rect, zoom: bool) -> Size {
    let pane = panes(area, Focus::Output, zoom).output.unwrap_or(area);
    Size {
        cols: pane.width.saturating_sub(2).max(1),
        rows: pane.height.saturating_sub(2).max(1),
    }
}

/// Draws the whole screen.
///
/// Below [`MIN_SIZE`] only a one-line notice is drawn. Before the first
/// run is finished the wizard takes the whole screen. Otherwise: header,
/// the panes of [`panes`], getah bar, and any overlay on top.
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
    let [header, _, getah] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    model.list_rows = usize::from(area.height.saturating_sub(4));
    let layout = panes(area, model.focus, model.zoom);
    draw_header(
        frame,
        header,
        model,
        theme,
        layout.projects.is_some() && layout.output.is_some(),
    );
    if let Some(rect) = layout.projects {
        draw_projects(frame, rect, model, theme);
    }
    if let Some(rect) = layout.sessions {
        cards::draw(frame, rect, model, theme);
    }
    if let Some(rect) = layout.output {
        output::draw(frame, rect, model, theme);
    }
    draw_getah(frame, getah, model, theme);
    match &model.overlay {
        Some(Overlay::Help) => help::draw(frame, area, model.focus.scope(), theme),
        Some(Overlay::Form(f)) => form::draw_settings(frame, area, f, theme, model.host_light),
        Some(Overlay::Picker(p)) => dialogs::draw_picker(frame, area, p, theme),
        Some(Overlay::Confirm(c)) => dialogs::draw_confirm(frame, area, *c, model, theme),
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
            Focus::Output => "output",
        };
        match project {
            Some(name) => format!("{name} › {pane}"),
            None => pane.to_owned(),
        }
    };
    let t = tally(model);
    let spin = cards::spinner(model.frame, theme);
    let counts: Vec<String> = [
        (t.needs_you, format!("! {} needs you", t.needs_you)),
        (t.failed, format!("x {} failed", t.failed)),
        (t.working, format!("{spin} {} working", t.working)),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(_, text)| format!("{text} · "))
    .collect();
    let version = format!("{}v{} ", counts.concat(), env!("CARGO_PKG_VERSION"));
    let room =
        usize::from(area.width).saturating_sub(" bungkus-mc  ".len() + version.chars().count() + 1);
    let line = Line::from(vec![
        Span::styled(
            " bungkus-mc",
            theme.fg(Token::Accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", truncate(&crumbs, room)),
            theme.fg(Token::FgMuted),
        ),
    ]);
    frame.render_widget(line, area);
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

/// Draws the projects pane: `<marker><spinner> <name>` … `<badge>` per
/// row (DESIGN §5.3).
///
/// The marker is `>` on the selected row while the pane is focused and
/// `:` (the ascii form of `▌`) while it is not. The spinner column turns
/// while any session of the project works; the badge is the worst other
/// state, failed > needs you > your turn, with its count.
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
    let spin = cards::spinner(model.frame, theme);
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
            let states: Vec<&State> = sessions::order(&model.cards, &project.path)
                .into_iter()
                .map(|i| &model.cards[i].state)
                .collect();
            let working = states.iter().any(|s| matches!(s, State::Working));
            let badge = [
                State::Failed(String::new()),
                State::NeedsYou,
                State::YourTurn,
            ]
            .iter()
            .find_map(|want| {
                let n = states.iter().filter(|s| s.rank() == want.rank()).count();
                (n > 0).then(|| (cards::glyph(want, spin), n))
            });
            let badge_text = badge.map_or_else(String::new, |((g, _, _), n)| format!("{g} {n} "));
            let badge_token = badge.map_or(Token::Fg, |((_, t, _), _)| t);
            let room = width.saturating_sub(badge_text.len());
            let text = truncate(&project.name, room);
            let pad = room.saturating_sub(text.chars().count());
            let spinner = if working { spin } else { ' ' };
            Line::from(vec![
                Span::styled(marker, theme.fg(Token::Ok)),
                Span::styled(spinner.to_string(), theme.fg(Token::Ok)),
                Span::raw(" "),
                Span::styled(text, name),
                Span::raw(" ".repeat(pad)),
                Span::styled(badge_text, theme.fg(badge_token)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draws the last line: the mode word as a badge, then the focused pane's
/// key hints or the current message (DESIGN §5.1).
fn draw_getah(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let interact = model.focus == Focus::Output;
    let (mode, token) = if interact {
        (" INTERACT ", Token::Warn)
    } else if model.filtering {
        (" FILTER ", Token::Info)
    } else {
        (" NORMAL ", Token::FgMuted)
    };
    let text = if let Some(message) = &model.message {
        message.clone()
    } else if interact {
        let agent = model
            .selected_card()
            .map_or("the agent", |i| model.cards[i].kind.command());
        format!(
            "keys go to {agent} · {} back to mc",
            model.exit_chord.label()
        )
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

/// Returns a pane: heavy `ok` border when focused, light `border`
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
        model.update(crate::app::AppEvent::Input(Event::Key(KeyEvent::new(
            code,
            KeyModifiers::NONE,
        ))));
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
    fn sessions_and_interact_match_goldens() {
        use crate::app::model::tests::with_session;
        use crate::term::PtyEvent;

        let mut model = sample(PROJECTS);
        let (first, _w1) = with_session(&mut model, "checkout redesign");
        let (second, _w2) = with_session(&mut model, "flaky payment test");
        let (third, _w3) = with_session(&mut model, "bump deps");
        let output =
            |id, bytes: &[u8]| crate::app::AppEvent::Pty(PtyEvent::Output(id, bytes.to_vec()));
        model.update(output(
            second,
            b"\x1b[1m> fix the flaky date test\x1b[0m\r\n  run it twice",
        ));
        model.update(crate::app::AppEvent::Pty(PtyEvent::Exited(third, Some(0))));
        model.update(output(first, b"boom"));
        model.update(crate::app::AppEvent::Pty(PtyEvent::Exited(first, Some(1))));
        model.focus = Focus::Sessions;
        model.card = 1;
        assert_golden("sessions-120x40.txt", &render(&mut model, 120, 40));
        model.interact();
        assert_golden("interact-120x40.txt", &render(&mut model, 120, 40));
        assert_golden("interact-80x24.txt", &render(&mut model, 80, 24));
        key(&mut model, KeyCode::Char('\x1c'));
        model.focus = Focus::Sessions;
        key(&mut model, KeyCode::Char('q'));
        assert_golden("quit-confirm-80x24.txt", &render(&mut model, 80, 24));
    }

    #[test]
    fn hooked_cards_match_golden() {
        use crate::app::model::tests::{hook_line, with_session};

        let mut model = sample(PROJECTS);
        let (a, _w1) = with_session(&mut model, "checkout redesign");
        let (b, _w2) = with_session(&mut model, "seo audit");
        for card in &mut model.cards {
            card.expect_hooks();
        }
        let recorded = include_str!("agent/testdata/claude/session.jsonl");
        for line in recorded.lines().take(6) {
            model.update(hook_line(a, line));
        }
        model.update(hook_line(
            a,
            r#"{"hook_event_name":"Notification","notification_type":"permission_prompt"}"#,
        ));
        model.update(hook_line(b, r#"{"hook_event_name":"UserPromptSubmit"}"#));
        model.update(hook_line(b, r#"{"hook_event_name":"Stop","last_assistant_message":"Three fixes, see above","background_tasks":[]}"#));
        model.focus = Focus::Sessions;
        model.card = 0;
        assert_golden("hooks-120x40.txt", &render(&mut model, 120, 40));
    }

    #[test]
    fn picker_matches_golden() {
        let mut model = sample(PROJECTS);
        key(&mut model, KeyCode::Char('n'));
        for ch in "fix the flaky date test".chars() {
            key(&mut model, KeyCode::Char(ch));
        }
        assert_golden("picker-80x24.txt", &render(&mut model, 80, 24));
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
