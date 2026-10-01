//! The help overlay, generated from the keymap (DESIGN §5.5).

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::ui::keymap::{self, Scope};
use crate::ui::theme::{Theme, Token};
use crate::ui::{centred, dialog};

/// Width of a key label column.
const LABEL: usize = 21;
/// Width of a help text column.
const HELP: usize = 16;

/// Draws the focused pane's bindings, then the global ones, two per row.
pub(super) fn draw(frame: &mut Frame, area: Rect, scope: Scope, theme: Theme) {
    let pane = match scope {
        Scope::Projects => "projects pane",
        Scope::Sessions => "sessions pane",
        Scope::Global => "everywhere",
    };
    let mut lines = vec![Line::from("")];
    let pane_rows = keymap::help_rows(scope);
    if !pane_rows.is_empty() {
        lines.extend(pairs(&pane_rows, theme));
        lines.push(Line::from(""));
    }
    lines.push(Line::styled("  everywhere", theme.fg(Token::FgMuted)));
    lines.extend(pairs(&keymap::help_rows(Scope::Global), theme));
    lines.push(Line::from(""));
    lines.push(
        Line::styled(
            "press a key to run it · esc close  ",
            theme.fg(Token::FgMuted),
        )
        .alignment(Alignment::Right),
    );
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let width = u16::try_from(2 + 2 * (LABEL + HELP) + 2).unwrap_or(u16::MAX);
    let rect = centred(area, width, height);
    frame.render_widget(Clear, rect);
    let block = dialog(&format!("keys · {pane}"), theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Lays `(label, help)` rows out two per line.
fn pairs(rows: &[(&str, &str)], theme: Theme) -> Vec<Line<'static>> {
    rows.chunks(2)
        .map(|pair| {
            let mut spans = vec![Span::raw("  ")];
            for (label, help) in pair {
                spans.push(Span::styled(
                    format!("{label:<LABEL$}"),
                    theme.fg(Token::Accent),
                ));
                spans.push(Span::styled(format!("{help:<HELP$}"), theme.fg(Token::Fg)));
            }
            Line::from(spans)
        })
        .collect()
}
