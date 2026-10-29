//! Window root: the tab sidebar, the titlebar band, and the themed container
//! around the active tab's panes.

use std::collections::HashMap;

use anyhow::Context as _;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, BoxShadow, Context, Entity, EntityId, FocusHandle,
    Focusable, FontWeight, Hsla, MouseButton, SharedString, Subscription, Task, Transformation,
    Window, WindowAppearance, WindowBackgroundAppearance, div, percentage, point, prelude::*, px,
    svg,
};
use motty_render::{TerminalStyle, hsla};
use motty_term::alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use motty_term::{GridSize, Session, SessionConfig, SessionEvent};
use motty_theme::{Border, Config, Material, Mode, Rgba, Shadow, Theme};

use crate::actions::*;
use crate::agents::Badge;
use crate::editor::FileEditor;
use crate::materials::{Backdrop, Backdrops, Region};
use crate::panes::{Direction, Pane, PaneStyle};
use crate::recovery::{PaneState, TabState, WindowState};
use crate::terminal_view::PaneMode;
use crate::terminal_view::{TerminalEvent, TerminalView};
use crate::titlebar;

mod agents;
mod closing;
mod details;
mod files;
mod global_search;
mod menu_actions;
pub(crate) use new_menu::{shells, ssh_hosts};
mod new_menu;
mod open_quickly;
mod palette;
mod status;
mod tabs;
mod welcome;

pub(crate) use agents::Place;
pub(crate) use details::{listening_ports, open_in, open_with_apps, web_url};
pub(crate) use palette::keycaps;
pub(crate) use tabs::MovedTab;
pub use welcome::first_run;
pub(crate) use welcome::{cli_command, install_cli};
mod picker;

/// The native titlebar height that the sidebar layout reserves.
const TITLEBAR_RESERVE: f32 = 28.;
/// The title is centered in a band slightly taller than the reserve, level
/// with the traffic lights.
const TITLE_BAND: f32 = 32.;
/// Space kept free at the right of the title band (toolbar buttons).
const TITLE_RIGHT_INSET: f32 = 66.;
/// With the sidebar hidden, Otty centers the title between the window's
/// left edge and this far from its right edge.
const COLLAPSED_TITLE_RIGHT_INSET: f32 = 16.;
/// Room a mode pill takes from the title band's right end (the widest,
/// HINTS with "Esc Exit", is about 130pt, from 43.5pt in).
const PILL_INTRUSION: f32 = 130. + 43.5 - TITLE_RIGHT_INSET;
/// Top of the "TABS" header row and the first tab card.
const SIDEBAR_HEADER_TOP: f32 = 37.5;
/// What dragging the sidebar's edge allows, and how wide the grip is.
const SIDEBAR_MIN: f32 = 160.;
const SIDEBAR_MAX: f32 = 480.;
const SIDEBAR_GRAB: f32 = 6.;
const SIDEBAR_HEADER_HEIGHT: f32 = 22.5;
const TAB_HEIGHT: f32 = 36.;
const TAB_GAP: f32 = 2.;
/// The header's text and icon sit this far inside the sidebar's padding.
const SIDEBAR_TEXT_INSET: f32 = 10.;
const SIDEBAR_ICON_INSET: f32 = 8.5;
/// Process label beside each tab title: small mono text with wide tracking.
const LABEL_FONT_SIZE: f32 = 9.5;
/// How often config and theme files are checked for changes.
const CONFIG_POLL: Duration = Duration::from_millis(500);
const LABEL_PITCH: f32 = 6.25;
const LABEL_COLUMNS: f32 = 6.;

/// Tabs closed lately, newest last, so ⇧⌘T in any window brings one back
/// (even from a window that closed with it).
#[derive(Default)]
struct ClosedTabs(Vec<ClosedTab>);

impl gpui::Global for ClosedTabs {}

struct ClosedTab {
    /// Unique for the run, so Open Quickly can reopen this one.
    id: u64,
    title: String,
    cwd: Option<std::path::PathBuf>,
    state: TabState,
    /// Where it was in its window's tab list.
    index: usize,
    /// Each pane's scrollback; panes name theirs by index.
    scrollback: Vec<Vec<u8>>,
}

const MAX_CLOSED_TABS: usize = 10;
/// A sidebar divider sits this far inside the cards' edges, with this much
/// room (besides the gap between tabs) above and below.
const DIVIDER_INSET: f32 = 8.;
const DIVIDER_SPACE: f32 = 4.;

/// A sidebar tab: terminals in splits, or a file editor.
pub(crate) struct Tab {
    content: Content,
    /// A divider follows this tab in the sidebar.
    divider: bool,
    /// Last opened, looked at or reported on: Inbox orders by it.
    updated: std::time::SystemTime,
    /// When it was opened: the tabs panel's "Created Time" order.
    created: std::time::SystemTime,
    /// Set with Rename Tab: replaces the title, or comes before it.
    name: Option<tabs::TabName>,
    /// Set with Mark Tab, shown until looked at (or Clear Badge).
    mark: Option<Badge>,
    /// The title menu's Notifications & Privileges, when changed for this
    /// tab; config.toml's otherwise.
    privileges: Option<motty_theme::Badges>,
}

enum Content {
    Terminals(Terminals),
    Editor(Entity<FileEditor>),
}

struct Terminals {
    root: Pane,
    /// The pane that has (or last had) focus.
    focused: Entity<TerminalView>,
    /// Showing only the focused pane.
    zoomed: bool,
}

impl Tab {
    fn new(view: Entity<TerminalView>) -> Self {
        Self {
            content: Content::Terminals(Terminals {
                root: Pane::Leaf(view.clone()),
                focused: view,
                zoomed: false,
            }),
            divider: false,
            updated: std::time::SystemTime::now(),
            created: std::time::SystemTime::now(),
            name: None,
            mark: None,
            privileges: None,
        }
    }

    fn editor(editor: Entity<FileEditor>) -> Self {
        Self {
            content: Content::Editor(editor),
            divider: false,
            updated: std::time::SystemTime::now(),
            created: std::time::SystemTime::now(),
            name: None,
            mark: None,
            privileges: None,
        }
    }

    /// The tab's terminals, unless it's an editor.
    fn terms(&self) -> Option<&Terminals> {
        match &self.content {
            Content::Terminals(t) => Some(t),
            Content::Editor(_) => None,
        }
    }

    fn terms_mut(&mut self) -> Option<&mut Terminals> {
        match &mut self.content {
            Content::Terminals(t) => Some(t),
            Content::Editor(_) => None,
        }
    }

    fn as_editor(&self) -> Option<&Entity<FileEditor>> {
        match &self.content {
            Content::Editor(e) => Some(e),
            Content::Terminals(_) => None,
        }
    }
}

pub struct Workspace {
    config: Config,
    theme: Theme,
    mode: Mode,
    tabs: Vec<Tab>,
    active: usize,
    focus_handle: FocusHandle,
    ui_font: SharedString,
    mono_font: SharedString,
    /// Last title given to the native window (shown in Mission Control, menus).
    window_title: String,
    /// The command palette, while open.
    palette: Option<palette::Palette>,
    /// Open Quickly, while open.
    open_quickly: Option<open_quickly::OpenQuickly>,
    global: Option<global_search::GlobalSearch>,
    /// The close was confirmed (or needed no asking): the window may go.
    closing_window: bool,
    /// The welcome guide, while open.
    welcome: Option<welcome::Welcome>,
    backdrops: Backdrops,
    /// Last background appearance and edited flag given to the native window;
    /// both reach AppKit on every call, so they are only set on change.
    native_state: Option<(WindowBackgroundAppearance, bool)>,
    /// The tabs sidebar (`sidebar-visible`, ⌘⇧L).
    sidebar_visible: bool,
    /// The details panel (⌘⇧R) and what it shows.
    details: details::Details,
    /// The pointer is over the window: Otty shows its titlebar buttons then.
    pointer_inside: bool,
    /// The new-tab menu is open; its button stays pressed.
    new_menu_open: bool,
    /// The bell turned the sidebar into Inbox.
    inbox: bool,
    /// The tabs panel's Group and Order (its filter button).
    grouping: tabs::Grouping,
    order: tabs::Order,
    /// A tab's name being typed.
    renaming: Option<tabs::Rename>,
    /// View › Pin Window: above other apps' windows.
    pinned: bool,
    _subscriptions: Vec<Subscription>,
    /// Each terminal's event subscription, dropped with its pane.
    pane_subscriptions: HashMap<EntityId, Subscription>,
    /// Ids for new splits.
    next_split: usize,
    /// The split whose divider is being dragged.
    dragging: Option<usize>,
    /// Dragging the sidebar's edge: its width meanwhile.
    sidebar_drag: Option<f32>,
    /// Where the window title was last drawn, for the title menu.
    title_bounds: Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    title_menu_open: bool,
    _process_poll: Task<()>,
    _config_poll: Task<()>,
}

pub fn mode_for(appearance: WindowAppearance) -> Mode {
    match appearance {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Mode::Dark,
        WindowAppearance::Light | WindowAppearance::VibrantLight => Mode::Light,
    }
}

/// Spawns the user's shell for a new pane in `cwd` (home when `None`).
/// `command` (from `-e`) replaces the shell.
fn spawn_shell(
    config: &Config,
    command: Option<(String, Vec<String>)>,
    cwd: Option<std::path::PathBuf>,
    preload: Option<Vec<u8>>,
    size: GridSize,
    pane_id: u64,
) -> anyhow::Result<(Session, async_channel::Receiver<SessionEvent>)> {
    // Commands from `-e` run as they are; the user's shell gets integration.
    let crate::shell_integration::Launch {
        mut env,
        command: shell_command,
    } = if command.is_none() {
        crate::shell_integration::launch(config)
    } else {
        Default::default()
    };
    // Programs (and agents) in the pane can drive Motty through `motty`.
    env.insert(motty_ipc::PANE_ENV.into(), pane_id.to_string());
    if let Some(dir) = cli_dir() {
        let path = std::env::var("PATH").unwrap_or_default();
        env.insert("PATH".into(), format!("{}:{path}", dir.display()));
    }
    if let Some(socket) = crate::ipc::socket() {
        env.insert(motty_ipc::SOCKET_ENV.into(), socket.display().to_string());
    }
    let session = SessionConfig {
        command: command
            .or(shell_command)
            .or_else(|| config.shell.clone().map(|s| (s, vec!["-l".into()]))),
        env,
        working_directory: cwd.or_else(dirs::home_dir),
        scrollback: config.scrollback.unwrap_or(10_000),
        cursor_style: cursor_style(config),
        preload,
    };
    Session::spawn(session, size)
}

/// Where the `motty` CLI is: the bundle's Resources/bin, or beside the app
/// binary in a cargo build.
/// The `motty` CLI inside this build, if it's there.
pub fn cli_path() -> Option<std::path::PathBuf> {
    cli_dir().map(|d| d.join("motty"))
}

/// Re-reads the config in every window now, rather than at the next poll.
pub fn reload_all(cx: &mut App) {
    for window in cx.windows() {
        if let Some(w) = window.downcast::<Workspace>() {
            let _ = w.update(cx, |ws, _, cx| ws.reload_config(cx));
        }
    }
}

fn cli_dir() -> Option<&'static std::path::Path> {
    static DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        [exe_dir.join("../Resources/bin"), exe_dir]
            .into_iter()
            .find(|d| d.join("motty").is_file())
            .and_then(|d| d.canonicalize().ok())
    })
    .as_deref()
}

/// `cursor-style` and the blink policy's starting state, as the terminal's
/// default cursor.
pub fn cursor_style(config: &Config) -> CursorStyle {
    let shape = match config.cursor_style {
        motty_theme::CursorStyle::Block => CursorShape::Block,
        motty_theme::CursorStyle::Bar => CursorShape::Beam,
        motty_theme::CursorStyle::Underline => CursorShape::Underline,
        motty_theme::CursorStyle::BlockHollow => CursorShape::HollowBlock,
    };
    CursorStyle {
        shape,
        blinking: config.cursor_blink.initial(),
    }
}

pub fn terminal_style(config: &Config, theme: &Theme, cx: &App) -> TerminalStyle {
    let family = crate::fonts::terminal_family(config, theme, cx);
    TerminalStyle::from_theme(theme, Some(&family), config.font_size, config.ligatures)
}

impl Workspace {
    pub fn new(
        config: Config,
        command: Option<(String, Vec<String>)>,
        cwd: Option<std::path::PathBuf>,
        restore: Option<&WindowState>,
        moved: Option<MovedTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mode = mode_for(window.appearance());
        let parsed = config.resolve_theme(mode);
        for w in config.warnings.iter().chain(&parsed.warnings) {
            tracing::warn!("{w}");
        }
        let theme = parsed.theme;
        crate::native_menu::follow_theme(theme.meta.mode == motty_theme::Mode::Dark);
        let ui_font = crate::fonts::ui_family(&theme.token.font_ui, cx).into();
        let mono_font = crate::fonts::terminal_family(&config, &theme, cx).into();

        let subscriptions = vec![
            cx.observe_window_appearance(window, |this, window, cx| {
                let mode = mode_for(window.appearance());
                if mode != this.mode {
                    this.mode = mode;
                    this.reload_theme(cx);
                }
            }),
            // The menus describe the frontmost window.
            cx.observe_window_activation(window, |_, window, cx| {
                crate::secure_input::set_active(window.is_window_active(), cx);
                if window.is_window_active() {
                    cx.defer(crate::menus::refresh);
                }
            }),
        ];
        // The close button asks first when something is still running.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |this, cx| this.should_close(window, cx))
                .unwrap_or(true)
        });
        let process_poll = cx.spawn(async move |this, cx| {
            let mut tick = 0u32;
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| {
                    let mut changed = false;
                    for tab in this.tabs.iter().filter_map(Tab::terms) {
                        for view in tab.root.leaves() {
                            changed |= view.update(cx, |t, _| t.refresh_process());
                        }
                    }
                    if changed {
                        cx.notify();
                        cx.defer(refresh_dock);
                    }
                    this.refresh_details(false, cx);
                    // Now and then for what the menus show: ports, git.
                    tick += 1;
                    if tick.is_multiple_of(5) {
                        cx.defer(crate::menus::refresh);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        let config_poll = cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let fingerprint = || executor.spawn(async { motty_theme::config::files_fingerprint() });
            let mut seen = fingerprint().await;
            loop {
                executor.timer(CONFIG_POLL).await;
                let now = fingerprint().await;
                if now == seen {
                    continue;
                }
                seen = now;
                if this.update(cx, |this, cx| this.reload_config(cx)).is_err() {
                    break;
                }
            }
        });

        let sidebar_visible = config.sidebar_visible;

        let mut this = Self {
            config,
            theme,
            mode,
            tabs: Vec::new(),
            active: 0,
            focus_handle: cx.focus_handle(),
            ui_font,
            mono_font,
            window_title: String::new(),
            palette: None,
            open_quickly: None,
            global: None,
            closing_window: false,
            welcome: None,
            details: Default::default(),
            backdrops: Backdrops::default(),
            native_state: None,
            sidebar_visible,
            pointer_inside: false,
            new_menu_open: false,
            inbox: false,
            grouping: Default::default(),
            order: Default::default(),
            renaming: None,
            pinned: false,
            _subscriptions: subscriptions,
            pane_subscriptions: HashMap::new(),
            next_split: 0,
            dragging: None,
            sidebar_drag: None,
            title_bounds: Rc::default(),
            title_menu_open: false,
            _process_poll: process_poll,
            _config_poll: config_poll,
        };
        if let Some(state) = restore {
            this.restore(state, window, cx);
        }
        if let Some(moved) = moved {
            this.adopt(moved, window, cx);
        }
        if this.tabs.is_empty()
            && let Err(e) = this.add_tab(command, cwd, window, cx)
        {
            tracing::error!("failed to start shell: {e:#}");
        }
        this
    }

    /// What session recovery saves of this window. With `scrollback_dir`,
    /// each pane's scrollback is written there too (`files` numbers them).
    pub fn snapshot(
        &self,
        window: &Window,
        scrollback_dir: Option<&std::path::Path>,
        files: &mut usize,
        cx: &App,
    ) -> WindowState {
        let mut write = |bytes: Vec<u8>| {
            let dir = scrollback_dir?;
            *files += 1;
            let name = format!("{files}.ansi");
            std::fs::write(dir.join(&name), bytes).ok()?;
            Some(name)
        };
        let b = window.bounds();
        WindowState {
            bounds: [
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            ],
            sidebar_visible: self.sidebar_visible,
            details_visible: self.details.visible,
            grouping: self.grouping.key().to_owned(),
            order: self.order.key().to_owned(),
            // Editor tabs aren't kept: their files are on disk.
            active: self.tabs[..self.active.min(self.tabs.len())]
                .iter()
                .filter(|t| t.terms().is_some())
                .count(),
            tabs: self
                .tabs
                .iter()
                .filter_map(|tab| {
                    let write = scrollback_dir.is_some().then_some(&mut write as _);
                    tab_state(tab, write, cx)
                })
                .collect(),
        }
    }

    /// Reopens a saved window's tabs and splits, each pane in its folder
    /// with its scrollback.
    fn restore(&mut self, state: &WindowState, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = state.sidebar_visible;
        self.details.visible = state.details_visible;
        self.grouping = tabs::Grouping::from_key(&state.grouping);
        self.order = tabs::Order::from_key(&state.order);
        for tab in &state.tabs {
            match self.restore_tab(tab, &crate::recovery::read_scrollback, window, cx) {
                Ok(tab) => self.tabs.push(tab),
                Err(e) => tracing::error!("couldn't restore a tab: {e:#}"),
            }
        }
        if !self.tabs.is_empty() {
            self.activate(state.active.min(self.tabs.len() - 1), window, cx);
        }
    }

    /// A tab rebuilt from `state`; `scrollback` looks up each pane's saved
    /// scrollback by name.
    fn restore_tab(
        &mut self,
        state: &TabState,
        scrollback: &dyn Fn(&str) -> Option<Vec<u8>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Tab> {
        let root = self.restore_pane(&state.root, [0., 0., 1., 1.], scrollback, window, cx)?;
        let leaves: Vec<_> = root.leaves().into_iter().cloned().collect();
        let focused = leaves
            .get(state.focused)
            .or(leaves.first())
            .cloned()
            .expect("a pane tree has a leaf");
        Ok(Tab {
            content: Content::Terminals(Terminals {
                root,
                focused,
                zoomed: state.zoomed,
            }),
            divider: state.divider,
            updated: std::time::SystemTime::now(),
            created: std::time::SystemTime::now(),
            name: state.name.clone().map(|text| tabs::TabName {
                text,
                prefix: state.name_prefix,
            }),
            mark: None,
            privileges: None,
        })
    }

    fn restore_pane(
        &mut self,
        state: &PaneState,
        rect: [f32; 4],
        scrollback: &dyn Fn(&str) -> Option<Vec<u8>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Pane> {
        Ok(match state {
            PaneState::Leaf {
                cwd,
                scrollback: saved,
                agent,
                queue,
            } => {
                let cwd = cwd.clone().filter(|d| d.is_dir());
                let preload = saved.as_deref().and_then(scrollback);
                let view = self.new_terminal_with(None, cwd, preload, rect, window, cx)?;
                // Privilege: Resume Session on Recovery.
                let resume = agent.as_ref().filter(|_| self.config.badges.resume_session);
                if let Some(line) = resume
                    .and_then(|[agent, id]| crate::agents::sessions::resume_command(agent, id))
                {
                    let line = format!("{line}\r");
                    view.update(cx, |v, cx| v.write_input(&line, cx));
                }
                if !queue.is_empty() {
                    let queue = queue.clone();
                    view.update(cx, |v, cx| v.enqueue(queue, window, cx));
                }
                Pane::Leaf(view)
            }
            PaneState::Split {
                vertical,
                ratio,
                first,
                second,
            } => {
                let ratio = ratio.clamp(0.05, 0.95);
                let [x, y, w, h] = rect;
                let (a, b) = if *vertical {
                    (
                        [x, y, w, h * ratio],
                        [x, y + h * ratio, w, h * (1. - ratio)],
                    )
                } else {
                    (
                        [x, y, w * ratio, h],
                        [x + w * ratio, y, w * (1. - ratio), h],
                    )
                };
                let first = self.restore_pane(first, a, scrollback, window, cx)?;
                let second = match self.restore_pane(second, b, scrollback, window, cx) {
                    Ok(pane) => pane,
                    // Keep what started.
                    Err(e) => {
                        tracing::error!("couldn't restore a pane: {e:#}");
                        return Ok(first);
                    }
                };
                let id = self.next_split;
                self.next_split += 1;
                Pane::Split(crate::panes::Split {
                    id,
                    axis: if *vertical {
                        gpui::Axis::Vertical
                    } else {
                        gpui::Axis::Horizontal
                    },
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                    bounds: Default::default(),
                })
            }
        })
    }

    /// A terminal view for `session`, closing its pane when the shell exits.
    /// Starts a shell (or `command`) in `cwd` for a pane at `rect` (a share
    /// of the container), closing the pane when it exits.
    fn new_terminal(
        &mut self,
        command: Option<(String, Vec<String>)>,
        cwd: Option<std::path::PathBuf>,
        rect: [f32; 4],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Entity<TerminalView>> {
        self.new_terminal_with(command, cwd, None, rect, window, cx)
    }

    /// [`Self::new_terminal`], first showing `preload` (restored scrollback).
    fn new_terminal_with(
        &mut self,
        command: Option<(String, Vec<String>)>,
        cwd: Option<std::path::PathBuf>,
        preload: Option<Vec<u8>>,
        rect: [f32; 4],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Entity<TerminalView>> {
        let style = terminal_style(&self.config, &self.theme, cx);
        // The PTY starts at the pane's size: programs read it on startup,
        // and output from before a resize would be reflowed.
        let area = self.pane_area(window);
        let pane = gpui::size(area.width * rect[2], area.height * rect[3]);
        let grid = motty_render::grid_for(&style, window, pane);
        let id = crate::terminal_view::next_pane_id();
        let (session, events) = spawn_shell(&self.config, command, cwd, preload, grid, id)?;
        let (meta, blink) = (self.config.option_as_meta, self.config.cursor_blink);
        let terminal = cx.new(|cx| {
            let mut view = TerminalView::new(id, session, events, style, meta, blink, window, cx);
            view.set_link_options(crate::terminal_view::LinkOptions::from_config(&self.config));
            view.set_behavior(self.config.behavior, self.config.progress_commands());
            view.set_autocomplete(self.config.autocomplete.clone(), cx);
            view.refresh_process();
            view
        });
        self.watch_terminal(&terminal, window, cx);
        Ok(terminal)
    }

    /// Follows a pane's events: titles, exits, finished commands and
    /// notifications.
    fn watch_terminal(
        &mut self,
        terminal: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let subscription = cx.subscribe_in(
            terminal,
            window,
            |this, terminal, event, window, cx| match event {
                TerminalEvent::TitleChanged => {
                    cx.notify();
                    cx.defer(refresh_dock);
                }
                TerminalEvent::Exited => this.close_pane(terminal, window, cx),
                TerminalEvent::CommandFinished { exit, duration } => {
                    tracing::debug!(?exit, ?duration, "command finished");
                    crate::ipc::command_finished(terminal, *exit, *duration, cx);
                    this.command_status(terminal, *exit, window, cx);
                    cx.defer(crate::menus::refresh);
                    cx.defer(refresh_dock);
                }
                TerminalEvent::Notification { title, body } => {
                    // Only for panes the user can't see right now.
                    if !window.is_window_active() || !this.is_focused_pane(terminal) {
                        let title = title
                            .clone()
                            .unwrap_or_else(|| terminal.read(cx).display_title());
                        crate::notifications::notify(&title, body);
                    }
                }
            },
        );
        self.pane_subscriptions
            .insert(terminal.entity_id(), subscription);
    }

    fn add_tab(
        &mut self,
        command: Option<(String, Vec<String>)>,
        cwd: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let terminal = self.new_terminal(command, cwd, [0., 0., 1., 1.], window, cx)?;
        // `new-tab-position`: after the active tab, or at the end.
        let ix = match self.config.new_tab_position {
            motty_theme::NewTabPosition::AfterCurrent if !self.tabs.is_empty() => {
                (self.active + 1).min(self.tabs.len())
            }
            _ => self.tabs.len(),
        };
        self.tabs.insert(ix, Tab::new(terminal));
        self.activate(ix, window, cx);
        Ok(())
    }

    /// Where a new tab or split starts, as the `*-working-directory` keys
    /// say: where the active pane is, home, or a folder.
    fn start_dir(&self, kind: motty_theme::NewTerminal, cx: &App) -> Option<std::path::PathBuf> {
        match self.config.working_dir(kind) {
            motty_theme::WorkingDir::Inherit => self.active_folder(cx),
            motty_theme::WorkingDir::Home => dirs::home_dir(),
            motty_theme::WorkingDir::Path(path) => Some(path),
        }
    }

    pub fn has_tabs(&self) -> bool {
        !self.tabs.is_empty()
    }

    /// Splits the focused pane, starting a shell (or `command`) in the new
    /// half where the focused one is, and focuses it.
    pub fn split(
        &mut self,
        direction: Direction,
        command: Option<(String, Vec<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = self.split_pane(direction, command, None, true, window, cx) {
            tracing::error!("failed to start shell: {e:#}");
        }
    }

    /// Splits the focused pane of the active tab; the new pane starts in
    /// `cwd`, else where the focused one is.
    pub fn split_pane(
        &mut self,
        direction: Direction,
        command: Option<(String, Vec<String>)>,
        cwd: Option<std::path::PathBuf>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Entity<TerminalView>> {
        let cwd = cwd.or_else(|| self.start_dir(motty_theme::NewTerminal::Split, cx));
        let tab = self.active_terms().context("no terminal tab")?;
        let rect = direction.half(tab.root.rect_of(&tab.focused).unwrap_or([0., 0., 1., 1.]));
        let terminal = self.new_terminal(command, cwd, rect, window, cx)?;
        let id = self.next_split;
        self.next_split += 1;
        let tab = self.active_terms_mut().context("no terminal tab")?;
        tab.zoomed = false;
        tab.root
            .split(&tab.focused.clone(), terminal.clone(), direction, id);
        if focus {
            self.focus_pane(terminal.clone(), window, cx);
        }
        cx.notify();
        Ok(terminal)
    }

    /// Opens a tab running `command` (else a shell) in `cwd` (else where the
    /// active tab is) and activates it.
    pub fn open_tab_with(
        &mut self,
        command: Option<(String, Vec<String>)>,
        cwd: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Entity<TerminalView>> {
        let cwd = cwd.or_else(|| self.active_folder(cx));
        self.add_tab(command, cwd, window, cx)?;
        let tab = self
            .tabs
            .get(self.active)
            .and_then(Tab::terms)
            .context("no tab")?;
        Ok(tab.focused.clone())
    }

    /// Every pane, by tab.
    pub fn panes(&self) -> impl Iterator<Item = (usize, &Entity<TerminalView>)> {
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(ix, tab)| Some((ix, tab.terms()?)))
            .flat_map(|(ix, tab)| tab.root.leaves().into_iter().map(move |v| (ix, v)))
    }

    pub fn find_pane(&self, id: u64, cx: &App) -> Option<Entity<TerminalView>> {
        self.panes()
            .map(|(_, v)| v)
            .find(|v| v.read(cx).id == id)
            .cloned()
    }

    /// The pane that has focus in the active tab.
    /// Whether `view` is in the active tab (so it's drawn).
    pub fn is_pane_shown(&self, view: &Entity<TerminalView>) -> bool {
        self.active_terms()
            .is_some_and(|tab| tab.root.contains(view))
    }

    pub fn focused_pane(&self) -> Option<Entity<TerminalView>> {
        self.active_terms().map(|tab| tab.focused.clone())
    }

    pub fn is_focused_pane(&self, view: &Entity<TerminalView>) -> bool {
        self.active_terms().is_some_and(|tab| tab.focused == *view)
    }

    /// The active tab's terminals, unless it's an editor.
    fn active_terms(&self) -> Option<&Terminals> {
        self.tabs.get(self.active).and_then(Tab::terms)
    }

    fn active_terms_mut(&mut self) -> Option<&mut Terminals> {
        self.tabs.get_mut(self.active).and_then(Tab::terms_mut)
    }

    /// Where new tabs start: the active terminal's folder, or the edited
    /// file's.
    fn active_folder(&self, cx: &App) -> Option<std::path::PathBuf> {
        match &self.tabs.get(self.active)?.content {
            Content::Terminals(t) => t.focused.read(cx).cwd.clone(),
            Content::Editor(e) => e.read(cx).folder(),
        }
    }

    /// What has focus in the active tab: its focused pane or its editor.
    fn active_focus(&self, cx: &App) -> Option<gpui::FocusHandle> {
        Some(match &self.tabs.get(self.active)?.content {
            Content::Terminals(t) => t.focused.focus_handle(cx),
            Content::Editor(e) => e.focus_handle(cx),
        })
    }

    /// The tab holding `view`.
    fn tab_of(&self, view: &Entity<TerminalView>) -> Option<usize> {
        self.tabs
            .iter()
            .position(|t| t.terms().is_some_and(|t| t.root.contains(view)))
    }

    /// `focus-follows-mouse`: the pane under the pointer takes focus from
    /// the focused one (not from the palette, a rename or the find bar).
    fn follow_mouse(
        &mut self,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(focused) = self.focused_pane() else {
            return;
        };
        if focused != *view
            && window.is_window_active()
            && focused.focus_handle(cx).is_focused(window)
        {
            self.focus_pane(view.clone(), window, cx);
        }
    }

    /// A folder dropped on a pane's edge: a terminal there, in the folder.
    fn drop_split(&mut self, a: &DropSplit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self
            .panes()
            .map(|(_, v)| v.clone())
            .find(|v| v.read(cx).id == a.pane)
        else {
            return;
        };
        let direction = match a.side.as_str() {
            "left" => Direction::Left,
            "up" => Direction::Up,
            "down" => Direction::Down,
            _ => Direction::Right,
        };
        self.reveal_pane(&view, window, cx);
        if let Err(e) = self.split_pane(direction, None, Some(a.path.clone()), true, window, cx) {
            tracing::error!("failed to start shell: {e:#}");
        }
    }

    /// Activates the pane's tab and focuses the pane.
    pub fn reveal_pane(
        &mut self,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.tab_of(view) {
            self.activate(ix, window, cx);
            self.focus_pane(view.clone(), window, cx);
        }
    }

    pub fn close_pane_entity(
        &mut self,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_pane(view, window, cx);
    }

    fn focus_pane(
        &mut self,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.active_terms_mut() else {
            return;
        };
        if !tab.root.contains(&view) {
            return;
        }
        if tab.focused != view {
            tab.zoomed = false;
        }
        window.focus(&view.focus_handle(cx), cx);
        tab.focused = view;
        cx.notify();
    }

    fn focus_neighbor(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.active_terms() else {
            return;
        };
        if let Some(view) = tab.root.neighbor(&tab.focused, direction) {
            self.focus_pane(view, window, cx);
        }
    }

    /// Focuses the next (or previous) pane in reading order, wrapping around.
    fn cycle_pane(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_terms() else {
            return;
        };
        let leaves = tab.root.leaves();
        let Some(ix) = leaves.iter().position(|v| **v == tab.focused) else {
            return;
        };
        let next = leaves[(ix as isize + delta).rem_euclid(leaves.len() as isize) as usize].clone();
        self.focus_pane(next, window, cx);
    }

    /// Removes a pane (its shell exited or it was closed); the last pane
    /// takes its tab with it.
    fn close_pane(
        &mut self,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.tab_of(view) else {
            return;
        };
        self.pane_subscriptions.remove(&view.entity_id());
        let Some(tab) = self.tabs[ix].terms_mut() else {
            return;
        };
        if tab.root.leaves().len() == 1 {
            return self.close_tab_at(ix, window, cx);
        }
        if tab.focused == *view {
            let near = [
                Direction::Left,
                Direction::Up,
                Direction::Right,
                Direction::Down,
            ]
            .into_iter()
            .find_map(|d| tab.root.neighbor(view, d));
            tab.focused = near.unwrap_or_else(|| tab.root.leaves()[0].clone());
            tab.zoomed = false;
        }
        let root = std::mem::replace(&mut tab.root, Pane::Leaf(tab.focused.clone()));
        tab.root = root.without(view).expect("another pane remains");
        if ix == self.active {
            let focused = tab.focused.clone();
            self.focus_pane(focused, window, cx);
        }
        cx.notify();
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(ix) else {
            return;
        };
        tab.updated = std::time::SystemTime::now();
        // Looking at a tab clears a mark of something that happened.
        if tab.mark.is_some_and(|b| !b.ongoing()) {
            tab.mark = None;
        }
        self.active = ix;
        self.clear_seen(ix, cx);
        cx.defer(crate::menus::refresh);
        cx.defer(refresh_dock);
        if let Some(focus) = self.active_focus(cx) {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        if let Content::Editor(editor) = &tab.content {
            if editor.read(cx).is_dirty() {
                return self.confirm_close_editor(ix, window, cx);
            }
            self.remove_tab(ix, window, cx);
            return;
        }
        let mut scrollback = Vec::new();
        let Some(state) = tab_state(
            &self.tabs[ix],
            Some(&mut |bytes| {
                scrollback.push(bytes);
                Some((scrollback.len() - 1).to_string())
            }),
            cx,
        ) else {
            return;
        };
        let Some(terms) = self.tabs[ix].terms() else {
            return;
        };
        let title = tab_title(&self.tabs[ix], cx);
        let cwd = terms.focused.read(cx).cwd.clone();
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let closed = &mut cx.default_global::<ClosedTabs>().0;
        if closed.len() == MAX_CLOSED_TABS {
            closed.remove(0);
        }
        closed.push(ClosedTab {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            title,
            cwd,
            state,
            index: ix,
            scrollback,
        });
        self.remove_tab(ix, window, cx);
    }

    /// Takes the tab out (no questions, nothing kept for reopening) and
    /// activates a neighbor, or closes the window with the last tab.
    fn remove_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.tabs.remove(ix);
        match &tab.content {
            Content::Terminals(t) => {
                for view in t.root.leaves() {
                    self.pane_subscriptions.remove(&view.entity_id());
                }
            }
            Content::Editor(e) => {
                self.pane_subscriptions.remove(&e.entity_id());
            }
        }
        if self.tabs.is_empty() {
            window.remove_window();
            return;
        }
        let next = if self.active > ix || self.active >= self.tabs.len() {
            self.active.saturating_sub(1)
        } else {
            self.active
        };
        self.activate(next.min(self.tabs.len() - 1), window, cx);
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(window, cx);
    }

    /// Opens a shell tab and activates it.
    pub fn open_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.start_dir(motty_theme::NewTerminal::Tab, cx);
        if let Err(e) = self.add_tab(None, cwd, window, cx) {
            tracing::error!("failed to start shell: {e:#}");
        }
    }

    fn reopen_closed_tab(
        &mut self,
        _: &ReopenClosedTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = self.reopen_tab(None, window, cx) {
            tracing::info!("{e:#}");
            window.play_system_bell();
        }
    }

    /// ⇧⌘T: the last closed tab (or the one with `id`) comes back where it
    /// was, with its splits, folders and scrollback (and new shells).
    /// Returns its focused pane.
    pub fn reopen_tab(
        &mut self,
        id: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Entity<TerminalView>> {
        let stack = &mut cx.default_global::<ClosedTabs>().0;
        let ix = match id {
            Some(id) => stack.iter().position(|c| c.id == id),
            None => stack.len().checked_sub(1),
        };
        let closed = stack.remove(ix.context("no closed tab to reopen")?);
        let scrollback = |name: &str| {
            let i: usize = name.parse().ok()?;
            closed.scrollback.get(i).cloned()
        };
        let tab = self
            .restore_tab(&closed.state, &scrollback, window, cx)
            .context("couldn't reopen the tab")?;
        let focused = tab.terms().context("not a terminal tab")?.focused.clone();
        let ix = closed.index.min(self.tabs.len());
        self.tabs.insert(ix, tab);
        self.activate(ix, window, cx);
        Ok(focused)
    }

    /// Opens the config file in the default text editor, first creating
    /// Motty's own when there's none.
    fn open_config_file(&self) {
        let path = self.config.path.clone().unwrap_or_else(|| {
            let path = motty_theme::config::motty_dir().join("config.toml");
            let created =
                std::fs::create_dir_all(motty_theme::config::motty_dir()).and_then(|_| {
                    std::fs::write(
                        &path,
                        "# Motty settings: https://docs.otty.sh/reference/configuration\n",
                    )
                });
            if let Err(e) = created {
                tracing::warn!("couldn't create {}: {e}", path.display());
            }
            path
        });
        if let Err(e) = std::process::Command::new("/usr/bin/open")
            .arg("-t")
            .arg(&path)
            .spawn()
        {
            tracing::warn!("couldn't open {}: {e}", path.display());
        }
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_cascade(window, cx);
    }

    fn cycle(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len() as isize;
        if n > 0 {
            self.activate(
                (self.active as isize + delta).rem_euclid(n) as usize,
                window,
                cx,
            );
        }
    }

    /// The `privilege-*` and `ipc-allow-*` switches.
    pub fn badges(&self) -> motty_theme::Badges {
        self.config.badges
    }

    /// Re-reads config.toml and the themes after one of them changed on disk.
    pub fn reload_config(&mut self, cx: &mut Context<Self>) {
        let config = crate::load_config();
        for w in &config.warnings {
            tracing::warn!("{w}");
        }
        for w in crate::actions::bind_keys(cx, &config.keybinds) {
            tracing::warn!("{w}");
        }
        // Menu items show the shortcuts as bound when the menus are set.
        if config.keybinds != self.config.keybinds {
            crate::menus::set(cx);
        }
        // The toggle wins until the key itself changes.
        if config.sidebar_visible != self.config.sidebar_visible {
            self.sidebar_visible = config.sidebar_visible;
        }
        self.config = config;
        let (meta, blink, cursor) = (
            self.config.option_as_meta,
            self.config.cursor_blink,
            cursor_style(&self.config),
        );
        let scrollback = self.config.scrollback.unwrap_or(10_000);
        let links = crate::terminal_view::LinkOptions::from_config(&self.config);
        let autocomplete = self.config.autocomplete.clone();
        let (behavior, progress) = (self.config.behavior, self.config.progress_commands());
        cursor_hiding(&self.config, cx);
        for view in self.panes().map(|(_, v)| v) {
            view.update(cx, |t, cx| {
                t.set_link_options(links.clone());
                t.set_behavior(behavior, progress.clone());
                t.set_autocomplete(autocomplete.clone(), cx);
                t.apply_config(meta, blink, cursor, scrollback, cx)
            });
        }
        self.reload_theme(cx);
    }

    pub fn reload_theme(&mut self, cx: &mut Context<Self>) {
        let parsed = self.config.resolve_theme(self.mode);
        for w in &parsed.warnings {
            tracing::warn!("{w}");
        }
        self.theme = parsed.theme;
        crate::native_menu::follow_theme(self.theme.meta.mode == motty_theme::Mode::Dark);
        self.ui_font = crate::fonts::ui_family(&self.theme.token.font_ui, cx).into();
        self.mono_font = crate::fonts::terminal_family(&self.config, &self.theme, cx).into();
        let style = terminal_style(&self.config, &self.theme, cx);
        for tab in &self.tabs {
            match &tab.content {
                Content::Terminals(t) => {
                    for view in t.root.leaves() {
                        view.update(cx, |t, cx| t.set_style(style.clone(), cx));
                    }
                }
                Content::Editor(e) => e.update(cx, |e, cx| {
                    e.set_style(style.clone(), cx);
                    e.set_line_numbers(self.config.editor_line_numbers, cx);
                }),
            }
        }
        cx.notify();
    }

    /// The active tab's panes; unfocused ones fade like Otty's
    /// `unfocused-split-opacity`.
    fn render_panes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tab = match &self.tabs.get(self.active)?.content {
            Content::Terminals(t) => t,
            Content::Editor(e) => return Some(e.clone().into_any_element()),
        };
        if tab.zoomed {
            return Some(tab.focused.clone().into_any_element());
        }
        let focused = tab.focused.clone();
        let split = matches!(tab.root, Pane::Split(_));
        let weak = cx.entity().downgrade();
        let style = PaneStyle {
            divider: hsla(self.theme.divider.color),
            divider_width: self.theme.divider.width,
            dimmed: &move |view| split && *view != focused,
            dim_opacity: self.config.unfocused_split_opacity.unwrap_or(0.6),
            on_drag_start: Rc::new(move |id, _, cx| {
                if let Some(this) = weak.upgrade() {
                    this.update(cx, |this, _| this.dragging = Some(id));
                }
            }),
            on_hover: (split && self.config.behavior.focus_follows_mouse).then(|| {
                let weak = cx.entity().downgrade();
                Rc::new(
                    move |view: &Entity<TerminalView>,
                          event: &gpui::MouseMoveEvent,
                          window: &mut Window,
                          cx: &mut App| {
                        if event.pressed_button.is_some() || cx.has_active_drag() {
                            return;
                        }
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| this.follow_mouse(view, window, cx));
                    },
                ) as Rc<crate::panes::PaneHover>
            }),
        };
        Some(tab.root.render(&style))
    }

    fn drag_divider(
        &mut self,
        event: &gpui::MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.dragging else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.dragging = None;
            return;
        }
        if let Some(split) = self.active_terms_mut().and_then(|t| t.root.split_mut(id)) {
            split.drag_to(event.position);
            cx.notify();
        }
    }

    fn equalize_splits(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_terms_mut() {
            tab.root.equalize();
            cx.notify();
        }
    }

    /// Otty's ⌃⇧⌘ arrows: move the divider beside the focused pane.
    fn move_divider(&mut self, direction: Direction, cx: &mut Context<Self>) {
        const STEP: f32 = 10.;
        let Some(tab) = self.active_terms_mut() else {
            return;
        };
        let (axis, delta) = match direction {
            Direction::Left => (gpui::Axis::Horizontal, -STEP),
            Direction::Right => (gpui::Axis::Horizontal, STEP),
            Direction::Up => (gpui::Axis::Vertical, -STEP),
            Direction::Down => (gpui::Axis::Vertical, STEP),
        };
        let focused = tab.focused.clone();
        if let Some(split) = tab.root.enclosing_split(&focused, axis) {
            split.nudge(delta);
            cx.notify();
        }
    }

    fn toggle_zoom(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_terms_mut()
            && matches!(tab.root, Pane::Split(_))
        {
            tab.zoomed = !tab.zoomed;
            cx.notify();
        }
    }

    /// The container's size: what the active tab's panes share.
    fn pane_area(&self, window: &Window) -> gpui::Size<gpui::Pixels> {
        let viewport = window.viewport_size();
        let m = &self.theme.container.margin;
        let sidebar = if self.sidebar_visible {
            self.sidebar_width()
        } else {
            0.
        };
        let details = if self.details.visible {
            self.details_width()
        } else {
            0.
        };
        gpui::size(
            (viewport.width - px(sidebar + details + m.left + m.right)).max(px(0.)),
            (viewport.height - px(TITLEBAR_RESERVE + m.top + m.bottom)).max(px(0.)),
        )
    }

    /// `sidebar-width` excludes the border, which sits outside it.
    fn sidebar_width(&self) -> f32 {
        let border = self.theme.sidebar.border.as_ref().map_or(0., |b| b.width);
        self.sidebar_inner_width() + border
    }

    /// `sidebar-width`, or where its edge is being dragged.
    fn sidebar_inner_width(&self) -> f32 {
        self.sidebar_drag
            .or(self.config.sidebar_width)
            .unwrap_or(self.theme.sidebar.width)
    }

    /// Follows the pointer while the sidebar's edge is dragged.
    fn drag_sidebar(&mut self, event: &gpui::MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        if self.sidebar_drag.is_none() {
            return false;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.finish_sidebar_drag(cx);
            return true;
        }
        let border = self.theme.sidebar.border.as_ref().map_or(0., |b| b.width);
        let width = (f32::from(event.position.x) - border).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        self.sidebar_drag = Some(width.round());
        cx.notify();
        true
    }

    /// Saves the dragged width as `sidebar-width`, as Otty does.
    fn finish_sidebar_drag(&mut self, cx: &mut Context<Self>) {
        let Some(width) = self.sidebar_drag else {
            return;
        };
        let value = toml::Value::Integer(width as i64);
        match motty_theme::config::save(&[("sidebar-width", Some(value))]) {
            Ok(_) => {
                self.config.sidebar_width = Some(width);
                self.sidebar_drag = None;
                cx.defer(reload_all);
            }
            Err(e) => {
                tracing::warn!("couldn't save sidebar-width: {e}");
                self.sidebar_drag = None;
            }
        }
        cx.notify();
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let side = &t.sidebar;
        let sidebar = with_border_right(
            div()
                .id("sidebar")
                .relative()
                .w(px(self.sidebar_width()))
                .h_full()
                .flex_none()
                .flex()
                .flex_col()
                .bg(hsla(side.background))
                .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move()),
            t,
            &side.border,
        );

        let header = div()
            .mt(px(SIDEBAR_HEADER_TOP))
            .h(px(SIDEBAR_HEADER_HEIGHT))
            .pl(px(t.sidebar.padding.left + SIDEBAR_TEXT_INSET))
            .pr(px(t.sidebar.padding.right + SIDEBAR_ICON_INSET))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(10.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(hsla(t.token.tertiary))
            .child(if self.inbox { "INBOX" } else { "TABS" })
            .child(
                div()
                    .id("tabs-filter")
                    .rounded(px(4.))
                    .hover(|s| s.bg(hsla(native_label(t, 0.08, 0.1))))
                    .child(list_icon(hsla(t.token.tertiary)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.show_filter_menu(event.position, window, cx);
                        }),
                    ),
            );

        let list: Vec<AnyElement> = if self.inbox {
            self.inbox_rows(cx)
        } else {
            let mut rows = Vec::new();
            let grouped = self.grouping != tabs::Grouping::None;
            for group in self.sidebar_groups(cx) {
                if let Some(heading) = group.heading {
                    rows.push(self.group_heading(heading, group.folder, cx));
                }
                for ix in group.tabs {
                    let card = self.render_tab_card(ix, None, cx);
                    // Dividers mark places in the tabs' own order.
                    if !self.tabs[ix].divider || grouped {
                        rows.push(card);
                        continue;
                    }
                    // Insert Divider: a hairline inset from the cards, in the
                    // middle of a gap.
                    rows.push(
                        div()
                            .flex()
                            .flex_col()
                            .child(card)
                            .child(
                                div()
                                    .mt(px(TAB_GAP + DIVIDER_SPACE))
                                    .mb(px(DIVIDER_SPACE))
                                    .mx(px(DIVIDER_INSET))
                                    .h(px(1.))
                                    .bg(hsla(native_label(t, 0.1, 0.12))),
                            )
                            .into_any_element(),
                    );
                }
            }
            rows
        };

        sidebar
            .child(header)
            .child(
                // The list's top edge clips the outer half of the first card's
                // border, as in Otty.
                div()
                    .id("tab-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                            if !this.inbox {
                                this.show_filter_menu(event.position, window, cx);
                            }
                        }),
                    )
                    .child(
                        div()
                            .pl(px(t.sidebar.padding.left))
                            .pr(px(t.sidebar.padding.right))
                            .pb(px(1.))
                            .flex()
                            .flex_col()
                            .gap(px(TAB_GAP))
                            .children(list),
                    ),
            )
            .child(
                // Drag the edge to resize; it straddles the border.
                div()
                    .id("sidebar-edge")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(-SIDEBAR_GRAB / 2.))
                    .w(px(SIDEBAR_GRAB))
                    .occlude()
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.sidebar_drag = Some(this.sidebar_inner_width());
                        }),
                    ),
            )
            .into_any_element()
    }

    /// What a tab's card shows: its title, the process (or pane count),
    /// the most pressing badge of its panes and that pane's status line.
    fn tab_summary(&self, tab: &Tab, cx: &App) -> TabSummary {
        match &tab.content {
            Content::Terminals(terms) => {
                let view = terms.focused.read(cx);
                // With splits, Otty counts the panes instead.
                let panes = terms.root.leaves();
                let process = if panes.len() > 1 {
                    panes.len().to_string()
                } else {
                    view.process.clone().unwrap_or_default()
                };
                let mut summary = TabSummary {
                    title: tabs::named(tab.name.as_ref(), view.display_title()),
                    process,
                    badge: None,
                    line: None,
                    running: false,
                    progress: None,
                    system: Default::default(),
                };
                let caffeinate = tab.privileges.unwrap_or(self.config.badges).caffeinate;
                let mut best = usize::MAX;
                for pane in panes {
                    let pane = pane.read(cx);
                    summary.running |= pane.busy;
                    let s = &mut summary.system;
                    s.sudo |= pane.system.sudo;
                    s.caffeinate |= pane.system.caffeinate
                        || caffeinate
                            && pane.status.as_ref().is_some_and(|s| {
                                s.agent.is_some() && s.in_turn && s.badge != Some(Badge::Awaiting)
                            });
                    if s.ssh.is_none() {
                        s.ssh.clone_from(&pane.system.ssh);
                    }
                    summary.progress = most_pressing(summary.progress, pane.progress);
                    let Some(status) = &pane.status else { continue };
                    let rank = status.badge.map_or(9, badge_rank);
                    if rank < best || summary.line.is_none() {
                        if rank < best {
                            best = rank;
                            summary.badge = status.badge;
                        }
                        summary.line = status.message.clone().or_else(|| status.agent.clone());
                    }
                    summary.running |= status.in_turn;
                }
                // Mark Tab: shown unless a pane has something more pressing.
                if let Some(mark) = tab.mark
                    && badge_rank(mark) < best
                {
                    summary.badge = Some(mark);
                }
                summary
            }
            // Otty marks unsaved changes with a star.
            Content::Editor(editor) => {
                let editor = editor.read(cx);
                let dirty = editor.is_dirty();
                TabSummary {
                    title: tabs::named(
                        tab.name.as_ref(),
                        format!("{}{}", editor.title(), if dirty { "*" } else { "" }),
                    ),
                    process: String::new(),
                    badge: None,
                    line: dirty.then(|| "Unsaved changes".to_owned()),
                    running: false,
                    progress: None,
                    system: Default::default(),
                }
            }
        }
    }

    /// A tab's card in the sidebar; Inbox adds a line about what it's doing.
    fn render_tab_card(
        &self,
        ix: usize,
        line: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let summary = self.tab_summary(&self.tabs[ix], cx);
        let active = ix == self.active;
        let style = if active {
            &t.tabs.active
        } else {
            &t.tabs.normal
        };
        let hover_bg = t.tabs.hover.background;
        let height = style.height.unwrap_or(TAB_HEIGHT);
        let label_color = hsla(if active {
            native_tertiary_label(t)
        } else {
            t.token.tertiary
        });
        // The success dot and checkmark only mark what happened while you
        // were away, so the active tab never shows them.
        let badge = summary
            .badge
            .filter(|b| !(active && matches!(b, Badge::Finished | Badge::Done)));
        // Progress shows unless something needs the user.
        let label = match (badge, summary.progress) {
            (Some(badge), _) if badge.needs_you() => badge_icon(badge, ix, t),
            (_, Some(progress)) => progress_icon(progress, ix, t),
            (Some(badge), None) => badge_icon(badge, ix, t),
            (None, None) => tracked(&summary.process, LABEL_PITCH).into_any_element(),
        };
        let title = div()
            .flex_1()
            .min_w_0()
            .pt(px(1.))
            .truncate()
            .child(summary.title.clone());
        let renaming = self.renaming.as_ref().filter(|r| r.ix == ix);
        let title = match (&line, renaming) {
            (_, Some(rename)) => self.rename_field(rename, cx),
            (None, None) => title.into_any_element(),
            (Some(line), None) => div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(title)
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(label_color)
                        .child(line.clone()),
                )
                .into_any_element(),
        };
        let card = div()
            .id(("tab", ix))
            .relative()
            .flex_none()
            .when(line.is_none(), |d| d.h(px(height)))
            .when(line.is_some(), |d| d.min_h(px(height)).py(px(7.)))
            .px(px(style.padding.left))
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .rounded(px(style.radius))
            .bg(hsla(style.background))
            // GPUI paints a shadow under the whole box, where it would
            // show through a translucent fill; CSS (and Otty) only
            // draw it outside, so translucent cards go without.
            .when(style.background.a >= 1., |d| {
                d.shadow(shadows(&style.shadow))
            })
            .text_color(hsla(style.foreground))
            .text_size(px(style.font_size))
            .font_weight(FontWeight(style.font_weight))
            .when(!active, |d| d.hover(move |s| s.bg(hsla(hover_bg))))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    if this.renaming.as_ref().is_some_and(|r| r.ix == ix) {
                        return;
                    }
                    this.activate(ix, window, cx);
                    if event.click_count == 2 {
                        this.start_rename(ix, window, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.show_tab_menu(ix, event.position, window, cx);
                }),
            )
            // Dragging a card puts the tab where it's dropped.
            .on_drag(tabs::TabDrag { ix }, {
                let title = summary.title.clone();
                move |_, _, _, cx| {
                    let title = title.clone();
                    cx.new(|_| tabs::DragPreview { title })
                }
            })
            .drag_over::<tabs::TabDrag>(move |s, _, _, _| s.bg(hsla(hover_bg)))
            .on_drop(cx.listener(move |this, drag: &tabs::TabDrag, window, cx| {
                this.move_tab(drag.ix, ix, window, cx)
            }))
            // `indicator = "bar"`: Otty puts it 8pt into the card,
            // centered, under the start of the title.
            .when(style.indicator_bar, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(8.))
                        .top(px((height - 16.) / 2.))
                        .w(px(6.))
                        .h(px(16.))
                        .bg(hsla(style.indicator)),
                )
            })
            // Otty's title baseline is half a point below the card's center.
            .child(title)
            .child(
                // The label column keeps room for six characters, so
                // titles truncate at the same place for any process.
                // It ends half a point inside the card's padding.
                div()
                    .flex_none()
                    .mr(px(0.5))
                    .min_w(px(LABEL_COLUMNS * LABEL_PITCH))
                    .flex()
                    .justify_end()
                    .font_family(self.mono_font.clone())
                    .text_size(px(LABEL_FONT_SIZE))
                    .text_color(label_color)
                    .gap(px(4.))
                    .children(system_badges(&summary.system, label_color))
                    .child(label),
            );
        card.children(
            style
                .border
                .as_ref()
                .and_then(|b| tab_border(t, b, style.radius)),
        )
        .into_any_element()
    }

    /// A heading over a group of tabs; its "+" opens a tab in the group's
    /// folder.
    fn group_heading(
        &self,
        heading: String,
        folder: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let id = SharedString::from(format!("group-{heading}"));
        div()
            .id(id.clone())
            .group(id.clone())
            .pt(px(8.))
            .pl(px(SIDEBAR_TEXT_INSET))
            .pr(px(4.))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(10.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(hsla(t.token.tertiary))
            .child(div().truncate().child(heading.to_uppercase()))
            .child(
                div()
                    .id(SharedString::from(format!("{id}-new")))
                    .invisible()
                    .group_hover(id, |s| s.visible())
                    .px(px(4.))
                    .rounded(px(4.))
                    .hover(|s| s.bg(hsla(native_label(t, 0.08, 0.1))))
                    .child("+")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            let cwd = folder.clone().or_else(|| this.active_folder(cx));
                            if let Err(e) = this.add_tab(None, cwd, window, cx) {
                                tracing::error!("failed to start shell: {e:#}");
                            }
                        }),
                    ),
            )
            .into_any_element()
    }

    /// Inbox: tabs that need you, then running ones, then the rest by when
    /// they were last active, each with a line on what it's doing.
    fn inbox_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = &self.theme;
        let now = std::time::SystemTime::now();
        let today = days_ago(now, now);
        let mut groups: [InboxGroup; 5] = [
            ("Needs You", Vec::new()),
            ("Running", Vec::new()),
            ("Today", Vec::new()),
            ("Yesterday", Vec::new()),
            ("Earlier", Vec::new()),
        ];
        let mut order: Vec<usize> = (0..self.tabs.len()).collect();
        order.sort_by_key(|&ix| std::cmp::Reverse(self.tabs[ix].updated));
        for ix in order {
            let summary = self.tab_summary(&self.tabs[ix], cx);
            let group = match summary.badge {
                Some(b) if b.needs_you() || b == Badge::Finished => 0,
                Some(Badge::Running) => 1,
                _ if summary.running => 1,
                _ => match today - days_ago(self.tabs[ix].updated, now) {
                    0 => 2,
                    1 => 3,
                    _ => 4,
                },
            };
            let line = summary.line.or_else(|| {
                Some(
                    match summary.badge? {
                        Badge::Awaiting => "Needs your input",
                        Badge::Error => "Failed",
                        Badge::Done => "Finished",
                        Badge::Running => "Working…",
                        Badge::Finished => "Command finished",
                    }
                    .to_owned(),
                )
            });
            groups[group].1.push((ix, line));
        }
        let heading = |name: &'static str| {
            div()
                .pt(px(8.))
                .pl(px(SIDEBAR_TEXT_INSET))
                .text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(hsla(t.token.tertiary))
                .child(name.to_uppercase())
                .into_any_element()
        };
        let mut rows = Vec::new();
        for (name, tabs) in groups {
            if tabs.is_empty() {
                continue;
            }
            rows.push(heading(name));
            for (ix, line) in tabs {
                rows.push(self.render_tab_card(ix, line, cx));
            }
        }
        rows
    }
}

/// Whole days since the epoch in local time, roughly: enough to tell
/// today from yesterday.
impl Workspace {
    /// What the menu bar shows about this window.
    pub(crate) fn snapshot_for_menus(&self, cx: &App) -> crate::menus::Snapshot {
        let mut s = crate::menus::Snapshot {
            pinned: self.pinned,
            secure: crate::secure_input::manual(cx),
            ..Default::default()
        };
        if let Some(view) = self.focused_pane() {
            let v = view.read(cx);
            s.pane = Some(v.id);
            s.cwd = v.cwd.clone();
            let session = v.session();
            s.last = session.shell().blocks.back().and_then(|b| {
                Some(crate::menus::LastCommand {
                    text: session.block_command(b)?,
                    row: b.prompt_row,
                    exit: b.exit,
                    duration: b.duration(),
                    running: b.started.is_some() && b.finished.is_none(),
                })
            });
            s.agent = v
                .status
                .as_ref()
                .and_then(|st| Some((st.agent.clone()?, st.session.clone())));
            s.read_only = v.is_read_only();
            s.vi = v.in_vi_mode();
            s.fork = v.status.as_ref().and_then(|st| {
                let (agent, id) = (st.agent_id.as_deref()?, st.session.as_deref()?);
                crate::agents::sessions::fork_command(agent, id)
                    .map(|_| crate::agents::sessions::fork_label(agent))
            });
        }
        s.tabs = self
            .tabs
            .iter()
            .filter_map(|tab| {
                let terms = tab.terms()?;
                let procs = terms
                    .root
                    .leaves()
                    .iter()
                    .flat_map(|v| v.read(cx).session().processes())
                    .map(|p| (p.pid, p.name))
                    .collect();
                Some((tab_title(tab, cx), procs))
            })
            .collect();
        s.privileges = self.privileges_for_menus();
        s.open_with = open_with_apps(&self.config.open_with_apps);
        s
    }

    /// Shell › Git: types the command at the prompt; one that changes
    /// something remote runs after asking.
    fn type_command(&mut self, a: TypeCommand, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.focused_pane() else {
            return;
        };
        if !a.run {
            view.update(cx, |v, cx| v.write_input(&a.text, cx));
            return;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Info,
            &format!("Run “{}”?", a.text),
            Some("It runs in the focused pane."),
            &["Run", "Cancel"],
            cx,
        );
        cx.spawn(async move |_, cx| {
            if answer.await == Ok(0) {
                view.update(cx, |v, cx| v.write_input(&format!("{}\r", a.text), cx));
            }
        })
        .detach();
    }
}

/// The frontmost window's state for the menu bar.
/// `mouse-hide-while-typing`: GPUI hides the pointer on typing unless told
/// not to.
pub(crate) fn cursor_hiding(config: &Config, cx: &mut App) {
    cx.set_cursor_hide_mode(if config.behavior.hide_mouse_while_typing {
        gpui::CursorHideMode::OnTyping
    } else {
        gpui::CursorHideMode::Never
    });
}

/// The Dock icon from every window's tabs: red while a failure waits to
/// be looked at, a bar while something reports progress.
pub(crate) fn refresh_dock(cx: &mut App) {
    use motty_term::shell::Progress;
    let mut error = false;
    let mut progress: Option<Progress> = None;
    let mut behavior = None;
    for window in cx.windows() {
        let Some(workspace) = window.downcast::<Workspace>() else {
            continue;
        };
        let Ok(this) = workspace.read(cx) else {
            continue;
        };
        behavior.get_or_insert(this.config.behavior);
        for tab in &this.tabs {
            let summary = this.tab_summary(tab, cx);
            error |= summary.badge == Some(Badge::Error);
            progress = most_pressing(progress, summary.progress);
        }
    }
    let behavior = behavior.unwrap_or_default();
    let state = match progress {
        _ if error && behavior.dock_error_badge => crate::dock::DockState::Error,
        Some(Progress::Error(_)) if behavior.dock_error_badge => crate::dock::DockState::Error,
        Some(Progress::Normal(p)) if behavior.dock_progress => {
            crate::dock::DockState::Progress(Some(p))
        }
        Some(_) if behavior.dock_progress => crate::dock::DockState::Progress(None),
        _ => crate::dock::DockState::Normal,
    };
    crate::dock::set(state, cx);
}

pub(crate) fn menu_snapshot(cx: &App) -> Option<crate::menus::Snapshot> {
    let window = cx
        .active_window()
        .or_else(|| cx.window_stack()?.into_iter().next())?;
    let workspace = window.downcast::<Workspace>()?;
    workspace.read(cx).ok().map(|w| w.snapshot_for_menus(cx))
}

/// An Inbox heading and its tabs, each with its line.
type InboxGroup = (&'static str, Vec<(usize, Option<String>)>);

fn days_ago(time: std::time::SystemTime, _now: std::time::SystemTime) -> i64 {
    let secs = time
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    (secs + local_offset_secs()) / 86_400
}

/// The local time zone's offset from UTC.
fn local_offset_secs() -> i64 {
    // SAFETY: localtime_r writes into the struct we pass.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm.tm_gmtoff
    }
}

struct TabSummary {
    title: String,
    process: String,
    badge: Option<Badge>,
    line: Option<String>,
    /// A command or an agent's turn is under way.
    running: bool,
    /// The panes' progress, the most pressing first.
    progress: Option<motty_term::shell::Progress>,
    /// Sudo, SSH or caffeinate in any pane (an agent kept awake counts).
    system: motty_term::SystemState,
}

/// Which badge a tab shows when its panes have several: the one most in
/// need of the user.
fn badge_rank(badge: Badge) -> usize {
    match badge {
        Badge::Awaiting => 0,
        Badge::Error => 1,
        Badge::Done => 2,
        Badge::Running => 3,
        Badge::Finished => 4,
    }
}

/// Otty's tab badges: a spinner, a hand, a check, an accent dot, a
/// triangle.
/// Of two progress reports, the one to show: an error, else the further
/// along, else a spinner.
fn most_pressing(
    a: Option<motty_term::shell::Progress>,
    b: Option<motty_term::shell::Progress>,
) -> Option<motty_term::shell::Progress> {
    use motty_term::shell::Progress::*;
    let rank = |p: &motty_term::shell::Progress| match p {
        Error(_) => 3,
        Normal(_) => 1,
        _ => 0,
    };
    match (a, b) {
        (Some(Normal(x)), Some(Normal(y))) => Some(Normal(x.max(y))),
        (Some(a), Some(b)) => Some(if rank(&b) > rank(&a) { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// A ring filled to the percentage (red on error), or a spinner when
/// the amount isn't known.
fn progress_icon(progress: motty_term::shell::Progress, ix: usize, t: &Theme) -> AnyElement {
    use motty_term::shell::Progress;
    let (share, color) = match progress {
        Progress::Normal(p) => (f32::from(p.min(100)) / 100., hsla(t.token.accent)),
        Progress::Error(p) => (
            f32::from(p.min(100)).max(1.) / 100.,
            gpui::rgb(0xff3b30).into(),
        ),
        _ => return badge_icon(Badge::Running, ix, t),
    };
    let track = hsla(t.token.tertiary).opacity(0.35);
    titlebar::icon(12., 12., move |o, window| {
        let (cx, cy, r) = (6., 6., 4.5);
        let ring = |from: f32, to: f32| {
            let mut path = gpui::PathBuilder::stroke(px(1.8));
            let at = |a: f32| titlebar::at(o, cx + r * a.sin(), cy - r * a.cos());
            let tau = std::f32::consts::TAU;
            path.move_to(at(from * tau));
            // Arcs over half a turn go in two halves.
            let mid = (from + to) / 2.;
            for end in [mid, to] {
                path.arc_to(point(px(r), px(r)), px(0.), false, true, at(end * tau));
            }
            path
        };
        if let Ok(path) = ring(0., 0.9999).build() {
            window.paint_path(path, track);
        }
        if let Ok(path) = ring(0., share).build() {
            window.paint_path(path, color);
        }
    })
    .into_any_element()
}

/// The devices, shield and coffee cup: a tab on another host, in a sudo
/// session, keeping the Mac awake. They reflect ongoing state, so they show
/// on the active tab too.
fn system_badges(system: &motty_term::SystemState, color: Hsla) -> Vec<AnyElement> {
    [
        (system.ssh.is_some(), "icons/monitor-smartphone.svg"),
        (system.sudo, "icons/shield.svg"),
        (system.caffeinate, "icons/coffee.svg"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, path)| {
        svg()
            .path(path)
            .flex_none()
            .size(px(12.))
            .text_color(color)
            .into_any_element()
    })
    .collect()
}

fn badge_icon(badge: Badge, ix: usize, t: &Theme) -> AnyElement {
    let icon = |path: &'static str, color: Hsla| {
        svg().path(path).flex_none().size(px(12.)).text_color(color)
    };
    match badge {
        Badge::Running => icon("icons/loader-circle.svg", hsla(t.token.tertiary))
            .with_animation(
                ("tab-spinner", ix),
                Animation::new(Duration::from_millis(900)).repeat(),
                |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
            )
            .into_any_element(),
        Badge::Awaiting => icon("icons/hand.svg", gpui::rgb(0xff9500).into()).into_any_element(),
        Badge::Error => {
            icon("icons/triangle-alert.svg", gpui::rgb(0xff3b30).into()).into_any_element()
        }
        Badge::Done => icon("icons/check.svg", gpui::rgb(0x34c759).into()).into_any_element(),
        Badge::Finished => div()
            .flex_none()
            .size(px(7.))
            .mt(px(1.))
            .rounded_full()
            .bg(hsla(t.token.accent))
            .into_any_element(),
    }
}

impl Workspace {
    fn toggle_inbox(&mut self, _: &ToggleInbox, _: &mut Window, cx: &mut Context<Self>) {
        self.inbox = !self.inbox;
        // Inbox lives in the sidebar.
        self.sidebar_visible |= self.inbox;
        cx.notify();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    /// Where the titlebar button group starts: it ends where the sidebar
    /// header's icon box does, or follows the traffic lights.
    fn titlebar_group_left(&self) -> f32 {
        let t = &self.theme;
        if self.sidebar_visible {
            let width = self.sidebar_inner_width();
            width - t.sidebar.padding.right - SIDEBAR_ICON_INSET + 6. - titlebar::GROUP
        } else {
            titlebar::COLLAPSED_LEFT
        }
    }

    /// Otty's titlebar buttons, shown while the pointer is over the window:
    /// new tab, notifications and the tabs panel toggle (at the sidebar's
    /// edge, or after the traffic lights when it is hidden), and the details
    /// panel toggle at the right.
    fn render_titlebar_buttons(
        &self,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
        let t = &self.theme;
        let icon = hsla(native_label(t, 0.5, 0.55));
        let highlight = hsla(native_label(t, 0.08, 0.1));
        let button = move |id: &'static str, width: f32| {
            div()
                .id(id)
                .flex_none()
                .w(px(width))
                .h(px(titlebar::BUTTON))
                .flex()
                .rounded(px(titlebar::RADIUS))
                .hover(move |s| s.bg(highlight))
                // Keep a click from starting a window drag.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        let group = div()
            .absolute()
            .top(px(titlebar::TOP))
            .left(px(self.titlebar_group_left()))
            .flex()
            .gap(px(titlebar::GAP))
            .child(
                // The plus opens a tab; the chevron, the menu of other ways.
                button("new-tab", titlebar::SPLIT)
                    .when(self.new_menu_open, |d| d.bg(highlight))
                    .child(
                        div()
                            .id("new-tab-plus")
                            .child(titlebar::plus(icon))
                            .on_click(cx.listener(|this, _, window, cx| this.open_tab(window, cx))),
                    )
                    .child(
                        div()
                            .id("new-tab-menu")
                            .child(titlebar::chevron(icon))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.show_new_menu(window, cx);
                                }),
                            ),
                    ),
            )
            .child(
                button("notifications", titlebar::BUTTON)
                    .when(self.inbox, |d| d.bg(highlight))
                    .child(titlebar::bell(icon))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_inbox(&ToggleInbox, window, cx)
                    })),
            )
            .child(
                button("tabs-panel", titlebar::BUTTON)
                    .child(titlebar::panel(icon, icon, false))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
            );
        // With the details panel open, its toggle stays at the strip's end.
        let details = (self.pointer_inside || self.details.visible).then(|| {
            div()
                .absolute()
                .top(px(titlebar::TOP))
                .right(px(titlebar::DETAILS_RIGHT))
                .child(
                    button("details-panel", titlebar::BUTTON)
                        .child(titlebar::panel(
                            icon,
                            if self.details.visible {
                                icon
                            } else {
                                hsla(native_tertiary_label(t))
                            },
                            true,
                        ))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_details(cx))),
                )
                .into_any_element()
        });
        (
            self.pointer_inside.then(|| group.into_any_element()),
            details,
        )
    }

    /// The titlebar pills at the right: the pinned window, the focused
    /// pane's SSH host and sudo session, Secure Keyboard Entry, and the
    /// focused pane's mode (measured from Otty's READ ONLY pill).
    fn render_pills(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pane = self.focused_pane().map(|v| v.read(cx));
        let read_only = pane.is_some_and(|p| p.is_read_only());
        // Nothing typed reaches a read-only pane, so these would only mislead.
        let secure = (self.config.behavior.secure_input_indication
            && !read_only
            && window.is_window_active()
            && crate::secure_input::is_on(cx))
        .then(|| self.render_secure_pill());
        let sudo = pane.filter(|p| p.system.sudo && !read_only).map(|_| {
            self.render_label_pill("sudo", "icons/shield.svg", "SUDO".into())
                .into_any_element()
        });
        let ssh = pane
            .and_then(|p| p.system.ssh.clone())
            .map(|host| self.render_ssh_pill(host, cx));
        let pin = self.pinned.then(|| self.render_pin_pill(cx));
        let mode = self.render_mode_pill(cx);
        if [&secure, &sudo, &ssh, &pin, &mode]
            .iter()
            .all(|p| p.is_none())
        {
            return None;
        }
        Some(
            div()
                .absolute()
                .top(px(6.))
                .right(px(43.5))
                .flex()
                .gap(px(6.))
                .children(pin)
                .children(ssh)
                .children(sudo)
                .children(secure)
                .children(mode)
                .into_any_element(),
        )
    }

    /// A pill in the SECURE INPUT style: an icon and a label.
    fn render_label_pill(
        &self,
        id: &'static str,
        icon: &'static str,
        label: SharedString,
    ) -> gpui::Stateful<gpui::Div> {
        let t = &self.theme;
        let ink = hsla(native_label(t, 1., 1.));
        div()
            .id(id)
            .h(px(20.))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .rounded(px(6.))
            .bg(hsla(native_label(t, 0.065, 0.1)))
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ink)
            .child(svg().path(icon).flex_none().size(px(12.)).text_color(ink))
            .child(label)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    }

    /// The SSH host; a click shows the host, its address and the connection.
    fn render_ssh_pill(&self, host: String, cx: &mut Context<Self>) -> AnyElement {
        let shown = if host.is_empty() {
            "SSH".to_owned()
        } else {
            host.rsplit('@').next().unwrap_or(&host).to_owned()
        };
        let highlight = hsla(native_label(&self.theme, 0.12, 0.16));
        self.render_label_pill("ssh", "icons/monitor-smartphone.svg", shown.into())
            .cursor_pointer()
            .hover(move |s| s.bg(highlight))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    let at = event.position();
                    this.show_ssh_details(host.clone(), at, window, cx)
                }),
            )
            .into_any_element()
    }

    /// Pinned windows float above the others; the pill unpins.
    fn render_pin_pill(&self, cx: &mut Context<Self>) -> AnyElement {
        let highlight = hsla(native_label(&self.theme, 0.12, 0.16));
        self.render_label_pill("pinned", "icons/pin.svg", "PINNED".into())
            .cursor_pointer()
            .hover(move |s| s.bg(highlight))
            .on_click(
                cx.listener(|_, _, window, cx| window.dispatch_action(Box::new(PinWindow), cx)),
            )
            .into_any_element()
    }

    /// What `ssh -G` makes of `host`, and the address it resolves to; read
    /// off the main thread, then shown as a menu at `at`.
    fn show_ssh_details(
        &mut self,
        host: String,
        at: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = window.window_handle();
        cx.spawn(async move |_, cx| {
            let details = cx.background_spawn(async move { ssh_details(&host) }).await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let mut entries = vec![crate::native_menu::Entry::Header("SSH".into())];
                for (label, symbol) in details {
                    entries.push(crate::native_menu::Entry::Info {
                        label: label.into(),
                        symbol,
                    });
                }
                crate::native_menu::popup(entries, at, None, window, cx, |_, _, _| {});
            });
        })
        .detach();
    }

    fn render_secure_pill(&self) -> AnyElement {
        let t = &self.theme;
        let ink = hsla(native_label(t, 1., 1.));
        div()
            .id("secure-input")
            .h(px(20.))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .rounded(px(6.))
            .bg(hsla(native_label(t, 0.065, 0.1)))
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ink)
            .child(titlebar::lock(ink))
            .child("SECURE INPUT")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .into_any_element()
    }

    fn render_mode_pill(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = self.focused_pane()?;
        let mode = view.read(cx).mode()?;
        let t = &self.theme;
        let ink = hsla(native_label(t, 1., 1.));
        let secondary = hsla(native_label(t, 0.5, 0.55));
        let (lock, label) = match mode {
            PaneMode::Hints => (false, "HINTS".to_owned()),
            PaneMode::Vi { count: None } => (false, "VI MODE".to_owned()),
            PaneMode::Vi { count: Some(n) } => (false, format!("VI MODE  {n}")),
            PaneMode::ReadOnly => (true, "READ ONLY".to_owned()),
        };
        Some(
            div()
                .h(px(20.))
                .flex()
                .items_center()
                .gap(px(10.))
                .pl(px(12.))
                .pr(px(8.))
                .rounded(px(6.))
                .bg(hsla(native_label(t, 0.065, 0.1)))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink)
                .when(lock, |d| d.child(titlebar::lock(ink)))
                .child(SharedString::from(label))
                .when(mode == PaneMode::Hints, |d| {
                    d.child(
                        div()
                            .font_weight(FontWeight::NORMAL)
                            .text_size(px(12.))
                            .text_color(secondary)
                            .child("Esc Exit"),
                    )
                })
                .child(
                    div()
                        .id("exit-mode")
                        .ml(px(2.))
                        .rounded(px(3.))
                        .hover(|d| d.bg(hsla(native_label(t, 0.08, 0.1))))
                        .child(titlebar::small_close(secondary))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| view.update(cx, |v, cx| v.exit_mode(cx))),
                )
                .into_any_element(),
        )
    }

    /// Native materials behind the window and the sidebar, bottom-most first.
    fn backdrops_wanted(&self) -> Vec<Backdrop> {
        let t = &self.theme;
        let mut wanted = vec![Backdrop {
            region: Region::Window,
            material: t.window.material,
        }];
        // A sidebar on the window's own material is the same surface in Otty,
        // not a second layer (stacked glass would lighten the column).
        if self.sidebar_visible && t.sidebar.material != t.window.material {
            wanted.push(Backdrop {
                region: Region::Sidebar {
                    width: self.sidebar_width(),
                },
                material: t.sidebar.material,
            });
        }
        if self.details.visible && t.details_panel.material != t.window.material {
            wanted.push(Backdrop {
                region: Region::DetailsPanel {
                    width: self.details_width(),
                },
                material: t.details_panel.material,
            });
        }
        wanted
    }
}

/// Motty draws its own materials, so the window only needs to be see-through
/// when the theme uses one.
pub fn background_appearance(t: &Theme) -> WindowBackgroundAppearance {
    if [
        t.window.material,
        t.sidebar.material,
        t.details_panel.material,
    ]
    .iter()
    .all(|m| *m == Material::None)
    {
        WindowBackgroundAppearance::Opaque
    } else {
        WindowBackgroundAppearance::Transparent
    }
}

/// AppKit's tertiary label color for the theme's mode.
fn native_tertiary_label(t: &Theme) -> Rgba {
    native_label(t, 0.26, 0.25)
}

/// Black (light mode) or white (dark mode) at the given strength, like the
/// system's label colors.
fn native_label(t: &Theme, light: f32, dark: f32) -> Rgba {
    match t.meta.mode {
        Mode::Light => Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: light,
        },
        Mode::Dark => Rgba {
            r: 1.,
            g: 1.,
            b: 1.,
            a: dark,
        },
    }
}

/// A tab card's border, stroked centered on the card's edge (half outside),
/// as Otty draws it. Otty skips translucent tab borders (seen with the
/// `rgba()` borders of its glass themes), so only opaque ones are drawn.
fn tab_border(t: &Theme, b: &Border, radius: f32) -> Option<AnyElement> {
    let color = t.border_color(b);
    if color.a < 1. || b.width <= 0. {
        return None;
    }
    let half = px(b.width / 2.);
    Some(
        div()
            .absolute()
            .top(-half)
            .left(-half)
            .right(-half)
            .bottom(-half)
            .rounded(px(radius + b.width / 2.))
            .border(px(b.width))
            .border_color(hsla(color))
            .into_any_element(),
    )
}

/// Lays out `text` one character per fixed-width cell, which emulates the
/// letter-spacing AppKit applies to small labels (GPUI has no tracking).
fn tracked(text: &str, pitch: f32) -> gpui::Div {
    div().flex().children(text.chars().map(|c| {
        div()
            .w(px(pitch))
            .flex()
            .justify_center()
            .child(c.to_string())
    }))
}

/// Three-line "list" glyph used beside section headers.
fn list_icon(color: gpui::Hsla) -> AnyElement {
    let bar = |w: f32| div().h(px(1.)).w(px(w)).rounded(px(0.5)).bg(color);
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(2.5))
        .child(bar(8.5))
        .child(bar(6.5))
        .child(bar(4.5))
        .into_any_element()
}

fn shadows(list: &[Shadow]) -> Vec<BoxShadow> {
    list.iter()
        .map(|s| BoxShadow {
            color: hsla(s.color),
            offset: point(px(s.offset_x), px(s.offset_y)),
            // CSS blur is twice the Gaussian's sigma; GPUI takes the sigma.
            blur_radius: px(s.blur / 2.),
            spread_radius: px(s.spread),
            inset: s.inset,
        })
        .collect()
}

fn with_border<E: Styled>(el: E, t: &Theme, border: &Option<Border>) -> E {
    match border {
        Some(b) => el.border(px(b.width)).border_color(hsla(t.border_color(b))),
        None => el,
    }
}

fn with_border_right<E: Styled>(el: E, t: &Theme, border: &Option<Border>) -> E {
    match border {
        Some(b) => el
            .border_r(px(b.width))
            .border_color(hsla(t.border_color(b))),
        None => el,
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.sidebar_visible.then(|| self.render_sidebar(cx));
        let details = self.details.visible.then(|| self.render_details(cx));
        let (titlebar_buttons, details_button) = self.render_titlebar_buttons(cx);
        let t = &self.theme;
        let c = &t.container;
        // A click into another pane moves focus without telling us.
        if let Some(tab) = self.tabs.get_mut(self.active).and_then(Tab::terms_mut)
            && let Some(view) = tab
                .root
                .leaves()
                .into_iter()
                .find(|v| v.focus_handle(cx).is_focused(window))
            && *view != tab.focused
        {
            tab.focused = view.clone();
            tab.zoomed = false;
            cx.defer(crate::menus::refresh);
        }
        let title = self
            .tabs
            .get(self.active)
            .map(|tab| tab_title(tab, cx))
            .unwrap_or_default();
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title.clone();
        }
        // Like Otty, a running command marks the window edited (the dot in
        // the close button), since closing it would kill that command.
        let busy = self.panes().any(|(_, view)| view.read(cx).busy)
            || self
                .tabs
                .iter()
                .any(|tab| tab.as_editor().is_some_and(|e| e.read(cx).is_dirty()));
        let state = (
            background_appearance(t),
            busy && !self.config.hide_dirty_indicator,
        );
        if self.native_state != Some(state) {
            window.set_background_appearance(state.0);
            window.set_window_edited(state.1);
            self.native_state = Some(state);
        }
        let backdrops = self.backdrops_wanted();
        self.backdrops
            .sync(window, &backdrops, self.theme.meta.mode == Mode::Dark);

        // Long titles truncate, clear of the buttons on the left (without a
        // sidebar) and of a mode pill, with the same room on both sides so
        // the title stays centered.
        let pill = self
            .focused_pane()
            .is_some_and(|view| view.read(cx).mode().is_some());
        let side = [
            titlebar::TITLE_PAD_RIGHT,
            if pill { PILL_INTRUSION } else { 0. },
            if self.sidebar_visible {
                0.
            } else {
                titlebar::COLLAPSED_LEFT + titlebar::GROUP + titlebar::TITLE_PAD_LEFT
            },
        ]
        .into_iter()
        .fold(0., f32::max);
        let title_band = div()
            .absolute()
            .top_0()
            .left_0()
            .right(px(if self.details.visible {
                0.
            } else if self.sidebar_visible {
                TITLE_RIGHT_INSET
            } else {
                COLLAPSED_TITLE_RIGHT_INSET
            }))
            .h(px(TITLE_BAND))
            .px(px(side))
            .flex()
            .items_center()
            .justify_center()
            .text_color(hsla(t.titlebar.foreground))
            .text_size(px(t.titlebar.font_size))
            .child(
                div()
                    .relative()
                    .min_w_0()
                    .h(px(titlebar::BUTTON))
                    .flex()
                    .items_center()
                    .when(self.pointer_inside || self.title_menu_open, |d| {
                        // The title and its "more" ellipsis are one button;
                        // its highlight and the ellipsis hang around the
                        // title without moving it.
                        let highlight = hsla(native_label(t, 0.08, 0.1));
                        let bounds = self.title_bounds.clone();
                        d.child(
                            div()
                                .id("title")
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(px(-titlebar::TITLE_PAD_LEFT))
                                .right(px(-titlebar::TITLE_PAD_RIGHT))
                                .rounded(px(titlebar::RADIUS))
                                .hover(move |s| s.bg(highlight))
                                .when(self.title_menu_open, |d| d.bg(highlight))
                                .child(
                                    gpui::canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                                        .size_full(),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.show_title_menu(window, cx);
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(gpui::relative(1.))
                                .pl(px(titlebar::ELLIPSIS_GAP))
                                .flex()
                                .items_center()
                                .child(titlebar::ellipsis(hsla(native_label(t, 0.55, 0.6)))),
                        )
                    })
                    .child(div().relative().min_w_0().truncate().child(title)),
            );

        // Otty draws the container border over the padding rather than
        // inside it, so the grid doesn't move when a theme adds a border.
        let border_overlay = c.border.as_ref().map(|_| {
            with_border(
                div().absolute().inset_0().rounded(px(c.radius)),
                t,
                &c.border,
            )
        });
        let container = div()
            .relative()
            .flex_1()
            .min_h_0()
            .mt(px(c.margin.top))
            .mr(px(c.margin.right))
            .mb(px(c.margin.bottom))
            .ml(px(c.margin.left))
            .rounded(px(c.radius))
            .overflow_hidden()
            .bg(hsla(c.background))
            .shadow(shadows(&c.shadow))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(div().flex_1().min_h_0().children(self.render_panes(cx)))
                    .children(self.render_pinned_composer(cx)),
            )
            .children(border_overlay);

        let main = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            // Behind the container card is the window itself (a material when
            // the theme has one); the card paints the terminal's color.
            .bg(hsla(t.window.background))
            .child(
                div()
                    .id("titlebar")
                    // Otty keeps the native height with a sidebar, whatever
                    // `[titlebar] height` says (Nord sets 40).
                    .h(px(TITLEBAR_RESERVE))
                    .flex_none()
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .child(container)
            .child(title_band);

        let root = div().id("workspace");
        self.register_menu_actions(root, cx)
            .relative()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.pointer_inside != *hovered {
                    this.pointer_inside = *hovered;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleTabsPanel, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::new_file_editor))
            .on_action(cx.listener(Self::toggle_inbox))
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| {
                this.start_rename(this.active, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveTabToNewWindow, window, cx| {
                this.move_tab_to_new_window(this.active, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveTabUp, window, cx| {
                let ix = this.active;
                this.move_tab(ix, ix.wrapping_sub(1), window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveTabDown, window, cx| {
                let ix = this.active;
                this.move_tab(ix, ix + 1, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ClearBadge, _, cx| this.clear_badge(this.active, cx)))
            .on_action(
                cx.listener(|this, _: &ShowNextUnread, window, cx| {
                    this.show_next_unread(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &PinWindow, window, cx| {
                this.pinned = !this.pinned;
                crate::materials::set_pinned(window, this.pinned);
                cx.defer(crate::menus::refresh);
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &OpenFile, window, cx| this.open_from_finder(window, cx)),
            )
            .on_action(cx.listener(|this, a: &OpenFolder, window, cx| {
                if let Err(e) = this.add_tab(None, Some(a.path.clone()), window, cx) {
                    tracing::error!("failed to start shell: {e:#}");
                }
            }))
            .on_action(cx.listener(|this, a: &RevealRow, _, cx| {
                if let Some(view) = this.focused_pane() {
                    view.update(cx, |v, cx| v.reveal_row(a.row, cx));
                }
            }))
            .on_action(cx.listener(|this, a: &TypeCommand, window, cx| {
                this.type_command(a.clone(), window, cx)
            }))
            .on_mouse_move(cx.listener(|this, event, window, cx| {
                if !this.drag_sidebar(event, cx) {
                    this.drag_divider(event, window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging = None;
                    this.finish_sidebar_drag(cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &SplitRight, w, cx| {
                    this.split(Direction::Right, None, w, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &SplitLeft, w, cx| this.split(Direction::Left, None, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SplitDown, w, cx| this.split(Direction::Down, None, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SplitUp, w, cx| this.split(Direction::Up, None, w, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusPaneLeft, w, cx| {
                this.focus_neighbor(Direction::Left, w, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPaneRight, w, cx| {
                this.focus_neighbor(Direction::Right, w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FocusPaneUp, w, cx| {
                    this.focus_neighbor(Direction::Up, w, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FocusPaneDown, w, cx| {
                this.focus_neighbor(Direction::Down, w, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusNextPane, w, cx| this.cycle_pane(1, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPreviousPane, w, cx| this.cycle_pane(-1, w, cx)))
            .on_action(cx.listener(|this, _: &EqualizeSplits, _, cx| this.equalize_splits(cx)))
            .on_action(cx.listener(|this, _: &ZoomSplit, _, cx| this.toggle_zoom(cx)))
            .on_action(cx.listener(|this, _: &MoveDividerLeft, _, cx| {
                this.move_divider(Direction::Left, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveDividerRight, _, cx| {
                this.move_divider(Direction::Right, cx)
            }))
            .on_action(
                cx.listener(|this, _: &MoveDividerUp, _, cx| this.move_divider(Direction::Up, cx)),
            )
            .on_action(cx.listener(|this, _: &MoveDividerDown, _, cx| {
                this.move_divider(Direction::Down, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                this.close_asking(closing::Closing::Window, window, cx)
            }))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &ZoomWindow, window, _| window.zoom_window())
            .on_action(|_: &ToggleFullScreen, window, _| window.toggle_fullscreen())
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::reopen_closed_tab))
            .on_action(cx.listener(Self::toggle_palette))
            .on_action(cx.listener(|this, _: &ToggleDetailsPanel, _, cx| this.toggle_details(cx)))
            .on_action(cx.listener(|this, _: &DetailsInfo, _, cx| {
                this.show_details_tab(details::DetailsTab::Info, cx)
            }))
            .on_action(cx.listener(|this, _: &DetailsOutline, _, cx| {
                this.show_details_tab(details::DetailsTab::Outline, cx)
            }))
            .on_action(cx.listener(|this, _: &DetailsGit, _, cx| {
                this.show_details_tab(details::DetailsTab::Git, cx)
            }))
            .on_action(cx.listener(|this, _: &DetailsFiles, _, cx| {
                this.show_details_tab(details::DetailsTab::Files, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenQuickly, window, cx| {
                this.show_open_quickly(open_quickly::Filter::All, window, cx)
            }))
            .on_action(cx.listener(|this, _: &JumpTo, window, cx| {
                this.show_open_quickly(open_quickly::Filter::Current, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenFilesQuickly, window, cx| {
                this.show_open_quickly(open_quickly::Filter::Files, window, cx)
            }))
            .on_action(
                cx.listener(|this, a: &DropSplit, window, cx| this.drop_split(a, window, cx)),
            )
            .on_action(cx.listener(|this, a: &EditFiles, window, cx| {
                for path in &a.paths {
                    this.open_editor(Some(path.clone()), window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &GlobalSearch, window, cx| {
                    this.open_global_search(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &SendToChat, window, cx| this.send_to_chat(window, cx)),
            )
            .on_action(
                cx.listener(|this, a: &ResumeSession, window, cx| {
                    this.resume_session(a, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &AgentHistory, window, cx| {
                    this.show_agent_history(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ViewSessionHistory, window, cx| {
                this.view_focused_history(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ForkSessionInNewTab, window, cx| {
                this.fork_focused("tab", window, cx)
            }))
            .on_action(cx.listener(|this, _: &ForkSessionInNewWindow, window, cx| {
                this.fork_focused("window", window, cx)
            }))
            .on_action(cx.listener(|this, _: &ForkSessionRight, window, cx| {
                this.fork_focused("right", window, cx)
            }))
            .on_action(cx.listener(|this, _: &ForkSessionLeft, window, cx| {
                this.fork_focused("left", window, cx)
            }))
            .on_action(cx.listener(|this, _: &ForkSessionDown, window, cx| {
                this.fork_focused("down", window, cx)
            }))
            .on_action(cx.listener(|this, _: &ForkSessionUp, window, cx| {
                this.fork_focused("up", window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenConfigFile, _, _| this.open_config_file()))
            .on_action(cx.listener(|this, _: &ShowWelcome, w, cx| this.show_welcome(w, cx)))
            .on_action(cx.listener(|this, _: &NextTab, w, cx| this.cycle(1, w, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, w, cx| this.cycle(-1, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab1, w, cx| this.activate(0, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab2, w, cx| this.activate(1, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab3, w, cx| this.activate(2, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab4, w, cx| this.activate(3, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab5, w, cx| this.activate(4, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab6, w, cx| this.activate(5, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab7, w, cx| this.activate(6, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab8, w, cx| this.activate(7, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateLastTab, w, cx| {
                let last = this.tabs.len().saturating_sub(1);
                this.activate(last, w, cx)
            }))
            .size_full()
            .flex()
            .flex_row()
            // The window fill only shows under the sidebar (the main column
            // paints its own); a sidebar material must not be covered by it.
            .when(t.sidebar.material == Material::None, |d| {
                d.bg(hsla(t.window.background))
            })
            .font_family(self.ui_font.clone())
            .children(sidebar)
            .child(main)
            .children(details)
            .children(titlebar_buttons)
            .children(details_button)
            .children(self.render_pills(window, cx))
            .children(self.render_palette(window, cx))
            .children(self.render_open_quickly(window, cx))
            .children(self.render_global_search(window, cx))
            .children(self.render_welcome(window, cx))
    }
}

/// What session recovery (or reopening a closed tab) keeps of `tab`. With
/// `scrollback`, each pane's scrollback is handed to it, and the name it
/// returns is saved with the pane.
/// The tab's name in the sidebar and the window title.
fn tab_title(tab: &Tab, cx: &App) -> String {
    let title = match &tab.content {
        Content::Terminals(t) => t.focused.read(cx).display_title(),
        Content::Editor(e) => e.read(cx).title(),
    };
    tabs::named(tab.name.as_ref(), title)
}

/// What recovery and ⇧⌘T keep of a terminal tab (editors aren't kept).
fn tab_state(
    whole: &Tab,
    mut scrollback: Option<&mut dyn FnMut(Vec<u8>) -> Option<String>>,
    cx: &App,
) -> Option<TabState> {
    let tab = whole.terms()?;
    fn save(
        pane: &Pane,
        scrollback: &mut Option<&mut dyn FnMut(Vec<u8>) -> Option<String>>,
        cx: &App,
    ) -> PaneState {
        match pane {
            Pane::Leaf(view) => {
                let view = view.read(cx);
                let scrollback = scrollback.as_mut().and_then(|keep| {
                    let term = view.session().term.lock();
                    let bytes = motty_term::snapshot::serialize(
                        &*term,
                        crate::recovery::scrollback_lines(),
                    )?;
                    drop(term);
                    if bytes.is_empty() {
                        return None;
                    }
                    keep(bytes)
                });
                let agent = view
                    .status
                    .as_ref()
                    .and_then(|s| Some([s.agent_id.clone()?, s.session.clone()?]));
                PaneState::Leaf {
                    cwd: view.cwd.clone(),
                    scrollback,
                    agent,
                    queue: view.queued().to_vec(),
                }
            }
            Pane::Split(split) => PaneState::Split {
                vertical: split.axis == gpui::Axis::Vertical,
                ratio: split.ratio,
                first: Box::new(save(&split.first, scrollback, cx)),
                second: Box::new(save(&split.second, scrollback, cx)),
            },
        }
    }
    Some(TabState {
        root: save(&tab.root, &mut scrollback, cx),
        focused: tab
            .root
            .leaves()
            .iter()
            .position(|v| **v == tab.focused)
            .unwrap_or(0),
        zoomed: tab.zoomed,
        divider: whole.divider,
        name: whole.name.as_ref().map(|n| n.text.clone()),
        name_prefix: whole.name.as_ref().is_some_and(|n| n.prefix),
    })
}

/// The SSH pill's menu lines for `dest` (`user@host` as typed): host,
/// address and connection, from `ssh -G` (which reads the config only).
fn ssh_details(dest: &str) -> Vec<(String, &'static str)> {
    use std::net::ToSocketAddrs;
    let config = std::process::Command::new("ssh")
        .arg("-G")
        .arg(dest)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let value = |key: &str| {
        config.lines().find_map(|l| {
            let (k, v) = l.split_once(' ')?;
            (k == key).then(|| v.to_owned())
        })
    };
    let host = value("hostname").unwrap_or_else(|| dest.rsplit('@').next().unwrap_or(dest).into());
    let port: u16 = value("port").and_then(|p| p.parse().ok()).unwrap_or(22);
    let mut out = vec![(host.clone(), "network")];
    if let Some(addr) = (host.as_str(), port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
    {
        out.push((addr.ip().to_string(), "globe"));
    }
    let user = value("user").unwrap_or_default();
    out.push((format!("{user}@{host}:{port}"), "link"));
    out
}
