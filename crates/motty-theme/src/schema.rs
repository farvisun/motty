//! Every key config.toml takes: its type, default, allowed values, where
//! Settings shows it and what it does. `motty config schema` prints this
//! table, `config set` and `config validate` check values against it, and
//! the parser's unknown keys are the ones missing from it.

use toml::Value;

use crate::config::{AgentCommand, Keybind};
use crate::details_view::DetailsView;
use crate::tab_rule::TabRule;

/// What one value (or one entry of a list) holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Bool,
    Int {
        min: i64,
        max: i64,
    },
    Float {
        min: f64,
        max: f64,
    },
    Str,
    /// A file path (`~` and `$VAR` expand).
    Path,
    /// `#rrggbb`, `#rrggbbaa` or a CSS color name.
    Color,
    /// The name of an installed or built-in theme.
    Theme,
    /// One of `values` (or an older spelling in `also`); `bools` when
    /// `true` / `false` are accepted too.
    Enum {
        values: &'static [&'static str],
        also: &'static [&'static str],
        bools: bool,
    },
    /// `inherit`, `home`, or a folder.
    Dir,
    /// `NAME=value`.
    Pair,
    /// `trigger=action`.
    Keybind,
    /// `field="value" …` pairs (see [`crate::tab_rule`]).
    TabRule,
    /// `field="value" …` pairs (see [`crate::details_view`]).
    DetailsView,
    /// `N`, `Npx` or `N%` (see [`crate::text::CellHeight`]).
    CellHeight,
}

/// How a key holding several values is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    /// One line per entry, the key repeated.
    Repeated,
    /// One line holding an array.
    Inline,
}

#[derive(Clone, Copy, Debug)]
pub struct KeySpec {
    pub key: &'static str,
    pub kind: Kind,
    pub list: Option<List>,
    /// As config.toml would spell it; empty when there's none (the theme
    /// or the system decides).
    pub default: &'static str,
    /// Older or other terminals' names; writes rewrite them to `key`.
    pub aliases: &'static [&'static str],
    /// `Section › Group` in Settings; empty for file-only keys.
    pub settings: &'static str,
    pub description: &'static str,
}

const fn key(
    key: &'static str,
    kind: Kind,
    default: &'static str,
    settings: &'static str,
    description: &'static str,
) -> KeySpec {
    KeySpec {
        key,
        kind,
        list: None,
        default,
        aliases: &[],
        settings,
        description,
    }
}

impl KeySpec {
    const fn alias(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }

    const fn list(mut self, list: List) -> Self {
        self.list = Some(list);
        self
    }

    /// The key's name in `motty config show --json`.
    pub fn field(&self) -> String {
        self.key.replace('-', "_")
    }

    /// `bool`, `enum`, `list<string>`, …
    pub fn type_name(&self) -> String {
        let one = match self.kind {
            Kind::Bool => "bool",
            Kind::Int { .. } => "integer",
            Kind::Float { .. } => "float",
            Kind::Str => "string",
            Kind::Path => "path",
            Kind::Color => "color",
            Kind::Theme => "theme",
            Kind::Enum { .. } => "enum",
            Kind::Dir => "enum | path",
            Kind::Pair => "NAME=VALUE",
            Kind::Keybind => "trigger=action",
            Kind::TabRule => "rule",
            Kind::DetailsView => "view",
            Kind::CellHeight => "N | Npx | N%",
        };
        match self.list {
            Some(_) => format!("list<{one}>"),
            None => one.to_owned(),
        }
    }

    /// The values an enum takes (`true` / `false` included when it takes
    /// those too), or a range for numbers.
    pub fn allowed(&self) -> Vec<String> {
        match self.kind {
            Kind::Enum { values, bools, .. } => {
                let mut out: Vec<String> = values.iter().map(|v| (*v).to_owned()).collect();
                if bools {
                    for b in ["true", "false"] {
                        if !out.iter().any(|v| v == b) {
                            out.push(b.to_owned());
                        }
                    }
                }
                out
            }
            Kind::Bool => vec!["true".into(), "false".into()],
            Kind::Dir => vec!["inherit".into(), "home".into(), "<path>".into()],
            _ => Vec::new(),
        }
    }

    /// `min..=max` for numbers.
    pub fn range(&self) -> Option<(f64, f64)> {
        match self.kind {
            Kind::Int { min, max } => Some((min as f64, max as f64)),
            Kind::Float { min, max } => Some((min, max)),
            _ => None,
        }
    }

    /// The default as a value, `None` when there's none.
    pub fn default_value(&self) -> Option<Value> {
        if self.default.is_empty() {
            return self.list.map(|_| Value::Array(Vec::new()));
        }
        if self.key == "progress-bar-commands" {
            return Some(Value::Array(
                crate::config::PROGRESS_BAR_COMMANDS
                    .iter()
                    .map(|c| Value::String((*c).to_owned()))
                    .collect(),
            ));
        }
        if self.list.is_some() {
            // Described rather than listed (`(built-in)`).
            return Some(Value::Array(Vec::new()));
        }
        Some(match self.kind {
            Kind::Bool => Value::Boolean(self.default == "true"),
            Kind::Int { .. } => Value::Integer(self.default.parse().unwrap_or_default()),
            Kind::Float { .. } => Value::Float(self.default.parse().unwrap_or_default()),
            _ => Value::String(self.default.to_owned()),
        })
    }

    /// Entries of this list are named (`NAME=value`, `trigger=action`,
    /// `id="…"`), so a signed `+entry` replaces the one of the same name.
    pub fn entry_name(&self, entry: &str) -> Option<String> {
        match self.kind {
            Kind::Pair => entry.split_once('=').map(|(n, _)| n.trim().to_owned()),
            Kind::Keybind => Keybind::parse(entry).map(|k| k.trigger.to_lowercase()),
            Kind::DetailsView => DetailsView::parse(entry).ok().map(|v| v.id),
            Kind::Str if self.key == "open-with-app" => {
                entry.split_once('|').map(|(n, _)| n.trim().to_owned())
            }
            _ => None,
        }
    }
}

const BLINK: Kind = Kind::Enum {
    values: &["default_off", "default_on", "always_off", "always_on"],
    also: &[],
    bools: true,
};
const CONFIRM: Kind = Kind::Enum {
    values: &["process", "always", "never"],
    also: &[],
    bools: true,
};
const AUTO_HIDE: Kind = Kind::Enum {
    values: &["default", "always", "auto"],
    also: &[],
    bools: false,
};
const REPLAY: Kind = Kind::Enum {
    values: &["auto", "ask_once", "manually", "skip"],
    also: &["ask-once", "manual", "off"],
    bools: false,
};
const ACCESS: Kind = Kind::Enum {
    values: &["ask", "allow", "deny"],
    also: &[],
    bools: false,
};
const FACE: Kind = Kind::Enum {
    values: &["off", "auto", "primary-only", "synthetic"],
    also: &[],
    bools: true,
};
const FLOAT_0_1: Kind = Kind::Float { min: 0., max: 1. };
const WIDTH: Kind = Kind::Float {
    min: 0.,
    max: 4000.,
};

const fn bool(
    k: &'static str,
    default: &'static str,
    settings: &'static str,
    description: &'static str,
) -> KeySpec {
    key(k, Kind::Bool, default, settings, description)
}

const fn color(k: &'static str, description: &'static str) -> KeySpec {
    key(k, Kind::Color, "", "", description)
}

/// Every key, grouped as the Configuration Reference groups them.
#[rustfmt::skip]
pub const KEYS: &[KeySpec] = &[
    // Font
    key("font-family", Kind::Str, "JetBrains Mono", "Appearance › Font", "Terminal font family; falls back to the theme's font-mono list, then Menlo."),
    key("font-size", Kind::Float { min: 4., max: 200. }, "13", "Appearance › Font", "Font size in points."),
    key("font-family-bold", Kind::Str, "", "", "Family for bold cells; empty reuses font-family (see text-bold)."),
    key("font-family-italic", Kind::Str, "", "", "Family for italic cells; empty reuses font-family (see text-italic)."),
    key("font-family-bold-italic", Kind::Str, "", "", "Family for bold italic cells; empty reuses font-family-bold, then font-family."),
    key("font-ligatures", Kind::Enum { values: &["off", "calt", "dlig"], also: &[], bools: true }, "dlig", "Appearance › Text", "Ligature level: off, calt (standard and contextual) or dlig (discretionary too).").alias(&["ligatures"]),
    bool("font-ligatures-alphabet", "false", "Advanced › Text Rendering", "Ligatures inside runs of letters, digits and CJK too; off keeps them to symbol runs."),
    key("font-blending", Kind::Enum { values: &["srgb-over", "macos-like", "linear", "perceptual"], also: &[], bools: false }, "srgb-over", "Appearance › Text", "Glyph edge compositing. GPUI blends in gamma space, so srgb-over, macos-like and linear draw alike; perceptual thickens dark text on light backgrounds.").alias(&["font-smooth", "font-antialiased"]),
    key("text-bold", FACE, "auto", "Appearance › Text", "Bold cells: off (normal weight), auto or primary-only (the family's bold face), synthetic (overstruck when the family has no bold).").alias(&["font-bold"]),
    key("text-italic", FACE, "auto", "Appearance › Text", "Italic cells: off (upright), auto or primary-only (the family's italic face); synthetic acts as auto, since GPUI can't slant glyphs.").alias(&["font-italic"]),
    bool("text-underline", "true", "Appearance › Text", "Draw SGR underlines; link underlines and strikethrough stay either way.").alias(&["font-underline"]),
    bool("text-blink", "false", "Appearance › Text", "Blinking (SGR 5/6) cells blink about once a second; off draws them steady.").alias(&["font-blink"]),
    bool("font-thicken", "false", "Advanced › Text Rendering", "Darken glyph stems with a half-pixel overstrike; on by itself when the background is translucent."),
    key("adjust-cell-height", Kind::CellHeight, "", "Appearance › Text", "Cell height: N or Npx adds points, N% scales; negative compacts. Glyphs stay centred. Unset: the theme's adjust-cell-height."),
    bool("arrow-box-drawing-join", "true", "Advanced › Text Rendering", "Arrows ← → ↑ ↓ and triangles ◀ ▶ ▲ ▼ next to a box rule meet it on the centre line."),
    bool("strip-cjk-wrap-pad", "false", "Advanced › Text Rendering", "Hide the lone reverse-video space zsh leaves at a soft wrap before a wide character."),
    // Cursor
    key("cursor-style", Kind::Enum { values: &["block", "bar", "underline", "block_hollow"], also: &[], bools: false }, "block", "Appearance › Cursor", "Cursor shape programs get until they ask for another."),
    key("cursor-style-blink", BLINK, "default_off", "Appearance › Cursor", "default_* set the starting blink and let programs change it; always_* pin it.").alias(&["cursor-blink"]),
    key("cursor-animation", Kind::Enum { values: &["off", "smooth"], also: &[], bools: false }, "off", "Appearance › Cursor", "Cursor motion: smooth glides the caret on same-row moves and adds a small overshoot on click and focus."),
    color("cursor-color", "Cursor color, for themes that don't set one."),
    color("cursor-text", "Text color under the cursor, for themes that don't set one."),
    // Shell & environment
    key("command", Kind::Str, "$SHELL", "Shell › Shell", "Shell or command new terminals run (as a login shell).").alias(&["shell"]),
    key("widen-ambiguous", Kind::Enum { values: &["enclosed-alphanumerics", "number-forms", "math-operators", "misc-technical", "misc-symbols", "dingbats", "arrows", "geometric-shapes"], also: &[], bools: false }, "", "Advanced › Text Rendering", "Blocks whose East-Asian-Ambiguous characters take two cells, for new tabs. Only for fonts that draw them two cells wide.").list(List::Inline),
    bool("bidi", "true", "Advanced › Text Rendering", "Show Hebrew, Arabic and Persian right to left (Unicode bidi), per line when drawing; copy, paste and find keep the logical order. Programs can opt out with BDSM (CSI 8 h)."),
    key("term", Kind::Str, "auto", "Shell › Shell", "$TERM for new terminals: auto (xterm-256color) or an installed terminfo name; a missing one falls back to xterm-256color."),
    key("env", Kind::Pair, "", "", "An environment variable for new terminals, NAME=value. One line each.").list(List::Repeated),
    key("working-directory", Kind::Dir, "", "", "Where new terminals start: inherit, home or a folder (~ allowed). The per-kind keys below win."),
    key("window-working-directory", Kind::Dir, "home", "", "Where a new window's first terminal starts."),
    key("tab-working-directory", Kind::Dir, "inherit", "Shell › Working Directory", "Where a new tab starts."),
    key("split-working-directory", Kind::Dir, "inherit", "Shell › Working Directory", "Where a new split starts."),
    key("scrollback-lines", Kind::Int { min: 0, max: 100_000_000 }, "10000", "Shell › Shell", "Lines of scrollback kept per pane.").alias(&["scrollback"]),
    bool("dedup-repaint-scrollback", "true", "Advanced › All Settings", "Drop the stale copies programs that repaint a region taller than the window (Claude Code's classic renderer) leave in scrollback. Legitimate repeats are never removed."),
    bool("shell-integration", "true", "Shell › Shell", "Inject prompt marks and cwd reporting into zsh, bash and fish."),
    key("shell-integration-disabled-shells", Kind::Str, "", "", "Shell names (zsh, fish, …) shell integration leaves alone.").list(List::Inline),
    key("progress-bar-commands", Kind::Str, "(built-in list)", "", "Command prefixes that show a spinner while they run, matched as whole words. [] turns it off.").list(List::Inline),
    key("include", Kind::Path, "", "", "Another config file read before this one; later includes win and the main file wins over all.").list(List::Repeated),
    // Terminal identity & VT
    key("terminal-identity", Kind::Str, "motty", "Shell › Shell", "The name programs read when they ask which terminal this is (XTVERSION): motty, kitty, ghostty, or text of your own."),
    bool("kitty-keyboard", "true", "Advanced › Keyboard", "Programs may turn on the kitty keyboard protocol (unambiguous keys, key releases)."),
    bool("vt-keypad-app-allowed", "true", "", "Honour application keypad mode (DECKPAM); off, the keypad always types digits."),
    bool("vt-kam-allowed", "true", "", "Programs may lock the keyboard (KAM)."),
    key("enquiry-response", Kind::Str, "", "", "The reply to ENQ (0x05); empty sends none."),
    key("osc-color-report-format", Kind::Enum { values: &["none", "8-bit", "16-bit"], also: &[], bools: false }, "16-bit", "", "How OSC 4/10/11 color queries are answered: not at all, rgb:rr/gg/bb or rgb:rrrr/gggg/bbbb."),
    bool("title-report", "false", "", "Programs may read the title (XTWINOPS 21t)."),
    // Window
    key("unfocused-split-opacity", FLOAT_0_1, "0.6", "View › Splits", "How opaque unfocused split panes are drawn; 1.0 turns dimming off."),
    // Terminal colors
    color("foreground", "Text color of the built-in theme, used when no theme is set."),
    color("background", "Background of the built-in theme, used when no theme is set."),
    color("palette-0", "ANSI black, used when no theme is set."),
    color("palette-1", "ANSI red, used when no theme is set."),
    color("palette-2", "ANSI green, used when no theme is set."),
    color("palette-3", "ANSI yellow, used when no theme is set."),
    color("palette-4", "ANSI blue, used when no theme is set."),
    color("palette-5", "ANSI magenta, used when no theme is set."),
    color("palette-6", "ANSI cyan, used when no theme is set."),
    color("palette-7", "ANSI white, used when no theme is set."),
    color("palette-8", "ANSI bright black, used when no theme is set."),
    color("palette-9", "ANSI bright red, used when no theme is set."),
    color("palette-10", "ANSI bright green, used when no theme is set."),
    color("palette-11", "ANSI bright yellow, used when no theme is set."),
    color("palette-12", "ANSI bright blue, used when no theme is set."),
    color("palette-13", "ANSI bright magenta, used when no theme is set."),
    color("palette-14", "ANSI bright cyan, used when no theme is set."),
    color("palette-15", "ANSI bright white, used when no theme is set."),
    key("minimum-contrast", Kind::Float { min: 1., max: 21. }, "1.0", "Advanced › Text Rendering", "Lowest WCAG contrast text keeps against its background; text lightens or darkens to reach it. 1 turns it off."),
    // Theme
    key("theme", Kind::Theme, "", "Appearance › Theme", "Active theme (the light slot, and the dark one when theme-dark isn't set). Unset: Motty Light / Motty Dark."),
    key("theme-dark", Kind::Theme, "", "Appearance › Theme", "Theme while macOS is in dark mode."),
    // Mouse & input
    bool("mouse-hide-while-typing", "false", "Controls › Mouse", "Hide the pointer while typing."),
    bool("focus-follows-mouse", "false", "Controls › Mouse", "The pane under the pointer takes focus without a click."),
    key("macos-option-as-alt", Kind::Enum { values: &["true", "false", "left", "right"], also: &[], bools: true }, "false", "Controls › Keyboard", "Option sends Alt (Meta) to programs.").alias(&["option-as-meta"]),
    bool("shift-arrow-select", "true", "Selection › Selection", "⇧-arrows select instead of reaching the program."),
    key("right-click-action", Kind::Enum { values: &["context-menu", "copy", "paste", "copy-or-paste", "ignore"], also: &[], bools: false }, "context-menu", "Controls › Mouse", "What a right-click in a terminal does (⌃-click always opens the menu)."),
    // Links & open with
    bool("link-detection", "true", "Controls › Links", "Find URLs and paths under the pointer (OSC 8 links work either way)."),
    key("link-cmd-click", Kind::Enum { values: &["open", "copy", "nothing"], also: &[], bools: false }, "open", "Controls › Links", "What ⌘-click on a link does."),
    key("link-cmd-shift-click", Kind::Enum { values: &["reveal-finder", "open-system-default"], also: &[], bools: false }, "reveal-finder", "Controls › Links", "What ⌘⇧-click on a path does (URLs are copied)."),
    key("open-with-app", Kind::Str, "", "", "An app for Open In menus, \"Name|bundle.id\". One line each.").list(List::Repeated),
    // Clipboard & selection
    key("clipboard-read", ACCESS, "ask", "Advanced › Clipboard", "Programs reading the clipboard (OSC 52)."),
    key("clipboard-write", ACCESS, "allow", "Advanced › Clipboard", "Programs setting the clipboard (OSC 52)."),
    bool("clipboard-trim-trailing-spaces", "false", "Controls › Clipboard", "Copied lines lose their trailing spaces."),
    bool("copy-on-select", "false", "Controls › Clipboard", "Every selection goes straight to the clipboard."),
    bool("clipboard-paste-protection", "true", "Controls › Clipboard", "Ask before a risky paste (several lines, sudo, control characters)."),
    bool("clipboard-paste-bracketed-safe", "true", "Controls › Clipboard", "Don't ask when the program takes bracketed paste."),
    bool("selection-clear-on-typing", "true", "Selection › Selection", "Typing drops the selection."),
    bool("selection-clear-on-copy", "false", "Selection › Selection", "⌘C drops the selection (not with copy-on-select)."),
    // Sidebar & layout
    key("window-layout", Kind::Enum { values: &["sidebar-left", "tabs-top", "tabs-bottom"], also: &["sidebar", "top", "bottom"], bools: false }, "sidebar-left", "Appearance › Tabs", "Tabs in a sidebar, or a tab bar above or below."),
    key("auto-hide-tab-bar", AUTO_HIDE, "default", "", "When the horizontal tab bar shows: default (the toggle), always, or auto (two or more tabs).").alias(&["window-show-tab-bar"]),
    key("auto-hide-tabs-panel", AUTO_HIDE, "default", "", "When the tabs sidebar shows: default (the toggle), always, or auto (two or more tabs)."),
    bool("sidebar-visible", "true", "View › Tabs Panel", "Windows open with the tabs sidebar shown."),
    key("sidebar-width", WIDTH, "", "", "Tabs sidebar width in points (the theme's when unset); written back when you drag its edge."),
    key("details-panel-width", WIDTH, "", "", "Details panel width in points; written back when you drag its edge."),
    key("details-view", Kind::DetailsView, "(Info, Outline, Git, Files)", "View › Details Panel", "One details panel view per line, in tab order; built-ins you don't list follow yours.").list(List::Repeated),
    // Tab appearance
    bool("title-from-osc", "true", "View › Tabs & Title", "Programs may rename the tab and window (OSC 0/1/2)."),
    key("tab-rule", Kind::TabRule, "", "View › Tabs & Title", "A tab rule (alias, icon, title for matching tabs); one per line, highest priority first.").list(List::Repeated),
    key("tab-icon-placement", Kind::Enum { values: &["combined", "separate"], also: &[], bools: false }, "combined", "View › Tab Icon & Badge", "combined shares the badge slot; separate puts the icon left of the title."),
    // App behavior
    key("on-launch", Kind::Enum { values: &["restore_session", "new_window"], also: &[], bools: false }, "restore_session", "General › General", "Reopen the last session, or start with a new window (a crash still restores)."),
    bool("session-restore-banner", "true", "General › General", "\"Closed at\" / \"Restored at\" lines under a restored pane's scrollback."),
    bool("quit-after-last-window-closed", "false", "General › General", "Quit rather than stay in the Dock."),
    key("confirm-close-pane", CONFIRM, "process", "General › Close Confirmation", "When closing a split asks first."),
    key("confirm-close-tab", CONFIRM, "process", "General › Close Confirmation", "When closing a tab asks first."),
    key("confirm-close-window", Kind::Enum { values: &["process", "always", "multiple_tabs", "never"], also: &[], bools: true }, "process", "General › Close Confirmation", "When closing a window asks first."),
    bool("confirm-close-ignore-idle-agents", "false", "General › Close Confirmation", "An agent waiting at its prompt doesn't count as running."),
    bool("window-hide-dirty-indicator", "false", "General › Close Confirmation", "Never show the dot on the window's close button."),
    key("new-tab-position", Kind::Enum { values: &["auto", "end", "after-current"], also: &[], bools: false }, "auto", "Appearance › Tabs", "Where a new tab opens."),
    // Autocomplete
    key("autocomplete-shortcut", Kind::Enum { values: &["tab", "tab+right-arrow", "right-arrow", "ctrl+space", "disable"], also: &[], bools: false }, "tab", "Controls › Autocomplete", "The key that accepts a suggestion."),
    bool("autocomplete-inline-suggestion", "true", "Controls › Autocomplete", "Show the rest of a command you ran before, dimmed after the cursor."),
    bool("autocomplete-on-device-learning", "true", "Controls › Autocomplete", "Remember the commands you run (kept on this Mac)."),
    bool("autocomplete-skip-secrets", "false", "Controls › Autocomplete", "Never remember a command that looks like it carries a password, token or key."),
    key("autocomplete-history-ignore", Kind::Str, "", "Controls › Autocomplete", "Glob for commands never remembered (ssh *, export *TOKEN*). One line each.").list(List::Repeated),
    // Notifications, sounds & badges
    key("notification-foreground", Kind::Enum { values: &["off", "always", "tab-unfocused"], also: &[], bools: false }, "off", "Shell › Notification", "Banners while Motty is in front."),
    bool("privilege-sound-on-error", "false", "Shell › Notification", "A beep when a command exits non-zero."),
    bool("privilege-sound-shell", "true", "Shell › Notification", "Programs may ring the bell (BEL)."),
    bool("privilege-notification-on-finish", "false", "Shell › Notification", "A banner when a command exits 0."),
    bool("privilege-notification-on-error", "true", "Shell › Notification", "A banner when a command exits non-zero."),
    bool("privilege-notification-on-watch-finish", "true", "Shell › Notification", "A banner when a watched command finishes."),
    bool("privilege-notification-shell", "true", "Shell › Notification", "Programs may post banners (OSC 9 / 777 / 99)."),
    bool("privilege-badge-exit-status", "true", "Shell › Tab Badge", "A dot or triangle on the tab after a command."),
    bool("privilege-badge-agent-processing", "true", "Agents › Agent Behavior", "Badge while an agent is working."),
    bool("privilege-badge-agent-task-complete", "true", "Agents › Agent Behavior", "Badge when an agent finishes a task."),
    bool("privilege-badge-agent-awaiting-input", "true", "Agents › Agent Behavior", "Badge when an agent waits for you."),
    bool("privilege-notify-agent-task-complete", "true", "Agents › Agent Behavior", "A banner when an agent finishes a task."),
    bool("privilege-notify-agent-awaiting-input", "true", "Agents › Agent Behavior", "A banner when an agent waits for you."),
    bool("privilege-caffeinate-agent-processing", "false", "Agents › Agent Behavior", "Keep the Mac awake while an agent works."),
    bool("privilege-resume-agent-session", "true", "Agents › Agent Behavior", "A recovered pane resumes the agent session it had."),
    bool("privilege-mouse-shell", "true", "Advanced › Mouse", "Programs may turn on mouse reporting."),
    bool("privilege-title-shell", "true", "Advanced › Title", "Programs may set the title (OSC 0 / 2)."),
    bool("privilege-clipboard-shell", "true", "Advanced › Clipboard", "Programs may use the clipboard at all (OSC 52)."),
    // IPC security
    bool("ipc-allow-send-keys", "false", "Advanced › IPC", "motty send and agents may type into panes."),
    bool("ipc-allow-sensitive-sessions", "false", "Advanced › IPC", "…even into SSH sessions and password prompts."),
    // Secure input
    bool("auto-secure-input", "true", "Controls › Secure Input", "Secure Keyboard Entry while a password prompt waits."),
    bool("secure-input-indication", "true", "Controls › Secure Input", "A titlebar pill while Secure Keyboard Entry is on."),
    // Recipes
    key("recipe-replay-saved", REPLAY, "ask_once", "Recipes › Command Replay", "How a saved recipe's commands run when it opens."),
    key("recipe-replay-file", REPLAY, "manually", "Recipes › Command Replay", "How a recipe file's commands run when it opens."),
    // Agents
    key("agent-command", Kind::Pair, "", "Agents › Coding Agents", "What starts an agent, AGENT=COMMAND (a wrapper, a path, global flags). One line per agent.").list(List::Repeated),
    key("agent-quota-off", Kind::Str, "", "Agents › Coding Agents", "An agent whose plan quota isn't read. One line each.").list(List::Repeated),
    // Editor
    bool("editor-line-wrap", "true", "Editor › Editor", "Wrap long lines instead of scrolling sideways."),
    key("editor-tab-size", Kind::Int { min: 1, max: 16 }, "4", "Editor › Editor", "A tab's width in columns."),
    bool("editor-visible-whitespace", "false", "Editor › Editor", "Spaces, tabs and line ends as glyphs."),
    bool("editor-show-line-numbers", "true", "Editor › Editor", "The line-number gutter."),
    bool("editor-default-to-preview-readonly", "true", "Editor › Editor", "Markdown, SVG and HTML open as previews, and files open read-only."),
    bool("editor-scroll-past-end", "true", "Editor › Editor", "The last line can scroll up to the top."),
    // Terminal scrolling
    key("terminal-scroll-past-end", Kind::Enum { values: &["disabled", "last-line-with-content", "last-line-in-middle", "cursor-line"], also: &[], bools: false }, "disabled", "Controls › Scroll", "Scroll past the last line: where the last row with text (or the cursor's) can be scrolled to. Always off on the alternate screen."),
    key("terminal-scroll-past-first-line", Kind::Enum { values: &["disabled", "same-as-last-line", "first-line-with-content", "first-line-in-middle"], also: &[], bools: false }, "disabled", "Controls › Scroll", "Scroll past the first scrollback line, pushing it down into the view."),
    bool("terminal-scroll-past-end-sticky", "false", "", "Keep the past-end offset when output arrives instead of using it up."),
    bool("terminal-scroll-smooth", "true", "Controls › Scroll", "Pixel-granular scrolling; snaps to the nearest row when the gesture ends."),
    key("output-scroll-animation", Kind::Enum { values: &["auto", "off", "smooth"], also: &[], bools: false }, "auto", "Advanced › All Settings", "How the view follows the scrolling the terminal does to itself: auto follows terminal-scroll-smooth."),
    bool("terminal-scrollbar-hide-on-auto-scroll", "false", "Advanced › All Settings", "Show the scrollbar only while the view is parked away from the live output, not while it follows output."),
    // Dock icon
    bool("dock-icon-animate-progress", "false", "Appearance › Dock Icon", "A progress bar on the Dock icon while a command reports progress."),
    bool("dock-icon-error-badge", "true", "Appearance › Dock Icon", "A red Dock icon after a failure in a tab you haven't looked at."),
    // Keybindings
    key("keybind", Kind::Keybind, "(built-in)", "Key Bindings › Shortcuts", "Bind a chord to an action, unbind, or text:/csi:/esc: payloads. One line each.").list(List::Repeated),
];

/// The key's spec, by its name or an alias.
pub fn lookup(name: &str) -> Option<&'static KeySpec> {
    KEYS.iter()
        .find(|k| k.key == name)
        .or_else(|| KEYS.iter().find(|k| k.aliases.contains(&name)))
}

/// `name`'s real name (`shell` → `command`); unknown keys stay as they are.
pub fn canonical(name: &str) -> &str {
    lookup(name).map_or(name, |k| k.key)
}

/// Whether `value` is one `kind` takes.
fn check_one(spec: &KeySpec, value: &Value) -> Result<(), String> {
    let want = |what: &str| Err(format!("expected {what}, got {value}"));
    match spec.kind {
        Kind::Bool => match value {
            Value::Boolean(_) => Ok(()),
            _ => want("true or false"),
        },
        Kind::Int { min, max } => match value.as_integer() {
            Some(n) if (min..=max).contains(&n) => Ok(()),
            Some(n) => Err(format!("{n} is outside {min}–{max}")),
            None => want("a whole number"),
        },
        Kind::Float { min, max } => {
            let n = value
                .as_float()
                .or_else(|| value.as_integer().map(|i| i as f64));
            match n {
                Some(n) if n >= min && n <= max => Ok(()),
                Some(n) => Err(format!("{n} is outside {min}–{max}")),
                None => want("a number"),
            }
        }
        Kind::Enum {
            values,
            also,
            bools,
        } => match value {
            Value::Boolean(_) if bools => Ok(()),
            Value::String(s) if values.contains(&s.as_str()) || also.contains(&s.as_str()) => {
                Ok(())
            }
            _ => Err(format!(
                "{value} isn't one of {}",
                spec.allowed().join(", ")
            )),
        },
        Kind::Color => match value.as_str().map(crate::css::parse_color) {
            Some(Ok(_)) => Ok(()),
            Some(Err(e)) => Err(e.0),
            None => want("a color such as \"#1e1e2e\""),
        },
        Kind::Theme => match value.as_str() {
            Some(name) if crate::config::find_theme(name).is_some() => Ok(()),
            Some(name) => Err(format!("no theme named {name:?} (see `motty theme list`)")),
            None => want("a theme name"),
        },
        Kind::Pair => match value.as_str().and_then(AgentCommand::parse) {
            Some(_) => Ok(()),
            None => want("NAME=value"),
        },
        Kind::Keybind => match value.as_str().and_then(Keybind::parse) {
            Some(_) => Ok(()),
            None => want("trigger=action"),
        },
        Kind::TabRule => match value.as_str() {
            Some(s) => TabRule::parse(s).map(|_| ()),
            None => want("a rule such as 'path=\"~/x\" icon=\"rust\"'"),
        },
        Kind::DetailsView => match value.as_str() {
            Some(s) => DetailsView::parse(s).map(|_| ()),
            None => want("a view such as 'id=\"git\"'"),
        },
        Kind::CellHeight => match crate::text::CellHeight::parse(value) {
            Some(_) => Ok(()),
            None => want("N, Npx or N% (2, 2px, 10%)"),
        },
        Kind::Str | Kind::Path | Kind::Dir => match value {
            Value::String(_) => Ok(()),
            _ => want("a string"),
        },
    }
}

/// Whether `value` (one entry for a repeated key, the array for an inline
/// one) suits `spec`.
pub fn check(spec: &KeySpec, value: &Value) -> Result<(), String> {
    match (spec.list, value) {
        (Some(_), Value::Array(items)) => items.iter().try_for_each(|v| check_one(spec, v)),
        // A comma-separated string is read as the list too.
        (Some(List::Inline), Value::String(_)) => Ok(()),
        _ => check_one(spec, value),
    }
}

/// A value typed on the command line, as the key's type reads it:
/// `true`, `14`, `0.8`, or text taken as it is.
pub fn parse_text(spec: &KeySpec, text: &str) -> Result<Value, String> {
    let text = text.trim();
    let boolean = match text {
        "true" | "on" | "yes" => Some(true),
        "false" | "off" | "no" => Some(false),
        _ => None,
    };
    let value = match spec.kind {
        Kind::Bool => {
            Value::Boolean(boolean.ok_or_else(|| format!("expected true or false, got {text:?}"))?)
        }
        Kind::Int { .. } => Value::Integer(
            text.parse()
                .map_err(|_| format!("expected a whole number, got {text:?}"))?,
        ),
        // `14` stays `14` in the file.
        Kind::Float { .. } => match text.parse::<i64>() {
            Ok(n) => Value::Integer(n),
            Err(_) => Value::Float(
                text.parse()
                    .map_err(|_| format!("expected a number, got {text:?}"))?,
            ),
        },
        // `2` stays a number; `2px` and `10%` are strings.
        Kind::CellHeight => match text.parse::<i64>() {
            Ok(n) => Value::Integer(n),
            Err(_) => match text.parse::<f64>() {
                Ok(f) => Value::Float(f),
                Err(_) => Value::String(text.to_owned()),
            },
        },
        // `off` is a word of its own here.
        Kind::Enum { values, bools, .. } => match boolean {
            Some(b) if bools && !values.contains(&text) => Value::Boolean(b),
            _ => Value::String(text.to_owned()),
        },
        _ => Value::String(text.to_owned()),
    };
    check_one(spec, &value)?;
    Ok(value)
}

/// One `key = value` line as Motty writes it.
pub fn line(key: &str, value: &Value) -> String {
    let kind = lookup(key).map(|k| k.kind);
    match (kind, value) {
        // `keybind = cmd+t=new_tab` is written bare, as Otty does.
        (Some(Kind::Keybind), Value::String(s)) if !s.contains('#') => format!("{key} = {s}"),
        // Rules hold double quotes; a literal string keeps them readable.
        (Some(Kind::TabRule | Kind::DetailsView), Value::String(s))
            if !s.contains('\'') && !s.contains('\n') =>
        {
            format!("{key} = '{s}'")
        }
        _ => format!("{key} = {value}"),
    }
}

/// A value as `config get` prints it: strings bare, the rest as TOML.
pub fn display(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_aliases_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for k in KEYS {
            assert!(seen.insert(k.key), "{} twice", k.key);
            for a in k.aliases {
                assert!(seen.insert(a), "{a} twice");
            }
        }
        assert_eq!(canonical("shell"), "command");
        assert_eq!(canonical("ligatures"), "font-ligatures");
        assert_eq!(canonical("no-such-key"), "no-such-key");
    }

    #[test]
    fn defaults_pass_their_own_checks() {
        for k in KEYS {
            let Some(value) = k.default_value() else {
                continue;
            };
            if k.list.is_some() || matches!(k.kind, Kind::Str | Kind::Path) {
                continue;
            }
            assert_eq!(check(k, &value), Ok(()), "{}", k.key);
        }
    }

    #[test]
    fn command_line_values() {
        let size = lookup("font-size").unwrap();
        assert_eq!(parse_text(size, "14"), Ok(Value::Integer(14)));
        assert_eq!(parse_text(size, "13.5"), Ok(Value::Float(13.5)));
        assert!(parse_text(size, "big").is_err());
        assert!(parse_text(size, "900").is_err());
        let style = lookup("cursor-style").unwrap();
        assert!(parse_text(style, "bar").is_ok());
        assert!(parse_text(style, "triangle").unwrap_err().contains("block"));
        let lig = lookup("font-ligatures").unwrap();
        assert_eq!(parse_text(lig, "off"), Ok(Value::String("off".into())));
        assert_eq!(parse_text(lig, "true"), Ok(Value::Boolean(true)));
        let alt = lookup("macos-option-as-alt").unwrap();
        assert_eq!(parse_text(alt, "left"), Ok(Value::String("left".into())));
        assert_eq!(parse_text(alt, "true"), Ok(Value::String("true".into())));
        let on = lookup("copy-on-select").unwrap();
        assert_eq!(parse_text(on, "on"), Ok(Value::Boolean(true)));
        assert!(parse_text(lookup("editor-tab-size").unwrap(), "40").is_err());
        assert!(parse_text(lookup("theme").unwrap(), "Nord").is_ok());
        assert!(parse_text(lookup("theme").unwrap(), "No Such Theme").is_err());
        assert!(parse_text(lookup("env").unwrap(), "EDITOR=nvim").is_ok());
        assert!(parse_text(lookup("env").unwrap(), "EDITOR").is_err());
        assert!(parse_text(lookup("keybind").unwrap(), "cmd+t=new_tab").is_ok());
        assert!(parse_text(lookup("cursor-color").unwrap(), "#zzzzzz").is_err());
        let height = lookup("adjust-cell-height").unwrap();
        assert_eq!(parse_text(height, "2"), Ok(Value::Integer(2)));
        assert_eq!(parse_text(height, "10%"), Ok(Value::String("10%".into())));
        assert!(parse_text(height, "tall").is_err());
        let widen = lookup("widen-ambiguous").unwrap();
        assert!(check(widen, &Value::Array(vec![Value::String("arrows".into())])).is_ok());
        assert!(check(widen, &Value::Array(vec![Value::String("emoji".into())])).is_err());
        assert_eq!(canonical("font-bold"), "text-bold");
        assert_eq!(canonical("font-smooth"), "font-blending");
    }

    #[test]
    fn named_entries() {
        let env = lookup("env").unwrap();
        assert_eq!(env.entry_name("EDITOR=nvim").as_deref(), Some("EDITOR"));
        let kb = lookup("keybind").unwrap();
        assert_eq!(
            kb.entry_name("cmd+==increase_font_size").as_deref(),
            Some("cmd+=")
        );
        let view = lookup("details-view").unwrap();
        assert_eq!(
            view.entry_name("id=\"git\" enabled=\"false\"").as_deref(),
            Some("git")
        );
        assert_eq!(
            lookup("agent-quota-off").unwrap().entry_name("claude"),
            None
        );
    }
}
