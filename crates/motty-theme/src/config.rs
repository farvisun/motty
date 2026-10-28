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
    /// `sidebar-visible`: whether windows open with the tabs sidebar shown.
    pub sidebar_visible: bool,
    /// How opaque unfocused split panes are drawn (Otty's default is 0.6).
    pub unfocused_split_opacity: Option<f32>,
    pub details_panel_width: Option<f32>,
    pub option_as_meta: bool,
    pub scrollback: Option<usize>,
    /// Inject Motty's prompt marks and cwd reporting into the shell.
    pub shell_integration: bool,
    /// Shell names (`zsh`, `fish`) to leave alone.
    pub shell_integration_disabled_shells: Vec<String>,
    pub shell: Option<String>,
    /// `cursor-style`: the shape programs get until they ask for another.
    pub cursor_style: CursorStyle,
    pub cursor_blink: CursorBlink,
    /// `cursor-color` and `cursor-text`, for themes that don't set them.
    pub cursor_color: Option<Rgba>,
    pub cursor_text: Option<Rgba>,
    /// `confirm-close-pane`, `confirm-close-tab`, `confirm-close-window`.
    pub confirm_close_pane: ConfirmClose,
    pub confirm_close_tab: ConfirmClose,
    pub confirm_close_window: ConfirmClose,
    /// `window-hide-dirty-indicator`: never show the close button's dot.
    pub hide_dirty_indicator: bool,
    /// `on-launch = restore_session` (default) reopens the last session;
    /// `new_window` starts fresh (a crash still restores).
    pub restore_session: bool,
    /// `link-detection`: find URLs and paths under the pointer (OSC 8
    /// links work either way).
    pub link_detection: bool,
    pub link_cmd_click: LinkClick,
    /// `right-click-action`: what a right-click in a terminal does.
    pub right_click: RightClick,
    pub link_cmd_shift_click: LinkShiftClick,
    /// `open-with-app = "Name|bundle.id"` entries, in file order.
    pub open_with_apps: Vec<String>,
    /// The `autocomplete-*` keys.
    pub autocomplete: Autocomplete,
    /// `keybind = <trigger>=<action>` entries, in file order.
    pub keybinds: Vec<Keybind>,
    /// The `privilege-*` keys for tab badges and agent notifications.
    pub badges: Badges,
    /// Where the config was read from, if anywhere.
    pub path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

/// What marks a tab and what posts a notification (Otty's `privilege-*`
/// keys; all on except keeping the Mac awake).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Badges {
    /// `privilege-badge-exit-status`: a dot or triangle after a command.
    pub exit_status: bool,
    /// `privilege-badge-agent-processing`
    pub agent_processing: bool,
    /// `privilege-badge-agent-task-complete`
    pub agent_complete: bool,
    /// `privilege-badge-agent-awaiting-input`
    pub agent_awaiting: bool,
    /// `privilege-notify-agent-task-complete`
    pub notify_complete: bool,
    /// `privilege-notify-agent-awaiting-input`
    pub notify_awaiting: bool,
    /// `privilege-caffeinate-agent-processing`
    pub caffeinate: bool,
}

impl Default for Badges {
    fn default() -> Self {
        Self {
            exit_status: true,
            agent_processing: true,
            agent_complete: true,
            agent_awaiting: true,
            notify_complete: true,
            notify_awaiting: true,
            caffeinate: false,
        }
    }
}

/// Inline suggestions from the commands run before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Autocomplete {
    /// `autocomplete-shortcut`: the key that accepts a suggestion.
    pub shortcut: AcceptKey,
    /// `autocomplete-inline-suggestion`: draw the ghost text at all.
    pub inline: bool,
    /// `autocomplete-on-device-learning`: remember the commands run.
    pub learn: bool,
    /// `autocomplete-skip-secrets`: never remember a command that looks
    /// like it carries a password, token or key.
    pub skip_secrets: bool,
    /// `autocomplete-history-ignore`: globs for commands never remembered.
    pub history_ignore: Vec<String>,
}

impl Default for Autocomplete {
    fn default() -> Self {
        Self {
            shortcut: AcceptKey::Tab,
            inline: true,
            learn: true,
            skip_secrets: false,
            history_ignore: Vec::new(),
        }
    }
}

/// `autocomplete-shortcut` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AcceptKey {
    #[default]
    Tab,
    /// Tab or →.
    TabRightArrow,
    /// → only; Tab stays with the shell.
    RightArrow,
    CtrlSpace,
    Disable,
}

/// One `keybind` line: a chord such as `cmd+shift+t` and what it does
/// (an action name, `unbind`, or a `text:`/`csi:`/`esc:` payload).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keybind {
    pub trigger: String,
    pub action: String,
}

impl Keybind {
    /// Splits `trigger=action` at the first `=` that isn't the trigger's key
    /// (`cmd+==increase_font_size` binds ⌘=).
    pub fn parse(value: &str) -> Option<Self> {
        let at = value
            .char_indices()
            .find(|&(i, c)| c == '=' && i > 0 && !value[..i].ends_with('+'))?
            .0;
        let (trigger, action) = (value[..at].trim(), value[at + 1..].trim());
        (!trigger.is_empty() && !action.is_empty()).then(|| Self {
            trigger: trigger.to_owned(),
            action: action.to_owned(),
        })
    }
}

/// `cursor-style` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorStyle {
    #[default]
    Block,
    Bar,
    Underline,
    BlockHollow,
}

/// `confirm-close-*` values: when closing a pane, tab or window asks first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConfirmClose {
    /// Only while something other than the shell is running.
    #[default]
    Process,
    Always,
    /// Windows only: when the window holds more than one tab.
    MultipleTabs,
    Never,
}

/// `right-click-action` values. ⌃-click always opens the menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RightClick {
    #[default]
    ContextMenu,
    Copy,
    Paste,
    /// Copies the selection if there is one, else pastes.
    CopyOrPaste,
    Ignore,
}

/// `link-cmd-click`: what ⌘-click on a link does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkClick {
    #[default]
    Open,
    Copy,
    Nothing,
}

/// `link-cmd-shift-click`: what ⌘⇧-click on a path does (URLs are copied).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkShiftClick {
    #[default]
    RevealFinder,
    OpenSystemDefault,
}

/// `cursor-style-blink`: whether the cursor blinks before a program asks
/// (`default_*`), or whatever programs ask (`always_*`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorBlink {
    #[default]
    DefaultOff,
    DefaultOn,
    AlwaysOff,
    AlwaysOn,
}

impl CursorBlink {
    /// Blinking before any program asks.
    pub fn initial(self) -> bool {
        matches!(self, Self::DefaultOn | Self::AlwaysOn)
    }

    /// Blinking, given what the running program asked for.
    pub fn resolve(self, requested: bool) -> bool {
        match self {
            Self::AlwaysOn => true,
            Self::AlwaysOff => false,
            Self::DefaultOn | Self::DefaultOff => requested,
        }
    }
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
                let local =
                    std::fs::read_to_string(dir.join("config.local.toml")).unwrap_or_default();
                let mut cfg = Self::parse(&format!("{src}\n{local}"));
                cfg.path = Some(path);
                return cfg;
            }
        }
        Self::parse("")
    }

    /// Defaults follow Otty's: ligatures on, Option is not Alt, no blinking.
    pub fn parse(src: &str) -> Self {
        let mut out = Config {
            ligatures: true,
            sidebar_visible: true,
            shell_integration: true,
            link_detection: true,
            restore_session: true,
            ..Default::default()
        };
        let (entries, mut warnings) = parse_entries(src);
        let mut warn = |k: &str, m: String| warnings.push(format!("config.toml {k}: {m}"));
        let str_of = |v: &toml::Value| v.as_str().map(str::to_owned);
        let num_of = |v: &toml::Value| {
            v.as_float()
                .map(|f| f as f32)
                .or_else(|| v.as_integer().map(|i| i as f32))
        };
        let mut color_of = |k: &str, v: &toml::Value| match v.as_str().map(css::parse_color) {
            Some(Ok(c)) => Some(c),
            Some(Err(e)) => {
                warn(k, e.0);
                None
            }
            None => None,
        };
        // Unknown enum values (`color_of` holds `warn` for the whole loop).
        let mut unknown = Vec::new();
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
                    out.ligatures = value
                        .as_bool()
                        .unwrap_or_else(|| value.as_str() != Some("off"))
                }
                "sidebar-width" => out.sidebar_width = num_of(value),
                "sidebar-visible" => out.sidebar_visible = value.as_bool().unwrap_or(true),
                "unfocused-split-opacity" => {
                    out.unfocused_split_opacity = num_of(value).map(|o| o.clamp(0., 1.))
                }
                "details-panel-width" => out.details_panel_width = num_of(value),
                // `true`, `false`, `left` or `right`; one side counts as on
                // until left/right modifiers are told apart.
                "macos-option-as-alt" | "option-as-meta" => {
                    out.option_as_meta = value
                        .as_bool()
                        .unwrap_or_else(|| value.as_str().is_some_and(|v| v != "false"))
                }
                "scrollback" | "scrollback-lines" => {
                    out.scrollback = value.as_integer().map(|i| i.max(0) as usize)
                }
                "shell" => out.shell = str_of(value),
                "shell-integration" => out.shell_integration = value.as_bool().unwrap_or(true),
                "shell-integration-disabled-shells" => {
                    out.shell_integration_disabled_shells = value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect();
                }
                "cursor-style" => match value.as_str() {
                    Some("block") => out.cursor_style = CursorStyle::Block,
                    Some("bar") => out.cursor_style = CursorStyle::Bar,
                    Some("underline") => out.cursor_style = CursorStyle::Underline,
                    Some("block_hollow") => out.cursor_style = CursorStyle::BlockHollow,
                    _ => unknown.push(format!("config.toml {key}: unknown cursor style {value}")),
                },
                "cursor-style-blink" | "cursor-blink" => match (value.as_bool(), value.as_str()) {
                    (Some(true), _) | (_, Some("default_on")) => {
                        out.cursor_blink = CursorBlink::DefaultOn
                    }
                    (Some(false), _) | (_, Some("default_off")) => {
                        out.cursor_blink = CursorBlink::DefaultOff
                    }
                    (_, Some("always_on")) => out.cursor_blink = CursorBlink::AlwaysOn,
                    (_, Some("always_off")) => out.cursor_blink = CursorBlink::AlwaysOff,
                    _ => unknown.push(format!("config.toml {key}: unknown blink policy {value}")),
                },
                "cursor-color" => out.cursor_color = color_of(key, value),
                "cursor-text" => out.cursor_text = color_of(key, value),
                "privilege-badge-exit-status"
                | "privilege-badge-agent-processing"
                | "privilege-badge-agent-task-complete"
                | "privilege-badge-agent-awaiting-input"
                | "privilege-notify-agent-task-complete"
                | "privilege-notify-agent-awaiting-input"
                | "privilege-caffeinate-agent-processing" => match value.as_bool() {
                    Some(on) => {
                        let b = &mut out.badges;
                        *match key {
                            "privilege-badge-exit-status" => &mut b.exit_status,
                            "privilege-badge-agent-processing" => &mut b.agent_processing,
                            "privilege-badge-agent-task-complete" => &mut b.agent_complete,
                            "privilege-badge-agent-awaiting-input" => &mut b.agent_awaiting,
                            "privilege-notify-agent-task-complete" => &mut b.notify_complete,
                            "privilege-notify-agent-awaiting-input" => &mut b.notify_awaiting,
                            _ => &mut b.caffeinate,
                        } = on;
                    }
                    None => unknown.push(format!("config.toml {key}: expected true or false")),
                },
                "window-hide-dirty-indicator" => {
                    out.hide_dirty_indicator = value.as_bool().unwrap_or(false)
                }
                "on-launch" => match value.as_str() {
                    Some("restore_session") => out.restore_session = true,
                    Some("new_window") => out.restore_session = false,
                    _ => unknown.push(format!("config.toml {key}: unknown value {value}")),
                },
                "link-detection" => {
                    out.link_detection = value.as_bool().unwrap_or(value.as_str() != Some("off"))
                }
                "confirm-close-pane" | "confirm-close-tab" | "confirm-close-window" => {
                    let rule = match (value.as_str(), value.as_bool()) {
                        (Some("process"), _) | (_, Some(true)) => ConfirmClose::Process,
                        (Some("always"), _) => ConfirmClose::Always,
                        (Some("never"), _) | (_, Some(false)) => ConfirmClose::Never,
                        (Some("multiple_tabs"), _) if key == "confirm-close-window" => {
                            ConfirmClose::MultipleTabs
                        }
                        _ => {
                            unknown.push(format!("config.toml {key}: unknown value {value}"));
                            continue;
                        }
                    };
                    match key {
                        "confirm-close-pane" => out.confirm_close_pane = rule,
                        "confirm-close-tab" => out.confirm_close_tab = rule,
                        _ => out.confirm_close_window = rule,
                    }
                }
                "right-click-action" => match value.as_str() {
                    Some("context-menu") => out.right_click = RightClick::ContextMenu,
                    Some("copy") => out.right_click = RightClick::Copy,
                    Some("paste") => out.right_click = RightClick::Paste,
                    Some("copy-or-paste") => out.right_click = RightClick::CopyOrPaste,
                    Some("ignore") => out.right_click = RightClick::Ignore,
                    _ => unknown.push(format!("config.toml {key}: unknown value {value}")),
                },
                "link-cmd-click" => match value.as_str() {
                    Some("open") => out.link_cmd_click = LinkClick::Open,
                    Some("copy") => out.link_cmd_click = LinkClick::Copy,
                    Some("nothing") => out.link_cmd_click = LinkClick::Nothing,
                    _ => unknown.push(format!("config.toml {key}: unknown action {value}")),
                },
                "link-cmd-shift-click" => match value.as_str() {
                    Some("reveal-finder") => {
                        out.link_cmd_shift_click = LinkShiftClick::RevealFinder
                    }
                    Some("open-system-default") => {
                        out.link_cmd_shift_click = LinkShiftClick::OpenSystemDefault
                    }
                    _ => unknown.push(format!("config.toml {key}: unknown action {value}")),
                },
                "open-with-app" => out.open_with_apps.extend(str_of(value)),
                "autocomplete-shortcut" => match value.as_str() {
                    Some("tab") => out.autocomplete.shortcut = AcceptKey::Tab,
                    Some("tab+right-arrow") => out.autocomplete.shortcut = AcceptKey::TabRightArrow,
                    Some("right-arrow") => out.autocomplete.shortcut = AcceptKey::RightArrow,
                    Some("ctrl+space") => out.autocomplete.shortcut = AcceptKey::CtrlSpace,
                    Some("disable") => out.autocomplete.shortcut = AcceptKey::Disable,
                    _ => unknown.push(format!("config.toml {key}: unknown key {value}")),
                },
                "autocomplete-inline-suggestion" => {
                    out.autocomplete.inline = value.as_bool().unwrap_or(true)
                }
                "autocomplete-on-device-learning" => {
                    out.autocomplete.learn = value.as_bool().unwrap_or(true)
                }
                "autocomplete-skip-secrets" => {
                    out.autocomplete.skip_secrets = value.as_bool().unwrap_or(false)
                }
                // A list, a comma-separated string, or the key repeated.
                "autocomplete-history-ignore" => {
                    let globs: Vec<String> = match value.as_array() {
                        Some(list) => list
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect(),
                        None => value
                            .as_str()
                            .unwrap_or_default()
                            .split(',')
                            .map(|g| g.trim().to_owned())
                            .collect(),
                    };
                    out.autocomplete
                        .history_ignore
                        .extend(globs.into_iter().filter(|g| !g.is_empty()));
                }
                "keybind" => match value.as_str().and_then(Keybind::parse) {
                    Some(keybind) => out.keybinds.push(keybind),
                    None => unknown.push(format!("config.toml keybind: can't read {value}")),
                },
                k if k.starts_with("palette-") => {
                    if let Ok(i @ 0..16) = k["palette-".len()..].parse::<usize>() {
                        out.palette[i] = color_of(key, value);
                    }
                }
                _ => {}
            }
        }
        warnings.extend(unknown);
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

    /// Loads the active theme. Like Otty, a theme's own terminal colors win:
    /// the config's `foreground`, `background` and `palette-N` only color the
    /// built-in theme used when no theme is configured.
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
        if name.is_none() {
            self.apply_overrides(&mut parsed.theme);
        }
        self.apply_cursor(&mut parsed.theme);
        parsed
    }

    /// Otty uses `cursor-color` and `cursor-text` only where the theme has no
    /// cursor color of its own (Paper turns red, Nord keeps its own).
    fn apply_cursor(&self, theme: &mut Theme) {
        let c = &mut theme.cursor;
        if let Some(color) = self.cursor_color.filter(|_| !c.color_themed) {
            for shape in [&mut c.block, &mut c.bar, &mut c.underline] {
                if *shape == c.color {
                    *shape = color;
                }
            }
            c.color = color;
        }
        if let Some(text) = self.cursor_text.filter(|_| !c.text_themed) {
            c.block_text = c.block_text.or(Some(text));
            c.text = Some(text);
        }
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
        // `keybind = cmd+t=new_tab` isn't TOML: the value is the raw rest of
        // the line (quotes optional, so a `#` in it is no comment).
        if let Some(rest) = trimmed.strip_prefix("keybind")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            let value = value.trim();
            let value = match value.strip_prefix('"').and(value.strip_suffix('"')) {
                Some(_) => toml::from_str::<toml::Table>(&format!("v = {value}"))
                    .ok()
                    .and_then(|t| t.get("v")?.as_str().map(str::to_owned))
                    .unwrap_or_else(|| value.to_owned()),
                None => value.to_owned(),
            };
            entries.push(("keybind".to_owned(), toml::Value::String(value)));
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

/// `src` with `key = value` set: the last line for `key` is replaced (so
/// comments and everything else stay as they were), or the line appended.
pub fn set_in(src: &str, key: &str, value: &toml::Value) -> String {
    let line = format!("{key} = {value}");
    let is_key = |l: &str| {
        l.trim_start()
            .strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    };
    let mut lines: Vec<String> = src.lines().map(str::to_owned).collect();
    match lines.iter().rposition(|l| is_key(l)) {
        Some(i) => lines[i] = line,
        None => {
            while lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.pop();
            }
            lines.push(line);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// `src` without any `key = …` line (back to the default).
pub fn remove_in(src: &str, key: &str) -> String {
    let mut out: String = src
        .lines()
        .filter(|l| {
            !l.trim_start()
                .strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
        .flat_map(|l| [l, "\n"])
        .collect();
    if out.is_empty() {
        out.push('\n');
    }
    out
}

/// `src` with `action`'s `keybind` lines replaced so that exactly `chords`
/// run it: chords not among `defaults` are bound, and defaults not among
/// `chords` are unbound. Binding the defaults again leaves no lines at all.
/// Chords are compared as written, so callers pass one spelling.
pub fn set_keybinds_in(src: &str, action: &str, defaults: &[String], chords: &[String]) -> String {
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    let mut lines: Vec<String> = src
        .lines()
        .filter(|l| {
            let Some(value) = keybind_value(l) else {
                return true;
            };
            let Some(kb) = Keybind::parse(&value) else {
                return true;
            };
            let own = same(&kb.action, action);
            let unbinds_default =
                same(&kb.action, "unbind") && defaults.iter().any(|d| same(d, &kb.trigger));
            !(own || unbinds_default)
        })
        .map(str::to_owned)
        .collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    for d in defaults {
        if !chords.iter().any(|c| same(c, d)) {
            lines.push(format!("keybind = {d}=unbind"));
        }
    }
    for c in chords {
        if !defaults.iter().any(|d| same(c, d)) {
            lines.push(format!("keybind = {c}={action}"));
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The value of a `keybind = …` line, unquoted.
fn keybind_value(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("keybind")?.trim_start();
    let value = rest.strip_prefix('=')?.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    Some(value.to_owned())
}

/// Applies [`set_keybinds_in`] to Motty's config.toml.
pub fn save_keybinds(
    action: &str,
    defaults: &[String],
    chords: &[String],
) -> std::io::Result<PathBuf> {
    edit(|src| set_keybinds_in(src, action, defaults, chords))
}

/// `src` with every `keybind` line removed: all shortcuts back to default.
pub fn reset_keybinds_in(src: &str) -> String {
    remove_in(src, "keybind")
}

/// Rewrites Motty's config.toml through `f`, starting from Otty's (or a
/// header) when Motty has none yet.
pub fn edit(f: impl FnOnce(&str) -> String) -> std::io::Result<PathBuf> {
    let path = motty_dir().join("config.toml");
    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(_) => match std::fs::read_to_string(otty_dir().join("config.toml")) {
            Ok(otty) => format!("# Started from Otty's config.toml.\n{otty}"),
            Err(_) => "# Motty settings: https://docs.otty.sh/reference/configuration\n".into(),
        },
    };
    let out = f(&src);
    std::fs::create_dir_all(motty_dir())?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Writes `pairs` into Motty's own config.toml. The first time, an Otty
/// config being read is copied over first, so nothing it set is lost.
pub fn save_values(pairs: &[(&str, toml::Value)]) -> std::io::Result<PathBuf> {
    let changes: Vec<(&str, Option<toml::Value>)> =
        pairs.iter().map(|(k, v)| (*k, Some(v.clone()))).collect();
    save(&changes)
}

/// Sets each key to its value, or removes it for `None`, in Motty's own
/// config.toml (see [`save_values`]).
pub fn save(changes: &[(&str, Option<toml::Value>)]) -> std::io::Result<PathBuf> {
    edit(|src| {
        let mut src = src.to_owned();
        for (key, value) in changes {
            src = match value {
                Some(value) => set_in(&src, key, value),
                None => remove_in(&src, key),
            };
        }
        src
    })
}

const MOTTY_DARK: &str = include_str!("../themes/motty-dark.ottytheme");
const MOTTY_LIGHT: &str = include_str!("../themes/motty-light.ottytheme");

pub fn builtin(mode: Mode) -> ParsedTheme {
    theme::parse(if mode == Mode::Dark {
        MOTTY_DARK
    } else {
        MOTTY_LIGHT
    })
    .expect("built-in theme parses")
}

pub fn theme_dirs() -> Vec<PathBuf> {
    vec![motty_dir().join("themes"), otty_dir().join("themes")]
}

/// Changes whenever a config file or theme file is added, removed or saved,
/// for polling-based hot reload (a few dozen `stat` calls).
pub fn files_fingerprint() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut stamp = |path: &Path| {
        path.hash(&mut hasher);
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .hash(&mut hasher);
    };
    for dir in [motty_dir(), otty_dir()] {
        stamp(&dir.join("config.toml"));
        stamp(&dir.join("config.local.toml"));
    }
    for dir in theme_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_theme_file(p))
            .collect();
        paths.sort();
        for path in paths {
            stamp(&path);
        }
    }
    hasher.finish()
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
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_theme_file(p))
            .collect();
        paths.sort();
        // File stems are cheap to check first; fall back to parsing names.
        if let Some(p) = paths.iter().find(|p| {
            p.file_stem()
                .is_some_and(|s| normalize(&s.to_string_lossy()) == wanted)
        }) && let Some(t) = load_file(p)
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
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for path in entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_theme_file(p))
        {
            if let Some(t) = load_file(&path)
                && !out
                    .iter()
                    .any(|(n, ..)| normalize(n) == normalize(&t.theme.meta.name))
            {
                out.push((t.theme.meta.name, t.theme.meta.mode, Some(path)));
            }
        }
    }
    out.sort_by_key(|(n, ..)| n.to_lowercase());
    out
}

fn is_theme_file(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()),
        Some("ottytheme" | "toml")
    )
}

fn load_file(p: &Path) -> Option<ParsedTheme> {
    theme::parse(&std::fs::read_to_string(p).ok()?).ok()
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
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
        assert!(d.ligatures && !d.option_as_meta && d.cursor_blink == CursorBlink::DefaultOff);
        assert_eq!(d.cursor_style, CursorStyle::Block);
        assert!(d.sidebar_visible && !Config::parse("sidebar-visible = false\n").sidebar_visible);
        assert_eq!(d.confirm_close_window, ConfirmClose::Process);
        let cfg = Config::parse(
            "confirm-close-pane = \"never\"\nconfirm-close-tab = \"always\"\nconfirm-close-window = \"multiple_tabs\"\n",
        );
        assert_eq!(cfg.confirm_close_pane, ConfirmClose::Never);
        assert_eq!(cfg.confirm_close_tab, ConfirmClose::Always);
        assert_eq!(cfg.confirm_close_window, ConfirmClose::MultipleTabs);
        let cfg = Config::parse("confirm-close-tab = \"multiple_tabs\"\n");
        assert_eq!(cfg.confirm_close_tab, ConfirmClose::Process);
        assert_eq!(cfg.warnings.len(), 1);
        let cfg = Config::parse(
            "font-ligatures = \"off\"\nmacos-option-as-alt = \"left\"\ncursor-style-blink = \"default_on\"\nwindow-hide-dirty-indicator = true\n",
        );
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        assert!(
            !cfg.ligatures
                && cfg.option_as_meta
                && cfg.cursor_blink == CursorBlink::DefaultOn
                && cfg.hide_dirty_indicator
        );
        let cursor = Config::parse(
            "cursor-style = \"block_hollow\"\ncursor-style-blink = \"always_off\"\ncursor-color = \"#ff0000\"\n",
        );
        assert!(cursor.warnings.is_empty(), "{:?}", cursor.warnings);
        assert_eq!(
            (cursor.cursor_style, cursor.cursor_blink),
            (CursorStyle::BlockHollow, CursorBlink::AlwaysOff)
        );
        assert_eq!(cursor.cursor_color.map(|c| c.r), Some(1.));
        assert!(!CursorBlink::AlwaysOff.resolve(true) && CursorBlink::DefaultOff.resolve(true));
        // A later file (config.local.toml) overrides earlier keys.
        assert!(
            !Config::parse("macos-option-as-alt = true\nmacos-option-as-alt = \"false\"\n")
                .option_as_meta
        );
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
    fn theme_colors_beat_config_colors() {
        let themed = Config::parse("theme = \"Motty Light\"\nbackground = \"#ff0000\"\n");
        assert_ne!(
            themed.resolve_theme(Mode::Light).theme.terminal.background,
            Rgba::rgb(0xff0000)
        );
        let bare = Config::parse("background = \"#ff0000\"\n");
        assert_eq!(
            bare.resolve_theme(Mode::Light).theme.terminal.background,
            Rgba::rgb(0xff0000)
        );
    }

    #[test]
    fn config_cursor_color_fills_in_for_themes_without_one() {
        let base = "[meta]\nname='x'\n[terminal]\nforeground='#ffffff'\nbackground='#000000'\npalette=['#000000','#000000','#000000','#000000','#000000','#000000','#000000','#000000','#000000','#000000','#000000','#000000','#000000','#000000','#000000','#000000']\n";
        let cfg = Config::parse("cursor-color = \"#ff0000\"\ncursor-text = \"#00ff00\"\n");
        let mut plain = theme::parse(base).unwrap().theme;
        cfg.apply_cursor(&mut plain);
        assert_eq!(
            (plain.cursor.block, plain.cursor.bar),
            (Rgba::rgb(0xff0000), Rgba::rgb(0xff0000))
        );
        assert_eq!(plain.cursor.block_text, Some(Rgba::rgb(0x00ff00)));
        let mut themed = theme::parse(&format!("{base}cursor = '#0000ff'\n"))
            .unwrap()
            .theme;
        cfg.apply_cursor(&mut themed);
        assert_eq!(themed.cursor.block, Rgba::rgb(0x0000ff));
    }

    #[test]
    fn missing_theme_falls_back_with_warning() {
        let cfg = Config::parse("theme = \"Does Not Exist 123\"\n");
        let t = cfg.resolve_theme(Mode::Light);
        assert_eq!(t.theme.meta.name, "Motty Light");
        assert_eq!(t.warnings.len(), 1);
    }

    #[test]
    fn keybind_lines_keep_their_raw_value() {
        let cfg = Config::parse(
            "keybind = cmd+t=new_tab\n# free cmd+d\nkeybind = cmd+d=unbind\nkeybind = cmd+==increase_font_size\nkeybind = \"ctrl+x=text:# not a comment\"\nkeybind = cmd+k=csi:17~\nkeybind = nothing\n",
        );
        let pairs: Vec<(&str, &str)> = cfg
            .keybinds
            .iter()
            .map(|k| (k.trigger.as_str(), k.action.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("cmd+t", "new_tab"),
                ("cmd+d", "unbind"),
                ("cmd+=", "increase_font_size"),
                ("ctrl+x", "text:# not a comment"),
                ("cmd+k", "csi:17~"),
            ]
        );
        assert_eq!(cfg.warnings.len(), 1, "{:?}", cfg.warnings);
    }

    #[test]
    fn set_in_keeps_the_rest_of_the_file() {
        let src = "# my config\ntheme = \"Nord\"\n# theme = \"x\"\nfont-size = 13\n\n";
        let out = set_in(src, "theme", &toml::Value::String("Paper".into()));
        assert_eq!(
            out,
            "# my config\ntheme = \"Paper\"\n# theme = \"x\"\nfont-size = 13\n\n"
        );
        let out = set_in(&out, "on-launch", &toml::Value::String("new_window".into()));
        assert!(out.ends_with("font-size = 13\non-launch = \"new_window\"\n"));
        assert_eq!(Config::parse(&out).theme.as_deref(), Some("Paper"));
        // `theme-dark` isn't `theme`.
        let out = set_in(
            "theme-dark = \"A\"\n",
            "theme",
            &toml::Value::String("B".into()),
        );
        assert_eq!(out, "theme-dark = \"A\"\ntheme = \"B\"\n");
    }

    #[test]
    fn remove_in_drops_every_line_for_the_key() {
        let src = "theme = \"A\"\nfont-size = 12\n# theme = \"B\"\ntheme=\"C\"\n";
        assert_eq!(remove_in(src, "theme"), "font-size = 12\n# theme = \"B\"\n");
        assert_eq!(remove_in("theme-dark = 1\n", "theme"), "theme-dark = 1\n");
    }

    #[test]
    fn keybind_lines_follow_the_wanted_chords() {
        let defaults = vec!["cmd+t".to_string()];
        let src = "font-size = 12\nkeybind = cmd+k=clear\n";
        // Rebind: the new chord runs it, the default stops.
        let out = set_keybinds_in(src, "new_tab", &defaults, &["cmd+y".into()]);
        assert_eq!(
            out,
            "font-size = 12\nkeybind = cmd+k=clear\nkeybind = cmd+t=unbind\nkeybind = cmd+y=new_tab\n"
        );
        let config = Config::parse(&out);
        assert_eq!(config.keybinds.len(), 3);
        // Back to the default: no lines left for it.
        let back = set_keybinds_in(&out, "new_tab", &defaults, &defaults);
        assert_eq!(back, src);
        // No shortcut at all.
        let none = set_keybinds_in(src, "new_tab", &defaults, &[]);
        assert!(none.ends_with("keybind = cmd+t=unbind\n"));
        // Quoted lines count too.
        let quoted = "keybind = \"cmd+y=new_tab\"\n";
        assert_eq!(
            set_keybinds_in(quoted, "new_tab", &defaults, &defaults),
            "\n"
        );
        assert_eq!(reset_keybinds_in(&out), "font-size = 12\n");
    }
}
