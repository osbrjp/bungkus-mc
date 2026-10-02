//! The output pane: the selected session's live screen, or the empty
//! state with the mascot (DESIGN §4, §5.7, §8.5).
//!
//! In INTERACT the pane gets a double border in `warn` and `INTERACT` in
//! its title, in the same frame the focus moves here.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::{Focus, Model};
use crate::app::sessions::{Card, State};
use crate::term::screen::Screen;
use crate::ui::mascot::{self, Mascot, Mood};
use crate::ui::pane;
use crate::ui::sanitise::truncate;
use crate::ui::theme::{Theme, Token, bg, rgb, spec};

/// Draws the output pane.
pub(super) fn draw(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let Some(card) = model.selected_card().map(|i| &model.cards[i]) else {
        draw_empty(frame, area, theme, model.frame);
        return;
    };
    if model.is_quick(card) {
        draw_quick_note(frame, area, theme, card.running());
        return;
    }
    let interact = model.focus == Focus::Output;
    let title = output_title(card, model, area.width);
    let title_width = title.chars().count() + 4;
    let block = if interact {
        super::bordered(super::Weight::Double, theme)
            .border_style(theme.fg(Token::Warn))
            .title(Span::styled(format!(" {title} "), theme.fg(Token::Warn)))
    } else {
        pane(&title, false, theme)
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if card.state == State::Stopped {
        let lines = [
            Line::styled("Stopped.", theme.fg(Token::Fg)),
            Line::styled("r resumes it · d forgets it", theme.fg(Token::FgMuted)),
        ];
        return draw_centred(frame, inner, theme, mascot::Pose::Died, &lines);
    }
    let strip = super::strip_height(model.screen);
    let inner = if let Some(spot) = super::strip_mascot(area, model.screen) {
        let [top, rest] =
            Layout::vertical([Constraint::Length(strip), Constraint::Fill(1)]).areas(inner);
        super::draw_strip(frame, top, spot, mood(&card.state), model, theme);
        rest
    } else {
        inner
    };
    match &card.pty {
        Some(pty) if pty.last_output.is_some() => {
            let (fg, bg) = default_colors(theme);
            let screen = Screen {
                term: pty.term(),
                fg,
                bg,
            };
            let cursor = screen.cursor(inner);
            let busy = pty
                .last_output
                .is_some_and(|t| model.now.saturating_duration_since(t) < BUSY);
            let (w, h) = if busy {
                (mascot::MINI_WIDTH, mascot::MINI_HEIGHT)
            } else {
                (mascot::WIDTH, mascot::HEIGHT)
            };
            let corner = (strip == 0 && inner.width > w + 1 && inner.height > h)
                .then(|| Rect::new(inner.width - 1 - w, 0, w, h));
            let blank = corner.is_some_and(|c| screen.is_blank(c));
            frame.render_widget(screen, inner);
            let mood = mood(&card.state);
            match corner {
                Some(c) if blank => {
                    let pose = mood.pose(model.frame, !theme.no_color());
                    let at = Rect {
                        x: inner.x + c.x,
                        y: inner.y + c.y,
                        ..c
                    };
                    frame.render_widget(
                        Mascot {
                            theme,
                            pose,
                            mini: busy,
                        },
                        at,
                    );
                }
                Some(_) if usize::from(area.width) >= title_width + 8 => {
                    let (text, token) = if mood == Mood::Failed {
                        ("/xx\\", Token::Err)
                    } else {
                        ("/..\\", Token::Ok)
                    };
                    let at = Rect::new(area.right() - 8, area.y, 6, 1);
                    frame.render_widget(Line::styled(format!(" {text} "), theme.fg(token)), at);
                }
                _ => {}
            }
            if let (true, Some(position)) = (interact, cursor) {
                frame.set_cursor_position(position);
            }
        }
        _ => draw_message(frame, inner, card, theme),
    }
}

/// Returns the output pane's title: agent badge, short id, the session's
/// name cut to fit `width`, and the INTERACT note when the pane has focus.
fn output_title(card: &Card, model: &Model, width: u16) -> String {
    let interact = model.focus == Focus::Output;
    let badge = model.theme.icons.agent(card.kind);
    let head = format!("[3] output · {badge} {} · ", card.id.short());
    let tail = if interact {
        format!(" · INTERACT · {} to leave", model.exit_chord.label())
    } else {
        String::new()
    };
    let room = usize::from(width).saturating_sub(head.chars().count() + tail.chars().count() + 4);
    format!("{head}{}{tail}", truncate(&card.name, room))
}

/// Draws the quick-session popup (issue #46): a cleared, double-bordered
/// window with the session's live screen and the cursor, over the panes.
pub(super) fn draw_popup(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let Some(card) = model
        .popup
        .and_then(|id| model.cards.iter().find(|c| c.id == id))
    else {
        return;
    };
    let tail = if model.popup_menu {
        " · h hide · m move".to_owned()
    } else {
        format!(" · ctrl-m move · {} menu", model.exit_chord.label())
    };
    let badge = theme.icons.agent(card.kind);
    let head = format!("quick · {badge} {} · ", card.id.short());
    let room =
        usize::from(area.width).saturating_sub(head.chars().count() + tail.chars().count() + 4);
    let title = format!(" {head}{}{tail} ", truncate(&card.name, room));
    frame.render_widget(ratatui::widgets::Clear, area);
    let block = super::bordered(super::Weight::Double, theme)
        .border_style(theme.fg(Token::Warn))
        .title(Span::styled(title, theme.fg(Token::Warn)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(pty) = &card.pty else {
        return;
    };
    let (fg, bg) = default_colors(theme);
    let screen = Screen {
        term: pty.term(),
        fg,
        bg,
    };
    let cursor = screen.cursor(inner);
    frame.render_widget(screen, inner);
    if let Some(position) = cursor {
        frame.set_cursor_position(position);
    }
}

/// Draws one of the user's tools (the terminal pane, the editor popup):
/// its live screen under `title`, double-bordered with the cursor while it
/// has the keys.
pub(super) fn draw_tool(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    pty: &crate::term::session::Session,
    focused: bool,
    theme: Theme,
) {
    let block = if focused {
        super::bordered(super::Weight::Double, theme)
            .border_style(theme.fg(Token::Warn))
            .title(Span::styled(format!(" {title} "), theme.fg(Token::Warn)))
    } else {
        pane(title, false, theme)
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (fg, bg) = default_colors(theme);
    let screen = Screen {
        term: pty.term(),
        fg,
        bg,
    };
    let cursor = screen.cursor(inner);
    frame.render_widget(screen, inner);
    if let (true, Some(position)) = (focused, cursor) {
        frame.set_cursor_position(position);
    }
}

/// Draws the output pane for a selected quick session, which lives in
/// its popup instead (issue #46).
fn draw_quick_note(frame: &mut Frame, area: Rect, theme: Theme, running: bool) {
    let block = pane("[3] output", false, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [row] = Layout::vertical([Constraint::Length(1)])
        .flex(Flex::Center)
        .areas(inner);
    frame.render_widget(
        Line::styled(
            if running {
                "Quick session: enter opens it · m moves it to a project"
            } else {
                "Quick session ended: enter resumes it · m moves it · d forgets it"
            },
            theme.fg(Token::FgMuted),
        )
        .alignment(Alignment::Center),
        row,
    );
}

/// How recently the agent must have written to count as busy (mini sprite).
const BUSY: std::time::Duration = std::time::Duration::from_secs(1);

/// Returns the mascot's mood for a session state (DESIGN §5.7).
fn mood(state: &State) -> Mood {
    match state {
        State::NeedsYou => Mood::NeedsYou,
        State::Working => Mood::Working,
        State::YourTurn => Mood::YourTurn,
        State::Failed(_) => Mood::Failed,
        State::Stopped | State::Wrapped => Mood::Idle,
    }
}

/// Returns the colours for the agent's default foreground and background:
/// the painted theme colours when painting, the terminal's otherwise.
fn default_colors(theme: Theme) -> (Color, Color) {
    if theme.painted() {
        (
            rgb(spec(theme.name, Token::Fg).painted),
            rgb(bg(theme.name)),
        )
    } else {
        (Color::Reset, Color::Reset)
    }
}

/// Draws the line shown before the first output, or, for a session that
/// has ended with nothing on screen (one restored from `sessions.json`),
/// how to resume or forget it.
fn draw_message(frame: &mut Frame, inner: Rect, card: &Card, theme: Theme) {
    let (text, token) = match &card.state {
        State::Failed(reason) => (format!("{reason} · r retries · d forgets"), Token::Err),
        State::Stopped => (
            "Stopped. r resumes it · d forgets it".to_owned(),
            Token::FgMuted,
        ),
        State::Wrapped => (
            "Wrapped. r resumes it · d forgets it".to_owned(),
            Token::FgMuted,
        ),
        State::Working | State::YourTurn | State::NeedsYou => {
            ("Warming up the wok…".to_owned(), Token::FgMuted)
        }
    };
    let [row] = Layout::vertical([Constraint::Length(1)])
        .flex(Flex::Center)
        .areas(inner);
    frame.render_widget(
        Line::styled(text, theme.fg(token)).alignment(Alignment::Center),
        row,
    );
}

/// Draws the empty state: the mascot and two lines, centred (DESIGN
/// §5.7). A stopped session gets the same layout with the died mascot.
fn draw_empty(frame: &mut Frame, area: Rect, theme: Theme, tick: usize) {
    let block = pane("[3] output", false, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = [
        Line::styled("Nothing wrapped yet.", theme.fg(Token::Fg)),
        Line::styled("n to start a session", theme.fg(Token::FgMuted)),
    ];
    draw_centred(
        frame,
        inner,
        theme,
        Mood::Empty.pose(tick, theme.animated()),
        &lines,
    );
}

/// Draws the full-size mascot in `pose` with two lines under it, centred
/// in `inner`; the mascot is left out when `inner` is too short.
fn draw_centred(
    frame: &mut Frame,
    inner: Rect,
    theme: Theme,
    pose: mascot::Pose,
    lines: &[Line<'static>],
) {
    let with_mascot = inner.height >= mascot::HEIGHT + 4;
    let height = if with_mascot { mascot::HEIGHT + 3 } else { 2 };
    let [content] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(inner);
    let text_area = if with_mascot {
        let [sprite] = Layout::horizontal([Constraint::Length(mascot::WIDTH)])
            .flex(Flex::Center)
            .areas(content);
        frame.render_widget(
            Mascot {
                theme,
                pose,
                mini: false,
            },
            sprite,
        );
        Rect {
            y: content.y + mascot::HEIGHT + 1,
            height: 2,
            ..content
        }
    } else {
        content
    };
    frame.render_widget(
        Paragraph::new(lines.to_vec()).alignment(Alignment::Center),
        text_area,
    );
}
