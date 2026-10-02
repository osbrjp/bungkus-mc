//! A small Markdown reader for the issue and pull request text view
//! (issue #188): untrusted text in, sanitised styled lines out.
//!
//! It knows what GitHub descriptions mostly hold: headings, lists, quotes,
//! rules, fenced code, `code`, **bold**, *italic*, links and images (their
//! text only). Everything else, tables included, is shown as written. It
//! never emits a URL or a control character, and it draws nothing itself:
//! [`crate::ui`] gives each [`Ink`] its style.

use ratatui::text::Span;

use crate::ui::sanitise::sanitise;

/// Longest source line read, in characters; a longer one ends in `…`.
const LINE_MAX: usize = 4000;

/// How a piece of text is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ink {
    /// Body text.
    Plain,
    /// `**bold**`.
    Bold,
    /// `*italic*`.
    Italic,
    /// `` `code` `` and fenced code.
    Code,
    /// A `#` heading.
    Heading,
    /// List markers, quotes and rules.
    Muted,
}

/// One wrapped line: runs of text, each in one ink.
pub(crate) type TextLine = Vec<(Ink, String)>;

/// Reads `text` as Markdown and returns it as lines of at most `width`
/// columns.
///
/// Each source line is one block, as GitHub shows a description (a line
/// break is a line break). Lines wrap at a space where there is one, list
/// items and quotes with a hanging indent; wide characters (CJK) count
/// two columns.
#[must_use]
pub(crate) fn render(text: &str, width: usize) -> Vec<TextLine> {
    let mut out = Vec::new();
    let mut fence: Option<String> = None;
    for raw in text.lines() {
        let line = sanitise(raw, LINE_MAX);
        let trimmed = line.trim_start();
        match (&fence, fence_mark(trimmed)) {
            (None, Some(mark)) => {
                fence = Some(mark.to_owned());
                continue;
            }
            (Some(open), Some(mark)) if mark.starts_with(open) && trimmed.trim_end() == mark => {
                fence = None;
                continue;
            }
            _ => {}
        }
        let mut cells: Vec<(char, Ink)> = Vec::new();
        let mut indent = 0;
        if fence.is_some() {
            cells.extend("  ".chars().chain(line.chars()).map(|c| (c, Ink::Code)));
            indent = 2;
        } else if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
            continue;
        } else if let Some(title) = heading(trimmed) {
            inline(title, Ink::Heading, &mut cells);
        } else if is_rule(trimmed) {
            cells.extend(std::iter::repeat_n(('-', Ink::Muted), width));
        } else if let Some(quote) = trimmed.strip_prefix('>') {
            cells.extend("| ".chars().map(|c| (c, Ink::Muted)));
            inline(quote.trim_start(), Ink::Muted, &mut cells);
            indent = 2;
        } else {
            let lead = line.len() - trimmed.len() + list_marker(trimmed);
            cells.extend(line[..lead].chars().map(|c| (c, Ink::Muted)));
            indent = cells.len();
            inline(&line[lead..], Ink::Plain, &mut cells);
        }
        wrap(cells, width, indent.min(width / 2), &mut out);
    }
    out
}

/// Returns the run of three or more `` ` `` or `~` a code fence line
/// starts with. A line with a backtick after the run is no fence (one-line
/// `` ```code``` ``). A fence closes on a line that is only such a run, of
/// the same character and at least as long as the one that opened it.
fn fence_mark(line: &str) -> Option<&str> {
    let mark = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = line.len() - line.trim_start_matches(mark).len();
    (run >= 3 && !line[run..].contains('`')).then(|| &line[..run])
}

/// Returns where the closing `mark` (`*` or `**`) of emphasis is in
/// `text`: the first one that follows a character other than a space or
/// a `*` and is not followed by a `*`.
fn closing(text: &str, mark: &str) -> Option<usize> {
    text.match_indices(mark).map(|(at, _)| at).find(|&at| {
        let before = text[..at].chars().next_back();
        let after = text[at + mark.len()..].chars().next();
        before.is_some_and(|c| !c.is_whitespace() && c != '*') && after != Some('*')
    })
}

/// Returns the index of the `close` that matches an `open` already read,
/// skipping nested pairs.
fn matching(text: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0;
    for (at, ch) in text.char_indices() {
        if ch == open {
            depth += 1;
        } else if ch == close && depth == 0 {
            return Some(at);
        } else if ch == close {
            depth -= 1;
        }
    }
    None
}

/// Returns the title of a `#`…`######` heading line.
fn heading(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches('#');
    let level = line.len() - rest.len();
    rest.strip_prefix(' ').filter(|_| (1..=6).contains(&level))
}

/// Returns whether `line` is a horizontal rule: three or more of one of
/// `-`, `*`, `_` and nothing else.
fn is_rule(line: &str) -> bool {
    let line = line.trim_end();
    line.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|mark| line.chars().all(|c| c == *mark))
}

/// Returns the length in bytes of the list marker `line` starts with
/// (`- `, `* `, `+ `, `12. `, `3) `), or 0.
fn list_marker(line: &str) -> usize {
    if ["- ", "* ", "+ "].iter().any(|mark| line.starts_with(mark)) {
        return 2;
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let rest = &line[digits..];
    if (1..=9).contains(&digits) && (rest.starts_with(". ") || rest.starts_with(") ")) {
        digits + 2
    } else {
        0
    }
}

/// Splits `[label](address)…` after its `[` into the label and what
/// follows the `)`; `None` when `rest` does not start a link. Brackets
/// in the label (an image in a link) and parentheses in the address are
/// matched in pairs.
fn link(rest: &str) -> Option<(&str, &str)> {
    let close = matching(rest, '[', ']')?;
    let address = rest[close + 1..].strip_prefix('(')?;
    let end = matching(address, '(', ')')?;
    Some((&rest[..close], &address[end + 1..]))
}

/// Appends `text` to `out` with its inline Markdown read: `` `code` ``,
/// `**bold**`, `*italic*`, and the label of a link or an image. A mark
/// with no closing twin stays as written. Other text gets `base`.
fn inline(mut text: &str, base: Ink, out: &mut Vec<(char, Ink)>) {
    while let Some(ch) = text.chars().next() {
        let rest = &text[ch.len_utf8()..];
        if ch == '`'
            && let Some(end) = rest.find('`')
        {
            out.extend(rest[..end].chars().map(|c| (c, Ink::Code)));
            text = &rest[end + 1..];
        } else if let Some(inner) = text.strip_prefix("**")
            && !inner.starts_with(' ')
            && let Some(end) = closing(inner, "**")
        {
            inline(&inner[..end], Ink::Bold, out);
            text = &inner[end + 2..];
        } else if ch == '*'
            && !rest.starts_with([' ', '*'])
            && let Some(end) = closing(rest, "*")
        {
            inline(&rest[..end], Ink::Italic, out);
            text = &rest[end + 1..];
        } else if ch == '!' && rest.strip_prefix('[').and_then(link).is_some() {
            text = rest;
        } else if ch == '['
            && let Some((label, after)) = link(rest)
        {
            inline(label, base, out);
            text = after;
        } else {
            out.push((ch, base));
            text = rest;
        }
    }
}

/// Returns the columns `ch` takes on screen.
fn columns(ch: char) -> usize {
    Span::raw(ch.to_string()).width()
}

/// Wraps one block's `cells` to `width` columns and appends its lines to
/// `out`; lines after the first start with `indent` spaces.
fn wrap(cells: Vec<(char, Ink)>, width: usize, indent: usize, out: &mut Vec<TextLine>) {
    let mut current: Vec<(char, Ink)> = Vec::new();
    let (mut used, mut space) = (0, None);
    for (ch, ink) in cells {
        let wide = columns(ch);
        if used + wide > width {
            let mut rest: Vec<_> = std::iter::repeat_n((' ', Ink::Plain), indent).collect();
            if let Some(at) = space {
                rest.extend(current.split_off(at));
            }
            out.push(runs(&std::mem::replace(&mut current, rest)));
            used = current.iter().map(|(c, _)| columns(*c)).sum();
            space = None;
        }
        current.push((ch, ink));
        used += wide;
        if ch == ' ' {
            space = Some(current.len());
        }
    }
    out.push(runs(&current));
}

/// Joins neighbouring cells of one ink into runs.
fn runs(cells: &[(char, Ink)]) -> TextLine {
    let mut line: TextLine = Vec::new();
    for (ch, ink) in cells {
        match line.last_mut() {
            Some((last, run)) if last == ink => run.push(*ch),
            _ => line.push((*ink, ch.to_string())),
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &TextLine) -> String {
        line.iter().map(|(_, run)| run.as_str()).collect()
    }

    #[test]
    fn reads_blocks_and_inline_marks_and_leaves_the_rest_as_written() {
        let source = "## Why **now**\n\n- a `b` and [docs](https://x.y/z)\n  1. *deep* item\n\
                      > quoted\n---\n```rust\nlet a = *b;\n```\n<!-- hidden -->\n\
                      2 * 3 * 4 and snake_case and a lone ` and ![logo](l.png)\n\
                      - [ ] todo [x](u)\n| a | b |\x1b[31m";
        let lines = render(source, 70);
        let plain: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(
            plain,
            [
                "Why now",
                "",
                "- a b and docs",
                "  1. deep item",
                "| quoted",
                &"-".repeat(70),
                "  let a = *b;",
                "2 * 3 * 4 and snake_case and a lone ` and logo",
                "- [ ] todo x",
                "| a | b |",
            ]
        );
        let run = |s: &str| s.to_owned();
        assert_eq!(
            lines[0],
            [(Ink::Heading, run("Why ")), (Ink::Bold, run("now"))]
        );
        assert_eq!(
            lines[2],
            [
                (Ink::Muted, run("- ")),
                (Ink::Plain, run("a ")),
                (Ink::Code, run("b")),
                (Ink::Plain, run(" and docs")),
            ]
        );
        assert_eq!(lines[3][1], (Ink::Italic, run("deep")));
        assert_eq!(lines[6], [(Ink::Code, run("  let a = *b;"))]);
    }

    #[test]
    fn stars_brackets_and_fences_that_are_not_marks_stay_as_written() {
        for (source, want) in [
            ("pass *args and **kwargs", "pass *args and **kwargs"),
            ("**kwargs and **opts", "**kwargs and **opts"),
            ("a = *b * c", "a = *b * c"),
            ("[![ci](https://img/x.svg)](https://ci/run) ok", "ci ok"),
            ("[Foo](https://w.org/Foo_(bar)) end", "Foo end"),
            ("```one line```", "one line"),
        ] {
            let lines = render(source, 70);
            assert_eq!(text(&lines[0]), want, "{source}");
        }
        let nested = "````md\n```rust\ncode\n```\n````\nafter";
        let plain: Vec<String> = render(nested, 70).iter().map(text).collect();
        assert_eq!(plain, ["  ```rust", "  code", "  ```", "after"]);
    }

    #[test]
    fn wraps_at_spaces_with_a_hanging_indent_and_wide_characters_as_two_columns() {
        let source = format!("- {} tail\n\n{}", "a".repeat(66), "漢".repeat(40));
        let lines = render(&source, 70);
        let plain: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(plain[0], format!("- {} ", "a".repeat(66)));
        assert_eq!((plain[1].as_str(), plain[2].as_str()), ("  tail", ""));
        let widths: Vec<usize> = plain[3..]
            .iter()
            .map(|l| Span::raw(l.as_str()).width())
            .collect();
        assert_eq!(widths, [70, 10]);
    }
}
