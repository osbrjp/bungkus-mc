//! Input encoding toward the agent: keys, paste and mouse as the bytes a
//! terminal would send (ARCHITECTURE §4.1).
//!
//! `alacritty_terminal` has no input encoder (it lives in the Alacritty
//! binary), so this is ours. It tracks the modes the agent set in the
//! emulator: DECCKM, bracketed paste, mouse reporting and the kitty
//! keyboard flags. It never talks to a PTY; callers send the bytes to the
//! session's writer thread.

use alacritty_terminal::term::TermMode;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

/// Encodes one key press for the agent.
///
/// Legacy xterm encoding by default: printable → UTF-8, ctrl → C0, alt →
/// ESC prefix, arrows and Home/End in CSI or SS3 form by DECCKM with
/// xterm `;m` modifier params, `shift+enter`/`alt+enter` → `ESC CR`. While
/// the agent has pushed kitty keyboard flags, ambiguous keys (`esc`, and
/// any key with ctrl or alt, or a modified enter/tab/backspace) become
/// `CSI <code>;<mods> u`, and with "report all keys" every key does.
///
/// # Arguments
///
/// * `key`  - The key event from the host terminal.
/// * `mode` - The emulator's current mode flags.
///
/// # Returns
///
/// The bytes to write; empty for keys with no encoding.
#[must_use]
pub(crate) fn encode(key: &KeyEvent, mode: TermMode) -> Vec<u8> {
    let m = key.modifiers;
    let (shift, alt, ctrl) = (
        m.contains(KeyModifiers::SHIFT),
        m.contains(KeyModifiers::ALT),
        m.contains(KeyModifiers::CONTROL),
    );
    let param = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    if let Some(bytes) = kitty(key.code, param, mode) {
        return bytes;
    }
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
        KeyCode::F(n @ 1..=4) if param == 1 => {
            format!("\x1bO{}", char::from(b'P' + n - 1)).into_bytes()
        }
        KeyCode::F(n @ 1..=4) => format!("\x1b[1;{param}{}", char::from(b'P' + n - 1)).into_bytes(),
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]),
        _ => Vec::new(),
    }
}

/// Returns the kitty `CSI u` form of a key when the agent's kitty flags
/// ask for it, else `None` (legacy encoding applies).
///
/// Functional keys (arrows, F-keys, Home/End, …) keep their legacy CSI
/// forms, which the kitty protocol shares.
fn kitty(code: KeyCode, param: u8, mode: TermMode) -> Option<Vec<u8>> {
    let all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    if !all && !mode.contains(TermMode::DISAMBIGUATE_ESC_CODES) {
        return None;
    }
    let modified = param > 1;
    let (number, plain_is_legacy) = match code {
        KeyCode::Esc => (27, false),
        KeyCode::Enter => (13, true),
        KeyCode::Tab => (9, true),
        KeyCode::BackTab => return Some(csi_u(9, param.max(2))),
        KeyCode::Backspace => (127, true),
        KeyCode::Char(c) => {
            // Shift alone produces text; kitty reports the unshifted key.
            let only_shift = param == 2;
            if !all && (!modified || only_shift) {
                return None;
            }
            (u32::from(c.to_ascii_lowercase()), false)
        }
        _ => return None,
    };
    if !all && plain_is_legacy && !modified {
        return None;
    }
    Some(csi_u(number, param))
}

/// Formats `CSI <number> [; <mods>] u`.
fn csi_u(number: u32, param: u8) -> Vec<u8> {
    if param > 1 {
        format!("\x1b[{number};{param}u").into_bytes()
    } else {
        format!("\x1b[{number}u").into_bytes()
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

/// Encodes pasted text for the agent.
///
/// With bracketed paste (mode 2004) enabled, the text is wrapped in
/// `ESC[200~`…`ESC[201~` and any ESC inside is dropped so the paste cannot
/// end the bracket early. Otherwise newlines become CR, as a real terminal
/// sends them.
#[must_use]
pub(crate) fn paste(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let body: String = text.chars().filter(|&c| c != '\x1b').collect();
        format!("\x1b[200~{body}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// Encodes a mouse event for an agent that enabled mouse reporting.
///
/// SGR form (`CSI < b;x;y M/m`) when the agent asked for it, else the
/// legacy X10 form (coordinates capped at 223).
///
/// # Arguments
///
/// * `kind` - What happened.
/// * `col`, `row` - Zero-based cell inside the output pane.
/// * `held` - Held modifiers.
/// * `mode` - The emulator's mode flags.
///
/// # Returns
///
/// `None` when the agent did not ask for this kind of event.
#[must_use]
pub(crate) fn mouse(
    kind: MouseEventKind,
    col: u16,
    row: u16,
    held: KeyModifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let button = |b: MouseButton| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (code, release) = match kind {
        MouseEventKind::Down(b) if mode.intersects(TermMode::MOUSE_MODE) => (button(b), false),
        MouseEventKind::Up(b) if mode.intersects(TermMode::MOUSE_MODE) => (button(b), true),
        MouseEventKind::Drag(b)
            if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) =>
        {
            (button(b) + 32, false)
        }
        MouseEventKind::ScrollUp if mode.intersects(TermMode::MOUSE_MODE) => (64, false),
        MouseEventKind::ScrollDown if mode.intersects(TermMode::MOUSE_MODE) => (65, false),
        _ => return None,
    };
    let code = code
        + 4 * u8::from(held.contains(KeyModifiers::SHIFT))
        + 8 * u8::from(held.contains(KeyModifiers::ALT))
        + 16 * u8::from(held.contains(KeyModifiers::CONTROL));
    let (x, y) = (u32::from(col) + 1, u32::from(row) + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }
    let legacy = if release { 3 } else { code };
    let pos = |v: u32| u8::try_from(v.min(223) + 32).unwrap_or(255);
    Some(vec![0x1b, b'[', b'M', legacy + 32, pos(x), pos(y)])
}

/// A single key chord from config, such as `ctrl-\` (DESIGN §8.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Chord {
    /// The character pressed with ctrl.
    ch: char,
}

impl Chord {
    /// The default exit chord, `ctrl-\`.
    pub(crate) const DEFAULT: Self = Self { ch: '\\' };

    /// Parses `ctrl-<c>`; any other form is `None`.
    #[must_use]
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let rest = text.strip_prefix("ctrl-")?;
        let mut chars = rest.chars();
        let ch = chars.next()?;
        chars.next().is_none().then_some(Self {
            ch: ch.to_ascii_lowercase(),
        })
    }

    /// Returns whether `key` is this chord. Legacy terminals report 0x1c–0x1f
    /// as `ctrl-4`…`ctrl-7`, so those count as `\ ] ^ _`.
    #[must_use]
    pub(crate) fn matches(self, key: &KeyEvent) -> bool {
        let KeyCode::Char(c) = key.code else {
            return false;
        };
        key.modifiers.contains(KeyModifiers::CONTROL) && ctrl_byte(c) == ctrl_byte(self.ch)
    }

    /// Returns how the chord reads in hints.
    #[must_use]
    pub(crate) fn label(self) -> String {
        format!("ctrl-{}", self.ch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn encodes_legacy_keys() {
        use KeyModifiers as M;
        let none = TermMode::NONE;
        let cases: &[(KeyCode, KeyModifiers, TermMode, &[u8])] = &[
            (KeyCode::Enter, M::NONE, none, b"\r"),
            (KeyCode::Enter, M::SHIFT, none, b"\x1b\r"),
            (KeyCode::Esc, M::NONE, none, b"\x1b"),
            (KeyCode::Tab, M::NONE, none, b"\t"),
            (KeyCode::BackTab, M::SHIFT, none, b"\x1b[Z"),
            (KeyCode::Up, M::NONE, none, b"\x1b[A"),
            (KeyCode::Up, M::NONE, TermMode::APP_CURSOR, b"\x1bOA"),
            (KeyCode::Up, M::CONTROL, TermMode::APP_CURSOR, b"\x1b[1;5A"),
            (KeyCode::Char('c'), M::CONTROL, none, b"\x03"),
            (KeyCode::Char('x'), M::ALT, none, b"\x1bx"),
            (KeyCode::Char('é'), M::NONE, none, "é".as_bytes()),
            (KeyCode::Backspace, M::NONE, none, b"\x7f"),
            (KeyCode::Delete, M::NONE, none, b"\x1b[3~"),
            (KeyCode::F(1), M::NONE, none, b"\x1bOP"),
            (KeyCode::F(5), M::SHIFT, none, b"\x1b[15;2~"),
        ];
        for (code, mods, mode, want) in cases {
            assert_eq!(
                encode(&key(*code, *mods), *mode),
                *want,
                "{code:?} {mods:?} {mode:?}"
            );
        }
    }

    #[test]
    fn encodes_kitty_csi_u_while_the_agent_asks_for_it() {
        use KeyModifiers as M;
        let dis = TermMode::DISAMBIGUATE_ESC_CODES;
        let all = TermMode::REPORT_ALL_KEYS_AS_ESC;
        let cases: &[(KeyCode, KeyModifiers, TermMode, &[u8])] = &[
            (KeyCode::Esc, M::NONE, dis, b"\x1b[27u"),
            (KeyCode::Char('c'), M::CONTROL, dis, b"\x1b[99;5u"),
            (
                KeyCode::Char('A'),
                M::SHIFT | M::CONTROL,
                dis,
                b"\x1b[97;6u",
            ),
            (KeyCode::Char('a'), M::NONE, dis, b"a"),
            (KeyCode::Char('A'), M::SHIFT, dis, b"A"),
            (KeyCode::Enter, M::NONE, dis, b"\r"),
            (KeyCode::Enter, M::SHIFT, dis, b"\x1b[13;2u"),
            (KeyCode::BackTab, M::SHIFT, dis, b"\x1b[9;2u"),
            (KeyCode::Up, M::CONTROL, dis, b"\x1b[1;5A"),
            (KeyCode::Char('a'), M::NONE, all, b"\x1b[97u"),
            (KeyCode::Enter, M::NONE, all, b"\x1b[13u"),
        ];
        for (code, mods, mode, want) in cases {
            let got = encode(&key(*code, *mods), *mode);
            assert_eq!(
                got,
                *want,
                "{code:?} {mods:?} {mode:?}: {}",
                String::from_utf8_lossy(&got)
            );
        }
    }

    #[test]
    fn brackets_paste_only_when_enabled() {
        assert_eq!(paste("a\nb", TermMode::NONE), b"a\rb");
        assert_eq!(
            paste("a\x1bb", TermMode::BRACKETED_PASTE),
            b"\x1b[200~ab\x1b[201~"
        );
    }

    #[test]
    fn encodes_mouse_only_when_reporting_is_on() {
        let down = MouseEventKind::Down(MouseButton::Left);
        let none = KeyModifiers::NONE;
        assert_eq!(mouse(down, 0, 0, none, TermMode::NONE), None);
        let sgr = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(mouse(down, 4, 2, none, sgr).unwrap(), b"\x1b[<0;5;3M");
        let up = MouseEventKind::Up(MouseButton::Left);
        assert_eq!(mouse(up, 4, 2, none, sgr).unwrap(), b"\x1b[<0;5;3m");
        let wheel = mouse(MouseEventKind::ScrollUp, 0, 0, none, sgr).unwrap();
        assert_eq!(wheel, b"\x1b[<64;1;1M");
        let legacy = mouse(down, 4, 2, none, TermMode::MOUSE_REPORT_CLICK).unwrap();
        assert_eq!(legacy, [0x1b, b'[', b'M', 32, 37, 35]);
    }

    #[test]
    fn parses_and_matches_exit_chords() {
        let chord = Chord::parse("ctrl-\\").unwrap();
        assert_eq!(chord, Chord::DEFAULT);
        assert!(chord.matches(&key(KeyCode::Char('\\'), KeyModifiers::CONTROL)));
        assert!(
            chord.matches(&key(KeyCode::Char('4'), KeyModifiers::CONTROL)),
            "legacy 0x1c"
        );
        assert!(!chord.matches(&key(KeyCode::Char('\\'), KeyModifiers::NONE)));
        assert!(
            Chord::parse("ctrl-^")
                .unwrap()
                .matches(&key(KeyCode::Char('6'), KeyModifiers::CONTROL))
        );
        assert_eq!(Chord::parse("alt-x"), None);
        assert_eq!(Chord::parse("ctrl-ab"), None);
        assert_eq!(chord.label(), "ctrl-\\");
    }
}
