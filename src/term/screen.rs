//! Copies the emulator's visible cells into the ratatui buffer.
//!
//! This mapping is the allowlist of ARCHITECTURE §4.2: only a cell's
//! character (with its combining marks), colours and SGR flags cross over.
//! Hyperlinks and everything else an agent sends stay inside the emulator.

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use crate::term::session::Listener;

/// Draws an emulator's viewport (honouring the scrollback offset).
pub(crate) struct Screen<'a> {
    /// The emulator to draw.
    pub term: &'a Term<Listener>,
    /// Colour for the agent's default foreground.
    pub fg: Color,
    /// Colour for the agent's default background.
    pub bg: Color,
}

impl Widget for Screen<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let grid = self.term.grid();
        let top = -i32::try_from(grid.display_offset()).unwrap_or(0);
        let rows = area
            .height
            .min(u16::try_from(grid.screen_lines()).unwrap_or(u16::MAX));
        let cols = area
            .width
            .min(u16::try_from(grid.columns()).unwrap_or(u16::MAX));
        for y in 0..rows {
            for x in 0..cols {
                let cell = &grid[Point::new(Line(top + i32::from(y)), Column(usize::from(x)))];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let mut symbol = String::new();
                symbol.push(if cell.c.is_control() { ' ' } else { cell.c });
                symbol.extend(
                    cell.zerowidth()
                        .into_iter()
                        .flatten()
                        .filter(|c| !c.is_control()),
                );
                let style = self.style(cell.fg, cell.bg, cell.flags);
                buf[(area.x + x, area.y + y)]
                    .set_symbol(&symbol)
                    .set_style(style);
            }
        }
    }
}

impl Screen<'_> {
    /// Converts a cell's colours and flags into a ratatui style.
    fn style(&self, fg: AColor, bg: AColor, flags: Flags) -> Style {
        let mut style = Style::new()
            .fg(self.color(fg, self.fg))
            .bg(self.color(bg, self.bg));
        for (flag, modifier) in [
            (Flags::BOLD, Modifier::BOLD),
            (Flags::DIM, Modifier::DIM),
            (Flags::ITALIC, Modifier::ITALIC),
            (Flags::UNDERLINE, Modifier::UNDERLINED),
            (Flags::INVERSE, Modifier::REVERSED),
            (Flags::HIDDEN, Modifier::HIDDEN),
            (Flags::STRIKEOUT, Modifier::CROSSED_OUT),
        ] {
            if flags.contains(flag) {
                style = style.add_modifier(modifier);
            }
        }
        style
    }

    /// Maps an emulator colour; the default colours become the pane's.
    fn color(&self, color: AColor, default: Color) -> Color {
        match color {
            AColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
            AColor::Indexed(i) => Color::Indexed(i),
            AColor::Named(NamedColor::Foreground | NamedColor::BrightForeground) => self.fg,
            AColor::Named(NamedColor::Background) => self.bg,
            AColor::Named(n) => u8::try_from(n as usize)
                .ok()
                .filter(|&i| i < 16)
                .map_or(default, Color::Indexed),
        }
    }

    /// Returns where the agent's cursor is inside `area`, when it is shown
    /// and the view is not scrolled back.
    #[must_use]
    pub(crate) fn cursor(&self, area: Rect) -> Option<Position> {
        let grid = self.term.grid();
        let visible =
            self.term.mode().contains(TermMode::SHOW_CURSOR) && grid.display_offset() == 0;
        let point = grid.cursor.point;
        let x = u16::try_from(point.column.0).ok()?;
        let y = u16::try_from(point.line.0).ok()?;
        (visible && x < area.width && y < area.height)
            .then(|| Position::new(area.x + x, area.y + y))
    }

    /// Returns whether every cell in `area` (relative to the screen's top
    /// left) is blank: a space on the default background (DESIGN §5.7).
    #[must_use]
    pub(crate) fn is_blank(&self, area: Rect) -> bool {
        let grid = self.term.grid();
        let top = -i32::try_from(grid.display_offset()).unwrap_or(0);
        (area.y..area.bottom()).all(|y| {
            (area.x..area.right()).all(|x| {
                let (y, x) = (usize::from(y), usize::from(x));
                if y >= grid.screen_lines() || x >= grid.columns() {
                    return true;
                }
                let line = Line(top + i32::try_from(y).unwrap_or(0));
                let cell = &grid[Point::new(line, Column(x))];
                cell.c == ' ' && matches!(cell.bg, AColor::Named(NamedColor::Background))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::vte::ansi::Rgb;

    use super::*;
    use crate::term::session::{Colors, Session, Size};

    fn session(input: &[u8]) -> Session {
        let colors = Colors {
            fg: Rgb::default(),
            bg: Rgb::default(),
        };
        let (mut session, _writes) = Session::detached(Size { cols: 20, rows: 3 }, colors);
        session.advance(input);
        session
    }

    fn draw(session: &Session) -> Buffer {
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        Screen {
            term: session.term(),
            fg: Color::Reset,
            bg: Color::Reset,
        }
        .render(area, &mut buf);
        buf
    }

    #[test]
    fn hostile_sequences_leave_only_printable_cells() {
        let corpus: &[&[u8]] = &[
            b"\x1b]52;c;aGk=\x07",
            b"\x1b]0;title\x07",
            b"\x1b]8;;file:///etc/passwd\x1b\\x\x1b]8;;\x1b\\",
            b"\x1b]1337;File=a\x07",
            b"\x1b]9;notify\x07",
            b"\x1b_Gf=100;AAAA\x1b\\",
            b"\x1bPtmux;\x1b\x1b]0;x\x07\x1b\\",
            b"\x1b[21t",
            b"\x1bP$qm\x1b\\",
            b"\xc2\x9b31m\xc2\x85",
            b"\x1bc",
            b"\x1b[?1049h\x1b[?1049l",
            b"\x07\x08\x7f",
        ];
        for input in corpus {
            let buf = draw(&session(&[b"a", *input, b"b"].concat()));
            for cell in buf.content() {
                assert!(
                    !cell.symbol().chars().any(char::is_control),
                    "{input:?}: {cell:?}"
                );
            }
        }
    }

    #[test]
    fn copies_text_and_sgr_colours() {
        let buf = draw(&session(b"\x1b[1;31mred\x1b[0m \x1b[38;2;1;2;3mrgb"));
        assert_eq!(buf[(0, 0)].symbol(), "r");
        assert_eq!(buf[(0, 0)].fg, Color::Indexed(1));
        assert!(buf[(0, 0)].modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(4, 0)].fg, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn blank_check_sees_any_output() {
        let s = session(b"\r\n                 x");
        let screen = Screen {
            term: s.term(),
            fg: Color::Reset,
            bg: Color::Reset,
        };
        assert!(screen.is_blank(Rect::new(0, 0, 20, 1)));
        assert!(!screen.is_blank(Rect::new(10, 0, 10, 3)));
    }
}
