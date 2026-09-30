//! The Daun Pisang palette as a token spec, and colour-profile detection.
//!
//! [`spec`] mirrors the "Daun Teduh" tables of DESIGN §2.1, which are shared
//! with bungkus-cli's `styles.go`; a test parses DESIGN.md so the two cannot
//! drift. Colours are never downsampled automatically: 256- and 16-colour
//! terminals get the declared indices (DESIGN §2.3). The background is
//! painted only at TrueColor.

use ratatui::style::{Color, Modifier, Style};

/// A named colour role (DESIGN §2.1).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the full token spec is shared with bungkus-cli; state colours are used from M4"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Token {
    /// Primary text.
    Fg,
    /// Hints, timestamps, secondary text.
    FgMuted,
    /// Rules, inactive decoration.
    FgDim,
    /// Inactive pane border.
    Border,
    /// App title, selected row text, agent badge.
    Accent,
    /// Running, wrapped, focused border.
    Ok,
    /// Needs you, INTERACT mode word.
    Warn,
    /// Failed.
    Err,
    /// Session ids, links, "your turn".
    Info,
}

/// The declared values of one [`Token`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TokenSpec {
    /// Hex used when the background is painted.
    pub painted: u32,
    /// Hex used at TrueColor on the terminal's own background; `None` is
    /// the terminal's default colour.
    pub fallback: Option<u32>,
    /// Declared xterm-256 index.
    pub ansi256: u8,
    /// Declared 16-colour index.
    pub ansi16: u8,
}

/// The painted screen background, "Daun teduh".
pub(crate) const BG: u32 = 0x1c_2a21;

/// Returns the declared values of `token` (DESIGN §2.1, dark).
#[must_use]
pub(crate) const fn spec(token: Token) -> TokenSpec {
    const fn same(hex: u32, ansi256: u8, ansi16: u8) -> TokenSpec {
        TokenSpec {
            painted: hex,
            fallback: Some(hex),
            ansi256,
            ansi16,
        }
    }
    match token {
        Token::Fg => TokenSpec {
            painted: 0xd6_e2d3,
            fallback: None,
            ansi256: 253,
            ansi16: 15,
        },
        Token::FgMuted => TokenSpec {
            painted: 0x9a_ab9c,
            fallback: Some(0x8a_99a8),
            ansi256: 245,
            ansi16: 7,
        },
        Token::FgDim => TokenSpec {
            painted: 0x5c_7062,
            fallback: Some(0x55_5555),
            ansi256: 241,
            ansi16: 8,
        },
        Token::Border => TokenSpec {
            painted: 0x3b_4d40,
            fallback: Some(0x2a_2a2a),
            ansi256: 238,
            ansi16: 8,
        },
        Token::Accent => same(0xff_aa88, 216, 3),
        Token::Ok => same(0x7f_b069, 107, 2),
        Token::Warn => same(0xe8_c547, 185, 11),
        Token::Err => same(0xff_7361, 209, 9),
        Token::Info => same(0x6f_8fc7, 68, 12),
    }
}

/// How many colours the host terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Profile {
    /// `NO_COLOR`: glyphs, words and border weight only.
    NoColor,
    /// The 16 ANSI colours.
    Ansi16,
    /// The xterm 256-colour palette.
    Ansi256,
    /// 24-bit colour.
    TrueColor,
}

/// `TERM_PROGRAM` values known to support TrueColor (DESIGN §2.3).
const TRUECOLOR_PROGRAMS: [&str; 4] = ["iTerm.app", "WezTerm", "ghostty", "vscode"];

impl Profile {
    /// Detects the colour profile from environment variables (DESIGN §2.3).
    ///
    /// `NO_COLOR` (non-empty) wins; then `COLORTERM` of `truecolor`/`24bit`,
    /// a known TrueColor `TERM_PROGRAM` or a `TERM` ending in `-direct` mean
    /// TrueColor; a `TERM` containing `256color` means 256; else 16.
    ///
    /// # Arguments
    ///
    /// * `var` - Looks up an environment variable by name.
    #[must_use]
    pub(crate) fn detect(var: impl Fn(&str) -> Option<String>) -> Self {
        if var("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Self::NoColor;
        }
        let term = var("TERM").unwrap_or_default();
        let truecolor = matches!(var("COLORTERM").as_deref(), Some("truecolor" | "24bit"))
            || var("TERM_PROGRAM").is_some_and(|p| TRUECOLOR_PROGRAMS.contains(&p.as_str()))
            || term.ends_with("-direct");
        if truecolor {
            Self::TrueColor
        } else if term.contains("256color") {
            Self::Ansi256
        } else {
            Self::Ansi16
        }
    }
}

/// Whether mc paints its own background (config `background`, DESIGN §2).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "`Terminal` is selected by config, which arrives in M2"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Background {
    /// Paint [`BG`] at TrueColor.
    Paint,
    /// Always use the terminal's own background.
    Terminal,
}

/// Resolves [`Token`]s to ratatui colours for one terminal.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Theme {
    profile: Profile,
    background: Background,
}

impl Theme {
    /// Creates a theme for `profile` with the `background` setting.
    #[must_use]
    pub(crate) const fn new(profile: Profile, background: Background) -> Self {
        Self {
            profile,
            background,
        }
    }

    /// Returns whether the painted set is in use (TrueColor and `paint`).
    #[must_use]
    const fn painted(self) -> bool {
        matches!(
            (self.profile, self.background),
            (Profile::TrueColor, Background::Paint)
        )
    }

    /// Returns the colour of `token` for this terminal.
    ///
    /// Painted hex when painting, fallback hex at TrueColor otherwise, the
    /// declared index at 256/16, and the terminal default under `NO_COLOR`.
    /// [`Token::Fg`] is the terminal's own foreground whenever not painting.
    #[must_use]
    pub(crate) const fn color(self, token: Token) -> Color {
        let s = spec(token);
        if self.painted() {
            return rgb(s.painted);
        }
        match (self.profile, token) {
            (Profile::NoColor, _) | (_, Token::Fg) => Color::Reset,
            (Profile::Ansi16, _) => Color::Indexed(s.ansi16),
            (Profile::Ansi256, _) => Color::Indexed(s.ansi256),
            (Profile::TrueColor, _) => match s.fallback {
                Some(hex) => rgb(hex),
                None => Color::Reset,
            },
        }
    }

    /// Returns the screen style: painted background and primary text, or
    /// the terminal's own colours.
    #[must_use]
    pub(crate) const fn base(self) -> Style {
        let bg = if self.painted() {
            rgb(BG)
        } else {
            Color::Reset
        };
        Style::new().bg(bg).fg(self.color(Token::Fg))
    }

    /// Returns a style with `token` as the foreground.
    #[must_use]
    pub(crate) const fn fg(self, token: Token) -> Style {
        Style::new().fg(self.color(token))
    }

    /// Returns the inverse badge style: background-coloured text on
    /// `token` (DESIGN §2.1), by reverse video so it also works without
    /// colour.
    #[must_use]
    pub(crate) const fn badge(self, token: Token) -> Style {
        self.fg(token).add_modifier(Modifier::REVERSED)
    }
}

/// Converts `0xRRGGBB` to a ratatui colour.
#[must_use]
const fn rgb(hex: u32) -> Color {
    let [_, r, g, b] = hex.to_be_bytes();
    Color::Rgb(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Token; 9] = [
        Token::Fg,
        Token::FgMuted,
        Token::FgDim,
        Token::Border,
        Token::Accent,
        Token::Ok,
        Token::Warn,
        Token::Err,
        Token::Info,
    ];

    fn name(token: Token) -> &'static str {
        match token {
            Token::Fg => "fg",
            Token::FgMuted => "fg-muted",
            Token::FgDim => "fg-dim",
            Token::Border => "border",
            Token::Accent => "accent",
            Token::Ok => "ok",
            Token::Warn => "warn",
            Token::Err => "err",
            Token::Info => "info",
        }
    }

    /// One row of the DESIGN §2.1 table.
    struct DocRow {
        name: String,
        painted: u32,
        contrast: Option<f64>,
        ansi256: Option<u8>,
        ansi16: Option<u8>,
    }

    /// Parses the DESIGN §2.1 token table.
    fn doc_rows() -> Vec<DocRow> {
        let design = include_str!("../../docs/DESIGN.md");
        let section = design
            .split("### 2.1")
            .nth(1)
            .unwrap()
            .split("### 2.2")
            .next()
            .unwrap();
        let clean = |s: &str| s.trim().trim_matches('`').replace('★', "");
        section
            .lines()
            .filter(|l| l.starts_with("| `"))
            .map(|line| {
                let cols: Vec<&str> = line.split('|').collect();
                DocRow {
                    name: clean(cols[1]),
                    painted: u32::from_str_radix(&clean(cols[3])[1..], 16).unwrap(),
                    contrast: clean(cols[4])
                        .split_whitespace()
                        .next()
                        .unwrap()
                        .parse()
                        .ok(),
                    ansi256: clean(cols[6]).parse().ok(),
                    ansi16: clean(cols[7]).parse().ok(),
                }
            })
            .collect()
    }

    #[test]
    fn token_table_equals_design_spec() {
        let rows = doc_rows();
        assert_eq!(
            rows.len(),
            ALL.len() + 1,
            "DESIGN §2.1 rows: tokens plus bg"
        );
        let bg = rows.iter().find(|r| r.name == "bg").unwrap();
        assert_eq!(bg.painted, BG);
        for token in ALL {
            let row = rows.iter().find(|r| r.name == name(token)).unwrap();
            let s = spec(token);
            assert_eq!(s.painted, row.painted, "{} painted", row.name);
            assert_eq!(Some(s.ansi256), row.ansi256, "{} 256", row.name);
            assert_eq!(Some(s.ansi16), row.ansi16, "{} 16", row.name);
        }
    }

    #[test]
    fn fallback_set_equals_design_spec() {
        let expected = [
            (Token::Fg, None),
            (Token::FgMuted, Some(0x8a_99a8)),
            (Token::FgDim, Some(0x55_5555)),
            (Token::Border, Some(0x2a_2a2a)),
        ];
        for (token, want) in expected {
            assert_eq!(spec(token).fallback, want, "{}", name(token));
        }
        for token in [
            Token::Accent,
            Token::Ok,
            Token::Warn,
            Token::Err,
            Token::Info,
        ] {
            assert_eq!(
                spec(token).fallback,
                Some(spec(token).painted),
                "{}",
                name(token)
            );
        }
    }

    fn luminance(hex: u32) -> f64 {
        let [_, r, g, b] = hex.to_be_bytes();
        let lin = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn painted_contrast_matches_design_and_text_passes_aa() {
        for row in doc_rows().iter().filter(|r| r.name != "bg") {
            let ratio = contrast(row.painted, BG);
            let doc = row.contrast.unwrap();
            assert!(
                (ratio - doc).abs() < 0.01,
                "{}: {ratio:.2} vs doc {doc}",
                row.name
            );
            if !matches!(row.name.as_str(), "fg-dim" | "border") {
                assert!(ratio >= 4.5, "{} fails AA: {ratio:.2}", row.name);
            }
        }
    }

    #[test]
    fn state_colours_are_pairwise_distinct_at_256_and_16() {
        let states = [Token::Ok, Token::Warn, Token::Err, Token::Accent];
        for (i, a) in states.iter().enumerate() {
            for b in &states[i + 1..] {
                assert_ne!(spec(*a).ansi256, spec(*b).ansi256, "{a:?}/{b:?} at 256");
                assert_ne!(spec(*a).ansi16, spec(*b).ansi16, "{a:?}/{b:?} at 16");
            }
        }
        assert_ne!(spec(Token::FgMuted).ansi256, spec(Token::Info).ansi256);
        assert_ne!(spec(Token::FgMuted).ansi16, spec(Token::Info).ansi16);
    }

    #[test]
    fn paints_only_at_truecolor_with_paint() {
        let cases = [
            (
                Profile::TrueColor,
                Background::Paint,
                Color::Rgb(0x1c, 0x2a, 0x21),
            ),
            (Profile::TrueColor, Background::Terminal, Color::Reset),
            (Profile::Ansi256, Background::Paint, Color::Reset),
            (Profile::Ansi16, Background::Paint, Color::Reset),
            (Profile::NoColor, Background::Paint, Color::Reset),
        ];
        for (profile, background, want) in cases {
            assert_eq!(
                Theme::new(profile, background).base().bg,
                Some(want),
                "{profile:?}"
            );
        }
    }

    #[test]
    fn resolves_tokens_per_profile() {
        let cases = [
            (
                Profile::TrueColor,
                Background::Paint,
                Token::FgMuted,
                Color::Rgb(0x9a, 0xab, 0x9c),
            ),
            (
                Profile::TrueColor,
                Background::Terminal,
                Token::FgMuted,
                Color::Rgb(0x8a, 0x99, 0xa8),
            ),
            (
                Profile::TrueColor,
                Background::Terminal,
                Token::Fg,
                Color::Reset,
            ),
            (
                Profile::Ansi256,
                Background::Paint,
                Token::Ok,
                Color::Indexed(107),
            ),
            (Profile::Ansi256, Background::Paint, Token::Fg, Color::Reset),
            (
                Profile::Ansi16,
                Background::Paint,
                Token::Accent,
                Color::Indexed(3),
            ),
            (
                Profile::NoColor,
                Background::Paint,
                Token::Err,
                Color::Reset,
            ),
        ];
        for (profile, background, token, want) in cases {
            let got = Theme::new(profile, background).color(token);
            assert_eq!(got, want, "{profile:?} {background:?} {token:?}");
        }
    }

    #[test]
    fn detects_profile_from_environment() {
        let cases: &[(&[(&str, &str)], Profile)] = &[
            (
                &[("NO_COLOR", "1"), ("COLORTERM", "truecolor")],
                Profile::NoColor,
            ),
            (
                &[("NO_COLOR", ""), ("COLORTERM", "truecolor")],
                Profile::TrueColor,
            ),
            (&[("COLORTERM", "24bit")], Profile::TrueColor),
            (
                &[("TERM_PROGRAM", "ghostty"), ("TERM", "xterm")],
                Profile::TrueColor,
            ),
            (&[("TERM", "xterm-direct")], Profile::TrueColor),
            (&[("TERM", "tmux-256color")], Profile::Ansi256),
            (
                &[
                    ("TERM_PROGRAM", "Apple_Terminal"),
                    ("TERM", "xterm-256color"),
                ],
                Profile::Ansi256,
            ),
            (&[("TERM", "xterm")], Profile::Ansi16),
            (&[], Profile::Ansi16),
        ];
        for (env, want) in cases {
            let got = Profile::detect(|name| {
                env.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_string())
            });
            assert_eq!(got, *want, "{env:?}");
        }
    }
}
