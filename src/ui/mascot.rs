//! The bungkus mascot: pixel maps and a half-block renderer (DESIGN §5.7).
//!
//! Two pixels share one cell (`▀`/`▄`/`█` with fg/bg colours), so the
//! 16×14 sprite takes 16×7 cells. Transparent pixels leave the cell
//! untouched, so the mascot sits on whatever background the pane has.
//! Brand colours are fixed; only the legs follow the theme.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

use crate::ui::theme::{Theme, ThemeName, rgb};

/// The idle pose, one string per pixel row (DESIGN §5.7 "base rows").
const IDLE: [&str; 14] = [
    "................",
    ".......GG.......",
    "......GGGG......",
    ".....GGGGGG.....",
    "....GEGGGGEG....",
    "....GEGGGGEG....",
    "...TTGGGGGGUU...",
    "..TTTTGGGGUUUU..",
    ".TTTTTTGGUUUUUU.",
    "TTTTTTTTUUUUUUUU",
    "......L..L......",
    "......L..L......",
    ".....LL.LL......",
    "................",
];

/// Width of the full sprite in cells.
pub(crate) const WIDTH: u16 = 16;
/// Height of the full sprite in cells (two pixel rows per cell).
pub(crate) const HEIGHT: u16 = 7;

/// Draws the idle mascot with its top-left corner at the area's origin.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Mascot {
    /// Supplies the legs colour and whether colour is on at all.
    pub theme: Theme,
}

impl Mascot {
    /// Returns the colour of pixel `p`, or `None` when it is transparent.
    fn colour(self, p: u8) -> Option<Color> {
        let legs = match self.theme.name {
            ThemeName::Dark => 0x9a_ab9c,
            ThemeName::Light => 0x0b_120d,
        };
        let hex = match p {
            b'G' => 0x34_ab52,
            b'T' => 0xd9_c574,
            b'U' => 0xe7_d075,
            b'E' => 0x0b_120d,
            b'L' => legs,
            _ => return None,
        };
        Some(if self.theme.no_color() {
            Color::Reset
        } else {
            rgb(hex)
        })
    }
}

impl Widget for Mascot {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (row, pair) in IDLE.chunks(2).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            for (col, (&top, &bottom)) in pair[0]
                .as_bytes()
                .iter()
                .zip(pair[1].as_bytes())
                .enumerate()
            {
                let x = area.x + u16::try_from(col).unwrap_or(u16::MAX);
                if x >= area.right() || y >= area.bottom() {
                    continue;
                }
                let (symbol, style) = match (self.colour(top), self.colour(bottom)) {
                    (None, None) => continue,
                    (Some(t), None) => ("▀", Style::new().fg(t)),
                    (None, Some(b)) => ("▄", Style::new().fg(b)),
                    (Some(t), Some(b)) if t == b => ("█", Style::new().fg(t)),
                    (Some(t), Some(b)) => ("▀", Style::new().fg(t).bg(b)),
                };
                buf[(x, y)].set_symbol(symbol).set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{Background, Profile};

    #[test]
    fn idle_renders_the_documented_shape() {
        let theme = Theme::new(ThemeName::Dark, Profile::NoColor, Background::Paint);
        let mut buf = Buffer::empty(Rect::new(0, 0, WIDTH, HEIGHT));
        Mascot { theme }.render(buf.area, &mut buf);
        let rows: Vec<String> = (0..HEIGHT)
            .map(|y| (0..WIDTH).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect();
        let want = [
            "       ▄▄       ",
            "     ▄████▄     ",
            "    ████████    ",
            "  ▄██████████▄  ",
            "▄██████████████▄",
            "      █  █      ",
            "     ▀▀ ▀▀      ",
        ];
        assert_eq!(rows, want);
    }

    #[test]
    fn eyes_and_legs_take_their_colours() {
        let theme = Theme::new(ThemeName::Dark, Profile::TrueColor, Background::Paint);
        let mut buf = Buffer::empty(Rect::new(0, 0, WIDTH, HEIGHT));
        Mascot { theme }.render(buf.area, &mut buf);
        assert_eq!(buf[(5, 2)].fg, Color::Rgb(0x0b, 0x12, 0x0d), "eye cell");
        assert_eq!(
            buf[(6, 5)].fg,
            Color::Rgb(0x9a, 0xab, 0x9c),
            "dark legs token"
        );
        let light = theme.with_name(ThemeName::Light);
        Mascot { theme: light }.render(buf.area, &mut buf);
        assert_eq!(
            buf[(6, 5)].fg,
            Color::Rgb(0x0b, 0x12, 0x0d),
            "light legs token"
        );
    }
}
