//! The bar above an image or Quick Look pane, drawn as the editor's is
//! (`editor/toolbar.rs`): icon buttons on the left, Close on the right.

use std::path::PathBuf;

use gpui::{AnyElement, Div, FocusHandle, Hsla, MouseButton, Stateful, div, prelude::*, px};
use motty_render::hsla;
use motty_theme::Rgba;

pub const HEIGHT: f32 = 32.;
/// Icon buttons: 14pt glyphs in 24pt boxes, 28pt apart.
const ICON: f32 = 14.;
const ICON_BOX: f32 = 24.;

/// The foreground at alpha `a`, as the toolbar's ink.
#[derive(Clone, Copy)]
pub struct Ink(pub Rgba);

impl Ink {
    pub fn at(self, a: f32) -> Hsla {
        let mut c = self.0;
        c.a = a;
        hsla(c)
    }
}

/// An icon button with a tooltip.
pub fn icon(id: &'static str, path: &'static str, tip: &'static str, ink: Ink) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(ICON_BOX))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .hover(move |s| s.bg(ink.at(0.08)))
        .tooltip(move |_, cx| crate::tooltip::text(tip, cx))
        .child(
            gpui::svg()
                .path(path)
                .size(px(ICON))
                .text_color(ink.at(0.8)),
        )
}

/// Send to Chat: dispatches [`crate::actions::SendToChat`].
pub fn send_to_chat(ink: Ink) -> Stateful<Div> {
    icon("file-chat", "icons/message-square.svg", "Send to Chat", ink)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::actions::SendToChat), cx))
}

/// Share: macOS's share menu for `path`, below the click.
pub fn share(path: PathBuf, ink: Ink) -> Stateful<Div> {
    icon("file-share", "icons/share.svg", "Share", ink).on_click(move |event, window, _| {
        crate::share::show(window, crate::share::Shared::File(&path), event.position())
    })
}

/// "× Close": focuses the pane, then closes it as ⌘W would.
pub fn close(focus: FocusHandle, ink: Ink) -> Stateful<Div> {
    let color = ink.at(0.9);
    div()
        .id("file-close")
        .flex_none()
        .h(px(24.))
        .px(px(4.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(5.))
        .text_size(px(13.))
        .text_color(color)
        .font_weight(gpui::FontWeight::MEDIUM)
        .hover(move |s| s.bg(ink.at(0.07)))
        .child(
            gpui::svg()
                .path("icons/x.svg")
                .size(px(13.))
                .text_color(color),
        )
        .child("Close")
        .on_click(move |_, window, cx| {
            window.focus(&focus, cx);
            window.defer(cx, |window, cx| {
                window.dispatch_action(Box::new(crate::actions::CloseTab), cx)
            });
        })
}

/// The bar, `left` and `right` at its ends.
pub fn bar(left: impl IntoElement, right: impl IntoElement) -> AnyElement {
    div()
        .flex_none()
        .h(px(HEIGHT))
        .pl(px(10.))
        .pr(px(10.))
        .flex()
        .items_center()
        .justify_between()
        .child(left)
        .child(right)
        .into_any_element()
}
