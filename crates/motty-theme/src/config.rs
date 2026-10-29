//! `config.toml`: flat kebab-case `key = value` lines, compatible with Otty's
//! config file. Values are TOML literals, but keys may repeat: a repeated key
//! is a list (e.g. several `open-with-app` entries), so the file is parsed
//! line by line rather than as a strict TOML document.

use std::path::{Path, PathBuf};

use crate::css::{self, Rgba};
use crate::details_view::DetailsView;
use crate::schema;
use crate::store::Files;
use crate::tab_rule::TabRule;
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
    /// `details-view` lines, in tab order (see [`crate::details_view`]).
    pub details_views: Vec<DetailsView>,
    pub option_as_meta: bool,
    pub scrollback: Option<usize>,
    /// Inject Motty's prompt marks and cwd reporting into the shell.
    pub shell_integration: bool,
    /// Shell names (`zsh`, `fish`) to leave alone.
    pub shell_integration_disabled_shells: Vec<String>,
    /// `command` (or `shell`): what new terminals run.
    pub shell: Option<String>,
    /// `term`: the `$TERM` shells get; `None` for `auto` (xterm-256color).
    pub term: Option<String>,
    /// `env = NAME=value` lines, in file order.
    pub env: Vec<(String, String)>,
    /// `cursor-style`: the shape programs get until they ask for another.
    pub cursor_style: CursorStyle,
    pub cursor_blink: CursorBlink,
    /// Smooth scrolling, the output slide, the cursor glide, scrolling past
    /// the ends, the scrollbar and repaint de-duplication.
    pub motion: Motion,
    /// `cursor-color` and `cursor-text`, for themes that don't set them.
    pub cursor_color: Option<Rgba>,
    pub cursor_text: Option<Rgba>,
    /// `confirm-close-pane`, `confirm-close-tab`, `confirm-close-window`.
    pub confirm_close_pane: ConfirmClose,
    pub confirm_close_tab: ConfirmClose,
    pub confirm_close_window: ConfirmClose,
    /// `window-hide-dirty-indicator`: never show the close button's dot.
    pub hide_dirty_indicator: bool,
    /// `session-restore-banner`: "Closed at" / "Restored at" lines under a
    /// restored pane's scrollback.
    pub restore_banner: bool,
    /// `confirm-close-ignore-idle-agents`: an agent waiting at its prompt
    /// doesn't count as running when closing.
    pub close_ignores_idle_agents: bool,
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
    /// `agent-command = <agent>=<command>` lines, in file order (see
    /// [`Config::agent_command`]).
    pub agent_commands: Vec<AgentCommand>,
    /// `agent-quota-off = claude` entries: the agents whose plan quota
    /// isn't read. The ones switched off are recorded, so an agent that
    /// learns to report a quota arrives switched on.
    pub agent_quota_off: Vec<String>,
    /// The `autocomplete-*` keys.
    pub autocomplete: Autocomplete,
    /// `keybind = <trigger>=<action>` entries, in file order.
    pub keybinds: Vec<Keybind>,
    /// The `privilege-*` keys for tab badges and agent notifications.
    pub badges: Badges,
    /// Selection, clipboard, mouse and Secure Keyboard Entry switches.
    pub behavior: Behavior,
    /// `progress-bar-commands`: command prefixes that show a spinner while
    /// they run; `None` for the built-in list.
    pub progress_bar_commands: Option<Vec<String>>,
    /// `quit-after-last-window-closed`: quit rather than stay in the Dock.
    pub quit_after_last_window: bool,
    /// `working-directory`, and `window-`, `tab-` and `split-working-directory`
    /// for each kind of new terminal.
    pub working_directory: Option<WorkingDir>,
    pub window_working_directory: Option<WorkingDir>,
    pub tab_working_directory: Option<WorkingDir>,
    pub split_working_directory: Option<WorkingDir>,
    /// `new-tab-position`.
    pub new_tab_position: NewTabPosition,
    /// `auto-hide-tabs-panel`.
    pub auto_hide_tabs_panel: AutoHide,
    /// `auto-hide-tab-bar`, for the horizontal tab bar.
    pub auto_hide_tab_bar: AutoHide,
    /// `window-layout`: the sidebar, or a tab bar above or below.
    pub window_layout: WindowLayout,
    /// `tab-rule` lines, highest priority first.
    pub tab_rules: Vec<TabRule>,
    /// `tab-icon-placement`.
    pub tab_icon_placement: IconPlacement,
    /// `title-from-osc` (see [`Config::programs_set_title`]).
    pub title_from_osc: Option<bool>,
    /// `recipe-replay-saved` (see [`Config::replay_saved`]).
    pub recipe_replay_saved: Option<Replay>,
    /// `recipe-replay-file` (see [`Config::replay_file`]).
    pub recipe_replay_file: Option<Replay>,
    /// `notification-foreground`: banners while Motty is in front.
    pub notification_foreground: NotificationForeground,
    /// `clipboard-read`: a program reading the clipboard (OSC 52).
    pub clipboard_read: Access,
    /// `clipboard-write`: a program setting the clipboard (OSC 52).
    pub clipboard_write: Access,
    /// The `editor-*` keys: how file panes show text.
    pub editor: EditorPrefs,
    /// Terminal identity and the VT requests honoured.
    pub vt: VtPrefs,
    /// Where the config was read from, if anywhere.
    pub path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

/// What marks a tab and what posts a notification (Otty's `privilege-*`
/// keys; all on except keeping the Mac awake).
/// The `Terminal Identity & VT` keys, plus `kitty-keyboard`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VtPrefs {
    /// `kitty-keyboard`: programs may turn on the kitty keyboard protocol.
    pub kitty_keyboard: bool,
    /// `vt-keypad-app-allowed`: DECKPAM is honoured.
    pub keypad_app_allowed: bool,
    /// `vt-kam-allowed`: KAM may lock the keyboard.
    pub kam_allowed: bool,
    /// `enquiry-response`: the reply to ENQ.
    pub enquiry_response: String,
    /// `osc-color-report-format`: `none`, `8-bit` or `16-bit`.
    pub color_report: ColorReport,
    /// `title-report`: programs may read the title (XTWINOPS 21t).
    pub title_report: bool,
    /// `terminal-identity`: `motty`, `kitty`, `ghostty` or text of your
    /// own, for XTVERSION.
    pub identity: String,
}

impl Default for VtPrefs {
    fn default() -> Self {
        Self {
            kitty_keyboard: true,
            keypad_app_allowed: true,
            kam_allowed: true,
            enquiry_response: String::new(),
            color_report: ColorReport::Bits16,
            title_report: false,
            identity: "motty".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorReport {
    None,
    Bits8,
    #[default]
    Bits16,
}

/// Settings › Editor: how file panes show text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorPrefs {
    /// `editor-line-wrap`: wrap long lines instead of scrolling sideways.
    pub line_wrap: bool,
    /// `editor-show-line-numbers`.
    pub line_numbers: bool,
    /// `editor-visible-whitespace`: spaces, tabs and line ends as glyphs.
    pub whitespace: bool,
    /// `editor-tab-size`: a tab's width in columns (1–16).
    pub tab_size: usize,
    /// `editor-scroll-past-end`: the last line can scroll up to the top.
    pub scroll_past_end: bool,
    /// `editor-default-to-preview-readonly`: Markdown, SVG and HTML open
    /// as previews, and files open read-only.
    pub preview_readonly: bool,
}

impl Default for EditorPrefs {
    fn default() -> Self {
        Self {
            line_wrap: true,
            line_numbers: true,
            whitespace: false,
            tab_size: 4,
            scroll_past_end: true,
            preview_readonly: true,
        }
    }
}

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
    /// `privilege-resume-agent-session`: a recovered pane resumes the
    /// agent session it had.
    pub resume_session: bool,
    /// `privilege-notification-on-finish`: a banner when a command exits 0.
    pub notify_finish: bool,
    /// `privilege-notification-on-error`: …when one exits non-zero.
    pub notify_error: bool,
    /// `privilege-notification-on-watch-finish`: …when a watched command
    /// finishes.
    pub notify_watch: bool,
    /// `privilege-notification-shell`: programs may post banners (OSC 9 /
    /// 777 / 99).
    pub notify_shell: bool,
    /// `privilege-sound-on-error`: a beep when a command exits non-zero.
    pub sound_on_error: bool,
    /// `privilege-sound-shell`: programs may ring the bell (BEL).
    pub sound_shell: bool,
    /// `privilege-title-shell`: programs may set the title (OSC 0 / 2).
    pub title_shell: bool,
    /// `privilege-clipboard-shell`: OSC 52 at all (see `clipboard-read` /
    /// `clipboard-write`).
    pub clipboard_shell: bool,
    /// `privilege-mouse-shell`: programs may turn on mouse reporting.
    pub mouse_shell: bool,
    /// `ipc-allow-send-keys`: `motty pane send-text|send-keys` and agents may type into panes.
    pub ipc_send_keys: bool,
    /// `ipc-allow-sensitive-sessions`: …even into SSH sessions and
    /// password prompts. AppleScript's `do script`, `contents` and
    /// `history` reach SSH and sudo sessions only with it.
    pub ipc_sensitive: bool,
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
            resume_session: true,
            notify_finish: false,
            notify_error: true,
            notify_watch: true,
            notify_shell: true,
            sound_on_error: false,
            sound_shell: true,
            title_shell: true,
            clipboard_shell: true,
            mouse_shell: true,
            ipc_send_keys: false,
            ipc_sensitive: false,
        }
    }
}

/// Otty's on/off switches for selection, the clipboard, the mouse, Secure
/// Keyboard Entry and the Dock icon, with Otty's defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Behavior {
    /// `copy-on-select`
    pub copy_on_select: bool,
    /// `clipboard-trim-trailing-spaces`
    pub trim_trailing_spaces: bool,
    /// `clipboard-paste-protection`: ask before a risky paste.
    pub paste_protection: bool,
    /// `clipboard-paste-bracketed-safe`: no asking when the program takes
    /// bracketed paste (the text arrives inert).
    pub paste_bracketed_safe: bool,
    /// `selection-clear-on-typing`
    pub clear_selection_on_typing: bool,
    /// `selection-clear-on-copy`: after ⌘C (not with copy-on-select).
    pub clear_selection_on_copy: bool,
    /// `shift-arrow-select`: ⇧-arrows select instead of reaching the program.
    pub shift_arrow_select: bool,
    /// `mouse-hide-while-typing`
    pub hide_mouse_while_typing: bool,
    /// `focus-follows-mouse`
    pub focus_follows_mouse: bool,
    /// `auto-secure-input`: Secure Keyboard Entry at password prompts.
    pub auto_secure_input: bool,
    /// `secure-input-indication`: the titlebar pill while it's on.
    pub secure_input_indication: bool,
    /// `dock-icon-animate-progress`
    pub dock_progress: bool,
    /// `dock-icon-error-badge`: a red Dock icon after a failure.
    pub dock_error_badge: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            copy_on_select: false,
            trim_trailing_spaces: false,
            paste_protection: true,
            paste_bracketed_safe: true,
            clear_selection_on_typing: true,
            clear_selection_on_copy: false,
            shift_arrow_select: true,
            hide_mouse_while_typing: false,
            focus_follows_mouse: false,
            auto_secure_input: true,
            secure_input_indication: true,
            dock_progress: false,
            dock_error_badge: true,
        }
    }
}

/// Commands that get a spinner while they run, matched as whole-word
/// prefixes (`git push` matches `git push origin`, not `git status`).
pub const PROGRESS_BAR_COMMANDS: &[&str] = &[
    "curl",
    "wget",
    "rsync",
    "scp",
    "git fetch",
    "git pull",
    "git push",
    "git clone",
    "brew install",
    "brew update",
    "brew upgrade",
    "npm install",
    "npm ci",
    "pnpm install",
    "yarn install",
    "bun install",
    "pip install",
    "pip3 install",
    "cargo build",
    "cargo install",
    "cargo update",
    "docker pull",
    "docker push",
    "docker build",
    "apt install",
    "apt update",
    "apt upgrade",
    "apt-get install",
    "apt-get update",
    "apt-get upgrade",
];

impl Config {
    /// Replay for recipes you saved; Otty's default asks once.
    pub fn replay_saved(&self) -> Replay {
        self.recipe_replay_saved.unwrap_or(Replay::AskOnce)
    }

    /// Replay for recipe files from elsewhere: one at a time.
    pub fn replay_file(&self) -> Replay {
        self.recipe_replay_file.unwrap_or(Replay::Manually)
    }

    /// The command prefixes that show a spinner.
    pub fn progress_commands(&self) -> Vec<String> {
        match &self.progress_bar_commands {
            Some(list) => list.clone(),
            None => PROGRESS_BAR_COMMANDS
                .iter()
                .map(|c| (*c).to_owned())
                .collect(),
        }
    }
}

/// Whether `command` starts with `prefix` as whole words.
pub fn matches_prefix(command: &str, prefix: &str) -> bool {
    let mut words = command.split_whitespace();
    prefix.split_whitespace().all(|p| words.next() == Some(p)) && !prefix.trim().is_empty()
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

/// Where a new terminal starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkingDir {
    /// Where the active pane is.
    Inherit,
    Home,
    Path(PathBuf),
}

impl WorkingDir {
    fn parse(value: &str) -> Self {
        match value {
            "inherit" => Self::Inherit,
            // `profile` is Windows'; elsewhere it's home.
            "home" | "profile" | "" => Self::Home,
            path => Self::Path(match path.strip_prefix("~/") {
                Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
                None if path == "~" => dirs::home_dir().unwrap_or_default(),
                None => PathBuf::from(path),
            }),
        }
    }

    /// The value as config.toml spells it.
    pub fn key(&self) -> String {
        match self {
            Self::Inherit => "inherit".into(),
            Self::Home => "home".into(),
            Self::Path(p) => p.display().to_string(),
        }
    }
}

/// What kind of new terminal a working directory is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewTerminal {
    Window,
    Tab,
    Split,
}

/// `notification-foreground` values: whether banners show while Motty is
/// the active app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotificationForeground {
    /// Only while another app is in front (macOS's own rule).
    #[default]
    Off,
    Always,
    /// When it comes from a pane you aren't looking at.
    TabUnfocused,
}

/// `clipboard-read` / `clipboard-write` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Access {
    #[default]
    Ask,
    Allow,
    Deny,
}

/// How a recipe's commands run when it opens (`recipe-replay-*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replay {
    /// Each runs when the one before it is done.
    Auto,
    /// Typed in together; one ↩ runs them.
    AskOnce,
    /// Typed one at a time; ↩ runs each.
    Manually,
    /// Layout only.
    Skip,
}

impl Replay {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "auto" => Self::Auto,
            "ask_once" | "ask-once" => Self::AskOnce,
            "manually" | "manual" => Self::Manually,
            "skip" | "off" => Self::Skip,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::AskOnce => "ask_once",
            Self::Manually => "manually",
            Self::Skip => "skip",
        }
    }
}

/// `auto-hide-tabs-panel` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutoHide {
    /// Only the toggle; a new window copies the last one's.
    #[default]
    Default,
    /// New windows open with it shown.
    Always,
    /// Shown with two or more tabs.
    Auto,
}

/// `window-layout` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowLayout {
    #[default]
    SidebarLeft,
    TabsTop,
    TabsBottom,
}

impl WindowLayout {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sidebar-left" | "sidebar" => Self::SidebarLeft,
            "tabs-top" | "top" => Self::TabsTop,
            "tabs-bottom" | "bottom" => Self::TabsBottom,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::SidebarLeft => "sidebar-left",
            Self::TabsTop => "tabs-top",
            Self::TabsBottom => "tabs-bottom",
        }
    }

    /// The tab bar runs across the window instead of down its side.
    pub fn horizontal(self) -> bool {
        self != Self::SidebarLeft
    }
}

/// `tab-icon-placement` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconPlacement {
    /// Shares the badge slot on the right; a badge takes over while one
    /// is showing.
    #[default]
    Combined,
    /// Left of the title, badges on the right, shell name hidden.
    Separate,
}

impl AutoHide {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "default" => Self::Default,
            "always" => Self::Always,
            "auto" => Self::Auto,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Always => "always",
            Self::Auto => "auto",
        }
    }
}

/// `new-tab-position` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NewTabPosition {
    /// At the end.
    #[default]
    Auto,
    End,
    AfterCurrent,
}

/// An `agent-command` line: what starts an agent in place of its own
/// program (a wrapper, an absolute path, global flags). The arguments that
/// resume or fork a session are added after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCommand {
    /// The catalog id: `claude`, `codex`, `kimi`, …
    pub agent: String,
    /// Empty for the built-in command.
    pub command: String,
}

impl AgentCommand {
    /// `claude=claude --dangerously-skip-permissions`: the agent id up to the
    /// first `=`, the command after it.
    pub fn parse(s: &str) -> Option<Self> {
        let (agent, command) = s.split_once('=')?;
        let agent = agent.trim();
        if agent.is_empty() || agent.contains(char::is_whitespace) {
            return None;
        }
        Some(Self {
            agent: agent.to_owned(),
            command: command.trim().to_owned(),
        })
    }
}

impl Config {
    /// The command `agent` is started with, from its last `agent-command`
    /// line; `None` for the built-in one.
    pub fn agent_command(&self, agent: &str) -> Option<&str> {
        self.agent_commands
            .iter()
            .rfind(|c| c.agent == agent)
            .map(|c| c.command.as_str())
            .filter(|c| !c.is_empty())
    }
}

impl Config {
    /// Show Plan Quota for an agent: on unless `agent-quota-off` names it.
    pub fn shows_quota(&self, agent: &str) -> bool {
        !self.agent_quota_off.iter().any(|a| a == agent)
    }

    /// `title-from-osc`, on unless turned off: programs may rename tabs.
    pub fn programs_set_title(&self) -> bool {
        self.title_from_osc.unwrap_or(true)
    }

    /// Where a new window, tab or split starts: its own key, else
    /// `working-directory`, else home for a window and the active pane's
    /// folder otherwise.
    pub fn working_dir(&self, kind: NewTerminal) -> WorkingDir {
        let own = match kind {
            NewTerminal::Window => &self.window_working_directory,
            NewTerminal::Tab => &self.tab_working_directory,
            NewTerminal::Split => &self.split_working_directory,
        };
        own.clone()
            .or_else(|| self.working_directory.clone())
            .unwrap_or(match kind {
                NewTerminal::Window => WorkingDir::Home,
                _ => WorkingDir::Inherit,
            })
    }
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

/// How a terminal pane moves: Otty's `terminal-scroll-*`,
/// `output-scroll-animation`, `cursor-animation`,
/// `terminal-scrollbar-hide-on-auto-scroll` and `dedup-repaint-scrollback`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    /// `terminal-scroll-smooth`: pixel-granular scrolling that snaps to a
    /// row when the gesture ends.
    pub smooth_scroll: bool,
    /// `output-scroll-animation`: how the view follows output.
    pub output_animation: OutputAnimation,
    /// `cursor-animation`
    pub cursor_animation: CursorAnimation,
    /// `terminal-scroll-past-end`
    pub past_end: ScrollPastEnd,
    /// `terminal-scroll-past-first-line`
    pub past_first_line: ScrollPastFirst,
    /// `terminal-scroll-past-end-sticky`: new output leaves the past-end
    /// offset alone instead of using it up.
    pub past_end_sticky: bool,
    /// `terminal-scrollbar-hide-on-auto-scroll`: the scrollbar shows only
    /// while the view is parked away from the live output.
    pub hide_scrollbar_following: bool,
    /// `dedup-repaint-scrollback`
    pub dedup_repaints: bool,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            smooth_scroll: true,
            output_animation: OutputAnimation::Auto,
            cursor_animation: CursorAnimation::Off,
            past_end: ScrollPastEnd::Disabled,
            past_first_line: ScrollPastFirst::Disabled,
            past_end_sticky: false,
            hide_scrollbar_following: false,
            dedup_repaints: true,
        }
    }
}

impl Motion {
    /// Whether the view slides when the terminal scrolls itself.
    pub fn slides_output(&self) -> bool {
        match self.output_animation {
            OutputAnimation::Auto => self.smooth_scroll,
            OutputAnimation::Off => false,
            OutputAnimation::Smooth => true,
        }
    }
}

/// `output-scroll-animation`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputAnimation {
    /// Follows `terminal-scroll-smooth`.
    #[default]
    Auto,
    Off,
    Smooth,
}

/// `cursor-animation`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorAnimation {
    #[default]
    Off,
    Smooth,
}

/// `terminal-scroll-past-end`: where scrolling down stops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollPastEnd {
    /// At the bottom of the buffer.
    #[default]
    Disabled,
    /// The last row with text lands at the top.
    LastLineWithContent,
    /// …in the middle.
    LastLineInMiddle,
    /// The cursor's row lands at the top.
    CursorLine,
}

/// `terminal-scroll-past-first-line`: where scrolling up stops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollPastFirst {
    /// At the top of scrollback.
    #[default]
    Disabled,
    /// The mirror of `terminal-scroll-past-end`.
    SameAsLastLine,
    /// The first row of history lands at the bottom.
    FirstLineWithContent,
    /// …in the middle.
    FirstLineInMiddle,
}

impl OutputAnimation {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "auto" => Self::Auto,
            "off" => Self::Off,
            "smooth" => Self::Smooth,
            _ => return None,
        })
    }
}

impl CursorAnimation {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "off" => Self::Off,
            "smooth" => Self::Smooth,
            _ => return None,
        })
    }
}

impl ScrollPastEnd {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "disabled" => Self::Disabled,
            "last-line-with-content" => Self::LastLineWithContent,
            "last-line-in-middle" => Self::LastLineInMiddle,
            "cursor-line" => Self::CursorLine,
            _ => return None,
        })
    }
}

impl ScrollPastFirst {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "disabled" => Self::Disabled,
            "same-as-last-line" => Self::SameAsLastLine,
            "first-line-with-content" => Self::FirstLineWithContent,
            "first-line-in-middle" => Self::FirstLineInMiddle,
            _ => return None,
        })
    }

    /// What it amounts to, `SameAsLastLine` read through `past_end` (the
    /// cursor line has no mirror; its nearest is the first line at the
    /// bottom).
    pub fn resolve(self, past_end: ScrollPastEnd) -> Self {
        match (self, past_end) {
            (Self::SameAsLastLine, ScrollPastEnd::Disabled) => Self::Disabled,
            (Self::SameAsLastLine, ScrollPastEnd::LastLineInMiddle) => Self::FirstLineInMiddle,
            (Self::SameAsLastLine, _) => Self::FirstLineWithContent,
            (other, _) => other,
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
        Self::load_with(&[])
    }

    /// [`Config::load`], with `overrides` (`motty config set --transient`)
    /// in place of the files' values for the same keys.
    pub fn load_with(overrides: &[(String, toml::Value)]) -> Self {
        Files::current().load(overrides)
    }

    /// Defaults follow Otty's: ligatures on, Option is not Alt, no blinking.
    pub fn parse(src: &str) -> Self {
        Self::from_entries(parse_entries(src))
    }

    pub(crate) fn from_entries(
        (entries, mut warnings): (Vec<(String, toml::Value)>, Vec<String>),
    ) -> Self {
        let mut out = Config {
            ligatures: true,
            sidebar_visible: true,
            shell_integration: true,
            link_detection: true,
            restore_session: true,
            restore_banner: true,
            editor: EditorPrefs::default(),
            clipboard_write: Access::Allow,
            ..Default::default()
        };
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
            // A bare word where a number goes (`font-size = big`).
            if value.is_str()
                && matches!(
                    key.as_str(),
                    "font-size"
                        | "sidebar-width"
                        | "unfocused-split-opacity"
                        | "details-panel-width"
                )
            {
                unknown.push(format!("config.toml {key}: expected a number, got {value}"));
                continue;
            }
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
                "details-view" => match value.as_str().map(DetailsView::parse) {
                    Some(Ok(view)) => out.details_views.push(view),
                    Some(Err(e)) => unknown.push(format!("config.toml details-view: {e}")),
                    None => unknown.push(format!("config.toml details-view: can't read {value}")),
                },
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
                "command" | "shell" => out.shell = str_of(value),
                "term" => out.term = str_of(value).filter(|t| t != "auto"),
                "env" => match value.as_str().and_then(|v| v.split_once('=')) {
                    Some((name, v)) if !name.trim().is_empty() => {
                        out.env.push((name.trim().to_owned(), v.to_owned()))
                    }
                    _ => unknown.push(format!("config.toml env: expected NAME=value, got {value}")),
                },
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
                "cursor-animation" => match value.as_str().and_then(CursorAnimation::parse) {
                    Some(a) => out.motion.cursor_animation = a,
                    None => unknown.push(format!("config.toml {key}: unknown value {value}")),
                },
                "terminal-scroll-smooth" => {
                    out.motion.smooth_scroll = value.as_bool().unwrap_or(true)
                }
                "output-scroll-animation" => {
                    match value.as_str().and_then(OutputAnimation::parse) {
                        Some(a) => out.motion.output_animation = a,
                        None => unknown.push(format!("config.toml {key}: unknown value {value}")),
                    }
                }
                "terminal-scroll-past-end" => match value.as_str().and_then(ScrollPastEnd::parse) {
                    Some(p) => out.motion.past_end = p,
                    None => unknown.push(format!("config.toml {key}: unknown value {value}")),
                },
                "terminal-scroll-past-first-line" => {
                    match value.as_str().and_then(ScrollPastFirst::parse) {
                        Some(p) => out.motion.past_first_line = p,
                        None => unknown.push(format!("config.toml {key}: unknown value {value}")),
                    }
                }
                "terminal-scroll-past-end-sticky" => {
                    out.motion.past_end_sticky = value.as_bool().unwrap_or(false)
                }
                "terminal-scrollbar-hide-on-auto-scroll" => {
                    out.motion.hide_scrollbar_following = value.as_bool().unwrap_or(false)
                }
                "dedup-repaint-scrollback" => {
                    out.motion.dedup_repaints = value.as_bool().unwrap_or(true)
                }
                "cursor-color" => out.cursor_color = color_of(key, value),
                "cursor-text" => out.cursor_text = color_of(key, value),
                "privilege-badge-exit-status"
                | "privilege-badge-agent-processing"
                | "privilege-badge-agent-task-complete"
                | "privilege-badge-agent-awaiting-input"
                | "privilege-notify-agent-task-complete"
                | "privilege-notify-agent-awaiting-input"
                | "privilege-caffeinate-agent-processing"
                | "privilege-resume-agent-session"
                | "privilege-notification-on-finish"
                | "privilege-notification-on-error"
                | "privilege-notification-on-watch-finish"
                | "privilege-notification-shell"
                | "privilege-sound-on-error"
                | "privilege-sound-shell"
                | "privilege-title-shell"
                | "privilege-clipboard-shell"
                | "privilege-mouse-shell"
                | "ipc-allow-send-keys"
                | "ipc-allow-sensitive-sessions" => match value.as_bool() {
                    Some(on) => {
                        let b = &mut out.badges;
                        *match key {
                            "privilege-badge-exit-status" => &mut b.exit_status,
                            "privilege-badge-agent-processing" => &mut b.agent_processing,
                            "privilege-badge-agent-task-complete" => &mut b.agent_complete,
                            "privilege-badge-agent-awaiting-input" => &mut b.agent_awaiting,
                            "privilege-notify-agent-task-complete" => &mut b.notify_complete,
                            "privilege-notify-agent-awaiting-input" => &mut b.notify_awaiting,
                            "privilege-resume-agent-session" => &mut b.resume_session,
                            "privilege-notification-on-finish" => &mut b.notify_finish,
                            "privilege-notification-on-error" => &mut b.notify_error,
                            "privilege-notification-on-watch-finish" => &mut b.notify_watch,
                            "privilege-notification-shell" => &mut b.notify_shell,
                            "privilege-sound-on-error" => &mut b.sound_on_error,
                            "privilege-sound-shell" => &mut b.sound_shell,
                            "privilege-title-shell" => &mut b.title_shell,
                            "privilege-clipboard-shell" => &mut b.clipboard_shell,
                            "privilege-mouse-shell" => &mut b.mouse_shell,
                            "ipc-allow-send-keys" => &mut b.ipc_send_keys,
                            "ipc-allow-sensitive-sessions" => &mut b.ipc_sensitive,
                            _ => &mut b.caffeinate,
                        } = on;
                    }
                    None => unknown.push(format!("config.toml {key}: expected true or false")),
                },
                "copy-on-select"
                | "clipboard-trim-trailing-spaces"
                | "clipboard-paste-protection"
                | "clipboard-paste-bracketed-safe"
                | "selection-clear-on-typing"
                | "selection-clear-on-copy"
                | "shift-arrow-select"
                | "mouse-hide-while-typing"
                | "focus-follows-mouse"
                | "auto-secure-input"
                | "secure-input-indication"
                | "dock-icon-animate-progress"
                | "dock-icon-error-badge" => match value.as_bool() {
                    Some(on) => {
                        let b = &mut out.behavior;
                        *match key {
                            "copy-on-select" => &mut b.copy_on_select,
                            "clipboard-trim-trailing-spaces" => &mut b.trim_trailing_spaces,
                            "clipboard-paste-protection" => &mut b.paste_protection,
                            "clipboard-paste-bracketed-safe" => &mut b.paste_bracketed_safe,
                            "selection-clear-on-typing" => &mut b.clear_selection_on_typing,
                            "selection-clear-on-copy" => &mut b.clear_selection_on_copy,
                            "shift-arrow-select" => &mut b.shift_arrow_select,
                            "mouse-hide-while-typing" => &mut b.hide_mouse_while_typing,
                            "focus-follows-mouse" => &mut b.focus_follows_mouse,
                            "auto-secure-input" => &mut b.auto_secure_input,
                            "secure-input-indication" => &mut b.secure_input_indication,
                            "dock-icon-animate-progress" => &mut b.dock_progress,
                            _ => &mut b.dock_error_badge,
                        } = on;
                    }
                    None => unknown.push(format!("config.toml {key}: expected true or false")),
                },
                // A list or a comma-separated string; empty turns it off.
                "progress-bar-commands" => {
                    let list: Vec<String> = match value.as_array() {
                        Some(list) => list
                            .iter()
                            .filter_map(|v| v.as_str().map(|s| s.trim().to_owned()))
                            .collect(),
                        None => value
                            .as_str()
                            .unwrap_or_default()
                            .split(',')
                            .map(|c| c.trim().to_owned())
                            .collect(),
                    };
                    out.progress_bar_commands =
                        Some(list.into_iter().filter(|c| !c.is_empty()).collect());
                }
                "window-hide-dirty-indicator" => {
                    out.hide_dirty_indicator = value.as_bool().unwrap_or(false)
                }
                "session-restore-banner" => out.restore_banner = value.as_bool().unwrap_or(true),
                "confirm-close-ignore-idle-agents" => {
                    out.close_ignores_idle_agents = value.as_bool().unwrap_or(false)
                }
                "editor-show-line-numbers" => {
                    out.editor.line_numbers = value.as_bool().unwrap_or(true)
                }
                "editor-line-wrap" => out.editor.line_wrap = value.as_bool().unwrap_or(true),
                "editor-visible-whitespace" => {
                    out.editor.whitespace = value.as_bool().unwrap_or(false)
                }
                "editor-tab-size" => {
                    out.editor.tab_size = value.as_integer().map_or(4, |n| n.clamp(1, 16) as usize)
                }
                "editor-scroll-past-end" => {
                    out.editor.scroll_past_end = value.as_bool().unwrap_or(true)
                }
                "editor-default-to-preview-readonly" => {
                    out.editor.preview_readonly = value.as_bool().unwrap_or(true)
                }
                "quit-after-last-window-closed" => {
                    out.quit_after_last_window = value.as_bool().unwrap_or(false)
                }
                "working-directory" => {
                    out.working_directory = value.as_str().map(WorkingDir::parse)
                }
                "window-working-directory" => {
                    out.window_working_directory = value.as_str().map(WorkingDir::parse)
                }
                "tab-working-directory" => {
                    out.tab_working_directory = value.as_str().map(WorkingDir::parse)
                }
                "split-working-directory" => {
                    out.split_working_directory = value.as_str().map(WorkingDir::parse)
                }
                "new-tab-position" => match value.as_str() {
                    Some("auto") => out.new_tab_position = NewTabPosition::Auto,
                    Some("end") => out.new_tab_position = NewTabPosition::End,
                    Some("after-current") => out.new_tab_position = NewTabPosition::AfterCurrent,
                    _ => unknown.push(format!("config.toml {key}: unknown value {value}")),
                },
                "recipe-replay-saved" | "recipe-replay-file" => {
                    match value.as_str().and_then(Replay::parse) {
                        Some(r) if key == "recipe-replay-saved" => {
                            out.recipe_replay_saved = Some(r)
                        }
                        Some(r) => out.recipe_replay_file = Some(r),
                        None => unknown.push(format!("config.toml {key}: unknown value {value}")),
                    }
                }
                "auto-hide-tabs-panel" | "auto-hide-tab-bar" | "window-show-tab-bar" => {
                    match value.as_str().and_then(AutoHide::parse) {
                        Some(a) if key == "auto-hide-tabs-panel" => out.auto_hide_tabs_panel = a,
                        Some(a) => out.auto_hide_tab_bar = a,
                        None => unknown.push(format!("config.toml {key}: unknown value {value}")),
                    }
                }
                "window-layout" => match value.as_str().and_then(WindowLayout::parse) {
                    Some(l) => out.window_layout = l,
                    None => unknown.push(format!("config.toml {key}: unknown value {value}")),
                },
                "tab-rule" => match value.as_str().map(TabRule::parse) {
                    Some(Ok(rule)) => out.tab_rules.push(rule),
                    Some(Err(e)) => unknown.push(format!("config.toml tab-rule: {e}")),
                    None => unknown.push(format!("config.toml tab-rule: can't read {value}")),
                },
                "tab-icon-placement" => match value.as_str() {
                    Some("combined") => out.tab_icon_placement = IconPlacement::Combined,
                    Some("separate") => out.tab_icon_placement = IconPlacement::Separate,
                    _ => unknown.push(format!("config.toml {key}: unknown value {value}")),
                },
                "title-from-osc" => out.title_from_osc = value.as_bool(),
                "notification-foreground" => match value.as_str() {
                    Some("off") => out.notification_foreground = NotificationForeground::Off,
                    Some("always") => out.notification_foreground = NotificationForeground::Always,
                    Some("tab-unfocused") => {
                        out.notification_foreground = NotificationForeground::TabUnfocused
                    }
                    _ => unknown.push(format!("config.toml {key}: unknown value {value}")),
                },
                "clipboard-read" | "clipboard-write" => {
                    let access = match value.as_str() {
                        Some("ask") => Access::Ask,
                        Some("allow") => Access::Allow,
                        Some("deny") => Access::Deny,
                        _ => {
                            unknown.push(format!("config.toml {key}: unknown value {value}"));
                            continue;
                        }
                    };
                    if key == "clipboard-read" {
                        out.clipboard_read = access;
                    } else {
                        out.clipboard_write = access;
                    }
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
                "agent-command" => match value.as_str().and_then(AgentCommand::parse) {
                    Some(command) => out.agent_commands.push(command),
                    None => unknown.push(format!("config.toml agent-command: can't read {value}")),
                },
                "agent-quota-off" => {
                    let id = value.as_str().map(str::trim).unwrap_or_default();
                    if !id.is_empty() && !out.agent_quota_off.iter().any(|a| a == id) {
                        out.agent_quota_off.push(id.to_owned());
                    }
                }
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
                "kitty-keyboard" => out.vt.kitty_keyboard = value.as_bool().unwrap_or(true),
                "vt-keypad-app-allowed" => {
                    out.vt.keypad_app_allowed = value.as_bool().unwrap_or(true)
                }
                "vt-kam-allowed" => out.vt.kam_allowed = value.as_bool().unwrap_or(true),
                "enquiry-response" => out.vt.enquiry_response = str_of(value).unwrap_or_default(),
                "osc-color-report-format" => match value.as_str() {
                    Some("none") => out.vt.color_report = ColorReport::None,
                    Some("8-bit") => out.vt.color_report = ColorReport::Bits8,
                    Some("16-bit") => out.vt.color_report = ColorReport::Bits16,
                    _ => unknown.push(format!("config.toml {key}: unknown value {value}")),
                },
                "title-report" => out.vt.title_report = value.as_bool().unwrap_or(false),
                "terminal-identity" => match str_of(value) {
                    Some(name) if !name.trim().is_empty() => out.vt.identity = name,
                    _ => out.vt.identity = VtPrefs::default().identity,
                },
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
/// Keys come out under their real names (`shell` reads as `command`).
pub(crate) fn parse_entries(src: &str) -> (Vec<(String, toml::Value)>, Vec<String>) {
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
            Ok(table) => entries.extend(
                table
                    .into_iter()
                    .map(|(k, v)| (schema::canonical(&k).to_owned(), expand(v))),
            ),
            // Otty's reader is lenient: `theme = Nord`, `ligatures = on`.
            Err(e) => match lenient(&stmt) {
                Some(entry) => entries.push(entry),
                None => warnings.push(format!("config.toml line {}: {}", n + 1, e.message())),
            },
        }
    }
    (entries, warnings)
}

/// The `(key, value)` entries of config lines, keys under their real
/// names, with what couldn't be read.
pub fn parse_lines(src: &str) -> (Vec<(String, toml::Value)>, Vec<String>) {
    parse_entries(src)
}

/// `key = bare value` that isn't TOML: `on` / `off` as booleans, anything
/// else as the string up to a ` #` comment.
fn lenient(stmt: &str) -> Option<(String, toml::Value)> {
    let (key, value) = stmt.split_once('=')?;
    let key = key.trim();
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let value = value.trim_start().split(" #").next()?.trim();
    if value.is_empty() || value.contains('\n') {
        return None;
    }
    let value = match value {
        "on" => toml::Value::Boolean(true),
        "off" => toml::Value::Boolean(false),
        _ => expand(toml::Value::String(value.to_owned())),
    };
    Some((schema::canonical(key).to_owned(), value))
}

/// `~` and `$VAR` (or `${VAR}`) expanded in a string value, and in the
/// strings of an array.
fn expand(value: toml::Value) -> toml::Value {
    match value {
        toml::Value::String(s) => toml::Value::String(expand_str(&s)),
        toml::Value::Array(items) => toml::Value::Array(items.into_iter().map(expand).collect()),
        other => other,
    }
}

fn expand_str(s: &str) -> String {
    let mut s = s.to_owned();
    if s == "~" || s.starts_with("~/") {
        let home = dirs::home_dir().unwrap_or_default();
        s = format!("{}{}", home.display(), &s[1..]);
    }
    if !s.contains('$') {
        return s;
    }
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let (name, used) = match after.strip_prefix('{') {
            Some(inner) => match inner.find('}') {
                Some(end) => (&inner[..end], end + 2),
                None => ("", 0),
            },
            None => {
                let end = after
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(after.len());
                (&after[..end], end)
            }
        };
        match std::env::var(name).ok().filter(|_| !name.is_empty()) {
            Some(v) => out.push_str(&v),
            // Not a variable that's set: left as written.
            None => out.push_str(&rest[i..i + 1 + used]),
        }
        rest = &after[used..];
    }
    out.push_str(rest);
    out
}

/// The entries of `main` with its `include = path` files read first (in
/// order, so later ones win and `main` wins over all), then `local`
/// (config.local.toml): a key it sets, or resets with a `# key = …
/// (reset to default)` line, comes only from it, lists included.
pub(crate) fn layered(
    main: &str,
    local: &str,
    dir: &Path,
    read: &dyn Fn(&Path) -> Option<String>,
) -> (Vec<(String, toml::Value)>, Vec<String>) {
    let (main_entries, mut warnings) = parse_entries(main);
    let mut entries = Vec::new();
    let mut body = Vec::new();
    for (key, value) in main_entries {
        if key != "include" {
            body.push((key, value));
            continue;
        }
        let Some(path) = value.as_str() else {
            warnings.push("config.toml include: expected a path".into());
            continue;
        };
        let path = dir.join(path);
        match read(&path) {
            Some(src) => {
                let (included, more) = parse_entries(&src);
                // One level: an included file's own includes are ignored.
                entries.extend(included.into_iter().filter(|(k, _)| k != "include"));
                warnings.extend(more);
            }
            None => warnings.push(format!(
                "config.toml include: can't read {}",
                path.display()
            )),
        }
    }
    entries.extend(body);
    let (local_entries, more) = parse_entries(local);
    warnings.extend(more);
    let mut local_keys: std::collections::HashSet<String> =
        local_entries.iter().map(|(k, _)| k.clone()).collect();
    local_keys.extend(local.lines().filter_map(|l| {
        let rest = l.trim().strip_prefix('#')?.trim();
        rest.ends_with("(reset to default)")
            .then(|| {
                rest.split_once('=')
                    .map(|(k, _)| schema::canonical(k.trim()).to_owned())
            })
            .flatten()
    }));
    entries.retain(|(k, _)| !local_keys.contains(k));
    entries.extend(local_entries.into_iter().filter(|(k, _)| k != "include"));
    (entries, warnings)
}

/// `src` with `key = value` set where its last line (or an alias's) was,
/// else where it was reset, else appended (see [`crate::store::set_in`]).
pub fn set_in(src: &str, key: &str, value: &toml::Value) -> String {
    crate::store::set_in(src, key, value)
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
    edit_key("keybind", |src| {
        set_keybinds_in(src, action, defaults, chords)
    })
}

/// `src` with every `keybind` line removed: all shortcuts back to default.
pub fn reset_keybinds_in(src: &str) -> String {
    remove_in(src, "keybind")
}

/// Rewrites Motty's config.toml through `f`, starting from Otty's (or a
/// header) when Motty has none yet.
pub fn edit(f: impl FnOnce(&str) -> String) -> std::io::Result<PathBuf> {
    Files::current().edit_main(f)
}

/// Rewrites the file `key` is kept in: config.local.toml when its value
/// comes from there, else config.toml.
pub fn edit_key(key: &str, f: impl FnOnce(&str) -> String) -> std::io::Result<PathBuf> {
    Files::current().edit_key(key, f)
}

/// Writes `pairs` into Motty's own config.toml. The first time, an Otty
/// config being read is copied over first, so nothing it set is lost.
pub fn save_values(pairs: &[(&str, toml::Value)]) -> std::io::Result<PathBuf> {
    let changes: Vec<(&str, Option<toml::Value>)> =
        pairs.iter().map(|(k, v)| (*k, Some(v.clone()))).collect();
    save(&changes)
}

/// Replaces every `tab-rule` line with `rules`, in order, where the first
/// one was (or at the end).
pub fn save_tab_rules(rules: &[TabRule]) -> std::io::Result<PathBuf> {
    edit_key("tab-rule", |src| replace_tab_rules(src, rules))
}

fn replace_tab_rules(src: &str, rules: &[TabRule]) -> String {
    let lines: String = rules
        .iter()
        .map(|r| {
            let line = r.to_line();
            // A TOML literal string can't hold a `'`; fall back to a basic one.
            if line.contains('\'') {
                format!("tab-rule = {}\n", toml::Value::String(line))
            } else {
                format!("tab-rule = '{line}'\n")
            }
        })
        .collect();
    replace_repeated(src, "tab-rule", &lines)
}

/// Replaces every `agent-quota-off` line with one per agent in `agents`.
pub fn save_quota_off(agents: &[String]) -> std::io::Result<PathBuf> {
    edit_key("agent-quota-off", |src| replace_quota_off(src, agents))
}

fn replace_quota_off(src: &str, agents: &[String]) -> String {
    let lines: String = agents
        .iter()
        .map(|a| format!("agent-quota-off = {}\n", toml::Value::String(a.clone())))
        .collect();
    replace_repeated(src, "agent-quota-off", &lines)
}

/// Puts `lines` where the first `key` line was (or at the end), dropping
/// the other `key` lines.
fn replace_repeated(src: &str, key: &str, lines: &str) -> String {
    let mut out = String::new();
    let mut placed = false;
    for line in src.lines() {
        let this = line
            .trim_start()
            .split(['=', ' '])
            .next()
            .unwrap_or_default();
        if this == key {
            if !placed {
                out.push_str(lines);
                placed = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !placed {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(lines);
    }
    out
}

/// Sets `agent`'s `agent-command` in Motty's config.toml; an empty
/// `command` goes back to the built-in one.
pub fn save_agent_command(agent: &str, command: &str) -> std::io::Result<PathBuf> {
    edit_key("agent-command", |src| {
        set_agent_command_in(src, agent, command)
    })
}

/// `src` with `agent`'s `agent-command` lines replaced by one for `command`,
/// where the first one was (or at the end), or with none when it's empty.
fn set_agent_command_in(src: &str, agent: &str, command: &str) -> String {
    let command = command.trim();
    let line = (!command.is_empty()).then(|| {
        let value = toml::Value::String(format!("{agent}={command}"));
        format!("agent-command = {value}")
    });
    let names_agent = |l: &str| {
        let trimmed = l.trim_start();
        trimmed.split(['=', ' ']).next() == Some("agent-command")
            && parse_entries(trimmed).0.iter().any(|(k, v)| {
                k == "agent-command"
                    && v.as_str()
                        .and_then(AgentCommand::parse)
                        .is_some_and(|c| c.agent == agent)
            })
    };
    let mut lines: Vec<String> = Vec::new();
    let mut placed = false;
    for l in src.lines() {
        if names_agent(l) {
            if !placed {
                lines.extend(line.clone());
                placed = true;
            }
            continue;
        }
        lines.push(l.to_owned());
    }
    if !placed && let Some(line) = line {
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        lines.push(line);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Sets each key, or resets it for `None` (commented out as `# key = …
/// (reset to default)`), in the file its value comes from.
pub fn save(changes: &[(&str, Option<toml::Value>)]) -> std::io::Result<PathBuf> {
    Files::current().save(changes)
}

const MOTTY_DARK: &str = include_str!("../themes/motty-dark.ottytheme");
const MOTTY_LIGHT: &str = include_str!("../themes/motty-light.ottytheme");

/// Every theme shipped in the binary: Motty's own, then popular schemes
/// written from their published (MIT) palettes. A theme file of the same
/// name in a themes folder takes its place.
pub const BUILTIN_THEMES: &[&str] = &[
    MOTTY_DARK,
    MOTTY_LIGHT,
    include_str!("../themes/ayu-dark.ottytheme"),
    include_str!("../themes/ayu-light.ottytheme"),
    include_str!("../themes/catppuccin-latte.ottytheme"),
    include_str!("../themes/catppuccin-mocha.ottytheme"),
    include_str!("../themes/dracula.ottytheme"),
    include_str!("../themes/github-dark.ottytheme"),
    include_str!("../themes/github-light.ottytheme"),
    include_str!("../themes/gruvbox-dark.ottytheme"),
    include_str!("../themes/gruvbox-light.ottytheme"),
    include_str!("../themes/nord.ottytheme"),
    include_str!("../themes/one-dark.ottytheme"),
    include_str!("../themes/one-light.ottytheme"),
    include_str!("../themes/rose-pine.ottytheme"),
    include_str!("../themes/solarized-dark.ottytheme"),
    include_str!("../themes/solarized-light.ottytheme"),
    include_str!("../themes/tokyo-night.ottytheme"),
];

pub fn builtin(mode: Mode) -> ParsedTheme {
    theme::parse(if mode == Mode::Dark {
        MOTTY_DARK
    } else {
        MOTTY_LIGHT
    })
    .expect("built-in theme parses")
}

/// A built-in theme's file text, by name.
pub fn builtin_source(name: &str) -> Option<&'static str> {
    let wanted = normalize(name);
    BUILTIN_THEMES
        .iter()
        .copied()
        .find(|src| theme::parse(src).is_ok_and(|p| normalize(&p.theme.meta.name) == wanted))
}

/// Where Motty keeps themes it writes (imports, edits).
pub fn user_themes_dir() -> PathBuf {
    motty_dir().join("themes")
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
    let files = Files::current();
    stamp(&files.main);
    stamp(&files.local);
    if let Some((path, _)) = files.source() {
        stamp(&path);
        stamp(&path.with_file_name("config.local.toml"));
    }
    // Saving an included file reloads too.
    for path in files.includes() {
        stamp(&path);
    }
    // So does a font dropped into the fonts folder.
    for dir in [motty_dir(), otty_dir()] {
        let Ok(entries) = std::fs::read_dir(dir.join("fonts")) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            stamp(&path);
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
        for path in paths {
            stamp(&path);
        }
    }
    hasher.finish()
}

/// Finds a theme by `[meta] name` or file stem, case-insensitively.
pub fn find_theme(name: &str) -> Option<ParsedTheme> {
    let wanted = normalize(name);
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
    BUILTIN_THEMES
        .iter()
        .filter_map(|src| theme::parse(src).ok())
        .find(|p| normalize(&p.theme.meta.name) == wanted)
}

pub fn list_themes() -> Vec<(String, Mode, Option<PathBuf>)> {
    let mut out: Vec<(String, Mode, Option<PathBuf>)> = Vec::new();
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
    for p in BUILTIN_THEMES.iter().filter_map(|s| theme::parse(s).ok()) {
        if !out
            .iter()
            .any(|(n, ..)| normalize(n) == normalize(&p.theme.meta.name))
        {
            out.push((p.theme.meta.name, p.theme.meta.mode, None));
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
        assert!(d.behavior.paste_protection && !d.behavior.copy_on_select);
        let cfg = Config::parse(
            "copy-on-select = true\nauto-secure-input = false\nprogress-bar-commands = [\"make\", \"git push\"]\n",
        );
        assert!(cfg.behavior.copy_on_select && !cfg.behavior.auto_secure_input);
        assert_eq!(cfg.progress_commands(), ["make", "git push"]);
        assert!(
            Config::parse("progress-bar-commands = \"\"\n")
                .progress_commands()
                .is_empty()
        );
        assert!(matches_prefix("git push origin main", "git push"));
        assert!(!matches_prefix("git status", "git push"));
        assert!(!matches_prefix("curlie x", "curl"));
        assert!(!matches_prefix("ls", ""));
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
    fn notification_and_privilege_keys() {
        let d = Config::parse("");
        assert!(!d.badges.notify_finish && d.badges.notify_error && d.badges.sound_shell);
        assert_eq!(d.clipboard_read, Access::Ask);
        assert_eq!(d.clipboard_write, Access::Allow);
        assert_eq!(d.notification_foreground, NotificationForeground::Off);
        let cfg = Config::parse(
            "privilege-notification-on-finish = true\nprivilege-sound-shell = false\nprivilege-mouse-shell = false\nclipboard-read = \"deny\"\nclipboard-write = \"ask\"\nnotification-foreground = \"tab-unfocused\"\n",
        );
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        assert!(cfg.badges.notify_finish && !cfg.badges.sound_shell && !cfg.badges.mouse_shell);
        assert_eq!(cfg.clipboard_read, Access::Deny);
        assert_eq!(cfg.clipboard_write, Access::Ask);
        assert_eq!(
            cfg.notification_foreground,
            NotificationForeground::TabUnfocused
        );
        assert_eq!(
            Config::parse("clipboard-read = \"sometimes\"\n")
                .warnings
                .len(),
            1
        );
    }

    #[test]
    fn agent_commands() {
        let cfg = Config::parse(
            "agent-command = claude=claude --dangerously-skip-permissions\n\
             agent-command = codex=/opt/homebrew/bin/codex --profile work # mine\n\
             agent-command = \"kimi=kimi --yolo\"\n\
             agent-command = kimi=\n\
             agent-command = no command\n",
        );
        assert_eq!(
            cfg.warnings,
            ["config.toml agent-command: can't read \"no command\""]
        );
        assert_eq!(
            cfg.agent_command("claude"),
            Some("claude --dangerously-skip-permissions")
        );
        assert_eq!(
            cfg.agent_command("codex"),
            Some("/opt/homebrew/bin/codex --profile work")
        );
        // A later empty line puts the built-in command back.
        assert_eq!(cfg.agent_command("kimi"), None);
        assert_eq!(cfg.agent_command("pi"), None);
        assert_eq!(
            AgentCommand::parse(" omp = omp --x=1 "),
            Some(AgentCommand {
                agent: "omp".into(),
                command: "omp --x=1".into()
            })
        );
        assert_eq!(AgentCommand::parse("=claude"), None);
    }

    #[test]
    fn agent_commands_are_written_back() {
        let src = "theme = \"Nord\"\nagent-command = claude=claude\nfont-size = 13\nagent-command = codex=codex -p\nagent-command = \"claude=x\"\n\n";
        let out = set_agent_command_in(src, "claude", "my-wrapper claude --flag \"a b\"");
        assert_eq!(
            out,
            "theme = \"Nord\"\nagent-command = 'claude=my-wrapper claude --flag \"a b\"'\nfont-size = 13\nagent-command = codex=codex -p\n\n"
        );
        let cfg = Config::parse(&out);
        assert_eq!(
            cfg.agent_command("claude"),
            Some("my-wrapper claude --flag \"a b\"")
        );
        assert_eq!(cfg.agent_command("codex"), Some("codex -p"));
        let out = set_agent_command_in(&out, "kimi", "kimi --yolo");
        assert!(out.ends_with("agent-command = \"kimi=kimi --yolo\"\n"));
        let out = set_agent_command_in(&out, "claude", "  ");
        let cfg = Config::parse(&out);
        assert_eq!(cfg.agent_command("claude"), None);
        assert_eq!(cfg.agent_command("kimi"), Some("kimi --yolo"));
        assert_eq!(cfg.agent_commands.len(), 2);
    }

    #[test]
    fn tab_rules_round_trip() {
        let src = "theme = \"Nord\"\ntab-rule = 'path=\"~/a\" title=\"A\"'\nfont-size = 13\ntab-rule = 'icon=\"x\"'\n";
        let cfg = Config::parse(src);
        assert_eq!(cfg.tab_rules.len(), 2);
        let mut rules = cfg.tab_rules.clone();
        rules.reverse();
        rules[0].title = Some("it's ${folder}".into());
        let out = replace_tab_rules(src, &rules);
        assert_eq!(out.matches("tab-rule").count(), 2);
        assert!(out.starts_with("theme = \"Nord\"\ntab-rule"));
        assert!(out.contains("font-size = 13"));
        assert_eq!(Config::parse(&out).tab_rules, rules);
        let added = replace_tab_rules("theme = \"Nord\"\n", &rules[..1]);
        assert_eq!(Config::parse(&added).tab_rules, rules[..1]);
        assert_eq!(replace_tab_rules(src, &[]).matches("tab-rule").count(), 0);
    }

    #[test]
    fn quota_off_round_trips() {
        let src = "theme = \"Nord\"\nagent-quota-off = \"codex\"\nfont-size = 13\nagent-quota-off = codex\n";
        let cfg = Config::parse(src);
        assert_eq!(cfg.agent_quota_off, ["codex"]);
        assert!(cfg.shows_quota("claude") && !cfg.shows_quota("codex"));
        let off = ["codex".to_owned(), "claude".to_owned()];
        let out = replace_quota_off(src, &off);
        assert_eq!(out.matches("agent-quota-off").count(), 2);
        assert!(out.ends_with("font-size = 13\n"));
        assert_eq!(Config::parse(&out).agent_quota_off, off);
        let none = replace_quota_off(&out, &[]);
        assert!(!none.contains("agent-quota-off"));
        assert!(Config::parse("").shows_quota("omp"));
    }

    #[test]
    fn layout_keys() {
        let cfg = Config::parse(
            "window-layout = \"tabs-bottom\"\nwindow-show-tab-bar = \"auto\"\ntab-icon-placement = \"separate\"\ntitle-from-osc = false\n",
        );
        assert_eq!(cfg.window_layout, WindowLayout::TabsBottom);
        assert_eq!(cfg.auto_hide_tab_bar, AutoHide::Auto);
        assert_eq!(cfg.auto_hide_tabs_panel, AutoHide::Default);
        assert_eq!(cfg.tab_icon_placement, IconPlacement::Separate);
        assert!(!cfg.programs_set_title());
        assert!(Config::parse("").programs_set_title());
    }

    #[test]
    fn auto_hide_tabs_panel() {
        assert_eq!(Config::parse("").auto_hide_tabs_panel, AutoHide::Default);
        let cfg = Config::parse("auto-hide-tabs-panel = \"auto\"\n");
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        assert_eq!(cfg.auto_hide_tabs_panel, AutoHide::Auto);
        assert_eq!(
            Config::parse("auto-hide-tabs-panel = \"always\"\n").auto_hide_tabs_panel,
            AutoHide::Always
        );
    }

    #[test]
    fn lenient_values_and_expansion() {
        let cfg = Config::parse(
            "theme = Nord # dark\nligatures = off\nsidebar-visible = on\ncursor-color = #ff0000\n",
        );
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        assert_eq!(cfg.theme.as_deref(), Some("Nord"));
        assert!(!cfg.ligatures && cfg.sidebar_visible);
        assert_eq!(cfg.cursor_color.map(|c| c.r), Some(1.));
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            expand_str("~/projects"),
            format!("{}/projects", home.display())
        );
        assert_eq!(expand_str("$MOTTY_SURELY_UNSET/x"), "$MOTTY_SURELY_UNSET/x");
        // SAFETY: tests in this module don't read this variable elsewhere.
        unsafe { std::env::set_var("MOTTY_TEST_EXPAND", "fish") };
        assert_eq!(
            expand_str("/opt/${MOTTY_TEST_EXPAND}/bin/$MOTTY_TEST_EXPAND"),
            "/opt/fish/bin/fish"
        );
    }

    /// The keys `from_entries` reads, from its source: quoted kebab-case
    /// words that aren't compared as values (`Some("bar")`, `v != "false"`).
    fn keys_the_parser_reads() -> std::collections::BTreeSet<String> {
        let src = include_str!("config.rs");
        let start = src.find("pub(crate) fn from_entries(").unwrap();
        let end = start + src[start..].find("\n    /// The theme name").unwrap();
        let mut keys = std::collections::BTreeSet::new();
        let mut rest = &src[start..end];
        while let Some(i) = rest.find('"') {
            let after = &rest[i + 1..];
            let Some(j) = after.find('"') else { break };
            let word = &after[..j];
            let kebab = word
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            let value = rest[..i].ends_with("Some(") || rest[..i].ends_with("!= ");
            if !value && !word.is_empty() && kebab {
                keys.insert(word.to_owned());
            }
            rest = &after[j + 1..];
        }
        keys
    }

    #[test]
    fn schema_lists_every_key_the_parser_reads_and_no_other() {
        let read = keys_the_parser_reads();
        assert!(read.contains("font-size") && read.contains("keybind"));
        for key in &read {
            let known = key == "palette-" || crate::schema::lookup(key).is_some();
            assert!(known, "the parser reads {key:?}, which the schema lacks");
        }
        for spec in crate::schema::KEYS {
            // `include` is read by `layered`, the palette by its prefix.
            let read = read.contains(spec.key)
                || spec.key == "include"
                || spec.key.starts_with("palette-");
            assert!(
                read,
                "the schema has {:?}, which the parser ignores",
                spec.key
            );
            // Every key, written with its default, reads without warnings.
            if let Some(value) = spec.default_value().filter(|_| spec.list.is_none())
                && !matches!(
                    spec.kind,
                    crate::schema::Kind::Theme | crate::schema::Kind::Color
                )
            {
                let line = crate::schema::line(spec.key, &value);
                let c = Config::parse(&line);
                assert!(c.warnings.is_empty(), "{line}: {:?}", c.warnings);
            }
        }
    }

    #[test]
    fn motion_keys() {
        let d = Config::parse("").motion;
        assert_eq!(d, Motion::default());
        assert!(d.smooth_scroll && d.dedup_repaints && d.slides_output());
        let c = Config::parse(concat!(
            "terminal-scroll-smooth = false\n",
            "cursor-animation = \"smooth\"\n",
            "terminal-scroll-past-end = \"last-line-in-middle\"\n",
            "terminal-scroll-past-first-line = \"same-as-last-line\"\n",
            "terminal-scrollbar-hide-on-auto-scroll = true\n",
            "dedup-repaint-scrollback = false\n",
        ));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        let m = c.motion;
        assert!(!m.smooth_scroll && !m.dedup_repaints && m.hide_scrollbar_following);
        assert_eq!(m.cursor_animation, CursorAnimation::Smooth);
        assert_eq!(m.past_end, ScrollPastEnd::LastLineInMiddle);
        assert_eq!(
            m.past_first_line.resolve(m.past_end),
            ScrollPastFirst::FirstLineInMiddle
        );
        // auto follows smooth scroll; off and smooth override it.
        assert!(!m.slides_output());
        let c =
            Config::parse("terminal-scroll-smooth = false\noutput-scroll-animation = \"smooth\"\n");
        assert!(c.motion.slides_output());
        let c = Config::parse("output-scroll-animation = \"off\"\n");
        assert!(c.motion.smooth_scroll && !c.motion.slides_output());
        assert!(
            !Config::parse("cursor-animation = \"bouncy\"\n")
                .warnings
                .is_empty()
        );
        assert_eq!(
            ScrollPastFirst::SameAsLastLine.resolve(ScrollPastEnd::Disabled),
            ScrollPastFirst::Disabled
        );
        assert_eq!(
            ScrollPastFirst::SameAsLastLine.resolve(ScrollPastEnd::CursorLine),
            ScrollPastFirst::FirstLineWithContent
        );
    }

    #[test]
    fn command_env_and_aliases() {
        let c = Config::parse(
            "shell = \"/bin/zsh\"\nenv = \"EDITOR=nvim\"\nenv = A=b=c\nscrollback = 7\n",
        );
        assert_eq!(c.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(
            c.env,
            vec![("EDITOR".into(), "nvim".into()), ("A".into(), "b=c".into())]
        );
        assert_eq!(c.scrollback, Some(7));
        let c = Config::parse("command = /opt/homebrew/bin/fish\n");
        assert_eq!(c.shell.as_deref(), Some("/opt/homebrew/bin/fish"));
        // An alias in config.local.toml replaces the real key from config.toml.
        let (entries, _) = layered(
            "command = \"a\"\n",
            "shell = \"b\"\n",
            Path::new("/"),
            &|_| None,
        );
        assert_eq!(
            entries,
            vec![("command".into(), toml::Value::String("b".into()))]
        );
    }

    #[test]
    fn includes_and_local_layering() {
        let read = |p: &Path| match p.file_name()?.to_str()? {
            "a.toml" => Some("theme = \"A\"\nfont-size = 11\nopen-with-app = \"X|x\"\n".into()),
            "b.toml" => Some("theme = \"B\"\n".into()),
            _ => None,
        };
        let main = "include = \"a.toml\"\ninclude = \"b.toml\"\nopen-with-app = \"Y|y\"\n";
        let cfg = Config::from_entries(layered(main, "", Path::new("/c"), &read));
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        assert_eq!(cfg.theme.as_deref(), Some("B"));
        assert_eq!(cfg.font_size, Some(11.));
        assert_eq!(cfg.open_with_apps.len(), 2);
        // A local list replaces, and a reset line takes the key back to its
        // default.
        let local = "open-with-app = \"Z|z\"\n# font-size = 14 (reset to default)\n";
        let cfg = Config::from_entries(layered(main, local, Path::new("/c"), &read));
        assert_eq!(cfg.open_with_apps.len(), 1);
        assert_eq!(cfg.font_size, None);
        let missing = Config::from_entries(layered(
            "include = \"nope.toml\"\n",
            "",
            Path::new("/c"),
            &read,
        ));
        assert_eq!(missing.warnings.len(), 1);
    }

    #[test]
    fn every_builtin_parses_cleanly() {
        for src in BUILTIN_THEMES {
            let p = theme::parse(src).unwrap();
            assert!(
                p.warnings.is_empty(),
                "{}: {:?}",
                p.theme.meta.name,
                p.warnings
            );
        }
        assert_eq!(BUILTIN_THEMES.len(), 18);
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

    #[test]
    fn working_directories() {
        let c = Config::parse("tab-working-directory = \"home\"\nworking-directory = \"/tmp\"\n");
        assert_eq!(c.working_dir(NewTerminal::Tab), WorkingDir::Home);
        assert_eq!(
            c.working_dir(NewTerminal::Split),
            WorkingDir::Path("/tmp".into())
        );
        let c = Config::parse("new-tab-position = \"after-current\"\n");
        assert_eq!(c.working_dir(NewTerminal::Window), WorkingDir::Home);
        assert_eq!(c.working_dir(NewTerminal::Tab), WorkingDir::Inherit);
        assert_eq!(c.new_tab_position, NewTabPosition::AfterCurrent);
        assert!(!c.quit_after_last_window);
    }

    #[test]
    fn vt_keys() {
        assert_eq!(Config::parse("").vt, VtPrefs::default());
        let c = Config::parse(
            "kitty-keyboard = false\nvt-keypad-app-allowed = false\nvt-kam-allowed = false\n\
             enquiry-response = \"hi\"\nosc-color-report-format = \"8-bit\"\n\
             title-report = true\nterminal-identity = \"kitty\"\n",
        );
        assert_eq!(
            c.vt,
            VtPrefs {
                kitty_keyboard: false,
                keypad_app_allowed: false,
                kam_allowed: false,
                enquiry_response: "hi".into(),
                color_report: ColorReport::Bits8,
                title_report: true,
                identity: "kitty".into(),
            }
        );
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        let c = Config::parse("osc-color-report-format = \"24-bit\"\nterminal-identity = \"\"\n");
        assert_eq!(c.vt.color_report, ColorReport::Bits16);
        assert_eq!(c.vt.identity, "motty");
        assert_eq!(c.warnings.len(), 1);
    }
}
