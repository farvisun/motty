//! Key event → bytes for the PTY, following xterm's encoding.

use alacritty_terminal::term::TermMode;

#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub cmd: bool,
}

impl Mods {
    /// xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }

    fn any(self) -> bool {
        self.shift || self.alt || self.ctrl
    }
}

pub struct KeyInput<'a> {
    /// Key name as GPUI reports it (`"a"`, `"enter"`, `"up"`, `"f5"`, ...).
    pub key: &'a str,
    /// Text the key would insert with the current layout, if any.
    pub key_char: Option<&'a str>,
    pub mods: Mods,
}

/// Returns the bytes to send, or `None` if the key isn't for the terminal
/// (command shortcuts, bare modifiers).
pub fn encode(input: &KeyInput, mode: TermMode, option_as_meta: bool) -> Option<Vec<u8>> {
    let m = input.mods;
    if m.cmd {
        return None;
    }
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let csi_mod = |final_byte: char| format!("\x1b[1;{}{final_byte}", m.param()).into_bytes();
    let tilde = |code: u8| {
        if m.any() { format!("\x1b[{code};{}~", m.param()) } else { format!("\x1b[{code}~") }.into_bytes()
    };
    let cursor_key = |final_byte: char| {
        if m.any() {
            csi_mod(final_byte)
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let alt_prefix = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if m.alt && option_as_meta {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        out
    };

    let bytes = match input.key {
        "enter" => alt_prefix(b"\r"),
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => alt_prefix(b"\t"),
        "backspace" if m.ctrl => alt_prefix(b"\x08"),
        "backspace" => alt_prefix(b"\x7f"),
        "escape" => alt_prefix(b"\x1b"),
        "space" if m.ctrl => alt_prefix(b"\x00"),
        "up" => cursor_key('A'),
        "down" => cursor_key('B'),
        "right" => cursor_key('C'),
        "left" => cursor_key('D'),
        "home" => cursor_key('H'),
        "end" => cursor_key('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" | "f2" | "f3" | "f4" => {
            let final_byte = (b'P' + (input.key.as_bytes()[1] - b'1')) as char;
            if m.any() { csi_mod(final_byte) } else { format!("\x1bO{final_byte}").into_bytes() }
        }
        k if k.len() >= 2 && k.starts_with('f') && k[1..].parse::<u8>().is_ok() => {
            let n: u8 = k[1..].parse().unwrap();
            let code = match n {
                5 => 15,
                6..=10 => 11 + n,
                11 | 12 => 12 + n,
                13..=20 => [25, 26, 28, 29, 31, 32, 33, 34][(n - 13) as usize],
                _ => return None,
            };
            tilde(code)
        }
        key if m.ctrl && !m.cmd => {
            let c = key.chars().next()?;
            if key.chars().count() != 1 {
                return None;
            }
            let byte = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 - b'a' + 1,
                '@' | '2' | ' ' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '7' | '/' | '-' => 0x1f,
                '8' | '?' => 0x7f,
                _ => return input.key_char.map(|s| alt_prefix(s.as_bytes())),
            };
            alt_prefix(&[byte])
        }
        key => {
            // Option as Meta: send ESC + the unmodified key rather than the
            // layout's option character (e.g. alt-b → ESC b, not ∫).
            if m.alt && option_as_meta && key.chars().count() == 1 {
                let base = if m.shift { key.to_uppercase() } else { key.to_owned() };
                alt_prefix(base.as_bytes())
            } else {
                input.key_char?.as_bytes().to_vec()
            }
        }
    };
    Some(bytes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    /// Motion with a button held (or any motion in all-motion mode).
    Drag,
}

/// Mouse event report for applications that enabled mouse tracking.
/// `col`/`row` are zero-based viewport cells. Returns `None` when the
/// current mode doesn't ask for this kind of event.
pub fn mouse_report(button: MouseButton, action: MouseAction, col: usize, row: usize, mods: Mods, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    if action == MouseAction::Drag && !mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
        return None;
    }
    let mut code: u32 = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    if action == MouseAction::Drag {
        code += 32;
    }
    code += 4 * mods.shift as u32 + 8 * mods.alt as u32 + 16 * mods.ctrl as u32;
    let (x, y) = (col + 1, row + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if action == MouseAction::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{suffix}").into_bytes());
    }
    // Legacy X10 encoding: releases are reported as button 3, coordinates cap at 223.
    if action == MouseAction::Release {
        code = (code & !0b11) | 3;
    }
    if x > 223 || y > 223 {
        return None;
    }
    Some(vec![0x1b, b'[', b'M', 32 + code as u8, 32 + x as u8, 32 + y as u8])
}

/// Text to send for a paste, honoring bracketed-paste mode. Escape bytes are
/// stripped inside brackets so pasted content can't terminate the paste early.
pub fn paste_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let mut out = b"\x1b[200~".to_vec();
        out.extend(text.replace('\x1b', "").bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(key: &str, key_char: Option<&str>, mods: Mods, mode: TermMode) -> Option<Vec<u8>> {
        encode(&KeyInput { key, key_char, mods }, mode, true)
    }

    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false, cmd: false };

    #[test]
    fn printable_and_control() {
        assert_eq!(enc("a", Some("a"), NONE, TermMode::NONE).unwrap(), b"a");
        assert_eq!(enc("ب", Some("ب"), NONE, TermMode::NONE).unwrap(), "ب".as_bytes());
        assert_eq!(enc("c", Some("c"), Mods { ctrl: true, ..NONE }, TermMode::NONE).unwrap(), b"\x03");
        assert_eq!(enc("b", Some("∫"), Mods { alt: true, ..NONE }, TermMode::NONE).unwrap(), b"\x1bb");
        assert_eq!(enc("enter", None, NONE, TermMode::NONE).unwrap(), b"\r");
        assert_eq!(enc("backspace", None, NONE, TermMode::NONE).unwrap(), b"\x7f");
        assert!(enc("c", Some("c"), Mods { cmd: true, ..NONE }, TermMode::NONE).is_none());
    }

    #[test]
    fn cursor_keys_follow_mode_and_mods() {
        assert_eq!(enc("up", None, NONE, TermMode::NONE).unwrap(), b"\x1b[A");
        assert_eq!(enc("up", None, NONE, TermMode::APP_CURSOR).unwrap(), b"\x1bOA");
        assert_eq!(enc("left", None, Mods { alt: true, ..NONE }, TermMode::NONE).unwrap(), b"\x1b[1;3D");
        assert_eq!(enc("tab", None, Mods { shift: true, ..NONE }, TermMode::NONE).unwrap(), b"\x1b[Z");
    }

    #[test]
    fn mouse_and_paste() {
        let sgr = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(mouse_report(MouseButton::Left, MouseAction::Press, 0, 0, NONE, sgr).unwrap(), b"\x1b[<0;1;1M");
        assert_eq!(mouse_report(MouseButton::Left, MouseAction::Release, 4, 2, NONE, sgr).unwrap(), b"\x1b[<0;5;3m");
        assert!(mouse_report(MouseButton::Left, MouseAction::Drag, 0, 0, NONE, sgr).is_none());
        assert!(mouse_report(MouseButton::Left, MouseAction::Press, 0, 0, NONE, TermMode::NONE).is_none());
        assert_eq!(
            mouse_report(MouseButton::Left, MouseAction::Release, 0, 0, NONE, TermMode::MOUSE_REPORT_CLICK).unwrap(),
            vec![0x1b, b'[', b'M', 35, 33, 33]
        );
        assert_eq!(paste_bytes("a\nb", TermMode::NONE), b"a\rb");
        assert_eq!(paste_bytes("x\x1by", TermMode::BRACKETED_PASTE), b"\x1b[200~xy\x1b[201~");
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc("f1", None, NONE, TermMode::NONE).unwrap(), b"\x1bOP");
        assert_eq!(enc("f5", None, NONE, TermMode::NONE).unwrap(), b"\x1b[15~");
        assert_eq!(enc("f12", None, NONE, TermMode::NONE).unwrap(), b"\x1b[24~");
        assert_eq!(enc("delete", None, Mods { ctrl: true, ..NONE }, TermMode::NONE).unwrap(), b"\x1b[3;5~");
    }
}
