//! The three glyph sets (DESIGN §3): `ascii` (default), `unicode` and
//! `nerd`. Every state glyph is one cell wide; borders and the mascot do
//! not belong to the sets (they follow the locale instead).

use serde::Deserialize;

/// The `icons` setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum IconSet {
    /// Plain ASCII: unambiguous in every terminal and width setting.
    #[default]
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

    /// Returns the filled and empty cells of a limit bar.
    #[must_use]
    pub(crate) const fn bar(self) -> (char, char) {
        match self {
            Self::Ascii => ('#', '-'),
            Self::Unicode | Self::Nerd => ('▮', '▯'),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
