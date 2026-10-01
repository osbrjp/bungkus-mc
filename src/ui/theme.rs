//! The Daun Pisang palette as a token spec, and colour-profile detection.
//!
//! [`spec`] mirrors the tables of DESIGN §2.1 (dark, "Daun Teduh") and
//! §2.2 (light, "Santan"): per theme a painted set and a terminal fallback
//! set. bungkus-cli's `styles.go` implements the dark fallback set. A test
//! parses DESIGN.md so the implementations cannot drift. The background is
//! painted only at TrueColor, so 256- and 16-colour terminals get the
//! fallback set's declared indices, never an automatic downsample
//! (DESIGN §2.3).

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

use crate::ui::icons::IconSet;

/// A named colour role (DESIGN §2.1).
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

/// One of the two Daun Pisang themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThemeName {
    /// "Daun Teduh", painted on `#1c2a21`.
    Dark,
    /// "Santan", painted on `#f0f3d8`.
    Light,
}

/// The `theme` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ThemeChoice {
    /// Follow the terminal's background (one OSC 11 query at start).
    #[default]
    Auto,
    /// Always dark.
    Dark,
    /// Always light.
    Light,
}

impl ThemeChoice {
    /// All choices, in the order the settings screen lists them.
    pub(crate) const ALL: [Self; 3] = [Self::Auto, Self::Dark, Self::Light];

    /// Returns the word shown in the UI.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// Resolves the choice to a theme; `auto` uses `host_is_light`, the
    /// answer to the start-up OSC 11 query (`None` when unanswered: dark).
    #[must_use]
    pub(crate) const fn resolve(self, host_is_light: Option<bool>) -> ThemeName {
        match (self, host_is_light) {
            (Self::Light, _) | (Self::Auto, Some(true)) => ThemeName::Light,
            (Self::Dark | Self::Auto, _) => ThemeName::Dark,
        }
    }
}

/// The declared values of one [`Token`] in one theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TokenSpec {
    /// Hex used when the background is painted.
    pub painted: u32,
    /// Hex used at TrueColor on the terminal's own background; `None` is
    /// the terminal's default colour.
    pub fallback: Option<u32>,
    /// Declared xterm-256 index of the fallback set; unused for
    /// [`Token::Fg`], which is the terminal's own foreground there.
    pub ansi256: u8,
    /// Declared 16-colour index of the fallback set.
    pub ansi16: u8,
}

/// Returns the painted screen background of `theme`.
#[must_use]
pub(crate) const fn bg(theme: ThemeName) -> u32 {
    match theme {
        ThemeName::Dark => 0x1c_2a21,
        ThemeName::Light => 0xf0_f3d8,
    }
}

/// Returns the declared values of `token` in `theme` (DESIGN §2.1, §2.2).
#[must_use]
pub(crate) const fn spec(theme: ThemeName, token: Token) -> TokenSpec {
    const fn t(painted: u32, fallback: Option<u32>, ansi256: u8, ansi16: u8) -> TokenSpec {
        TokenSpec {
            painted,
            fallback,
            ansi256,
            ansi16,
        }
    }
    const fn same(hex: u32, ansi256: u8, ansi16: u8) -> TokenSpec {
        t(hex, Some(hex), ansi256, ansi16)
    }
    match theme {
        ThemeName::Dark => match token {
            Token::Fg => t(0xd6_e2d3, None, 0, 0),
            Token::FgMuted => t(0x9a_ab9c, Some(0x8a_99a8), 245, 7),
            Token::FgDim => t(0x5c_7062, Some(0x55_5555), 240, 8),
            Token::Border => t(0x3b_4d40, Some(0x2a_2a2a), 235, 8),
            Token::Accent => same(0xff_aa88, 216, 3),
            Token::Ok => same(0x7f_b069, 107, 2),
            Token::Warn => same(0xe8_c547, 185, 11),
            Token::Err => same(0xff_7361, 209, 9),
            Token::Info => same(0x6f_8fc7, 68, 12),
        },
        ThemeName::Light => match token {
            Token::Fg => t(0x1f_2a22, None, 0, 0),
            Token::FgMuted => t(0x56_645a, Some(0x5a_6470), 241, 8),
            Token::FgDim => t(0x8a_9a88, Some(0x8a_8f8a), 245, 8),
            Token::Border => t(0xc9_d1b4, Some(0xcf_d2c8), 252, 7),
            Token::Accent => t(0x9e_3a18, Some(0xb8_461f), 130, 5),
            Token::Ok => t(0x2e_7a3e, Some(0x2f_6b25), 22, 2),
            Token::Warn => t(0x7a_5200, Some(0x8a_5b00), 94, 3),
            Token::Err => t(0xa8_261e, Some(0xb3_261e), 124, 1),
            Token::Info => same(0x3f_5f8a, 60, 4),
        },
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Background {
    /// Paint the theme's background at TrueColor.
    #[default]
    Paint,
    /// Always use the terminal's own background.
    Terminal,
}

/// Resolves [`Token`]s to ratatui colours for one terminal and theme, and
/// carries the other terminal-dependent view choices (glyph set, locale,
/// motion).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Theme {
    /// Which palette.
    pub name: ThemeName,
    profile: Profile,
    background: Background,
    /// The state glyph set (`icons`).
    pub icons: IconSet,
    /// Whether the locale is UTF-8 (box-drawing borders, half-block mascot).
    pub utf8: bool,
    /// Whether spinners and the mascot move (`motion`).
    pub motion: bool,
}

impl Theme {
    /// Creates a theme for `profile` with the `background` setting.
    #[must_use]
    pub(crate) const fn new(name: ThemeName, profile: Profile, background: Background) -> Self {
        Self {
            name,
            profile,
            background,
            icons: IconSet::Ascii,
            utf8: true,
            motion: true,
        }
    }

    /// Returns this theme with the given glyph set, locale and motion.
    #[must_use]
    pub(crate) const fn with_view(self, icons: IconSet, utf8: bool, motion: bool) -> Self {
        Self {
            icons,
            utf8,
            motion,
            ..self
        }
    }

    /// Returns whether anything may animate: motion on and colour on
    /// (DESIGN §7).
    #[must_use]
    pub(crate) const fn animated(self) -> bool {
        self.motion && !self.no_color()
    }

    /// Returns this theme with another palette, keeping the terminal.
    #[must_use]
    pub(crate) const fn with_name(self, name: ThemeName) -> Self {
        Self { name, ..self }
    }

    /// Returns whether the painted set is in use (TrueColor and `paint`).
    #[must_use]
    pub(crate) const fn painted(self) -> bool {
        matches!(
            (self.profile, self.background),
            (Profile::TrueColor, Background::Paint)
        )
    }

    /// Returns the agent mark's colour, bold: Claude's orange (`#d97757`,
    /// 256-index 173, yellow at 16 colours) and Codex's green (`#10a37f`,
    /// 256-index 36, cyan at 16); plain bold under `NO_COLOR`.
    #[must_use]
    pub(crate) const fn agent_style(self, kind: crate::agent::Kind) -> ratatui::style::Style {
        use crate::agent::Kind::{Claude, Codex};
        let bold = ratatui::style::Style::new().add_modifier(ratatui::style::Modifier::BOLD);
        let color = match (kind, self.profile) {
            (_, Profile::NoColor) => return bold,
            (Claude, Profile::TrueColor) => rgb(0xd9_7757),
            (Codex, Profile::TrueColor) => rgb(0x10_a37f),
            (Claude, Profile::Ansi256) => Color::Indexed(173),
            (Codex, Profile::Ansi256) => Color::Indexed(36),
            (Claude, Profile::Ansi16) => Color::Indexed(3),
            (Codex, Profile::Ansi16) => Color::Indexed(6),
        };
        bold.fg(color)
    }

    /// Returns whether colour and motion are off (`NO_COLOR`).
    #[must_use]
    pub(crate) const fn no_color(self) -> bool {
        matches!(self.profile, Profile::NoColor)
    }

    /// Returns the colour of `token` for this terminal.
    ///
    /// Painted hex when painting, fallback hex at TrueColor otherwise, the
    /// declared index at 256/16, and the terminal default under `NO_COLOR`.
    /// [`Token::Fg`] is the terminal's own foreground whenever not painting.
    #[must_use]
    pub(crate) const fn color(self, token: Token) -> Color {
        let s = spec(self.name, token);
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
            rgb(bg(self.name))
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
pub(crate) const fn rgb(hex: u32) -> Color {
    let [_, r, g, b] = hex.to_be_bytes();
    Color::Rgb(r, g, b)
}

/// Returns whether an `rgb:RRRR/GGGG/BBBB` OSC 10/11 reply colour is light
/// (relative luminance above one half).
///
/// # Arguments
///
/// * `reply` - The terminal's reply; anything around the `rgb:` spec is
///   ignored.
#[must_use]
pub(crate) fn is_light_reply(reply: &str) -> Option<bool> {
    let spec = reply.split("rgb:").nth(1)?;
    let mut channels = spec.split('/').map(|part| {
        let hex: String = part.chars().take_while(char::is_ascii_hexdigit).collect();
        let max = 16_f64.powi(i32::try_from(hex.len()).ok()?) - 1.0;
        let value = u32::from_str_radix(&hex, 16).ok()?;
        (max > 0.0).then(|| f64::from(value) / max)
    });
    let (r, g, b) = (channels.next()??, channels.next()??, channels.next()??);
    Some(0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5)
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

    const THEMES: [(ThemeName, &str, &str); 2] = [
        (ThemeName::Dark, "### 2.1", "### 2.2"),
        (ThemeName::Light, "### 2.2", "### 2.3"),
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

    /// One row of a DESIGN §2 painted table.
    struct PaintedRow {
        name: String,
        hex: u32,
        contrast: Option<f64>,
    }

    /// One row of a DESIGN §2 fallback table.
    struct FallbackRow {
        name: String,
        /// `None` is the terminal default.
        hex: Option<u32>,
        ansi256: Option<u8>,
        ansi16: Option<u8>,
    }

    /// Parses the painted and fallback tables between two DESIGN.md
    /// headings, telling them apart by column count.
    fn doc_tables(from: &str, to: &str) -> (Vec<PaintedRow>, Vec<FallbackRow>) {
        let design = include_str!("../../docs/DESIGN.md");
        let section = design.split(from).nth(1).unwrap().split(to).next().unwrap();
        let clean = |s: &str| s.trim().trim_matches('`').replace('★', "");
        let hex = |s: &str| u32::from_str_radix(clean(s).strip_prefix('#')?, 16).ok();
        let contrast = |s: &str| clean(s).split_whitespace().next()?.parse().ok();
        let (mut painted, mut fallback) = (Vec::new(), Vec::new());
        for line in section.lines().filter(|l| l.starts_with("| `")) {
            let cols: Vec<&str> = line.split('|').collect();
            match cols.len() {
                7 => painted.push(PaintedRow {
                    name: clean(cols[1]),
                    hex: hex(cols[3]).unwrap(),
                    contrast: contrast(cols[4]),
                }),
                5 => painted.push(PaintedRow {
                    name: clean(cols[1]),
                    hex: hex(cols[2]).unwrap(),
                    contrast: contrast(cols[3]),
                }),
                6 => fallback.push(FallbackRow {
                    name: clean(cols[1]),
                    hex: hex(cols[2]),
                    ansi256: clean(cols[3]).parse().ok(),
                    ansi16: clean(cols[4]).parse().ok(),
                }),
                n => panic!("unexpected {n}-column row: {line}"),
            }
        }
        (painted, fallback)
    }

    #[test]
    fn painted_sets_equal_design_spec() {
        for (theme, from, to) in THEMES {
            let rows = doc_tables(from, to).0;
            assert_eq!(rows.len(), ALL.len() + 1, "{theme:?}: tokens plus bg");
            assert_eq!(rows.iter().find(|r| r.name == "bg").unwrap().hex, bg(theme));
            for token in ALL {
                let row = rows.iter().find(|r| r.name == name(token)).unwrap();
                assert_eq!(
                    spec(theme, token).painted,
                    row.hex,
                    "{theme:?} {}",
                    row.name
                );
            }
        }
    }

    #[test]
    fn fallback_sets_equal_design_spec() {
        for (theme, from, to) in THEMES {
            let rows = doc_tables(from, to).1;
            assert_eq!(rows.len(), ALL.len(), "{theme:?} fallback rows");
            for token in ALL {
                let row = rows.iter().find(|r| r.name == name(token)).unwrap();
                let s = spec(theme, token);
                assert_eq!(s.fallback, row.hex, "{theme:?} {} hex", row.name);
                if token == Token::Fg {
                    assert_eq!(
                        (row.ansi256, row.ansi16),
                        (None, None),
                        "fg: terminal default"
                    );
                } else {
                    assert_eq!(Some(s.ansi256), row.ansi256, "{theme:?} {} 256", row.name);
                    assert_eq!(Some(s.ansi16), row.ansi16, "{theme:?} {} 16", row.name);
                }
            }
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
        for (theme, from, to) in THEMES {
            for row in doc_tables(from, to).0.iter().filter(|r| r.name != "bg") {
                let ratio = contrast(row.hex, bg(theme));
                let doc = row.contrast.unwrap();
                assert!(
                    (ratio - doc).abs() < 0.01,
                    "{theme:?} {}: {ratio:.2} vs {doc}",
                    row.name
                );
                if !matches!(row.name.as_str(), "fg-dim" | "border") {
                    assert!(ratio >= 4.5, "{theme:?} {} fails AA: {ratio:.2}", row.name);
                }
            }
        }
    }

    #[test]
    fn state_colours_are_pairwise_distinct_at_256_and_16() {
        let states = [Token::Ok, Token::Warn, Token::Err, Token::Accent];
        for (theme, _, _) in THEMES {
            let s = |t| spec(theme, t);
            for (i, a) in states.iter().enumerate() {
                for b in &states[i + 1..] {
                    assert_ne!(s(*a).ansi256, s(*b).ansi256, "{theme:?} {a:?}/{b:?} at 256");
                    assert_ne!(s(*a).ansi16, s(*b).ansi16, "{theme:?} {a:?}/{b:?} at 16");
                }
            }
            assert_ne!(
                s(Token::FgMuted).ansi256,
                s(Token::Info).ansi256,
                "{theme:?}"
            );
            assert_ne!(s(Token::FgMuted).ansi16, s(Token::Info).ansi16, "{theme:?}");
        }
    }

    #[test]
    fn paints_only_at_truecolor_with_paint() {
        let cases = [
            (
                ThemeName::Dark,
                Profile::TrueColor,
                Background::Paint,
                Color::Rgb(0x1c, 0x2a, 0x21),
            ),
            (
                ThemeName::Light,
                Profile::TrueColor,
                Background::Paint,
                Color::Rgb(0xf0, 0xf3, 0xd8),
            ),
            (
                ThemeName::Dark,
                Profile::TrueColor,
                Background::Terminal,
                Color::Reset,
            ),
            (
                ThemeName::Dark,
                Profile::Ansi256,
                Background::Paint,
                Color::Reset,
            ),
            (
                ThemeName::Light,
                Profile::Ansi16,
                Background::Paint,
                Color::Reset,
            ),
            (
                ThemeName::Dark,
                Profile::NoColor,
                Background::Paint,
                Color::Reset,
            ),
        ];
        for (name, profile, background, want) in cases {
            let got = Theme::new(name, profile, background).base().bg;
            assert_eq!(got, Some(want), "{name:?} {profile:?} {background:?}");
        }
    }

    #[test]
    fn resolves_tokens_per_profile() {
        use Background::{Paint, Terminal};
        use ThemeName::{Dark, Light};
        let cases = [
            (
                Dark,
                Profile::TrueColor,
                Paint,
                Token::FgMuted,
                Color::Rgb(0x9a, 0xab, 0x9c),
            ),
            (
                Dark,
                Profile::TrueColor,
                Terminal,
                Token::FgMuted,
                Color::Rgb(0x8a, 0x99, 0xa8),
            ),
            (Dark, Profile::TrueColor, Terminal, Token::Fg, Color::Reset),
            (
                Dark,
                Profile::Ansi256,
                Paint,
                Token::Ok,
                Color::Indexed(107),
            ),
            (
                Dark,
                Profile::Ansi256,
                Paint,
                Token::Border,
                Color::Indexed(235),
            ),
            (Dark, Profile::Ansi256, Paint, Token::Fg, Color::Reset),
            (
                Dark,
                Profile::Ansi16,
                Paint,
                Token::Accent,
                Color::Indexed(3),
            ),
            (
                Light,
                Profile::Ansi256,
                Paint,
                Token::Ok,
                Color::Indexed(22),
            ),
            (
                Light,
                Profile::TrueColor,
                Paint,
                Token::Fg,
                Color::Rgb(0x1f, 0x2a, 0x22),
            ),
            (Dark, Profile::NoColor, Paint, Token::Err, Color::Reset),
        ];
        for (name, profile, background, token, want) in cases {
            let got = Theme::new(name, profile, background).color(token);
            assert_eq!(got, want, "{name:?} {profile:?} {background:?} {token:?}");
        }
    }

    #[test]
    fn resolves_the_theme_choice() {
        use ThemeChoice::{Auto, Dark, Light};
        let cases = [
            (Auto, None, ThemeName::Dark),
            (Auto, Some(false), ThemeName::Dark),
            (Auto, Some(true), ThemeName::Light),
            (Dark, Some(true), ThemeName::Dark),
            (Light, None, ThemeName::Light),
        ];
        for (choice, host, want) in cases {
            assert_eq!(choice.resolve(host), want, "{choice:?} {host:?}");
        }
    }

    #[test]
    fn reads_the_host_background_from_an_osc_11_reply() {
        let cases = [
            ("\u{1b}]11;rgb:1c1c/2a2a/2121\u{7}", Some(false)),
            ("\u{1b}]11;rgb:ffff/ffff/ffff\u{1b}\\", Some(true)),
            ("\u{1b}]11;rgb:f0/f3/d8\u{7}", Some(true)),
            ("\u{1b}]11;rgb:00/00\u{7}", None),
            ("garbage", None),
        ];
        for (reply, want) in cases {
            assert_eq!(is_light_reply(reply), want, "{reply:?}");
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
