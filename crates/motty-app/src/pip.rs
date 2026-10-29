//! View ▸ Picture in Picture: a small window that floats over every app and
//! Space, showing one terminal pane (Current Pane) or whichever is active
//! (Follow Active Pane). It draws the pane's screen scaled to fit, without
//! resizing the shell behind it.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, Entity, Global, Render, Subscription, TitlebarOptions, WeakEntity,
    Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, point, prelude::*, px,
    size,
};
use motty_render::{TerminalElement, TerminalStyle, hsla, measure};
use motty_term::alacritty_terminal::grid::Dimensions;

use crate::terminal_view::TerminalView;
use crate::workspace::Workspace;

/// What the window shows.
pub enum Source {
    Pane(Entity<TerminalView>),
    /// The active pane of this window's workspace, as it changes.
    Follow(WeakEntity<Workspace>),
}

/// The open Picture in Picture window, if any.
struct Open(WindowHandle<PipView>);

impl Global for Open {}

pub struct PipView {
    source: Source,
    /// The pane shown last, and the subscription that redraws with it.
    shown: Option<(Entity<TerminalView>, Subscription)>,
    _workspace: Option<Subscription>,
}

/// Opens Picture in Picture on `source`, replacing one already open. Deferred:
/// the new window's first render reads the workspace, which may be the
/// entity being updated right now.
pub fn open(source: Source, cx: &mut App) {
    cx.defer(move |cx| open_now(source, cx));
}

fn open_now(source: Source, cx: &mut App) {
    close(cx);
    let bounds = cx
        .primary_display()
        .map(|d| d.bounds())
        .map(|d| {
            let size = size(px(480.), px(300.));
            let at = point(
                d.right() - size.width - px(24.),
                d.bottom() - size.height - px(80.),
            );
            Bounds::new(at, size)
        })
        .unwrap_or_else(|| Bounds::new(point(px(100.), px(100.)), size(px(480.), px(300.))));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Picture in Picture".into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        kind: WindowKind::Floating,
        focus: false,
        window_min_size: Some(size(px(200.), px(120.))),
        app_id: Some("dev.motty.Motty".into()),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        crate::materials::float_everywhere(window);
        cx.new(|cx| PipView::new(source, cx))
    }) {
        Ok(handle) => cx.set_global(Open(handle)),
        Err(e) => tracing::error!("couldn't open Picture in Picture: {e:#}"),
    }
}

/// Closes Picture in Picture if it's open.
pub fn close(cx: &mut App) {
    if let Some(Open(handle)) = cx.try_global::<Open>() {
        let handle = *handle;
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        cx.remove_global::<Open>();
    }
}

pub fn is_open(cx: &App) -> bool {
    cx.try_global::<Open>()
        .is_some_and(|Open(handle)| handle.read(cx).is_ok())
}

impl PipView {
    fn new(source: Source, cx: &mut Context<Self>) -> Self {
        let workspace = match &source {
            Source::Follow(ws) => ws
                .upgrade()
                .map(|ws| cx.observe(&ws, |_, _, cx| cx.notify())),
            Source::Pane(_) => None,
        };
        Self {
            source,
            shown: None,
            _workspace: workspace,
        }
    }

    fn pane(&self, cx: &App) -> Option<Entity<TerminalView>> {
        match &self.source {
            Source::Pane(view) => Some(view.clone()),
            Source::Follow(ws) => ws.upgrade()?.read(cx).focused_pane(),
        }
    }
}

impl Render for PipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.pane(cx);
        if pane.as_ref() != self.shown.as_ref().map(|(p, _)| p) {
            self.shown = pane
                .clone()
                .map(|p| (p.clone(), cx.observe(&p, |_, _, cx| cx.notify())));
        }
        let Some(pane) = pane else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black())
                .text_color(gpui::white().opacity(0.6))
                .text_size(px(13.))
                .child("No terminal to show")
                .into_any_element();
        };
        let view = pane.read(cx);
        let term = view.session().term.clone();
        let (cols, rows) = {
            let t = term.lock();
            (t.columns(), t.screen_lines())
        };
        let style = fitted(&view.style(), cols, rows, window);
        let bg = hsla(style.background);
        let element = TerminalElement::new(
            term,
            Rc::new(style),
            false,
            false,
            Rc::new(Cell::new(None)),
            |_, _, _| {},
        );
        div()
            .group("pip")
            .relative()
            .size_full()
            .bg(bg)
            .overflow_hidden()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.start_window_move()
            })
            .child(element)
            .child(
                div()
                    .absolute()
                    .top(px(8.))
                    .left(px(8.))
                    .right(px(8.))
                    .flex()
                    .justify_between()
                    .invisible()
                    .group_hover("pip", |s| s.visible())
                    .child(
                        control("pip-close", "icons/x.svg", "Close Picture in Picture").on_click(
                            |_, _, cx| {
                                cx.stop_propagation();
                                close(cx);
                            },
                        ),
                    )
                    .child(
                        control(
                            "pip-return",
                            "icons/picture-in-picture-2.svg",
                            "Back to Motty",
                        )
                        .on_click(|_, _, cx| {
                            cx.stop_propagation();
                            close(cx);
                            cx.activate(true);
                        }),
                    ),
            )
            .into_any_element()
    }
}

/// A round button in the hover controls, like the system PiP window's.
fn control(id: &'static str, icon: &'static str, tip: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(gpui::black().opacity(0.55))
        .hover(|s| s.bg(gpui::black().opacity(0.75)))
        .tooltip(move |_, cx| crate::tooltip::text(tip, cx))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            gpui::svg()
                .path(icon)
                .size(px(13.))
                .text_color(gpui::white().opacity(0.9)),
        )
}

/// The pane's style at a font size that fits its `cols` × `rows` screen in
/// the window.
fn fitted(style: &TerminalStyle, cols: usize, rows: usize, window: &mut Window) -> TerminalStyle {
    let mut style = style.clone();
    let metrics = measure(&style, window);
    let pad = style.padding;
    let width = metrics.cell_width * cols as f32 + px(pad.left + pad.right);
    let height = metrics.line_height * rows as f32 + px(pad.top + pad.bottom);
    let area = window.viewport_size();
    let scale = (area.width / width).min(area.height / height).min(1.);
    style.font_size = (style.font_size * scale).max(px(4.));
    let shrink = |v: f32| v * scale;
    style.padding.left = shrink(pad.left);
    style.padding.right = shrink(pad.right);
    style.padding.top = shrink(pad.top);
    style.padding.bottom = shrink(pad.bottom);
    style
}
