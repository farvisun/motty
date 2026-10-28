//! `config.toml`: flat kebab-case `key = value` lines, compatible with Otty's
//! config file. Values are TOML literals, but keys may repeat: a repeated key
//! is a list (e.g. several `open-with-app` entries), so the file is parsed
//! line by line rather than as a strict TOML document.

use std::path::{Path, PathBuf};

use crate::css::{self, Rgba};
use crate::theme::{self, Mode, ParsedTheme, Theme};

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub theme: Option<String>,
    pub theme_dark: Option<String>,
    pub foreground: Option<Rgba>,
    pub background: Option<Rgba>,
    pub palette: [Option<Rgba>; 16],
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub ligatures: bool,
    pub sidebar_width: Option<f32>,
    pub details_panel_width: Option<f32>,
    pub option_as_meta: bool,
    pub scrollback: Option<usize>,
    pub shell: Option<String>,
    pub cursor_blink: bool,
    /// `window-hide-dirty-indicator`: never show the close button's dot.
    pub hide_dirty_indicator: bool,
    /// `open-with-app = "Name|bundle.id"` entries, in file order.
    pub open_with_apps: Vec<String>,
    /// Where the config was read from, if anywhere.
    pub path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

pub fn motty_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".config/motty")
}

pub fn otty_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".config/otty")
}

impl Config {
    /// Motty's own config wins; otherwise an existing Otty config is read as-is.
    /// `config.local.toml` beside it (machine-local settings) applies on top.
    pub fn load() -> Self {
        for dir in [motty_dir(), otty_dir()] {
            let path = dir.join("config.toml");
            if let Ok(src) = std::fs::read_to_string(&path) {
                let local = std::fs::read_to_string(dir.join("config.local.toml")).unwrap_or_default();
                let mut cfg = Self::parse(&format!("{src}\n{local}"));
                cfg.path = Some(path);
                return cfg;
            }
        }
        Self::parse("")
    }

    /// Defaults follow Otty's: ligatures on, Option is not Alt, no blinking.
    pub fn parse(src: &str) -> Self {
        let mut out = Config { ligatures: true, ..Default::default() };
        let (entries, mut warnings) = parse_entries(src);
        let mut warn = |k: &str, m: String| warnings.push(format!("config.toml {k}: {m}"));
        let str_of = |v: &toml::Value| v.as_str().map(str::to_owned);
        let num_of = |v: &toml::Value| v.as_float().map(|f| f as f32).or_else(|| v.as_integer().map(|i| i as f32));
        let mut color_of = |k: &str, v: &toml::Value| match v.as_str().map(css::parse_color) {
            Some(Ok(c)) => Some(c),
            Some(Err(e)) => {
                warn(k, e.0);
                None
            }
            None => None,
        };
        for (key, value) in &entries {
            let key = key.as_str();
            match key {
                "theme" => out.theme = str_of(value),
                "theme-dark" => out.theme_dark = str_of(value),
                "foreground" => out.foreground = color_of(key, value),
                "background" => out.background = color_of(key, value),
                "font-family" => out.font_family = str_of(value),
                "font-size" => out.font_size = num_of(value),
                // `off`, `calt` or `dlig` (older configs use a bool).
                "font-ligatures" | "ligatures" => {
                    out.ligatures = value.as_bool().unwrap_or_else(|| value.as_str() != Some("off"))
                }
                "sidebar-width" => out.sidebar_width = num_of(value),
                "details-panel-width" => out.details_panel_width = num_of(value),
                // `true`, `false`, `left` or `right`; one side counts as on
                // until left/right modifiers are told apart.
                "macos-option-as-alt" | "option-as-meta" => {
                    out.option_as_meta = value.as_bool().unwrap_or_else(|| value.as_str().is_some_and(|v| v != "false"))
                }
                "scrollback" | "scrollback-lines" => out.scrollback = value.as_integer().map(|i| i.max(0) as usize),
                "shell" => out.shell = str_of(value),
                // `default_off`, `default_on`, `always_off` or `always_on`.
                "cursor-style-blink" | "cursor-blink" => {
                    out.cursor_blink = value.as_bool().unwrap_or_else(|| value.as_str().is_some_and(|v| v.ends_with("_on")))
                }
                "window-hide-dirty-indicator" => out.hide_dirty_indicator = value.as_bool().unwrap_or(false),
                "open-with-app" => out.open_with_apps.extend(str_of(value)),
                k if k.starts_with("palette-") => {
                    if let Ok(i @ 0..16) = k["palette-".len()..].parse::<usize>() {
                        out.palette[i] = color_of(key, value);
                    }
                }
                _ => {}
            }
        }
        out.warnings = warnings;
        out
    }

    /// The theme name for the given system appearance.
    pub fn theme_name(&self, mode: Mode) -> Option<&str> {
        match mode {
            Mode::Dark => self.theme_dark.as_deref().or(self.theme.as_deref()),
            Mode::Light => self.theme.as_deref(),
        }
    }

    /// Loads the active theme and applies config color overrides. Overrides
    /// belong to the light/default `theme`, so they are skipped when a
    /// separate `theme-dark` is active.
    pub fn resolve_theme(&self, appearance: Mode) -> ParsedTheme {
        let name = self.theme_name(appearance);
        let mut parsed = name
            .map(|n| match find_theme(n) {
                Some(p) => p,
                None => {
                    let mut fallback = builtin(appearance);
                    fallback.warnings.push(format!("theme {n:?} not found"));
                    fallback
                }
            })
            .unwrap_or_else(|| builtin(appearance));
        let using_dark_variant = appearance == Mode::Dark && self.theme_dark.is_some();
        if !using_dark_variant {
            self.apply_overrides(&mut parsed.theme);
        }
        parsed
    }

    fn apply_overrides(&self, theme: &mut Theme) {
        if let Some(fg) = self.foreground {
            theme.terminal.foreground = fg;
        }
        if let Some(bg) = self.background {
            theme.terminal.background = bg;
        }
        for (i, c) in self.palette.iter().enumerate() {
            if let Some(c) = c {
                theme.terminal.palette[i] = *c;
            }
        }
    }
}

/// Splits the file into `(key, value)` entries in order, keeping repeats.
/// A value may span lines while brackets are open (multi-line arrays).
fn parse_entries(src: &str) -> (Vec<(String, toml::Value)>, Vec<String>) {
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut lines = src.lines().enumerate();
    while let Some((n, line)) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut stmt = trimmed.to_owned();
        let depth = |s: &str| s.matches('[').count() as i32 - s.matches(']').count() as i32;
        while depth(&stmt) > 0 {
            match lines.next() {
                Some((_, more)) => {
                    stmt.push('\n');
                    stmt.push_str(more);
                }
                None => break,
            }
        }
        match toml::from_str::<toml::Table>(&stmt) {
            Ok(table) => entries.extend(table),
            Err(e) => warnings.push(format!("config.toml line {}: {}", n + 1, e.message())),
        }
    }
    (entries, warnings)
}

const MOTTY_DARK: &str = include_str!("../themes/motty-dark.ottytheme");
const MOTTY_LIGHT: &str = include_str!("../themes/motty-light.ottytheme");

pub fn builtin(mode: Mode) -> ParsedTheme {
    theme::parse(if mode == Mode::Dark { MOTTY_DARK } else { MOTTY_LIGHT }).expect("built-in theme parses")
}

pub fn theme_dirs() -> Vec<PathBuf> {
    vec![motty_dir().join("themes"), otty_dir().join("themes")]
}

/// Finds a theme by `[meta] name` or file stem, case-insensitively.
pub fn find_theme(name: &str) -> Option<ParsedTheme> {
    let wanted = normalize(name);
    for builtin_src in [MOTTY_DARK, MOTTY_LIGHT] {
        if let Ok(p) = theme::parse(builtin_src)
            && normalize(&p.theme.meta.name) == wanted
        {
            return Some(p);
        }
    }
    for dir in theme_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| is_theme_file(p)).collect();
        paths.sort();
        // File stems are cheap to check first; fall back to parsing names.
        if let Some(p) = paths.iter().find(|p| p.file_stem().is_some_and(|s| normalize(&s.to_string_lossy()) == wanted))
            && let Some(t) = load_file(p)
        {
            return Some(t);
        }
        for p in &paths {
            if let Some(t) = load_file(p)
                && normalize(&t.theme.meta.name) == wanted
            {
                return Some(t);
            }
        }
    }
    None
}

pub fn list_themes() -> Vec<(String, Mode, Option<PathBuf>)> {
    let mut out: Vec<(String, Mode, Option<PathBuf>)> = [MOTTY_DARK, MOTTY_LIGHT]
        .iter()
        .filter_map(|s| theme::parse(s).ok())
        .map(|p| (p.theme.meta.name, p.theme.meta.mode, None))
        .collect();
    for dir in theme_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for path in entries.flatten().map(|e| e.path()).filter(|p| is_theme_file(p)) {
            if let Some(t) = load_file(&path)
                && !out.iter().any(|(n, ..)| normalize(n) == normalize(&t.theme.meta.name))
            {
                out.push((t.theme.meta.name, t.theme.meta.mode, Some(path)));
            }
        }
    }
    out.sort_by_key(|(n, ..)| n.to_lowercase());
    out
}

fn is_theme_file(p: &Path) -> bool {
    matches!(p.extension().and_then(|e| e.to_str()), Some("ottytheme" | "toml"))
}

fn load_file(p: &Path) -> Option<ParsedTheme> {
    theme::parse(&std::fs::read_to_string(p).ok()?).ok()
}

fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_otty_style_config() {
        let cfg = Config::parse(
            "theme = \"Paper\"\ntheme-dark = \"Nord\"\nforeground = \"#1a1a1a\"\npalette-1 = \"#a33a3a\"\nsidebar-width = 225\nprivilege-caffeinate-agent-processing = true\n",
        );
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        assert_eq!(cfg.theme_name(Mode::Light), Some("Paper"));
        assert_eq!(cfg.theme_name(Mode::Dark), Some("Nord"));
        assert_eq!(cfg.palette[1], Some(Rgba::rgb(0xa33a3a)));
        assert_eq!(cfg.sidebar_width, Some(225.));
    }

    #[test]
    fn repeated_keys_accumulate_and_bad_lines_are_skipped() {
        let cfg = Config::parse(
            "open-with-app = \"VS Code|com.microsoft.VSCode\"\nopen-with-app = \"Zed|dev.zed.Zed\"\nfont-size = oops\ntheme = \"Nord\"\n",
        );
        assert_eq!(cfg.open_with_apps.len(), 2);
        assert_eq!(cfg.theme.as_deref(), Some("Nord"));
        assert_eq!(cfg.warnings.len(), 1);
    }

    #[test]
    fn otty_enum_values_and_defaults() {
        let d = Config::parse("");
        assert!(d.ligatures && !d.option_as_meta && !d.cursor_blink);
        let cfg = Config::parse(
            "font-ligatures = \"off\"\nmacos-option-as-alt = \"left\"\ncursor-style-blink = \"default_on\"\nwindow-hide-dirty-indicator = true\n",
        );
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        assert!(!cfg.ligatures && cfg.option_as_meta && cfg.cursor_blink && cfg.hide_dirty_indicator);
        // A later file (config.local.toml) overrides earlier keys.
        assert!(!Config::parse("macos-option-as-alt = true\nmacos-option-as-alt = \"false\"\n").option_as_meta);
    }

    #[test]
    fn builtins_parse_cleanly() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = builtin(mode);
            assert!(t.warnings.is_empty(), "{:?}", t.warnings);
            assert_eq!(t.theme.meta.mode, mode);
        }
    }

    #[test]
    fn missing_theme_falls_back_with_warning() {
        let cfg = Config::parse("theme = \"Does Not Exist 123\"\n");
        let t = cfg.resolve_theme(Mode::Light);
        assert_eq!(t.theme.meta.name, "Motty Light");
        assert_eq!(t.warnings.len(), 1);
    }
}
