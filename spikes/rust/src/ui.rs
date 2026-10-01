//! Interactive mode: a session list on the left, the selected agent's live
//! screen on the right (SPEC.md §1).
//!
//! This module owns the host terminal (raw mode, alt screen, mouse, paste)
//! and restores it on exit. It never parses PTY output; that happens on the
//! pump threads in [`crate::session`].

use std::{io, path::Path, time::Duration};

use alacritty_terminal::{
    Term,
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point},
    term::{TermMode, cell::Flags},
    vte::ansi::Color as AColor,
};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::supports_keyboard_enhancement,
};
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, BorderType, List, ListState, Widget},
};

use crate::{
    keys,
    session::{Replier, Session, Size},
};

/// Width of the session list, per SPEC.md §1.
const LIST_WIDTH: u16 = 24;

/// Frame interval; the loop redraws at most this often.
const TICK: Duration = Duration::from_millis(16);

/// Lines scrolled per mouse wheel notch.
const WHEEL_LINES: i32 = 3;

/// Which pane receives keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Agent,
}

/// A list entry: the agent's name and its session, or why it failed to start.
struct Entry {
    name: &'static str,
    session: Result<Session>,
}

/// Runs the TUI until the user presses `q` on the list.
///
/// # Arguments
///
/// * `cwd` - Directory both agents start in.
///
/// # Errors
///
/// Fails if the host terminal cannot be set up or read.
pub fn run(cwd: &Path) -> Result<()> {
    let mut terminal = ratatui::init();
    let kitty = supports_keyboard_enhancement().unwrap_or(false);
    execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    if kitty {
        execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    let result = event_loop(&mut terminal, cwd);
    if kitty {
        execute!(io::stdout(), PopKeyboardEnhancementFlags)?;
    }
    execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste)?;
    ratatui::restore();
    result
}

/// Returns the agent pane's inner size for a host terminal of `area`.
fn pane_size(area: Rect) -> Size {
    Size {
        cols: area.width.saturating_sub(LIST_WIDTH + 2).max(1),
        rows: area.height.saturating_sub(2).max(1),
    }
}

/// Polls input, forwards it and redraws until quit.
///
/// # Errors
///
/// Fails if drawing or reading host terminal events fails.
fn event_loop(terminal: &mut DefaultTerminal, cwd: &Path) -> Result<()> {
    let mut size = pane_size(terminal.size()?.into());
    let mut entries: Vec<Entry> = ["claude", "codex"]
        .into_iter()
        .map(|name| Entry {
            name,
            session: Session::spawn(&[name.to_owned()], cwd, &[], size),
        })
        .collect();
    let mut list = ListState::default().with_selected(Some(0));
    let mut focus = Focus::List;

    loop {
        terminal.draw(|f| draw(f, &mut entries, &mut list, focus))?;
        if !event::poll(TICK)? {
            continue;
        }
        let selected = list.selected().unwrap_or(0);
        let session = entries.get(selected).and_then(|e| e.session.as_ref().ok());
        match event::read()? {
            Event::Resize(w, h) => {
                size = pane_size(Rect::new(0, 0, w, h));
                for s in entries.iter().filter_map(|e| e.session.as_ref().ok()) {
                    // reason: a child that already exited cannot be resized; nothing to do.
                    let _ = s.resize(size);
                }
            }
            Event::Paste(text) if focus == Focus::Agent => {
                if let Some(s) = session {
                    s.write(&keys::paste(&text, s.mode())).ok();
                }
            }
            Event::Mouse(m) if m.column >= LIST_WIDTH => {
                let delta = match m.kind {
                    MouseEventKind::ScrollUp => WHEEL_LINES,
                    MouseEventKind::ScrollDown => -WHEEL_LINES,
                    _ => continue,
                };
                if let Some(s) = session {
                    s.term().scroll_display(Scroll::Delta(delta));
                }
            }
            Event::Key(k) if k.kind != KeyEventKind::Release => match focus {
                Focus::List => match k.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('j') | KeyCode::Down => list.select_next(),
                    KeyCode::Char('k') | KeyCode::Up => list.select_previous(),
                    KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter | KeyCode::Tab => {
                        focus = Focus::Agent;
                    }
                    _ => {}
                },
                Focus::Agent if is_leave_key(&k) => focus = Focus::List,
                Focus::Agent => {
                    if let Some(s) = session {
                        s.term().scroll_display(Scroll::Bottom);
                        s.write(&keys::encode(&k, s.mode())).ok();
                    }
                }
            },
            _ => {}
        }
    }
    for s in entries.iter_mut().filter_map(|e| e.session.as_mut().ok()) {
        s.kill();
    }
    Ok(())
}

/// Reports whether `k` is `ctrl+\`, which returns focus to the list.
///
/// Legacy terminals deliver 0x1c, which crossterm reports as `ctrl+4`.
fn is_leave_key(k: &KeyEvent) -> bool {
    k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('\\' | '4'))
}

/// Draws the list and the selected agent's screen.
fn draw(f: &mut Frame<'_>, entries: &mut [Entry], list: &mut ListState, focus: Focus) {
    let [left, right] =
        Layout::horizontal([Constraint::Length(LIST_WIDTH), Constraint::Fill(1)]).areas(f.area());
    let items: Vec<String> = entries
        .iter_mut()
        .map(|e| match &mut e.session {
            Ok(s) => {
                if s.exited() {
                    format!("{} [exited]", e.name)
                } else {
                    e.name.to_owned()
                }
            }
            Err(_) => format!("{} [failed]", e.name),
        })
        .collect();
    let list_block = Block::bordered()
        .title(" sessions ")
        .title_bottom(" q quit ")
        .border_type(if focus == Focus::List {
            BorderType::Double
        } else {
            BorderType::Plain
        });
    let widget = List::new(items)
        .block(list_block)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(widget, left, list);

    let entry = list.selected().and_then(|i| entries.get(i));
    let title = entry.map_or("", |e| e.name);
    let hint = if focus == Focus::Agent {
        " ctrl-\\ back "
    } else {
        " enter focus "
    };
    let block = Block::bordered()
        .title(format!(" {title} "))
        .title_bottom(hint)
        .border_type(if focus == Focus::Agent {
            BorderType::Double
        } else {
            BorderType::Plain
        });
    let inner = block.inner(right);
    f.render_widget(block, right);
    match entry.map(|e| &e.session) {
        Some(Ok(s)) => {
            let term = s.term();
            f.render_widget(Screen(&term), inner);
            let cursor = term.grid().cursor.point;
            let visible =
                term.mode().contains(TermMode::SHOW_CURSOR) && term.grid().display_offset() == 0;
            if focus == Focus::Agent && visible {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                // reason: the cursor is on screen, so line >= 0 and both fit the u16 pane.
                f.set_cursor_position(Position::new(
                    inner.x + cursor.column.0 as u16,
                    inner.y + cursor.line.0 as u16,
                ));
            }
        }
        Some(Err(e)) => f.render_widget(format!("failed to start: {e:#}"), inner),
        None => {}
    }
}

/// Widget that paints the emulator's viewport (honouring scrollback offset).
struct Screen<'a>(&'a Term<Replier>);

impl Widget for Screen<'_> {
    /// Copies each visible cell's text and style into the ratatui buffer.
    ///
    /// Wide-char spacer cells are skipped: ratatui already skips the column
    /// after a double-width symbol.
    fn render(self, area: Rect, buf: &mut Buffer) {
        let grid = self.0.grid();
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        // reason: offsets and sizes come from u16 pane sizes and the 10k scrollback cap.
        let top = -(grid.display_offset() as i32);
        let rows = area
            .height
            .min(u16::try_from(grid.screen_lines()).unwrap_or(u16::MAX));
        let cols = area
            .width
            .min(u16::try_from(grid.columns()).unwrap_or(u16::MAX));
        for y in 0..rows {
            for x in 0..cols {
                let cell = &grid[Point::new(Line(top + i32::from(y)), Column(usize::from(x)))];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                let mut sym = String::from(cell.c);
                sym.extend(cell.zerowidth().into_iter().flatten());
                let out = &mut buf[(area.x + x, area.y + y)];
                out.set_symbol(&sym)
                    .set_style(style(cell.fg, cell.bg, cell.flags));
            }
        }
    }
}

/// Converts an emulator cell's colours and flags into a ratatui style.
fn style(fg: AColor, bg: AColor, flags: Flags) -> Style {
    let mut s = Style::new().fg(color(fg)).bg(color(bg));
    for (flag, m) in [
        (Flags::BOLD, Modifier::BOLD),
        (Flags::DIM, Modifier::DIM),
        (Flags::ITALIC, Modifier::ITALIC),
        (Flags::UNDERLINE, Modifier::UNDERLINED),
        (Flags::INVERSE, Modifier::REVERSED),
        (Flags::HIDDEN, Modifier::HIDDEN),
        (Flags::STRIKEOUT, Modifier::CROSSED_OUT),
    ] {
        if flags.contains(flag) {
            s = s.add_modifier(m);
        }
    }
    s
}

/// Maps an emulator colour to a ratatui colour; defaults become `Reset`.
fn color(c: AColor) -> Color {
    match c {
        AColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        AColor::Indexed(i) => Color::Indexed(i),
        AColor::Named(n) => u8::try_from(n as usize)
            .ok()
            .filter(|&i| i < 16)
            .map_or(Color::Reset, Color::Indexed),
    }
}
