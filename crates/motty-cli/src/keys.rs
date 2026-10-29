//! What `pane send-keys` and `pane send-text` turn their arguments into:
//! the bytes a key press or typed text sends.

/// `send-keys` arguments: text as is, `key:<name>` tokens as the key, all
/// run together (`"echo hi" key:Enter`). Names: Enter, Tab, BTab, Escape,
/// Space, BSpace, Delete, Insert, Up, Down, Left, Right, Home, End, PageUp,
/// PageDown, F1–F12, `C-<key>` with Control and `M-<key>` with Meta (Esc
/// first), case ignored.
pub fn send_keys(args: &[String]) -> Result<String, String> {
    let mut out = String::new();
    for arg in args {
        match arg.strip_prefix("key:") {
            Some(name) => out.push_str(&key(name)?),
            None => out.push_str(arg),
        }
    }
    Ok(out)
}

/// One key name's bytes.
pub fn key(name: &str) -> Result<String, String> {
    if let Some(rest) = strip_mod(name, 'm') {
        return Ok(format!("\x1b{}", key(rest)?));
    }
    if let Some(rest) = strip_mod(name, 'c') {
        return control(rest).ok_or_else(|| format!("no Control key for {name:?}"));
    }
    let named = match name.to_ascii_lowercase().as_str() {
        "enter" | "return" | "cr" => "\r",
        "tab" => "\t",
        "btab" | "s-tab" => "\x1b[Z",
        "escape" | "esc" => "\x1b",
        "space" => " ",
        "bspace" | "backspace" | "bs" => "\x7f",
        "delete" | "del" | "dc" => "\x1b[3~",
        "insert" | "ic" => "\x1b[2~",
        "up" => "\x1b[A",
        "down" => "\x1b[B",
        "right" => "\x1b[C",
        "left" => "\x1b[D",
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "pageup" | "pgup" | "ppage" => "\x1b[5~",
        "pagedown" | "pgdn" | "npage" => "\x1b[6~",
        "f1" => "\x1bOP",
        "f2" => "\x1bOQ",
        "f3" => "\x1bOR",
        "f4" => "\x1bOS",
        "f5" => "\x1b[15~",
        "f6" => "\x1b[17~",
        "f7" => "\x1b[18~",
        "f8" => "\x1b[19~",
        "f9" => "\x1b[20~",
        "f10" => "\x1b[21~",
        "f11" => "\x1b[23~",
        "f12" => "\x1b[24~",
        _ if name.chars().count() == 1 => name,
        _ => return Err(format!("unknown key {name:?}")),
    };
    Ok(named.to_owned())
}

/// `C-x` / `M-x` (either case) without its modifier.
fn strip_mod(name: &str, m: char) -> Option<&str> {
    let mut chars = name.chars();
    let first = chars.next()?.to_ascii_lowercase();
    (first == m && chars.next() == Some('-') && name.len() > 2).then(|| &name[2..])
}

fn control(rest: &str) -> Option<String> {
    let byte = match rest.to_ascii_lowercase().as_str() {
        "space" | "@" | "`" => 0,
        "?" => 0x7f,
        k if k.len() == 1 => {
            let c = k.as_bytes()[0];
            match c {
                b'a'..=b'z' => c - b'a' + 1,
                b'[' => 0x1b,
                b'\\' => 0x1c,
                b']' => 0x1d,
                b'^' | b'~' => 0x1e,
                b'_' | b'/' => 0x1f,
                _ => return None,
            }
        }
        _ => return None,
    };
    Some((byte as char).to_string())
}

/// `send-text`'s C-style escapes: `\n`, `\r`, `\t`, `\e`, `\a`, `\b`,
/// `\0`, `\\`, `\xHH` and `\u{…}`/`\uXXXX`; anything else stays as typed.
pub fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = chars.next() else {
            out.push('\\');
            break;
        };
        let hex = |chars: &mut std::iter::Peekable<std::str::Chars>, max: usize| {
            let mut digits = String::new();
            while digits.len() < max && chars.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                digits.extend(chars.next());
            }
            u32::from_str_radix(&digits, 16)
                .ok()
                .and_then(char::from_u32)
        };
        match e {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'e' | 'E' => out.push('\x1b'),
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            '0' => out.push('\0'),
            '\\' => out.push('\\'),
            'x' => match hex(&mut chars, 2) {
                Some(c) => out.push(c),
                None => out.push_str("\\x"),
            },
            'u' if chars.peek() == Some(&'{') => {
                chars.next();
                let c = hex(&mut chars, 6);
                if chars.peek() == Some(&'}') {
                    chars.next();
                }
                out.extend(c);
            }
            'u' => match hex(&mut chars, 4) {
                Some(c) => out.push(c),
                None => out.push_str("\\u"),
            },
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(args: &[&str]) -> Result<String, String> {
        send_keys(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn text_and_keys_run_together() {
        assert_eq!(keys(&["echo hi", "key:Enter"]).unwrap(), "echo hi\r");
        assert_eq!(keys(&["key:C-c"]).unwrap(), "\x03");
        assert_eq!(keys(&["key:c-a", "key:C-["]).unwrap(), "\x01\x1b");
        assert_eq!(keys(&["key:M-x"]).unwrap(), "\x1bx");
        assert_eq!(keys(&["key:M-C-c"]).unwrap(), "\x1b\x03");
        assert_eq!(
            keys(&["key:up", "key:F5", "key:BTab"]).unwrap(),
            "\x1b[A\x1b[15~\x1b[Z"
        );
        assert_eq!(keys(&["key:Escape", "key:q"]).unwrap(), "\x1bq");
        assert_eq!(keys(&["key:C-Space"]).unwrap(), "\0");
        // Only `key:` tokens are keys.
        assert_eq!(keys(&["Enter"]).unwrap(), "Enter");
        assert!(keys(&["key:Hyper"]).is_err());
        assert!(keys(&["key:C-1"]).is_err());
    }

    #[test]
    fn c_escapes() {
        assert_eq!(unescape(r"git status\n"), "git status\n");
        assert_eq!(unescape(r"\x03"), "\x03");
        assert_eq!(unescape(r"a\tb\\n"), "a\tb\\n");
        assert_eq!(unescape(r"\e[Aé\u{1F600}"), "\x1b[Aé😀");
        assert_eq!(unescape(r"\q\"), "\\q\\");
        assert_eq!(unescape(r"\xZZ"), "\\xZZ");
    }
}
