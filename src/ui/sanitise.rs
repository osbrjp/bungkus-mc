//! The one string sanitiser for text that does not come from the PTY.
//!
//! Directory names, hook fields, prompts and process names are untrusted
//! (SECURITY.md "All strings not from the PTY"). Everything shown in a card,
//! the header or a dialog passes through [`sanitise`] first, so no such
//! string can move the cursor, set a title or write the clipboard.

/// Returns `input` with escape sequences and control characters removed,
/// cut to at most `max` characters.
///
/// Every `ESC`-led sequence is dropped up to its terminator (CSI to its
/// final byte; OSC, DCS, SOS, PM and APC to BEL or `ESC \`; any other
/// escape with its one following character). Then tabs become spaces and
/// every other C0 control, DEL and C1 control (U+0080–U+009F) is dropped.
/// A string longer than `max` characters ends in `…` within the limit.
///
/// # Arguments
///
/// * `input` - The untrusted text.
/// * `max`   - Maximum length in characters, including the `…`.
#[must_use]
pub(crate) fn sanitise(input: &str, max: usize) -> String {
    let mut out = String::with_capacity(input.len().min(max * 4));
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    truncate(&out, max)
}

/// Returns `s` cut to at most `max` characters, ending in `…` when cut.
#[must_use]
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    if max > 0 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hostile corpus of ARCHITECTURE §4.2 as strings.
    const HOSTILE: &[&str] = &[
        "\u{1b}]52;c;aGk=\u{7}",
        "\u{1b}]0;title\u{7}",
        "\u{1b}]2;title\u{1b}\\",
        "\u{1b}]8;;file:///etc/passwd\u{1b}\\x\u{1b}]8;;\u{1b}\\",
        "\u{1b}]8;;javascript:alert(1)\u{7}x",
        "\u{1b}]1337;File=a\u{7}",
        "\u{1b}]9;notify\u{7}",
        "\u{1b}]99;;notify\u{1b}\\",
        "\u{1b}]777;notify;a;b\u{7}",
        "\u{1b}_Gf=100;AAAA\u{1b}\\",
        "\u{1b}Ptmux;\u{1b}\u{1b}]0;x\u{7}\u{1b}\\",
        "\u{1b}[21t",
        "\u{1b}P$qm\u{1b}\\",
        "\u{1b}[c\u{1b}[6n\u{1b}[>q",
        "\u{9b}31m\u{85}\u{90}",
        "\u{1b}c",
        "\u{1b}[?1049h\u{1b}[?1049l",
        "\u{7}\u{8}\u{7f}\r\n",
    ];

    #[test]
    fn hostile_corpus_leaves_only_printable_text() {
        for input in HOSTILE {
            let out = sanitise(&format!("a{input}b"), 200);
            assert!(!out.chars().any(char::is_control), "{input:?} → {out:?}");
            assert!(
                out.starts_with('a') && out.ends_with('b'),
                "{input:?} → {out:?}"
            );
        }
    }

    #[test]
    fn keeps_text_and_turns_tabs_into_spaces() {
        let cases = [
            ("kedai-web", "kedai-web"),
            ("a\tb", "a b"),
            ("日本語 ⏺ ok", "日本語 ⏺ ok"),
            ("\u{1b}[1mbold\u{1b}[0m", "bold"),
            ("unterminated \u{1b}]0;title", "unterminated "),
        ];
        for (input, want) in cases {
            assert_eq!(sanitise(input, 80), want, "{input:?}");
        }
    }

    #[test]
    fn truncates_with_an_ellipsis_within_the_limit() {
        assert_eq!(sanitise("abcdef", 4), "abc…");
        assert_eq!(sanitise("abcd", 4), "abcd");
        assert_eq!(truncate("日本語です", 3), "日本…");
        assert_eq!(truncate("abc", 0), "");
    }
}
