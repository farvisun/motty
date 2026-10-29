//! Theme import: another terminal's color file as an `.ottytheme`, as
//! Otty's Import Theme does (Otty, iTerm2, Kitty, Alacritty, Ghostty).

use std::path::{Path, PathBuf};

use crate::css::Rgba;
use crate::theme::{self, Mode, luminance};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Otty,
    Iterm2,
    Kitty,
    Alacritty,
    Ghostty,
}

impl Format {
    pub const ALL: [Format; 5] = [
        Format::Otty,
        Format::Iterm2,
        Format::Kitty,
        Format::Alacritty,
        Format::Ghostty,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Format::Otty => "Otty",
            Format::Iterm2 => "iTerm2",
            Format::Kitty => "Kitty",
            Format::Alacritty => "Alacritty",
            Format::Ghostty => "Ghostty",
        }
    }

    /// Guessed from the file name and what's in it.
    pub fn detect(path: &Path, text: &str) -> Format {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_lowercase();
        match ext.as_str() {
            "ottytheme" => Format::Otty,
            "itermcolors" => Format::Iterm2,
            "conf" => Format::Kitty,
            _ if text.contains("<plist") => Format::Iterm2,
            _ if text.contains("[colors") => Format::Alacritty,
            _ if text.contains("[terminal]") => Format::Otty,
            _ if text.lines().any(|l| l.trim_start().starts_with("palette")) => Format::Ghostty,
            _ => Format::Kitty,
        }
    }
}

/// A converted theme, ready to preview and save.
#[derive(Clone, Debug)]
pub struct Imported {
    pub name: String,
    pub mode: Mode,
    pub background: Rgba,
    pub foreground: Rgba,
    pub palette: [Rgba; 16],
    /// The `.ottytheme` text to write.
    pub source: String,
}

#[derive(Default)]
struct Colors {
    foreground: Option<Rgba>,
    background: Option<Rgba>,
    cursor: Option<Rgba>,
    cursor_text: Option<Rgba>,
    selection: Option<Rgba>,
    palette: [Option<Rgba>; 16],
}

/// Converts `text` (read from `path`) in `format`.
pub fn convert(format: Format, path: &Path, text: &str) -> Result<Imported, String> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported".into());
    if format == Format::Otty {
        let parsed = theme::parse(text).map_err(|e| format!("{e:#}"))?;
        let t = parsed.theme;
        // Kept verbatim, chrome sections and all.
        return Ok(Imported {
            name: Some(t.meta.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or(stem),
            mode: t.meta.mode,
            background: t.terminal.background,
            foreground: t.terminal.foreground,
            palette: t.terminal.palette,
            source: text.to_owned(),
        });
    }
    let colors = match format {
        Format::Iterm2 => iterm2(text)?,
        Format::Kitty => kitty(text),
        Format::Alacritty => alacritty(text)?,
        Format::Ghostty => ghostty(text),
        Format::Otty => unreachable!(),
    };
    let (Some(background), Some(foreground)) = (colors.background, colors.foreground) else {
        return Err(format!(
            "No foreground and background colors found; is this a {} theme?",
            format.name()
        ));
    };
    let palette = fill_palette(&colors.palette);
    let name = pretty_name(&stem);
    let mode = if luminance(background) > 0.5 {
        Mode::Light
    } else {
        Mode::Dark
    };
    let mut source = format!(
        "# Imported from {} ({}).\n\n[meta]\nname = {}\nmode = \"{}\"\n\n[terminal]\nforeground = \"{}\"\nbackground = \"{}\"\n",
        format.name(),
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        toml::Value::String(name.clone()),
        if mode == Mode::Dark { "dark" } else { "light" },
        hex(foreground),
        hex(background),
    );
    for (key, color) in [
        ("cursor", colors.cursor),
        ("cursor-text", colors.cursor_text),
        ("selection-background", colors.selection),
    ] {
        if let Some(c) = color {
            source.push_str(&format!("{key} = \"{}\"\n", hex(c)));
        }
    }
    source.push_str("palette = [\n");
    for row in palette.chunks(4) {
        let row: Vec<_> = row.iter().map(|c| format!("\"{}\"", hex(*c))).collect();
        source.push_str(&format!("    {},\n", row.join(", ")));
    }
    source.push_str("]\n");
    Ok(Imported {
        name,
        mode,
        background,
        foreground,
        palette,
        source,
    })
}

/// Saves an import in `dir` as `<slug>.ottytheme`, adding `-1`, `-2`, …
/// rather than overwriting.
pub fn save(imported: &Imported, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let base = slug(&imported.name);
    let mut path = dir.join(format!("{base}.ottytheme"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{base}-{n}.ottytheme"));
        n += 1;
    }
    std::fs::write(&path, &imported.source)?;
    Ok(path)
}

/// Otty's file name for a theme: lowercased, anything but letters and
/// digits turned into `-`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_owned();
    if out.is_empty() { "theme".into() } else { out }
}

pub fn hex(c: Rgba) -> String {
    let b = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02X}{:02X}{:02X}", b(c.r), b(c.g), b(c.b))
}

/// `tokyo_night-storm` reads `Tokyo Night Storm`.
fn pretty_name(stem: &str) -> String {
    stem.split(['_', '-', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) if w.chars().all(|c| c.is_lowercase() || !c.is_alphabetic()) => {
                    f.to_uppercase().chain(c).collect()
                }
                _ => w.to_owned(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Missing bright colors repeat the normal ones; anything else missing
/// falls back to xterm's.
fn fill_palette(p: &[Option<Rgba>; 16]) -> [Rgba; 16] {
    const XTERM: [u32; 16] = [
        0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5, 0x7f7f7f,
        0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
    ];
    std::array::from_fn(|i| {
        p[i].or(if i >= 8 { p[i - 8] } else { None })
            .unwrap_or(Rgba::rgb(XTERM[i]))
    })
}

/// `#abc`, `#abcdef`, `abcdef`, `0xabcdef` or a CSS color.
fn loose_color(s: &str) -> Option<Rgba> {
    let s = s.trim().trim_matches(['"', '\'']);
    let h = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix('#'))
        .unwrap_or(s);
    if matches!(h.len(), 3 | 6) && h.chars().all(|c| c.is_ascii_hexdigit()) {
        return crate::css::parse_color(&format!("#{h}")).ok();
    }
    crate::css::parse_color(s).ok()
}

fn kitty(text: &str) -> Colors {
    let mut c = Colors::default();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(key), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        let v = loose_color(value);
        match key {
            "foreground" => c.foreground = v,
            "background" => c.background = v,
            "cursor" => c.cursor = v,
            "cursor_text_color" => c.cursor_text = v,
            "selection_background" => c.selection = v,
            k => {
                if let Some(i) = k
                    .strip_prefix("color")
                    .and_then(|n| n.parse::<usize>().ok())
                    && i < 16
                {
                    c.palette[i] = v;
                }
            }
        }
    }
    c
}

fn ghostty(text: &str) -> Colors {
    let mut c = Colors::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "foreground" => c.foreground = loose_color(value),
            "background" => c.background = loose_color(value),
            "cursor-color" => c.cursor = loose_color(value),
            "cursor-text" => c.cursor_text = loose_color(value),
            "selection-background" => c.selection = loose_color(value),
            "palette" => {
                if let Some((i, v)) = value.split_once('=')
                    && let Ok(i) = i.trim().parse::<usize>()
                    && i < 16
                {
                    c.palette[i] = loose_color(v);
                }
            }
            _ => {}
        }
    }
    c
}

fn alacritty(text: &str) -> Result<Colors, String> {
    let doc: toml::Table = text.parse().map_err(|e| format!("Not valid TOML: {e}"))?;
    let colors = doc
        .get("colors")
        .and_then(|v| v.as_table())
        .ok_or("No [colors] section")?;
    let get = |section: &str, key: &str| {
        colors
            .get(section)?
            .as_table()?
            .get(key)?
            .as_str()
            .and_then(loose_color)
    };
    let mut c = Colors {
        foreground: get("primary", "foreground"),
        background: get("primary", "background"),
        cursor: get("cursor", "cursor"),
        cursor_text: get("cursor", "text"),
        selection: get("selection", "background"),
        ..Default::default()
    };
    const NAMES: [&str; 8] = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    for (i, name) in NAMES.iter().enumerate() {
        c.palette[i] = get("normal", name);
        c.palette[i + 8] = get("bright", name);
    }
    Ok(c)
}

/// The plist's `<key>Ansi 0 Color</key><dict>…Red Component…</dict>`.
fn iterm2(text: &str) -> Result<Colors, String> {
    if !text.contains("<plist") {
        return Err("Not an iTerm2 .itermcolors file".into());
    }
    let mut c = Colors::default();
    let mut rest = text;
    while let Some(start) = rest.find("<key>") {
        let after = &rest[start + 5..];
        let Some(end) = after.find("</key>") else {
            break;
        };
        let key = &after[..end];
        let body = after[end + 6..].trim_start();
        rest = &after[end + 6..];
        let Some(dict) = body.strip_prefix("<dict>") else {
            continue;
        };
        let Some(close) = dict.find("</dict>") else {
            break;
        };
        let dict = &dict[..close];
        rest = &body[7 + close..];
        let comp = |name: &str| -> Option<f32> {
            let at = dict.find(&format!("<key>{name}</key>"))?;
            let after = &dict[at..];
            let open = after.find("<real>")? + 6;
            let shut = after[open..].find("</real>")?;
            after[open..open + shut].trim().parse().ok()
        };
        let Some((r, g, b)) = comp("Red Component")
            .zip(comp("Green Component"))
            .zip(comp("Blue Component"))
            .map(|((r, g), b)| (r, g, b))
        else {
            continue;
        };
        let rgba = Rgba { r, g, b, a: 1. };
        match key {
            "Foreground Color" => c.foreground = Some(rgba),
            "Background Color" => c.background = Some(rgba),
            "Cursor Color" => c.cursor = Some(rgba),
            "Cursor Text Color" => c.cursor_text = Some(rgba),
            "Selection Color" => c.selection = Some(rgba),
            k => {
                if let Some(i) = k
                    .strip_prefix("Ansi ")
                    .and_then(|r| r.strip_suffix(" Color"))
                    .and_then(|n| n.parse::<usize>().ok())
                    && i < 16
                {
                    c.palette[i] = Some(rgba);
                }
            }
        }
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    #[test]
    fn kitty_conf() {
        let text = "# a theme\nforeground #c0caf5\nbackground #1a1b26\ncursor #c0caf5\nselection_background #33467c\ncolor0 #15161e\ncolor1 #f7768e\ncolor9 #ff0000\n";
        let t = convert(Format::Kitty, &p("tokyo_night.conf"), text).unwrap();
        assert_eq!(t.name, "Tokyo Night");
        assert_eq!(t.mode, Mode::Dark);
        assert_eq!(hex(t.palette[1]), "#F7768E");
        assert_eq!(hex(t.palette[9]), "#FF0000");
        // A missing bright color repeats the normal one.
        assert_eq!(hex(t.palette[8]), "#15161E");
        let parsed = theme::parse(&t.source).unwrap();
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.theme.meta.name, "Tokyo Night");
        assert_eq!(hex(parsed.theme.terminal.background), "#1A1B26");
    }

    #[test]
    fn ghostty_file() {
        let text = "background = fdf6e3\nforeground = 657b83\npalette = 0=#073642\npalette = 15=fdf6e3\ncursor-color = 586e75\n";
        let t = convert(Format::Ghostty, &p("Solarized Light"), text).unwrap();
        assert_eq!(t.mode, Mode::Light);
        assert_eq!(hex(t.palette[0]), "#073642");
        assert_eq!(hex(t.palette[15]), "#FDF6E3");
        assert!(t.source.contains("cursor = \"#586E75\""));
        assert!(theme::parse(&t.source).is_ok());
    }

    #[test]
    fn alacritty_toml() {
        let text = "[colors.primary]\nbackground = '0x282828'\nforeground = '#ebdbb2'\n[colors.normal]\nred = '#cc241d'\n[colors.bright]\nred = '#fb4934'\n";
        let t = convert(Format::Alacritty, &p("gruvbox_dark.toml"), text).unwrap();
        assert_eq!(hex(t.background), "#282828");
        assert_eq!(hex(t.palette[1]), "#CC241D");
        assert_eq!(hex(t.palette[9]), "#FB4934");
        assert!(convert(Format::Alacritty, &p("x.toml"), "[window]\n").is_err());
    }

    #[test]
    fn iterm2_plist() {
        let comp = |r: f32, g: f32, b: f32| {
            format!(
                "<dict><key>Blue Component</key><real>{b}</real><key>Green Component</key><real>{g}</real><key>Red Component</key><real>{r}</real></dict>"
            )
        };
        let text = format!(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>Ansi 1 Color</key>{}<key>Background Color</key>{}<key>Foreground Color</key>{}</dict></plist>",
            comp(1., 0., 0.),
            comp(0., 0., 0.),
            comp(1., 1., 1.)
        );
        let t = convert(Format::Iterm2, &p("Mine.itermcolors"), &text).unwrap();
        assert_eq!(t.name, "Mine");
        assert_eq!(hex(t.palette[1]), "#FF0000");
        assert_eq!(hex(t.background), "#000000");
        assert!(convert(Format::Iterm2, &p("x"), "foreground #fff").is_err());
    }

    #[test]
    fn otty_files_are_kept_verbatim() {
        let palette = vec!["\"#101010\""; 16].join(", ");
        let text = format!(
            "[meta]\nname = \"Mine\"\n[terminal]\nforeground = \"#ffffff\"\nbackground = \"#000000\"\npalette = [{palette}]\n[titlebar]\nheight = 40\n"
        );
        let text = text.as_str();
        let t = convert(Format::Otty, &p("mine.ottytheme"), text).unwrap();
        assert_eq!(t.source, text);
        assert_eq!(t.name, "Mine");
    }

    #[test]
    fn detects_formats_and_slugs() {
        assert_eq!(Format::detect(&p("a.itermcolors"), ""), Format::Iterm2);
        assert_eq!(
            Format::detect(&p("a.toml"), "[colors.primary]"),
            Format::Alacritty
        );
        assert_eq!(Format::detect(&p("a"), "palette = 0=#000"), Format::Ghostty);
        assert_eq!(Format::detect(&p("a.conf"), ""), Format::Kitty);
        assert_eq!(slug("My Cool Theme"), "my-cool-theme");
        assert_eq!(slug("Rosé Pine!"), "rosé-pine");
    }

    #[test]
    fn saving_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("motty-import-{}", std::process::id()));
        let t = convert(
            Format::Kitty,
            &p("x.conf"),
            "foreground #fff\nbackground #000\n",
        )
        .unwrap();
        let a = save(&t, &dir).unwrap();
        let b = save(&t, &dir).unwrap();
        assert_ne!(a, b);
        assert!(b.to_string_lossy().ends_with("x-1.ottytheme"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
