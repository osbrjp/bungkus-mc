//! The changes popup (`D`): changed files on the left, the highlighted
//! file's diff on the right (DESIGN §5.5).

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Borders, Clear, Paragraph};

use crate::app::diff::{File, Mark, Viewer};
use crate::ui::dialogs::keep_end;
use crate::ui::sanitise::sanitise;
use crate::ui::theme::{Theme, Token};
use crate::ui::{Weight, bold_if, bordered, centred, dialog};

/// Widest the file list gets; it takes a third of a narrower popup.
const LIST_MAX: u16 = 40;

/// Longest file path kept before it is cut to the list's width.
const PATH_MAX: usize = 400;

/// Draws the changes popup over most of the screen: the file list with
/// each file's two status letters (staged in `ok`, unstaged in `err`),
/// scrolled to keep the highlighted one in view, the diff of that file
/// from the scrolled line on behind a rule, and the position
/// (`first line/lines`) before the hint.
pub(super) fn draw(frame: &mut Frame, area: Rect, viewer: &Viewer, theme: Theme) {
    let muted = theme.fg(Token::FgMuted);
    let rect = centred(
        area,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, rect);
    let count = viewer.files.as_ref().map_or(0, Vec::len);
    let title = match count {
        0 => "changes".to_owned(),
        n => format!("changes · {n}"),
    };
    let block = dialog(&title, theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let [body, hint] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let note = match viewer.files.as_deref() {
        None => Some("asking git…"),
        Some([]) => Some("nothing changed (or not a git repository)"),
        Some(_) => None,
    };
    let mut place = String::new();
    if let Some(note) = note {
        let lines = vec![Line::from(""), Line::styled(format!("  {note}"), muted)];
        frame.render_widget(Paragraph::new(lines), body);
    } else {
        let width = (inner.width / 3).min(LIST_MAX);
        let [list, diff] =
            Layout::horizontal([Constraint::Length(width), Constraint::Min(0)]).areas(body);
        draw_files(frame, list, viewer, theme);
        let rule = bordered(Weight::Light, theme)
            .borders(Borders::LEFT)
            .border_style(theme.fg(Token::Border));
        let pane = rule.inner(diff);
        frame.render_widget(rule, diff);
        let lines: Vec<Line> = match &viewer.lines {
            None => vec![Line::styled(" asking git…", muted)],
            Some(lines) if lines.is_empty() => vec![Line::styled(" (no diff)", muted)],
            Some(lines) => {
                place = format!("{}/{} · ", viewer.scroll + 1, lines.len());
                let shown = lines.iter().skip(viewer.scroll).take(pane.height.into());
                let style = |mark| match mark {
                    Mark::Meta => bold_if(muted, true),
                    Mark::Hunk => theme.fg(Token::Info),
                    Mark::Added => theme.fg(Token::Ok),
                    Mark::Removed => theme.fg(Token::Err),
                    Mark::Context => theme.fg(Token::Fg),
                };
                shown
                    .map(|(mark, text)| Line::styled(format!(" {text}"), style(*mark)))
                    .collect()
            }
        };
        frame.render_widget(Paragraph::new(lines), pane);
    }
    frame.render_widget(
        Line::styled(
            format!("{place}j/k file · d/u scroll · r reload · esc close  "),
            muted,
        )
        .alignment(Alignment::Right),
        hint,
    );
}

/// Draws the file list: a `>` on the highlighted row, the status letters,
/// then the path, which keeps its end (the file name) when cut.
fn draw_files(frame: &mut Frame, area: Rect, viewer: &Viewer, theme: Theme) {
    let files = viewer.files.as_deref().unwrap_or_default();
    let rows = usize::from(area.height);
    let room = usize::from(area.width).saturating_sub(6);
    let first = (viewer.selected + 1).saturating_sub(rows);
    let row = |(i, file): (usize, &File)| {
        let chosen = i == viewer.selected;
        let style = if chosen {
            bold_if(theme.fg(Token::Accent), true)
        } else {
            theme.fg(Token::Fg)
        };
        // An untracked file's `??` is all unstaged.
        let staged = if file.staged == '?' {
            Token::Err
        } else {
            Token::Ok
        };
        let path = keep_end(&sanitise(&file.path, PATH_MAX), room);
        Line::from(vec![
            Span::styled(if chosen { " > " } else { "   " }, style),
            Span::styled(file.staged.to_string(), theme.fg(staged)),
            Span::styled(file.unstaged.to_string(), theme.fg(Token::Err)),
            Span::styled(format!(" {path}"), style),
        ])
    };
    let shown = files.iter().enumerate().skip(first).take(rows);
    frame.render_widget(Paragraph::new(shown.map(row).collect::<Vec<_>>()), area);
}
