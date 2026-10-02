//! The three glyph sets (DESIGN §3): `ascii`, `unicode` and `nerd`, and
//! the `icons` setting that picks one (`auto` by default: `nerd` when a
//! Nerd Font is installed, else `ascii`). Every state glyph is one cell
//! wide; borders and the mascot do not belong to the sets (they follow the
//! locale instead).
//!
//! Detection only lists font folders; it never opens a font file or runs
//! a command.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The `icons` setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum IconChoice {
    /// `nerd` when a Nerd Font is installed, else `ascii`.
    #[default]
    Auto,
    /// Always [`IconSet::Ascii`].
    Ascii,
    /// Always [`IconSet::Unicode`].
    Unicode,
    /// Always [`IconSet::Nerd`].
    Nerd,
}

impl IconChoice {
    /// Parses a `--icons` value.
    #[must_use]
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(Self::Auto),
            "ascii" => Some(Self::Ascii),
            "unicode" => Some(Self::Unicode),
            "nerd" => Some(Self::Nerd),
            _ => None,
        }
    }

    /// Returns the glyph set this choice means.
    ///
    /// # Arguments
    ///
    /// * `nerd_font` - Whether a Nerd Font is usable; asked only for
    ///   [`IconChoice::Auto`].
    #[must_use]
    pub(crate) fn resolve(self, nerd_font: impl FnOnce() -> bool) -> IconSet {
        match self {
            Self::Auto if nerd_font() => IconSet::Nerd,
            Self::Auto | Self::Ascii => IconSet::Ascii,
            Self::Unicode => IconSet::Unicode,
            Self::Nerd => IconSet::Nerd,
        }
    }
}

/// How many folder levels below a font folder are searched; Linux distros
/// nest fonts as `fonts/truetype/<family>/`.
const FONT_DEPTH: u8 = 3;

/// Returns the folders where the user's and the system's fonts live on
/// macOS and Linux.
///
/// # Arguments
///
/// * `home` - The user's home directory, if known.
#[must_use]
pub(crate) fn font_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let user = ["Library/Fonts", ".local/share/fonts", ".fonts"];
    let system = [
        "/Library/Fonts",
        "/usr/local/share/fonts",
        "/usr/share/fonts",
    ];
    home.into_iter()
        .flat_map(|h| user.map(|d| h.join(d)))
        .chain(system.map(PathBuf::from))
        .collect()
}

/// Returns whether any of `dirs` holds a file or folder with "nerd" in its
/// name, which every Nerd Font release has (`JetBrainsMonoNerdFont-…`).
///
/// An installed font is the best signal there is: the terminal's own font
/// cannot be queried (ARCHITECTURE §12), so a user whose terminal uses
/// another font sets `icons` by hand. Unreadable folders count as empty.
#[must_use]
pub(crate) fn nerd_font_installed(dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| has_nerd_name(dir, FONT_DEPTH))
}

/// Searches `dir` and `depth` levels below it for a name containing
/// "nerd"; symlinked folders are not followed.
fn has_nerd_name(dir: &Path, depth: u8) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("nerd")
            || (depth > 0
                && entry.file_type().is_ok_and(|t| t.is_dir())
                && has_nerd_name(&entry.path(), depth - 1))
    })
}

/// A set of state glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IconSet {
    /// Plain ASCII: unambiguous in every terminal and width setting.
    Ascii,
    /// Narrow Unicode symbols.
    Unicode,
    /// Nerd Font private-use glyphs.
    Nerd,
}

/// A glyph with a meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    /// Your turn.
    YourTurn,
    /// Needs you.
    NeedsYou,
    /// Failed.
    Failed,
    /// Wrapped.
    Wrapped,
    /// Stopped.
    Stopped,
    /// A subagent row.
    Subagent,
    /// The focus marker on the selected row.
    Marker,
}

impl IconSet {
    /// Returns the glyph for `icon` in this set (DESIGN §3 table).
    #[must_use]
    pub(crate) const fn icon(self, icon: Icon) -> char {
        match (self, icon) {
            (Self::Ascii, Icon::YourTurn) => '~',
            (Self::Unicode, Icon::YourTurn) => '»',
            (Self::Nerd, Icon::YourTurn) => '\u{f0e7}',
            (Self::Ascii | Self::Unicode, Icon::NeedsYou) => '!',
            (Self::Nerd, Icon::NeedsYou) => '\u{f071}',
            (Self::Ascii, Icon::Failed) => 'x',
            (Self::Unicode, Icon::Failed) => '✗',
            (Self::Nerd, Icon::Failed) => '\u{f00d}',
            (Self::Ascii, Icon::Wrapped) => '+',
            (Self::Unicode, Icon::Wrapped) => '✓',
            (Self::Nerd, Icon::Wrapped) => '\u{f00c}',
            (Self::Ascii, Icon::Stopped) => '#',
            (Self::Unicode, Icon::Stopped) => '▪',
            (Self::Nerd, Icon::Stopped) => '\u{f04d}',
            (Self::Ascii, Icon::Subagent) => '*',
            (Self::Unicode, Icon::Subagent) => '◦',
            (Self::Nerd, Icon::Subagent | Icon::Marker) => '\u{f0da}',
            (Self::Ascii, Icon::Marker) => '>',
            (Self::Unicode, Icon::Marker) => '▸',
        }
    }

    /// Returns the spinner frames (DESIGN §3 "running").
    #[must_use]
    pub(crate) const fn spinner(self) -> &'static [char] {
        match self {
            Self::Ascii => &['|', '/', '-', '\\'],
            Self::Unicode | Self::Nerd => &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'],
        }
    }

    /// Returns how MCP server `name` shows on a card (DESIGN §5.2): in the
    /// `nerd` set a brand glyph alone when a word of the name is a known
    /// brand, else the plug glyph and the name; the name in the other sets.
    #[must_use]
    pub(crate) fn mcp(self, name: &str) -> String {
        match self {
            Self::Ascii | Self::Unicode => name.to_owned(),
            Self::Nerd => {
                let lower = name.to_ascii_lowercase();
                let has = |w: &&str| {
                    lower
                        .split(|c: char| !c.is_ascii_alphanumeric())
                        .any(|s| s == *w)
                };
                MCP_BRANDS
                    .iter()
                    .find(|(words, _)| words.iter().any(has))
                    .map_or_else(|| format!("{MCP_PLUG} {name}"), |(_, g)| g.to_string())
            }
        }
    }

    /// Returns the filled and empty cells of a limit bar.
    #[must_use]
    pub(crate) const fn bar(self) -> (char, char) {
        match self {
            Self::Ascii => ('#', '-'),
            Self::Unicode | Self::Nerd => ('▮', '▯'),
        }
    }
}

/// The `nerd` glyph of an MCP server without a brand glyph (plug).
const MCP_PLUG: char = '\u{f1e6}';

/// Words in an MCP server's name and the `nerd` glyph they stand for
/// (DESIGN §3); the first match wins.
const MCP_BRANDS: [(&[&str], char); 10] = [
    (&["github"], '\u{f09b}'),
    (&["gitlab"], '\u{f296}'),
    (&["slack"], '\u{f198}'),
    (&["chrome"], '\u{f268}'),
    (&["playwright", "puppeteer", "browser"], '\u{f0ac}'),
    (&["aws", "amazon"], '\u{f270}'),
    (&["gmail"], '\u{f0e0}'),
    (&["google", "drive"], '\u{f1a0}'),
    (&["postgres", "sqlite", "mysql", "supabase"], '\u{f1c0}'),
    (&["docker"], '\u{f308}'),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_servers_show_a_brand_glyph_only_in_the_nerd_set() {
        let cases = [
            (IconSet::Nerd, "plugin:slack:slack", "\u{f198}"),
            (IconSet::Nerd, "GitHub", "\u{f09b}"),
            (IconSet::Nerd, "claude.ai Google Drive", "\u{f1a0}"),
            (IconSet::Nerd, "blender", "\u{f1e6} blender"),
            (IconSet::Nerd, "webdriver", "\u{f1e6} webdriver"),
            (IconSet::Ascii, "github", "github"),
            (IconSet::Unicode, "blender", "blender"),
        ];
        for (set, name, want) in cases {
            assert_eq!(set.mcp(name), want, "{set:?} {name}");
        }
    }

    const ALL: [Icon; 7] = [
        Icon::YourTurn,
        Icon::NeedsYou,
        Icon::Failed,
        Icon::Wrapped,
        Icon::Stopped,
        Icon::Subagent,
        Icon::Marker,
    ];

    #[test]
    fn ascii_and_unicode_glyphs_are_one_cell_and_narrow() {
        for set in [IconSet::Ascii, IconSet::Unicode] {
            let glyphs = ALL
                .iter()
                .map(|i| set.icon(*i))
                .chain(set.spinner().iter().copied());
            for g in glyphs.chain([set.bar().0, set.bar().1]) {
                let width = ratatui::text::Span::raw(g.to_string()).width();
                assert_eq!(width, 1, "{set:?} {g:?}");
            }
        }
    }

    #[test]
    fn auto_picks_nerd_only_when_a_nerd_font_is_installed() {
        let root = std::env::temp_dir().join(format!("mc-fonts-{}", std::process::id()));
        let nested = root.join("truetype/jetbrains");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("DejaVuSansMono.ttf"), "").unwrap();
        let dirs = [root.join("missing"), root.clone()];
        assert!(!nerd_font_installed(&dirs));
        std::fs::write(nested.join("JetBrainsMonoNerdFont-Regular.ttf"), "").unwrap();
        assert!(nerd_font_installed(&dirs));
        std::fs::remove_dir_all(&root).unwrap();

        let cases = [
            (IconChoice::Auto, true, IconSet::Nerd),
            (IconChoice::Auto, false, IconSet::Ascii),
            (IconChoice::Ascii, true, IconSet::Ascii),
            (IconChoice::Unicode, true, IconSet::Unicode),
            (IconChoice::Nerd, false, IconSet::Nerd),
        ];
        for (choice, font, want) in cases {
            assert_eq!(choice.resolve(|| font), want, "{choice:?} {font}");
        }
    }

    #[test]
    fn states_are_distinct_within_a_set() {
        for set in [IconSet::Ascii, IconSet::Unicode, IconSet::Nerd] {
            let states: Vec<char> = ALL[..5].iter().map(|i| set.icon(*i)).collect();
            let mut unique = states.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), states.len(), "{set:?}");
        }
    }
}
