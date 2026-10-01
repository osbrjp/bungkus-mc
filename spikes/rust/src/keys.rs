//! Key and paste encoding: crossterm key events to the bytes a legacy
//! xterm-style terminal would send.
//!
//! `alacritty_terminal` has no input encoder (it lives in the alacritty
//! binary), so this table is ours. It never talks to a PTY; callers write
//! the returned bytes.

use alacritty_terminal::term::TermMode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Parses a harness key name (`shift+enter`, `ctrl+c`, …) into a key event.
///
/// # Arguments
///
/// * `name` - Modifiers joined with `+`, then the key, e.g. `shift+tab`.
///
/// # Returns
///
/// `None` if a modifier or key name is unknown.
#[must_use]
pub fn parse(name: &str) -> Option<KeyEvent> {
    let (mods, key) = name.rsplit_once('+').unwrap_or(("", name));
    let mut m = KeyModifiers::NONE;
    for part in mods.split('+').filter(|p| !p.is_empty()) {
        m |= match part {
            "shift" => KeyModifiers::SHIFT,
            "ctrl" => KeyModifiers::CONTROL,
            "alt" => KeyModifiers::ALT,
            _ => return None,
        };
    }
    let code = match key {
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" if m.contains(KeyModifiers::SHIFT) => {
            m.remove(KeyModifiers::SHIFT);
            KeyCode::BackTab
        }
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        k if k.chars().count() == 1 => KeyCode::Char(k.chars().next()?),
        _ => return None,
    };
    Some(KeyEvent::new(code, m))
}

/// Encodes one key press as PTY input bytes.
///
/// Arrows and Home/End honour DECCKM (`APP_CURSOR`). `shift+enter` and
/// `alt+enter` send `ESC CR`, which Claude Code treats as a newline.
/// The kitty keyboard protocol is not produced: the emulator is configured
/// not to advertise it, so agents fall back to legacy input.
///
/// # Arguments
///
/// * `key`  - The key event from crossterm or [`parse`].
/// * `mode` - The emulator's current mode flags.
///
/// # Returns
///
/// The bytes to write; empty for keys with no legacy encoding.
#[must_use]
pub fn encode(key: &KeyEvent, mode: TermMode) -> Vec<u8> {
    let m = key.modifiers;
    let alt = m.contains(KeyModifiers::ALT);
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let shift = m.contains(KeyModifiers::SHIFT);
    let param = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let app = mode.contains(TermMode::APP_CURSOR);
    let cursor = |c: char| match (param, app) {
        (1, true) => format!("\x1bO{c}").into_bytes(),
        (1, false) => format!("\x1b[{c}").into_bytes(),
        _ => format!("\x1b[1;{param}{c}").into_bytes(),
    };
    let tilde = |n: u8| match param {
        1 => format!("\x1b[{n}~").into_bytes(),
        _ => format!("\x1b[{n};{param}~").into_bytes(),
    };
    let meta = |b: &[u8]| {
        if alt {
            [b"\x1b", b].concat()
        } else {
            b.to_vec()
        }
    };
    match key.code {
        KeyCode::Char(c) if ctrl => meta(&[ctrl_byte(c)]),
        KeyCode::Char(c) => meta(c.encode_utf8(&mut [0; 4]).as_bytes()),
        KeyCode::Enter if shift || alt => b"\x1b\r".to_vec(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Backspace if ctrl => meta(b"\x08"),
        KeyCode::Backspace => meta(b"\x7f"),
        KeyCode::Esc => meta(b"\x1b"),
        KeyCode::Tab if shift => b"\x1b[Z".to_vec(),
        KeyCode::Tab => meta(b"\t"),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => format!("\x1bO{}", char::from(b'P' + n - 1)).into_bytes(),
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]),
        _ => Vec::new(),
    }
}

/// Maps `ctrl+<c>` to its C0 control byte.
///
/// Letters map to 0x01–0x1a; `@`/space to NUL; `[ \ ] ^ _` to 0x1b–0x1f.
/// crossterm reports legacy 0x1c–0x1f as `ctrl+4`…`ctrl+7`, so those map too.
fn ctrl_byte(c: char) -> u8 {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '/' => 0x1f,
        '?' | '8' => 0x7f,
        _ => 0,
    }
}

/// Encodes pasted text for the PTY.
///
/// With bracketed paste (mode 2004) enabled, the text is wrapped in
/// `ESC[200~`…`ESC[201~` and any ESC inside is dropped so the paste cannot
/// end the bracket early. Otherwise newlines become CR, as a real terminal
/// sends them.
///
/// # Arguments
///
/// * `text` - Pasted text.
/// * `mode` - The emulator's current mode flags.
#[must_use]
pub fn paste(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let body: String = text.chars().filter(|&c| c != '\x1b').collect();
        format!("\x1b[200~{body}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_every_harness_key_name() {
        let cases: &[(&str, TermMode, &[u8])] = &[
            ("enter", TermMode::NONE, b"\r"),
            ("shift+enter", TermMode::NONE, b"\x1b\r"),
            ("esc", TermMode::NONE, b"\x1b"),
            ("tab", TermMode::NONE, b"\t"),
            ("shift+tab", TermMode::NONE, b"\x1b[Z"),
            ("up", TermMode::NONE, b"\x1b[A"),
            ("up", TermMode::APP_CURSOR, b"\x1bOA"),
            ("down", TermMode::NONE, b"\x1b[B"),
            ("left", TermMode::NONE, b"\x1b[D"),
            ("right", TermMode::NONE, b"\x1b[C"),
            ("ctrl+c", TermMode::NONE, b"\x03"),
            ("ctrl+d", TermMode::NONE, b"\x04"),
            ("backspace", TermMode::NONE, b"\x7f"),
            ("ctrl+up", TermMode::APP_CURSOR, b"\x1b[1;5A"),
        ];
        for (name, mode, want) in cases {
            let key = parse(name).unwrap_or_else(|| panic!("unparsed {name}"));
            assert_eq!(encode(&key, *mode), *want, "{name}");
        }
        assert!(parse("hyper+x").is_none());
    }

    #[test]
    fn brackets_paste_only_when_enabled() {
        assert_eq!(paste("a\nb", TermMode::NONE), b"a\rb");
        assert_eq!(
            paste("a\x1bb", TermMode::BRACKETED_PASTE),
            b"\x1b[200~ab\x1b[201~"
        );
    }
}
