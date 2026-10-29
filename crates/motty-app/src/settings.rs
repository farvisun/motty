//! The settings window (⌘,), laid out like Otty's: a sidebar of sections
//! with a search field, and on the right groups of rows, each a setting
//! with a native pop-up button, a switch or a stepper. Every change is
//! written to config.toml straight away and applied to open windows.
//!
//! Rows are data ([`rows`]), built from the current config each frame, so
//! the sections and the search results are the same rows.

use std::borrow::Cow;
use std::time::Duration;

use gpui::{
    Action, Animation, AnimationExt, AnyElement, App, AppContext, AssetSource, Bounds, ClickEvent,
    ClipboardItem, Context, FocusHandle, Focusable, Global, Hsla, KeyDownEvent, Keystroke,
    ScrollHandle, SharedString, Subscription, TitlebarOptions, Window, WindowAppearance,
    WindowBounds, WindowHandle, WindowOptions, div, ease_out_quint, point, prelude::*, px, size,
    svg,
};
use motty_render::hsla;
use motty_theme::{
    AcceptKey, Config, ConfirmClose, CursorBlink, CursorStyle, LinkClick, LinkShiftClick, Mode,
    Rgba, RightClick,
};
use toml::Value;

use crate::actions::*;
use crate::agents::{self, hooks};
use crate::native_menu::{self, Entry, Item};
use motty_ipc::skills;

const WIDTH: f32 = 720.;
const HEIGHT: f32 = 520.;
const SIDEBAR: f32 = 197.;
const PAD: f32 = 28.;
const ROW_MIN: f32 = 43.;
/// Where the first sidebar item starts, and the distance between items.
const NAV_TOP: f32 = 84.5;
const NAV_PITCH: f32 = 31.5;
const NAV_HEIGHT: f32 = 29.;

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
            "chevron-down",
            "loader-circle",
            "check",
            "hand",
            "triangle-alert",
            "text-select",
            "panels-top-left",
            "blocks",
            "book-open",
            "shield",
            "coffee",
            "monitor-smartphone",
            "pin"
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
    Selection,
    Appearance,
    View,
    KeyBindings,
    Agents,
    Integrations,
    Recipes,
    Editor,
    Advanced,
}

impl Section {
    const ALL: [Section; 12] = [
        Section::General,
        Section::Shell,
        Section::Controls,
        Section::Selection,
        Section::Appearance,
        Section::View,
        Section::KeyBindings,
        Section::Agents,
        Section::Integrations,
        Section::Recipes,
        Section::Editor,
        Section::Advanced,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Shell => "Shell",
            Section::Controls => "Controls",
            Section::Selection => "Selection",
            Section::Appearance => "Appearance",
            Section::View => "View",
            Section::KeyBindings => "Key Bindings",
            Section::Agents => "Agents",
            Section::Integrations => "Integrations",
            Section::Recipes => "Recipes",
            Section::Editor => "Editor",
            Section::Advanced => "Advanced",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Section::General => "icons/circle-alert.svg",
            Section::Shell => "icons/terminal.svg",
            Section::Controls => "icons/mouse-pointer-2.svg",
            Section::Selection => "icons/text-select.svg",
            Section::Appearance => "icons/palette.svg",
            Section::View => "icons/panels-top-left.svg",
            Section::KeyBindings => "icons/keyboard.svg",
            Section::Agents => "icons/plug.svg",
            Section::Integrations => "icons/blocks.svg",
            Section::Recipes => "icons/book-open.svg",
            Section::Editor => "icons/file-text.svg",
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
    /// A shortcut that can be recorded: `row` indexes [`key_binding_rows`].
    Shortcut {
        row: usize,
        caps: Vec<SharedString>,
        custom: bool,
    },
    /// A `text:`/`csi:`/`esc:` binding: `ix` indexes [`sequences`].
    Sequence {
        ix: usize,
        caps: Vec<SharedString>,
        payload: String,
    },
    /// Buttons for the whole list: add a sequence, reset everything.
    BindingTools,
    /// A coding agent's integration switch: `ix` indexes [`agents::CATALOG`].
    Agent {
        ix: usize,
        installed: bool,
        open: bool,
    },
    Info(SharedString),
    /// A skill's Copy button: `ix` indexes [`skills::SKILLS`].
    Skill(usize),
}

/// The dialog a skill's Copy opens: the name it's installed under, and
/// whether agents may type into panes.
struct SkillDialog {
    ix: usize,
    name: String,
    allow: bool,
}

struct Row {
    section: Section,
    group: &'static str,
    title: SharedString,
    description: Option<SharedString>,
    control: Control,
    /// Shown greyed out, and not clickable.
    dim: bool,
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
        dim: false,
    }
}

impl Row {
    fn dimmed(mut self, dim: bool) -> Self {
        self.dim = dim;
        self
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
    /// What the next keystroke is recorded for.
    recording: Option<Record>,
    /// A sequence binding's text being typed: its index and the text.
    editing: Option<(usize, String)>,
    /// "Reset to Default" asks once more before it clears every binding.
    confirm_reset: bool,
    /// Each agent's program and whether Motty's hooks are in its settings.
    agents: Vec<AgentInfo>,
    /// The agent whose details are shown.
    agent_open: Option<usize>,
    skill: Option<SkillDialog>,
    /// The section shown before the current one, and a count bumped on each
    /// switch, which restarts the transition.
    previous: Section,
    epoch: usize,
    _intercept: Subscription,
}

/// `~/…` for a path in the home folder.
fn tilde(path: &std::path::Path) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// An agent's mark: its initial on a colour of its own (Motty doesn't
/// ship the agents' logos).
fn agent_mark(ix: usize) -> AnyElement {
    const COLORS: [u32; 12] = [
        0xd97757, 0x10a37f, 0x3b82f6, 0x4b5563, 0x6366f1, 0x8b5cf6, 0xa855f7, 0x111827, 0x0ea5e9,
        0x14b8a6, 0xf59e0b, 0x22c55e,
    ];
    let agent = &agents::CATALOG[ix];
    let initial: String = agent.label.chars().take(1).collect();
    div()
        .flex_none()
        .size(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .bg(hsla(Rgba::rgb(COLORS[ix % COLORS.len()])))
        .text_size(px(13.))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(hsla(Rgba::rgb(0xffffff)))
        .child(initial)
        .into_any_element()
}

struct AgentInfo {
    path: Option<std::path::PathBuf>,
    installed: bool,
}

fn scan_agents() -> Vec<AgentInfo> {
    let env = hooks::Env::current();
    agents::CATALOG
        .iter()
        .map(|a| AgentInfo {
            path: agents::locate(a.command),
            installed: hooks::installed(a.id, &env),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Record {
    /// A new chord for `key_binding_rows()[row]`.
    Action(usize),
    /// A new chord for an existing sequence binding.
    Sequence(usize),
    /// The chord for a sequence binding being added.
    NewSequence,
}

/// The `keybind` lines that type text: (trigger, payload).
fn sequences(config: &Config) -> Vec<(String, String)> {
    config
        .keybinds
        .iter()
        .filter(|k| {
            ["text:", "csi:", "esc:"]
                .iter()
                .any(|p| k.action.starts_with(p))
        })
        .map(|k| (k.trigger.clone(), k.action.clone()))
        .collect()
}

/// Key caps for a config trigger (`cmd+shift+t`).
fn trigger_caps(trigger: &str) -> Vec<SharedString> {
    let gpui_form = trigger.replace('+', "-");
    let gpui_form = if trigger.ends_with("++") {
        format!("{}+", &gpui_form[..gpui_form.len() - 2])
    } else {
        gpui_form
    };
    match Keystroke::parse_with_os(&gpui_form, gpui::OperatingSystem::Mac) {
        Ok(k) => keycaps_of(&k),
        Err(_) => vec![trigger.to_owned().into()],
    }
}

struct Open(WindowHandle<SettingsWindow>);
impl Global for Open {}

/// ⌘, opens the settings window, or brings it forward.
pub fn open(section: Option<Section>, cx: &mut App) {
    if let Some(Open(handle)) = cx.try_global::<Open>().map(|o| Open(o.0))
        && handle
            .update(cx, |this, window, cx| {
                if let Some(section) = section {
                    this.show_section(section);
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
        // While a shortcut is being recorded, every key in this window is
        // taken before any binding (⌘N would otherwise open a window).
        let window_id = window.window_handle().window_id();
        let this = cx.entity().downgrade();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle().window_id() != window_id {
                return;
            }
            let Some(this) = this.upgrade() else {
                return;
            };
            if this.read(cx).recording.is_none() {
                return;
            }
            let ks = event.keystroke.clone();
            this.update(cx, |this, cx| this.record(&ks, cx));
            cx.stop_propagation();
        });
        // Changes made elsewhere (the file, the welcome guide) show up when
        // the window comes back to the front.
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.config = crate::load_config();
                this.agents = scan_agents();
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
            recording: None,
            editing: None,
            confirm_reset: false,
            agents: scan_agents(),
            agent_open: None,
            skill: None,
            previous: section,
            epoch: 0,
            _intercept: intercept,
        }
    }

    fn show_section(&mut self, section: Section) {
        if section != self.section {
            self.previous = self.section;
            self.section = section;
            self.epoch += 1;
        }
        self.query.clear();
        self.recording = None;
        self.editing = None;
        self.confirm_reset = false;
        self.scroll.scroll_to_item(0);
    }

    /// Binds `row`'s action to exactly `chords`.
    fn set_shortcut(&mut self, row: usize, chords: Vec<String>, cx: &mut Context<Self>) {
        let Some((_, title, action)) = key_binding_rows().into_iter().nth(row) else {
            return;
        };
        let name = crate::actions::config_name(&*action);
        let defaults = crate::actions::default_chords(&*action);
        match motty_theme::config::save_keybinds(&name, &defaults, &chords) {
            Ok(_) => self.after_keybind_save(cx),
            Err(e) => {
                self.status = Some((false, format!("Couldn't save {title}: {e}")));
            }
        }
    }

    /// Replaces the sequence binding `old` (or adds one) with `new`.
    fn set_sequence(
        &mut self,
        old: Option<(String, String)>,
        new: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let result = motty_theme::config::edit(|src| {
            let mut out = String::new();
            let mut replaced = false;
            for line in src.lines() {
                let is_old = old.as_ref().is_some_and(|(t, a)| {
                    let parsed = line
                        .trim_start()
                        .strip_prefix("keybind")
                        .map(str::trim_start)
                        .and_then(|r| r.strip_prefix('='))
                        .map(|v| v.trim().trim_matches('"'))
                        .and_then(motty_theme::config::Keybind::parse);
                    parsed.is_some_and(|k| k.trigger == *t && k.action == *a)
                });
                if is_old {
                    if let Some((t, a)) = &new
                        && !replaced
                    {
                        out.push_str(&format!("keybind = \"{t}={a}\"\n"));
                        replaced = true;
                    }
                    continue;
                }
                out.push_str(line);
                out.push('\n');
            }
            if !replaced && let Some((t, a)) = &new {
                out.push_str(&format!("keybind = \"{t}={a}\"\n"));
            }
            out
        });
        match result {
            Ok(_) => self.after_keybind_save(cx),
            Err(e) => self.status = Some((false, format!("Couldn't save config.toml: {e}"))),
        }
    }

    fn after_keybind_save(&mut self, cx: &mut Context<Self>) {
        crate::workspace::reload_all(cx);
        self.config = crate::load_config();
        // The keymap changed under the menus too.
        crate::menus::set(cx);
        cx.notify();
    }

    /// The next keystroke while recording.
    fn record(&mut self, ks: &Keystroke, cx: &mut Context<Self>) {
        let Some(target) = self.recording else {
            return;
        };
        let m = &ks.modifiers;
        let plain = !(m.platform || m.control || m.alt || m.shift || m.function);
        if plain && ks.key == "escape" {
            self.recording = None;
            return cx.notify();
        }
        if plain && matches!(ks.key.as_str(), "backspace" | "delete") {
            self.recording = None;
            self.status = None;
            match target {
                Record::Action(row) => self.set_shortcut(row, Vec::new(), cx),
                Record::Sequence(ix) => {
                    let old = sequences(&self.config).get(ix).cloned();
                    self.set_sequence(old, None, cx);
                }
                Record::NewSequence => {}
            }
            return cx.notify();
        }
        let function_key = ks.key.len() > 1
            && ks.key.starts_with('f')
            && ks.key[1..].chars().all(|c| c.is_ascii_digit());
        if !(m.platform || m.control || m.alt || function_key) {
            self.status = Some((
                false,
                "Shortcuts need ⌘, ⌃ or ⌥ (or a function key).".into(),
            ));
            return cx.notify();
        }
        let chord = crate::actions::config_chord(ks);
        let caps = keycaps_of(ks).concat();
        // Whoever had the chord loses it: the config's line wins.
        let taken = self.rows(cx).into_iter().find_map(|r| match &r.control {
            Control::Shortcut { row, caps: c, .. }
                if c.concat() == caps && Record::Action(*row) != target =>
            {
                Some(r.title.clone())
            }
            _ => None,
        });
        self.status = taken.map(|t| (true, format!("{caps} no longer runs {t}.")));
        self.recording = None;
        match target {
            Record::Action(row) => self.set_shortcut(row, vec![chord], cx),
            Record::Sequence(ix) => {
                let old = sequences(&self.config).get(ix).cloned();
                let payload = old.as_ref().map(|(_, a)| a.clone()).unwrap_or_default();
                self.set_sequence(old, Some((chord, payload)), cx);
            }
            Record::NewSequence => {
                self.set_sequence(None, Some((chord.clone(), "text:".into())), cx);
                let ix = sequences(&self.config)
                    .iter()
                    .position(|(t, _)| *t == chord)
                    .unwrap_or(0);
                self.editing = Some((ix, "text:".into()));
            }
        }
        cx.notify();
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
        if let Some(dialog) = self.skill.as_mut() {
            match ks.key.as_str() {
                "escape" => self.skill = None,
                "enter" => self.copy_skill(cx),
                "backspace" => {
                    dialog.name.pop();
                }
                _ if m.platform || m.control => return,
                _ => match ks.key_char.as_deref() {
                    Some(c) if c.chars().all(|c| c.is_alphanumeric() || "-_.".contains(c)) => {
                        dialog.name.push_str(c)
                    }
                    _ => return,
                },
            }
            cx.stop_propagation();
            return cx.notify();
        }
        if let Some((ix, text)) = self.editing.as_mut() {
            let ix = *ix;
            match ks.key.as_str() {
                "escape" => self.editing = None,
                "enter" => {
                    let text = text.clone();
                    self.editing = None;
                    let old = sequences(&self.config).get(ix).cloned();
                    if let Some((trigger, _)) = old.clone() {
                        self.set_sequence(old, Some((trigger, text)), cx);
                    }
                }
                "backspace" => {
                    text.pop();
                }
                _ if m.platform || m.control => return,
                _ => match ks.key_char.as_deref() {
                    Some(c) if !c.chars().any(char::is_control) => text.push_str(c),
                    _ => return,
                },
            }
            cx.stop_propagation();
            return cx.notify();
        }
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
            "Restore Banners",
            Some("“Closed at” and “Restored at” under a restored pane's scrollback"),
            Control::Toggle {
                key: "session-restore-banner",
                on: c.restore_banner,
                invert: false,
            },
        ));
        rows.push(row(
            General,
            "General",
            "Quit When Last Window Closes",
            Some("Otherwise Motty stays in the Dock; click it for a new window"),
            Control::Toggle {
                key: "quit-after-last-window-closed",
                on: c.quit_after_last_window,
                invert: false,
            },
        ));
        rows.push(row(
            View,
            "Tabs Panel",
            "Show Tabs Panel",
            Some("New windows open with the tabs sidebar"),
            Control::Toggle {
                key: "sidebar-visible",
                on: c.sidebar_visible,
                invert: false,
            },
        ));
        let rule_label = |r: ConfirmClose| match r {
            ConfirmClose::Process => "Running Process",
            ConfirmClose::Always => "Always",
            ConfirmClose::MultipleTabs => "Multiple Tabs",
            ConfirmClose::Never => "Never",
        };
        let rules: &[(&str, &str)] = &[
            ("Running Process", "process"),
            ("Always", "always"),
            ("Never", "never"),
        ];
        rows.push(row(
            General,
            "Close Confirmation",
            "Close Pane",
            Some("Ask before closing a split pane"),
            choice(
                "confirm-close-pane",
                rules,
                rule_label(c.confirm_close_pane),
            ),
        ));
        rows.push(row(
            General,
            "Close Confirmation",
            "Close Tab",
            Some("Ask before closing a tab"),
            choice("confirm-close-tab", rules, rule_label(c.confirm_close_tab)),
        ));
        rows.push(row(
            General,
            "Close Confirmation",
            "Close Window",
            Some("Ask before closing a window"),
            choice(
                "confirm-close-window",
                &[
                    ("Running Process", "process"),
                    ("Always", "always"),
                    ("Multiple Tabs", "multiple_tabs"),
                    ("Never", "never"),
                ],
                rule_label(c.confirm_close_window),
            ),
        ));
        rows.push(row(
            General,
            "Close Confirmation",
            "Ignore Idle Coding Agents",
            Some("An agent waiting at its prompt doesn't count as running"),
            Control::Toggle {
                key: "confirm-close-ignore-idle-agents",
                on: c.close_ignores_idle_agents,
                invert: false,
            },
        ));
        rows.push(row(
            General,
            "Close Confirmation",
            "Hide Close Button Dot",
            Some("Never show the dot in the close button while a command runs"),
            Control::Toggle {
                key: "window-hide-dirty-indicator",
                on: c.hide_dirty_indicator,
                invert: false,
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
        let dir_choice =
            |key: &'static str, inherit: &str, own: &Option<motty_theme::WorkingDir>| {
                let mut options: Vec<(SharedString, Option<Value>)> = vec![
                    (inherit.to_owned().into(), None),
                    ("Home Folder".into(), s("home")),
                ];
                let current = match own {
                    None | Some(motty_theme::WorkingDir::Inherit) => 0,
                    Some(motty_theme::WorkingDir::Home) => 1,
                    Some(motty_theme::WorkingDir::Path(path)) => {
                        options.push((tilde(path).into(), s(&path.display().to_string())));
                        2
                    }
                };
                Control::Choice {
                    key,
                    options,
                    current,
                }
            };
        rows.push(row(
            Shell,
            "Working Directory",
            "New Tab",
            None,
            dir_choice(
                "tab-working-directory",
                "Same as Current Tab",
                &c.tab_working_directory,
            ),
        ));
        rows.push(row(
            Shell,
            "Working Directory",
            "New Split",
            None,
            dir_choice(
                "split-working-directory",
                "Same as Current Pane",
                &c.split_working_directory,
            ),
        ));
        let b = c.badges;
        for (title, note, key, on) in [
            (
                "Notify on Finish",
                "A banner when a command exits 0",
                "privilege-notification-on-finish",
                b.notify_finish,
            ),
            (
                "Notify on Error",
                "A banner when a command exits non-zero",
                "privilege-notification-on-error",
                b.notify_error,
            ),
            (
                "Notify on Watch Finish",
                "A banner when a watched command finishes",
                "privilege-notification-on-watch-finish",
                b.notify_watch,
            ),
            (
                "Notification \u{2014} Shell Controlled",
                "Programs may post banners (OSC 9, 777, 99)",
                "privilege-notification-shell",
                b.notify_shell,
            ),
        ] {
            rows.push(row(
                Shell,
                "Notification",
                title,
                Some(note),
                Control::Toggle {
                    key,
                    on,
                    invert: false,
                },
            ));
        }
        rows.push(row(
            Shell,
            "Notification",
            "Notify While Foreground",
            Some("Banners while Motty is the active app"),
            choice(
                "notification-foreground",
                &[
                    ("Off", "off"),
                    ("Always", "always"),
                    ("When the Tab Isn't Focused", "tab-unfocused"),
                ],
                match c.notification_foreground {
                    motty_theme::NotificationForeground::Off => "Off",
                    motty_theme::NotificationForeground::Always => "Always",
                    motty_theme::NotificationForeground::TabUnfocused => {
                        "When the Tab Isn't Focused"
                    }
                },
            ),
        ));
        for (title, note, key, on) in [
            (
                "Sound on Error Exit",
                "A beep when a command exits non-zero",
                "privilege-sound-on-error",
                b.sound_on_error,
            ),
            (
                "Sound \u{2014} Shell Controlled",
                "Programs may ring the bell (BEL)",
                "privilege-sound-shell",
                b.sound_shell,
            ),
        ] {
            rows.push(row(
                Shell,
                "Notification",
                title,
                Some(note),
                Control::Toggle {
                    key,
                    on,
                    invert: false,
                },
            ));
        }
        rows.push(row(
            Shell,
            "Tab Badge",
            "Badge When a Command Finishes",
            Some("A dot on a tab you weren't watching when its command succeeds, a triangle when it fails"),
            Control::Toggle {
                key: "privilege-badge-exit-status",
                on: c.badges.exit_status,
                invert: false,
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

        let b = &c.behavior;
        let toggle = |key: &'static str, on: bool| Control::Toggle {
            key,
            on,
            invert: false,
        };
        for (group, label, note, key, on) in [
            (
                "Mouse",
                "Mouse Over to Focus",
                "The pane under the pointer takes focus without a click",
                "focus-follows-mouse",
                b.focus_follows_mouse,
            ),
            (
                "Mouse",
                "Hide When Typing",
                "The pointer hides while you type and comes back when it moves",
                "mouse-hide-while-typing",
                b.hide_mouse_while_typing,
            ),
            (
                "Clipboard",
                "Copy on Select",
                "Every selection goes straight to the clipboard",
                "copy-on-select",
                b.copy_on_select,
            ),
            (
                "Clipboard",
                "Trim Trailing Spaces",
                "Copied lines lose their trailing spaces",
                "clipboard-trim-trailing-spaces",
                b.trim_trailing_spaces,
            ),
            (
                "Clipboard",
                "Paste Protection",
                "Ask before pasting several lines, a newline, sudo or control characters",
                "clipboard-paste-protection",
                b.paste_protection,
            ),
            (
                "Clipboard",
                "Trust Bracketed Paste",
                "Don't ask when the program takes bracketed paste: the text arrives inert",
                "clipboard-paste-bracketed-safe",
                b.paste_bracketed_safe,
            ),
            (
                "Selection",
                "Shift+Arrow Select",
                "⇧-arrows select from the cursor (⌥ for a block) instead of reaching the program",
                "shift-arrow-select",
                b.shift_arrow_select,
            ),
            (
                "Selection",
                "Clear Selection on Typing",
                "Any key sent to the program drops the selection",
                "selection-clear-on-typing",
                b.clear_selection_on_typing,
            ),
            (
                "Selection",
                "Clear Selection on Copy",
                "⌘C drops the selection (not with Copy on Select)",
                "selection-clear-on-copy",
                b.clear_selection_on_copy,
            ),
            (
                "Secure Input",
                "Auto Secure Input",
                "Secure Keyboard Entry while a password prompt waits (sudo, ssh, login)",
                "auto-secure-input",
                b.auto_secure_input,
            ),
            (
                "Secure Input",
                "Secure Input Indicator",
                "A titlebar pill while Secure Keyboard Entry is on",
                "secure-input-indication",
                b.secure_input_indication,
            ),
        ] {
            let section = if group == "Selection" {
                Selection
            } else {
                Controls
            };
            rows.push(row(section, group, label, Some(note), toggle(key, on)));
        }

        // Controls › Autocomplete
        let a = &c.autocomplete;
        rows.push(row(
            Controls,
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
            Controls,
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
            Controls,
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
            Controls,
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
            Controls,
            "Autocomplete",
            "Command History",
            Some("Forget every remembered command"),
            Control::Button("Clear…", Command::ClearHistory),
        ));

        // Editor
        rows.push(row(
            Editor,
            "Editor",
            "Show Line Numbers",
            Some("The line-number gutter in file editor tabs"),
            Control::Toggle {
                key: "editor-show-line-numbers",
                on: c.editor_line_numbers,
                invert: false,
            },
        ));

        // Agents
        // Nothing for these to act on until an agent reports in.
        let any = self.agents.iter().any(|a| a.installed);
        let b = c.badges;
        for (title, key, on) in [
            (
                "Badge While Processing",
                "privilege-badge-agent-processing",
                b.agent_processing,
            ),
            (
                "Badge When Task Completes",
                "privilege-badge-agent-task-complete",
                b.agent_complete,
            ),
            (
                "Badge When Awaiting Input",
                "privilege-badge-agent-awaiting-input",
                b.agent_awaiting,
            ),
            (
                "Notify When Task Completes",
                "privilege-notify-agent-task-complete",
                b.notify_complete,
            ),
            (
                "Notify When Awaiting Input",
                "privilege-notify-agent-awaiting-input",
                b.notify_awaiting,
            ),
            (
                "Prevent Sleep While Processing",
                "privilege-caffeinate-agent-processing",
                b.caffeinate,
            ),
            (
                "Resume Session on Recovery",
                "privilege-resume-agent-session",
                b.resume_session,
            ),
        ] {
            rows.push(
                row(
                    Agents,
                    "Agent Behavior",
                    title,
                    None,
                    Control::Toggle {
                        key,
                        on,
                        invert: false,
                    },
                )
                .dimmed(!any),
            );
        }
        for (ix, (agent, info)) in agents::CATALOG.iter().zip(&self.agents).enumerate() {
            let found = info.path.as_deref().map(tilde);
            rows.push(Row {
                section: Agents,
                group: "Code Agent Integration",
                title: agent.label.into(),
                description: Some(found.unwrap_or_else(|| "Not installed".into()).into()),
                control: Control::Agent {
                    ix,
                    installed: info.installed,
                    open: self.agent_open == Some(ix),
                },
                dim: false,
            });
        }
        let cli = crate::workspace::cli_command();
        rows.push(row(
            Integrations,
            "MCP Server",
            "Claude Code",
            Some("Lets the agent list panes, run commands in splits and read output"),
            Control::Copy(format!("claude mcp add motty -- {cli} mcp")),
        ));
        rows.push(row(
            Integrations,
            "MCP Server",
            "Codex",
            None,
            Control::Copy(format!("codex mcp add motty -- {cli} mcp")),
        ));
        for (ix, skill) in skills::SKILLS.iter().enumerate() {
            rows.push(row(
                Agents,
                "Skills",
                format!("/{}", skill.name),
                Some(skill.description),
                Control::Skill(ix),
            ));
        }

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
            View,
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

        rows.push(row(
            Appearance,
            "Tabs",
            "Auto Hide Tabs Panel",
            Some("Auto shows it once a window has two or more tabs"),
            choice(
                "auto-hide-tabs-panel",
                &[
                    ("Default", "default"),
                    ("Always", "always"),
                    ("Auto", "auto"),
                ],
                match c.auto_hide_tabs_panel {
                    motty_theme::AutoHide::Default => "Default",
                    motty_theme::AutoHide::Always => "Always",
                    motty_theme::AutoHide::Auto => "Auto",
                },
            ),
        ));
        rows.push(row(
            Appearance,
            "Tabs",
            "New Tab Position",
            None,
            choice(
                "new-tab-position",
                &[
                    ("Automatic", "auto"),
                    ("At the End", "end"),
                    ("After Current Tab", "after-current"),
                ],
                match c.new_tab_position {
                    motty_theme::NewTabPosition::Auto => "Automatic",
                    motty_theme::NewTabPosition::End => "At the End",
                    motty_theme::NewTabPosition::AfterCurrent => "After Current Tab",
                },
            ),
        ));

        for (label, note, key, on) in [
            (
                "Animate Icon on Progress",
                "A progress bar on the Dock icon while a command reports progress",
                "dock-icon-animate-progress",
                c.behavior.dock_progress,
            ),
            (
                "Red Icon on Error",
                "The Dock icon turns red when a command fails in a tab you haven't looked at",
                "dock-icon-error-badge",
                c.behavior.dock_error_badge,
            ),
        ] {
            rows.push(row(
                Appearance,
                "Dock Icon",
                label,
                Some(note),
                Control::Toggle {
                    key,
                    on,
                    invert: false,
                },
            ));
        }

        // Recipes
        rows.push(row(
            Recipes,
            "Recipes",
            "No Saved Recipes",
            Some("Saved commands, layouts and snippets are listed here"),
            Control::Info("None".into()),
        ));

        // Key bindings
        rows.push(row(
            KeyBindings,
            "Shortcuts",
            "Click a shortcut to change it",
            Some("Esc cancels, ⌫ removes it. Saved to config.toml."),
            Control::BindingTools,
        ));
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        for (ix, (group, title, action)) in key_binding_rows().into_iter().enumerate() {
            let caps = keymap
                .bindings_for_action(&*action)
                .last()
                .and_then(|b| b.keystrokes().first().map(|k| keycaps_of(k.inner())))
                .unwrap_or_default();
            let name = crate::actions::config_name(&*action);
            let defaults = crate::actions::default_chords(&*action);
            let custom = c.keybinds.iter().any(|k| {
                k.action.eq_ignore_ascii_case(&name)
                    || (k.action == "unbind"
                        && defaults.iter().any(|d| d.eq_ignore_ascii_case(&k.trigger)))
            });
            rows.push(row(
                KeyBindings,
                group,
                title,
                None,
                Control::Shortcut {
                    row: ix,
                    caps,
                    custom,
                },
            ));
        }
        drop(keymap);
        for (ix, (trigger, payload)) in sequences(c).into_iter().enumerate() {
            let title = if payload.starts_with("text:") {
                "Send Text"
            } else {
                "Send Sequence"
            };
            rows.push(row(
                KeyBindings,
                "Text / Sequence",
                title,
                None,
                Control::Sequence {
                    ix,
                    caps: trigger_caps(&trigger),
                    payload,
                },
            ));
        }

        // Advanced
        let path = self.config_path();
        let shown = tilde(&path);
        rows.push(Row {
            section: Advanced,
            group: "Config File",
            title: "config.toml".into(),
            description: Some(format!("{shown} · settings here are saved to it").into()),
            control: Control::Button("Open", Command::OpenConfig),
            dim: false,
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
        let access = |a: motty_theme::Access| match a {
            motty_theme::Access::Ask => "Ask",
            motty_theme::Access::Allow => "Allow",
            motty_theme::Access::Deny => "Deny",
        };
        let accesses = [("Ask", "ask"), ("Allow", "allow"), ("Deny", "deny")];
        for (group, title, note, key, on) in [
            (
                "Title",
                "Title \u{2014} Shell Controlled",
                "Programs may set the tab and window title (OSC 0, 2)",
                "privilege-title-shell",
                c.badges.title_shell,
            ),
            (
                "Clipboard",
                "Clipboard \u{2014} Shell Controlled",
                "Programs may use the clipboard at all (OSC 52)",
                "privilege-clipboard-shell",
                c.badges.clipboard_shell,
            ),
        ] {
            rows.push(row(
                Advanced,
                group,
                title,
                Some(note),
                Control::Toggle {
                    key,
                    on,
                    invert: false,
                },
            ));
        }
        rows.push(row(
            Advanced,
            "Clipboard",
            "Clipboard Read",
            Some("A program pasting from your clipboard"),
            choice("clipboard-read", &accesses, access(c.clipboard_read)),
        ));
        rows.push(row(
            Advanced,
            "Clipboard",
            "Clipboard Write",
            Some("A program copying to your clipboard"),
            choice("clipboard-write", &accesses, access(c.clipboard_write)),
        ));
        rows.push(row(
            Advanced,
            "Mouse",
            "Mouse \u{2014} Shell Controlled",
            Some("Programs may turn on mouse reporting"),
            Control::Toggle {
                key: "privilege-mouse-shell",
                on: c.badges.mouse_shell,
                invert: false,
            },
        ));
        rows.push(row(
            Advanced,
            "IPC",
            "IPC Allow Send Keys",
            Some("motty send and agents' skills may type into panes"),
            Control::Toggle {
                key: "ipc-allow-send-keys",
                on: b.ipc_send_keys,
                invert: false,
            },
        ));
        rows.push(row(
            Advanced,
            "IPC",
            "IPC Allow Sensitive Sessions",
            Some("…even into SSH sessions and password prompts"),
            Control::Toggle {
                key: "ipc-allow-sensitive-sessions",
                on: b.ipc_sensitive,
                invert: false,
            },
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
            div()
                .id(section.title())
                .h(px(NAV_HEIGHT))
                .mt(px(NAV_PITCH - NAV_HEIGHT))
                .pl(px(20.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    svg()
                        .path(section.icon())
                        .size(px(16.))
                        .text_color(c.secondary),
                )
                .child(div().text_color(c.text).child(section.title()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.show_section(section);
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
        // The selection slides from the last section to this one.
        let index = |s: Section| Section::ALL.iter().position(|x| *x == s).unwrap_or(0) as f32;
        let (from, to) = (index(self.previous), index(self.section));
        // Nothing moves when the window first opens.
        let animate = self.epoch > 0;
        let highlight = (!searching).then(|| {
            let bar = div()
                .absolute()
                .left_0()
                .right_0()
                .h(px(NAV_HEIGHT))
                .bg(c.selected)
                .top(px(NAV_TOP + NAV_PITCH * to));
            if !animate {
                return bar.into_any_element();
            }
            bar.with_animation(
                ("nav-highlight", self.epoch),
                Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
                move |el, t| el.top(px(NAV_TOP + NAV_PITCH * (from + (to - from) * t))),
            )
            .into_any_element()
        });
        let sidebar = div()
            .relative()
            .w(px(SIDEBAR))
            .flex_none()
            .h_full()
            .bg(c.sidebar)
            .border_r(px(1.))
            .border_color(c.separator)
            .flex()
            .flex_col()
            .children(highlight)
            .child(search)
            .children(nav);

        // Groups in the order they first come, each with all its rows.
        let mut groups: Vec<(String, Vec<Row>)> = Vec::new();
        for r in shown {
            let name = if searching {
                format!("{} · {}", r.section.title(), r.group)
            } else {
                r.group.to_owned()
            };
            match groups.iter_mut().find(|(g, _)| *g == name) {
                Some((_, rows)) => rows.push(r),
                None => groups.push((name, vec![r])),
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
            .child({
                let body = div().flex().flex_col().children(content);
                if animate {
                    // A new section rises into place as it fades in.
                    body.with_animation(
                        ("section", self.epoch),
                        Animation::new(Duration::from_millis(240)).with_easing(ease_out_quint()),
                        |el, t| el.opacity(t).mt(px(12. * (1. - t))),
                    )
                    .into_any_element()
                } else {
                    body.into_any_element()
                }
            })
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

        let dialog = self
            .skill
            .as_ref()
            .map(|d| self.render_skill_dialog(d, &c, cx));
        div()
            .id("settings")
            .key_context("Settings")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .relative()
            .size_full()
            .flex()
            .bg(c.content)
            .text_size(px(13.))
            .font_family(".SystemUIFont")
            .text_color(c.text)
            .child(sidebar)
            .child(main)
            .children(dialog)
    }
}

impl SettingsWindow {
    /// Puts the skill's installer prompt on the clipboard, and saves the
    /// typing switch when the dialog showed it.
    fn copy_skill(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.skill.take() else {
            return;
        };
        let skill = &skills::SKILLS[dialog.ix];
        let name = match dialog.name.trim() {
            "" => skill.name,
            name => name,
        };
        cx.write_to_clipboard(ClipboardItem::new_string(skills::installer(skill, name)));
        if skill.types && dialog.allow != self.config.badges.ipc_send_keys {
            self.apply(
                "ipc-allow-send-keys",
                Some(Value::Boolean(dialog.allow)),
                cx,
            );
        }
        self.status = Some((
            true,
            format!("Copied. Paste it into your coding agent and send; then type /{name}."),
        ));
        cx.notify();
    }

    fn render_skill_dialog(
        &self,
        d: &SkillDialog,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let skill = &skills::SKILLS[d.ix];
        let name = match d.name.trim() {
            "" => skill.name,
            name => name,
        };
        let usage = format!("/{name}{}", &skill.usage[skill.name.len() + 1..]);
        let label = |text: &'static str| {
            div()
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c.header)
                .child(text)
        };
        let button = |id: &'static str, text: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px(px(14.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .when(primary, |d| {
                    d.bg(c.accent).text_color(hsla(Rgba::rgb(0xffffff)))
                })
                .when(!primary, |d| {
                    d.bg(c.control)
                        .border(px(0.5))
                        .border_color(c.control_border)
                })
                .shadow_xs()
                .child(text)
        };
        let field = div()
            .h(px(30.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(c.field)
            .border(px(1.))
            .border_color(c.accent)
            .child(div().text_color(c.secondary).child("/"))
            .child(SharedString::from(d.name.clone()))
            .child(div().w(px(1.5)).h(px(15.)).bg(c.accent));
        let (track, knob_left) = if d.allow {
            (c.accent, 18.)
        } else {
            (c.switch_off, 2.)
        };
        let switch = skill.types.then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child("Allow Agents to Type into Panes")
                        .child(div().text_size(px(11.)).text_color(c.secondary).child(
                            "Without it, every send is refused. SSH sessions and \
                                     password prompts refuse typing either way.",
                        )),
                )
                .child(
                    div()
                        .id("skill-allow")
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
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(d) = this.skill.as_mut() {
                                d.allow = !d.allow;
                            }
                            cx.notify();
                        })),
                )
        });
        let card = div()
            .id("skill-dialog")
            .occlude()
            .w(px(420.))
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .rounded(px(12.))
            .bg(c.card)
            .border(px(1.))
            .border_color(c.separator)
            .shadow_lg()
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(SharedString::from(format!("Install /{}", skill.name))),
            )
            .child(div().text_color(c.secondary).child(
                "Copy puts an installer prompt on the clipboard. Paste it into your \
                         coding agent and send: the agent writes the skill file itself.",
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(label("NAME"))
                    .child(field)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(c.secondary)
                            .child("Rename it if you already have a skill by this name."),
                    ),
            )
            .children(switch)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(label("ONCE INSTALLED, TYPE"))
                    .child(
                        div()
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(px(6.))
                            .bg(c.field)
                            .font_family("Menlo")
                            .text_size(px(12.))
                            .child(SharedString::from(usage)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("skill-cancel", "Cancel", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.skill = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        button("skill-copy", "Copy", true)
                            .on_click(cx.listener(|this, _, _, cx| this.copy_skill(cx))),
                    ),
            );
        div()
            .id("skill-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(Rgba::rgb(0x000000).with_alpha(0.25)))
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| {
                this.skill = None;
                cx.notify();
            }))
            .child(card)
            .into_any_element()
    }
}

impl SettingsWindow {
    /// Adds or removes Motty's hooks in an agent's settings.
    fn switch_agent(&mut self, ix: usize, on: bool, cx: &mut Context<Self>) {
        let agent = &agents::CATALOG[ix];
        let env = hooks::Env::current();
        let result = if on {
            hooks::install(agent.id, &env, &hooks::Cli::current())
        } else {
            hooks::uninstall(agent.id, &env)
        };
        self.status = Some(match result {
            Ok(path) if on => (
                true,
                format!(
                    "Added Motty's hooks to {}. Restart {} once to connect it.",
                    tilde(&path),
                    agent.label
                ),
            ),
            Ok(path) => (
                true,
                format!("Removed Motty's hooks from {}.", tilde(&path)),
            ),
            Err(e) => (false, e),
        });
        self.agents = scan_agents();
        cx.notify();
    }

    /// What an agent's switch writes, under its row.
    fn agent_details(&self, ix: usize, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let agent = &agents::CATALOG[ix];
        let env = hooks::Env::current();
        let path = hooks::target(agent.id, &env);
        let plugin = matches!(agent.id, "opencode" | "pi" | "omp" | "copilot");
        let what = if plugin {
            "Motty's own file there, deleted again when the switch is off."
        } else {
            "Hooks that run motty hook; nothing else in the file is touched, and switching off takes them out again."
        };
        let line = |text: String, mono: bool| {
            div()
                .text_size(px(11.))
                .text_color(c.secondary)
                .when(mono, |d| d.font_family("Menlo"))
                .child(SharedString::from(text))
        };
        div()
            .ml(px(38.))
            .mb(px(10.))
            .p(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .rounded(px(7.))
            .bg(c.field)
            .child(line("Writes to".into(), false))
            .children(path.as_ref().map(|p| line(tilde(p), true)))
            .child(line(what.into(), false))
            .children(hooks::note(agent.id).map(|n| line(n.into(), false)))
            .children(path.filter(|p| p.exists()).map(|p| {
                div()
                    .id(("reveal-agent", ix))
                    .mt(px(2.))
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .flex_none()
                    .self_start()
                    .items_center()
                    .rounded(px(6.))
                    .bg(c.control)
                    .border(px(0.5))
                    .border_color(c.control_border)
                    .text_size(px(12.))
                    .child("Reveal in Finder")
                    .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&p)))
            }))
            .into_any_element()
    }

    /// Key caps that record a new chord when clicked.
    fn recorder(
        &self,
        id: (&'static str, usize),
        caps: Vec<SharedString>,
        recording: bool,
        target: Record,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent = hsla(Rgba::rgb(0x047afe));
        div()
            .id(id)
            .min_w(px(76.))
            .h(px(26.))
            .px(px(4.))
            .flex()
            .items_center()
            .justify_end()
            .gap(px(4.))
            .rounded(px(6.))
            .border(px(if recording { 1.5 } else { 0. }))
            .border_color(accent)
            .hover(|s| s.bg(c.selected))
            .when(recording, |d| {
                d.justify_center()
                    .px(px(8.))
                    .text_size(px(12.))
                    .text_color(accent)
                    .child("Type shortcut…")
            })
            .when(!recording && caps.is_empty(), |d| {
                d.text_color(c.secondary).pr(px(8.)).child("—")
            })
            .when(!recording, |d| {
                d.children(caps.into_iter().map(|cap| {
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
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.editing = None;
                this.recording = if this.recording == Some(target) {
                    None
                } else {
                    Some(target)
                };
                window.focus(&this.focus, cx);
                cx.notify();
            }))
    }

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
        let details = match &r.control {
            Control::Agent { ix, open: true, .. } => Some(self.agent_details(*ix, c, cx)),
            _ => None,
        };
        let icon = match &r.control {
            Control::Agent { ix, .. } => Some(agent_mark(*ix)),
            _ => None,
        };
        let dim = r.dim;
        let control = self.render_control(ix, r.control, c, cx);
        // Greyed out: the control is covered so it takes no clicks.
        let control = div().flex_none().relative().child(control).when(dim, |d| {
            d.child(div().id(("dim", ix)).absolute().inset_0().occlude())
        });
        let line = div()
            .min_h(px(ROW_MIN))
            .py(px(8.))
            .flex()
            .items_center()
            .gap(px(16.))
            .when(dim, |d| d.opacity(0.45))
            .children(icon)
            .child(label)
            .child(control);
        div()
            .mx(px(15.))
            .when(separator, |d| d.border_b(px(1.)).border_color(c.separator))
            .child(line)
            .children(details)
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
            Control::Skill(skill) => div()
                .id(("skill", ix))
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
                .child("Copy")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.skill = Some(SkillDialog {
                        ix: skill,
                        name: skills::SKILLS[skill].name.to_owned(),
                        allow: this.config.badges.ipc_send_keys || skills::SKILLS[skill].types,
                    });
                    cx.notify();
                }))
                .into_any_element(),
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
            Control::Shortcut { row, caps, custom } => {
                let recording = self.recording == Some(Record::Action(row));
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when(custom, |d| {
                        d.child(
                            div()
                                .id(("reset", ix))
                                .px(px(6.))
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .rounded(px(5.))
                                .text_size(px(11.))
                                .text_color(c.secondary)
                                .hover(|s| s.bg(c.selected))
                                .child("Reset")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let Some((.., action)) =
                                        key_binding_rows().into_iter().nth(row)
                                    else {
                                        return;
                                    };
                                    let defaults = crate::actions::default_chords(&*action);
                                    this.set_shortcut(row, defaults, cx);
                                })),
                        )
                    })
                    .child(self.recorder(
                        ("shortcut", ix),
                        caps,
                        recording,
                        Record::Action(row),
                        c,
                        cx,
                    ))
                    .into_any_element()
            }
            Control::Sequence {
                ix: seq,
                caps,
                payload,
            } => {
                let recording = self.recording == Some(Record::Sequence(seq));
                let editing = self
                    .editing
                    .as_ref()
                    .filter(|(i, _)| *i == seq)
                    .map(|(_, t)| t.clone());
                let shown = editing.clone().unwrap_or_else(|| payload.clone());
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(self.recorder(
                        ("seq-keys", ix),
                        caps,
                        recording,
                        Record::Sequence(seq),
                        c,
                        cx,
                    ))
                    .child(
                        div()
                            .id(("seq-text", ix))
                            .w(px(150.))
                            .h(px(26.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .bg(c.control)
                            .border(px(if editing.is_some() { 1.5 } else { 0.5 }))
                            .border_color(if editing.is_some() {
                                hsla(Rgba::rgb(0x047afe))
                            } else {
                                c.control_border
                            })
                            .font_family("Menlo")
                            .text_size(px(11.5))
                            .child(div().min_w_0().truncate().child(SharedString::from(shown)))
                            .when(editing.is_some(), |d| {
                                d.child(div().w(px(1.5)).h(px(14.)).bg(hsla(Rgba::rgb(0x047afe))))
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.recording = None;
                                this.editing = Some((seq, payload.clone()));
                                window.focus(&this.focus, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(("seq-delete", ix))
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .text_color(c.secondary)
                            .hover(|s| s.bg(c.selected))
                            .child("✕")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let old = sequences(&this.config).get(seq).cloned();
                                this.editing = None;
                                this.set_sequence(old, None, cx);
                            })),
                    )
                    .into_any_element()
            }
            Control::BindingTools => {
                let adding = self.recording == Some(Record::NewSequence);
                div()
                    .flex_none()
                    .flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .id(("add-sequence", ix))
                            .h(px(26.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .bg(c.control)
                            .border(px(0.5))
                            .border_color(c.control_border)
                            .shadow_xs()
                            .child(if adding {
                                "Press keys…"
                            } else {
                                "+ Text Shortcut"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editing = None;
                                this.recording = Some(Record::NewSequence);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(("reset-all", ix))
                            .h(px(26.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .bg(c.control)
                            .border(px(0.5))
                            .border_color(c.control_border)
                            .shadow_xs()
                            .when(self.confirm_reset, |d| d.text_color(c.error))
                            .child(if self.confirm_reset {
                                "Reset All?"
                            } else {
                                "Reset to Default"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.confirm_reset {
                                    this.confirm_reset = true;
                                    return cx.notify();
                                }
                                this.confirm_reset = false;
                                match motty_theme::config::edit(
                                    motty_theme::config::reset_keybinds_in,
                                ) {
                                    Ok(_) => {
                                        this.status = Some((
                                            true,
                                            "Every shortcut is back to its default.".into(),
                                        ));
                                        this.after_keybind_save(cx);
                                    }
                                    Err(e) => {
                                        this.status =
                                            Some((false, format!("Couldn't save config.toml: {e}")))
                                    }
                                }
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }
            Control::Agent {
                ix: agent,
                installed,
                open,
            } => {
                let (track, knob_left) = if installed {
                    (c.accent, 18.)
                } else {
                    (c.switch_off, 2.)
                };
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .when(installed, |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(hsla(Rgba::rgb(0x34c759)))
                                .child("Installed"),
                        )
                    })
                    .child(
                        div()
                            .id(("agent", ix))
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
                                this.switch_agent(agent, !installed, cx)
                            })),
                    )
                    .child(
                        div()
                            .id(("agent-details", ix))
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .hover(|s| s.bg(c.selected))
                            .child(
                                svg()
                                    .path("icons/chevron-down.svg")
                                    .size(px(13.))
                                    .text_color(c.secondary)
                                    .with_transformation(gpui::Transformation::rotate(
                                        gpui::radians(if open {
                                            0.
                                        } else {
                                            -std::f32::consts::FRAC_PI_2
                                        }),
                                    )),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.agent_open = (this.agent_open != Some(agent)).then_some(agent);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }
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
