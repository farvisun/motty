//! A plain text tooltip, drawn like macOS's: a small pale box with a hairline.

use gpui::{
    AnyView, App, Context, IntoElement, Render, SharedString, Window, div, prelude::*, px, rgb,
    rgba,
};

struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dark = matches!(
            window.appearance(),
            gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
        );
        div()
            .mt(px(18.))
            .px(px(6.))
            .py(px(2.))
            .rounded(px(4.))
            .border_1()
            .border_color(if dark {
                rgba(0xffffff26)
            } else {
                rgba(0x00000026)
            })
            .bg(if dark { rgb(0x2b2b2b) } else { rgb(0xf5f5f5) })
            .text_size(px(11.))
            .text_color(if dark { rgb(0xe8e8e8) } else { rgb(0x1d1d1d) })
            .shadow_sm()
            .child(self.0.clone())
    }
}

pub fn text(text: impl Into<SharedString>, cx: &mut App) -> AnyView {
    let text = text.into();
    cx.new(|_| Tip(text)).into()
}
