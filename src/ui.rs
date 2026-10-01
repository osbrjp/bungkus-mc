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
pub(crate) mod icons;
pub(crate) mod keymap;
pub(crate) mod mascot;
mod output;
pub(crate) mod sanitise;
pub(crate) mod theme;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::form::FormKind;
use crate::app::model::{Focus, Model, Overlay};
use crate::app::sessions::{self, State};
use crate::store::config::tilde;
use crate::term::session::Size;
use crate::ui::mascot::{Mascot, Mood};
use crate::ui::sanitise::truncate;
use crate::ui::theme::{Theme, Token};

/// Smallest usable screen, per DESIGN §4.
const MIN_SIZE: (u16, u16) = (80, 24);

/// Width from which the three panes sit side by side (DESIGN §4).
const THREE_PANE_WIDTH: u16 = 100;

/// Smallest and largest outer width of the projects pane (DESIGN §4).
pub(crate) const PROJECTS_RANGE: (u16, u16) = (16, 40);
/// Smallest and largest outer width of the sessions pane (DESIGN §4).
pub(crate) const SESSIONS_RANGE: (u16, u16) = (28, 72);
/// Columns the output pane always keeps in the three-pane layout.
const OUTPUT_MIN: u16 = 40;

/// Outer widths of the projects and sessions panes, dragged by their
/// right borders and saved as `panes` in `config.json`; the output pane
/// takes the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub(crate) struct Widths {
    /// The projects pane.
    pub projects: u16,
    /// The sessions pane.
    pub sessions: u16,
}

impl Default for Widths {
    fn default() -> Self {
        Self {
            projects: 22,
            sessions: 38,
        }
    }
}

impl Widths {
    /// Returns the widths clamped to [`PROJECTS_RANGE`] and
    /// [`SESSIONS_RANGE`], then narrowed (sessions first) so the output
    /// pane keeps [`OUTPUT_MIN`] of a body `width` columns wide.
    #[must_use]
    pub(crate) fn fit(self, width: u16) -> Self {
        let room = width.saturating_sub(OUTPUT_MIN);
        let projects = self.projects.clamp(PROJECTS_RANGE.0, PROJECTS_RANGE.1);
        let sessions = self
            .sessions
            .clamp(SESSIONS_RANGE.0, SESSIONS_RANGE.1)
            .min(room.saturating_sub(projects))
            .max(SESSIONS_RANGE.0);
        let projects = projects
            .min(room.saturating_sub(sessions))
            .max(PROJECTS_RANGE.0);
        Self { projects, sessions }
    }
}

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
pub(crate) fn panes(area: Rect, focus: Focus, zoom: bool, widths: Widths) -> Panes {
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
    let widths = widths.fit(body.width);
    let [projects, sessions, output] = Layout::horizontal([
        Constraint::Length(widths.projects),
        Constraint::Length(widths.sessions),
        Constraint::Fill(1),
    ])
    .areas(body);
    Panes {
        projects: Some(projects),
        sessions: Some(sessions),
        output: Some(output),
    }
}

/// Returns the quick-session popup: centred, a fixed share of the screen
/// (not draggable or resizable; issue #46).
#[must_use]
pub(crate) fn popup_rect(area: Rect) -> Rect {
    let width = (area.width.saturating_sub(10))
        .clamp(60, 120)
        .min(area.width);
    let height = (area.height.saturating_sub(6))
        .clamp(16, 40)
        .min(area.height);
    centred(area, width, height)
}

/// Returns the PTY size of a quick session: the popup's inner size.
#[must_use]
pub(crate) fn popup_size(area: Rect) -> Size {
    let popup = popup_rect(area);
    Size {
        cols: popup.width.saturating_sub(2).max(1),
        rows: popup.height.saturating_sub(2).max(1),
    }
}

/// Returns the output pane's inner size, which every session's PTY has
/// (the size the pane has whenever it is shown).
#[must_use]
pub(crate) fn output_size(area: Rect, zoom: bool, widths: Widths) -> Size {
    let pane = panes(area, Focus::Output, zoom, widths)
        .output
        .unwrap_or(area);
    Size {
        cols: pane.width.saturating_sub(2).max(1),
        rows: pane.height.saturating_sub(2 + strip_height(area)).max(1),
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
    model.list_rows = usize::from(area.height.saturating_sub(5));
    let layout = panes(area, model.focus, model.zoom, model.widths);
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
    if model.popup.is_some() {
        output::draw_popup(frame, popup_rect(area), model, theme);
    }
    match &model.overlay {
        Some(Overlay::Help) => help::draw(frame, area, model.focus.scope(), theme),
        Some(Overlay::Form(f)) => form::draw_settings(frame, area, f, theme, model.host_light),
        Some(Overlay::Picker(p)) => dialogs::draw_picker(frame, area, p, theme),
        Some(Overlay::Stop(d)) => dialogs::draw_stop(frame, area, d, model, theme),
        Some(Overlay::Forget(id)) => dialogs::draw_forget(frame, area, *id, model, theme),
        Some(Overlay::TakeOver(ext, _)) => dialogs::draw_take_over(frame, area, ext, theme),
        Some(Overlay::ResumeAgent(kind)) => dialogs::draw_resume_agent(frame, area, *kind, theme),
        Some(Overlay::Move(dialog)) => dialogs::draw_move(frame, area, dialog, model, theme),
        Some(Overlay::StopOutside(ext)) => dialogs::draw_stop_outside(frame, area, ext, theme),
        Some(Overlay::NewProject(dialog)) => dialogs::draw_new_project(frame, area, dialog, theme),
        Some(Overlay::TrashProject(projects)) => {
            dialogs::draw_trash_project(frame, area, projects, model, theme);
        }
        None => {}
    }
}

/// Screens at least this big give the output pane a three-row strip at
/// its top with the mascot at the right (DESIGN §5.7); smaller ones keep
/// the corner mascot that steps aside for agent output.
const STRIP_SIZE: (u16, u16) = (120, 36);

/// Columns between the bubble's tail and the mascot.
const STRIP_GAP: u16 = 1;

/// Returns the rows the output pane gives the mascot strip: 3 on big
/// screens, 0 otherwise.
#[must_use]
pub(crate) fn strip_height(screen: Rect) -> u16 {
    if screen.width >= STRIP_SIZE.0 && screen.height >= STRIP_SIZE.1 {
        mascot::MINI_HEIGHT
    } else {
        0
    }
}

/// Returns where the strip mascot sits in output pane `output` (the click
/// target), if the screen has a strip.
#[must_use]
pub(crate) fn strip_mascot(output: Rect, screen: Rect) -> Option<Rect> {
    (strip_height(screen) > 0).then(|| {
        Rect::new(
            output.right().saturating_sub(mascot::MINI_WIDTH + 2),
            output.y + 1,
            mascot::MINI_WIDTH,
            mascot::MINI_HEIGHT,
        )
    })
}

/// Draws the strip mascot at `spot` in `mood` and, after a click, its
/// speech bubble to the left (within `strip`).
///
/// A click plays [`mascot::poke_pose`] and shows a quote for
/// [`mascot::POKE`].
pub(super) fn draw_strip(
    frame: &mut Frame,
    strip: Rect,
    spot: Rect,
    mood: Mood,
    model: &Model,
    theme: Theme,
) {
    let clicked = model
        .poke
        .map(|(at, quote)| (model.now.saturating_duration_since(at), quote));
    let pose = clicked
        .and_then(|(elapsed, _)| mascot::poke_pose(elapsed, theme.animated()))
        .unwrap_or_else(|| mood.pose(model.frame, theme.animated()));
    frame.render_widget(
        Mascot {
            theme,
            pose,
            mini: true,
        },
        spot,
    );
    let Some((_, quote)) = clicked else {
        return;
    };
    let text = mascot::QUOTES[quote % mascot::QUOTES.len()];
    let width = u16::try_from(text.chars().count() + 4).unwrap_or(u16::MAX);
    let room = spot.x.saturating_sub(strip.x + STRIP_GAP + 1);
    if width > room {
        return;
    }
    let bubble = Rect::new(spot.x - width - STRIP_GAP - 1, strip.y, width, strip.height);
    frame.render_widget(Clear, bubble);
    let block = bordered(Weight::Light, theme).border_style(theme.fg(Token::Ok));
    let inner = block.inner(bubble);
    frame.render_widget(block, bubble);
    frame.render_widget(Line::styled(format!(" {text}"), theme.fg(Token::Fg)), inner);
    let tail = if theme.utf8 { "◂" } else { "<" };
    frame.render_widget(
        Line::styled(tail, theme.fg(Token::Ok)),
        Rect::new(bubble.right(), strip.y + 1, 1, 1),
    );
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

/// Draws the projects pane: the search row, then
/// `<marker><spinner> <number> <name>` … `<badge>` per row (DESIGN §5.3).
///
/// A linked git worktree sits below its repository with `├ `/`└ ` (ascii
/// `|-`/`` `- ``) and its name without the repository's prefix.
///
/// The number is the project's position in the (searched) list, the one
/// digits jump to. The search row reads `/ search` until `/` is pressed,
/// then shows the typed text with the cursor after it.
///
/// The marker is `>` on the selected row while the pane is focused and
/// `:` (the ascii form of `▌`) while it is not. The spinner column turns
/// while any session of the project works; the badge is the worst other
/// state, failed > needs you > your turn, with its count.
fn draw_projects(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let focused = model.focus == Focus::Projects;
    let block = pane("[1] projects", focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [search, inner] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    draw_search(frame, search, model, theme);
    let visible = model.visible();
    let quick_first = usize::from(
        visible
            .first()
            .is_some_and(|p| model.root() == Some(p.path.as_path())),
    );
    let digits = visible.len().max(1).to_string().len();
    let rows = usize::from(inner.height);
    let width = usize::from(inner.width).saturating_sub(4 + digits);
    let offset = (model.selected + 1).saturating_sub(rows);
    let spin = cards::spinner(model.frame, theme);
    let lines: Vec<Line> = visible
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows)
        .map(|(i, project)| {
            let selected = i == model.selected;
            let marker = match (selected, focused) {
                (true, true) => theme.icons.icon(icons::Icon::Marker),
                (true, false) if theme.icons == icons::IconSet::Ascii => ':',
                (true, false) => '▌',
                (false, _) => ' ',
            };
            let name = match (selected, model.in_visual(i)) {
                (_, true) => theme.fg(Token::Accent).add_modifier(Modifier::REVERSED),
                (true, false) => theme.fg(Token::Accent),
                (false, false) => theme.fg(Token::Fg),
            };
            let states: Vec<State> = sessions::order(&model.cards, &project.path)
                .into_iter()
                .map(|i| model.cards[i].state.clone())
                .chain(model.external_in(&project.path).iter().map(|e| e.state()))
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
                (n > 0).then(|| (cards::glyph(want, spin, theme.icons), n))
            });
            let badge_text = badge.map_or_else(String::new, |((g, _, _), n)| format!(" {g} {n} "));
            let badge_token = badge.map_or(Token::Fg, |((_, t, _), _)| t);
            let branch = project.worktree_of.as_ref().map(|of| {
                let last = visible
                    .get(i + 1)
                    .is_none_or(|next| next.worktree_of.as_ref() != Some(of));
                match (theme.utf8, last) {
                    (true, true) => "└ ",
                    (true, false) => "├ ",
                    (false, true) => "`-",
                    (false, false) => "|-",
                }
            });
            let label = worktree_label(project);
            let room = width.saturating_sub(badge_text.len() + branch.map_or(0, |_| 2));
            let text = truncate(label, room);
            let pad = room.saturating_sub(text.chars().count());
            let spinner = if working { spin } else { ' ' };
            Line::from(vec![
                Span::styled(marker.to_string(), theme.fg(Token::Ok)),
                Span::styled(spinner.to_string(), theme.fg(Token::Ok)),
                Span::raw(" "),
                Span::styled(
                    if model.root() == Some(project.path.as_path()) {
                        format!("{:>digits$} ", 0)
                    } else {
                        format!("{:>digits$} ", i + 1 - quick_first)
                    },
                    theme.fg(Token::Ok),
                ),
                Span::styled(branch.unwrap_or_default(), theme.fg(Token::FgMuted)),
                Span::styled(text, name),
                Span::raw(" ".repeat(pad)),
                Span::styled(badge_text, theme.fg(badge_token)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Returns the name a project row shows: a worktree drops its
/// repository's name when it starts with it (`nrha-timii-i746` below
/// `nrha-timii` reads `i746`).
fn worktree_label(project: &crate::workspace::Project) -> &str {
    project
        .worktree_of
        .as_deref()
        .and_then(|of| project.name.strip_prefix(of))
        .map(|rest| rest.trim_start_matches(['-', '_', '.']))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(&project.name)
}

/// Draws the projects search row (FILTER mode puts the cursor after the
/// text).
fn draw_search(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let room = usize::from(area.width).saturating_sub(3);
    let line = if model.filtering || !model.filter.is_empty() {
        let token = if model.filtering {
            Token::Accent
        } else {
            Token::Fg
        };
        let text = truncate_start(&model.filter, room);
        let cursor = u16::try_from(text.chars().count()).unwrap_or(0);
        if model.filtering {
            frame.set_cursor_position((area.x + 3 + cursor, area.y));
        }
        Line::from(vec![
            Span::styled(" / ", theme.fg(token)),
            Span::styled(text, theme.fg(Token::Fg)),
        ])
    } else {
        Line::styled(" / search", theme.fg(Token::FgMuted))
    };
    frame.render_widget(line, area);
}

/// Returns the last `max` characters of `s`, so the end of a long search
/// stays visible.
fn truncate_start(s: &str, max: usize) -> String {
    let n = s.chars().count();
    s.chars().skip(n.saturating_sub(max)).collect()
}

/// Draws the last line: the mode word as a badge, then the focused pane's
/// key hints or the current message (DESIGN §5.1).
fn draw_getah(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let interact = model.focus == Focus::Output || model.popup.is_some();
    let (mode, token) = if model.popup.is_some() {
        (" QUICK ", Token::Warn)
    } else if interact {
        (" INTERACT ", Token::Warn)
    } else if model.filtering {
        (" FILTER ", Token::Info)
    } else if model.visual.is_some() {
        (" VISUAL ", Token::Info)
    } else {
        (" NORMAL ", Token::FgMuted)
    };
    let text = if let Some(message) = &model.message {
        message.clone()
    } else if let Some(id) = model.popup {
        let agent = model
            .cards
            .iter()
            .find(|c| c.id == id)
            .map_or("the agent", |c| c.kind.command());
        if model.popup_menu {
            "h hide · m move to a project or a new one · any other key: back".to_owned()
        } else {
            format!(
                "keys go to {agent} · ctrl-m move · {} menu",
                model.exit_chord.label()
            )
        }
    } else if interact {
        let agent = model
            .selected_card()
            .map_or("the agent", |i| model.cards[i].kind.command());
        format!(
            "keys go to {agent} · {} back to mc",
            model.exit_chord.label()
        )
    } else if model.visual.is_some() {
        "j/k extend · d move to Trash · esc cancel".to_owned()
    } else if model.filtering {
        "type to search · ↑↓ pick · enter open · esc clear".to_owned()
    } else if let Some(ch) = model.pending {
        format!("{ch}…")
    } else {
        keymap::hints(model.focus.scope()).join(" · ")
    };
    let line = Line::from(vec![
        Span::styled(mode, theme.badge(token)),
        Span::styled(format!(" {text}"), theme.fg(Token::FgMuted)),
    ]);
    let left = line.width();
    frame.render_widget(line, area);
    let limits = Line::from(limit_spans(model, theme, area.width >= THREE_PANE_WIDTH));
    if left + limits.width() + 2 <= usize::from(area.width) {
        frame.render_widget(limits.alignment(Alignment::Right), area);
    }
}

/// Returns the plan limits for the getah bar's right end (DESIGN §6.1):
/// `C 5h 42% · 7d 18% · X 5h 10% · 7d 3%`, with 5-cell bars when only one
/// vendor reported, only the 5-hour figure when narrow; `warn` from 80 %,
/// `err` from 95 %, `fg-muted` once a window's reset time has passed. A
/// report older than [`LIMITS_FRESH_MINUTES`] gets its age (`(12m ago)`):
/// the figures come from Claude's status line in mc's own sessions, so use
/// elsewhere only shows after their next turn.
fn limit_spans(model: &Model, theme: Theme, wide: bool) -> Vec<Span<'static>> {
    let vendors: Vec<(
        char,
        &Vec<crate::agent::usage::Window>,
        Option<std::time::Instant>,
    )> = crate::agent::Kind::ALL
        .iter()
        .map(|k| {
            (
                k.badge(),
                &model.limits[*k as usize],
                model.limits_at[*k as usize],
            )
        })
        .filter(|(_, w, _)| !w.is_empty())
        .collect();
    let bars = vendors.len() == 1 && wide;
    let mut spans = Vec::new();
    for (v, (badge, windows, at)) in vendors.iter().enumerate() {
        if v > 0 {
            spans.push(Span::styled(" · ", theme.fg(Token::FgMuted)));
        }
        spans.push(Span::styled(format!("{badge} "), theme.fg(Token::Accent)));
        let shown = windows.iter().filter(|w| wide || w.label == "5h");
        for (i, w) in shown.enumerate() {
            let stale = w.resets_at.is_some_and(|at| at < model.unix_now);
            let token = match w.used_pct {
                _ if stale => Token::FgMuted,
                p if p >= 95.0 => Token::Err,
                p if p >= 80.0 => Token::Warn,
                _ => Token::Fg,
            };
            if i > 0 {
                spans.push(Span::styled(" · ", theme.fg(Token::FgMuted)));
            }
            let label = if wide {
                format!("{} ", w.label)
            } else {
                String::new()
            };
            let bar = if bars {
                let filled = (0..5).filter(|n| f64::from(*n) * 20.0 < w.used_pct).count();
                let (on, off) = theme.icons.bar();
                format!(
                    "{}{} ",
                    on.to_string().repeat(filled),
                    off.to_string().repeat(5 - filled)
                )
            } else {
                String::new()
            };
            spans.push(Span::styled(
                format!("{label}{bar}{:.0}%", w.used_pct),
                theme.fg(token),
            ));
        }
        let age = at.map(|at| model.now.saturating_duration_since(at).as_secs() / 60);
        if let Some(minutes) = age.filter(|m| *m >= LIMITS_FRESH_MINUTES) {
            spans.push(Span::styled(
                format!(" ({})", age_label(minutes)),
                theme.fg(Token::FgMuted),
            ));
        }
    }
    if !spans.is_empty() {
        spans.push(Span::raw(" "));
    }
    spans
}

/// Minutes after which the status bar shows how old the plan limits are:
/// they refresh only when a session in mc reports (DESIGN §6.1).
const LIMITS_FRESH_MINUTES: u64 = 5;

/// Returns `12m ago` / `3h ago` for an age in minutes.
fn age_label(minutes: u64) -> String {
    if minutes < 60 {
        format!("{minutes}m ago")
    } else {
        format!("{}h ago", minutes / 60)
    }
}

/// Returns the bordered box of a text field in a dialog (light border in
/// `accent`, so it reads as the place to type).
pub(super) fn field_block(theme: Theme) -> Block<'static> {
    bordered(Weight::Light, theme).border_style(theme.fg(Token::Accent))
}

/// Border weights (DESIGN §3): light, heavy (focused), double (INTERACT
/// and dialogs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Weight {
    /// Inactive pane.
    Light,
    /// Focused pane.
    Heavy,
    /// INTERACT and dialogs.
    Double,
}

/// Returns a bordered block of `weight`: box-drawing on a UTF-8 locale,
/// the ASCII forms `+-|`, `#=|` and `*=*` otherwise.
fn bordered(weight: Weight, theme: Theme) -> Block<'static> {
    use ratatui::symbols::border::Set;
    let ascii = |corner: &'static str, horizontal: &'static str, vertical: &'static str| Set {
        top_left: corner,
        top_right: corner,
        bottom_left: corner,
        bottom_right: corner,
        vertical_left: vertical,
        vertical_right: vertical,
        horizontal_top: horizontal,
        horizontal_bottom: horizontal,
    };
    let block = Block::bordered();
    match (weight, theme.utf8) {
        (Weight::Light, true) => block.border_type(BorderType::Plain),
        (Weight::Heavy, true) => block.border_type(BorderType::Thick),
        (Weight::Double, true) => block.border_type(BorderType::Double),
        (Weight::Light, false) => block.border_set(ascii("+", "-", "|")),
        (Weight::Heavy, false) => block.border_set(ascii("#", "=", "|")),
        (Weight::Double, false) => block.border_set(ascii("*", "=", "*")),
    }
}

/// Returns a pane: heavy `ok` border when focused, light `border`
/// otherwise (DESIGN §3, borders).
fn pane(title: &str, focused: bool, theme: Theme) -> Block<'static> {
    let (weight, border, text) = if focused {
        (Weight::Heavy, Token::Ok, Token::Fg)
    } else {
        (Weight::Light, Token::Border, Token::FgMuted)
    };
    bordered(weight, theme)
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
    bordered(Weight::Double, theme)
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
        let running = model
            .cards
            .iter()
            .position(crate::app::sessions::Card::running)
            .unwrap();
        model.cards[running].pid = Some(4400);
        let snapshot: Vec<_> = crate::proc::parse_ps(include_str!("proc/testdata/ps-macos.txt"))
            .into_iter()
            .filter(|p| p.uid == 501)
            .collect();
        let ports = std::collections::HashMap::from([(4471, vec![5173]), (4502, vec![5432])]);
        model.open_stop(crate::app::stop::StopKind::Quit, &snapshot, &ports);
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
        model.update(crate::app::model::tests::usage_line(a));
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
    fn limits_render_per_vendor_with_thresholds() {
        use crate::agent::usage::Window;
        let w = |label: &str, used_pct: f64, resets_at: u64| Window {
            label: label.into(),
            used_pct,
            resets_at: Some(resets_at),
        };
        let mut model = sample(PROJECTS);
        model.unix_now = 1_000;
        model.limits[0] = vec![w("5h", 42.0, 2_000), w("7d", 81.0, 2_000)];
        let line = |m: &Model, wide| {
            limit_spans(m, m.theme, wide)
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        assert_eq!(
            line(&model, true),
            "C 5h ###-- 42% · 7d #####- 81% ".replace("#####-", "#####")
        );
        model.limits[1] = vec![w("5h", 10.0, 2_000), w("7d", 3.0, 500)];
        assert_eq!(line(&model, true), "C 5h 42% · 7d 81% · X 5h 10% · 7d 3% ");
        assert_eq!(line(&model, false), "C 42% · X 10% ");
        let theme = Theme::new(
            theme::ThemeName::Dark,
            theme::Profile::Ansi256,
            theme::Background::Paint,
        );
        let spans = limit_spans(&model, theme, true);
        let colour = |text: &str| {
            spans
                .iter()
                .find(|s| s.content.contains(text))
                .unwrap()
                .style
                .fg
        };
        assert_eq!(colour("81%"), Some(theme.color(Token::Warn)));
        assert_eq!(
            colour("3%"),
            Some(theme.color(Token::FgMuted)),
            "stale window is dimmed"
        );
        model.limits = [Vec::new(), Vec::new()];
        assert_eq!(
            line(&model, true),
            "",
            "nothing when no session reported limits"
        );
    }

    #[test]
    fn a_non_utf8_locale_with_unicode_icons_matches_golden() {
        use crate::app::model::tests::with_session;
        let mut model = sample(PROJECTS);
        model.theme = model.theme.with_view(icons::IconSet::Unicode, false, true);
        let (id, _w) = with_session(&mut model, "checkout redesign");
        model.update(crate::app::AppEvent::Pty(crate::term::PtyEvent::Exited(
            id,
            Some(0),
        )));
        model.focus = Focus::Sessions;
        assert_golden("ascii-borders-120x40.txt", &render(&mut model, 120, 40));
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
        assert!(screen.contains("┃ / t"), "{screen}");
        assert!(screen.contains("cursor: 5,2"), "{screen}");
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

    #[test]
    fn shows_sessions_outside_mc_but_not_its_own() {
        use crate::app::model::tests::with_session;
        use crate::external::External;

        let mut model = sample(PROJECTS);
        let (_id, _w) = with_session(&mut model, "mine");
        let project = model.selected_project().unwrap().path.clone();
        model.cards[0].pid = Some(700);
        let ext = |pid, name: &str, status: Option<&str>| External {
            kind: crate::agent::Kind::Claude,
            pid,
            cwd: project.join("sub"),
            name: name.into(),
            status: status.map(Into::into),
            session_id: None,
            started_ms: None,
        };
        model.external = vec![
            ext(700, "mine again", Some("busy")),
            ext(701, "from another tab", Some("idle")),
            External {
                cwd: "/elsewhere".into(),
                ..ext(702, "other project", None)
            },
        ];
        let screen = render(&mut model, 120, 40);
        assert!(
            screen.contains("outside · enter take over · x stop"),
            "{screen}"
        );
        assert!(screen.contains("from another tab"), "{screen}");
        assert!(screen.contains("pid 701"), "{screen}");
        assert!(
            !screen.contains("mine again"),
            "mc's own session is not listed twice"
        );
        assert!(!screen.contains("other project"), "{screen}");
        assert!(
            screen.contains("6 elsewhere"),
            "sessions in no project get a row"
        );
        model.focus = crate::app::model::Focus::Projects;
        for _ in 0..5 {
            model.update(crate::app::model::tests::press(KeyCode::Char('j')));
        }
        let screen = render(&mut model, 120, 40);
        assert!(screen.contains("other project"), "{screen}");
        assert!(screen.contains("pid 702 · /elsewhere"), "{screen}");
        model.external.pop();
        model.update(crate::app::AppEvent::External(model.external.clone()));
        assert_eq!(
            model.visible().len(),
            5,
            "the row goes with its last session"
        );
        assert_eq!(model.selected, 4);
    }

    #[test]
    fn pane_widths_stay_in_range_and_leave_the_output_room() {
        let w = |projects, sessions| Widths { projects, sessions };
        let cases = [
            (w(22, 38), 120, w(22, 38), "defaults fit"),
            (w(5, 5), 200, w(16, 28), "minimums"),
            (w(90, 90), 300, w(40, 72), "caps"),
            (w(40, 72), 120, w(40, 40), "sessions give way to output"),
            (w(40, 72), 100, w(32, 28), "then projects"),
        ];
        for (asked, width, want, why) in cases {
            assert_eq!(asked.fit(width), want, "{why}");
        }
    }

    #[test]
    fn dragging_a_border_resizes_the_pane_and_saves_on_release() {
        use ratatui::crossterm::event::{
            Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        };

        use crate::app::AppEvent;
        use crate::app::model::Cmd;

        let mut model = sample(PROJECTS);
        let mouse = |kind, column| {
            AppEvent::Input(Event::Mouse(MouseEvent {
                kind,
                column,
                row: 10,
                modifiers: KeyModifiers::NONE,
            }))
        };
        assert!(
            model
                .update(mouse(MouseEventKind::Down(MouseButton::Left), 21))
                .is_none()
        );
        model.update(mouse(MouseEventKind::Drag(MouseButton::Left), 29));
        model.update(mouse(MouseEventKind::Drag(MouseButton::Left), 1));
        assert_eq!(model.widths.projects, 16, "held at the minimum");
        model.update(mouse(MouseEventKind::Drag(MouseButton::Left), 29));
        let saved = model.update(mouse(MouseEventKind::Up(MouseButton::Left), 29));
        let want = Widths {
            projects: 30,
            sessions: 38,
        };
        assert!(
            matches!(saved, Some(Cmd::SaveWidths(w)) if w == want),
            "{saved:?}"
        );
        let screen = render(&mut model, 120, 40);
        assert!(
            screen
                .lines()
                .nth(1)
                .unwrap()
                .starts_with("┏ [1] projects ━━━━━━━━━━━━━━┓"),
            "{screen}"
        );
        model.update(mouse(MouseEventKind::Drag(MouseButton::Left), 60));
        assert_eq!(model.widths, want, "no drag without a press on a border");
    }

    #[test]
    fn worktrees_sit_below_their_repository() {
        let mut model = sample(PROJECTS);
        model.projects[1].name = "kedai-web-fix".into();
        model.projects[1].worktree_of = Some("kedai-web".into());
        model.projects[2].worktree_of = Some("kedai-web".into());
        let screen = render(&mut model, 120, 40);
        assert!(screen.contains("1 kedai-web"), "{screen}");
        assert!(screen.contains("2 ├ fix "), "{screen}");
        assert!(screen.contains("3 └ roti-docs"), "{screen}");
        model.theme = model.theme.with_view(model.theme.icons, false, true);
        let screen = render(&mut model, 120, 40);
        assert!(screen.contains("2 |-fix"), "{screen}");
    }

    #[test]
    fn big_screens_put_the_mascot_in_the_output_pane_and_a_click_shows_a_quote() {
        use ratatui::crossterm::event::{
            Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        };

        use crate::app::model::tests::with_session;

        assert_eq!(
            strip_height(Rect::new(0, 0, 100, 30)),
            0,
            "small screens: no strip"
        );
        let mut model = sample(PROJECTS);
        let (_id, _w) = with_session(&mut model, "s");
        let output = panes(model.screen, model.focus, model.zoom, model.widths)
            .output
            .unwrap();
        let spot = strip_mascot(output, model.screen).unwrap();
        assert!(
            output.contains(spot.as_position()),
            "the mascot is in the output pane"
        );
        let click = |column, row| {
            crate::app::AppEvent::Input(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }))
        };
        model.focus = crate::app::model::Focus::Sessions;
        model.update(click(spot.x + 2, spot.y + 1));
        let (at, quote) = model.poke.unwrap();
        let screen = render(&mut model, 120, 40);
        assert!(screen.contains(mascot::QUOTES[quote]), "{screen}");
        model.now = at + mascot::POKE;
        model.update(crate::app::AppEvent::Tick);
        assert!(model.poke.is_none(), "the quote goes after a while");
    }

    #[test]
    fn a_stopped_session_shows_the_died_mascot_in_the_middle() {
        use crate::app::model::tests::with_session;

        let mut model = sample(PROJECTS);
        let (_id, _w) = with_session(&mut model, "s");
        model.cards[0].state = State::Stopped;
        let screen = render(&mut model, 120, 40);
        assert!(screen.contains("Stopped."), "{screen}");
        assert!(screen.contains("r resumes it · d forgets it"), "{screen}");
        assert!(screen.contains('x'), "died eyes");
        assert!(
            screen.contains("▄██████████████▄"),
            "the full-size mascot: {screen}"
        );
    }

    #[test]
    fn old_plan_limits_show_their_age() {
        use crate::agent::usage::Window;

        let mut model = sample(PROJECTS);
        model.limits[0] = vec![Window {
            label: "5h".into(),
            used_pct: 7.0,
            resets_at: None,
        }];
        model.limits_at[0] = Some(model.now);
        assert!(
            !render(&mut model, 120, 40).contains("ago)"),
            "fresh: no age"
        );
        model.now += std::time::Duration::from_mins(12);
        assert!(render(&mut model, 120, 40).contains("7% (12m ago)"));
    }

    #[test]
    fn the_move_dialog_shows_the_mascot_field_and_recent_projects() {
        use crate::app::model::tests::with_session;
        use crate::app::quick::MoveDialog;

        let mut model = sample(PROJECTS);
        let (id, _w) = with_session(&mut model, "s");
        model.overlay = Some(Overlay::Move(MoveDialog {
            id,
            query: String::new(),
            selected: 0,
            error: None,
        }));
        let screen = render(&mut model, 120, 40);
        assert!(
            screen.contains("Move into a project, or name a new one:"),
            "{screen}"
        );
        assert!(screen.contains("recent"), "{screen}");
        assert!(screen.contains("▄██████████████▄"), "the mascot: {screen}");
        let height = |screen: &str| screen.lines().filter(|l| l.contains('║')).count();
        if let Some(Overlay::Move(d)) = &mut model.overlay {
            d.query = "zz".into();
        }
        let typed = render(&mut model, 120, 40);
        assert!(typed.contains("+ enter to create zz"), "{typed}");
        assert_eq!(
            height(&screen),
            height(&typed),
            "the dialog keeps its height"
        );
    }
}
