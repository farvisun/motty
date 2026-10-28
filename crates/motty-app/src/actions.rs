use std::rc::Rc;

use gpui::{Action, App, Global, KeyBinding, KeyBindingContextPredicate, NoAction, actions};
use motty_theme::config::Keybind;

actions!(
    motty,
    [
        Copy,
        Paste,
        SelectAll,
        Clear,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        Quit,
        NewTab,
        CloseTab,
        ReopenClosedTab,
        NextTab,
        PreviousTab,
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        ActivateLastTab,
        ToggleTabsPanel,
        ToggleDetailsPanel,
        AcceptSuggestion,
        DetailsInfo,
        DetailsOutline,
        DetailsGit,
        DetailsFiles,
        SplitRight,
        SplitLeft,
        SplitDown,
        SplitUp,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        FocusNextPane,
        FocusPreviousPane,
        EqualizeSplits,
        ZoomSplit,
        CursorLineStart,
        CursorLineEnd,
        CursorWordLeft,
        CursorWordRight,
        DeleteToLineStart,
        DeleteToLineEnd,
        DeleteWordLeft,
        DeleteWordRight,
        Undo,
        ScrollToTop,
        ScrollToBottom,
        ScrollPageUp,
        ScrollPageDown,
        ScrollLineUp,
        ScrollLineDown,
        NewWindow,
        CloseWindow,
        Minimize,
        ToggleFullScreen,
        MoveDividerLeft,
        MoveDividerRight,
        MoveDividerUp,
        MoveDividerDown,
        Find,
        FindNext,
        FindPrevious,
        GlobalSearch,
        ToggleViMode,
        ToggleViKeyHints,
        ToggleReadOnly,
        HintOpen,
        HintCopy,
        HintPaste,
        HintReveal,
        CommandPalette,
        OpenQuickly,
        JumpTo,
        OpenFilesQuickly,
        CopyPath,
        RevealInFinder,
        OpenConfigFile,
        ShowWelcome,
        OpenSettings,
        About,
        HideApp,
        HideOthers,
        ShowAll,
        ZoomWindow,
        OpenHelp,
    ]
);

/// Sends text to the focused terminal as if typed: what `text:`, `csi:`
/// and `esc:` keybinds do.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = motty, no_json)]
pub struct SendText {
    pub text: String,
}

const TERMINAL: Option<&str> = Some("Terminal");
const WORKSPACE: Option<&str> = Some("Workspace");

struct DefaultKey {
    keys: &'static str,
    action: Box<dyn Action>,
    context: Option<&'static str>,
}

fn b(keys: &'static str, action: impl Action, context: Option<&'static str>) -> DefaultKey {
    DefaultKey {
        keys,
        action: Box::new(action),
        context,
    }
}

fn defaults() -> Vec<DefaultKey> {
    vec![
        b("cmd-c", Copy, TERMINAL),
        b("cmd-v", Paste, TERMINAL),
        b("cmd-a", SelectAll, TERMINAL),
        b("cmd-k", Clear, TERMINAL),
        b("cmd-f", Find, TERMINAL),
        b("ctrl-shift-space", ToggleViMode, TERMINAL),
        b("cmd-/", ToggleViKeyHints, TERMINAL),
        b("cmd-g", FindNext, TERMINAL),
        b("cmd-shift-g", FindPrevious, TERMINAL),
        b("cmd-=", IncreaseFontSize, TERMINAL),
        b("cmd-+", IncreaseFontSize, TERMINAL),
        b("cmd--", DecreaseFontSize, TERMINAL),
        b("cmd-0", ResetFontSize, TERMINAL),
        b("cmd-q", Quit, None),
        b("cmd-,", OpenSettings, None),
        b("cmd-h", HideApp, None),
        b("alt-cmd-h", HideOthers, None),
        b("cmd-t", NewTab, WORKSPACE),
        b("cmd-w", CloseTab, WORKSPACE),
        b("cmd-shift-t", ReopenClosedTab, WORKSPACE),
        b("cmd-shift-p", CommandPalette, WORKSPACE),
        b("cmd-shift-o", OpenQuickly, WORKSPACE),
        b("cmd-j", JumpTo, WORKSPACE),
        b("cmd-shift-]", NextTab, WORKSPACE),
        b("ctrl-tab", NextTab, WORKSPACE),
        b("cmd-shift-[", PreviousTab, WORKSPACE),
        b("ctrl-shift-tab", PreviousTab, WORKSPACE),
        b("cmd-1", ActivateTab1, WORKSPACE),
        b("cmd-2", ActivateTab2, WORKSPACE),
        b("cmd-3", ActivateTab3, WORKSPACE),
        b("cmd-4", ActivateTab4, WORKSPACE),
        b("cmd-5", ActivateTab5, WORKSPACE),
        b("cmd-6", ActivateTab6, WORKSPACE),
        b("cmd-7", ActivateTab7, WORKSPACE),
        b("cmd-8", ActivateTab8, WORKSPACE),
        b("cmd-9", ActivateLastTab, WORKSPACE),
        b("cmd-shift-l", ToggleTabsPanel, WORKSPACE),
        b("cmd-shift-r", ToggleDetailsPanel, WORKSPACE),
        b("cmd-d", SplitRight, WORKSPACE),
        b("cmd-alt-d", SplitLeft, WORKSPACE),
        b("cmd-shift-d", SplitDown, WORKSPACE),
        b("cmd-alt-shift-d", SplitUp, WORKSPACE),
        b("cmd-ctrl-left", FocusPaneLeft, WORKSPACE),
        b("cmd-ctrl-right", FocusPaneRight, WORKSPACE),
        b("cmd-ctrl-up", FocusPaneUp, WORKSPACE),
        b("cmd-ctrl-down", FocusPaneDown, WORKSPACE),
        b("cmd-]", FocusNextPane, WORKSPACE),
        b("cmd-[", FocusPreviousPane, WORKSPACE),
        b("cmd-ctrl-=", EqualizeSplits, WORKSPACE),
        b("cmd-shift-enter", ZoomSplit, WORKSPACE),
        b("cmd-ctrl-shift-left", MoveDividerLeft, WORKSPACE),
        b("cmd-ctrl-shift-right", MoveDividerRight, WORKSPACE),
        b("cmd-ctrl-shift-up", MoveDividerUp, WORKSPACE),
        b("cmd-ctrl-shift-down", MoveDividerDown, WORKSPACE),
        b("cmd-n", NewWindow, None),
        b("cmd-shift-w", CloseWindow, WORKSPACE),
        b("cmd-m", Minimize, WORKSPACE),
        b("cmd-ctrl-f", ToggleFullScreen, WORKSPACE),
        // Natural text editing, as Otty binds it: readline/zle keys.
        b("cmd-left", CursorLineStart, TERMINAL),
        b("cmd-right", CursorLineEnd, TERMINAL),
        b("alt-left", CursorWordLeft, TERMINAL),
        b("alt-right", CursorWordRight, TERMINAL),
        b("cmd-backspace", DeleteToLineStart, TERMINAL),
        b("cmd-delete", DeleteToLineEnd, TERMINAL),
        b("alt-backspace", DeleteWordLeft, TERMINAL),
        b("alt-delete", DeleteWordRight, TERMINAL),
        b("cmd-z", Undo, TERMINAL),
        b("cmd-home", ScrollToTop, TERMINAL),
        b("cmd-end", ScrollToBottom, TERMINAL),
        b("cmd-pageup", ScrollPageUp, TERMINAL),
        b("cmd-pagedown", ScrollPageDown, TERMINAL),
        b("cmd-alt-up", ScrollLineUp, TERMINAL),
        b("cmd-alt-down", ScrollLineDown, TERMINAL),
        b("cmd-alt-pageup", ScrollLineUp, TERMINAL),
        b("cmd-alt-pagedown", ScrollLineDown, TERMINAL),
    ]
}

/// The ⌘ chords Open Quickly handles itself: filters, rows, open.
const OPEN_QUICKLY_KEYS: [&str; 17] = [
    "cmd-0",
    "cmd-w",
    "cmd-r",
    "cmd-z",
    "cmd-s",
    "cmd-j",
    "cmd-p",
    "cmd-1",
    "cmd-2",
    "cmd-3",
    "cmd-4",
    "cmd-5",
    "cmd-6",
    "cmd-7",
    "cmd-8",
    "cmd-9",
    "cmd-enter",
];

/// The keybinds last bound, so a reload that leaves them alone is free.
#[derive(Default)]
struct Bound(Option<Vec<Keybind>>);

impl Global for Bound {}

/// Binds the default keys, minus the chords the config takes over, plus the
/// config's `keybind` lines (later lines win). Returns what it couldn't read.
pub fn bind_keys(cx: &mut App, keybinds: &[Keybind]) -> Vec<String> {
    if cx.default_global::<Bound>().0.as_deref() == Some(keybinds) {
        return Vec::new();
    }
    cx.set_global(Bound(Some(keybinds.to_vec())));
    let mut warnings = Vec::new();
    // The config's last line for a chord is the one that counts.
    let mut user: Vec<(String, Option<Box<dyn Action>>)> = Vec::new();
    for keybind in keybinds {
        let parsed =
            chord(&keybind.trigger).and_then(|keys| Ok((keys, user_action(&keybind.action, cx)?)));
        match parsed {
            Ok((keys, action)) => {
                user.retain(|(k, _)| *k != keys);
                user.push((keys, action));
            }
            Err(e) => warnings.push(format!(
                "keybind {}={}: {e}",
                keybind.trigger, keybind.action
            )),
        }
    }
    let defaults = defaults();
    let mut bindings = Vec::new();
    let mut load = |keys: &str, action: Box<dyn Action>, context: Option<&str>| {
        let predicate = context
            .and_then(|c| KeyBindingContextPredicate::parse(c).ok())
            .map(Rc::new);
        match KeyBinding::load(keys, action, predicate, false, None, cx) {
            Ok(binding) => bindings.push(binding),
            Err(e) => warnings.push(format!("keybind {keys}: {e}")),
        }
    };
    for d in &defaults {
        let keys = normalize(d.keys);
        if !user.iter().any(|(k, _)| *k == keys) {
            load(d.keys, d.action.boxed_clone(), d.context);
        }
    }
    // Bindings run before key handlers, so these keep the window's ⌘W, ⌘J
    // and ⌘1–9 (default or the config's) out of Open Quickly's way.
    for keys in OPEN_QUICKLY_KEYS {
        load(keys, Box::new(NoAction), Some("OpenQuickly"));
    }
    for (keys, action) in user {
        let Some(action) = action else { continue };
        // Where the action's default key works, else anywhere in a window.
        let context = if action.name() == SendText::name_for_type() {
            TERMINAL
        } else {
            defaults
                .iter()
                .find(|d| d.action.partial_eq(&*action))
                .map_or(WORKSPACE, |d| d.context)
        };
        load(&keys, action, context);
    }
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
    warnings
}

/// A config trigger (`cmd+shift+t`, `opt+left`, `cmd++`) as a GPUI
/// keystroke in canonical modifier order.
/// A keystroke as `keybind` lines spell it: `ctrl+alt+shift+cmd+t`.
pub fn config_chord(ks: &gpui::Keystroke) -> String {
    let m = &ks.modifiers;
    let mut parts: Vec<&str> = [
        (m.control, "ctrl"),
        (m.alt, "alt"),
        (m.shift, "shift"),
        (m.platform, "cmd"),
        (m.function, "fn"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| name)
    .collect();
    parts.push(&ks.key);
    parts.join("+")
}

/// The chords that run `action` when the config says nothing about it.
pub fn default_chords(action: &dyn Action) -> Vec<String> {
    defaults()
        .into_iter()
        .filter(|d| d.action.partial_eq(action))
        .filter_map(|d| gpui::Keystroke::parse(d.keys).ok())
        .map(|ks| config_chord(&ks))
        .collect()
}

/// The name a `keybind` line uses for `action`: `motty::NewTab` → `new_tab`.
pub fn config_name(action: &dyn Action) -> String {
    let name = action.name();
    let name = name.rsplit("::").next().unwrap_or(name);
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn chord(trigger: &str) -> anyhow::Result<String> {
    let trigger = trigger.trim().to_lowercase();
    let (mods, key) = split_chord(&trigger, '+');
    let mut out = Vec::new();
    for m in mods {
        out.push(match m {
            "cmd" | "command" | "super" | "meta" => "cmd",
            "ctrl" | "control" => "ctrl",
            "alt" | "opt" | "option" => "alt",
            "shift" => "shift",
            "fn" => "fn",
            _ => anyhow::bail!("unknown modifier {m:?}"),
        });
    }
    let key = match key {
        "return" => "enter",
        "esc" => "escape",
        "page_up" | "page-up" => "pageup",
        "page_down" | "page-down" => "pagedown",
        "arrow_up" | "arrowup" => "up",
        "arrow_down" | "arrowdown" => "down",
        "arrow_left" | "arrowleft" => "left",
        "arrow_right" | "arrowright" => "right",
        "forward_delete" | "del" => "delete",
        "minus" => "-",
        "equal" | "equals" => "=",
        "plus" => "+",
        "comma" => ",",
        "period" => ".",
        "slash" => "/",
        "backslash" => "\\",
        "semicolon" => ";",
        "quote" | "apostrophe" => "'",
        "grave" | "backquote" | "backtick" => "`",
        "bracket_left" | "left_bracket" => "[",
        "bracket_right" | "right_bracket" => "]",
        k if k.chars().count() == 1 => k,
        k @ ("enter" | "escape" | "tab" | "space" | "backspace" | "delete" | "insert" | "home"
        | "end" | "pageup" | "pagedown" | "up" | "down" | "left" | "right") => k,
        k if k
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=35).contains(&n)) =>
        {
            k
        }
        _ => anyhow::bail!("unknown key {key:?}"),
    };
    Ok(canonical(out, key))
}

/// A GPUI keystroke such as `cmd-shift-[` in canonical modifier order, so
/// it compares equal to the same chord from the config.
fn normalize(keys: &str) -> String {
    let (mods, key) = split_chord(keys, '-');
    canonical(mods, key)
}

/// Modifiers and key; the key may itself be the separator (`cmd--`, `cmd++`).
fn split_chord(s: &str, sep: char) -> (Vec<&str>, &str) {
    let (mods, key) = match s.strip_suffix(sep) {
        Some(rest) if rest.is_empty() || rest.ends_with(sep) => {
            (rest.strip_suffix(sep).unwrap_or(rest), &s[s.len() - 1..])
        }
        _ => s.rsplit_once(sep).unwrap_or(("", s)),
    };
    let mods = mods.split(sep).filter(|m| !m.is_empty()).collect();
    (mods, key)
}

fn canonical<'a>(mut mods: Vec<&'a str>, key: &'a str) -> String {
    const ORDER: [&str; 5] = ["ctrl", "alt", "shift", "cmd", "fn"];
    mods.sort_by_key(|m| ORDER.iter().position(|o| o == m));
    mods.dedup();
    mods.push(key);
    mods.join("-")
}

/// What a keybind's action does: `None` unbinds the chord.
fn user_action(action: &str, cx: &App) -> anyhow::Result<Option<Box<dyn Action>>> {
    if let Some(text) = action.strip_prefix("text:") {
        return Ok(Some(Box::new(SendText {
            text: unescape(text),
        })));
    }
    if let Some(seq) = action.strip_prefix("csi:") {
        return Ok(Some(Box::new(SendText {
            text: format!("\x1b[{}", unescape(seq)),
        })));
    }
    if let Some(seq) = action.strip_prefix("esc:") {
        return Ok(Some(Box::new(SendText {
            text: format!("\x1b{}", unescape(seq)),
        })));
    }
    let name = action.trim().replace('-', "_");
    if name == "unbind" {
        return Ok(None);
    }
    if let Some(n) = name.strip_prefix("goto_tab:") {
        let action: Box<dyn Action> = match n.parse::<u8>() {
            Ok(1) => Box::new(ActivateTab1),
            Ok(2) => Box::new(ActivateTab2),
            Ok(3) => Box::new(ActivateTab3),
            Ok(4) => Box::new(ActivateTab4),
            Ok(5) => Box::new(ActivateTab5),
            Ok(6) => Box::new(ActivateTab6),
            Ok(7) => Box::new(ActivateTab7),
            Ok(8) => Box::new(ActivateTab8),
            Ok(9) => Box::new(ActivateLastTab),
            _ => anyhow::bail!("tabs go from 1 to 9"),
        };
        return Ok(Some(action));
    }
    // Names other terminals use for the same thing.
    let name = match name.as_str() {
        "new_split:right" => "split_right",
        "new_split:left" => "split_left",
        "new_split:down" => "split_down",
        "new_split:up" => "split_up",
        "goto_split:left" => "focus_pane_left",
        "goto_split:right" => "focus_pane_right",
        "goto_split:up" | "goto_split:top" => "focus_pane_up",
        "goto_split:down" | "goto_split:bottom" => "focus_pane_down",
        "goto_split:next" => "focus_next_pane",
        "goto_split:previous" => "focus_previous_pane",
        "toggle_split_zoom" => "zoom_split",
        "last_tab" => "activate_last_tab",
        "toggle_fullscreen" => "toggle_full_screen",
        "clear_screen" => "clear",
        "copy_to_clipboard" => "copy",
        "paste_from_clipboard" => "paste",
        "toggle_command_palette" => "command_palette",
        "open_config" => "open_config_file",
        name => name,
    };
    let pascal: String = name
        .split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect();
    cx.build_action(&format!("motty::{pascal}"), None)
        .map(Some)
        .map_err(|_| anyhow::anyhow!("unknown action"))
}

/// `\n`, `\r`, `\t`, `\e`, `\\` and `\xHH` escapes in a text payload.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('e') => out.push('\x1b'),
            Some('x') => {
                let hex: String = chars.clone().take(2).collect();
                match u8::from_str_radix(&hex, 16) {
                    Ok(b) if hex.len() == 2 => {
                        out.push(b as char);
                        chars.nth(1);
                    }
                    _ => out.push_str("\\x"),
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_chords_match_gpui_keystrokes() {
        assert_eq!(chord("cmd+shift+t").unwrap(), normalize("cmd-shift-t"));
        assert_eq!(chord("Shift+Opt+D+cmd").ok(), None);
        assert_eq!(
            chord("opt+shift+cmd+d").unwrap(),
            normalize("cmd-alt-shift-d")
        );
        assert_eq!(chord("cmd+=").unwrap(), normalize("cmd-="));
        assert_eq!(chord("cmd++").unwrap(), "cmd-+");
        assert_eq!(chord("cmd+minus").unwrap(), normalize("cmd--"));
        assert_eq!(chord("ctrl+shift+space").unwrap(), "ctrl-shift-space");
        assert_eq!(chord("cmd+page_up").unwrap(), "cmd-pageup");
        assert_eq!(chord("f6").unwrap(), "f6");
        assert!(chord("hyper+x").is_err());
        assert!(chord("cmd+nope").is_err());
    }

    #[test]
    fn open_quickly_keys_shadow_window_bindings() {
        use gpui::{KeyContext, Keymap, Keystroke};
        let mut bindings = vec![KeyBinding::new("cmd-w", CloseTab, WORKSPACE)];
        bindings.extend(
            OPEN_QUICKLY_KEYS
                .iter()
                .map(|keys| KeyBinding::new(keys, NoAction, Some("OpenQuickly"))),
        );
        let keymap = Keymap::new(bindings);
        let stack = |names: &[&str]| -> Vec<KeyContext> {
            names
                .iter()
                .map(|n| KeyContext::parse(n).unwrap())
                .collect()
        };
        let cmd_w = [Keystroke::parse("cmd-w").unwrap()];
        let (in_picker, _) =
            keymap.bindings_for_input(&cmd_w, &stack(&["Workspace", "OpenQuickly"]));
        assert!(in_picker.is_empty());
        let (in_pane, _) = keymap.bindings_for_input(&cmd_w, &stack(&["Workspace", "Terminal"]));
        assert_eq!(in_pane.len(), 1);
    }

    #[test]
    fn text_payload_escapes() {
        assert_eq!(unescape(r"hi\n"), "hi\n");
        assert_eq!(unescape(r"\x1b[A\e"), "\x1b[A\x1b");
        assert_eq!(unescape(r"a\xZZ\\"), "a\\xZZ\\");
    }

    #[test]
    fn names_and_default_chords_for_config_lines() {
        assert_eq!(config_name(&NewTab), "new_tab");
        assert_eq!(config_name(&ActivateTab1), "activate_tab1");
        assert_eq!(default_chords(&NewTab), vec!["cmd+t".to_string()]);
        let ks = gpui::Keystroke::parse("cmd-shift-t").unwrap();
        assert_eq!(config_chord(&ks), "shift+cmd+t");
        // What we write parses back to the same binding.
        assert_eq!(chord("shift+cmd+t").unwrap(), normalize("cmd-shift-t"));
    }
}
