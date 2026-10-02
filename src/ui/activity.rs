//! The activity overlay (`A`): what mc and its sessions use of the device
//! (DESIGN §5.5).

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::activity::{Activity, Row};
use crate::ui::sanitise::truncate;
use crate::ui::theme::{Theme, Token};
use crate::ui::{centred, dialog};

/// Width of the label column.
const LABEL: usize = 32;
/// Width of the whole dialog, borders included.
const WIDTH: u16 = 66;
/// Lines of the dialog that are not process rows.
const CHROME: u16 = 11;

/// Formats KiB as `812 MB` or `1.4 GB`.
fn memory(kb: u64) -> String {
    let mb = kb / 1024;
    if mb < 1024 {
        format!("{mb} MB")
    } else {
        format!("{}.{} GB", mb / 1024, mb % 1024 * 10 / 1024)
    }
}

/// Formats tenths of a percent as `12.3%`, or `-` before the second sample.
fn percent(tenths: Option<u64>) -> String {
    tenths.map_or_else(|| "-".into(), |t| format!("{}.{}%", t / 10, t % 10))
}

/// Formats one table line; the label is cut and padded by display width,
/// so wide (CJK, emoji) session names keep the columns aligned.
fn line(label: &str, procs: &str, cpu: &str, ram: &str) -> String {
    let width = |s: &str| Span::raw(s).width();
    let mut label = truncate(label, LABEL);
    while width(&label) > LABEL {
        label.pop();
    }
    let pad = " ".repeat(LABEL - width(&label));
    format!("  {label}{pad} {procs:>5} {cpu:>9} {ram:>10}")
}

/// Draws the activity monitor: one row per group of processes, their
/// total and its share of the device's memory, then the device line.
/// Rows that do not fit are counted in an `… and N more` line; the total
/// always covers every row.
pub(super) fn draw(frame: &mut Frame, area: Rect, activity: &Activity, theme: Theme) {
    let muted = theme.fg(Token::FgMuted);
    let row_line = |r: &Row| {
        line(
            &r.label,
            &r.procs.to_string(),
            &percent(r.cpu_tenths),
            &memory(r.rss_kb),
        )
    };
    let rows = &activity.rows;
    let room = usize::from(area.height.saturating_sub(CHROME)).max(1);
    let shown = if rows.len() > room {
        room - 1
    } else {
        rows.len()
    };
    let mut lines = vec![
        Line::from(""),
        Line::styled(line("", "procs", "cpu", "ram"), theme.fg(Token::Ok)),
    ];
    if rows.is_empty() {
        lines.push(Line::styled("  measuring…", muted));
    }
    lines.extend(
        rows[..shown]
            .iter()
            .map(|r| Line::styled(row_line(r), theme.fg(Token::Fg))),
    );
    if shown < rows.len() {
        lines.push(Line::styled(
            format!("  … and {} more", rows.len() - shown),
            muted,
        ));
    }
    let rss: u64 = rows.iter().map(|r| r.rss_kb).sum();
    let total = Row {
        label: "total".into(),
        procs: rows.iter().map(|r| r.procs).sum(),
        rss_kb: rss,
        cpu_tenths: rows.iter().map(|r| r.cpu_tenths).sum(),
    };
    lines.push(Line::from(""));
    lines.push(Line::styled(row_line(&total), theme.fg(Token::Accent)));
    lines.push(Line::from(""));
    let cores = activity.cores;
    let ram = activity.mem_total_kb.map_or_else(
        || "ram -".into(),
        |all| {
            let tenths = rss * 1000 / all.max(1);
            format!("ram {}, {} used here", memory(all), percent(Some(tenths)))
        },
    );
    let temp = activity
        .temp_c
        .map_or_else(|| "temp n/a".into(), |c| format!("temp {c}°C"));
    lines.push(Line::styled(
        format!("  device  {cores} cores · {ram} · {temp}"),
        theme.fg(Token::Fg),
    ));
    lines.push(Line::styled(
        "  cpu: 100% is one core · updates every 2 s",
        muted,
    ));
    lines.push(Line::from(""));
    lines.push(Line::styled("esc close  ", muted).alignment(Alignment::Right));
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let rect = centred(area, WIDTH, height);
    frame.render_widget(Clear, rect);
    let block = dialog("activity", theme);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_memory_and_percent() {
        assert_eq!(memory(500), "0 MB");
        assert_eq!(memory(812 * 1024), "812 MB");
        assert_eq!(memory(1024 * 1024 + 512 * 1024), "1.5 GB");
        assert_eq!(percent(Some(123)), "12.3%");
        assert_eq!(percent(None), "-");
        let wide = line(&"漢".repeat(40), "1", "-", "1 MB");
        assert_eq!(
            Span::raw(wide).width(),
            Span::raw(line("mc", "1", "-", "1 MB")).width()
        );
    }
}
