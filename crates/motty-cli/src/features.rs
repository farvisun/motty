//! `motty features`: self-contained demo sheets for eyeballing what the
//! terminal renders (colors, styles, international text, OSC sequences).

use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

/// One demo in the catalog.
pub struct Feature {
    pub name: &'static str,
    pub description: &'static str,
}

/// The catalog, in the order `features list` prints it.
pub const FEATURES: &[Feature] = &[
    Feature {
        name: "true-color",
        description: "24-bit gradients: hue rainbow, gray and RGB ramps",
    },
    Feature {
        name: "256-colors",
        description: "16 system colors, the 6x6x6 cube and the grayscale ramp",
    },
    Feature {
        name: "styles",
        description: "SGR styles: bold, italic, underline variants, colored underline, strike, overline",
    },
    Feature {
        name: "emoji",
        description: "Single emoji, ZWJ families, skin tones, flags, keycaps, VS16",
    },
    Feature {
        name: "cjk",
        description: "Chinese, Japanese and Korean with a width alignment ruler",
    },
    Feature {
        name: "rtl",
        description: "Arabic, Persian, Hebrew and mixed bidi lines",
    },
    Feature {
        name: "thai",
        description: "Complex scripts: Thai, Devanagari, Tamil, Bengali, combining marks",
    },
    Feature {
        name: "box-drawing",
        description: "Boxes, a table, block elements, shades, braille, powerline glyphs",
    },
    Feature {
        name: "links",
        description: "OSC 8 hyperlinks: plain, grouped by id=, file:// to the current dir",
    },
    Feature {
        name: "progress",
        description: "OSC 9;4 determinate progress bar from 0 to 100%",
    },
    Feature {
        name: "error-state",
        description: "OSC 9;4 error variant, held briefly then cleared",
    },
    Feature {
        name: "indeterminate",
        description: "OSC 9;4 indeterminate (busy) state with a spinner",
    },
    Feature {
        name: "notification",
        description: "Desktop notifications via OSC 9 and OSC 777",
    },
    Feature {
        name: "unicode",
        description: "One-shot sheet: combining marks, widths, emoji clusters, zero-width, astral, RTL, Thai",
    },
];

/// Alternative spellings accepted by [`find`].
const ALIASES: &[(&str, &str)] = &[
    ("truecolor", "true-color"),
    ("24bit", "true-color"),
    ("rgb", "true-color"),
    ("colors", "256-colors"),
    ("colours", "256-colors"),
    ("256", "256-colors"),
    ("style", "styles"),
    ("sgr", "styles"),
    ("emojis", "emoji"),
    ("east-asian", "cjk"),
    ("bidi", "rtl"),
    ("arabic", "rtl"),
    ("hebrew", "rtl"),
    ("complex-scripts", "thai"),
    ("indic", "thai"),
    ("box", "box-drawing"),
    ("boxes", "box-drawing"),
    ("powerline", "box-drawing"),
    ("link", "links"),
    ("hyperlinks", "links"),
    ("osc8", "links"),
    ("osc9", "progress"),
    ("error", "error-state"),
    ("spinner", "indeterminate"),
    ("busy", "indeterminate"),
    ("notify", "notification"),
    ("notifications", "notification"),
    ("smoke", "unicode"),
];

const RESET: &str = "\x1b[0m";
const PROGRESS_CLEAR: &str = "\x1b]9;4;0\x07";

/// Looks a feature up by name or alias, ignoring case.
pub fn find(name: &str) -> Option<&'static Feature> {
    let name = name.trim().to_ascii_lowercase();
    let name = ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name.as_str(), |(_, target)| target);
    FEATURES.iter().find(|f| f.name == name)
}

/// Writes the demo for `feature` to `out`. Animated ones sleep between
/// frames unless `fast`.
pub fn play(feature: &Feature, out: &mut dyn Write, fast: bool) -> io::Result<()> {
    match feature.name {
        "true-color" => true_color(out)?,
        "256-colors" => colors_256(out)?,
        "styles" => styles(out)?,
        "emoji" => emoji(out)?,
        "cjk" => cjk(out)?,
        "rtl" => rtl(out)?,
        "thai" => thai(out)?,
        "box-drawing" => box_drawing(out)?,
        "links" => links(out)?,
        "progress" => progress(out, fast)?,
        "error-state" => error_state(out, fast)?,
        "indeterminate" => indeterminate(out, fast)?,
        "notification" => notification(out, fast)?,
        "unicode" => unicode(out)?,
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown feature: {other}"),
            ));
        }
    }
    out.flush()
}

fn heading(out: &mut dyn Write, title: &str) -> io::Result<()> {
    writeln!(out, "\x1b[1m{title}{RESET}")
}

/// A dim, fixed-width label followed by the sample, reset at the end.
fn row(out: &mut dyn Write, label: &str, content: &str) -> io::Result<()> {
    writeln!(out, "\x1b[2m{label:<14}\x1b[22m {content}{RESET}")
}

fn pause(fast: bool, ms: u64) {
    if !fast {
        thread::sleep(Duration::from_millis(ms));
    }
}

fn hsv(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |n: f32| ((n + m) * 255.0).round() as u8;
    (to(r), to(g), to(b))
}

fn true_color(out: &mut dyn Write) -> io::Result<()> {
    const W: usize = 64;
    heading(
        out,
        "24-bit color (each cell is an upper half-block over its background)",
    )?;
    for line in 0..3 {
        let mut s = String::new();
        for x in 0..W {
            let h = x as f32 / W as f32 * 360.0;
            let (tr, tg, tb) = hsv(h, 1.0, 1.0 - (line * 2) as f32 * 0.15);
            let (br, bg, bb) = hsv(h, 1.0, 1.0 - (line * 2 + 1) as f32 * 0.15);
            s.push_str(&format!(
                "\x1b[38;2;{tr};{tg};{tb}m\x1b[48;2;{br};{bg};{bb}m\u{2580}"
            ));
        }
        row(out, if line == 0 { "hue" } else { "" }, &s)?;
    }
    let ramp = |rgb: fn(u8) -> (u8, u8, u8)| -> String {
        (0..W)
            .map(|x| {
                let (r, g, b) = rgb((x * 255 / (W - 1)) as u8);
                format!("\x1b[48;2;{r};{g};{b}m ")
            })
            .collect()
    };
    row(out, "gray", &ramp(|n| (n, n, n)))?;
    row(out, "red", &ramp(|n| (n, 0, 0)))?;
    row(out, "green", &ramp(|n| (0, n, 0)))?;
    row(out, "blue", &ramp(|n| (0, 0, n)))
}

fn colors_256(out: &mut dyn Write) -> io::Result<()> {
    heading(out, "256 colors (SGR 38;5 / 48;5)")?;
    // Black text on the light system colors, white on the dark ones.
    let fg = |n: u8| {
        if matches!(n, 2 | 3 | 6 | 7 | 10 | 11 | 14 | 15) {
            30
        } else {
            97
        }
    };
    for (label, range) in [("system 0-7", 0..8u8), ("bright 8-15", 8..16)] {
        let cells: String = range
            .map(|n| format!("\x1b[48;5;{n}m\x1b[{}m {n:>3} ", fg(n)))
            .collect();
        row(out, label, &cells)?;
    }
    let fgs: String = (0..16u8)
        .map(|n| format!("\x1b[38;5;{n}m{n:>3} "))
        .collect();
    row(out, "fg 0-15", &fgs)?;
    for r in 0..6u16 {
        let base = 16 + r * 36;
        let cells: String = (base..base + 36)
            .map(|n| format!("\x1b[48;5;{n}m  "))
            .collect();
        row(out, &format!("cube {base}-{}", base + 35), &cells)?;
    }
    let grays: String = (232..=255u8)
        .map(|n| format!("\x1b[48;5;{n}m   "))
        .collect();
    row(out, "gray 232-255", &grays)
}

fn styles(out: &mut dyn Write) -> io::Result<()> {
    const SAMPLE: &str = "The quick brown fox";
    heading(out, "Text styles (SGR)")?;
    let rows = [
        ("bold 1", "1"),
        ("dim 2", "2"),
        ("italic 3", "3"),
        ("underline 4", "4"),
        ("double 4:2", "4:2"),
        ("curly 4:3", "4:3"),
        ("dotted 4:4", "4:4"),
        ("dashed 4:5", "4:5"),
        ("colored 58:2", "4:3;58:2::255:64:64"),
        ("colored 58:5", "4;58:5:33"),
        ("blink 5", "5"),
        ("inverse 7", "7"),
        ("strike 9", "9"),
        ("overline 53", "53"),
        ("bold italic", "1;3"),
    ];
    for (label, sgr) in rows {
        row(out, label, &format!("\x1b[{sgr}m{SAMPLE}"))?;
    }
    // Brackets show where the hidden text sits.
    row(out, "hidden 8", &format!("[\x1b[8m{SAMPLE}{RESET}]"))
}

fn emoji(out: &mut dyn Write) -> io::Result<()> {
    heading(out, "Emoji (each should take two cells)")?;
    row(out, "single", "😀 🚀 🎉 🐱 🍕 🌍")?;
    row(
        out,
        "zwj family",
        "👨\u{200D}👩\u{200D}👧\u{200D}👦 👩\u{200D}💻 🧑\u{200D}🤝\u{200D}🧑 🏳\u{FE0F}\u{200D}🌈",
    )?;
    row(out, "skin tones", "👋 👋🏻 👋🏼 👋🏽 👋🏾 👋🏿")?;
    row(out, "flags", "🇬🇧 🇮🇷 🇯🇵 🇺🇸 🇺🇦 🇧🇷")?;
    row(
        out,
        "keycaps",
        "1\u{FE0F}\u{20E3} 2\u{FE0F}\u{20E3} #\u{FE0F}\u{20E3} *\u{FE0F}\u{20E3}",
    )?;
    row(
        out,
        "text vs VS16",
        "\u{263A} \u{263A}\u{FE0F}  \u{2708} \u{2708}\u{FE0F}  \u{2764} \u{2764}\u{FE0F}",
    )?;
    row(out, "ruler", "|0123456789|")?;
    row(out, "aligned", "|😀🚀🎉🐱🍕|")
}

fn cjk(out: &mut dyn Write) -> io::Result<()> {
    heading(out, "CJK (wide characters take two cells)")?;
    row(out, "Chinese", "你好，世界。繁體中文")?;
    row(out, "Japanese", "こんにちは世界 カタカナ 漢字")?;
    row(out, "halfwidth", "ｶﾀｶﾅ ﾊﾝｶｸ")?;
    row(out, "Korean", "안녕하세요 세계 한국어")?;
    row(out, "ruler", "|0123456789|")?;
    row(out, "ASCII", "|abcdefghij|")?;
    row(out, "Chinese", "|你好世界人|")?;
    row(out, "Japanese", "|ひらがなだ|")?;
    row(out, "Korean", "|한국어문자|")?;
    row(out, "mixed", "|ab你好cd한|")
}

fn rtl(out: &mut dyn Write) -> io::Result<()> {
    heading(out, "Right-to-left text")?;
    row(out, "Arabic", "مرحبا بالعالم")?;
    row(out, "Persian", "سلام دنیا، می\u{200C}خواهم")?;
    row(out, "Hebrew", "שלום עולם")?;
    row(out, "digits", "٠١٢٣٤٥٦٧٨٩ ۰۱۲۳۴۵۶۷۸۹")?;
    row(out, "mixed", "Motty says مرحبا to you")?;
    row(out, "mixed", "Version 2.0 של התוכנה עובד")?;
    row(out, "numbers", "السعر 120 دولار و 50 سنتا")
}

fn thai(out: &mut dyn Write) -> io::Result<()> {
    heading(out, "Complex scripts and combining marks")?;
    row(out, "Thai", "สวัสดีชาวโลก ภาษาไทย")?;
    row(out, "Thai stacks", "กิ่ง ก้ำ ปี่ ฟุ้ง")?;
    row(out, "Devanagari", "नमस्ते दुनिया क्षत्रिय")?;
    row(out, "Tamil", "வணக்கம் உலகம்")?;
    row(out, "Bengali", "নমস্কার বিশ্ব")?;
    row(out, "Khmer", "សួស្តី ពិភពលោក")?;
    row(
        out,
        "combining",
        "e\u{301} a\u{308} n\u{303} o\u{302}\u{323} q\u{307}\u{323}",
    )
}

fn box_drawing(out: &mut dyn Write) -> io::Result<()> {
    heading(out, "Box drawing and block elements")?;
    row(out, "", "light   heavy   double  rounded")?;
    row(out, "boxes", "┌───┐   ┏━━━┓   ╔═══╗   ╭───╮")?;
    row(out, "", "│   │   ┃   ┃   ║   ║   │   │")?;
    row(out, "", "└───┘   ┗━━━┛   ╚═══╝   ╰───╯")?;
    row(out, "table", "┌──────┬──────┐")?;
    row(out, "", "│ name │ size │")?;
    row(out, "", "├──────┼──────┤")?;
    row(out, "", "│ a.rs │ 12 K │")?;
    row(out, "", "└──────┴──────┘")?;
    row(out, "blocks", "▀▄█▌▐ ▁▂▃▄▅▆▇█ ▏▎▍▌▋▊▉")?;
    row(out, "shades", "░░▒▒▓▓██")?;
    row(out, "quadrants", "▖▗▘▙▚▛▜▝▞▟")?;
    row(out, "braille", "⠁⠃⠇⡇⣇⣧⣷⣿ ⣿⣶⣤⣀")?;
    row(
        out,
        "powerline",
        "\u{E0B0} \u{E0B1} \u{E0B2} \u{E0B3} \u{E0B4} \u{E0B6}  (needs a Nerd Font)",
    )?;
    row(
        out,
        "segments",
        "\x1b[97;44m motty \x1b[34;42m\u{E0B0}\x1b[30;42m main \x1b[32;49m\u{E0B0}",
    )
}

fn links(out: &mut dyn Write) -> io::Result<()> {
    heading(out, "OSC 8 hyperlinks (hover or Cmd-click)")?;
    row(
        out,
        "plain",
        "\x1b]8;;https://example.com\x1b\\example.com\x1b]8;;\x1b\\",
    )?;
    row(
        out,
        "BEL ended",
        "\x1b]8;;https://example.org\x07example.org\x1b]8;;\x07",
    )?;
    // Both halves share id=, so they highlight together on hover.
    let start = "\x1b]8;id=motty-demo;https://github.com/farvisun/motty\x1b\\";
    let end = "\x1b]8;;\x1b\\";
    row(
        out,
        "with id=",
        &format!("{start}first half{end} and {start}second half{end}"),
    )?;
    let dir = std::env::current_dir().unwrap_or_else(|_| "/".into());
    row(
        out,
        "file://",
        &format!(
            "\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
            file_url(&dir),
            dir.display()
        ),
    )
}

/// `file://` URL for `path`, percent-encoding everything but unreserved bytes.
fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

fn progress(out: &mut dyn Write, fast: bool) -> io::Result<()> {
    const WIDTH: usize = 30;
    heading(out, "OSC 9;4 progress (watch the tab and dock)")?;
    for pct in (0..=100).step_by(5) {
        let filled = pct * WIDTH / 100;
        write!(
            out,
            "\x1b]9;4;1;{pct}\x07\r\x1b[2m{:<14}\x1b[22m \x1b[32m{}\x1b[2m{}{RESET} {pct:>3}%",
            "progress",
            "█".repeat(filled),
            "░".repeat(WIDTH - filled),
        )?;
        out.flush()?;
        pause(fast, 100);
    }
    writeln!(out, "{RESET}")?;
    write!(out, "{PROGRESS_CLEAR}")
}

fn error_state(out: &mut dyn Write, fast: bool) -> io::Result<()> {
    heading(out, "OSC 9;4 error state")?;
    write!(out, "\x1b]9;4;2;60\x07")?;
    row(
        out,
        "error",
        "\x1b[31mprogress at 60% in the error state, held 2 s",
    )?;
    out.flush()?;
    pause(fast, 2000);
    write!(out, "{PROGRESS_CLEAR}")
}

fn indeterminate(out: &mut dyn Write, fast: bool) -> io::Result<()> {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    heading(out, "OSC 9;4 indeterminate state")?;
    write!(out, "\x1b]9;4;3\x07")?;
    for i in 0..20 {
        write!(
            out,
            "\r\x1b[2m{:<14}\x1b[22m \x1b[36m{}{RESET} working",
            "busy",
            FRAMES[i % FRAMES.len()]
        )?;
        out.flush()?;
        pause(fast, 100);
    }
    writeln!(out, "{RESET}")?;
    write!(out, "{PROGRESS_CLEAR}")
}

fn notification(out: &mut dyn Write, fast: bool) -> io::Result<()> {
    heading(
        out,
        "Desktop notifications (may be suppressed while focused)",
    )?;
    write!(out, "\x1b]9;Motty: hello from OSC 9\x07")?;
    row(out, "OSC 9", "sent \"Motty: hello from OSC 9\"")?;
    out.flush()?;
    // A gap so the two arrive as separate notifications.
    pause(fast, 500);
    write!(out, "\x1b]777;notify;Motty;Hello from OSC 777\x07")?;
    row(
        out,
        "OSC 777",
        "sent title \"Motty\", body \"Hello from OSC 777\"",
    )
}

fn unicode(out: &mut dyn Write) -> io::Result<()> {
    heading(out, "Unicode smoke sheet")?;
    row(
        out,
        "combining",
        "e\u{301} a\u{308} n\u{303} o\u{302}\u{323} Z\u{335}a\u{336}l\u{337}g\u{338}o\u{334}",
    )?;
    row(out, "ruler", "|0123456789|")?;
    row(out, "ASCII", "|abcdefghij|")?;
    row(out, "wide", "|你好世界한|")?;
    row(out, "emoji", "|😀🚀🎉🐱🍕|")?;
    row(out, "halfwidth", "|ｱｲｳｴｵｶｷｸｹｺ|")?;
    row(
        out,
        "combining",
        "|e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}abcde|",
    )?;
    row(
        out,
        "clusters",
        "👨\u{200D}👩\u{200D}👧\u{200D}👦 🇮🇷 👋🏽 1\u{FE0F}\u{20E3} \u{2764}\u{FE0F}",
    )?;
    // Brackets should touch: these code points have no width.
    row(
        out,
        "zero-width",
        "ZWSP [\u{200B}] ZWNJ [\u{200C}] ZWJ [\u{200D}] WJ [\u{2060}] BOM [\u{FEFF}]",
    )?;
    row(out, "astral", "𝔘𝔫𝔦𝔠𝔬𝔡𝔢 𝐁𝐨𝐥𝐝 𓀀 🀄 𝄞 𐍈")?;
    row(out, "RTL", "English עברית العربية فارسی")?;
    row(out, "Thai", "สวัสดี ภาษาไทย กิ่ง")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(feature: &Feature) -> String {
        let mut buf = Vec::new();
        play(feature, &mut buf, true).unwrap();
        String::from_utf8(buf).expect("valid UTF-8")
    }

    #[test]
    fn names_are_unique_and_findable() {
        for (i, f) in FEATURES.iter().enumerate() {
            assert!(FEATURES[..i].iter().all(|g| g.name != f.name), "{}", f.name);
            assert_eq!(find(f.name).map(|g| g.name), Some(f.name));
        }
    }

    #[test]
    fn aliases_resolve() {
        assert_eq!(find("truecolor").unwrap().name, "true-color");
        assert_eq!(find("colors").unwrap().name, "256-colors");
        assert_eq!(find("256").unwrap().name, "256-colors");
        assert_eq!(find("Unicode").unwrap().name, "unicode");
        assert!(find("images").is_none());
        for (alias, target) in ALIASES {
            assert_eq!(find(alias).map(|f| f.name), Some(*target), "{alias}");
            assert!(FEATURES.iter().all(|f| f.name != *alias), "{alias}");
        }
    }

    #[test]
    fn every_feature_plays() {
        for f in FEATURES {
            let s = render(f);
            assert!(!s.is_empty(), "{}", f.name);
            let tail = s.trim_end_matches('\n');
            assert!(
                tail.ends_with(RESET) || tail.ends_with(PROGRESS_CLEAR),
                "{} ends with {:?}",
                f.name,
                tail.chars().rev().take(12).collect::<String>()
            );
        }
    }

    #[test]
    fn progress_sets_and_clears() {
        let s = render(find("progress").unwrap());
        assert!(s.contains("\x1b]9;4;1;"));
        assert!(s.contains("\x1b]9;4;1;100\x07"));
        assert!(s.ends_with("\x1b]9;4;0\x07"));
    }

    #[test]
    fn unknown_feature_is_an_error() {
        let fake = Feature {
            name: "images",
            description: "",
        };
        assert!(play(&fake, &mut Vec::new(), true).is_err());
    }
}
