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
    let band = super::band_height(model.screen) > 1;
    let Some(card) = model.selected_card().map(|i| &model.cards[i]) else {
        draw_empty(frame, area, theme, model.frame, band);
        return;
    };
    if model.is_quick(card) {
        let block = pane("output", false, theme);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let [row] = Layout::vertical([Constraint::Length(1)])
            .flex(Flex::Center)
            .areas(inner);
        frame.render_widget(
            Line::styled(
                "Quick session: enter opens it · m move · p new project",
                theme.fg(Token::FgMuted),
            )
            .alignment(Alignment::Center),
            row,
        );
        return;
    }
    let interact = model.focus == Focus::Output;
    let head = format!("output · {} {} · ", card.kind.badge(), card.id.short());
    let tail = if interact {
        format!(" · INTERACT · {} to leave", model.exit_chord.label())
    } else {
        String::new()
    };
    let room =
        usize::from(area.width).saturating_sub(head.chars().count() + tail.chars().count() + 4);
    let title = format!("{head}{}{tail}", truncate(&card.name, room));
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
            let corner = (!band && inner.width > w + 1 && inner.height > h)
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

/// Draws the quick-session popup (issue #46): a cleared, double-bordered
/// window with the session's live screen and the cursor, over the panes.
pub(super) fn draw_popup(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let Some(card) = model
        .popup
        .and_then(|id| model.cards.iter().find(|c| c.id == id))
    else {
        return;
    };
    let tail = format!(" · {} hide", model.exit_chord.label());
    let head = format!("quick · {} {} · ", card.kind.badge(), card.id.short());
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

/// Draws the line shown before the first output, or why nothing runs.
fn draw_message(frame: &mut Frame, inner: Rect, card: &Card, theme: Theme) {
    let (text, token) = match &card.state {
        State::Failed(reason) => (reason.clone(), Token::Err),
        State::Working | State::YourTurn | State::NeedsYou | State::Stopped | State::Wrapped => {
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
/// §5.7); the mascot is left out when the pane is too short or the band
/// at the top already shows it.
fn draw_empty(frame: &mut Frame, area: Rect, theme: Theme, tick: usize, band: bool) {
    let block = pane("output", false, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let with_mascot = !band && inner.height >= mascot::HEIGHT + 4;
    let height = if with_mascot { mascot::HEIGHT + 3 } else { 2 };
    let [content] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(inner);
    let text_area = if with_mascot {
        let [sprite] = Layout::horizontal([Constraint::Length(mascot::WIDTH)])
            .flex(Flex::Center)
            .areas(content);
        let pose = Mood::Empty.pose(tick, theme.animated());
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
    let lines = vec![
        Line::styled("Nothing wrapped yet.", theme.fg(Token::Fg)),
        Line::styled("n to start a session", theme.fg(Token::FgMuted)),
    ];
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        text_area,
    );
}
