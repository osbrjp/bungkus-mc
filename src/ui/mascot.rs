//! The bungkus mascot: pixel maps, poses, moods and a half-block renderer
//! (DESIGN §5.7).
//!
//! Two pixels share one cell (`▀`/`▄`/`█` with fg/bg colours), so the
//! 16×14 sprite takes 16×7 cells and the 8×6 mini sprite 8×3. Transparent
//! pixels leave the cell untouched, so the mascot sits on whatever
//! background the pane has. Brand colours are fixed; only the legs follow
//! the theme. The died pose draws its eye cells as a bold `x`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use crate::ui::theme::{Theme, ThemeName, rgb};

/// The idle pose, one string per pixel row (DESIGN §5.7 "base rows").
const BASE: [&str; 14] = [
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

/// The mini idle pose (DESIGN §5.7 "Mini sprite").
const MINI: [&str; 6] = [
    "...GG...", "..GGGG..", ".GEGGEG.", "TTTGGUUU", "..L..L..", "........",
];

/// Width of the full sprite in cells.
pub(crate) const WIDTH: u16 = 16;
/// Height of the full sprite in cells (two pixel rows per cell).
pub(crate) const HEIGHT: u16 = 7;
/// Width of the mini sprite in cells.
pub(crate) const MINI_WIDTH: u16 = 8;
/// Height of the mini sprite in cells.
pub(crate) const MINI_HEIGHT: u16 = 3;

/// One frame of the mascot (DESIGN §5.7 "Full frames").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pose {
    /// Resting.
    Idle,
    /// Eyes closed.
    Blink,
    /// Glancing up-left, at the agent's prompt.
    LookL,
    /// Glancing up-right, feet turned right.
    LookR,
    /// Squashed before or after a hop.
    Duck,
    /// In the air.
    Hop,
    /// Walking, left foot forward.
    StepL,
    /// Walking, right foot forward.
    StepR,
    /// Cross eyes: something went wrong.
    Died,
}

/// What the mascot is reacting to (DESIGN §5.7 "Mood follows the state").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mood {
    /// A session waits for the user.
    NeedsYou,
    /// A session is working.
    Working,
    /// A session finished its turn.
    YourTurn,
    /// A session failed, or anything else went wrong.
    Failed,
    /// Nothing to react to.
    Idle,
}

impl Mood {
    /// Returns the pose for animation `frame` (350 ms ticks), or the
    /// static pose when `motion` is off.
    #[must_use]
    pub(crate) fn pose(self, frame: usize, motion: bool) -> Pose {
        use Pose::{Blink, Duck, Hop, Idle, LookL, LookR, StepL, StepR};
        let seq: &[Pose] = match self {
            Self::NeedsYou => &[Duck, Hop, Duck, Idle, LookL, LookL, LookL, Idle],
            Self::Working => &[StepL, Idle, StepR, Idle, StepL, Idle, StepR, LookR],
            Self::YourTurn => &[Idle, LookL, LookL, Idle, LookR, LookR, Idle, Blink],
            Self::Failed => return Pose::Died,
            Self::Idle => return Pose::Idle,
        };
        if motion { seq[frame % seq.len()] } else { Idle }
    }
}

/// Returns the pixel rows of `pose`: the full 14 rows, or the mini 6.
#[must_use]
fn pixels(pose: Pose, mini: bool) -> Vec<String> {
    let rows: Vec<String> = if mini { MINI.iter() } else { BASE.iter() }
        .map(|r| (*r).to_owned())
        .collect();
    let mut rows = rows;
    let mut set = |i: usize, row: &str| row.clone_into(&mut rows[i]);
    if mini {
        match pose {
            Pose::Idle | Pose::Died => {}
            Pose::Blink => set(2, ".GGGGGG."),
            Pose::LookL => {
                set(1, "..EGEG..");
                set(2, ".GGGGGG.");
            }
            Pose::LookR => {
                set(1, "..GEGE..");
                set(2, ".GGGGGG.");
            }
            Pose::Duck => {
                for (i, r) in [
                    "........", "...GG...", "..GGGG..", ".GEGGEG.", "TTTGGUUU", "..L..L..",
                ]
                .iter()
                .enumerate()
                {
                    set(i, r);
                }
            }
            Pose::Hop => {
                for (i, r) in [
                    "..GGGG..", ".GEGGEG.", "TTTGGUUU", "..L..L..", "..L..L..", "........",
                ]
                .iter()
                .enumerate()
                {
                    set(i, r);
                }
            }
            Pose::StepL => set(4, ".L...L.."),
            Pose::StepR => set(4, "..L...L."),
        }
        return rows;
    }
    let body = |shift: isize| -> Vec<String> {
        (0..14)
            .map(|i| {
                let src = isize::try_from(i).unwrap_or(0) - shift;
                if (1..=9).contains(&src) {
                    BASE[usize::try_from(src).unwrap_or(0)].to_owned()
                } else {
                    ".".repeat(16)
                }
            })
            .collect()
    };
    match pose {
        Pose::Idle => {}
        Pose::Blink => {
            set(4, "....GGGGGGGG....");
            set(5, "....GEGGGGEG....");
        }
        Pose::LookL => {
            set(3, ".....EGGGEG.....");
            set(4, "....GEGGGEGG....");
            set(5, "....GGGGGGGG....");
        }
        Pose::LookR => {
            set(3, ".....GEGGGE.....");
            set(4, "....GGEGGGEG....");
            set(5, "....GGGGGGGG....");
            set(12, "......LL.LL.....");
        }
        Pose::Duck => {
            rows = body(1);
            rows[11] = "......L..L......".into();
            rows[12] = ".....LL.LL......".into();
        }
        Pose::Hop => {
            rows = body(-1);
            for row in &mut rows[9..=11] {
                "......L..L......".clone_into(row);
            }
            rows[12] = ".....LL.LL......".into();
        }
        Pose::StepL => {
            set(11, ".....LL..L......");
            set(12, "........LL......");
        }
        Pose::StepR => {
            set(11, "......L.LL......");
            set(12, ".....LL.........");
        }
        Pose::Died => {
            set(4, "....GEGGGGEG....");
            set(5, "....GGEGGEGG....");
        }
    }
    rows
}

/// Draws the mascot with its top-left corner at the area's origin.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Mascot {
    /// Supplies the legs colour and whether colour is on at all.
    pub theme: Theme,
    /// Which frame.
    pub pose: Pose,
    /// The 8×3 mini sprite instead of the full one.
    pub mini: bool,
}

impl Mascot {
    /// The resting full-size mascot.
    #[must_use]
    pub(crate) const fn idle(theme: Theme) -> Self {
        Self {
            theme,
            pose: Pose::Idle,
            mini: false,
        }
    }

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
        let rows = pixels(self.pose, self.mini);
        for (row, pair) in rows.chunks(2).enumerate() {
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
                let eye = top == b'E' || bottom == b'E';
                if self.pose == Pose::Died && eye {
                    let style = Style::new()
                        .fg(self.colour(b'E').unwrap_or(Color::Reset))
                        .bg(self.colour(b'G').unwrap_or(Color::Reset))
                        .add_modifier(Modifier::BOLD);
                    buf[(x, y)].set_symbol("x").set_style(style);
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

    /// Returns the shape of a pose as DESIGN draws it: `█` for two opaque
    /// halves, `▀`/`▄` for one, `x` for died eye cells.
    fn shape(pose: Pose, mini: bool) -> Vec<String> {
        let rows = pixels(pose, mini);
        rows.chunks(2)
            .map(|pair| {
                let line: String = pair[0]
                    .bytes()
                    .zip(pair[1].bytes())
                    .map(|(t, b)| match (t, b) {
                        (b'E', _) | (_, b'E') if pose == Pose::Died => 'x',
                        (b'.', b'.') => ' ',
                        (_, b'.') => '▀',
                        (b'.', _) => '▄',
                        _ => '█',
                    })
                    .collect();
                line.trim_end().to_owned()
            })
            .collect()
    }

    /// Cuts the DESIGN §5.7 rendered-shape blocks into per-pose columns.
    fn design_shapes(title: &str, width: usize, gap: usize, count: usize) -> Vec<Vec<String>> {
        let design = include_str!("../../docs/DESIGN.md");
        let block = design
            .split(title)
            .nth(1)
            .unwrap()
            .split("```")
            .nth(1)
            .unwrap();
        let mut out = vec![Vec::new(); count * 2];
        let mut group = 0;
        for line in block.lines().skip(1) {
            if line.trim().is_empty() {
                group += 1;
                continue;
            }
            if line.starts_with(|c: char| c.is_ascii_alphabetic()) {
                continue;
            }
            let chars: Vec<char> = line.chars().collect();
            for pose in 0..count {
                let start = pose * (width + gap);
                let cell: String = chars.iter().skip(start).take(width).collect();
                out[group * count + pose].push(cell.trim_end().to_owned());
            }
        }
        out
    }

    #[test]
    fn full_frames_equal_the_rendered_shapes_in_design() {
        use Pose::{Died, Duck, Hop, Idle, LookL, LookR, StepL, StepR};
        let design = design_shapes("Rendered shapes (colours omitted):", 16, 2, 4);
        for (i, pose) in [Idle, LookL, LookR, Duck, Hop, StepL, StepR, Died]
            .into_iter()
            .enumerate()
        {
            let mut want = design[i].clone();
            while want.len() < 7 {
                want.insert(0, String::new());
            }
            assert_eq!(shape(pose, false), want, "{pose:?}");
        }
    }

    #[test]
    fn mini_frames_equal_the_rendered_shapes_in_design() {
        use Pose::{Duck, Hop, Idle, LookL, StepL};
        let design = design_shapes("Mini frames, same order:)", 8, 2, 5);
        for (i, pose) in [Idle, LookL, Duck, Hop, StepL].into_iter().enumerate() {
            assert_eq!(shape(pose, true), design[i], "{pose:?}");
        }
    }

    #[test]
    fn frames_have_their_documented_sizes() {
        for pose in [Pose::Idle, Pose::Hop, Pose::Duck, Pose::Died] {
            let full = pixels(pose, false);
            assert!(
                full.len() == 14 && full.iter().all(|r| r.len() == 16),
                "{pose:?}"
            );
            let mini = pixels(pose, true);
            assert!(
                mini.len() == 6 && mini.iter().all(|r| r.len() == 8),
                "{pose:?}"
            );
        }
        let hop = pixels(Pose::Hop, false);
        assert_eq!(hop[0], ".......GG.......", "the hop keeps the tip");
    }

    #[test]
    fn moods_follow_the_documented_sequences_and_stop_without_motion() {
        use Pose::{Blink, Duck, Hop, Idle, LookL, LookR, StepL, StepR};
        let seq = |m: Mood| (0..8).map(|f| m.pose(f, true)).collect::<Vec<_>>();
        assert_eq!(
            seq(Mood::NeedsYou),
            [Duck, Hop, Duck, Idle, LookL, LookL, LookL, Idle]
        );
        assert_eq!(
            seq(Mood::Working),
            [StepL, Idle, StepR, Idle, StepL, Idle, StepR, LookR]
        );
        assert_eq!(
            seq(Mood::YourTurn),
            [Idle, LookL, LookL, Idle, LookR, LookR, Idle, Blink]
        );
        assert_eq!(Mood::Failed.pose(3, true), Pose::Died);
        assert_eq!(Mood::Working.pose(2, false), Pose::Idle);
    }

    #[test]
    fn died_eyes_render_as_bold_x_on_leaf() {
        let theme = Theme::new(ThemeName::Dark, Profile::TrueColor, Background::Paint);
        let mut buf = Buffer::empty(Rect::new(0, 0, WIDTH, HEIGHT));
        Mascot {
            theme,
            pose: Pose::Died,
            mini: false,
        }
        .render(buf.area, &mut buf);
        let eye = &buf[(5, 2)];
        assert_eq!(eye.symbol(), "x");
        assert_eq!(
            (eye.fg, eye.bg),
            (Color::Rgb(0x0b, 0x12, 0x0d), Color::Rgb(0x34, 0xab, 0x52))
        );
        assert!(eye.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn legs_take_the_theme_token() {
        let theme = Theme::new(ThemeName::Dark, Profile::TrueColor, Background::Paint);
        let mut buf = Buffer::empty(Rect::new(0, 0, WIDTH, HEIGHT));
        Mascot::idle(theme).render(buf.area, &mut buf);
        assert_eq!(
            buf[(6, 5)].fg,
            Color::Rgb(0x9a, 0xab, 0x9c),
            "dark legs token"
        );
        Mascot::idle(theme.with_name(ThemeName::Light)).render(buf.area, &mut buf);
        assert_eq!(
            buf[(6, 5)].fg,
            Color::Rgb(0x0b, 0x12, 0x0d),
            "light legs token"
        );
    }
}
