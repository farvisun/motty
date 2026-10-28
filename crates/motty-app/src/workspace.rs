//! Window root: the tab sidebar, the titlebar band, and the themed container
//! around the active terminal.

use std::time::Duration;

use gpui::{
    AnyElement, App, BoxShadow, Context, Entity, FocusHandle, Focusable, FontWeight, MouseButton, SharedString,
    Subscription, Task, Window, WindowAppearance, WindowBackgroundAppearance, div, point, prelude::*, px,
};
use motty_render::{TerminalStyle, hsla};
use motty_term::{GridSize, Session, SessionConfig, SessionEvent};
use motty_theme::{Border, Config, Material, Mode, Rgba, Shadow, Theme};

use crate::actions::*;
use crate::materials::{Backdrop, Backdrops, Region};
use crate::terminal_view::{TerminalEvent, TerminalView};

/// The native titlebar height that the sidebar layout reserves.
const TITLEBAR_RESERVE: f32 = 28.;
/// The title is centered in a band slightly taller than the reserve, level
/// with the traffic lights.
const TITLE_BAND: f32 = 32.;
/// Space kept free at the right of the title band (toolbar buttons).
const TITLE_RIGHT_INSET: f32 = 66.;
/// Top of the "TABS" header row and the first tab card.
const SIDEBAR_HEADER_TOP: f32 = 37.5;
const SIDEBAR_HEADER_HEIGHT: f32 = 22.5;
const TAB_HEIGHT: f32 = 37.;
const TAB_GAP: f32 = 1.;
const SIDEBAR_INSET: f32 = 7.5;
const SIDEBAR_TEXT_INSET: f32 = 18.;
const SIDEBAR_ICON_INSET: f32 = 16.5;
/// Process label beside each tab title: small mono text with wide tracking.
const LABEL_FONT_SIZE: f32 = 9.5;
const LABEL_PITCH: f32 = 6.25;
const LABEL_COLUMNS: f32 = 6.;

struct Tab {
    terminal: Entity<TerminalView>,
    _subscription: Subscription,
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
    _subscriptions: Vec<Subscription>,
    _process_poll: Task<()>,
}

pub fn mode_for(appearance: WindowAppearance) -> Mode {
    match appearance {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Mode::Dark,
        WindowAppearance::Light | WindowAppearance::VibrantLight => Mode::Light,
    }
}

/// Spawns the user's shell for a new pane in `cwd` (home when `None`).
/// `command` (from `-e`) replaces the shell.
pub fn spawn_shell(
    config: &Config,
    command: Option<(String, Vec<String>)>,
    cwd: Option<std::path::PathBuf>,
) -> anyhow::Result<(Session, async_channel::Receiver<SessionEvent>)> {
    let session = SessionConfig {
        command: command.or_else(|| config.shell.clone().map(|s| (s, vec!["-l".into()]))),
        working_directory: cwd.or_else(dirs::home_dir),
        scrollback: config.scrollback.unwrap_or(10_000),
        ..Default::default()
    };
    let initial = GridSize { cols: 80, rows: 24, cell_width: 8., cell_height: 16. };
    Session::spawn(session, initial)
}

pub fn terminal_style(config: &Config, theme: &Theme, cx: &App) -> TerminalStyle {
    let family = crate::fonts::terminal_family(config, theme, cx);
    TerminalStyle::from_theme(theme, Some(&family), config.font_size, config.ligatures)
}

impl Workspace {
    pub fn new(
        config: Config,
        session: Session,
        events: async_channel::Receiver<SessionEvent>,
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
                        changed |= tab.terminal.update(cx, |t, _| t.refresh_process());
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
            _subscriptions: subscriptions,
            _process_poll: process_poll,
        };
        this.add_tab(session, events, window, cx);
        this
    }

    fn add_tab(&mut self, session: Session, events: async_channel::Receiver<SessionEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let style = terminal_style(&self.config, &self.theme, cx);
        let (meta, blink) = (self.config.option_as_meta, self.config.cursor_blink);
        let terminal = cx.new(|cx| {
            let mut view = TerminalView::new(session, events, style, meta, blink, window, cx);
            view.refresh_process();
            view
        });
        let subscription = cx.subscribe_in(&terminal, window, |this, terminal, event, window, cx| match event {
            TerminalEvent::TitleChanged => cx.notify(),
            TerminalEvent::Exited => {
                if let Some(ix) = this.tabs.iter().position(|t| &t.terminal == terminal) {
                    this.close_tab_at(ix, window, cx);
                }
            }
        });
        self.tabs.push(Tab { terminal, _subscription: subscription });
        self.activate(self.tabs.len() - 1, window, cx);
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        self.active = ix;
        let focus = tab.terminal.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tabs.remove(ix);
        if self.tabs.is_empty() {
            window.remove_window();
            return;
        }
        let next = if self.active > ix || self.active >= self.tabs.len() { self.active.saturating_sub(1) } else { self.active };
        self.activate(next.min(self.tabs.len() - 1), window, cx);
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(window, cx);
    }

    /// Opens a shell tab and activates it.
    pub fn open_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // New tabs start where the active tab is, like Otty.
        let cwd = self.tabs.get(self.active).and_then(|tab| tab.terminal.read(cx).cwd.clone());
        match spawn_shell(&self.config, None, cwd) {
            Ok((session, events)) => self.add_tab(session, events, window, cx),
            Err(e) => tracing::error!("failed to start shell: {e:#}"),
        }
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_at(self.active, window, cx);
    }

    fn cycle(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len() as isize;
        if n > 0 {
            self.activate((self.active as isize + delta).rem_euclid(n) as usize, window, cx);
        }
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
        for tab in &self.tabs {
            tab.terminal.update(cx, |t, cx| t.set_style(style.clone(), cx));
        }
        cx.notify();
    }

    /// `sidebar-width` excludes the border, which sits outside it.
    fn sidebar_width(&self) -> f32 {
        let border = self.theme.sidebar.border.as_ref().map_or(0., |b| b.width);
        self.config.sidebar_width.unwrap_or(self.theme.sidebar.width) + border
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
            .pl(px(SIDEBAR_TEXT_INSET))
            .pr(px(SIDEBAR_ICON_INSET))
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
                let view = tab.terminal.read(cx);
                let title = view.display_title();
                let process = view.process.clone().unwrap_or_default();
                let active = ix == self.active;
                let style = if active { &t.tabs.active } else { &t.tabs.normal };
                let hover_bg = t.tabs.hover.background;
                let card = div()
                    .id(("tab", ix))
                    .h(px(style.height.unwrap_or(TAB_HEIGHT)))
                    .px(px(style.padding.left))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .rounded(px(style.radius))
                    .bg(hsla(style.background))
                    .shadow(shadows(&style.shadow))
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
                    // Otty's title sits half a point below and left of where
                    // the card's centering and padding put it.
                    .child(div().flex_1().min_w_0().pt(px(1.)).ml(px(-0.5)).truncate().child(title))
                    .child(
                        // The label column keeps room for six characters, so
                        // titles truncate at the same place for any process.
                        div()
                            .flex_none()
                            .min_w(px(LABEL_COLUMNS * LABEL_PITCH))
                            .flex()
                            .justify_end()
                            .font_family(self.mono_font.clone())
                            .text_size(px(LABEL_FONT_SIZE))
                            .text_color(hsla(if active { native_tertiary_label(t) } else { t.token.tertiary }))
                            .child(tracked(&process, LABEL_PITCH)),
                    );
                // Inactive cards get a transparent border so text stays aligned
                // with the bordered active card.
                match &style.border {
                    Some(b) => card.border(px(b.width)).border_color(hsla(t.border_color(b))),
                    None => card.border(px(1.)).border_color(gpui::transparent_black()),
                }
                .into_any_element()
            })
            .collect();

        sidebar
            .child(header)
            .child(
                // Cards sit on a 38pt pitch starting half a point above the
                // list, whose top edge clips the first card's border.
                div()
                    .overflow_hidden()
                    .child(div().mt(px(-0.5)).px(px(SIDEBAR_INSET)).flex().flex_col().gap(px(TAB_GAP)).children(tabs)),
            )
            .into_any_element()
    }
}

impl Workspace {
    /// Native materials behind the window and the sidebar, bottom-most first.
    fn backdrops_wanted(&self) -> Vec<Backdrop> {
        let t = &self.theme;
        vec![
            Backdrop { region: Region::Window, material: t.window.material },
            Backdrop { region: Region::Sidebar { width: self.sidebar_width() }, material: t.sidebar.material },
        ]
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
    match t.meta.mode {
        Mode::Light => Rgba { r: 0., g: 0., b: 0., a: 0.26 },
        Mode::Dark => Rgba { r: 1., g: 1., b: 1., a: 0.25 },
    }
}

/// Lays out `text` one character per fixed-width cell, which emulates the
/// letter-spacing AppKit applies to small labels (GPUI has no tracking).
fn tracked(text: &str, pitch: f32) -> gpui::Div {
    div()
        .flex()
        .children(text.chars().map(|c| div().w(px(pitch)).flex().justify_center().child(c.to_string())))
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
            blur_radius: px(s.blur),
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
        Some(b) => el.border_r(px(b.width)).border_color(hsla(t.border_color(b))),
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
        let sidebar = self.render_sidebar(cx);
        let t = &self.theme;
        let c = &t.container;
        let active = self.tabs.get(self.active).map(|tab| tab.terminal.clone());
        let title = active.as_ref().map(|a| a.read(cx).display_title()).unwrap_or_default();
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title.clone();
        }
        // Like Otty, a running command marks the window edited (the dot in
        // the close button), since closing it would kill that command.
        let busy = self.tabs.iter().any(|tab| tab.terminal.read(cx).busy);
        let state = (background_appearance(t), busy && !self.config.hide_dirty_indicator);
        if self.native_state != Some(state) {
            window.set_background_appearance(state.0);
            window.set_window_edited(state.1);
            self.native_state = Some(state);
        }
        let backdrops = self.backdrops_wanted();
        self.backdrops.sync(window, &backdrops);

        let title_band = div()
            .absolute()
            .top_0()
            .left_0()
            .right(px(TITLE_RIGHT_INSET))
            .h(px(TITLE_BAND))
            .flex()
            .items_center()
            .justify_center()
            .text_color(hsla(t.titlebar.foreground))
            .text_size(px(t.titlebar.font_size))
            .child(title);

        let container = with_border(
            div()
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
                .children(active),
            t,
            &c.border,
        );

        let main = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(hsla(t.terminal.background))
            .child(
                div()
                    .id("titlebar")
                    .h(px(t.titlebar.height.unwrap_or(TITLEBAR_RESERVE)))
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
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::new_tab))
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
            .bg(hsla(t.window.background))
            .font_family(self.ui_font.clone())
            .child(sidebar)
            .child(main)
    }
}
