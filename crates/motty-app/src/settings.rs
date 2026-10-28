//! The settings window (⌘,), laid out like Otty's: a sidebar of sections
//! with a search field, and on the right groups of rows, each a setting
//! with a native pop-up button, a switch or a stepper. Every change is
//! written to config.toml straight away and applied to open windows.
//!
//! Rows are data ([`rows`]), built from the current config each frame, so
//! the sections and the search results are the same rows.

use std::borrow::Cow;

use gpui::{
    Action, AnyElement, App, AppContext, AssetSource, Bounds, ClickEvent, ClipboardItem, Context,
    FocusHandle, Focusable, Global, Hsla, KeyDownEvent, ScrollHandle, SharedString,
    TitlebarOptions, Window, WindowAppearance, WindowBounds, WindowHandle, WindowOptions, div,
    point, prelude::*, px, size, svg,
};
use motty_render::hsla;
use motty_theme::{
    AcceptKey, Config, CursorBlink, CursorStyle, LinkClick, LinkShiftClick, Mode, Rgba, RightClick,
};
use toml::Value;

use crate::actions::*;
use crate::native_menu::{self, Entry, Item};

const WIDTH: f32 = 720.;
const HEIGHT: f32 = 520.;
const SIDEBAR: f32 = 197.;
const PAD: f32 = 28.;
const ROW_MIN: f32 = 43.;

/// The icons and other files the UI draws, compiled in.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        macro_rules! icons {
            ($($name:literal),*) => {
                match path {
                    $(concat!("icons/", $name, ".svg") => {
                        Some(include_bytes!(concat!("../assets/icons/", $name, ".svg")).as_slice())
                    })*
                    _ => None,
                }
            };
        }
        Ok(icons!(
            "circle-alert",
            "terminal",
            "mouse-pointer-2",
            "file-text",
            "plug",
            "palette",
            "keyboard",
            "wrench",
            "search",
            "chevron-down"
        )
        .map(Cow::Borrowed))
    }

    fn list(&self, _: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    General,
    Shell,
    Controls,
    Editor,
    Agents,
    Appearance,
    KeyBindings,
    Advanced,
}

impl Section {
    const ALL: [Section; 8] = [
        Section::General,
        Section::Shell,
        Section::Controls,
        Section::Editor,
        Section::Agents,
        Section::Appearance,
        Section::KeyBindings,
        Section::Advanced,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Shell => "Shell",
            Section::Controls => "Controls",
            Section::Editor => "Editor",
            Section::Agents => "Agents",
            Section::Appearance => "Appearance",
            Section::KeyBindings => "Key Bindings",
            Section::Advanced => "Advanced",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Section::General => "icons/circle-alert.svg",
            Section::Shell => "icons/terminal.svg",
            Section::Controls => "icons/mouse-pointer-2.svg",
            Section::Editor => "icons/file-text.svg",
            Section::Agents => "icons/plug.svg",
            Section::Appearance => "icons/palette.svg",
            Section::KeyBindings => "icons/keyboard.svg",
            Section::Advanced => "icons/wrench.svg",
        }
    }
}

/// What a button row does.
#[derive(Clone, Copy, Debug)]
enum Command {
    ShowWelcome,
    InstallCli,
    OpenConfig,
    RevealConfig,
    ReloadConfig,
    ClearHistory,
}

enum Control {
    Toggle {
        key: &'static str,
        on: bool,
        /// The key says the opposite of the label (`…-hide-…`).
        invert: bool,
    },
    /// A pop-up button; an option without a value removes the key.
    Choice {
        key: &'static str,
        options: Vec<(SharedString, Option<Value>)>,
        current: usize,
    },
    Stepper {
        key: &'static str,
        value: f32,
        min: f32,
        max: f32,
    },
    Button(&'static str, Command),
    /// Text shown as is, and copied with a button.
    Copy(String),
    Keys(Vec<SharedString>),
    Info(SharedString),
}

struct Row {
    section: Section,
    group: &'static str,
    title: SharedString,
    description: Option<SharedString>,
    control: Control,
}

fn row(
    section: Section,
    group: &'static str,
    title: impl Into<SharedString>,
    description: Option<&'static str>,
    control: Control,
) -> Row {
    Row {
        section,
        group,
        title: title.into(),
        description: description.map(Into::into),
        control,
    }
}

fn s(v: &str) -> Option<Value> {
    Some(Value::String(v.into()))
}

/// A choice among fixed `(label, value)` pairs, `current` matched by label.
fn choice(key: &'static str, options: &[(&str, &str)], current: &str) -> Control {
    Control::Choice {
        key,
        current: options.iter().position(|(l, _)| *l == current).unwrap_or(0),
        options: options
            .iter()
            .map(|(label, value)| (SharedString::from(label.to_string()), s(value)))
            .collect(),
    }
}

/// Keeps a font or theme name that's set but not offered, so it shows.
fn names_choice(
    key: &'static str,
    default_label: String,
    names: &[String],
    current: Option<&str>,
) -> Control {
    let mut options: Vec<(SharedString, Option<Value>)> = vec![(default_label.into(), None)];
    options.extend(names.iter().map(|n| (n.clone().into(), s(n))));
    let mut ix = 0;
    if let Some(current) = current {
        ix = match names.iter().position(|n| n.eq_ignore_ascii_case(current)) {
            Some(i) => i + 1,
            None => {
                options.push((current.to_owned().into(), s(current)));
                options.len() - 1
            }
        };
    }
    Control::Choice {
        key,
        options,
        current: ix,
    }
}

pub struct SettingsWindow {
    focus: FocusHandle,
    section: Section,
    query: String,
    config: Config,
    themes: Vec<(String, Mode)>,
    fonts: Vec<String>,
    shells: Vec<String>,
    scroll: ScrollHandle,
    /// The result of the last button or save, shown under the content.
    status: Option<(bool, String)>,
    /// The line last copied.
    copied: Option<String>,
}

struct Open(WindowHandle<SettingsWindow>);
impl Global for Open {}

/// ⌘, opens the settings window, or brings it forward.
pub fn open(section: Option<Section>, cx: &mut App) {
    if let Some(Open(handle)) = cx.try_global::<Open>().map(|o| Open(o.0))
        && handle
            .update(cx, |this, window, cx| {
                if let Some(section) = section {
                    this.section = section;
                }
                window.activate_window();
                cx.notify();
            })
            .is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(16.), px(16.))),
        }),
        window_min_size: Some(size(px(600.), px(380.))),
        app_id: Some("dev.motty.Motty".into()),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        cx.new(|cx| SettingsWindow::new(section.unwrap_or(Section::General), window, cx))
    }) {
        Ok(handle) => {
            cx.set_global(Open(handle));
            let _ = handle.update(cx, |_, window, _| {
                if std::env::var_os("MOTTY_NO_ACTIVATE").is_some() {
                    crate::materials::show_inactive(window);
                } else {
                    window.activate_window();
                }
            });
        }
        Err(e) => tracing::error!("couldn't open settings: {e:#}"),
    }
}

impl SettingsWindow {
    fn new(section: Section, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Changes made elsewhere (the file, the welcome guide) show up when
        // the window comes back to the front.
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.config = crate::load_config();
                cx.notify();
            }
        })
        .detach();
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        let mut themes: Vec<(String, Mode)> = motty_theme::config::list_themes()
            .into_iter()
            .map(|(n, m, _)| (n, m))
            .collect();
        themes.sort_by_key(|(n, _)| n.to_lowercase());
        let mut fonts: Vec<String> = cx
            .text_system()
            .all_font_names()
            .into_iter()
            .filter(|n| looks_monospace(n))
            .collect();
        fonts.sort_by_key(|n| n.to_lowercase());
        fonts.dedup();
        let shells = std::fs::read_to_string("/etc/shells")
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with('/'))
            .map(str::to_owned)
            .collect();
        Self {
            focus,
            section,
            query: String::new(),
            config: crate::load_config(),
            themes,
            fonts,
            shells,
            scroll: ScrollHandle::new(),
            status: None,
            copied: None,
        }
    }

    /// Saves one key and applies it everywhere.
    fn apply(&mut self, key: &str, value: Option<Value>, cx: &mut Context<Self>) {
        match motty_theme::config::save(&[(key, value)]) {
            Ok(_) => {
                self.status = None;
                crate::workspace::reload_all(cx);
                self.config = crate::load_config();
            }
            Err(e) => self.status = Some((false, format!("Couldn't save config.toml: {e}"))),
        }
        cx.notify();
    }

    fn run(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::ShowWelcome => {
                let workspace = cx
                    .windows()
                    .into_iter()
                    .find_map(|w| w.downcast::<crate::workspace::Workspace>());
                match workspace {
                    Some(w) => {
                        let _ = w.update(cx, |ws, window, cx| {
                            window.activate_window();
                            ws.show_welcome(window, cx);
                        });
                    }
                    None => cx.dispatch_action(&NewWindow),
                }
            }
            Command::InstallCli => {
                let Some(cli) = crate::workspace::cli_path() else {
                    self.status = Some((false, "The motty command isn't in this build.".into()));
                    return cx.notify();
                };
                self.status = Some((true, "Installing…".into()));
                cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move { crate::workspace::install_cli(&cli) })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.status = Some(match result {
                            Ok(s) => (true, s),
                            Err(s) => (false, s),
                        });
                        cx.notify();
                    });
                })
                .detach();
            }
            Command::OpenConfig => {
                let path = self.config_path();
                if !path.exists() {
                    let _ = motty_theme::config::save(&[]);
                }
                if let Err(e) = std::process::Command::new("/usr/bin/open")
                    .arg("-t")
                    .arg(&path)
                    .spawn()
                {
                    self.status = Some((false, format!("Couldn't open {}: {e}", path.display())));
                }
            }
            Command::RevealConfig => cx.reveal_path(&self.config_path()),
            Command::ReloadConfig => {
                crate::workspace::reload_all(cx);
                self.config = crate::load_config();
                self.status = Some((true, "Reloaded.".into()));
            }
            Command::ClearHistory => {
                crate::history::clear();
                self.status = Some((true, "Command history cleared.".into()));
            }
        }
        let _ = window;
        cx.notify();
    }

    fn config_path(&self) -> std::path::PathBuf {
        self.config
            .path
            .clone()
            .unwrap_or_else(|| motty_theme::config::motty_dir().join("config.toml"))
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        if m.platform && ks.key == "w" {
            window.remove_window();
            return;
        }
        if m.platform || m.control {
            return;
        }
        match ks.key.as_str() {
            "escape" => self.query.clear(),
            "backspace" => {
                self.query.pop();
            }
            _ => match ks.key_char.as_deref() {
                Some(c) if !c.chars().any(char::is_control) => self.query.push_str(c),
                _ => return,
            },
        }
        self.scroll.scroll_to_item(0);
        cx.stop_propagation();
        cx.notify();
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let c = &self.config;
        let mut rows = Vec::new();
        use Section::*;

        // General
        rows.push(row(
            General,
            "General",
            "On Launch",
            None,
            choice(
                "on-launch",
                &[
                    ("New Window", "new_window"),
                    ("Restore Last Session", "restore_session"),
                ],
                if c.restore_session {
                    "Restore Last Session"
                } else {
                    "New Window"
                },
            ),
        ));
        rows.push(row(
            General,
            "General",
            "Show Tabs Panel",
            Some("New windows open with the tabs sidebar"),
            Control::Toggle {
                key: "sidebar-visible",
                on: c.sidebar_visible,
                invert: false,
            },
        ));
        rows.push(row(
            General,
            "General",
            "Running Process Indicator",
            Some("A dot in the close button while a command runs"),
            Control::Toggle {
                key: "window-hide-dirty-indicator",
                on: !c.hide_dirty_indicator,
                invert: true,
            },
        ));
        rows.push(row(
            General,
            "Help",
            "Welcome Guide",
            Some("Theme, launch, agents and shortcuts, step by step"),
            Control::Button("Show", Command::ShowWelcome),
        ));

        // Shell
        let shell_names: Vec<String> = self.shells.clone();
        let login = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        rows.push(row(
            Shell,
            "Shell",
            "Shell",
            Some("The program new tabs and splits run"),
            names_choice(
                "shell",
                format!("System Default ({})", basename(&login)),
                &shell_names,
                c.shell.as_deref(),
            ),
        ));
        rows.push(row(
            Shell,
            "Shell",
            "Shell Integration",
            Some("Prompt marks, working directory and command status for zsh, bash and fish"),
            Control::Toggle {
                key: "shell-integration",
                on: c.shell_integration,
                invert: false,
            },
        ));
        let scrollback = c.scrollback.unwrap_or(10_000);
        let presets = [1_000usize, 5_000, 10_000, 50_000, 100_000];
        let mut options: Vec<(SharedString, Option<Value>)> = presets
            .iter()
            .map(|n| {
                let label = format!("{} lines", group_digits(*n));
                let value = (*n != 10_000).then_some(Value::Integer(*n as i64));
                (label.into(), value)
            })
            .collect();
        let mut current = presets.iter().position(|n| *n == scrollback);
        if current.is_none() {
            options.push((
                format!("{} lines", group_digits(scrollback)).into(),
                Some(Value::Integer(scrollback as i64)),
            ));
            current = Some(options.len() - 1);
        }
        rows.push(row(
            Shell,
            "Shell",
            "Scrollback",
            Some("Lines each pane keeps"),
            Control::Choice {
                key: "scrollback",
                options,
                current: current.unwrap_or(2),
            },
        ));
        let installed = crate::workspace::cli_command() == "motty";
        rows.push(row(
            Shell,
            "Motty CLI",
            "Command-Line Tool",
            Some(if installed {
                "Installed at /usr/local/bin/motty"
            } else {
                "Shells in Motty already have it; install it for other apps and terminals"
            }),
            if installed {
                Control::Info("Installed".into())
            } else {
                Control::Button("Install CLI", Command::InstallCli)
            },
        ));

        // Controls
        rows.push(row(
            Controls,
            "Mouse",
            "Right-Click Action",
            Some("What right-click does in a terminal (⌃-click always opens the menu)"),
            choice(
                "right-click-action",
                &[
                    ("Context Menu", "context-menu"),
                    ("Copy", "copy"),
                    ("Paste", "paste"),
                    ("Copy or Paste", "copy-or-paste"),
                    ("Ignore", "ignore"),
                ],
                match c.right_click {
                    RightClick::ContextMenu => "Context Menu",
                    RightClick::Copy => "Copy",
                    RightClick::Paste => "Paste",
                    RightClick::CopyOrPaste => "Copy or Paste",
                    RightClick::Ignore => "Ignore",
                },
            ),
        ));
        rows.push(row(
            Controls,
            "Links",
            "Detect Links",
            Some("Find URLs and file paths in output (OSC 8 links always work)"),
            Control::Toggle {
                key: "link-detection",
                on: c.link_detection,
                invert: false,
            },
        ));
        rows.push(row(
            Controls,
            "Links",
            "⌘-Click",
            None,
            choice(
                "link-cmd-click",
                &[
                    ("Open", "open"),
                    ("Copy", "copy"),
                    ("Do Nothing", "nothing"),
                ],
                match c.link_cmd_click {
                    LinkClick::Open => "Open",
                    LinkClick::Copy => "Copy",
                    LinkClick::Nothing => "Do Nothing",
                },
            ),
        ));
        rows.push(row(
            Controls,
            "Links",
            "⌘⇧-Click on a Path",
            None,
            choice(
                "link-cmd-shift-click",
                &[
                    ("Reveal in Finder", "reveal-finder"),
                    ("Open with Default App", "open-system-default"),
                ],
                match c.link_cmd_shift_click {
                    LinkShiftClick::RevealFinder => "Reveal in Finder",
                    LinkShiftClick::OpenSystemDefault => "Open with Default App",
                },
            ),
        ));
        rows.push(row(
            Controls,
            "Keyboard",
            "Option as Meta",
            Some("⌥ sends Esc-prefixed keys instead of typing special characters"),
            Control::Toggle {
                key: "macos-option-as-alt",
                on: c.option_as_meta,
                invert: false,
            },
        ));

        // Editor
        let a = &c.autocomplete;
        rows.push(row(
            Editor,
            "Autocomplete",
            "Inline Suggestions",
            Some("Show the rest of a command you ran before, dimmed after the cursor"),
            Control::Toggle {
                key: "autocomplete-inline-suggestion",
                on: a.inline,
                invert: false,
            },
        ));
        rows.push(row(
            Editor,
            "Autocomplete",
            "Accept With",
            None,
            choice(
                "autocomplete-shortcut",
                &[
                    ("Tab", "tab"),
                    ("Tab or →", "tab+right-arrow"),
                    ("→", "right-arrow"),
                    ("⌃Space", "ctrl+space"),
                    ("Off", "disable"),
                ],
                match a.shortcut {
                    AcceptKey::Tab => "Tab",
                    AcceptKey::TabRightArrow => "Tab or →",
                    AcceptKey::RightArrow => "→",
                    AcceptKey::CtrlSpace => "⌃Space",
                    AcceptKey::Disable => "Off",
                },
            ),
        ));
        rows.push(row(
            Editor,
            "Autocomplete",
            "Learn From Commands",
            Some("Remember the commands you run, on this Mac only"),
            Control::Toggle {
                key: "autocomplete-on-device-learning",
                on: a.learn,
                invert: false,
            },
        ));
        rows.push(row(
            Editor,
            "Autocomplete",
            "Skip Secrets",
            Some("Never remember commands that look like they carry a password or token"),
            Control::Toggle {
                key: "autocomplete-skip-secrets",
                on: a.skip_secrets,
                invert: false,
            },
        ));
        rows.push(row(
            Editor,
            "Autocomplete",
            "Command History",
            Some("Forget every remembered command"),
            Control::Button("Clear…", Command::ClearHistory),
        ));

        // Agents
        let cli = crate::workspace::cli_command();
        rows.push(row(
            Agents,
            "MCP Server",
            "Claude Code",
            Some("Lets the agent list panes, run commands in splits and read output"),
            Control::Copy(format!("claude mcp add motty -- {cli} mcp")),
        ));
        rows.push(row(
            Agents,
            "MCP Server",
            "Codex",
            None,
            Control::Copy(format!("codex mcp add motty -- {cli} mcp")),
        ));

        // Appearance
        let light: Vec<String> = self.themes.iter().map(|(n, _)| n.clone()).collect();
        rows.push(row(
            Appearance,
            "Theme",
            "Light Theme",
            Some("Used while macOS is in light mode"),
            names_choice("theme", "Motty Light".into(), &light, c.theme.as_deref()),
        ));
        rows.push(row(
            Appearance,
            "Theme",
            "Dark Theme",
            Some("Used while macOS is in dark mode"),
            names_choice(
                "theme-dark",
                match c.theme.as_deref() {
                    Some(t) => format!("Same as Light ({t})"),
                    None => "Motty Dark".into(),
                },
                &light,
                c.theme_dark.as_deref(),
            ),
        ));
        rows.push(row(
            Appearance,
            "Font",
            "Font Family",
            None,
            names_choice(
                "font-family",
                format!("Theme Default ({})", default_font(c, cx)),
                &self.fonts,
                c.font_family.as_deref(),
            ),
        ));
        rows.push(row(
            Appearance,
            "Font",
            "Font Size",
            None,
            Control::Stepper {
                key: "font-size",
                value: c.font_size.unwrap_or(13.),
                min: 6.,
                max: 72.,
            },
        ));
        rows.push(row(
            Appearance,
            "Font",
            "Ligatures",
            Some("Join character pairs like -> and != where the font has them"),
            Control::Toggle {
                key: "font-ligatures",
                on: c.ligatures,
                invert: false,
            },
        ));
        rows.push(row(
            Appearance,
            "Cursor",
            "Cursor Style",
            None,
            choice(
                "cursor-style",
                &[
                    ("Block", "block"),
                    ("Block (Hollow)", "block_hollow"),
                    ("Bar", "bar"),
                    ("Underline", "underline"),
                ],
                match c.cursor_style {
                    CursorStyle::Block => "Block",
                    CursorStyle::BlockHollow => "Block (Hollow)",
                    CursorStyle::Bar => "Bar",
                    CursorStyle::Underline => "Underline",
                },
            ),
        ));
        rows.push(row(
            Appearance,
            "Cursor",
            "Cursor Blink",
            Some("The Default options let programs turn blinking on or off"),
            choice(
                "cursor-style-blink",
                &[
                    ("Default Off", "default_off"),
                    ("Default On", "default_on"),
                    ("Always Off", "always_off"),
                    ("Always On", "always_on"),
                ],
                match c.cursor_blink {
                    CursorBlink::DefaultOff => "Default Off",
                    CursorBlink::DefaultOn => "Default On",
                    CursorBlink::AlwaysOff => "Always Off",
                    CursorBlink::AlwaysOn => "Always On",
                },
            ),
        ));
        let opacity = c.unfocused_split_opacity.unwrap_or(0.6);
        let steps = [1.0f32, 0.9, 0.8, 0.7, 0.6, 0.5, 0.4];
        rows.push(row(
            Appearance,
            "Splits",
            "Unfocused Split Opacity",
            Some("How strongly panes other than the focused one are dimmed"),
            Control::Choice {
                key: "unfocused-split-opacity",
                current: steps
                    .iter()
                    .position(|v| (v - opacity).abs() < 0.05)
                    .unwrap_or(4),
                options: steps
                    .iter()
                    .map(|v| {
                        let label = format!("{}%", (v * 100.).round());
                        let value = ((v - 0.6).abs() > 0.01).then_some(Value::Float(*v as f64));
                        (label.into(), value)
                    })
                    .collect(),
            },
        ));

        // Key bindings
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        for (group, title, action) in key_binding_rows() {
            let caps = keymap
                .bindings_for_action(&*action)
                .last()
                .and_then(|b| b.keystrokes().first().map(|k| keycaps_of(k.inner())))
                .unwrap_or_default();
            rows.push(row(KeyBindings, group, title, None, Control::Keys(caps)));
        }

        // Advanced
        let path = self.config_path();
        let home = dirs::home_dir().unwrap_or_default();
        let shown = match path.strip_prefix(&home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        };
        rows.push(Row {
            section: Advanced,
            group: "Config File",
            title: "config.toml".into(),
            description: Some(format!("{shown} · settings here are saved to it").into()),
            control: Control::Button("Open", Command::OpenConfig),
        });
        rows.push(row(
            Advanced,
            "Config File",
            "Show in Finder",
            None,
            Control::Button("Reveal", Command::RevealConfig),
        ));
        rows.push(row(
            Advanced,
            "Config File",
            "Reload Config",
            Some("Motty also notices when the file changes"),
            Control::Button("Reload", Command::ReloadConfig),
        ));
        for w in &c.warnings {
            rows.push(row(
                Advanced,
                "Problems",
                w.clone(),
                None,
                Control::Info("".into()),
            ));
        }
        rows
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A guess from the name: CoreText's fixed-pitch trait isn't exposed here.
fn looks_monospace(name: &str) -> bool {
    const HINTS: [&str; 16] = [
        "mono",
        "code",
        "menlo",
        "monaco",
        "courier",
        "consol",
        "hack",
        "iosevka",
        "terminal",
        "fixed",
        "inconsolata",
        "jetbrains",
        "fira",
        "cascadia",
        "meslo",
        "plex mono",
    ];
    let lower = name.to_lowercase();
    !name.starts_with('.') && HINTS.iter().any(|h| lower.contains(h))
}

fn default_font(config: &Config, cx: &App) -> String {
    let mode = mode_for(cx.window_appearance());
    let theme = config.resolve_theme(mode).theme;
    let mut no_family = config.clone();
    no_family.font_family = None;
    crate::fonts::terminal_family(&no_family, &theme, cx)
}

fn mode_for(appearance: WindowAppearance) -> Mode {
    match appearance {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Mode::Dark,
        _ => Mode::Light,
    }
}

fn key_binding_rows() -> Vec<(&'static str, &'static str, Box<dyn Action>)> {
    vec![
        ("General", "Command Palette", Box::new(CommandPalette)),
        ("General", "Open Quickly", Box::new(OpenQuickly)),
        ("General", "Jump To", Box::new(JumpTo)),
        ("General", "Open Files Quickly", Box::new(OpenFilesQuickly)),
        ("General", "Settings", Box::new(OpenSettings)),
        ("General", "New Window", Box::new(NewWindow)),
        ("Tabs", "New Tab", Box::new(NewTab)),
        ("Tabs", "Close Tab", Box::new(CloseTab)),
        ("Tabs", "Reopen Closed Tab", Box::new(ReopenClosedTab)),
        ("Tabs", "Next Tab", Box::new(NextTab)),
        ("Tabs", "Previous Tab", Box::new(PreviousTab)),
        ("Panes", "Split Right", Box::new(SplitRight)),
        ("Panes", "Split Down", Box::new(SplitDown)),
        ("Panes", "Split Left", Box::new(SplitLeft)),
        ("Panes", "Split Up", Box::new(SplitUp)),
        ("Panes", "Focus Next Pane", Box::new(FocusNextPane)),
        ("Panes", "Focus Previous Pane", Box::new(FocusPreviousPane)),
        ("Panes", "Zoom Split", Box::new(ZoomSplit)),
        ("Panes", "Equalize Splits", Box::new(EqualizeSplits)),
        ("Terminal", "Find", Box::new(Find)),
        ("Terminal", "Clear Screen", Box::new(Clear)),
        ("Terminal", "Vi Mode", Box::new(ToggleViMode)),
        ("Terminal", "Read-Only Mode", Box::new(ToggleReadOnly)),
        ("Terminal", "Hints: Open Link", Box::new(HintOpen)),
        ("View", "Toggle Tabs Panel", Box::new(ToggleTabsPanel)),
        ("View", "Toggle Details Panel", Box::new(ToggleDetailsPanel)),
        ("View", "Increase Font Size", Box::new(IncreaseFontSize)),
        ("View", "Decrease Font Size", Box::new(DecreaseFontSize)),
        ("View", "Reset Font Size", Box::new(ResetFontSize)),
        ("View", "Toggle Full Screen", Box::new(ToggleFullScreen)),
    ]
}

fn keycaps_of(ks: &gpui::Keystroke) -> Vec<SharedString> {
    crate::workspace::keycaps(ks)
}

/// The window's colors, from Otty's light settings window (sidebar #f7f7f7,
/// rows #fafafa on white, selection #e9e9e9) and a matching dark set.
struct Colors {
    sidebar: Hsla,
    content: Hsla,
    card: Hsla,
    separator: Hsla,
    selected: Hsla,
    field: Hsla,
    text: Hsla,
    secondary: Hsla,
    header: Hsla,
    control: Hsla,
    control_border: Hsla,
    switch_off: Hsla,
    accent: Hsla,
    error: Hsla,
}

impl Colors {
    fn new(dark: bool) -> Self {
        let c =
            |light: u32, dark_value: u32| hsla(Rgba::rgb(if dark { dark_value } else { light }));
        let ink = |a: f32| {
            hsla(if dark {
                Rgba::rgb(0xffffff).with_alpha(a)
            } else {
                Rgba::rgb(0x000000).with_alpha(a)
            })
        };
        Self {
            sidebar: c(0xf7f7f7, 0x262626),
            content: c(0xffffff, 0x1e1e1e),
            card: c(0xfafafa, 0x262626),
            separator: ink(0.06),
            selected: c(0xe9e9e9, 0x353535),
            field: c(0xededed, 0x303030),
            text: ink(0.88),
            secondary: ink(0.5),
            header: ink(0.38),
            control: c(0xffffff, 0x3a3a3a),
            control_border: ink(0.14),
            switch_off: c(0xe2e2e2, 0x3c3c3c),
            accent: hsla(Rgba::rgb(0x34c759)),
            error: hsla(Rgba::rgb(0xe5484d)),
        }
    }
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = mode_for(window.appearance()) == Mode::Dark;
        let c = Colors::new(dark);
        let all = self.rows(cx);
        let query = self.query.trim().to_lowercase();
        let searching = !query.is_empty();
        let shown: Vec<Row> = all
            .into_iter()
            .filter(|r| {
                if searching {
                    r.title.to_lowercase().contains(&query)
                        || r.description
                            .as_ref()
                            .is_some_and(|d| d.to_lowercase().contains(&query))
                        || r.group.to_lowercase().contains(&query)
                        || r.section.title().to_lowercase().contains(&query)
                } else {
                    r.section == self.section
                }
            })
            .collect();

        let nav = Section::ALL.into_iter().map(|section| {
            let selected = !searching && section == self.section;
            div()
                .id(section.title())
                .h(px(29.))
                .mt(px(2.5))
                .pl(px(20.))
                .flex()
                .items_center()
                .gap(px(10.))
                .when(selected, |d| d.bg(c.selected))
                .child(
                    svg()
                        .path(section.icon())
                        .size(px(16.))
                        .text_color(c.secondary),
                )
                .child(div().text_color(c.text).child(section.title()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.section = section;
                    this.query.clear();
                    this.scroll.scroll_to_item(0);
                    cx.notify();
                }))
        });
        let search = div()
            .mx(px(12.))
            .mt(px(38.))
            .mb(px(14.))
            .h(px(30.))
            .px(px(11.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded_full()
            .bg(c.field)
            .child(
                svg()
                    .path("icons/search.svg")
                    .size(px(14.))
                    .text_color(c.secondary),
            )
            .child(if searching {
                div()
                    .flex()
                    .items_center()
                    .text_color(c.text)
                    .child(SharedString::from(self.query.clone()))
                    .child(div().w(px(1.5)).h(px(15.)).bg(hsla(Rgba::rgb(0x047afe))))
            } else {
                div().text_color(c.secondary).child("Search")
            });
        let sidebar = div()
            .w(px(SIDEBAR))
            .flex_none()
            .h_full()
            .bg(c.sidebar)
            .border_r(px(1.))
            .border_color(c.separator)
            .flex()
            .flex_col()
            .child(search)
            .children(nav);

        // Groups in the order their rows come.
        let mut groups: Vec<(String, Vec<Row>)> = Vec::new();
        for r in shown {
            let name = if searching {
                format!("{} · {}", r.section.title(), r.group)
            } else {
                r.group.to_owned()
            };
            match groups.last_mut() {
                Some((g, rows)) if *g == name => rows.push(r),
                _ => groups.push((name, vec![r])),
            }
        }
        let empty = groups.is_empty();
        let mut ix = 0;
        let content = groups.into_iter().map(|(name, rows)| {
            let n = rows.len();
            let rendered: Vec<AnyElement> = rows
                .into_iter()
                .enumerate()
                .map(|(i, r)| {
                    ix += 1;
                    self.render_row(ix, r, i + 1 < n, &c, cx)
                })
                .collect();
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .mt(px(22.))
                        .mb(px(8.))
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(c.header)
                        .child(SharedString::from(name.to_uppercase())),
                )
                .child(
                    div()
                        .rounded(px(10.))
                        .bg(c.card)
                        .border(px(1.))
                        .border_color(c.separator)
                        .flex()
                        .flex_col()
                        .children(rendered),
                )
        });
        let status = self.status.as_ref().map(|(ok, text)| {
            div()
                .mt(px(14.))
                .text_size(px(12.))
                .text_color(if *ok { c.secondary } else { c.error })
                .child(SharedString::from(text.clone()))
        });
        let main = div()
            .id("settings-content")
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(c.content)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(PAD))
            .pt(px(10.))
            .pb(px(PAD))
            .children(content)
            .when(empty, |d| {
                d.child(
                    div()
                        .mt(px(60.))
                        .flex()
                        .justify_center()
                        .text_color(c.secondary)
                        .child("No matching settings"),
                )
            })
            .children(status);

        div()
            .id("settings")
            .key_context("Settings")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .flex()
            .bg(c.content)
            .text_size(px(13.))
            .font_family(".SystemUIFont")
            .text_color(c.text)
            .child(sidebar)
            .child(main)
    }
}

impl SettingsWindow {
    fn render_row(
        &self,
        ix: usize,
        r: Row,
        separator: bool,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut label = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(div().text_color(c.text).child(r.title.clone()));
        if let Some(d) = &r.description {
            label = label.child(
                div()
                    .text_size(px(11.))
                    .text_color(c.secondary)
                    .child(d.clone()),
            );
        }
        // A command line gets its own line under the label.
        if let Control::Copy(_) = &r.control {
            let control = self.render_control(ix, r.control, c, cx);
            return div()
                .mx(px(15.))
                .py(px(10.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .when(separator, |d| d.border_b(px(1.)).border_color(c.separator))
                .child(label)
                .child(control)
                .into_any_element();
        }
        let control = self.render_control(ix, r.control, c, cx);
        div()
            .min_h(px(ROW_MIN))
            .mx(px(15.))
            .py(px(8.))
            .flex()
            .items_center()
            .gap(px(16.))
            .when(separator, |d| d.border_b(px(1.)).border_color(c.separator))
            .child(label)
            .child(control)
            .into_any_element()
    }

    fn render_control(
        &self,
        ix: usize,
        control: Control,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();
        match control {
            Control::Toggle { key, on, invert } => {
                let (track, knob_left) = if on {
                    (c.accent, 18.)
                } else {
                    (c.switch_off, 2.)
                };
                div()
                    .id(("toggle", ix))
                    .flex_none()
                    .relative()
                    .w(px(38.))
                    .h(px(22.))
                    .rounded_full()
                    .bg(track)
                    .child(
                        div()
                            .absolute()
                            .top(px(2.))
                            .left(px(knob_left))
                            .size(px(18.))
                            .rounded_full()
                            .bg(hsla(Rgba::rgb(0xffffff)))
                            .shadow_sm(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let next = !on;
                        this.apply(key, Some(Value::Boolean(next != invert)), cx)
                    }))
                    .into_any_element()
            }
            Control::Choice {
                key,
                options,
                current,
            } => {
                let label = options
                    .get(current)
                    .map(|(l, _)| l.clone())
                    .unwrap_or_default();
                div()
                    .id(("choice", ix))
                    .flex_none()
                    .max_w(px(240.))
                    .h(px(26.))
                    .pl(px(10.))
                    .pr(px(7.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(6.))
                    .bg(c.control)
                    .border(px(0.5))
                    .border_color(c.control_border)
                    .shadow_xs()
                    .child(div().min_w_0().truncate().child(label))
                    .child(
                        svg()
                            .path("icons/chevron-down.svg")
                            .size(px(12.))
                            .flex_none()
                            .text_color(c.secondary),
                    )
                    .on_click(move |event: &ClickEvent, window, cx| {
                        let entries: Vec<Entry> = options
                            .iter()
                            .enumerate()
                            .map(|(i, (l, _))| Item::new(l.clone()).checked(i == current).into())
                            .collect();
                        let values: Vec<Option<Value>> =
                            options.iter().map(|(_, v)| v.clone()).collect();
                        let entity = entity.clone();
                        let at = event.position() - point(px(14.), px(10.));
                        native_menu::popup(
                            entries,
                            at,
                            Some(current),
                            window,
                            cx,
                            move |i, _, cx| {
                                if i == current {
                                    return;
                                }
                                if let Some(value) = values.get(i).cloned() {
                                    entity.update(cx, |this, cx| this.apply(key, value, cx));
                                }
                            },
                        );
                    })
                    .into_any_element()
            }
            Control::Stepper {
                key,
                value,
                min,
                max,
            } => {
                let button = |id: &'static str, label: &'static str, delta: f32| {
                    div()
                        .id((id, ix))
                        .w(px(26.))
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(c.text)
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let next = (value + delta).clamp(min, max);
                            if next != value {
                                let v = if next.fract() == 0. {
                                    Value::Integer(next as i64)
                                } else {
                                    Value::Float(next as f64)
                                };
                                this.apply(key, Some(v), cx);
                            }
                        }))
                };
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .bg(c.control)
                    .border(px(0.5))
                    .border_color(c.control_border)
                    .child(button("minus", "−", -1.))
                    .child(
                        div()
                            .min_w(px(36.))
                            .flex()
                            .justify_center()
                            .border_l(px(0.5))
                            .border_r(px(0.5))
                            .border_color(c.control_border)
                            .child(SharedString::from(format!("{value}"))),
                    )
                    .child(button("plus", "+", 1.))
                    .into_any_element()
            }
            Control::Button(label, command) => div()
                .id(("button", ix))
                .flex_none()
                .h(px(26.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .bg(c.control)
                .border(px(0.5))
                .border_color(c.control_border)
                .shadow_xs()
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.run(command, window, cx)))
                .into_any_element(),
            Control::Copy(text) => {
                let copied = self.copied.as_deref() == Some(text.as_str());
                div()
                    .w_full()
                    .min_h(px(34.))
                    .pl(px(10.))
                    .pr(px(4.))
                    .py(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(7.))
                    .bg(c.field)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .font_family("Menlo")
                            .text_color(c.secondary)
                            .child(SharedString::from(text.clone())),
                    )
                    .child(
                        div()
                            .id(("copy", ix))
                            .flex_none()
                            .h(px(26.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .bg(c.control)
                            .border(px(0.5))
                            .border_color(c.control_border)
                            .child(if copied { "Copied" } else { "Copy" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                this.copied = Some(text.clone());
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }
            Control::Keys(caps) => div()
                .flex_none()
                .flex()
                .gap(px(4.))
                .when(caps.is_empty(), |d| d.text_color(c.secondary).child("—"))
                .children(caps.into_iter().map(|cap| {
                    div()
                        .min_w(px(21.))
                        .h(px(20.))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .border(px(0.5))
                        .border_color(c.control_border)
                        .bg(c.control)
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(c.secondary)
                        .child(cap)
                }))
                .into_any_element(),
            Control::Info(text) => div()
                .flex_none()
                .text_color(c.secondary)
                .child(text)
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_grouped() {
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(100_000), "100,000");
        assert_eq!(group_digits(500), "500");
    }

    #[test]
    fn monospace_guess() {
        assert!(looks_monospace("JetBrains Mono"));
        assert!(looks_monospace("SF Mono"));
        assert!(looks_monospace("Menlo"));
        assert!(!looks_monospace("Helvetica"));
        assert!(!looks_monospace(".SF NS Mono"));
    }
}
