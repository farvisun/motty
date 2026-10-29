//! `motty export`: the effective config as another terminal's config file.
//! Every key the target takes is written (defaults included), colors come
//! from the active theme, and the keys you set that the target can't take
//! are reported as dropped.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::Args;
use motty_theme::store::Files;
use motty_theme::{Config, CursorBlink, CursorStyle, Mode, Theme, WorkingDir};
use serde_json::json;

use crate::config_cmd::print_json;
use crate::import::Terminal;

#[derive(Args)]
pub struct ExportArgs {
    #[arg(value_enum)]
    pub target: Terminal,
    /// Write to a file instead of stdout.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

/// The exported file and the Motty keys it carries.
pub struct Exported {
    pub text: String,
    pub covered: Vec<&'static str>,
}

/// Keys every target takes, through the theme's colors.
const COLORS: [&str; 6] = [
    "theme",
    "theme-dark",
    "foreground",
    "background",
    "cursor-color",
    "cursor-text",
];

fn hex(c: motty_theme::Rgba) -> String {
    motty_theme::import::hex(c).to_lowercase()
}

/// The system's appearance, for picking `theme` or `theme-dark`.
fn system_mode() -> Mode {
    let dark = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "Dark");
    if dark { Mode::Dark } else { Mode::Light }
}

pub fn run(args: &ExportArgs, json: bool) -> anyhow::Result<ExitCode> {
    let files = Files::current();
    let config = files.load(&[]);
    let theme = config.resolve_theme(system_mode()).theme;
    let exported = export(args.target, &config, &theme);
    // What you set that doesn't make it across.
    let mut dropped: Vec<String> = Vec::new();
    for (key, _) in files.entries().0 {
        let covered = exported.covered.contains(&key.as_str())
            || key.starts_with("palette-")
            || COLORS.contains(&key.as_str());
        if !covered && !dropped.contains(&key) {
            dropped.push(key);
        }
    }
    if let Some(path) = &args.output {
        std::fs::write(path, &exported.text)
            .with_context(|| format!("can't write {}", path.display()))?;
    }
    if json {
        print_json(json!({ "data": {
            "target": args.target.name(),
            "path": args.output,
            "text": args.output.is_none().then_some(&exported.text),
            "dropped": dropped,
        }}));
        return Ok(ExitCode::SUCCESS);
    }
    if args.output.is_none() {
        print!("{}", exported.text);
    }
    if !dropped.is_empty() {
        eprintln!(
            "motty: not in {}'s config: {}",
            args.target.name(),
            dropped.join(", ")
        );
    }
    Ok(ExitCode::SUCCESS)
}

pub fn export(target: Terminal, config: &Config, theme: &Theme) -> Exported {
    match target {
        Terminal::Ghostty => ghostty(config, theme),
        Terminal::Kitty => kitty(config, theme),
        Terminal::Alacritty => alacritty(config, theme),
    }
}

fn family(config: &Config) -> &str {
    config.font_family.as_deref().unwrap_or("JetBrains Mono")
}

fn size(config: &Config) -> f32 {
    config.font_size.unwrap_or(13.)
}

fn working_dir(config: &Config) -> Option<String> {
    config.working_directory.as_ref().map(WorkingDir::key)
}

/// A Motty keybind action in Ghostty's terms.
fn ghostty_action(action: &str) -> Option<String> {
    let to = |s: &str| Some(s.to_owned());
    if ["text:", "csi:", "esc:", "goto_tab:"]
        .iter()
        .any(|p| action.starts_with(p))
    {
        return to(action);
    }
    match action {
        "copy" => to("copy_to_clipboard"),
        "paste" => to("paste_from_clipboard"),
        "clear" => to("clear_screen"),
        "split_right" => to("new_split:right"),
        "split_left" => to("new_split:left"),
        "split_down" => to("new_split:down"),
        "split_up" => to("new_split:up"),
        "focus_pane_left" => to("goto_split:left"),
        "focus_pane_right" => to("goto_split:right"),
        "focus_pane_up" => to("goto_split:up"),
        "focus_pane_down" => to("goto_split:down"),
        "focus_next_pane" => to("goto_split:next"),
        "focus_previous_pane" => to("goto_split:previous"),
        "zoom_split" => to("toggle_split_zoom"),
        "toggle_full_screen" => to("toggle_fullscreen"),
        "activate_last_tab" => to("last_tab"),
        "command_palette" => to("toggle_command_palette"),
        "open_config_file" => to("open_config"),
        "increase_font_size" => to("increase_font_size:1"),
        "decrease_font_size" => to("decrease_font_size:1"),
        "unbind" | "new_tab" | "close_tab" | "new_window" | "close_window" | "next_tab"
        | "previous_tab" | "reset_font_size" | "select_all" | "scroll_to_top"
        | "scroll_to_bottom" | "scroll_page_up" | "scroll_page_down" | "equalize_splits"
        | "quit" | "undo" | "redo" => to(action),
        _ => None,
    }
}

fn ghostty(config: &Config, theme: &Theme) -> Exported {
    let mut out = String::from("# Exported from Motty.\n");
    let mut covered = vec!["font-family", "font-size"];
    let mut line = |covered: &mut Vec<&'static str>, key: &'static str, k: &str, v: String| {
        if !key.is_empty() {
            covered.push(key);
        }
        let _ = writeln!(out, "{k} = {v}");
    };
    line(
        &mut covered,
        "",
        "font-family",
        format!("\"{}\"", family(config)),
    );
    line(&mut covered, "", "font-size", size(config).to_string());
    if let Some(shell) = &config.shell {
        line(&mut covered, "command", "command", shell.clone());
    }
    for (name, value) in &config.env {
        line(&mut covered, "env", "env", format!("{name}={value}"));
    }
    if let Some(dir) = working_dir(config) {
        line(&mut covered, "working-directory", "working-directory", dir);
    }
    let style = match config.cursor_style {
        CursorStyle::Block => "block",
        CursorStyle::Bar => "bar",
        CursorStyle::Underline => "underline",
        CursorStyle::BlockHollow => "block_hollow",
    };
    line(&mut covered, "cursor-style", "cursor-style", style.into());
    line(
        &mut covered,
        "cursor-style-blink",
        "cursor-style-blink",
        config.cursor_blink.initial().to_string(),
    );
    let t = &theme.terminal;
    line(&mut covered, "", "foreground", hex(t.foreground));
    line(&mut covered, "", "background", hex(t.background));
    for (i, c) in t.palette.iter().enumerate() {
        line(&mut covered, "", "palette", format!("{i}={}", hex(*c)));
    }
    line(&mut covered, "", "cursor-color", hex(theme.cursor.color));
    if let Some(text) = theme.cursor.text {
        line(&mut covered, "", "cursor-text", hex(text));
    }
    line(
        &mut covered,
        "",
        "selection-background",
        hex(t.selection_background),
    );
    let b = &config.behavior;
    let access = |a: motty_theme::Access| match a {
        motty_theme::Access::Ask => "ask",
        motty_theme::Access::Allow => "allow",
        motty_theme::Access::Deny => "deny",
    };
    for (key, k, v) in [
        (
            "mouse-hide-while-typing",
            "mouse-hide-while-typing",
            b.hide_mouse_while_typing.to_string(),
        ),
        (
            "focus-follows-mouse",
            "focus-follows-mouse",
            b.focus_follows_mouse.to_string(),
        ),
        (
            "macos-option-as-alt",
            "macos-option-as-alt",
            config.option_as_meta.to_string(),
        ),
        (
            "clipboard-read",
            "clipboard-read",
            access(config.clipboard_read).into(),
        ),
        (
            "clipboard-write",
            "clipboard-write",
            access(config.clipboard_write).into(),
        ),
        (
            "clipboard-trim-trailing-spaces",
            "clipboard-trim-trailing-spaces",
            b.trim_trailing_spaces.to_string(),
        ),
        (
            "copy-on-select",
            "copy-on-select",
            b.copy_on_select.to_string(),
        ),
        (
            "clipboard-paste-protection",
            "clipboard-paste-protection",
            b.paste_protection.to_string(),
        ),
        (
            "clipboard-paste-bracketed-safe",
            "clipboard-paste-bracketed-safe",
            b.paste_bracketed_safe.to_string(),
        ),
        (
            "selection-clear-on-typing",
            "selection-clear-on-typing",
            b.clear_selection_on_typing.to_string(),
        ),
        // Ghostty's floor is 0.15.
        (
            "unfocused-split-opacity",
            "unfocused-split-opacity",
            config
                .unfocused_split_opacity
                .unwrap_or(0.6)
                .clamp(0.15, 1.)
                .to_string(),
        ),
        (
            "quit-after-last-window-closed",
            "quit-after-last-window-closed",
            config.quit_after_last_window.to_string(),
        ),
        (
            "shell-integration",
            "shell-integration",
            (if config.shell_integration {
                "detect"
            } else {
                "none"
            })
            .into(),
        ),
        (
            "link-detection",
            "link-url",
            config.link_detection.to_string(),
        ),
        (
            "on-launch",
            "window-save-state",
            (if config.restore_session {
                "always"
            } else {
                "never"
            })
            .into(),
        ),
        (
            "new-tab-position",
            "window-new-tab-position",
            (if config.new_tab_position == motty_theme::NewTabPosition::AfterCurrent {
                "current"
            } else {
                "end"
            })
            .into(),
        ),
        (
            "confirm-close-tab",
            "confirm-close-surface",
            match config.confirm_close_tab {
                motty_theme::ConfirmClose::Always => "always",
                motty_theme::ConfirmClose::Never => "false",
                _ => "true",
            }
            .into(),
        ),
        (
            "privilege-notification-shell",
            "desktop-notifications",
            config.badges.notify_shell.to_string(),
        ),
        (
            "auto-secure-input",
            "macos-auto-secure-input",
            b.auto_secure_input.to_string(),
        ),
        (
            "secure-input-indication",
            "macos-secure-input-indication",
            b.secure_input_indication.to_string(),
        ),
        (
            "right-click-action",
            "right-click-action",
            match config.right_click {
                motty_theme::RightClick::ContextMenu => "context-menu",
                motty_theme::RightClick::Copy => "copy",
                motty_theme::RightClick::Paste => "paste",
                motty_theme::RightClick::CopyOrPaste => "copy-or-paste",
                motty_theme::RightClick::Ignore => "ignore",
            }
            .into(),
        ),
    ] {
        line(&mut covered, key, k, v);
    }
    let mut any_keybind = false;
    for kb in &config.keybinds {
        if let Some(action) = ghostty_action(&kb.action) {
            any_keybind = true;
            let _ = writeln!(out, "keybind = {}={action}", kb.trigger);
        }
    }
    if any_keybind || config.keybinds.is_empty() {
        covered.push("keybind");
    }
    Exported { text: out, covered }
}

/// A Motty keybind action in kitty's terms.
fn kitty_action(action: &str) -> Option<String> {
    let to = |s: &str| Some(s.to_owned());
    if let Some(text) = action.strip_prefix("text:") {
        return Some(format!("send_text all {text}"));
    }
    if let Some(n) = action.strip_prefix("goto_tab:") {
        return Some(format!("goto_tab {n}"));
    }
    match action {
        "new_tab" | "close_tab" | "next_tab" | "previous_tab" | "quit" | "scroll_page_up"
        | "scroll_page_down" | "scroll_line_up" | "scroll_line_down" | "toggle_fullscreen" => {
            to(action)
        }
        "new_window" => to("new_os_window"),
        "close_window" => to("close_os_window"),
        "copy" => to("copy_to_clipboard"),
        "paste" => to("paste_from_clipboard"),
        "increase_font_size" => to("change_font_size all +1.0"),
        "decrease_font_size" => to("change_font_size all -1.0"),
        "reset_font_size" => to("change_font_size all 0"),
        "scroll_to_top" => to("scroll_home"),
        "scroll_to_bottom" => to("scroll_end"),
        "toggle_full_screen" => to("toggle_fullscreen"),
        "clear" => to("clear_terminal clear active"),
        "split_right" => to("launch --location=vsplit"),
        "split_down" => to("launch --location=hsplit"),
        "focus_pane_left" => to("neighboring_window left"),
        "focus_pane_right" => to("neighboring_window right"),
        "focus_pane_up" => to("neighboring_window up"),
        "focus_pane_down" => to("neighboring_window down"),
        "focus_next_pane" => to("next_window"),
        "focus_previous_pane" => to("previous_window"),
        "zoom_split" => to("toggle_layout stack"),
        "open_config_file" => to("edit_config_file"),
        "unbind" => to("no_op"),
        _ => None,
    }
}

fn kitty(config: &Config, theme: &Theme) -> Exported {
    let mut out = String::from("# Exported from Motty.\n");
    let mut covered: Vec<&'static str> = Vec::new();
    let mut line = |key: &'static str, k: &str, v: String| {
        if !key.is_empty() {
            covered.push(key);
        }
        let _ = writeln!(out, "{k} {v}");
    };
    let yes = |b: bool| if b { "yes" } else { "no" }.to_owned();
    line("font-family", "font_family", family(config).into());
    line("font-size", "font_size", size(config).to_string());
    line(
        "font-ligatures",
        "disable_ligatures",
        if config.ligatures { "never" } else { "always" }.into(),
    );
    if let Some(shell) = &config.shell {
        line("command", "shell", shell.clone());
    }
    for (name, value) in &config.env {
        line("env", "env", format!("{name}={value}"));
    }
    line(
        "scrollback-lines",
        "scrollback_lines",
        config.scrollback.unwrap_or(10_000).to_string(),
    );
    let shape = match config.cursor_style {
        CursorStyle::Block | CursorStyle::BlockHollow => "block",
        CursorStyle::Bar => "beam",
        CursorStyle::Underline => "underline",
    };
    line("cursor-style", "cursor_shape", shape.into());
    line(
        "cursor-style-blink",
        "cursor_blink_interval",
        if config.cursor_blink.initial() {
            "-1"
        } else {
            "0"
        }
        .into(),
    );
    let t = &theme.terminal;
    line("", "foreground", hex(t.foreground));
    line("", "background", hex(t.background));
    line("", "selection_background", hex(t.selection_background));
    line("", "cursor", hex(theme.cursor.color));
    if let Some(text) = theme.cursor.text {
        line("", "cursor_text_color", hex(text));
    }
    for (i, c) in t.palette.iter().enumerate() {
        line("", &format!("color{i}"), hex(*c));
    }
    let b = &config.behavior;
    line(
        "copy-on-select",
        "copy_on_select",
        if b.copy_on_select { "clipboard" } else { "no" }.into(),
    );
    line(
        "clipboard-trim-trailing-spaces",
        "strip_trailing_spaces",
        if b.trim_trailing_spaces {
            "always"
        } else {
            "never"
        }
        .into(),
    );
    line(
        "focus-follows-mouse",
        "focus_follows_mouse",
        yes(b.focus_follows_mouse),
    );
    line(
        "mouse-hide-while-typing",
        "mouse_hide_wait",
        if b.hide_mouse_while_typing { "-1" } else { "0" }.into(),
    );
    line(
        "macos-option-as-alt",
        "macos_option_as_alt",
        yes(config.option_as_meta),
    );
    let read = match config.clipboard_read {
        motty_theme::Access::Allow => " read-clipboard",
        motty_theme::Access::Ask => " read-clipboard-ask",
        motty_theme::Access::Deny => "",
    };
    let write = match config.clipboard_write {
        motty_theme::Access::Allow => "write-clipboard",
        motty_theme::Access::Ask => "write-clipboard-ask",
        motty_theme::Access::Deny => "no-append",
    };
    line(
        "clipboard-write",
        "clipboard_control",
        format!("{write}{read}"),
    );
    line(
        "confirm-close-window",
        "confirm_os_window_close",
        match config.confirm_close_window {
            motty_theme::ConfirmClose::Never => "0",
            motty_theme::ConfirmClose::Always => "1",
            _ => "-1",
        }
        .into(),
    );
    line(
        "privilege-sound-shell",
        "enable_audio_bell",
        yes(config.badges.sound_shell),
    );
    line("link-detection", "detect_urls", yes(config.link_detection));
    line(
        "shell-integration",
        "shell_integration",
        if config.shell_integration {
            "enabled"
        } else {
            "disabled"
        }
        .into(),
    );
    line(
        "privilege-notification-on-finish",
        "notify_on_cmd_finish",
        if config.badges.notify_finish {
            "unfocused"
        } else {
            "never"
        }
        .into(),
    );
    let mut any = false;
    for kb in &config.keybinds {
        if let Some(action) = kitty_action(&kb.action) {
            any = true;
            let _ = writeln!(out, "map {} {action}", kb.trigger);
        }
    }
    if any || config.keybinds.is_empty() {
        covered.push("keybind");
    }
    // In clipboard_control with clipboard-write.
    covered.push("clipboard-read");
    Exported { text: out, covered }
}

/// A Motty keybind as an Alacritty `[[keyboard.bindings]]` table.
fn alacritty_binding(trigger: &str, action: &str) -> Option<String> {
    let name = match action {
        "copy" => "Copy",
        "paste" => "Paste",
        "increase_font_size" => "IncreaseFontSize",
        "decrease_font_size" => "DecreaseFontSize",
        "reset_font_size" => "ResetFontSize",
        "scroll_page_up" => "ScrollPageUp",
        "scroll_page_down" => "ScrollPageDown",
        "scroll_line_up" => "ScrollLineUp",
        "scroll_line_down" => "ScrollLineDown",
        "scroll_to_top" => "ScrollToTop",
        "scroll_to_bottom" => "ScrollToBottom",
        "clear" => "ClearHistory",
        "new_window" => "CreateNewWindow",
        "new_tab" => "CreateNewTab",
        "next_tab" => "SelectNextTab",
        "previous_tab" => "SelectPreviousTab",
        "activate_last_tab" => "SelectLastTab",
        "toggle_full_screen" => "ToggleFullscreen",
        "quit" => "Quit",
        "hide_app" => "Hide",
        "minimize" => "Minimize",
        "find" => "SearchForward",
        "unbind" => "ReceiveChar",
        _ => return None,
    };
    let mut parts: Vec<&str> = trigger.split('+').collect();
    // `cmd++`: the key is the `+` itself.
    let key = if trigger.ends_with("++") {
        parts.truncate(parts.len().saturating_sub(2));
        "Plus".to_owned()
    } else {
        let key = parts.pop()?;
        match key {
            "=" => "Equals".into(),
            "-" => "Minus".into(),
            "enter" => "Return".into(),
            k if k.len() == 1 => k.to_uppercase(),
            k => {
                let mut c = k.chars();
                c.next()?.to_uppercase().chain(c).collect()
            }
        }
    };
    let mods: Vec<&str> = parts
        .iter()
        .map(|m| match *m {
            "cmd" | "super" | "command" => "Command",
            "ctrl" | "control" => "Control",
            "alt" | "opt" | "option" => "Option",
            _ => "Shift",
        })
        .collect();
    let mut out = format!(
        "\n[[keyboard.bindings]]\nkey = {}\n",
        toml::Value::String(key)
    );
    if !mods.is_empty() {
        let _ = writeln!(out, "mods = \"{}\"", mods.join("|"));
    }
    let _ = writeln!(out, "action = \"{name}\"");
    Some(out)
}

fn alacritty(config: &Config, theme: &Theme) -> Exported {
    let mut covered: Vec<&'static str> = vec!["font-family", "font-size", "scrollback-lines"];
    let q = |s: &str| toml::Value::String(s.to_owned()).to_string();
    let mut out = String::from("# Exported from Motty.\n");
    // Alacritty takes a folder, not Motty's `home` / `inherit`.
    if let Some(WorkingDir::Path(dir)) = &config.working_directory {
        covered.push("working-directory");
        let dir = dir.display().to_string();
        let _ = writeln!(out, "\n[general]\nworking_directory = {}", q(&dir));
    }
    if !config.env.is_empty() {
        covered.push("env");
        out.push_str("\n[env]\n");
        for (name, value) in &config.env {
            let bare = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            let key = if bare { name.clone() } else { q(name) };
            let _ = writeln!(out, "{key} = {}", q(value));
        }
    }
    let _ = writeln!(
        out,
        "\n[font]\nsize = {:?}\n\n[font.normal]\nfamily = {}",
        size(config),
        q(family(config))
    );
    let _ = writeln!(
        out,
        "\n[scrolling]\nhistory = {}",
        config.scrollback.unwrap_or(10_000).min(100_000)
    );
    covered.extend(["cursor-style", "cursor-style-blink"]);
    let shape = match config.cursor_style {
        CursorStyle::Block | CursorStyle::BlockHollow => "Block",
        CursorStyle::Bar => "Beam",
        CursorStyle::Underline => "Underline",
    };
    let blinking = match config.cursor_blink {
        CursorBlink::DefaultOff => "Off",
        CursorBlink::DefaultOn => "On",
        CursorBlink::AlwaysOff => "Never",
        CursorBlink::AlwaysOn => "Always",
    };
    let _ = writeln!(
        out,
        "\n[cursor.style]\nshape = \"{shape}\"\nblinking = \"{blinking}\""
    );
    let t = &theme.terminal;
    let _ = writeln!(
        out,
        "\n[colors.primary]\nforeground = \"{}\"\nbackground = \"{}\"",
        hex(t.foreground),
        hex(t.background)
    );
    let _ = write!(
        out,
        "\n[colors.cursor]\ncursor = \"{}\"\n",
        hex(theme.cursor.color)
    );
    if let Some(text) = theme.cursor.text {
        let _ = writeln!(out, "text = \"{}\"", hex(text));
    }
    let _ = writeln!(
        out,
        "\n[colors.selection]\nbackground = \"{}\"",
        hex(t.selection_background)
    );
    for (title, range) in [("normal", 0..8), ("bright", 8..16)] {
        let _ = writeln!(out, "\n[colors.{title}]");
        for (i, name) in [
            "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
        ]
        .iter()
        .enumerate()
        {
            let _ = writeln!(out, "{name} = \"{}\"", hex(t.palette[range.start + i]));
        }
    }
    covered.extend(["copy-on-select", "mouse-hide-while-typing"]);
    let b = &config.behavior;
    let _ = writeln!(
        out,
        "\n[selection]\nsave_to_clipboard = {}\n\n[mouse]\nhide_when_typing = {}",
        b.copy_on_select, b.hide_mouse_while_typing
    );
    covered.extend(["macos-option-as-alt", "title-from-osc"]);
    let _ = writeln!(
        out,
        "\n[window]\noption_as_alt = \"{}\"\ndynamic_title = {}",
        if config.option_as_meta {
            "Both"
        } else {
            "None"
        },
        config.programs_set_title()
    );
    if let Some(shell) = &config.shell {
        covered.push("command");
        let mut words = shell.split_whitespace();
        let program = words.next().unwrap_or_default();
        let args: Vec<String> = words.map(q).collect();
        let _ = writeln!(out, "\n[terminal.shell]\nprogram = {}", q(program));
        if !args.is_empty() {
            let _ = writeln!(out, "args = [{}]", args.join(", "));
        }
    }
    let mut any = false;
    for kb in &config.keybinds {
        if let Some(table) = alacritty_binding(&kb.trigger, &kb.action) {
            any = true;
            out.push_str(&table);
        }
    }
    if any || config.keybinds.is_empty() {
        covered.push("keybind");
    }
    Exported { text: out, covered }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::{self, Outcome, Row};

    fn sets(rows: Vec<Row>) -> Vec<(String, toml::Value)> {
        rows.into_iter()
            .flat_map(|r| match r.outcome {
                Outcome::Set(pairs) => pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    fn has(pairs: &[(String, toml::Value)], key: &str, value: &str) -> bool {
        pairs
            .iter()
            .any(|(k, v)| k == key && motty_theme::schema::display(v) == value)
    }

    const CONFIG: &str = "font-family = \"Fira Code\"\nfont-size = 15\ncommand = \"/bin/zsh -l\"\nenv = \"EDITOR=nvim\"\ncursor-style = \"bar\"\ncursor-style-blink = \"default_on\"\ncopy-on-select = true\nmouse-hide-while-typing = true\nkeybind = cmd+t=new_tab\nkeybind = cmd+shift+d=split_down\nkeybind = cmd+y=open_quickly\n";

    fn config() -> (Config, Theme) {
        let config = Config::parse(CONFIG);
        let theme = config.resolve_theme(Mode::Dark).theme;
        (config, theme)
    }

    #[test]
    fn ghostty_round_trip() {
        let (config, theme) = config();
        let out = export(Terminal::Ghostty, &config, &theme);
        let back = sets(import::ghostty(&out.text));
        for (key, value) in [
            ("font-family", "Fira Code"),
            ("font-size", "15.0"),
            ("command", "/bin/zsh -l"),
            ("env", "EDITOR=nvim"),
            ("cursor-style", "bar"),
            ("cursor-style-blink", "default_on"),
            ("copy-on-select", "true"),
            ("mouse-hide-while-typing", "true"),
            ("keybind", "cmd+t=new_tab"),
            ("keybind", "cmd+shift+d=split_down"),
            ("background", &hex(theme.terminal.background)),
            ("palette-4", &hex(theme.terminal.palette[4])),
        ] {
            assert!(
                has(&back, key, value),
                "{key} = {value} lost:\n{}",
                out.text
            );
        }
        // A binding Ghostty has no action for stays behind.
        assert!(!out.text.contains("open_quickly"));
    }

    #[test]
    fn kitty_round_trip() {
        let (config, theme) = config();
        let out = export(Terminal::Kitty, &config, &theme);
        let back = sets(import::kitty(&out.text));
        for (key, value) in [
            ("font-family", "Fira Code"),
            ("font-size", "15.0"),
            ("command", "/bin/zsh -l"),
            ("env", "EDITOR=nvim"),
            ("cursor-style", "bar"),
            ("cursor-style-blink", "default_on"),
            ("copy-on-select", "true"),
            ("mouse-hide-while-typing", "true"),
            ("keybind", "cmd+t=new_tab"),
            ("keybind", "cmd+shift+d=split_down"),
            ("foreground", &hex(theme.terminal.foreground)),
        ] {
            assert!(
                has(&back, key, value),
                "{key} = {value} lost:\n{}",
                out.text
            );
        }
    }

    #[test]
    fn alacritty_round_trip() {
        let (config, theme) = config();
        let out = export(Terminal::Alacritty, &config, &theme);
        let back = sets(import::alacritty(&out.text).expect("valid TOML"));
        for (key, value) in [
            ("font-family", "Fira Code"),
            ("font-size", "15.0"),
            ("command", "/bin/zsh -l"),
            ("env", "EDITOR=nvim"),
            ("cursor-style", "bar"),
            ("cursor-style-blink", "default_on"),
            ("copy-on-select", "true"),
            ("keybind", "cmd+t=new_tab"),
            ("palette-9", &hex(theme.terminal.palette[9])),
        ] {
            assert!(
                has(&back, key, value),
                "{key} = {value} lost:\n{}",
                out.text
            );
        }
    }
}
