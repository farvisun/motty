//! Window root: titlebar band and the themed container around the terminal.

use gpui::{
    AnyElement, App, BoxShadow, Context, Entity, Focusable, FontWeight, Subscription, Window, WindowAppearance, div, point,
    prelude::*, px,
};
use motty_render::{TerminalStyle, hsla};
use motty_term::{GridSize, Session, SessionConfig, SessionEvent};
use motty_theme::{Config, Mode, Shadow, Theme};

use crate::terminal_view::{TerminalEvent, TerminalView};

pub struct Workspace {
    config: Config,
    theme: Theme,
    mode: Mode,
    terminal: Entity<TerminalView>,
    _subscriptions: Vec<Subscription>,
}

pub fn mode_for(appearance: WindowAppearance) -> Mode {
    match appearance {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Mode::Dark,
        WindowAppearance::Light | WindowAppearance::VibrantLight => Mode::Light,
    }
}

/// Spawns the user's shell for a new pane.
/// `command` (from `-e`) replaces the login shell.
pub fn spawn_shell(config: &Config, command: Option<(String, Vec<String>)>) -> anyhow::Result<(Session, async_channel::Receiver<SessionEvent>)> {
    let session = SessionConfig {
        command: command.or_else(|| config.shell.clone().map(|s| (s, vec!["-l".into()]))),
        working_directory: dirs::home_dir(),
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
        let style = terminal_style(&config, &theme, cx);
        let option_as_meta = config.option_as_meta;
        let cursor_blink = config.cursor_blink;
        let terminal = cx.new(|cx| TerminalView::new(session, events, style, option_as_meta, cursor_blink, window, cx));
        let focus = terminal.focus_handle(cx);
        window.focus(&focus, cx);

        let subscriptions = vec![
            cx.subscribe_in(&terminal, window, |this, _, event, window, cx| match event {
                TerminalEvent::TitleChanged => {
                    if let Some(title) = this.terminal.read(cx).title.clone() {
                        window.set_window_title(&title);
                    }
                    cx.notify();
                }
                TerminalEvent::Exited => window.remove_window(),
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                let mode = mode_for(window.appearance());
                if mode != this.mode {
                    this.mode = mode;
                    this.reload_theme(cx);
                }
            }),
        ];

        Self { config, theme, mode, terminal, _subscriptions: subscriptions }
    }

    pub fn reload_theme(&mut self, cx: &mut Context<Self>) {
        let parsed = self.config.resolve_theme(self.mode);
        for w in &parsed.warnings {
            tracing::warn!("{w}");
        }
        self.theme = parsed.theme;
        let style = terminal_style(&self.config, &self.theme, cx);
        self.terminal.update(cx, |t, cx| t.set_style(style, cx));
        cx.notify();
    }
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

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let c = &t.container;
        let title = self.terminal.read(cx).title.clone().unwrap_or_else(|| "Motty".into());

        let titlebar: AnyElement = div()
            .h(px(t.titlebar.height))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .when_some(t.titlebar.background, |d, bg| d.bg(hsla(bg)))
            .text_color(hsla(t.titlebar.foreground))
            .text_size(px(t.titlebar.font_size))
            .font_weight(FontWeight(t.titlebar.font_weight))
            .child(title)
            .into_any_element();

        let mut container = div()
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
            .child(self.terminal.clone());
        if let Some(border) = &c.border {
            container = container.border(px(border.width)).border_color(hsla(t.border_color(border)));
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(t.window.background))
            .font_family(t.token.font_ui.iter().find(|f| !f.starts_with('-')).cloned().unwrap_or_else(|| ".SystemUIFont".into()))
            .child(titlebar)
            .child(container)
    }
}
