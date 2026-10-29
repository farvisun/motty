//! xterm's modifyOtherKeys (XTMODKEYS `CSI > 4 ; n m`): modified keys the
//! classic encoding flattens arrive as `CSI 27 ; mods ; code ~`, so ⌃; ⌃.
//! and the like reach Emacs or fish as keys of their own. The code is the
//! character after Shift (⇧⌃a is 65), as xterm sends it.
//!
//! - Level 1 leaves keys with a well-known classic meaning alone: ⌃ or ⌃⇧
//!   on `@`…`~` (control characters), ⌃Space, ⌃2…⌃8 and ⌃/, Shift alone,
//!   and Backspace.
//! - Level 2 sends everything modified except text that Shift alone types
//!   (⇧1 is `!`; ⇧a and ⇧Space still report) and ⌃Backspace.

use alacritty_terminal::term::TermMode;

use crate::input::KeyInput;

/// `None` leaves the key to the classic encoding.
pub fn encode(input: &KeyInput, mode: TermMode, option_as_meta: bool) -> Option<Vec<u8>> {
    let level2 = mode.contains(TermMode::MODIFY_OTHER_KEYS_2);
    let m = input.mods;
    if input.keypad {
        return None;
    }
    let alt = m.alt && !input.alt_consumed(option_as_meta);
    let (shift, ctrl) = (m.shift, m.ctrl);
    if !(shift || alt || ctrl) {
        return None;
    }
    let shift_only = shift && !alt && !ctrl;
    let ctrl_only = ctrl && !alt && !shift;
    let send = |code: u32| {
        let param = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
        Some(format!("\x1b[27;{param};{code}~").into_bytes())
    };
    match input.key {
        "enter" => send(13),
        "tab" => send(9),
        "escape" if level2 || alt => send(27),
        "escape" => None,
        "backspace" if level2 && !ctrl_only => send(127),
        "backspace" => None,
        _ => {
            let base = input.char_key()?;
            let code = if shift { input.shifted_char()? } else { base };
            if level2 {
                // Shift alone types the shifted character, except on letters
                // and Space, which xterm reports.
                if shift_only && !(base.is_alphabetic() || base == ' ') {
                    return None;
                }
                return send(code as u32);
            }
            if alt {
                return send(code as u32);
            }
            if shift_only {
                return None;
            }
            // ⌃ (and ⌃⇧) keep their control characters where there is one.
            if (64..=127).contains(&(code as u32))
                || (ctrl_only && matches!(base, ' ' | '2'..='8' | '/'))
            {
                return None;
            }
            send(code as u32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{self, Mods};

    const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
        cmd: false,
        caps_lock: false,
    };

    fn mods(spec: &str) -> Mods {
        Mods {
            shift: spec.contains('s'),
            alt: spec.contains('a'),
            ctrl: spec.contains('c'),
            ..NONE
        }
    }

    /// Through the full encoder, so classic fallbacks show too.
    fn enc(key: &str, unshifted: char, shifted: char, spec: &str, level: u8) -> String {
        let m = mods(spec);
        let typed = if m.shift { shifted } else { unshifted }.to_string();
        let input = KeyInput {
            key,
            key_char: (!m.ctrl && !m.alt).then_some(typed.as_str()),
            mods: m,
            unshifted: Some(unshifted),
            shifted: Some(shifted),
            ..Default::default()
        };
        let mode = match level {
            1 => TermMode::MODIFY_OTHER_KEYS_1,
            2 => TermMode::MODIFY_OTHER_KEYS_2,
            _ => TermMode::NONE,
        };
        let bytes = input::encode(&input, mode, true).unwrap_or_default();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn levels_follow_xterm() {
        // (key, unshifted, shifted, mods, level 0, level 1, level 2), from
        // xterm's tables (vte's modifyOtherKeys-example.txt).
        let cases: &[(&str, char, char, &str, &str, &str, &str)] = &[
            ("a", 'a', 'A', "s", "A", "A", "\x1b[27;2;65~"),
            (
                "a",
                'a',
                'A',
                "a",
                "\x1ba",
                "\x1b[27;3;97~",
                "\x1b[27;3;97~",
            ),
            ("a", 'a', 'A', "c", "\x01", "\x01", "\x1b[27;5;97~"),
            ("a", 'a', 'A', "sc", "\x01", "\x01", "\x1b[27;6;65~"),
            (
                "a",
                'a',
                'A',
                "ac",
                "\x1b\x01",
                "\x1b[27;7;97~",
                "\x1b[27;7;97~",
            ),
            (";", ';', ':', "c", "", "\x1b[27;5;59~", "\x1b[27;5;59~"),
            (";", ';', ':', "s", ":", ":", ":"),
            (".", '.', '>', "c", "", "\x1b[27;5;46~", "\x1b[27;5;46~"),
            ("/", '/', '?', "c", "\x1f", "\x1f", "\x1b[27;5;47~"),
            (
                "/",
                '/',
                '?',
                "sc",
                "\x1f",
                "\x1b[27;6;63~",
                "\x1b[27;6;63~",
            ),
            ("space", ' ', ' ', "c", "\0", "\0", "\x1b[27;5;32~"),
            ("space", ' ', ' ', "s", " ", " ", "\x1b[27;2;32~"),
            ("1", '1', '!', "c", "", "\x1b[27;5;49~", "\x1b[27;5;49~"),
            ("1", '1', '!', "s", "!", "!", "!"),
            ("2", '2', '@', "c", "\0", "\0", "\x1b[27;5;50~"),
            (
                "3",
                '3',
                '#',
                "sc",
                "\x1b",
                "\x1b[27;6;35~",
                "\x1b[27;6;35~",
            ),
        ];
        for &(key, u, s, spec, l0, l1, l2) in cases {
            assert_eq!(enc(key, u, s, spec, 0), l0, "{key} {spec} level 0");
            assert_eq!(enc(key, u, s, spec, 1), l1, "{key} {spec} level 1");
            assert_eq!(enc(key, u, s, spec, 2), l2, "{key} {spec} level 2");
        }
    }

    #[test]
    fn c0_keys() {
        let named = |key: &str, spec: &str, level: u8| {
            let input = KeyInput {
                key,
                mods: mods(spec),
                ..Default::default()
            };
            let mode = if level == 1 {
                TermMode::MODIFY_OTHER_KEYS_1
            } else {
                TermMode::MODIFY_OTHER_KEYS_2
            };
            String::from_utf8(input::encode(&input, mode, true).unwrap()).unwrap()
        };
        assert_eq!(named("enter", "c", 1), "\x1b[27;5;13~");
        assert_eq!(named("enter", "s", 1), "\x1b[27;2;13~");
        assert_eq!(named("tab", "c", 1), "\x1b[27;5;9~");
        assert_eq!(named("escape", "c", 1), "\x1b");
        assert_eq!(named("escape", "a", 1), "\x1b[27;3;27~");
        assert_eq!(named("escape", "c", 2), "\x1b[27;5;27~");
        assert_eq!(named("backspace", "a", 1), "\x1b\x7f");
        assert_eq!(named("backspace", "a", 2), "\x1b[27;3;127~");
        assert_eq!(named("backspace", "c", 2), "\x08");
        // Keys with modified forms of their own keep them.
        assert_eq!(named("up", "c", 2), "\x1b[1;5A");
        assert_eq!(named("enter", "", 2), "\r");
    }
}
