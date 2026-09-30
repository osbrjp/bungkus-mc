//! The sessions pane: one card per session of the selected project
//! (DESIGN §5.2, §5.3), or the empty-state copy (§11).

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::agent::Kind;
use crate::agent::usage::tokens;
use crate::app::model::{Focus, Model};
use crate::app::sessions::{Card, State};
use crate::external::External;
use crate::ui::icons::{Icon, IconSet};
use crate::ui::sanitise::truncate;
use crate::ui::theme::{Theme, Token};
use crate::ui::{pane, workspace_label};

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
    let external = project.map_or_else(Vec::new, |p| model.external_in(&p.path));
    if indices.is_empty() && external.is_empty() {
        draw_empty(frame, inner, model, theme);
        return;
    }
    let width = usize::from(inner.width);
    let spin = spinner(model.frame, theme);
    let cards: Vec<Vec<Line>> = indices
        .iter()
        .enumerate()
        .map(|(pos, &i)| {
            let selected = pos == model.card && focused;
            let marker = if selected {
                theme.icons.icon(Icon::Marker).to_string()
            } else {
                " ".to_owned()
            };
            let mut lines = card_lines(
                &model.cards[i],
                &marker,
                selected,
                width,
                spin,
                model.now,
                theme,
            );
            lines.push(Line::from(""));
            lines
        })
        .collect();
    let rows = usize::from(inner.height);
    let mut start = model.card.min(cards.len().saturating_sub(1));
    let mut used = cards.get(start).map_or(0, Vec::len);
    while start > 0 && used + cards[start - 1].len() <= rows {
        start -= 1;
        used += cards[start].len();
    }
    let mut lines: Vec<Line> = cards.into_iter().skip(start).flatten().collect();
    if !external.is_empty() {
        lines.push(Line::styled(
            " outside mc · enter to take over",
            theme.fg(Token::FgMuted),
        ));
        for (pos, ext) in external.into_iter().enumerate() {
            let selected = focused && pos + indices.len() == model.card;
            let place = if project.is_some_and(|p| p.path.as_os_str().is_empty()) {
                crate::store::config::tilde(&ext.cwd, model.home.as_deref())
            } else {
                "in its own terminal".to_owned()
            };
            lines.extend(external_lines(ext, selected, &place, width, spin, theme));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Returns a session running outside mc as two lines: marker, glyph,
/// agent badge, name and state word, then its pid and `place` (its own
/// terminal, or its folder in the elsewhere group).
fn external_lines(
    ext: &External,
    selected: bool,
    place: &str,
    width: usize,
    spin: char,
    theme: Theme,
) -> [Line<'static>; 2] {
    let state = ext.state();
    let (glyph, token, word) = glyph(&state, spin, theme.icons);
    let word = if ext.status.is_none() {
        "running"
    } else {
        word
    };
    let fixed = 2 + 2 + 2 + 1;
    let name = truncate(&ext.name, width.saturating_sub(fixed + word.len() + 1));
    let pad = width.saturating_sub(fixed + name.chars().count() + word.len());
    [
        Line::from(vec![
            Span::styled(
                if selected {
                    format!("{}  ", theme.icons.icon(Icon::Marker))
                } else {
                    "   ".to_owned()
                },
                theme.fg(Token::Ok),
            ),
            Span::styled(glyph.to_string(), theme.fg(token)),
            Span::raw(" "),
            Span::styled(ext.kind.badge().to_string(), theme.fg(Token::Accent)),
            Span::raw(" "),
            Span::styled(name, theme.fg(Token::Fg)),
            Span::raw(" ".repeat(pad)),
            Span::styled(word, theme.fg(Token::FgMuted)),
        ]),
        Line::styled(
            format!(
                "     {}",
                truncate(
                    &format!("pid {} · {place}", ext.pid),
                    width.saturating_sub(5)
                )
            ),
            theme.fg(Token::FgMuted),
        ),
    ]
}

/// Returns the spinner frame for `frame`, or its first frame when motion
/// or colour is off (DESIGN §7).
pub(super) fn spinner(frame: usize, theme: Theme) -> char {
    let frames = theme.icons.spinner();
    if theme.animated() {
        frames[frame % frames.len()]
    } else {
        frames[0]
    }
}

/// Draws the no-sessions / no-projects copy.
fn draw_empty(frame: &mut Frame, inner: Rect, model: &Model, theme: Theme) {
    let workspace = workspace_label(model);
    let text = match (&model.scan_error, model.selected_project()) {
        (Some(error), _) => format!("Can't open {workspace}: {error}. w to pick another folder."),
        (None, Some(p)) => {
            format!("No sessions in {}. n to start one.", p.name)
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
pub(super) fn glyph(state: &State, spin: char, icons: IconSet) -> (char, Token, &'static str) {
    match state {
        State::Working => (spin, Token::Ok, "working"),
        State::YourTurn => (icons.icon(Icon::YourTurn), Token::Info, "your turn"),
        State::NeedsYou => (icons.icon(Icon::NeedsYou), Token::Warn, "needs you"),
        State::Failed(_) => (icons.icon(Icon::Failed), Token::Err, "failed"),
        State::Stopped => (icons.icon(Icon::Stopped), Token::FgMuted, "stopped"),
        State::Wrapped => (icons.icon(Icon::Wrapped), Token::Ok, "wrapped"),
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
    expanded: bool,
    width: usize,
    spin: char,
    now: Instant,
    theme: Theme,
) -> Vec<Line<'static>> {
    let (glyph, glyph_token, word) = glyph(&card.state, spin, theme.icons);
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
    let mut lines = vec![title, body(&detail(card, now), text)];
    if expanded {
        for row in expanded_usage(card) {
            lines.push(body(&row, text));
        }
    } else {
        let (usage, ctx, token) = compact_usage(card);
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(gutter, theme.fg(gutter_token)),
            Span::styled(format!("  {usage}"), text),
            Span::styled(ctx, theme.fg(token)),
        ]));
    }
    let skip = card.subagents.len().saturating_sub(SUBAGENT_ROWS);
    for sub in card.subagents.iter().skip(skip) {
        let mark = if sub.ended.is_some() {
            theme.icons.icon(Icon::Wrapped)
        } else {
            spin
        };
        let time = format!("{}m", minutes(sub.started, sub.ended.unwrap_or(now)));
        let right_len = time.len() + 2;
        let desc = truncate(&sub.description, body_width.saturating_sub(right_len + 2));
        let pad = body_width.saturating_sub(desc.chars().count() + 2 + right_len);
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(gutter, theme.fg(gutter_token)),
            Span::styled(
                format!("  {} ", theme.icons.icon(Icon::Subagent)),
                theme.fg(Token::FgMuted),
            ),
            Span::styled(desc, text),
            Span::raw(" ".repeat(pad)),
            Span::styled(format!("{mark} "), theme.fg(Token::Ok)),
            Span::styled(time, theme.fg(Token::FgMuted)),
        ]));
    }
    lines
}

/// Returns the context fill for display: `None` when unknown or once the
/// session ended (the last report is not a live context, DESIGN §6.2).
fn live_ctx(card: &Card) -> Option<f64> {
    card.usage
        .as_ref()
        .and_then(|u| u.ctx_pct)
        .filter(|_| card.running())
}

/// Returns the compact usage line (DESIGN §6.2) as the text before the
/// context figure, the context figure, and the context colour (`warn`
/// from 80 %, `err` from 90 %).
fn compact_usage(card: &Card) -> (String, String, Token) {
    let u = card.usage.clone().unwrap_or_default();
    let tok = u.tokens().map_or_else(|| "-".to_owned(), tokens);
    let cost = u
        .cost_usd
        .map_or_else(|| "-".to_owned(), |c| format!("${c:.2}"));
    let (ctx, token) = match live_ctx(card) {
        Some(p) if p >= 90.0 => (format!("ctx {p:.0}%"), Token::Err),
        Some(p) if p >= 80.0 => (format!("ctx {p:.0}%"), Token::Warn),
        Some(p) => (format!("ctx {p:.0}%"), Token::Fg),
        None => ("ctx -".to_owned(), Token::Fg),
    };
    (format!("{tok} tok · {cost} · "), ctx, token)
}

/// Returns the expanded usage rows of the selected card (DESIGN §6.3).
fn expanded_usage(card: &Card) -> Vec<String> {
    let u = card.usage.clone().unwrap_or_default();
    let n = |v: Option<u64>| v.map_or_else(|| "-".to_owned(), tokens);
    let mut rows = vec![format!("tokens   in {} · out {}", n(u.input), n(u.output))];
    if card.kind == Kind::Claude {
        rows.push(format!(
            "cache    read {} · write {}",
            n(u.cache_read),
            n(u.cache_write)
        ));
        let cost = u
            .cost_usd
            .map_or_else(|| "-".to_owned(), |c| format!("${c:.2} (list price)"));
        rows.push(format!("cost     {cost}"));
    }
    rows.push(match (live_ctx(card), u.ctx_size) {
        (Some(p), Some(size)) => {
            let used = u.input.map_or_else(|| "-".to_owned(), tokens);
            format!("context  {p:.0}% of {} · {used} used", tokens(size))
        }
        (Some(p), None) => format!("context  {p:.0}%"),
        (None, _) => "context  -".to_owned(),
    });
    if !u.limits.is_empty() {
        let windows: Vec<String> = u
            .limits
            .iter()
            .map(|w| format!("{} {:.0}%", w.label, w.used_pct))
            .collect();
        rows.push(format!("limits   {}", windows.join(" · ")));
    }
    rows
}

/// Returns the state line: state word and detail (DESIGN §5.2).
fn detail(card: &Card, now: Instant) -> String {
    if card.output_only(now) {
        return if card.hooked && card.kind == Kind::Codex {
            "hooks not trusted · /hooks in codex".to_owned()
        } else {
            "output only · live tree unavailable".to_owned()
        };
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
            let subs = card.subagents.len() + card.restored_subagents as usize;
            let plural = if subs == 1 { "" } else { "s" };
            format!("{m}m · {} tools · {subs} subagent{plural}", card.tool_calls)
        }
    }
}
