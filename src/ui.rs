//! Rendering: header, the three panes, the getah bar and overlays
//! (DESIGN §4, §5).
//!
//! Views draw the [`Model`] into a ratatui [`Frame`]; they never read the
//! environment or the terminal. The only thing a view writes back is the
//! number of list rows on screen, for half-page moves. [`panes`] is the
//! one layout function: drawing, mouse hit tests and PTY sizes all use it.

mod activity;
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
use crate::app::tools::TermView;
use crate::store::config::tilde;
use crate::term::session::Size;
use crate::ui::mascot::{Mascot, Mood};
use crate::ui::sanitise::truncate;
use crate::ui::theme::{Theme, Token};

pub(crate) use cards::session_at;

/// Smallest usable screen, per DESIGN §4.
const MIN_SIZE: (u16, u16) = (80, 24);

/// Width from which the three panes sit side by side (DESIGN §4).
const THREE_PANE_WIDTH: u16 = 100;
/// Fewest rows of the sessions pane when it sits above the output pane on
/// a narrower screen: room for one card.
const STACKED_SESSIONS: u16 = 7;

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
    /// The terminal pane, below the output pane.
    pub terminal: Option<Rect>,
    /// The sessions pane.
    pub sessions: Option<Rect>,
    /// The output pane.
    pub output: Option<Rect>,
}

/// Lays out the panes for a screen of `area` (DESIGN §4).
///
/// Three side by side from [`THREE_PANE_WIDTH`] columns. On a narrower
/// screen the projects pane keeps the left and the right column stacks
/// the sessions pane (a third of it, [`STACKED_SESSIONS`] rows at least)
/// over the output pane. `zoom` gives the body to the output pane. The
/// header and getah bar take one line each.
#[must_use]
pub(crate) fn panes(area: Rect, zoom: bool, widths: Widths) -> Panes {
    let [_, body, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    if zoom {
        return Panes {
            projects: None,
            sessions: None,
            output: Some(body),
            terminal: None,
        };
    }
    if area.width < THREE_PANE_WIDTH {
        let projects = widths.projects.clamp(PROJECTS_RANGE.0, PROJECTS_RANGE.1);
        let [projects, right] =
            Layout::horizontal([Constraint::Length(projects), Constraint::Fill(1)]).areas(body);
        let sessions = (right.height / 3).max(STACKED_SESSIONS);
        let [sessions, output] =
            Layout::vertical([Constraint::Length(sessions), Constraint::Fill(1)]).areas(right);
        return Panes {
            projects: Some(projects),
            sessions: Some(sessions),
            output: Some(output),
            terminal: None,
        };
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
        terminal: None,
    }
}

impl Panes {
    /// Returns these panes with the terminal pane, when `shown`, in the
    /// lower third of the output pane.
    #[must_use]
    pub(crate) fn with_terminal(self, shown: bool) -> Self {
        let (Some(output), true) = (self.output, shown) else {
            return self;
        };
        let terminal = Rect {
            y: output.bottom() - output.height / 3,
            height: output.height / 3,
            ..output
        };
        Self {
            output: Some(Rect {
                height: output.height - terminal.height,
                ..output
            }),
            terminal: Some(terminal),
            ..self
        }
    }
}

/// Returns the terminal pane's inner size, which its shell's PTY has.
#[must_use]
pub(crate) fn terminal_size(area: Rect, zoom: bool, widths: Widths) -> Size {
    let pane = panes(area, zoom, widths)
        .with_terminal(true)
        .terminal
        .unwrap_or(area);
    Size {
        cols: pane.width.saturating_sub(2).max(1),
        rows: pane.height.saturating_sub(2).max(1),
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
/// (the size the pane has whenever it is shown); `terminal` says whether
/// the terminal pane takes its lower third.
#[must_use]
pub(crate) fn output_size(area: Rect, zoom: bool, widths: Widths, terminal: bool) -> Size {
    let pane = panes(area, zoom, widths)
        .with_terminal(terminal)
        .output
        .unwrap_or(area);
    Size {
        cols: pane.width.saturating_sub(2).max(1),
        rows: pane.height.saturating_sub(2 + STRIP_HEIGHT).max(1),
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
    let layout = model.panes(area);
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
    if let (Some(rect), Some(shell)) = (layout.terminal, model.shell()) {
        let focused = model.term_view == TermView::Focused;
        let card = model.selected_card().map(|i| &model.cards[i]);
        let project = model.selected_project().map_or("", |p| p.name.as_str());
        let name = card.map_or(project, |c| c.name.as_str());
        let title = if focused {
            let leave = model.exit_chord.label();
            format!("[4] terminal · {name} · {leave} to leave")
        } else {
            format!("[4] terminal · {name} · t hides · T closes")
        };
        output::draw_tool(frame, rect, &title, &shell.pty, focused, theme);
    } else if let (Some(rect), Some(_)) = (layout.output, model.shell()) {
        draw_hidden_terminal(frame, rect, theme);
    }
    draw_getah(frame, getah, model, theme);
    if model.popup.is_some() {
        output::draw_popup(frame, popup_rect(area), model, theme);
    }
    if let Some(editor) = &model.editor {
        let rect = popup_rect(area);
        frame.render_widget(ratatui::widgets::Clear, rect);
        output::draw_tool(
            frame,
            rect,
            "editor · quit it to close",
            &editor.pty,
            true,
            theme,
        );
    }
    match &model.overlay {
        Some(Overlay::Help) => help::draw(frame, area, model.focus.scope(), theme),
        Some(Overlay::Activity) => activity::draw(frame, area, &model.activity, theme),
        Some(Overlay::Form(f)) => form::draw_settings(frame, area, f, theme, model.host_light),
        Some(Overlay::Picker(p)) => dialogs::draw_picker(frame, area, p, theme),
        Some(Overlay::Stop(d)) => dialogs::draw_stop(frame, area, d, model, theme),
        Some(Overlay::Forget(id)) => dialogs::draw_forget(frame, area, *id, model, theme),
        Some(Overlay::TakeOver(ext, _)) => dialogs::draw_take_over(frame, area, ext, theme),
        Some(Overlay::ResumeAgent(kind)) => dialogs::draw_resume_agent(frame, area, *kind, theme),
        Some(Overlay::Move(dialog)) => dialogs::draw_move(frame, area, dialog, model, theme),
        Some(Overlay::StopOutside(ext)) => dialogs::draw_stop_outside(frame, area, ext, theme),
        Some(Overlay::NewProject(dialog)) => dialogs::draw_new_project(frame, area, dialog, theme),
        Some(Overlay::Switcher(switcher)) => {
            dialogs::draw_switcher(frame, area, switcher, model, theme);
        }
        Some(Overlay::Finder(finder)) => dialogs::draw_finder(frame, area, finder, theme),
        Some(Overlay::Links(viewer)) => dialogs::draw_links(frame, area, viewer, theme),
        Some(Overlay::CleanWorktrees(project)) => {
            dialogs::draw_clean_worktrees(frame, area, project, theme);
        }
        Some(Overlay::TrashProject(projects)) => {
            dialogs::draw_trash_project(frame, area, projects, model, theme);
        }
        None => {}
    }
}

/// Columns between the bubble's tail and the mascot.
const STRIP_GAP: u16 = 1;

/// Rows the output pane gives the mascot strip at its top (DESIGN §5.7),
/// so the mascot and its bubble never cover agent output.
pub(crate) const STRIP_HEIGHT: u16 = mascot::MINI_HEIGHT;

/// Returns where the strip mascot sits in output pane `output` (the click
/// target).
#[must_use]
pub(crate) fn strip_mascot(output: Rect) -> Rect {
    Rect::new(
        output.right().saturating_sub(mascot::MINI_WIDTH + 2),
        output.y + 1,
        mascot::MINI_WIDTH,
        mascot::MINI_HEIGHT,
    )
}

/// Draws the strip mascot at `spot` in `mood` and, after a click or a
/// notification, its speech bubble to the left (within `strip`).
///
/// A click plays [`mascot::poke_pose`] and shows a quote for
/// [`mascot::POKE`]. A notification (a session finished, needs you or failed, see
/// [`Model::notice`]) is said for [`mascot::NOTICE`], cut to the room the
/// strip has; a quote from a click goes first.
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
    let room = spot.x.saturating_sub(strip.x + STRIP_GAP + 1);
    let text = match (clicked, &model.notice) {
        (Some((_, quote)), _) => mascot::QUOTES[quote % mascot::QUOTES.len()].to_owned(),
        (None, Some((_, notice))) => {
            sanitise::sanitise(notice, usize::from(room.saturating_sub(4)))
        }
        (None, None) => return,
    };
    let width = u16::try_from(text.chars().count() + 4).unwrap_or(u16::MAX);
    if width > room || text.is_empty() {
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

/// Draws line 1: the app name and where you are, the version right, and
/// after it the newer release once the update check found one.
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
    let newer = model
        .newer
        .as_ref()
        .map_or_else(String::new, |tag| format!("→ {tag} · U updates "));
    let right = version.chars().count() + newer.chars().count();
    let room = usize::from(area.width).saturating_sub(" bungkus-mc  ".len() + right + 1);
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
        Line::from(vec![
            Span::styled(version, theme.fg(Token::FgMuted)),
            Span::styled(newer, theme.fg(Token::Info)),
        ])
        .alignment(Alignment::Right),
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
/// When recent projects are grouped ([`Model::rest`]), the rest line
/// `+ <count> more (e)` follows them (`-` while the rest shows; `▸`/`▾`
/// outside the ascii icon set).
///
/// The marker is `>` on the selected row while the pane is focused and
/// `:` (the ascii form of `▌`) while it is not. The spinner column turns
/// while any session of the project works; from two working sessions the
/// spinner with their number (`|2`) sits at the right; after it the badge is the worst other state,
/// failed > needs you > your turn, with its count.
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
    let bars = visible.iter().map(|p| link_count(model, &p.path));
    let bars = bars.max().unwrap_or(0).max(1);
    let width = usize::from(inner.width).saturating_sub(3 + digits + bars);
    let rest = model.rest();
    let rest_at = rest.map(|rest| rest.at);
    let chosen = line_of(model.selected, rest_at);
    let offset = projects_offset(chosen, rows);
    let header = rest_line(rest.map_or(0, |rest| rest.count), model.show_rest, theme);
    let spin = cards::spinner(model.frame, theme);
    let lines: Vec<Line> = (offset..)
        .take(rows)
        .map_while(|line| row_at(line, rest_at, visible.len()))
        .map(|row| {
            let Row::Project(i) = row else {
                return header.clone();
            };
            let project = visible[i];
            let selected = i == model.selected;
            let marker = match (selected, focused) {
                (true, true) => theme.icons.icon(icons::Icon::Marker),
                (true, false) if theme.icons == icons::IconSet::Ascii => ':',
                (true, false) => '▌',
                (false, _) => ' ',
            };
            let name = match (selected, model.is_chosen(i, &project.path)) {
                (_, true) => theme.fg(Token::Accent).add_modifier(Modifier::REVERSED),
                (true, false) => theme.fg(Token::Accent),
                (false, false) => theme.fg(Token::Fg),
            };
            let states: Vec<State> = sessions::order(&model.cards, &project.path)
                .into_iter()
                .map(|i| model.cards[i].state.clone())
                .chain(model.external_in(&project.path).iter().map(|e| e.state()))
                .collect();
            let working = states.iter().filter(|s| **s == State::Working).count();
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
            let gap = if badge.is_some() { "" } else { " " };
            let count = (working > 1).then(|| format!(" {spin}{working}{gap}"));
            let count = count.unwrap_or_default();
            let taken = badge_text.chars().count() + count.chars().count();
            let branch = tree_mark(&visible, i, theme.utf8);
            let label = worktree_label(project);
            let room = width.saturating_sub(taken + branch.map_or(0, |_| 2));
            let text = truncate(label, room);
            let pad = room.saturating_sub(text.chars().count());
            let spinner = if working > 0 { spin } else { ' ' };
            let mut spans = vec![
                Span::styled(marker.to_string(), theme.fg(Token::Ok)),
                Span::styled(spinner.to_string(), theme.fg(Token::Ok)),
                Span::raw(" "),
                Span::styled(
                    if model.root() == Some(project.path.as_path()) {
                        format!("{:>digits$}", 0)
                    } else {
                        format!("{:>digits$}", i + 1 - quick_first)
                    },
                    theme.fg(Token::Ok),
                ),
            ];
            spans.extend(link_bars(model, &project.path, bars, theme));
            spans.extend([
                Span::styled(branch.unwrap_or_default(), theme.fg(Token::FgMuted)),
                Span::styled(text, name),
                Span::raw(" ".repeat(pad)),
                Span::styled(count, theme.fg(Token::Ok)),
                Span::styled(badge_text, theme.fg(badge_token)),
            ]);
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    shade(frame, inner, chosen - offset, 1, theme);
}

/// Gives `height` rows of `inner`, from row `top`, the selection
/// background (the selected project row, the selected card), clipped to
/// the pane; nothing where mc does not paint its background.
pub(crate) fn shade(frame: &mut Frame, inner: Rect, top: usize, height: usize, theme: Theme) {
    let Some(style) = theme.selection() else {
        return;
    };
    let clamp = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    let rows = Rect {
        y: inner.y.saturating_add(clamp(top)),
        height: clamp(height),
        ..inner
    };
    frame
        .buffer_mut()
        .set_style(rows.intersection(inner), style);
}

/// Rows of the projects pane above its first project: the top border and
/// the search row.
const PROJECTS_HEAD: u16 = 2;

/// Returns the index of the first project row shown when `selected` must
/// stay inside `rows` rows.
fn projects_offset(selected: usize, rows: usize) -> usize {
    (selected + 1).saturating_sub(rows)
}

/// What a line of the projects list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    /// The project at this index into [`Model::visible`].
    Project(usize),
    /// The line that shows or hides the projects that are not recent.
    Rest,
}

/// Returns the list line of project row `selected`: the rest line, which
/// sits before row `rest`, pushes the rows after it down by one.
fn line_of(selected: usize, rest: Option<usize>) -> usize {
    selected + usize::from(rest.is_some_and(|at| selected >= at))
}

/// Returns what list line `line` holds among `len` project rows, or
/// `None` past the end (the inverse of [`line_of`]).
fn row_at(line: usize, rest: Option<usize>, len: usize) -> Option<Row> {
    let row = match rest {
        Some(at) if line == at => return Some(Row::Rest),
        Some(at) if line > at => line - 1,
        _ => line,
    };
    (row < len).then_some(Row::Project(row))
}

/// Returns what is drawn at screen row `row` of projects pane `pane`,
/// for mouse hit tests.
///
/// # Arguments
///
/// * `pane`     - The projects pane.
/// * `selected` - The selected row, which the list scrolls to keep in view.
/// * `rest`     - The row the rest line sits before, if any.
/// * `len`      - How many project rows there are.
/// * `row`      - The screen row.
///
/// # Returns
///
/// `None` on the border, the search row and below the list.
#[must_use]
pub(crate) fn project_at(
    pane: Rect,
    selected: usize,
    rest: Option<usize>,
    len: usize,
    row: u16,
) -> Option<Row> {
    let rows = pane.height.saturating_sub(PROJECTS_HEAD + 1);
    let line = row
        .checked_sub(pane.y + PROJECTS_HEAD)
        .filter(|line| *line < rows)?;
    let offset = projects_offset(line_of(selected, rest), usize::from(rows));
    row_at(offset + usize::from(line), rest, len)
}

/// The colours of the group bars, by a group's place in the workspace's
/// `groups`; the fifth group has the first colour again.
const GROUP_TOKENS: [Token; 4] = [Token::Accent, Token::Info, Token::Warn, Token::Ok];

/// Returns how many link bars `project` carries: one per group it is in,
/// or one when it is in none and a session of another project works in
/// it too (`/add-dir`).
fn link_count(model: &Model, project: &std::path::Path) -> usize {
    let groups = model.groups_of(project).len();
    groups.max(usize::from(model.shared(project)))
}

/// Returns the `width` cells between a project's number and its name: the
/// link bars (`❚`, `|` without UTF-8), then spaces.
///
/// A project in groups carries one bar per group, in the group's colour
/// ([`GROUP_TOKENS`]): full while the selected project is in that group,
/// dim otherwise. A project in no group that a session of another project
/// works in too (`/add-dir`) carries one bar: `fg` when that link is with
/// the selected project, else `fg-muted`.
fn link_bars(
    model: &Model,
    project: &std::path::Path,
    width: usize,
    theme: Theme,
) -> Vec<Span<'static>> {
    let bar = if theme.utf8 { "❚" } else { "|" };
    let chosen = model.selected_project().map(|p| p.path.as_path());
    let active = chosen.map_or_else(Vec::new, |c| model.groups_of(c));
    let groups = model.groups_of(project);
    let mut bars: Vec<Span> = groups
        .iter()
        .map(|group| {
            let style = theme.fg(GROUP_TOKENS[group % GROUP_TOKENS.len()]);
            if active.contains(group) {
                Span::styled(bar, style)
            } else {
                Span::styled(bar, style.add_modifier(Modifier::DIM))
            }
        })
        .collect();
    if groups.is_empty() && model.shared(project) {
        let near = chosen.is_some_and(|c| c == project || model.related(c, project));
        let token = if near { Token::Fg } else { Token::FgMuted };
        bars.push(Span::styled(bar, theme.fg(token)));
    }
    bars.push(Span::raw(" ".repeat(width.saturating_sub(bars.len()))));
    bars
}

/// Returns the connector that ties worktree row `i` of `visible` to its
/// repository above (the last one of a repository closes the tree), or
/// `None` for a row that is no worktree.
fn tree_mark(visible: &[&crate::workspace::Project], i: usize, utf8: bool) -> Option<&'static str> {
    let of = visible.get(i)?.worktree_of.as_ref()?;
    let last = visible
        .get(i + 1)
        .is_none_or(|next| next.worktree_of.as_ref() != Some(of));
    Some(match (utf8, last) {
        (true, true) => "└ ",
        (true, false) => "├ ",
        (false, true) => "`-",
        (false, false) => "|-",
    })
}

/// Returns the rest line of the projects list: `count` projects that are
/// not recent, folded away or (`open`) shown below it.
fn rest_line(count: usize, open: bool, theme: Theme) -> Line<'static> {
    let fold = match (open, theme.icons == icons::IconSet::Ascii) {
        (false, true) => '+',
        (true, true) => '-',
        (false, false) => '▸',
        (true, false) => '▾',
    };
    Line::styled(
        format!(" {fold} {count} more (e)"),
        theme.fg(Token::FgMuted),
    )
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

/// Marks a hidden terminal pane whose shell still runs: a label on the
/// bottom border of the output pane `output` (DESIGN §8.1).
fn draw_hidden_terminal(frame: &mut Frame, output: Rect, theme: Theme) {
    let label = " [4] terminal · t shows · T closes ";
    let width = u16::try_from(label.chars().count()).unwrap_or(u16::MAX);
    let at = Rect {
        x: output.x + 1,
        y: output.bottom().saturating_sub(1),
        width: width.min(output.width.saturating_sub(2)),
        height: 1,
    };
    frame.render_widget(Span::styled(label, theme.fg(Token::FgMuted)), at);
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
    let terminal = model.term_view == TermView::Focused && model.shell().is_some();
    let (mode, token) = if model.editor.is_some() {
        (" EDITOR ", Token::Warn)
    } else if model.popup.is_some() {
        (" QUICK ", Token::Warn)
    } else if terminal {
        (" TERMINAL ", Token::Warn)
    } else if interact {
        (" INTERACT ", Token::Warn)
    } else if model.filtering {
        (" FILTER ", Token::Info)
    } else if model.choosing() {
        (" VISUAL ", Token::Info)
    } else {
        (" NORMAL ", Token::FgMuted)
    };
    let text = if let Some(message) = &model.message {
        message.clone()
    } else if model.editor.is_some() {
        "keys go to the editor · quit it to come back".to_owned()
    } else if terminal && model.popup.is_none() {
        format!(
            "keys go to the terminal · {} back to mc",
            model.exit_chord.label()
        )
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
        let here = model.selected_card().map(|i| model.cards[i].id);
        let waiting = model
            .cards
            .iter()
            .any(|c| c.state == State::NeedsYou && Some(c.id) != here);
        format!(
            "keys go to {agent} · {} back to mc{}",
            model.exit_chord.label(),
            if waiting {
                " · ctrl-] next needs you"
            } else {
                ""
            }
        )
    } else if model.choosing() {
        "v mark · j/k move · g group · d move to Trash · esc clear".to_owned()
    } else if model.filtering {
        "type to search · ↑↓ pick · enter open · esc clear".to_owned()
    } else if let Some(ch) = model.pending {
        format!("{ch}…")
    } else {
        let mut hints = keymap::hints(model.focus.scope());
        if tally(model).needs_you > 0 {
            hints.insert(0, "! next needs you");
        }
        hints.join(" · ")
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
        crate::agent::Kind,
        &Vec<crate::agent::usage::Window>,
        Option<std::time::Instant>,
    )> = crate::agent::Kind::ALL
        .iter()
        .map(|k| (*k, &model.limits[*k as usize], model.limits_at[*k as usize]))
        .filter(|(_, w, _)| !w.is_empty())
        .collect();
    let bars = vendors.len() == 1 && wide;
    let mut spans = Vec::new();
    for (v, (kind, windows, at)) in vendors.iter().enumerate() {
        if v > 0 {
            spans.push(Span::styled(" · ", theme.fg(Token::FgMuted)));
        }
        spans.push(Span::styled(
            format!("{} ", theme.icons.agent(*kind)),
            theme.agent_style(*kind),
        ));
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
            spans.push(Span::styled(label, theme.fg(token)));
            if bars {
                let (on, off) = bar(w.used_pct, 5, theme.utf8);
                spans.push(Span::styled(on, theme.fg(token)));
                spans.push(Span::styled(format!("{off} "), theme.fg(Token::FgMuted)));
            }
            spans.push(Span::styled(format!("{:.0}%", w.used_pct), theme.fg(token)));
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

/// Returns a progress bar of `cells` cells filled to `pct` percent as its
/// filled and its empty part, to be coloured apart: solid `█` then shaded
/// `░` (the look of indicatif's default bar), or `#` then `-` on a
/// terminal without UTF-8.
///
/// The fill goes to the nearest cell, with one cell at least for anything
/// above zero, so an empty bar means nothing used.
#[must_use]
pub(crate) fn bar(pct: f64, cells: u8, utf8: bool) -> (String, String) {
    let filled = (1..=cells)
        .filter(|cell| pct * f64::from(cells) >= (f64::from(*cell) - 0.5) * 100.0)
        .count()
        .max(usize::from(pct > 0.0 && cells > 0));
    let empty = usize::from(cells) - filled;
    let (on, off) = if utf8 { ("█", "░") } else { ("#", "-") };
    (on.repeat(filled), off.repeat(empty))
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
    fn activity_overlay_matches_golden() {
        use crate::app::activity::Row;

        let mut model = sample(PROJECTS);
        key(&mut model, KeyCode::Char('A'));
        assert!(render(&mut model, 80, 24).contains("measuring"));
        let row = |label: &str, procs, rss_kb, cpu| Row {
            label: label.into(),
            procs,
            rss_kb,
            cpu_tenths: Some(cpu),
        };
        model.activity.rows = vec![
            row("mc", 1, 28_000, 12),
            row("#a3f1 checkout redesign", 7, 1_640_000, 1234),
            row("terminals & other", 2, 9_000, 0),
        ];
        model.activity.cores = 8;
        model.activity.mem_total_kb = Some(16 * 1024 * 1024);
        model.activity.temp_c = Some(54);
        assert_golden("activity-80x24.txt", &render(&mut model, 80, 24));
        key(&mut model, KeyCode::Esc);
        assert_eq!(model.overlay, None);
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
        assert_eq!(line(&model, true), "C 5h ██░░░ 42% · 7d ████░ 81% ");
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
    fn a_newer_release_stays_in_the_header_after_a_key_clears_the_message() {
        let mut model = crate::app::model::tests::sample(&["a"]);
        model.update(crate::app::AppEvent::UpdateAvailable("v9.9.9".into()));
        model.update(crate::app::model::tests::press(
            ratatui::crossterm::event::KeyCode::Char('j'),
        ));
        assert_eq!(model.message, None);
        let screen = render(&mut model, 120, 40);
        let header = screen.lines().next().unwrap();
        let want = format!("v{} → v9.9.9 · U updates", env!("CARGO_PKG_VERSION"));
        assert!(header.ends_with(&want), "{header}");
    }

    #[test]
    fn the_selected_row_and_card_are_shaded_only_when_painting() {
        use crate::ui::theme::{Background, Profile, ThemeName, rgb, selection_bg};
        let mut model = crate::app::model::tests::sample(&["a", "b"]);
        crate::app::model::tests::with_session(&mut model, "one");
        crate::app::model::tests::with_session(&mut model, "two");
        let shaded = |model: &mut Model, profile| -> Vec<(u16, u16)> {
            model.theme = Theme::new(ThemeName::Dark, profile, Background::Paint);
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            terminal.draw(|frame| draw(frame, model)).unwrap();
            let buffer = terminal.backend().buffer();
            let want = rgb(selection_bg(ThemeName::Dark));
            // Column 2 is inside the projects pane, column 24 the sessions pane.
            [2_u16, 24]
                .into_iter()
                .flat_map(|x| (0..40).map(move |y| (x, y)))
                .filter(|&(x, y)| buffer[(x, y)].bg == want)
                .collect()
        };
        assert_eq!(shaded(&mut model, Profile::Ansi256), []);
        let rows = shaded(&mut model, Profile::TrueColor);
        let projects = rows.iter().filter(|(x, _)| *x == 2).count();
        let card = rows.iter().filter(|(x, _)| *x == 24).count();
        assert_eq!((projects, card), (1, 3), "one project row, one 3-line card");
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
        model.show_rest = true;
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
        model.workspaces = vec!["/other".into()];
        model.external.push(External {
            cwd: "/other/project".into(),
            ..ext(703, "other workspace", None)
        });
        assert_eq!(
            model.visible().len(),
            5,
            "a session of another saved workspace gets no row here"
        );
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
    fn the_mascot_sits_in_the_output_pane_and_a_click_shows_a_quote() {
        use ratatui::crossterm::event::{
            Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        };

        use crate::app::model::tests::with_session;

        let mut model = sample(PROJECTS);
        let (_id, _w) = with_session(&mut model, "s");
        let output = panes(model.screen, model.zoom, model.widths)
            .output
            .unwrap();
        let spot = strip_mascot(output);
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
    fn a_notification_shows_in_the_strip_mascots_bubble_for_a_while() {
        use crate::app::model::tests::with_session;

        let mut model = sample(PROJECTS);
        let (_id, _w) = with_session(&mut model, "s");
        model.notice = Some((model.now, "#a3f1 needs you: \x1b[31mdeploy".into()));
        let screen = render(&mut model, 120, 40);
        assert!(screen.contains("#a3f1 needs you: deploy"), "{screen}");
        model.now += mascot::NOTICE;
        model.update(crate::app::AppEvent::Tick);
        assert!(model.notice.is_none(), "it goes after a while");
    }

    #[test]
    fn a_notification_shows_on_a_small_screen_and_with_no_session() {
        let mut model = sample(PROJECTS);
        model.screen = Rect::new(0, 0, 80, 24);
        model.notice = Some((model.now, "#a3f1 finished: deploy".into()));
        let screen = render(&mut model, 80, 24);
        assert!(screen.contains("#a3f1 finished: deploy"), "{screen}");
        assert!(
            screen.contains("Nothing wrapped yet."),
            "the empty state stays: {screen}"
        );
    }

    #[test]
    fn a_bar_fills_to_the_nearest_cell_and_never_hides_a_small_use() {
        let joined = |pct, cells, utf8| {
            let (on, off) = bar(pct, cells, utf8);
            format!("{on}{off}")
        };
        for (pct, want) in [
            (0.0, "░░░░░░░░░░"),
            (0.4, "█░░░░░░░░░"),
            (23.0, "██░░░░░░░░"),
            (85.0, "█████████░"),
            (94.0, "█████████░"),
            (100.0, "██████████"),
            (130.0, "██████████"),
        ] {
            assert_eq!(joined(pct, 10, true), want, "{pct}");
        }
        assert_eq!(joined(50.0, 5, false), "###--", "no UTF-8");
    }

    #[test]
    fn a_project_row_counts_its_working_sessions_from_two() {
        use crate::app::model::tests::with_session;

        let mut model = sample(PROJECTS);
        let _one = with_session(&mut model, "one");
        model.cards[0].state = State::Working;
        let row = |model: &mut Model| -> String {
            let screen = render(model, 120, 40);
            screen.lines().nth(3).unwrap().chars().take(22).collect()
        };
        assert_eq!(
            row(&mut model),
            "│:| 1 kedai-web      │",
            "one: the spinner alone"
        );
        let _two = with_session(&mut model, "two");
        model.cards[1].state = State::Working;
        assert_eq!(row(&mut model), "│:| 1 kedai-web   |2 │");
    }

    #[test]
    fn a_folder_another_projects_session_added_gets_the_link_bar() {
        use crate::app::model::tests::with_session;

        let mut model = sample(PROJECTS);
        let _w = with_session(&mut model, "s");
        let (home, added) = (
            model.projects[0].path.clone(),
            model.projects[2].path.clone(),
        );
        let usage = crate::agent::usage::Usage {
            added_dirs: vec![added.join("docs"), home.clone()],
            ..Default::default()
        };
        model.cards[0].usage = Some(usage);
        assert!(model.shared(&added) && model.related(&home, &added));
        assert!(!model.shared(&home), "its own folder is no link");
        let root = model.root().unwrap().to_path_buf();
        assert!(!model.shared(&root) && !model.shared(std::path::Path::new("")));
        assert!(!model.shared(&model.projects[1].path));
        let names: Vec<&str> = model.visible().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["kedai-web", "roti-docs"],
            "in use: it stays in view"
        );
        let screen = render(&mut model, 120, 40);
        assert!(screen.contains(" 2❚roti-docs"), "{screen}");
        model.cards[0].state = State::Wrapped;
        assert!(!model.shared(&added), "only while the session runs");
    }

    #[test]
    fn every_group_has_its_own_bar_and_the_selected_projects_groups_are_not_dim() {
        let mut model = sample(PROJECTS);
        model.show_rest = true;
        let name = |i: usize| model.projects[i].name.clone();
        model.overrides.groups = vec![vec![name(0), name(1)], vec![name(1), name(2)]];
        let bars = |model: &Model, i: usize| -> Vec<(String, Style)> {
            link_bars(model, &model.projects[i].path, 2, model.theme)
                .into_iter()
                .map(|span| (span.content.into_owned(), span.style))
                .collect()
        };
        let theme = model.theme;
        let (first, second) = (theme.fg(GROUP_TOKENS[0]), theme.fg(GROUP_TOKENS[1]));
        let dim = |style: Style| style.add_modifier(Modifier::DIM);
        let bar = |style| ("❚".to_owned(), style);
        let pad = |n: usize| (" ".repeat(n), Style::default());
        assert_eq!(model.selected, 0, "the first project is selected");
        assert_eq!(bars(&model, 0), [bar(first), pad(1)]);
        assert_eq!(
            bars(&model, 1),
            [bar(first), bar(dim(second)), pad(0)],
            "two groups: two bars, the other group's is dim"
        );
        assert_eq!(bars(&model, 2), [bar(dim(second)), pad(1)]);
        let screen = render(&mut model, 120, 40);
        assert!(screen.contains("❚❚"), "{screen}");
    }

    #[test]
    fn narrower_screens_stack_the_sessions_over_the_output() {
        let stacked = panes(Rect::new(0, 0, 80, 24), false, Widths::default());
        let (projects, sessions, output) = (
            stacked.projects.unwrap(),
            stacked.sessions.unwrap(),
            stacked.output.unwrap(),
        );
        assert_eq!((sessions.x, output.x), (projects.right(), projects.right()));
        assert_eq!((sessions.height, output.y), (7, sessions.bottom()));
        let with = stacked.with_terminal(true);
        let (output, terminal) = (with.output.unwrap(), with.terminal.unwrap());
        assert_eq!((terminal.x, terminal.y), (output.x, output.bottom()));
        let wide = panes(Rect::new(0, 0, 100, 24), false, Widths::default());
        assert_eq!(wide.sessions.unwrap().y, wide.output.unwrap().y);
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
