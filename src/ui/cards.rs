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

/// How a card stands out in the sessions pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    /// Not the selected card.
    None,
    /// Selected while the sessions pane has focus: the focus marker on the
    /// title line, usage expanded.
    Focused,
    /// Selected while focus is elsewhere: the card the output pane shows,
    /// with a bar down its left edge and its name in `accent`.
    Shown,
}

/// Draws the sessions pane.
pub(super) fn draw(frame: &mut Frame, area: Rect, model: &Model, theme: Theme) {
    let project = model.selected_project();
    let title = project.map_or_else(
        || "[2] sessions".to_owned(),
        |p| format!("[2] sessions · {}", p.name),
    );
    let focused = model.focus == Focus::Sessions;
    let block = pane(&title, focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = rows(inner, model, theme);
    if lines.is_empty() {
        draw_empty(frame, inner, model, theme);
        return;
    }
    // The selected block: a card without its blank line, or an outside
    // session's two lines.
    let chosen = |(_, row): &(Line, Option<usize>)| *row == Some(model.card);
    let top = lines.iter().position(chosen).unwrap_or(0);
    let height = lines.iter().filter(|line| chosen(line)).count();
    let lines: Vec<Line> = lines.into_iter().map(|(line, _)| line).collect();
    frame.render_widget(Paragraph::new(lines), inner);
    crate::ui::shade(frame, inner, top, height, theme);
}

/// Returns the row of the sessions pane (a card, or an outside session
/// after the cards: what [`Model::card`] counts) drawn at screen row
/// `row` of pane `area`, for mouse hit tests.
///
/// # Returns
///
/// `None` on the border, between cards, on the "outside" heading and
/// below the list.
#[must_use]
pub(crate) fn session_at(area: Rect, model: &Model, row: u16) -> Option<usize> {
    let inner = pane("", false, model.theme).inner(area);
    let line = row.checked_sub(inner.y).filter(|l| *l < inner.height)?;
    let lines = rows(inner, model, model.view_theme());
    lines.get(usize::from(line)).and_then(|(_, row)| *row)
}

/// Returns the lines of the sessions pane inside `inner`, top one first,
/// each with the row it belongs to (the blank line under a card and the
/// "outside" heading belong to none). The list starts at the first card
/// that still lets the selected one fit. Empty when the project has no
/// session.
fn rows(inner: Rect, model: &Model, theme: Theme) -> Vec<(Line<'static>, Option<usize>)> {
    let project = model.selected_project();
    let focused = model.focus == Focus::Sessions;
    let indices = model.project_cards();
    let external = project.map_or_else(Vec::new, |p| model.external_in(&p.path));
    let width = usize::from(inner.width);
    let spin = spinner(model.frame, theme);
    let cards: Vec<Vec<Line>> = indices
        .iter()
        .enumerate()
        .map(|(pos, &i)| {
            let mark = match (pos == model.card, focused) {
                (false, _) => Mark::None,
                (true, true) => Mark::Focused,
                (true, false) => Mark::Shown,
            };
            let card = &model.cards[i];
            let folder = card.folder();
            let around = (
                model.repos.get(&folder),
                model.links.get(&folder),
                model.has_shell(card.id),
            );
            let mut lines = card_lines(card, around, mark, width, spin, model.now, theme);
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
    let mut lines: Vec<(Line, Option<usize>)> = cards
        .into_iter()
        .enumerate()
        .skip(start)
        .flat_map(|(pos, card)| {
            let last = card.len() - 1;
            card.into_iter()
                .enumerate()
                .map(move |(n, line)| (line, (n < last).then_some(pos)))
        })
        .collect();
    if !external.is_empty() {
        lines.push((
            Line::styled(
                " outside · enter take over · x stop",
                theme.fg(Token::FgMuted),
            ),
            None,
        ));
        for (pos, ext) in external.into_iter().enumerate() {
            let selected = focused && pos + indices.len() == model.card;
            let place = if project.is_some_and(|p| p.path.as_os_str().is_empty()) {
                crate::store::config::tilde(&ext.cwd, model.home.as_deref())
            } else {
                "in its own terminal".to_owned()
            };
            let row = Some(pos + indices.len());
            let ext = external_lines(ext, selected, &place, width, spin, theme);
            lines.extend(ext.map(|line| (line, row)));
        }
    }
    lines
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
            Span::styled(
                theme.icons.agent(ext.kind).to_string(),
                theme.agent_style(ext.kind),
            ),
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
/// the git line when the session's folder is in a repository, the compact
/// usage line (`-` until M5), then one line per subagent.
///
/// Column 0 carries `mark`: the focus marker on the title line, or the
/// same bar the projects pane uses for its selected row on every line.
/// `repo` is the state of the session's folder, `linked` the issue and
/// pull request of its branch (their numbers end the git line) and `shell` whether the
/// session has a shell in the terminal pane: `>_` before the title
/// line's right-hand word.
fn card_lines(
    card: &Card,
    (repo, linked, shell): (
        Option<&crate::app::repo::Status>,
        Option<&crate::app::links::Links>,
        bool,
    ),
    mark: Mark,
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
    let shell_mark = if shell { ">_ " } else { "" };
    let fixed = 2 + 2 + 2 + id.len() + 1 + shell_mark.len();
    let name = truncate(&card.name, width.saturating_sub(fixed + right.len() + 1));
    let pad = width.saturating_sub(fixed + name.chars().count() + right.len());
    let title_style = match (mark, muted) {
        (Mark::Shown, _) => theme.fg(Token::Accent).add_modifier(Modifier::BOLD),
        (Mark::None | Mark::Focused, true) => text,
        (Mark::None | Mark::Focused, false) => text.add_modifier(Modifier::BOLD),
    };
    let edge = match mark {
        Mark::None | Mark::Focused => Span::raw(" "),
        Mark::Shown if theme.icons == IconSet::Ascii => Span::styled(":", theme.fg(Token::Accent)),
        Mark::Shown => Span::styled("▌", theme.fg(Token::Accent)),
    };
    let title = Line::from(vec![
        match mark {
            Mark::Focused => Span::styled(
                theme.icons.icon(Icon::Marker).to_string(),
                theme.fg(Token::Ok),
            ),
            Mark::None | Mark::Shown => edge.clone(),
        },
        Span::styled(gutter, theme.fg(gutter_token)),
        Span::styled(glyph.to_string(), theme.fg(glyph_token)),
        Span::raw(" "),
        Span::styled(
            theme.icons.agent(card.kind).to_string(),
            theme.agent_style(card.kind),
        ),
        Span::raw(" "),
        Span::styled(id, theme.fg(Token::Info)),
        Span::raw(" "),
        Span::styled(name, title_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(shell_mark, theme.fg(Token::Ok)),
        Span::styled(right, theme.fg(Token::FgMuted)),
    ]);
    let body_width = width.saturating_sub(4);
    let body = |s: &str, style: Style| {
        Line::from(vec![
            edge.clone(),
            Span::styled(gutter, theme.fg(gutter_token)),
            Span::styled(format!("  {}", truncate(s, body_width)), style),
        ])
    };
    let mut lines = vec![title, body(&detail(card, now), text)];
    if let Some(repo) = repo {
        let tags = linked.map(crate::app::links::Links::label);
        let label = match tags.filter(|tags| !tags.is_empty()) {
            Some(tags) => {
                let room = body_width.saturating_sub(tags.chars().count() + 3);
                format!("{} · {tags}", repo.label(room))
            }
            None => repo.label(body_width),
        };
        lines.push(body(&label, theme.fg(Token::FgMuted)));
    }
    if !card.mcp.is_empty() && mark != Mark::Focused {
        let line = mcp_line(&card.mcp, theme.icons);
        lines.push(body(&line, theme.fg(Token::FgMuted)));
    }
    let lead = [
        edge.clone(),
        Span::styled(gutter, theme.fg(gutter_token)),
        "  ".into(),
    ];
    let rows = usage_rows(card, mark == Mark::Focused, body_width, text, theme);
    let prefixed = |row: Vec<Span<'static>>| lead.iter().cloned().chain(row).collect();
    lines.extend(rows.into_iter().map(prefixed));
    let skip = card.subagents.len().saturating_sub(SUBAGENT_ROWS);
    for sub in card.subagents.iter().skip(skip) {
        let mut spans = vec![edge.clone(), Span::styled(gutter, theme.fg(gutter_token))];
        spans.extend(subagent_spans(sub, body_width, spin, now, text, theme));
        lines.push(Line::from(spans));
    }
    lines
}

/// Returns a subagent row after the card's two edge columns: glyph,
/// description, then its state glyph and time at the right of `width`.
fn subagent_spans(
    sub: &crate::app::sessions::Subagent,
    width: usize,
    spin: char,
    now: Instant,
    text: Style,
    theme: Theme,
) -> [Span<'static>; 5] {
    let mark = if sub.ended.is_some() {
        theme.icons.icon(Icon::Wrapped)
    } else {
        spin
    };
    let time = format!("{}m", minutes(sub.started, sub.ended.unwrap_or(now)));
    let right_len = time.len() + 2;
    let desc = truncate(&sub.description, width.saturating_sub(right_len + 2));
    let pad = width.saturating_sub(desc.chars().count() + 2 + right_len);
    [
        Span::styled(
            format!("  {} ", theme.icons.icon(Icon::Subagent)),
            theme.fg(Token::FgMuted),
        ),
        Span::styled(desc, text),
        Span::raw(" ".repeat(pad)),
        Span::styled(format!("{mark} "), theme.fg(Token::Ok)),
        Span::styled(time, theme.fg(Token::FgMuted)),
    ]
}

/// Returns how each of `servers` shows in `icons` ([`IconSet::mcp`]), once
/// per label: servers of one brand share one glyph.
fn mcp_glyphs(servers: &[&str], icons: IconSet) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    for label in servers.iter().map(|s| icons.mcp(s)) {
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels
}

/// Returns the compact MCP line (DESIGN §5.2): glyphs in the `nerd` set,
/// `mcp` and the names otherwise. Servers of one brand share one glyph.
fn mcp_line(servers: &[String], icons: IconSet) -> String {
    let servers: Vec<&str> = servers.iter().map(String::as_str).collect();
    let labels = mcp_glyphs(&servers, icons);
    match icons {
        IconSet::Nerd => labels.join("  "),
        IconSet::Ascii | IconSet::Unicode => format!("mcp {}", labels.join(" · ")),
    }
}

/// Returns the usage rows of a card: the expanded rows when it is the
/// focused one, else the one compact line with its context figure in the
/// threshold colour.
fn usage_rows(
    card: &Card,
    expanded: bool,
    width: usize,
    text: Style,
    theme: Theme,
) -> Vec<Vec<Span<'static>>> {
    if expanded {
        return expanded_usage(card, width, text, theme);
    }
    let (usage, ctx, token) = compact_usage(card);
    vec![vec![
        Span::styled(usage, text),
        Span::styled(ctx, theme.fg(token)),
    ]]
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

/// Cells of the bars on the selected card: one per 10 %.
const BAR: u8 = 10;

/// Returns a bar row of the selected card: `label`, a [`crate::ui::bar`]
/// of [`BAR`] cells, the percentage and (when `width` has room) `detail`,
/// as `context  ██░░░░░░░░ 23% 45k/200k`.
///
/// The filled part takes `fill` (the row's threshold colour) and the rest
/// `fg-muted`; the shapes differ too, so it reads as a bar without colour.
fn bar_row(
    label: &str,
    pct: f64,
    detail: &str,
    fill: Token,
    (width, text): (usize, Style),
    theme: Theme,
) -> Vec<Span<'static>> {
    let (on, off) = crate::ui::bar(pct, BAR, theme.utf8);
    let mut tail = format!(" {pct:.0}%");
    let used = label.chars().count() + usize::from(BAR) + tail.len();
    if used + detail.chars().count() <= width {
        tail.push_str(detail);
    }
    vec![
        Span::styled(label.to_owned(), text),
        Span::styled(on, theme.fg(fill)),
        Span::styled(off, theme.fg(Token::FgMuted)),
        Span::styled(tail, text),
    ]
}

/// Returns the expanded usage rows of the selected card (DESIGN §6.3),
/// each cut to `width` and in `text` style. The context (while the
/// session runs) is a [`bar_row`]: `warn` from 80 % and `err` from 90 %,
/// as in the compact line. Plan limits are in the getah bar only.
fn expanded_usage(card: &Card, width: usize, text: Style, theme: Theme) -> Vec<Vec<Span<'static>>> {
    let u = card.usage.clone().unwrap_or_default();
    let n = |v: Option<u64>| v.map_or_else(|| "-".to_owned(), tokens);
    let plain = |s: String| vec![Span::styled(truncate(&s, width), text)];
    let level = |pct: f64| match pct {
        p if p >= 90.0 => Token::Err,
        p if p >= 80.0 => Token::Warn,
        _ => Token::Ok,
    };
    let mut rows = vec![plain(format!(
        "tokens   in {} · out {}",
        n(u.input),
        n(u.output)
    ))];
    if card.kind == Kind::Claude {
        rows.push(plain(format!(
            "cache    read {} · write {}",
            n(u.cache_read),
            n(u.cache_write)
        )));
        let cost = u
            .cost_usd
            .map_or_else(|| "-".to_owned(), |c| format!("${c:.2} (list price)"));
        rows.push(plain(format!("cost     {cost}")));
    }
    rows.push(match live_ctx(card) {
        Some(pct) => {
            let detail = match (u.input, u.ctx_size) {
                (Some(used), Some(size)) => format!(" {}/{}", tokens(used), tokens(size)),
                (None, Some(size)) => format!(" of {}", tokens(size)),
                (_, None) => String::new(),
            };
            bar_row("context  ", pct, &detail, level(pct), (width, text), theme)
        }
        None => plain("context  -".to_owned()),
    });
    if !card.mcp.is_empty() {
        let servers: Vec<&str> = card.mcp.iter().map(String::as_str).collect();
        let servers = match theme.icons {
            IconSet::Nerd => mcp_glyphs(&servers, theme.icons).join("  "),
            IconSet::Ascii | IconSet::Unicode => servers.join(" · "),
        };
        rows.push(plain(format!("mcp      {servers}")));
    }
    rows
}

/// Returns the state line: state word and detail (DESIGN §5.2).
fn detail(card: &Card, now: Instant) -> String {
    if card.cloud && !matches!(card.state, State::Failed(_)) {
        return match &card.cloud_session {
            _ if card.running() => "cloud · starting".to_owned(),
            Some(id) => format!("cloud · {id}"),
            None => "cloud · r lists cloud sessions".to_owned(),
        };
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{Background, Profile, ThemeName};

    #[test]
    fn bar_rows_keep_the_detail_only_when_it_fits_and_colour_the_fill() {
        let theme = Theme::new(ThemeName::Dark, Profile::NoColor, Background::Paint);
        let row = |pct, width, theme| -> String {
            let args = (width, Style::default());
            bar_row("context  ", pct, " 45k/200k", Token::Ok, args, theme)
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        };
        assert_eq!(row(23.0, 36, theme), "context  ██░░░░░░░░ 23% 45k/200k");
        assert_eq!(row(23.0, 24, theme), "context  ██░░░░░░░░ 23%", "narrow");
        let no_utf8 = theme.with_view(IconSet::Ascii, false, true);
        assert_eq!(row(23.0, 24, no_utf8), "context  ##-------- 23%");
        let args = (36, Style::default());
        let spans = bar_row("context  ", 96.0, "", Token::Err, args, theme);
        assert_eq!(spans[1].style, theme.fg(Token::Err));
        assert_eq!(spans[2].style, theme.fg(Token::FgMuted));
    }

    #[test]
    fn a_card_in_a_repository_gets_the_git_line_below_its_state() {
        let theme = Theme::new(ThemeName::Dark, Profile::NoColor, Background::Paint);
        let now = Instant::now();
        let card = Card::new(
            crate::term::SessionId::new(),
            Kind::Claude,
            "/p".into(),
            Some("s"),
            None,
            now,
        );
        let repo = crate::app::repo::Status::parse("# branch.head main\n? x\n").unwrap();
        let text = |repo| -> Vec<String> {
            card_lines(&card, (repo, None, false), Mark::None, 36, '*', now, theme)
                .iter()
                .map(ToString::to_string)
                .collect()
        };
        assert_eq!(text(None).len(), 3);
        assert!(!text(None)[0].contains(">_"), "no shell: no mark");
        let with_shell = card_lines(&card, (None, None, true), Mark::None, 36, '*', now, theme);
        let title = with_shell[0].to_string();
        assert!(title.ends_with(" >_ 0m") && title.chars().count() == 36);
        let lines = text(Some(&repo));
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[2].trim(), "main · 1 changed");
    }

    #[test]
    fn a_card_lists_its_mcp_servers_as_glyphs_or_names() {
        let theme = Theme::new(ThemeName::Dark, Profile::NoColor, Background::Paint);
        let now = Instant::now();
        let mut card = Card::new(
            crate::term::SessionId::new(),
            Kind::Claude,
            "/p".into(),
            Some("s"),
            None,
            now,
        );
        card.mcp = vec!["github".into(), "miko".into()];
        let line = |theme, mark, n: usize| {
            card_lines(&card, (None, None, false), mark, 36, '*', now, theme)[n]
                .to_string()
                .trim()
                .to_owned()
        };
        assert_eq!(line(theme, Mark::None, 2), "mcp github · miko");
        let nerd = theme.with_view(IconSet::Nerd, true, true);
        assert_eq!(line(nerd, Mark::None, 2), "\u{f09b}  \u{f1e6} miko");
        assert_eq!(
            line(nerd, Mark::Focused, 6),
            "mcp      \u{f09b}  \u{f1e6} miko"
        );
        assert_eq!(line(theme, Mark::Focused, 6), "mcp      github · miko");
        assert!(line(nerd, Mark::None, 0).contains("\u{ec82} "), "the logo");
        assert!(line(theme, Mark::None, 0).contains(" C "), "the letter");
        card.mcp.clear();
        assert_eq!(
            card_lines(&card, (None, None, false), Mark::None, 36, '*', now, theme).len(),
            3
        );
    }
}
