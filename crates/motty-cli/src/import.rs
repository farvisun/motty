//! `motty import`: another terminal's config (Ghostty, Kitty, Alacritty)
//! read into Motty's, zoxide's folders, a theme or a font. Following
//! Otty's tables, every key of the source lands in one bucket: imported
//! (Motty has it, the value written as is or translated), conflicting
//! (Motty already has another value), similar (a close knob, never written)
//! or skipped (no counterpart). Without `--overwrite` or `--keep` nothing is
//! written: the report is a preview.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Args, ValueEnum};
use motty_theme::config::find_theme;
use motty_theme::schema;
use motty_theme::store::Files;
use serde_json::json;
use toml::Value;

use crate::config_cmd::{print_json, to_json};

#[derive(Args)]
pub struct ImportArgs {
    /// ghostty, kitty, alacritty, zoxide, theme, font, or a file or URL
    /// (a font file is imported as a font, anything else as a theme).
    pub source: String,
    /// The file to read (theme and font: required; terminals: their usual
    /// config file).
    pub path: Option<String>,
    /// Take the other terminal's value on every conflict.
    #[arg(long, conflicts_with = "keep")]
    pub overwrite: bool,
    /// Apply only the keys that don't conflict.
    #[arg(long)]
    pub keep: bool,
    /// Themes: switch to the imported theme.
    #[arg(long)]
    pub activate: bool,
    /// Fonts: use the imported font as font-family.
    #[arg(long)]
    pub apply: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Terminal {
    Ghostty,
    Kitty,
    Alacritty,
}

impl Terminal {
    pub fn name(self) -> &'static str {
        match self {
            Terminal::Ghostty => "Ghostty",
            Terminal::Kitty => "Kitty",
            Terminal::Alacritty => "Alacritty",
        }
    }

    /// Where the terminal keeps its config.
    fn default_path(self) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let candidates = match self {
            Terminal::Ghostty => vec![
                config.join("ghostty/config.ghostty"),
                config.join("ghostty/config"),
                home.join("Library/Application Support/com.mitchellh.ghostty/config.ghostty"),
                home.join("Library/Application Support/com.mitchellh.ghostty/config"),
            ],
            Terminal::Kitty => vec![config.join("kitty/kitty.conf")],
            Terminal::Alacritty => vec![config.join("alacritty/alacritty.toml")],
        };
        candidates
            .iter()
            .find(|p| p.is_file())
            .unwrap_or(&candidates[0])
            .clone()
    }
}

pub fn run(args: &ImportArgs, json: bool) -> anyhow::Result<ExitCode> {
    let terminal = match args.source.as_str() {
        "ghostty" => Some(Terminal::Ghostty),
        "kitty" => Some(Terminal::Kitty),
        "alacritty" => Some(Terminal::Alacritty),
        _ => None,
    };
    if let Some(terminal) = terminal {
        let path = args
            .path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| terminal.default_path());
        import_config(terminal, &path, args, json)?;
        return Ok(ExitCode::SUCCESS);
    }
    let needs_path = || {
        args.path
            .clone()
            .with_context(|| format!("`motty import {}` needs a file or URL", args.source))
    };
    match args.source.as_str() {
        "zoxide" => zoxide(json)?,
        "theme" => {
            crate::themes::import(&needs_path()?, args.activate, args.overwrite, json)?;
        }
        "font" => {
            crate::fonts::import(&needs_path()?, args.apply, json)?;
        }
        // A file or URL: a font, else a theme.
        source => {
            let name = source.split(['?', '#']).next().unwrap_or(source);
            if crate::fonts::is_font(Path::new(name)) {
                crate::fonts::import(source, args.apply, json)?;
            } else {
                crate::themes::import(source, args.activate, args.overwrite, json)?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// What becomes of one key of the other terminal's config.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Motty keys and values (a list key, one entry each).
    Set(Vec<(&'static str, Value)>),
    /// A close Motty knob, left for you to set.
    Similar(&'static str, &'static str),
    Skip(String),
}

/// One line of the source and what becomes of it.
#[derive(Clone, Debug)]
pub struct Row {
    /// `key = value` as the source has it.
    pub from: String,
    pub outcome: Outcome,
}

fn set(key: &'static str, value: Value) -> Outcome {
    Outcome::Set(vec![(key, value)])
}

fn s(v: impl Into<String>) -> Value {
    Value::String(v.into())
}

fn skip(reason: impl Into<String>) -> Outcome {
    Outcome::Skip(reason.into())
}

fn no_equivalent() -> Outcome {
    skip("Motty has no equivalent")
}

/// `true`, `yes`, … as a boolean.
fn truthy(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "y" | "on" | "1" => Some(true),
        "false" | "no" | "n" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn boolean(key: &'static str, v: &str) -> Outcome {
    match truthy(v) {
        Some(b) => set(key, Value::Boolean(b)),
        None => skip(format!("expected true or false, got {v:?}")),
    }
}

/// `#rrggbb` from `#rgb`, `#rrggbb`, `0xrrggbb` or a color name.
fn color(v: &str) -> Option<Value> {
    let v = v.trim().trim_matches(['"', '\'']);
    let v = match v.strip_prefix("0x") {
        Some(hex) => format!("#{hex}"),
        None if v.len() == 6 && v.chars().all(|c| c.is_ascii_hexdigit()) => format!("#{v}"),
        None => v.to_owned(),
    };
    let c = motty_theme::css::parse_color(&v).ok()?;
    Some(s(motty_theme::import::hex(c).to_lowercase()))
}

fn color_key(key: &'static str, v: &str) -> Outcome {
    match color(v) {
        Some(c) => set(key, c),
        None => skip(format!("{v:?} isn't a color Motty reads")),
    }
}

fn number(key: &'static str, v: &str) -> Outcome {
    match v.trim().parse::<f64>() {
        Ok(n) => set(key, Value::Float(n)),
        Err(_) => skip(format!("expected a number, got {v:?}")),
    }
}

/// A theme name, if Motty has that theme.
fn theme(key: &'static str, name: &str) -> Outcome {
    let name = name.trim();
    match find_theme(name) {
        Some(t) => set(key, s(t.theme.meta.name)),
        None => skip(format!(
            "Motty has no theme named {name:?}; import its file with `motty theme import`"
        )),
    }
}

fn unquote(v: &str) -> &str {
    let v = v.trim();
    v.strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(v)
}

/// The action names Motty's `keybind` lines take (its own, as the app
/// names them), for deciding whether a binding carries over.
const ACTIONS: &[&str] = &[
    "copy",
    "paste",
    "select_all",
    "clear",
    "increase_font_size",
    "decrease_font_size",
    "reset_font_size",
    "quit",
    "new_tab",
    "close_tab",
    "reopen_closed_tab",
    "next_tab",
    "previous_tab",
    "activate_last_tab",
    "toggle_tabs_panel",
    "toggle_details_panel",
    "split_right",
    "split_left",
    "split_down",
    "split_up",
    "focus_pane_left",
    "focus_pane_right",
    "focus_pane_up",
    "focus_pane_down",
    "focus_next_pane",
    "focus_previous_pane",
    "equalize_splits",
    "zoom_split",
    "undo",
    "redo",
    "cut",
    "scroll_to_top",
    "scroll_to_bottom",
    "scroll_page_up",
    "scroll_page_down",
    "scroll_line_up",
    "scroll_line_down",
    "new_window",
    "close_window",
    "minimize",
    "toggle_full_screen",
    "find",
    "find_next",
    "find_previous",
    "command_palette",
    "open_quickly",
    "open_config_file",
    "open_settings",
    "hide_app",
    "rename_tab",
    "move_tab_up",
    "move_tab_down",
    "toggle_vi_mode",
    "close_all",
];

/// Keys carried over as they are: a key name Motty shares with the source.
fn motty_action(name: &str) -> Option<String> {
    ACTIONS.contains(&name).then(|| name.to_owned())
}

/// A `goto_tab` argument Motty has (1–9).
fn goto_tab(n: &str) -> Option<String> {
    let n: u8 = n.trim().parse().ok()?;
    (1..=9).contains(&n).then(|| format!("goto_tab:{n}"))
}

/// Normalizes a trigger's key names Motty spells differently.
fn trigger(t: &str) -> String {
    t.split('+')
        .map(|part| {
            let p = part.trim().to_ascii_lowercase();
            let digits = [
                "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
            ];
            if let Some(i) = digits.iter().position(|d| *d == p) {
                return i.to_string();
            }
            if let Some(rest) = p.strip_prefix("digit_").or_else(|| p.strip_prefix("key_")) {
                return rest.to_owned();
            }
            match p.as_str() {
                "kitty_mod" => p,
                "super" | "command" => "cmd".into(),
                "control" => "ctrl".into(),
                "option" | "opt" => "alt".into(),
                _ => p,
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

// ---- Ghostty ----

/// Ghostty's action names in Motty's terms.
fn ghostty_action(action: &str) -> Option<String> {
    let (name, arg) = match action.split_once(':') {
        Some((n, a)) => (n, Some(a)),
        None => (action, None),
    };
    let to = |s: &str| Some(s.to_owned());
    match (name, arg) {
        ("text" | "csi" | "esc", Some(_)) => to(action),
        ("unbind", None) => to("unbind"),
        ("goto_tab", Some(n)) => goto_tab(n),
        ("new_split", Some("right" | "auto")) => to("split_right"),
        ("new_split", Some("left")) => to("split_left"),
        ("new_split", Some("down")) => to("split_down"),
        ("new_split", Some("up")) => to("split_up"),
        ("goto_split", Some("left")) => to("focus_pane_left"),
        ("goto_split", Some("right")) => to("focus_pane_right"),
        ("goto_split", Some("up" | "top")) => to("focus_pane_up"),
        ("goto_split", Some("down" | "bottom")) => to("focus_pane_down"),
        ("goto_split", Some("next")) => to("focus_next_pane"),
        ("goto_split", Some("previous")) => to("focus_previous_pane"),
        ("increase_font_size", _) => to("increase_font_size"),
        ("decrease_font_size", _) => to("decrease_font_size"),
        ("copy_to_clipboard", _) => to("copy"),
        ("paste_from_clipboard", _) => to("paste"),
        ("clear_screen", None) => to("clear"),
        ("toggle_split_zoom", None) => to("zoom_split"),
        ("toggle_fullscreen", None) => to("toggle_full_screen"),
        ("last_tab", None) => to("activate_last_tab"),
        ("toggle_command_palette", None) => to("command_palette"),
        ("open_config", None) => to("open_config_file"),
        ("close_surface", None) => to("close_tab"),
        ("close_all_windows", None) => to("close_all"),
        (name, None) => motty_action(name),
        _ => None,
    }
}

fn ghostty_keybind(value: &str) -> Outcome {
    let value = unquote(value);
    if value == "clear" {
        return skip("Motty keeps its default shortcuts; unbind them one by one");
    }
    let Some(kb) = motty_theme::config::Keybind::parse(value) else {
        return skip("can't read this keybind");
    };
    let mut t = kb.trigger.as_str();
    for prefix in ["unconsumed:", "performable:"] {
        t = t.strip_prefix(prefix).unwrap_or(t);
    }
    if t.starts_with("global:") || t.starts_with("all:") {
        return skip("Motty has no global shortcuts");
    }
    if t.contains('>') {
        return skip("Motty has no key sequences");
    }
    match ghostty_action(&kb.action) {
        Some(action) => set("keybind", s(format!("{}={action}", trigger(t)))),
        None => skip(format!("Motty has no action like {:?}", kb.action)),
    }
}

/// Ghostty's `key = value` lines (quotes optional, `#` comments).
fn ghostty_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_owned(), unquote(v).to_owned()))
        })
        .collect()
}

pub fn ghostty(text: &str) -> Vec<Row> {
    let mut fonts = 0;
    ghostty_lines(text)
        .into_iter()
        .map(|(key, value)| {
            let v = value.as_str();
            let outcome = match key.as_str() {
                "font-family" if v.is_empty() => skip("an empty value only clears Ghostty's list"),
                "font-family" => {
                    fonts += 1;
                    if fonts == 1 {
                        set("font-family", s(v))
                    } else {
                        skip("a fallback font; Motty takes one family")
                    }
                }
                "font-size" => number("font-size", v),
                "font-feature" => {
                    if v.contains("-calt") || v.contains("-liga") {
                        set("font-ligatures", s("off"))
                    } else {
                        skip("Motty only turns ligatures on or off")
                    }
                }
                "command" => {
                    let v = v
                        .strip_prefix("direct:")
                        .or_else(|| v.strip_prefix("shell:"))
                        .unwrap_or(v);
                    set("command", s(v))
                }
                "env" => match v.split_once('=') {
                    Some((name, _)) if !name.trim().is_empty() => set("env", s(v)),
                    _ => skip("expected NAME=value"),
                },
                "working-directory" => set("working-directory", s(v)),
                "window-inherit-working-directory" => match truthy(v) {
                    Some(inherit) => {
                        let dir = s(if inherit { "inherit" } else { "home" });
                        Outcome::Set(vec![
                            ("tab-working-directory", dir.clone()),
                            ("split-working-directory", dir),
                        ])
                    }
                    None => skip("expected true or false"),
                },
                "scrollback-limit" => match v.parse::<i64>() {
                    // Bytes; Otty reads 80 per line.
                    Ok(bytes) => set("scrollback-lines", Value::Integer(bytes / 80)),
                    Err(_) => skip("expected a number of bytes"),
                },
                "foreground" => color_key("foreground", v),
                "background" => color_key("background", v),
                "palette" => match v.split_once('=') {
                    Some((n, c)) => match (n.trim().parse::<usize>(), color(c)) {
                        (Ok(n @ 0..16), Some(c)) => Outcome::Set(vec![(PALETTE[n], c)]),
                        (Ok(_), Some(_)) => skip("Motty's palette has 16 colors"),
                        _ => skip("expected N=#rrggbb"),
                    },
                    None => skip("expected N=#rrggbb"),
                },
                "cursor-style" => match v {
                    "block" | "bar" | "underline" | "block_hollow" => set("cursor-style", s(v)),
                    _ => skip(format!("Motty has no cursor style {v:?}")),
                },
                "cursor-style-blink" => match truthy(v) {
                    Some(true) => set("cursor-style-blink", s("default_on")),
                    Some(false) => set("cursor-style-blink", s("default_off")),
                    None => skip("Ghostty's default (let programs decide) is Motty's"),
                },
                "cursor-color" => color_key("cursor-color", v),
                "cursor-text" => color_key("cursor-text", v),
                "mouse-hide-while-typing"
                | "focus-follows-mouse"
                | "clipboard-trim-trailing-spaces"
                | "clipboard-paste-protection"
                | "clipboard-paste-bracketed-safe"
                | "quit-after-last-window-closed"
                | "selection-clear-on-typing" => {
                    boolean(schema::lookup(&key).expect("a Motty key").key, v)
                }
                // `clipboard` copies to the clipboard too.
                "copy-on-select" => set("copy-on-select", Value::Boolean(v != "false")),
                "macos-option-as-alt" => match v {
                    "true" | "false" | "left" | "right" => set("macos-option-as-alt", s(v)),
                    _ => skip(format!("unknown value {v:?}")),
                },
                "clipboard-read" | "clipboard-write" => match v {
                    "ask" | "allow" | "deny" => set(
                        if key == "clipboard-read" {
                            "clipboard-read"
                        } else {
                            "clipboard-write"
                        },
                        s(v),
                    ),
                    _ => skip(format!("unknown value {v:?}")),
                },
                "theme" => {
                    // `light:Paper,dark:Nord` sets both slots.
                    let mut light = None;
                    let mut dark = None;
                    for part in v.split(',') {
                        match part.trim().split_once(':') {
                            Some(("light", n)) => light = Some(n),
                            Some(("dark", n)) => dark = Some(n),
                            _ => light = Some(part),
                        }
                    }
                    let mut pairs = Vec::new();
                    let mut missing = Vec::new();
                    for (slot, name) in [("theme", light), ("theme-dark", dark)] {
                        let Some(name) = name else { continue };
                        match theme(slot, name) {
                            Outcome::Set(p) => pairs.extend(p),
                            Outcome::Skip(why) => missing.push(why),
                            Outcome::Similar(..) => {}
                        }
                    }
                    if pairs.is_empty() {
                        skip(missing.join("; "))
                    } else {
                        Outcome::Set(pairs)
                    }
                }
                "keybind" => ghostty_keybind(v),
                "unfocused-split-opacity" => number("unfocused-split-opacity", v),
                "shell-integration" => set("shell-integration", Value::Boolean(v != "none")),
                "link-url" => boolean("link-detection", v),
                "window-save-state" => match v {
                    "always" => set("on-launch", s("restore_session")),
                    "never" => set("on-launch", s("new_window")),
                    _ => skip("Ghostty's default; Motty restores the session by default"),
                },
                "window-new-tab-position" => match v {
                    "current" => set("new-tab-position", s("after-current")),
                    "end" => set("new-tab-position", s("end")),
                    _ => skip(format!("unknown value {v:?}")),
                },
                "confirm-close-surface" => {
                    let rule = match v {
                        "always" => "always",
                        "false" => "never",
                        _ => "process",
                    };
                    Outcome::Set(vec![
                        ("confirm-close-pane", s(rule)),
                        ("confirm-close-tab", s(rule)),
                    ])
                }
                "desktop-notifications" => boolean("privilege-notification-shell", v),
                "macos-auto-secure-input" => boolean("auto-secure-input", v),
                "macos-secure-input-indication" => boolean("secure-input-indication", v),
                "right-click-action" => match v {
                    "context-menu" | "copy" | "paste" | "copy-or-paste" | "ignore" => {
                        set("right-click-action", s(v))
                    }
                    _ => skip(format!("unknown value {v:?}")),
                },
                "config-file" => skip("included files aren't followed; import that file too"),
                "mouse-shift-capture" => Outcome::Similar(
                    "shift-arrow-select",
                    "Motty always selects locally with ⇧ held; Ghostty's capture flips that",
                ),
                "window-padding-x" | "window-padding-y" | "window-padding-balance" => {
                    Outcome::Similar(
                        "theme [container].padding",
                        "Motty's padding comes from the theme, not a config key",
                    )
                }
                "window-decoration" => Outcome::Similar(
                    "auto-hide-tab-bar",
                    "Motty has no chrome-off mode; hiding the tab bar comes closest",
                ),
                "background-blur" | "background-blur-radius" => Outcome::Similar(
                    "theme [window].material",
                    "Motty's blur is the theme's material (glass, vibrancy)",
                ),
                "quit-after-last-window-closed-delay" => Outcome::Similar(
                    "quit-after-last-window-closed",
                    "Motty quits at once when it's on",
                ),
                "shell-integration-features" => Outcome::Similar(
                    "shell-integration",
                    "Motty's shell integration is one switch",
                ),
                _ => no_equivalent(),
            };
            Row {
                from: format!("{key} = {value}"),
                outcome,
            }
        })
        .collect()
}

const PALETTE: [&str; 16] = [
    "palette-0",
    "palette-1",
    "palette-2",
    "palette-3",
    "palette-4",
    "palette-5",
    "palette-6",
    "palette-7",
    "palette-8",
    "palette-9",
    "palette-10",
    "palette-11",
    "palette-12",
    "palette-13",
    "palette-14",
    "palette-15",
];

// ---- Kitty ----

fn kitty_action(words: &[&str]) -> Option<String> {
    let to = |s: &str| Some(s.to_owned());
    match words {
        ["new_tab" | "new_tab_with_cwd", ..] => to("new_tab"),
        ["close_tab"] => to("close_tab"),
        ["new_os_window" | "new_os_window_with_cwd", ..] => to("new_window"),
        ["close_os_window"] => to("close_window"),
        ["next_tab"] => to("next_tab"),
        ["previous_tab"] => to("previous_tab"),
        ["goto_tab", n] => goto_tab(n),
        ["copy_to_clipboard"] => to("copy"),
        ["paste_from_clipboard"] => to("paste"),
        ["increase_font_size"] => to("increase_font_size"),
        ["decrease_font_size"] => to("decrease_font_size"),
        ["restore_font_size"] => to("reset_font_size"),
        ["change_font_size", _, amount] => match amount.chars().next() {
            Some('+') => to("increase_font_size"),
            Some('-') => to("decrease_font_size"),
            _ if amount.parse::<f64>() == Ok(0.) => to("reset_font_size"),
            _ => None,
        },
        ["scroll_page_up"] => to("scroll_page_up"),
        ["scroll_page_down"] => to("scroll_page_down"),
        ["scroll_line_up"] => to("scroll_line_up"),
        ["scroll_line_down"] => to("scroll_line_down"),
        ["scroll_home"] => to("scroll_to_top"),
        ["scroll_end"] => to("scroll_to_bottom"),
        ["toggle_fullscreen"] => to("toggle_full_screen"),
        ["quit"] => to("quit"),
        ["clear_terminal", ..] => to("clear"),
        ["send_text", _, rest @ ..] if !rest.is_empty() => Some(format!("text:{}", rest.join(" "))),
        ["neighboring_window", dir] => match *dir {
            "left" => to("focus_pane_left"),
            "right" => to("focus_pane_right"),
            "up" | "top" => to("focus_pane_up"),
            "down" | "bottom" => to("focus_pane_down"),
            _ => None,
        },
        ["next_window"] => to("focus_next_pane"),
        ["previous_window"] => to("focus_previous_pane"),
        ["launch", args @ ..] if args.contains(&"--location=vsplit") => to("split_right"),
        ["launch", args @ ..] if args.contains(&"--location=hsplit") => to("split_down"),
        ["toggle_layout", "stack"] => to("zoom_split"),
        ["edit_config_file"] => to("open_config_file"),
        ["no_op"] => to("unbind"),
        _ => None,
    }
}

fn kitty_map(rest: &str, kitty_mod: &str) -> Outcome {
    let mut words = rest.split_whitespace();
    let Some(keys) = words.next() else {
        return skip("can't read this map");
    };
    // `map --when-focus-on …` and other options.
    if keys.starts_with("--") {
        return skip("Motty has no conditional shortcuts");
    }
    if keys.contains('>') {
        return skip("Motty has no key sequences");
    }
    let action: Vec<&str> = words.collect();
    let keys = keys.replace("kitty_mod", kitty_mod);
    match kitty_action(&action) {
        Some(a) => set("keybind", s(format!("{}={a}", trigger(&keys)))),
        None => skip(format!("Motty has no action like {:?}", action.join(" "))),
    }
}

/// Kitty's `key value` lines.
fn kitty_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
            Some((k.to_owned(), v.trim().to_owned()))
        })
        .collect()
}

pub fn kitty(text: &str) -> Vec<Row> {
    let lines = kitty_lines(text);
    let kitty_mod = lines
        .iter()
        .rfind(|(k, _)| k == "kitty_mod")
        .map_or("ctrl+shift".to_owned(), |(_, v)| v.clone());
    lines
        .into_iter()
        .map(|(key, value)| {
            let v = value.as_str();
            let yes = truthy(v);
            let outcome = match key.as_str() {
                "font_family" => {
                    // `font_family family="JetBrains Mono" style=…` (0.36+).
                    let name = match v.split_once("family=") {
                        Some((_, rest)) => unquote(rest.split(" style=").next().unwrap_or(rest)),
                        None => v,
                    };
                    match name {
                        "monospace" | "auto" | "" => skip("kitty's default font"),
                        name => set("font-family", s(name)),
                    }
                }
                "font_size" => number("font-size", v),
                "disable_ligatures" => match v {
                    "always" => set("font-ligatures", s("off")),
                    "never" => set("font-ligatures", s("dlig")),
                    _ => skip("Motty has no per-cursor ligature setting"),
                },
                "scrollback_lines" => match v.parse::<i64>() {
                    Ok(n) if n >= 0 => set("scrollback-lines", Value::Integer(n)),
                    _ => skip("Motty's scrollback has a limit"),
                },
                "cursor_shape" => match v {
                    "block" => set("cursor-style", s("block")),
                    "beam" => set("cursor-style", s("bar")),
                    "underline" => set("cursor-style", s("underline")),
                    _ => skip(format!("Motty has no cursor shape {v:?}")),
                },
                "cursor_blink_interval" => match v.split_whitespace().next().map(str::parse::<f64>)
                {
                    Some(Ok(0.)) => set("cursor-style-blink", s("default_off")),
                    Some(Ok(_)) => set("cursor-style-blink", s("default_on")),
                    _ => skip("expected a number"),
                },
                "cursor" => color_key("cursor-color", v),
                "cursor_text_color" => color_key("cursor-text", v),
                "foreground" => color_key("foreground", v),
                "background" => color_key("background", v),
                k if k.starts_with("color") => match k["color".len()..].parse::<usize>() {
                    Ok(n @ 0..16) => match color(v) {
                        Some(c) => Outcome::Set(vec![(PALETTE[n], c)]),
                        None => skip(format!("{v:?} isn't a color Motty reads")),
                    },
                    _ => skip("Motty's palette has 16 colors"),
                },
                "shell" if v == "." => skip("kitty's default: your login shell, as in Motty"),
                "shell" => set("command", s(v)),
                "env" => match v.split_once('=') {
                    Some((name, _)) if !name.trim().is_empty() => set("env", s(v)),
                    _ => skip("expected NAME=value"),
                },
                "copy_on_select" => set("copy-on-select", Value::Boolean(v != "no")),
                "strip_trailing_spaces" => set(
                    "clipboard-trim-trailing-spaces",
                    Value::Boolean(v != "never"),
                ),
                "focus_follows_mouse" => match yes {
                    Some(b) => set("focus-follows-mouse", Value::Boolean(b)),
                    None => skip("expected yes or no"),
                },
                // Seconds; 0 never hides the pointer.
                "mouse_hide_wait" => match v.split_whitespace().next().map(str::parse::<f64>) {
                    Some(Ok(n)) => set("mouse-hide-while-typing", Value::Boolean(n != 0.)),
                    _ => skip("expected a number"),
                },
                "macos_option_as_alt" => match v {
                    "left" | "right" => set("macos-option-as-alt", s(v)),
                    "both" => set("macos-option-as-alt", s("true")),
                    _ => match yes {
                        Some(b) => set("macos-option-as-alt", s(b.to_string())),
                        None => skip(format!("unknown value {v:?}")),
                    },
                },
                "confirm_os_window_close" => {
                    match v.split_whitespace().next().map(str::parse::<i64>) {
                        Some(Ok(0)) => set("confirm-close-window", s("never")),
                        Some(Ok(n)) if n < 0 => set("confirm-close-window", s("process")),
                        Some(Ok(_)) => set("confirm-close-window", s("always")),
                        _ => skip("expected a number"),
                    }
                }
                "enable_audio_bell" => match yes {
                    Some(b) => set("privilege-sound-shell", Value::Boolean(b)),
                    None => skip("expected yes or no"),
                },
                "detect_urls" => match yes {
                    Some(b) => set("link-detection", Value::Boolean(b)),
                    None => skip("expected yes or no"),
                },
                "shell_integration" => {
                    set("shell-integration", Value::Boolean(!v.contains("disabled")))
                }
                "clipboard_control" => {
                    let access = |read: bool| {
                        let (allow, ask) = if read {
                            ("read-clipboard", "read-clipboard-ask")
                        } else {
                            ("write-clipboard", "write-clipboard-ask")
                        };
                        let words: Vec<&str> = v.split_whitespace().collect();
                        s(if words.contains(&ask) {
                            "ask"
                        } else if words.contains(&allow) {
                            "allow"
                        } else {
                            "deny"
                        })
                    };
                    Outcome::Set(vec![
                        ("clipboard-read", access(true)),
                        ("clipboard-write", access(false)),
                    ])
                }
                "notify_on_cmd_finish" => set(
                    "privilege-notification-on-finish",
                    Value::Boolean(v.split_whitespace().next() != Some("never")),
                ),
                "map" => kitty_map(v, &kitty_mod),
                "kitty_mod" => skip("used for the map lines"),
                "include" | "globinclude" | "envinclude" => {
                    skip("included files aren't followed; import that file too")
                }
                "editor" => skip("Motty opens files in its own editor"),
                "window_padding_width" => Outcome::Similar(
                    "theme [container].padding",
                    "Motty's padding comes from the theme",
                ),
                "active_tab_background" | "inactive_tab_background" => {
                    Outcome::Similar("theme [tab]", "tab colors come from the theme")
                }
                "tab_bar_edge" | "tab_bar_style" => Outcome::Similar(
                    "window-layout",
                    "tabs-top or tabs-bottom puts a tab bar across the window",
                ),
                "hide_window_decorations" => Outcome::Similar(
                    "auto-hide-tab-bar",
                    "Motty has no chrome-off mode; hiding the tab bar comes closest",
                ),
                "background_blur" => Outcome::Similar(
                    "theme [window].material",
                    "Motty's blur is the theme's material",
                ),
                "cursor_stop_blinking_after" => Outcome::Similar(
                    "cursor-style-blink",
                    "Motty's blink is on or off, without a timeout",
                ),
                "inactive_text_alpha" => Outcome::Similar(
                    "unfocused-split-opacity",
                    "Motty fades the whole unfocused pane, not only its text",
                ),
                "scrollback_pager" => Outcome::Similar(
                    "(built in)",
                    "Motty has its own scrollback search and export",
                ),
                _ => no_equivalent(),
            };
            Row {
                from: format!("{key} {value}"),
                outcome,
            }
        })
        .collect()
}

// ---- Alacritty ----

/// `[a.b] c = 1` as `("a.b.c", 1)`; arrays (of tables too) stay whole.
fn flatten(prefix: &str, table: &toml::Table, out: &mut Vec<(String, Value)>) {
    for (k, v) in table {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Value::Table(t) => flatten(&key, t, out),
            other => out.push((key, other.clone())),
        }
    }
}

const ANSI: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

fn alacritty_key(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let key = match lower.as_str() {
        "return" | "enter" => "enter",
        "pageup" => "pageup",
        "pagedown" => "pagedown",
        "equals" | "equal" => "=",
        "minus" => "-",
        "plus" | "add" => "+",
        "comma" => ",",
        "period" => ".",
        "slash" => "/",
        "backslash" => "\\",
        "semicolon" => ";",
        "apostrophe" => "'",
        "grave" => "`",
        "lbracket" | "bracketleft" => "[",
        "rbracket" | "bracketright" => "]",
        "back" | "backspace" => "backspace",
        "arrowleft" | "left" => "left",
        "arrowright" | "right" => "right",
        "arrowup" | "up" => "up",
        "arrowdown" | "down" => "down",
        "escape" | "tab" | "space" | "home" | "end" | "delete" | "insert" => lower.as_str(),
        k if k.chars().count() == 1 => k,
        k if k.strip_prefix("key").is_some_and(|d| d.len() == 1) => &k[3..],
        k if k.strip_prefix('f').is_some_and(|n| n.parse::<u8>().is_ok()) => k,
        _ => return None,
    };
    Some(key.to_owned())
}

/// Control characters in Alacritty's `chars` as Motty's `text:` escapes.
fn escape_text(chars: &str) -> String {
    let mut out = String::new();
    for c in chars.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '\x1b' => out.push_str("\\e"),
            c if (c as u32) < 0x20 || c == '\x7f' => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn alacritty_binding(b: &Value) -> Outcome {
    let Some(t) = b.as_table() else {
        return skip("can't read this binding");
    };
    if t.contains_key("mode") {
        return skip("Motty has no per-mode shortcuts");
    }
    let Some(key) = t.get("key").and_then(Value::as_str).and_then(alacritty_key) else {
        return skip("Motty has no such key");
    };
    let mut mods = Vec::new();
    for m in t
        .get("mods")
        .and_then(Value::as_str)
        .unwrap_or("")
        .split('|')
    {
        match m.trim().to_ascii_lowercase().as_str() {
            "" | "none" => {}
            "command" | "super" => mods.push("cmd"),
            "control" => mods.push("ctrl"),
            "option" | "alt" => mods.push("alt"),
            "shift" => mods.push("shift"),
            other => return skip(format!("unknown modifier {other:?}")),
        }
    }
    mods.push(&key);
    let trigger = mods.join("+");
    if let Some(chars) = t.get("chars").and_then(Value::as_str) {
        return set(
            "keybind",
            s(format!("{trigger}=text:{}", escape_text(chars))),
        );
    }
    let action = t.get("action").and_then(Value::as_str).unwrap_or("");
    let to = match action {
        "Copy" => "copy",
        "Paste" => "paste",
        "IncreaseFontSize" => "increase_font_size",
        "DecreaseFontSize" => "decrease_font_size",
        "ResetFontSize" => "reset_font_size",
        "ScrollPageUp" => "scroll_page_up",
        "ScrollPageDown" => "scroll_page_down",
        "ScrollLineUp" => "scroll_line_up",
        "ScrollLineDown" => "scroll_line_down",
        "ScrollToTop" => "scroll_to_top",
        "ScrollToBottom" => "scroll_to_bottom",
        "ClearHistory" => "clear",
        "CreateNewWindow" | "SpawnNewInstance" => "new_window",
        "CreateNewTab" => "new_tab",
        "SelectNextTab" => "next_tab",
        "SelectPreviousTab" => "previous_tab",
        "SelectLastTab" => "activate_last_tab",
        "ToggleFullscreen" => "toggle_full_screen",
        "Quit" => "quit",
        "Hide" => "hide_app",
        "Minimize" => "minimize",
        "SearchForward" | "SearchBackward" => "find",
        "None" | "ReceiveChar" => "unbind",
        a => match a.strip_prefix("SelectTab").and_then(goto_tab) {
            Some(tab) => return set("keybind", s(format!("{trigger}={tab}"))),
            None => return skip(format!("Motty has no action like {a:?}")),
        },
    };
    set("keybind", s(format!("{trigger}={to}")))
}

pub fn alacritty(text: &str) -> anyhow::Result<Vec<Row>> {
    let table: toml::Table = toml::from_str(text).context("not an Alacritty TOML config")?;
    let mut flat = Vec::new();
    flatten("", &table, &mut flat);
    let mut rows = Vec::new();
    for (key, value) in flat {
        let text = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        let v = text.as_str();
        let outcome = match key.as_str() {
            "font.size" => number("font-size", v),
            "font.normal.family" => set("font-family", s(v)),
            "scrolling.history" => match value.as_integer() {
                Some(n) => set("scrollback-lines", Value::Integer(n)),
                None => skip("expected a number"),
            },
            "cursor.style" | "cursor.style.shape" => match v.to_ascii_lowercase().as_str() {
                "block" => set("cursor-style", s("block")),
                "beam" => set("cursor-style", s("bar")),
                "underline" => set("cursor-style", s("underline")),
                _ => skip(format!("Motty has no cursor shape {v:?}")),
            },
            "cursor.style.blinking" => match v.to_ascii_lowercase().as_str() {
                "never" => set("cursor-style-blink", s("always_off")),
                "off" => set("cursor-style-blink", s("default_off")),
                "on" => set("cursor-style-blink", s("default_on")),
                "always" => set("cursor-style-blink", s("always_on")),
                _ => skip(format!("unknown value {v:?}")),
            },
            "colors.primary.foreground" => color_key("foreground", v),
            "colors.primary.background" => color_key("background", v),
            "colors.cursor.cursor" => color_key("cursor-color", v),
            "colors.cursor.text" => color_key("cursor-text", v),
            k if k.starts_with("colors.normal.") || k.starts_with("colors.bright.") => {
                let bright = k.starts_with("colors.bright.");
                let name = k.rsplit('.').next().unwrap_or_default();
                match (ANSI.iter().position(|a| *a == name), color(v)) {
                    (Some(i), Some(c)) => {
                        Outcome::Set(vec![(PALETTE[i + if bright { 8 } else { 0 }], c)])
                    }
                    (None, _) => skip("not one of the 16 ANSI colors"),
                    (_, None) => skip(format!("{v:?} isn't a color Motty reads")),
                }
            }
            "selection.save_to_clipboard" => match value.as_bool() {
                Some(b) => set("copy-on-select", Value::Boolean(b)),
                None => skip("expected true or false"),
            },
            "mouse.hide_when_typing" => match value.as_bool() {
                Some(b) => set("mouse-hide-while-typing", Value::Boolean(b)),
                None => skip("expected true or false"),
            },
            "window.dynamic_title" => match value.as_bool() {
                Some(b) => set("title-from-osc", Value::Boolean(b)),
                None => skip("expected true or false"),
            },
            "window.option_as_alt" => match v {
                "Both" => set("macos-option-as-alt", s("true")),
                "OnlyLeft" => set("macos-option-as-alt", s("left")),
                "OnlyRight" => set("macos-option-as-alt", s("right")),
                "None" => set("macos-option-as-alt", s("false")),
                _ => skip(format!("unknown value {v:?}")),
            },
            "terminal.shell" | "shell" | "terminal.shell.program" | "shell.program" => {
                match value.as_str() {
                    Some(program) => {
                        // Arguments live beside the program.
                        let args_key = format!("{}.args", key.trim_end_matches(".program"));
                        let args = table_get(&table, &args_key)
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                            .filter(|a| !a.is_empty());
                        let command = match args {
                            Some(args) => format!("{program} {args}"),
                            None => program.to_owned(),
                        };
                        set("command", s(command))
                    }
                    None => skip("can't read the shell"),
                }
            }
            "terminal.shell.args" | "shell.args" => skip("part of the command line"),
            "env.TERM" => skip("Motty sets TERM itself"),
            k if k.starts_with("env.") => set("env", s(format!("{}={v}", &k["env.".len()..]))),
            "general.working_directory" | "working_directory" => set("working-directory", s(v)),
            "general.import" | "import" => skip("imported files aren't followed; import them too"),
            "keyboard.bindings" => {
                // One row per binding.
                for b in value.as_array().into_iter().flatten() {
                    rows.push(Row {
                        from: format!("[[keyboard.bindings]] {}", inline(b)),
                        outcome: alacritty_binding(b),
                    });
                }
                continue;
            }
            "window.padding.x" | "window.padding.y" => Outcome::Similar(
                "theme [container].padding",
                "Motty's padding comes from the theme",
            ),
            "window.dimensions.columns" | "window.dimensions.lines" => {
                Outcome::Similar("(window size)", "Motty remembers the window's last size")
            }
            "window.decorations" => Outcome::Similar(
                "auto-hide-tab-bar",
                "Motty has no chrome-off mode; hiding the tab bar comes closest",
            ),
            "hints.enabled" => Outcome::Similar(
                "link-detection",
                "Motty finds links under the pointer, and has a hints mode of its own",
            ),
            "mouse.bindings" => Outcome::Similar(
                "right-click-action",
                "Motty's mouse binding is what a right-click does",
            ),
            _ => no_equivalent(),
        };
        rows.push(Row {
            from: format!("{key} = {}", inline(&value)),
            outcome,
        });
    }
    Ok(rows)
}

fn table_get<'a>(table: &'a toml::Table, dotted: &str) -> Option<&'a Value> {
    let mut parts = dotted.split('.');
    let mut value = table.get(parts.next()?)?;
    for p in parts {
        value = value.as_table()?.get(p)?;
    }
    Some(value)
}

/// A value on one line.
fn inline(v: &Value) -> String {
    v.to_string().replace('\n', " ")
}

// ---- Classifying and applying ----

/// An imported value and the line it came from.
#[derive(Clone, Debug)]
pub struct Change {
    pub key: &'static str,
    pub value: Value,
    pub from: String,
}

#[derive(Default, Debug)]
pub struct Plan {
    pub imported: Vec<Change>,
    /// Already what Motty has.
    pub unchanged: Vec<Change>,
    /// Motty has another value (in `current`).
    pub conflicts: Vec<(Change, Value)>,
    pub similar: Vec<(String, &'static str, &'static str)>,
    pub skipped: Vec<(String, String)>,
}

/// Values compared the way a person would: numbers by value, text without
/// case (`#1E1E2E`, theme names).
fn same(a: &Value, b: &Value) -> bool {
    let num = |v: &Value| v.as_float().or_else(|| v.as_integer().map(|i| i as f64));
    match (num(a), num(b), a.as_str(), b.as_str()) {
        (Some(x), Some(y), ..) => x == y,
        (_, _, Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
        _ => a == b,
    }
}

/// Sorts `rows` into buckets against what Motty has (`current` gives a
/// key's entries from the config files; empty when unset).
pub fn plan(rows: Vec<Row>, current: &dyn Fn(&str) -> Vec<Value>) -> Plan {
    let mut out = Plan::default();
    // A later line for a single key wins, as in the source terminal.
    let mut last: HashMap<&'static str, usize> = HashMap::new();
    let mut changes = Vec::new();
    for row in rows {
        match row.outcome {
            Outcome::Set(pairs) => {
                for (key, value) in pairs {
                    let list = schema::lookup(key).and_then(|k| k.list).is_some();
                    if !list && let Some(&i) = last.get(key) {
                        changes[i] = None;
                    }
                    last.insert(key, changes.len());
                    changes.push(Some(Change {
                        key,
                        value,
                        from: row.from.clone(),
                    }));
                }
            }
            Outcome::Similar(motty, note) => out.similar.push((row.from, motty, note)),
            Outcome::Skip(why) => out.skipped.push((row.from, why)),
        }
    }
    for change in changes.into_iter().flatten() {
        let spec = schema::lookup(change.key).expect("translators write known keys");
        let have = current(change.key);
        let existing = match spec.list {
            // Named entries match by name, others whole.
            Some(_) => {
                let text = schema::display(&change.value);
                let name = spec.entry_name(&text);
                have.into_iter().find(|v| {
                    let t = schema::display(v);
                    t == text || (name.is_some() && spec.entry_name(&t) == name)
                })
            }
            None => have.into_iter().last(),
        };
        match existing {
            None => out.imported.push(change),
            Some(v) if same(&v, &change.value) => out.unchanged.push(change),
            Some(v) => out.conflicts.push((change, v)),
        }
    }
    out
}

/// Writes the plan's imports (and conflicts, with `overwrite`).
fn apply(plan: &Plan, files: &Files, overwrite: bool) -> anyhow::Result<usize> {
    let mut wanted: Vec<&Change> = plan.imported.iter().collect();
    if overwrite {
        wanted.extend(plan.conflicts.iter().map(|(c, _)| c));
    }
    // Lists gather their entries first, then one write each.
    let mut lists: Vec<(&'static str, Vec<Value>)> = Vec::new();
    for change in &wanted {
        let spec = schema::lookup(change.key).expect("known key");
        if spec.list.is_none() {
            files.set(change.key, std::slice::from_ref(&change.value), false)?;
            continue;
        }
        let i = match lists.iter().position(|(k, _)| *k == change.key) {
            Some(i) => i,
            None => {
                lists.push((change.key, files.values(change.key)));
                lists.len() - 1
            }
        };
        let entries = &mut lists[i].1;
        let text = schema::display(&change.value);
        let name = spec.entry_name(&text);
        match entries.iter().position(|v| {
            let t = schema::display(v);
            t == text || (name.is_some() && spec.entry_name(&t) == name)
        }) {
            Some(at) => entries[at] = change.value.clone(),
            None => entries.push(change.value.clone()),
        }
    }
    for (key, entries) in &lists {
        files.set(key, entries, false)?;
    }
    Ok(wanted.len())
}

fn import_config(
    terminal: Terminal,
    path: &Path,
    args: &ImportArgs,
    json: bool,
) -> anyhow::Result<()> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("can't read {}", path.display()))?;
    let rows = match terminal {
        Terminal::Ghostty => ghostty(&text),
        Terminal::Kitty => kitty(&text),
        Terminal::Alacritty => alacritty(&text)?,
    };
    let files = Files::current();
    let plan = plan(rows, &|key| files.values(key));
    let write = args.overwrite || args.keep;
    let written = if write {
        apply(&plan, &files, args.overwrite)?
    } else {
        0
    };
    if json {
        let change =
            |c: &Change| json!({ "key": c.key, "value": to_json(&c.value), "from": c.from });
        print_json(json!({ "data": {
            "source": terminal.name(),
            "path": path,
            "applied": write,
            "written": written,
            "imported": plan.imported.iter().map(change).collect::<Vec<_>>(),
            "unchanged": plan.unchanged.iter().map(change).collect::<Vec<_>>(),
            "conflicts": plan.conflicts.iter().map(|(c, current)| json!({
                "key": c.key,
                "value": to_json(&c.value),
                "current": to_json(current),
                "from": c.from,
                "overwritten": args.overwrite,
            })).collect::<Vec<_>>(),
            "similar": plan.similar.iter().map(|(from, motty, note)| json!({
                "from": from, "motty": motty, "note": note,
            })).collect::<Vec<_>>(),
            "skipped": plan.skipped.iter().map(|(from, why)| json!({
                "from": from, "reason": why,
            })).collect::<Vec<_>>(),
        }}));
        return Ok(());
    }
    println!("{} config: {}", terminal.name(), path.display());
    let line = |c: &Change| schema::line(c.key, &c.value);
    if !plan.imported.is_empty() {
        println!("\nImported ({}):", plan.imported.len());
        for c in &plan.imported {
            println!("  {:<44} ← {}", line(c), c.from);
        }
    }
    if !plan.conflicts.is_empty() {
        let what = if args.overwrite {
            "overwritten"
        } else {
            "kept Motty's"
        };
        println!("\nConflicts ({}, {what}):", plan.conflicts.len());
        for (c, current) in &plan.conflicts {
            println!(
                "  {}: {} → {}   ← {}",
                c.key,
                schema::display(current),
                schema::display(&c.value),
                c.from
            );
        }
    }
    if !plan.unchanged.is_empty() {
        println!("\nAlready set ({}):", plan.unchanged.len());
        for c in &plan.unchanged {
            println!("  {}", line(c));
        }
    }
    if !plan.similar.is_empty() {
        println!("\nSimilar, not imported ({}):", plan.similar.len());
        for (from, motty, note) in &plan.similar {
            println!("  {from}  ≈ {motty}: {note}");
        }
    }
    if !plan.skipped.is_empty() {
        println!("\nSkipped ({}):", plan.skipped.len());
        for (from, why) in &plan.skipped {
            println!("  {from}: {why}");
        }
    }
    if write {
        println!("\nWrote {written} settings to {}.", files.main.display());
    } else {
        println!(
            "\nNothing written. Run again with --overwrite (take {}'s values on conflicts) or --keep (keep Motty's).",
            terminal.name()
        );
    }
    Ok(())
}

// ---- zoxide ----

/// A frequent folder, as the app stores it (`folders.json`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Folder {
    pub rank: f64,
    /// Unix seconds.
    pub last: u64,
}

/// Merges `zoxide query --list --score` output into `db`: zoxide's score
/// becomes the folder's rank (the higher wins, so importing twice changes
/// nothing). New folders count as visited a day ago, so they don't outrank
/// the ones you were in this hour. Returns how many were added.
pub fn merge_zoxide(
    db: &mut HashMap<PathBuf, Folder>,
    listing: &str,
    now: u64,
    home: &Path,
) -> usize {
    let mut added = 0;
    for line in listing.lines() {
        let Some((score, path)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let (Ok(score), path) = (score.parse::<f64>(), PathBuf::from(path.trim())) else {
            continue;
        };
        if path == home || !path.is_absolute() {
            continue;
        }
        let entry = db.entry(path).or_insert_with(|| {
            added += 1;
            Folder {
                rank: 0.,
                last: now.saturating_sub(86_400),
            }
        });
        entry.rank = entry.rank.max(score);
    }
    added
}

fn zoxide(json: bool) -> anyhow::Result<()> {
    let output = std::process::Command::new("zoxide")
        .args(["query", "--list", "--score"])
        .output()
        .context("couldn't run zoxide (is it installed?)")?;
    if !output.status.success() {
        bail!(
            "zoxide failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    let file = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Motty/folders.json");
    let mut db: HashMap<PathBuf, Folder> = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let home = dirs::home_dir().unwrap_or_default();
    let added = merge_zoxide(&mut db, &listing, now, &home);
    let total = listing.lines().filter(|l| !l.trim().is_empty()).count();
    std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&db)?)?;
    std::fs::rename(&tmp, &file)?;
    if json {
        print_json(json!({ "data": { "folders": total, "added": added, "path": file } }));
    } else {
        println!(
            "Merged {total} zoxide folders ({added} new) into {}",
            file.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sets(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .filter_map(|r| match &r.outcome {
                Outcome::Set(pairs) => Some(
                    pairs
                        .iter()
                        .map(|(k, v)| schema::line(k, v))
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
                _ => None,
            })
            .collect()
    }

    fn skipped<'a>(rows: &'a [Row], from: &str) -> Option<&'a Outcome> {
        rows.iter()
            .find(|r| r.from.starts_with(from))
            .map(|r| &r.outcome)
            .filter(|o| !matches!(o, Outcome::Set(_)))
    }

    #[test]
    fn ghostty_fixture() {
        let rows = ghostty(include_str!("../tests/fixtures/ghostty.conf"));
        let got = sets(&rows);
        for want in [
            "font-family = \"JetBrains Mono\"",
            "font-size = 14.0",
            "command = \"/opt/homebrew/bin/fish\"",
            "env = \"EDITOR=nvim\"",
            "working-directory = \"home\"",
            "scrollback-lines = 125000",
            "background = \"#1e1e2e\"",
            "palette-1 = \"#f38ba8\"",
            "cursor-style = \"bar\"",
            "cursor-style-blink = \"default_on\"",
            "macos-option-as-alt = \"left\"",
            "copy-on-select = true",
            "clipboard-read = \"allow\"",
            "theme = \"Nord\"; theme-dark = \"Dracula\"",
            "keybind = cmd+t=new_tab",
            "keybind = cmd+d=split_right",
            "keybind = ctrl+shift+1=goto_tab:1",
            "keybind = cmd+k=text:\\x0c",
            "font-ligatures = \"off\"",
            "unfocused-split-opacity = 0.8",
            "shell-integration = false",
            "on-launch = \"new_window\"",
            "new-tab-position = \"after-current\"",
            "confirm-close-pane = \"always\"; confirm-close-tab = \"always\"",
            "tab-working-directory = \"home\"; split-working-directory = \"home\"",
        ] {
            assert!(
                got.iter().any(|g| g == want),
                "missing {want}; got {got:#?}"
            );
        }
        assert!(matches!(
            skipped(&rows, "font-family = Symbols"),
            Some(Outcome::Skip(_))
        ));
        assert!(matches!(
            skipped(&rows, "background-opacity"),
            Some(Outcome::Skip(_))
        ));
        assert!(matches!(
            skipped(&rows, "window-padding-x"),
            Some(Outcome::Similar(..))
        ));
        assert!(matches!(
            skipped(&rows, "keybind = global:"),
            Some(Outcome::Skip(_))
        ));
        assert!(matches!(
            skipped(&rows, "keybind = cmd+shift+w=write_scrollback_file"),
            Some(Outcome::Skip(_))
        ));
        // Every translated value is one Motty's schema takes.
        for row in &rows {
            if let Outcome::Set(pairs) = &row.outcome {
                for (k, v) in pairs {
                    let spec = schema::lookup(k).unwrap();
                    assert_eq!(schema::check(spec, v), Ok(()), "{k} = {v}");
                }
            }
        }
    }

    #[test]
    fn kitty_fixture() {
        let rows = kitty(include_str!("../tests/fixtures/kitty.conf"));
        let got = sets(&rows);
        for want in [
            "font-family = \"Fira Code\"",
            "font-size = 13.5",
            "scrollback-lines = 5000",
            "cursor-style = \"bar\"",
            "cursor-color = \"#cccccc\"",
            "foreground = \"#dddddd\"",
            "palette-0 = \"#000000\"",
            "palette-15 = \"#ffffff\"",
            "command = \"/bin/zsh\"",
            "env = \"LANG=en_US.UTF-8\"",
            "copy-on-select = true",
            "clipboard-trim-trailing-spaces = true",
            "mouse-hide-while-typing = true",
            "macos-option-as-alt = \"true\"",
            "cursor-style-blink = \"default_off\"",
            "clipboard-read = \"ask\"; clipboard-write = \"allow\"",
            "keybind = ctrl+shift+t=new_tab",
            "keybind = cmd+equal=increase_font_size",
            "keybind = cmd+0=reset_font_size",
            "keybind = ctrl+shift+left=focus_pane_left",
            "keybind = cmd+e=text:\\x1b[A",
            "font-ligatures = \"off\"",
            "confirm-close-window = \"never\"",
        ] {
            assert!(
                got.iter().any(|g| g == want),
                "missing {want}; got {got:#?}"
            );
        }
        assert!(matches!(
            skipped(&rows, "background_opacity"),
            Some(Outcome::Skip(_))
        ));
        assert!(matches!(
            skipped(&rows, "tab_bar_edge"),
            Some(Outcome::Similar(..))
        ));
        assert!(matches!(
            skipped(&rows, "map kitty_mod+x>"),
            Some(Outcome::Skip(_))
        ));
        for row in &rows {
            if let Outcome::Set(pairs) = &row.outcome {
                for (k, v) in pairs {
                    assert_eq!(
                        schema::check(schema::lookup(k).unwrap(), v),
                        Ok(()),
                        "{k} = {v}"
                    );
                }
            }
        }
    }

    #[test]
    fn alacritty_fixture() {
        let rows = alacritty(include_str!("../tests/fixtures/alacritty.toml")).unwrap();
        let got = sets(&rows);
        for want in [
            "font-size = 12.5",
            "font-family = \"Hack\"",
            "scrollback-lines = 20000",
            "cursor-style = \"underline\"",
            "cursor-style-blink = \"always_on\"",
            "foreground = \"#c5c8c6\"",
            "background = \"#1d1f21\"",
            "palette-1 = \"#cc6666\"",
            "palette-9 = \"#d54e53\"",
            "copy-on-select = true",
            "mouse-hide-while-typing = true",
            "macos-option-as-alt = \"left\"",
            "title-from-osc = false",
            "command = \"/bin/zsh -l\"",
            "env = \"EDITOR=hx\"",
            "working-directory = \"/tmp\"",
            "keybind = cmd+t=new_tab",
            "keybind = cmd+shift+==increase_font_size",
            "keybind = cmd+3=goto_tab:3",
            "keybind = alt+left=text:\\eb",
        ] {
            assert!(
                got.iter().any(|g| g == want),
                "missing {want}; got {got:#?}"
            );
        }
        assert!(matches!(skipped(&rows, "env.TERM"), Some(Outcome::Skip(_))));
        assert!(matches!(
            skipped(&rows, "window.padding.x"),
            Some(Outcome::Similar(..))
        ));
        assert!(matches!(
            skipped(&rows, "window.opacity"),
            Some(Outcome::Skip(_))
        ));
        for row in &rows {
            if let Outcome::Set(pairs) = &row.outcome {
                for (k, v) in pairs {
                    assert_eq!(
                        schema::check(schema::lookup(k).unwrap(), v),
                        Ok(()),
                        "{k} = {v}"
                    );
                }
            }
        }
    }

    #[test]
    fn plan_sorts_into_buckets() {
        let rows = ghostty(
            "font-size = 14\nfont-size = 15\ntheme = Nord\nenv = EDITOR=nvim\nenv = PAGER=less\ncursor-style = bar\nbackground-opacity = 0.9\nwindow-padding-x = 4\n",
        );
        let current = |key: &str| -> Vec<Value> {
            match key {
                "font-size" => vec![Value::Integer(13)],
                "theme" => vec![s("nord")],
                "env" => vec![s("EDITOR=vim")],
                _ => Vec::new(),
            }
        };
        let plan = plan(rows, &current);
        let keys = |c: &[Change]| c.iter().map(|c| c.key).collect::<Vec<_>>();
        assert_eq!(keys(&plan.imported), vec!["env", "cursor-style"]);
        assert_eq!(keys(&plan.unchanged), vec!["theme"]);
        let conflicts: Vec<_> = plan
            .conflicts
            .iter()
            .map(|(c, cur)| (c.key, schema::display(&c.value), schema::display(cur)))
            .collect();
        // The later font-size line wins; EDITOR is the same entry by name.
        assert_eq!(
            conflicts,
            vec![
                ("font-size", "15.0".into(), "13".into()),
                ("env", "EDITOR=nvim".into(), "EDITOR=vim".into()),
            ]
        );
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.similar.len(), 1);
    }

    #[test]
    fn applying_keeps_or_overwrites() {
        let dir = std::env::temp_dir().join(format!(
            "motty-import-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let files = Files::at(dir.join("config.toml"));
        std::fs::write(
            &files.main,
            "# mine\nfont-size = 13\nenv = \"EDITOR=vim\"\n",
        )
        .unwrap();
        let rows =
            ghostty("font-size = 15\nenv = EDITOR=nvim\nenv = PAGER=less\ncursor-style = bar\n");
        let p = plan(rows.clone(), &|k| files.values(k));
        assert_eq!(apply(&p, &files, false).unwrap(), 2);
        let src = std::fs::read_to_string(&files.main).unwrap();
        assert_eq!(
            src,
            "# mine\nfont-size = 13\nenv = \"EDITOR=vim\"\nenv = \"PAGER=less\"\ncursor-style = \"bar\"\n"
        );
        let p = plan(rows, &|k| files.values(k));
        assert_eq!(apply(&p, &files, true).unwrap(), 2);
        let src = std::fs::read_to_string(&files.main).unwrap();
        assert_eq!(
            src,
            "# mine\nfont-size = 15.0\nenv = \"EDITOR=nvim\"\nenv = \"PAGER=less\"\ncursor-style = \"bar\"\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn zoxide_scores_merge_once() {
        let tmp = std::env::temp_dir();
        let a = tmp.join("a");
        let mut db = HashMap::new();
        db.insert(
            a.clone(),
            Folder {
                rank: 50.,
                last: 1000,
            },
        );
        let listing = format!(
            "  12.5 {}\n 100.0 {}\n   3.0 /home/me\nnot a line\n",
            a.display(),
            tmp.join("b c").display()
        );
        let added = merge_zoxide(&mut db, &listing, 200_000, Path::new("/home/me"));
        assert_eq!(added, 1);
        assert_eq!(
            db[&a],
            Folder {
                rank: 50.,
                last: 1000
            }
        );
        assert_eq!(
            db[&tmp.join("b c")],
            Folder {
                rank: 100.,
                last: 200_000 - 86_400
            }
        );
        assert_eq!(
            merge_zoxide(&mut db, &listing, 300_000, Path::new("/home/me")),
            0
        );
        assert_eq!(db.len(), 2);
    }
}
