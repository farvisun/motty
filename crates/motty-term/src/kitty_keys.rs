//! The kitty keyboard protocol
//! (<https://sw.kovidgoyal.net/kitty/keyboard-protocol/>), for programs that
//! push flags with `CSI > flags u`. The Term keeps the flag stacks; this
//! turns a key into `CSI code[:shifted[:base]] ; mods[:event] ; text u` or
//! the protocol's functional forms (`CSI 1 ; mods A`, `CSI n ; mods ~`).
//!
//! Keys the flags leave alone (plain text, Enter/Tab/Backspace without
//! modifiers, everything when only alternate keys are asked for) keep the
//! classic encoding, so a shell gets usable keys if the program dies.

use alacritty_terminal::term::TermMode;

use crate::input::{self, KeyInput, KeyKind, MODIFIER_KEYS};

const SHIFT: u32 = 1;
const ALT: u32 = 2;
const CTRL: u32 = 4;
const SUPER: u32 = 8;
const CAPS_LOCK: u32 = 64;

/// How a key's escape code ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Final {
    /// `CSI 1 ; mods X`, or `CSI X` without modifiers.
    Letter(char),
    /// `CSI n ; mods ~`.
    Tilde(u32),
    /// `CSI n ; mods u`.
    U(u32),
}

pub fn encode(input: &KeyInput, mode: TermMode, option_as_meta: bool) -> Option<Vec<u8>> {
    let all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let disambiguate = all || mode.contains(TermMode::DISAMBIGUATE_ESC_CODES);
    let events = mode.contains(TermMode::REPORT_EVENT_TYPES);
    let kind = match input.kind {
        KeyKind::Repeat if !events => KeyKind::Press,
        kind => kind,
    };
    if kind == KeyKind::Release && !events {
        return None;
    }
    let m = input.mods;
    let locks = if m.caps_lock { CAPS_LOCK } else { 0 };

    if let Some(code) = modifier_code(input.key) {
        if !all {
            return None;
        }
        // The state after this event: a released Shift has no Shift bit.
        let bits =
            bit(m.shift, SHIFT) | bit(m.alt, ALT) | bit(m.ctrl, CTRL) | bit(m.cmd, SUPER) | locks;
        return Some(seq(Final::U(code), None, bits, kind, None));
    }
    // ⌘ chords are Motty's shortcuts.
    if m.cmd {
        return None;
    }
    let alt = m.alt && !input.alt_consumed(option_as_meta);
    let bits = bit(m.shift, SHIFT) | bit(alt, ALT) | bit(m.ctrl, CTRL);
    // Sent the classic way: a release has nothing to report.
    let classic = || {
        if kind == KeyKind::Release {
            None
        } else {
            input::legacy(input, mode, option_as_meta)
        }
    };
    let text = || {
        (all && mode.contains(TermMode::REPORT_ASSOCIATED_TEXT) && kind != KeyKind::Release)
            .then(|| input.text())
            .flatten()
    };

    if input.keypad
        && let Some(code) = keypad_code(input.key)
    {
        // Keypad digits and operators type text, like the main keys.
        let types_text = input.key != "enter" && bits & !SHIFT == 0;
        if !disambiguate || (types_text && !all) {
            return classic();
        }
        return Some(seq(Final::U(code), None, bits | locks, kind, text()));
    }

    if let Some(f) = functional(input.key) {
        // Enter, Tab and Backspace stay classic unless modified, and Esc
        // until disambiguated, so `reset` can still be typed after a crash.
        let c0 = matches!(input.key, "enter" | "tab" | "backspace");
        if !all
            && ((c0 && (bits == 0 || !disambiguate)) || (input.key == "escape" && !disambiguate))
        {
            return classic();
        }
        return Some(seq(f, None, bits | locks, kind, None));
    }

    let Some(code) = input.char_key() else {
        return classic();
    };
    // Plain and shifted text is text, and other modifiers wait for
    // disambiguation.
    if !all && (!disambiguate || bits & !SHIFT == 0) {
        return classic();
    }
    let alternates = mode.contains(TermMode::REPORT_ALTERNATE_KEYS).then(|| {
        let shifted = input.shifted_char().filter(|&s| m.shift && s != code);
        let base = input.base_layout.filter(|&b| b != code);
        (shifted, base)
    });
    // Lock modifiers are left off text keys so classic programs still
    // understand them; reporting every key reports them too.
    let locks = if all { locks } else { 0 };
    Some(seq(
        Final::U(code as u32),
        alternates,
        bits | locks,
        kind,
        text(),
    ))
}

fn bit(on: bool, value: u32) -> u32 {
    if on { value } else { 0 }
}

/// `CSI number[:alternates] ; mods[:event] ; text final`, leaving out what
/// isn't needed.
fn seq(
    f: Final,
    alternates: Option<(Option<char>, Option<char>)>,
    bits: u32,
    kind: KeyKind,
    text: Option<&str>,
) -> Vec<u8> {
    let event = match kind {
        KeyKind::Press => "",
        KeyKind::Repeat => ":2",
        KeyKind::Release => ":3",
    };
    let mods = if bits != 0 || !event.is_empty() {
        Some(format!("{}{event}", bits + 1))
    } else {
        text.map(|_| String::new())
    };
    let (number, end) = match f {
        Final::Letter(c) => match &mods {
            None => return format!("\x1b[{c}").into_bytes(),
            Some(_) => (1, c),
        },
        Final::Tilde(n) => (n, '~'),
        Final::U(n) => (n, 'u'),
    };
    let mut out = format!("\x1b[{number}");
    match alternates {
        Some((Some(shifted), Some(base))) => out += &format!(":{}:{}", shifted as u32, base as u32),
        Some((Some(shifted), None)) => out += &format!(":{}", shifted as u32),
        Some((None, Some(base))) => out += &format!("::{}", base as u32),
        _ => {}
    }
    if let Some(mods) = mods {
        out.push(';');
        out += &mods;
    }
    if let Some(text) = text {
        out.push(';');
        let codes: Vec<String> = text.chars().map(|c| (c as u32).to_string()).collect();
        out += &codes.join(":");
    }
    out.push(end);
    out.into_bytes()
}

/// The protocol's number for a modifier key.
fn modifier_code(key: &str) -> Option<u32> {
    let i = MODIFIER_KEYS.iter().position(|k| *k == key)?;
    Some(match MODIFIER_KEYS[i] {
        "caps_lock" => 57358,
        // left_shift … left_super, then right_shift … right_super.
        _ if i < 4 => 57441 + i as u32,
        _ => 57447 + (i - 4) as u32,
    })
}

/// Keys that don't type text.
fn functional(key: &str) -> Option<Final> {
    Some(match key {
        "escape" => Final::U(27),
        "enter" => Final::U(13),
        "tab" => Final::U(9),
        "backspace" => Final::U(127),
        "insert" => Final::Tilde(2),
        "delete" => Final::Tilde(3),
        "pageup" => Final::Tilde(5),
        "pagedown" => Final::Tilde(6),
        "up" => Final::Letter('A'),
        "down" => Final::Letter('B'),
        "right" => Final::Letter('C'),
        "left" => Final::Letter('D'),
        "home" => Final::Letter('H'),
        "end" => Final::Letter('F'),
        "f1" => Final::Letter('P'),
        "f2" => Final::Letter('Q'),
        // Not `CSI R`, which is also a cursor position report.
        "f3" => Final::Tilde(13),
        "f4" => Final::Letter('S'),
        k if k.len() >= 2 && k.starts_with('f') => {
            let n: u32 = k[1..].parse().ok()?;
            match n {
                5 => Final::Tilde(15),
                6..=10 => Final::Tilde(11 + n),
                11 | 12 => Final::Tilde(12 + n),
                13..=35 => Final::U(57376 + n - 13),
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// KP_0 … KP_EQUAL, KP_SEPARATOR.
fn keypad_code(key: &str) -> Option<u32> {
    if key == "enter" {
        return Some(57414);
    }
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(match c {
        '0'..='9' => 57399 + (c as u32 - '0' as u32),
        '.' => 57409,
        '/' => 57410,
        '*' => 57411,
        '-' => 57412,
        '+' => 57413,
        '=' => 57415,
        ',' => 57416,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Mods;

    const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
        cmd: false,
        caps_lock: false,
    };
    const SHIFT_M: Mods = Mods {
        shift: true,
        ..NONE
    };
    const ALT_M: Mods = Mods { alt: true, ..NONE };
    const CTRL_M: Mods = Mods { ctrl: true, ..NONE };
    const CTRL_SHIFT: Mods = Mods {
        ctrl: true,
        shift: true,
        ..NONE
    };

    fn flags(bits: u8) -> TermMode {
        let mut mode = TermMode::NONE;
        for (bit, flag) in [
            (1, TermMode::DISAMBIGUATE_ESC_CODES),
            (2, TermMode::REPORT_EVENT_TYPES),
            (4, TermMode::REPORT_ALTERNATE_KEYS),
            (8, TermMode::REPORT_ALL_KEYS_AS_ESC),
            (16, TermMode::REPORT_ASSOCIATED_TEXT),
        ] {
            if bits & bit != 0 {
                mode |= flag;
            }
        }
        mode
    }

    /// The key as GPUI reports it: `a` with ⇧ types `A`, ⌃ types nothing.
    fn key<'a>(key: &'a str, mods: Mods) -> KeyInput<'a> {
        let key_char = if mods.ctrl {
            None
        } else {
            match key {
                "space" => Some(" "),
                "enter" => Some("\n"),
                "tab" => Some("\t"),
                k if k.chars().count() == 1 => Some(if mods.shift {
                    // Enough for the tests' letters.
                    Box::leak(k.to_uppercase().into_boxed_str()) as &str
                } else {
                    k
                }),
                _ => None,
            }
        };
        KeyInput {
            key,
            key_char,
            mods,
            ..Default::default()
        }
    }

    fn enc(input: &KeyInput, bits: u8) -> Option<String> {
        encode(input, flags(bits), true).map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn table() {
        // (key, mods, flags, expected); `None` sends nothing.
        let cases: &[(&str, Mods, u8, Option<&str>)] = &[
            // Disambiguate: text stays text, modified keys become CSI u.
            ("a", NONE, 1, Some("a")),
            ("a", SHIFT_M, 1, Some("A")),
            ("a", CTRL_M, 1, Some("\x1b[97;5u")),
            ("a", ALT_M, 1, Some("\x1b[97;3u")),
            ("a", CTRL_SHIFT, 1, Some("\x1b[97;6u")),
            ("i", CTRL_M, 1, Some("\x1b[105;5u")),
            ("space", CTRL_M, 1, Some("\x1b[32;5u")),
            ("space", NONE, 1, Some(" ")),
            ("escape", NONE, 1, Some("\x1b[27u")),
            ("escape", NONE, 0b10, Some("\x1b")),
            ("escape", SHIFT_M, 1, Some("\x1b[27;2u")),
            ("enter", NONE, 1, Some("\r")),
            ("enter", SHIFT_M, 1, Some("\x1b[13;2u")),
            ("enter", ALT_M, 1, Some("\x1b[13;3u")),
            ("tab", NONE, 1, Some("\t")),
            ("tab", SHIFT_M, 1, Some("\x1b[9;2u")),
            ("backspace", NONE, 1, Some("\x7f")),
            ("backspace", CTRL_M, 1, Some("\x1b[127;5u")),
            // Functional keys.
            ("up", NONE, 1, Some("\x1b[A")),
            ("up", SHIFT_M, 1, Some("\x1b[1;2A")),
            ("home", CTRL_M, 1, Some("\x1b[1;5H")),
            ("f1", NONE, 1, Some("\x1b[P")),
            ("f3", NONE, 1, Some("\x1b[13~")),
            ("f4", ALT_M, 1, Some("\x1b[1;3S")),
            ("f5", NONE, 1, Some("\x1b[15~")),
            ("f12", CTRL_M, 1, Some("\x1b[24;5~")),
            ("f13", NONE, 1, Some("\x1b[57376u")),
            ("f35", NONE, 1, Some("\x1b[57398u")),
            ("delete", NONE, 1, Some("\x1b[3~")),
            ("pagedown", SHIFT_M, 1, Some("\x1b[6;2~")),
            // Report all keys: everything is an escape code.
            ("a", NONE, 8, Some("\x1b[97u")),
            ("a", SHIFT_M, 8, Some("\x1b[97;2u")),
            ("enter", NONE, 8, Some("\x1b[13u")),
            ("tab", NONE, 8, Some("\x1b[9u")),
            ("backspace", NONE, 8, Some("\x1b[127u")),
            ("escape", NONE, 8, Some("\x1b[27u")),
            ("up", NONE, 8, Some("\x1b[A")),
            // …with the text (the spec's `shift+a -> CSI 97 ; 2 ; 65 u`).
            ("a", SHIFT_M, 8 | 16, Some("\x1b[97;2;65u")),
            ("a", NONE, 8 | 16, Some("\x1b[97;;97u")),
            ("a", CTRL_M, 8 | 16, Some("\x1b[97;5u")),
            ("enter", NONE, 8 | 16, Some("\x1b[13u")),
            // Alternate keys: the shifted key only with Shift.
            ("a", CTRL_SHIFT, 1 | 4, Some("\x1b[97:65;6u")),
            ("a", CTRL_M, 1 | 4, Some("\x1b[97;5u")),
            ("a", SHIFT_M, 8 | 4, Some("\x1b[97:65;2u")),
            ("a", SHIFT_M, 1 | 4, Some("A")),
            // Event types alone don't change text or C0 keys.
            ("a", NONE, 2, Some("a")),
            ("a", CTRL_M, 2, Some("\x01")),
            ("up", NONE, 2, Some("\x1b[A")),
            // Alternate keys alone change nothing.
            ("a", CTRL_M, 4, Some("\x01")),
        ];
        for &(k, mods, bits, want) in cases {
            assert_eq!(
                enc(&key(k, mods), bits).as_deref(),
                want,
                "{k} {mods:?} flags {bits}"
            );
        }
    }

    #[test]
    fn events() {
        let with = |k: &'static str, mods, kind| KeyInput {
            kind,
            ..key(k, mods)
        };
        use KeyKind::*;
        let cases: &[(KeyInput, u8, Option<&str>)] = &[
            (with("a", CTRL_M, Press), 1 | 2, Some("\x1b[97;5u")),
            (with("a", CTRL_M, Repeat), 1 | 2, Some("\x1b[97;5:2u")),
            (with("a", CTRL_M, Release), 1 | 2, Some("\x1b[97;5:3u")),
            (with("a", CTRL_M, Release), 1, None),
            (with("a", CTRL_M, Repeat), 1, Some("\x1b[97;5u")),
            // Text has no release unless every key is reported.
            (with("a", NONE, Release), 1 | 2, None),
            (with("a", NONE, Release), 8 | 2, Some("\x1b[97;1:3u")),
            (with("a", NONE, Repeat), 8 | 2, Some("\x1b[97;1:2u")),
            (
                with("a", SHIFT_M, Release),
                8 | 2 | 16,
                Some("\x1b[97;2:3u"),
            ),
            (with("up", NONE, Release), 1 | 2, Some("\x1b[1;1:3A")),
            (with("up", NONE, Repeat), 1 | 2, Some("\x1b[1;1:2A")),
            (with("escape", NONE, Release), 1 | 2, Some("\x1b[27;1:3u")),
            (with("f5", NONE, Release), 1 | 2, Some("\x1b[15;1:3~")),
            // Enter, Tab and Backspace release only with every key.
            (with("enter", NONE, Release), 1 | 2, None),
            (with("enter", NONE, Release), 8 | 2, Some("\x1b[13;1:3u")),
            (with("backspace", NONE, Release), 1 | 2, None),
        ];
        for (input, bits, want) in cases {
            assert_eq!(
                enc(input, *bits).as_deref(),
                *want,
                "{} {:?} {:?} flags {bits}",
                input.key,
                input.mods,
                input.kind
            );
        }
    }

    #[test]
    fn modifier_keys_only_with_every_key() {
        let press = KeyInput {
            key: "left_shift",
            mods: SHIFT_M,
            ..Default::default()
        };
        assert_eq!(enc(&press, 1), None);
        assert_eq!(enc(&press, 8).as_deref(), Some("\x1b[57441;2u"));
        let release = KeyInput {
            key: "left_shift",
            kind: KeyKind::Release,
            ..Default::default()
        };
        assert_eq!(enc(&release, 8), None);
        assert_eq!(enc(&release, 8 | 2).as_deref(), Some("\x1b[57441;1:3u"));
        let right_ctrl = KeyInput {
            key: "right_control",
            mods: CTRL_M,
            ..Default::default()
        };
        assert_eq!(enc(&right_ctrl, 8).as_deref(), Some("\x1b[57448;5u"));
        let cmd = KeyInput {
            key: "left_super",
            mods: Mods { cmd: true, ..NONE },
            ..Default::default()
        };
        assert_eq!(enc(&cmd, 8).as_deref(), Some("\x1b[57444;9u"));
        let caps = KeyInput {
            key: "caps_lock",
            mods: Mods {
                caps_lock: true,
                ..NONE
            },
            ..Default::default()
        };
        assert_eq!(enc(&caps, 8).as_deref(), Some("\x1b[57358;65u"));
    }

    #[test]
    fn lock_modifiers() {
        let caps = Mods {
            caps_lock: true,
            ..NONE
        };
        // Not on text keys, so they stay text.
        let a = KeyInput {
            key_char: Some("A"),
            ..key("a", caps)
        };
        assert_eq!(enc(&a, 1).as_deref(), Some("A"));
        assert_eq!(
            enc(
                &KeyInput {
                    mods: Mods { ctrl: true, ..caps },
                    key_char: None,
                    ..a
                },
                1
            )
            .as_deref(),
            Some("\x1b[97;5u")
        );
        // On functional keys, and on every key when all are reported.
        assert_eq!(enc(&key("up", caps), 1).as_deref(), Some("\x1b[1;65A"));
        assert_eq!(enc(&a, 8).as_deref(), Some("\x1b[97;65u"));
        // Enter still types a return.
        assert_eq!(enc(&key("enter", caps), 1).as_deref(), Some("\r"));
    }

    #[test]
    fn alternate_and_base_layout_keys() {
        // ⌃⇧1 on a US layout: GPUI says `!`, the platform says `1`/`!`.
        let bang = KeyInput {
            key: "!",
            mods: CTRL_SHIFT,
            unshifted: Some('1'),
            shifted: Some('!'),
            base_layout: Some('1'),
            ..Default::default()
        };
        assert_eq!(enc(&bang, 1).as_deref(), Some("\x1b[49;6u"));
        assert_eq!(enc(&bang, 1 | 4).as_deref(), Some("\x1b[49:33;6u"));
        // ⌃С on a Russian layout is ⌃c on the US one.
        let es = KeyInput {
            key: "c",
            mods: CTRL_M,
            unshifted: Some('с'),
            shifted: Some('С'),
            base_layout: Some('c'),
            ..Default::default()
        };
        assert_eq!(enc(&es, 1 | 4).as_deref(), Some("\x1b[1089::99;5u"));
        assert_eq!(
            enc(
                &KeyInput {
                    mods: CTRL_SHIFT,
                    ..es
                },
                1 | 4
            )
            .as_deref(),
            Some("\x1b[1089:1057:99;6u")
        );
    }

    #[test]
    fn option_typing_a_character_is_text() {
        // Option isn't Meta: ⌥e types é, which is text, not alt+e.
        let input = KeyInput {
            key: "e",
            key_char: Some("é"),
            mods: ALT_M,
            ..Default::default()
        };
        let out = |bits| encode(&input, flags(bits), false).map(|b| String::from_utf8(b).unwrap());
        assert_eq!(out(1).as_deref(), Some("é"));
        assert_eq!(out(8 | 16).as_deref(), Some("\x1b[101;;233u"));
        // As Meta it's alt+e.
        assert_eq!(enc(&input, 1).as_deref(), Some("\x1b[101;3u"));
    }

    #[test]
    fn keypad() {
        let kp = |k: &'static str, mods| KeyInput {
            keypad: true,
            ..key(k, mods)
        };
        assert_eq!(enc(&kp("1", NONE), 1).as_deref(), Some("1"));
        assert_eq!(enc(&kp("1", NONE), 8).as_deref(), Some("\x1b[57400u"));
        assert_eq!(
            enc(&kp("1", NONE), 8 | 16).as_deref(),
            Some("\x1b[57400;;49u")
        );
        assert_eq!(enc(&kp("1", CTRL_M), 1).as_deref(), Some("\x1b[57400;5u"));
        assert_eq!(enc(&kp("enter", NONE), 1).as_deref(), Some("\x1b[57414u"));
        assert_eq!(enc(&kp("+", NONE), 8).as_deref(), Some("\x1b[57413u"));
        assert_eq!(enc(&kp("=", NONE), 8).as_deref(), Some("\x1b[57415u"));
        // Without disambiguation the keypad is classic.
        assert_eq!(enc(&kp("enter", NONE), 2).as_deref(), Some("\r"));
    }

    #[test]
    fn command_chords_are_motty_s() {
        let input = key("c", Mods { cmd: true, ..NONE });
        for bits in [1, 8, 31] {
            assert_eq!(enc(&input, bits), None);
        }
    }
}
