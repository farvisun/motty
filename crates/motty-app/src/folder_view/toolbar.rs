//! The bar above a folder pane, as Otty draws it: Find, then up, Open in
//! Terminal, hidden files, Send to Chat and Share; Close on the right.

use gpui::{AnyElement, Context, MouseButton, Window, div, prelude::*, px};
use motty_render::hsla;

use super::{FolderEvent, FolderView, accent};

const HEIGHT: f32 = 32.;
/// The Find field, measured from Otty: 140×20, 9pt from the edge.
const FIND_W: f32 = 140.;
const FIND_H: f32 = 20.;
/// Icon buttons: 14pt glyphs in 24pt boxes, 30pt apart.
const ICON: f32 = 14.;
const ICON_BOX: f32 = 24.;

impl FolderView {
    pub(super) fn render_toolbar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let fg = self.foreground();
        let ink = move |a: f32| {
            let mut c = fg;
            c.a = a;
            hsla(c)
        };
        let icon = |id: &'static str, path: &'static str, tip: &'static str| {
            div()
                .id(id)
                .flex_none()
                .size(px(ICON_BOX))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .hover(move |s| s.bg(ink(0.08)))
                .tooltip(move |_, cx| crate::tooltip::text(tip, cx))
                .child(gpui::svg().path(path).size(px(ICON)).text_color(ink(0.8)))
        };

        let typing = self.find_focus.is_focused(window);
        let caret = typing.then(|| div().flex_none().w(px(1.5)).h(px(14.)).bg(accent()));
        let text = if self.query.is_empty() {
            div()
                .flex()
                .items_center()
                .children(caret)
                .child(div().text_color(ink(0.45)).child("Find"))
        } else {
            div()
                .flex()
                .items_center()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(ink(0.9))
                .child(self.query.clone())
                .children(caret)
        };
        let find = div()
            .id("folder-find")
            .track_focus(&self.find_focus)
            .flex_none()
            .w(px(FIND_W))
            .h(px(FIND_H))
            .pl(px(6.))
            .pr(px(6.))
            .flex()
            .items_center()
            .gap(px(4.))
            .rounded(px(6.))
            .overflow_hidden()
            .bg(if self.style.dark {
                gpui::hsla(0., 0., 0., 0.25)
            } else {
                ink(0.06)
            })
            .text_size(px(13.))
            .cursor_text()
            .child(
                gpui::svg()
                    .flex_none()
                    .path("icons/search.svg")
                    .size(px(11.))
                    .text_color(ink(0.6)),
            )
            .child(text)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    window.focus(&this.find_focus, cx);
                    cx.notify();
                }),
            );

        let hidden = self.hidden;
        let left = div()
            .flex()
            .items_center()
            .child(find)
            .child(
                div()
                    .ml(px(9.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        icon("folder-up", "icons/arrow-up.svg", "Enclosing Folder")
                            .on_click(cx.listener(|this, _, _, cx| this.go_up(cx))),
                    )
                    .child(
                        icon(
                            "folder-terminal",
                            "icons/arrow-right-to-line.svg",
                            "Open in Terminal",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.emit(FolderEvent::OpenTerminal(this.path.clone()))
                        })),
                    )
                    .child(
                        if hidden {
                            icon("folder-hidden", "icons/eye.svg", "Hide Hidden Files")
                        } else {
                            icon("folder-hidden", "icons/eye-off.svg", "Show Hidden Files")
                        }
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hidden = !this.hidden;
                            this.rebuild();
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .ml(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        icon("folder-chat", "icons/message-square.svg", "Send to Chat")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(crate::actions::SendToChat), cx)
                            }),
                    )
                    .child({
                        let path = self.path.clone();
                        icon("folder-share", "icons/share.svg", "Share").on_click(
                            move |event, window, _| {
                                crate::share::show(
                                    window,
                                    crate::share::Shared::File(&path),
                                    event.position(),
                                )
                            },
                        )
                    }),
            );

        let close = div()
            .id("folder-close")
            .flex_none()
            .h(px(24.))
            .px(px(4.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(5.))
            .text_size(px(13.))
            .text_color(ink(0.9))
            .font_weight(gpui::FontWeight::MEDIUM)
            .hover(move |s| s.bg(ink(0.07)))
            .child(
                gpui::svg()
                    .path("icons/x.svg")
                    .size(px(13.))
                    .text_color(ink(0.9)),
            )
            .child("Close")
            .on_click(cx.listener(|this, _, window, cx| {
                // ⌘W on this pane.
                window.focus(&this.focus, cx);
                window.defer(cx, |window, cx| {
                    window.dispatch_action(Box::new(crate::actions::CloseTab), cx)
                });
            }));

        div()
            .flex_none()
            .h(px(HEIGHT))
            .pl(px(9.))
            .pr(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .child(left)
            .child(close)
            .into_any_element()
    }
}
