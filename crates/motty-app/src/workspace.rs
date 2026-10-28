//! Window root: the tab sidebar, the titlebar band, and the themed container
//! around the active tab's panes.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, BoxShadow, Context, Entity, EntityId, FocusHandle, Focusable, FontWeight,
    MouseButton, SharedString, Subscription, Task, Window, WindowAppearance,
    WindowBackgroundAppearance, div, point, prelude::*, px,
};
use motty_render::{TerminalStyle, hsla};
use motty_term::alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use motty_term::{GridSize, Session, SessionConfig, SessionEvent};
use motty_theme::{Border, Config, Material, Mode, Rgba, Shadow, Theme};

use crate::actions::*;
use crate::materials::{Backdrop, Backdrops, Region};
use crate::panes::{Direction, Pane, PaneStyle};
use crate::terminal_view::{TerminalEvent, TerminalView};
use crate::titlebar;

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
/// Top of the "TABS" header row and the first tab card.
const SIDEBAR_HEADER_TOP: f32 = 37.5;
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

struct Tab {
    root: Pane,
    /// The pane that has (or last had) focus.
    focused: Entity<TerminalView>,
    /// Showing only the focused pane.
    zoomed: bool,
}

impl Tab {
    fn new(view: Entity<TerminalView>) -> Self {
        Self {
            root: Pane::Leaf(view.clone()),
            focused: view,
            zoomed: false,
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
    backdrops: Backdrops,
    /// Last background appearance and edited flag given to the native window;
    /// both reach AppKit on every call, so they are only set on change.
    native_state: Option<(WindowBackgroundAppearance, bool)>,
    /// The tabs sidebar (`sidebar-visible`, ⌘⇧L).
    sidebar_visible: bool,
    /// The pointer is over the window: Otty shows its titlebar buttons then.
    pointer_inside: bool,
    _subscriptions: Vec<Subscription>,
    /// Each terminal's event subscription, dropped with its pane.
    pane_subscriptions: HashMap<EntityId, Subscription>,
    /// Ids for new splits.
    next_split: usize,
    /// The split whose divider is being dragged.
    dragging: Option<usize>,
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
    size: GridSize,
) -> anyhow::Result<(Session, async_channel::Receiver<SessionEvent>)> {
    // Commands from `-e` run as they are; the user's shell gets integration.
    let env = if command.is_none() {
        crate::shell_integration::env(config, &crate::shell_integration::login_shell(config))
    } else {
        Default::default()
    };
    let session = SessionConfig {
        command: command.or_else(|| config.shell.clone().map(|s| (s, vec!["-l".into()]))),
        env,
        working_directory: cwd.or_else(dirs::home_dir),
        scrollback: config.scrollback.unwrap_or(10_000),
        cursor_style: cursor_style(config),
    };
    Session::spawn(session, size)
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mode = mode_for(window.appearance());
        let parsed = config.resolve_theme(mode);
        for w in config.warnings.iter().chain(&parsed.warnings) {
            tracing::warn!("{w}");
        }
        let theme = parsed.theme;
        let ui_font = crate::fonts::ui_family(&theme.token.font_ui, cx).into();
        let mono_font = crate::fonts::terminal_family(&config, &theme, cx).into();

        let subscriptions = vec![cx.observe_window_appearance(window, |this, window, cx| {
            let mode = mode_for(window.appearance());
            if mode != this.mode {
                this.mode = mode;
                this.reload_theme(cx);
            }
        })];
        let process_poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| {
                    let mut changed = false;
                    for tab in &this.tabs {
                        for view in tab.root.leaves() {
                            changed |= view.update(cx, |t, _| t.refresh_process());
                        }
                    }
                    if changed {
                        cx.notify();
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
            backdrops: Backdrops::default(),
            native_state: None,
            sidebar_visible,
            pointer_inside: false,
            _subscriptions: subscriptions,
            pane_subscriptions: HashMap::new(),
            next_split: 0,
            dragging: None,
            _process_poll: process_poll,
            _config_poll: config_poll,
        };
        if let Err(e) = this.add_tab(command, cwd, window, cx) {
            tracing::error!("failed to start shell: {e:#}");
        }
        this
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
        let style = terminal_style(&self.config, &self.theme, cx);
        // The PTY starts at the pane's size: programs read it on startup,
        // and output from before a resize would be reflowed.
        let area = self.pane_area(window);
        let pane = gpui::size(area.width * rect[2], area.height * rect[3]);
        let grid = motty_render::grid_for(&style, window, pane);
        let (session, events) = spawn_shell(&self.config, command, cwd, grid)?;
        let (meta, blink) = (self.config.option_as_meta, self.config.cursor_blink);
        let terminal = cx.new(|cx| {
            let mut view = TerminalView::new(session, events, style, meta, blink, window, cx);
            view.refresh_process();
            view
        });
        let subscription = cx.subscribe_in(
            &terminal,
            window,
            |this, terminal, event, window, cx| match event {
                TerminalEvent::TitleChanged => cx.notify(),
                TerminalEvent::Exited => this.close_pane(terminal, window, cx),
                TerminalEvent::CommandFinished { exit, duration } => {
                    tracing::debug!(?exit, ?duration, "command finished");
                }
                TerminalEvent::Notification { title, body } => {
                    tracing::debug!(?title, body, "notification");
                }
            },
        );
        self.pane_subscriptions
            .insert(terminal.entity_id(), subscription);
        Ok(terminal)
    }

    fn add_tab(
        &mut self,
        command: Option<(String, Vec<String>)>,
        cwd: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let terminal = self.new_terminal(command, cwd, [0., 0., 1., 1.], window, cx)?;
        self.tabs.push(Tab::new(terminal));
        self.activate(self.tabs.len() - 1, window, cx);
        Ok(())
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
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let cwd = tab.focused.read(cx).cwd.clone();
        let rect = direction.half(tab.root.rect_of(&tab.focused).unwrap_or([0., 0., 1., 1.]));
        let terminal = match self.new_terminal(command, cwd, rect, window, cx) {
            Ok(terminal) => terminal,
            Err(e) => return tracing::error!("failed to start shell: {e:#}"),
        };
        let id = self.next_split;
        self.next_split += 1;
        let tab = &mut self.tabs[self.active];
        tab.zoomed = false;
        tab.root
            .split(&tab.focused.clone(), terminal.clone(), direction, id);
        self.focus_pane(terminal, window, cx);
    }

    fn focus_pane(
        &mut self,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
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
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if let Some(view) = tab.root.neighbor(&tab.focused, direction) {
            self.focus_pane(view, window, cx);
        }
    }

    /// Focuses the next (or previous) pane in reading order, wrapping around.
    fn cycle_pane(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active) else {
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
        let Some(ix) = self.tabs.iter().position(|t| t.root.contains(view)) else {
            return;
        };
        self.pane_subscriptions.remove(&view.entity_id());
        let tab = &mut self.tabs[ix];
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
            let focused = self.tabs[ix].focused.clone();
            self.focus_pane(focused, window, cx);
        }
        cx.notify();
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        self.active = ix;
        let focus = tab.focused.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(ix);
        for view in tab.root.leaves() {
            self.pane_subscriptions.remove(&view.entity_id());
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
        // New tabs start where the active tab is, like Otty.
        let cwd = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.focused.read(cx).cwd.clone());
        if let Err(e) = self.add_tab(None, cwd, window, cx) {
            tracing::error!("failed to start shell: {e:#}");
        }
    }

    /// ⌘W closes the focused pane, and the tab with its last pane.
    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.tabs.get(self.active).map(|t| t.focused.clone()) {
            self.close_pane(&view, window, cx);
        }
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

    /// Re-reads config.toml and the themes after one of them changed on disk.
    pub fn reload_config(&mut self, cx: &mut Context<Self>) {
        let config = crate::load_config();
        for w in &config.warnings {
            tracing::warn!("{w}");
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
        for view in self.tabs.iter().flat_map(|tab| tab.root.leaves()) {
            view.update(cx, |t, cx| {
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
        self.ui_font = crate::fonts::ui_family(&self.theme.token.font_ui, cx).into();
        self.mono_font = crate::fonts::terminal_family(&self.config, &self.theme, cx).into();
        let style = terminal_style(&self.config, &self.theme, cx);
        for view in self.tabs.iter().flat_map(|tab| tab.root.leaves()) {
            view.update(cx, |t, cx| t.set_style(style.clone(), cx));
        }
        cx.notify();
    }

    /// The active tab's panes; unfocused ones fade like Otty's
    /// `unfocused-split-opacity`.
    fn render_panes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tab = self.tabs.get(self.active)?;
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
        if let Some(split) = self
            .tabs
            .get_mut(self.active)
            .and_then(|t| t.root.split_mut(id))
        {
            split.drag_to(event.position);
            cx.notify();
        }
    }

    fn equalize_splits(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.root.equalize();
            cx.notify();
        }
    }

    /// Otty's ⌃⇧⌘ arrows: move the divider beside the focused pane.
    fn move_divider(&mut self, direction: Direction, cx: &mut Context<Self>) {
        const STEP: f32 = 10.;
        let Some(tab) = self.tabs.get_mut(self.active) else {
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
        if let Some(tab) = self.tabs.get_mut(self.active)
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
        gpui::size(
            (viewport.width - px(sidebar + m.left + m.right)).max(px(0.)),
            (viewport.height - px(TITLEBAR_RESERVE + m.top + m.bottom)).max(px(0.)),
        )
    }

    /// `sidebar-width` excludes the border, which sits outside it.
    fn sidebar_width(&self) -> f32 {
        let border = self.theme.sidebar.border.as_ref().map_or(0., |b| b.width);
        self.config
            .sidebar_width
            .unwrap_or(self.theme.sidebar.width)
            + border
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let side = &t.sidebar;
        let sidebar = with_border_right(
            div()
                .id("sidebar")
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
            .child("TABS")
            .child(list_icon(hsla(t.token.tertiary)));

        let tabs: Vec<AnyElement> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                let view = tab.focused.read(cx);
                let title = view.display_title();
                // With splits, Otty counts the panes instead.
                let panes = tab.root.leaves().len();
                let process = if panes > 1 {
                    panes.to_string()
                } else {
                    view.process.clone().unwrap_or_default()
                };
                let active = ix == self.active;
                let style = if active {
                    &t.tabs.active
                } else {
                    &t.tabs.normal
                };
                let hover_bg = t.tabs.hover.background;
                let card = div()
                    .id(("tab", ix))
                    .relative()
                    .h(px(style.height.unwrap_or(TAB_HEIGHT)))
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
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.activate(ix, window, cx);
                        }),
                    )
                    // `indicator = "bar"`: Otty puts it 8pt into the card,
                    // centered, under the start of the title.
                    .when(style.indicator_bar, |d| {
                        let h = style.height.unwrap_or(TAB_HEIGHT);
                        d.child(
                            div()
                                .absolute()
                                .left(px(8.))
                                .top(px((h - 16.) / 2.))
                                .w(px(6.))
                                .h(px(16.))
                                .bg(hsla(style.indicator)),
                        )
                    })
                    // Otty's title baseline is half a point below the card's center.
                    .child(div().flex_1().min_w_0().pt(px(1.)).truncate().child(title))
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
                            .text_color(hsla(if active {
                                native_tertiary_label(t)
                            } else {
                                t.token.tertiary
                            }))
                            .child(tracked(&process, LABEL_PITCH)),
                    );
                card.children(
                    style
                        .border
                        .as_ref()
                        .and_then(|b| tab_border(t, b, style.radius)),
                )
                .into_any_element()
            })
            .collect();

        sidebar
            .child(header)
            .child(
                // The list's top edge clips the outer half of the first card's
                // border, as in Otty.
                div().overflow_hidden().child(
                    div()
                        .pl(px(t.sidebar.padding.left))
                        .pr(px(t.sidebar.padding.right))
                        .pb(px(1.))
                        .flex()
                        .flex_col()
                        .gap(px(TAB_GAP))
                        .children(tabs),
                ),
            )
            .into_any_element()
    }
}

impl Workspace {
    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    /// Otty's titlebar buttons, shown while the pointer is over the window:
    /// new tab, notifications and the tabs panel toggle (at the sidebar's
    /// edge, or after the traffic lights when it is hidden), and the details
    /// panel toggle at the right.
    fn render_titlebar_buttons(
        &self,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
        if !self.pointer_inside {
            return (None, None);
        }
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
        // The group ends where the sidebar header's icon box does.
        let left = if self.sidebar_visible {
            let width = self.config.sidebar_width.unwrap_or(t.sidebar.width);
            width - t.sidebar.padding.right - SIDEBAR_ICON_INSET + 6. - titlebar::GROUP
        } else {
            titlebar::COLLAPSED_LEFT
        };
        let group = div()
            .absolute()
            .top(px(titlebar::TOP))
            .left(px(left))
            .flex()
            .gap(px(titlebar::GAP))
            .child(
                button("new-tab", titlebar::SPLIT)
                    .child(titlebar::plus(icon))
                    .child(titlebar::chevron(icon))
                    .on_click(cx.listener(|this, _, window, cx| this.open_tab(window, cx))),
            )
            .child(button("notifications", titlebar::BUTTON).child(titlebar::bell(icon)))
            .child(
                button("tabs-panel", titlebar::BUTTON)
                    .child(titlebar::panel(icon, icon, false))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
            );
        let details = div()
            .absolute()
            .top(px(titlebar::TOP))
            .right(px(titlebar::DETAILS_RIGHT))
            .child(
                button("details-panel", titlebar::BUTTON).child(titlebar::panel(
                    icon,
                    hsla(native_tertiary_label(t)),
                    true,
                )),
            );
        (
            Some(group.into_any_element()),
            Some(details.into_any_element()),
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
        wanted
    }
}

/// Motty draws its own materials, so the window only needs to be see-through
/// when the theme uses one.
pub fn background_appearance(t: &Theme) -> WindowBackgroundAppearance {
    if t.window.material == Material::None && t.sidebar.material == Material::None {
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
        let (titlebar_buttons, details_button) = self.render_titlebar_buttons(cx);
        let t = &self.theme;
        let c = &t.container;
        // A click into another pane moves focus without telling us.
        if let Some(tab) = self.tabs.get_mut(self.active)
            && let Some(view) = tab
                .root
                .leaves()
                .into_iter()
                .find(|v| v.focus_handle(cx).is_focused(window))
            && *view != tab.focused
        {
            tab.focused = view.clone();
            tab.zoomed = false;
        }
        let title = self
            .tabs
            .get(self.active)
            .map(|tab| tab.focused.read(cx).display_title())
            .unwrap_or_default();
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title.clone();
        }
        // Like Otty, a running command marks the window edited (the dot in
        // the close button), since closing it would kill that command.
        let busy = self
            .tabs
            .iter()
            .flat_map(|tab| tab.root.leaves())
            .any(|view| view.read(cx).busy);
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

        let title_band = div()
            .absolute()
            .top_0()
            .left_0()
            .right(px(if self.sidebar_visible {
                TITLE_RIGHT_INSET
            } else {
                COLLAPSED_TITLE_RIGHT_INSET
            }))
            .h(px(TITLE_BAND))
            .flex()
            .items_center()
            .justify_center()
            .text_color(hsla(t.titlebar.foreground))
            .text_size(px(t.titlebar.font_size))
            .child(
                div()
                    .relative()
                    .h(px(titlebar::BUTTON))
                    .flex()
                    .items_center()
                    .when(self.pointer_inside, |d| {
                        // The title and its "more" ellipsis are one button;
                        // its highlight and the ellipsis hang around the
                        // title without moving it.
                        let highlight = hsla(native_label(t, 0.08, 0.1));
                        d.child(
                            div()
                                .id("title")
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(px(-titlebar::TITLE_PAD_LEFT))
                                .right(px(-titlebar::TITLE_PAD_RIGHT))
                                .rounded(px(titlebar::RADIUS))
                                .hover(move |s| s.bg(highlight)),
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
                    .child(div().relative().child(title)),
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
            .children(self.render_panes(cx))
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

        div()
            .id("workspace")
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
            .on_mouse_move(cx.listener(Self::drag_divider))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = None),
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
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &ToggleFullScreen, window, _| window.toggle_fullscreen())
            .on_action(cx.listener(Self::close_tab))
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
            .children(titlebar_buttons)
            .children(details_button)
    }
}
