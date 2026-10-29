//! The Composer's Float Panel: a Spotlight-style panel the Composer pops
//! out into, over every app, so a prompt can be drafted while working in
//! another one. It shows the pane's own Composer, draft and chips; sending
//! or closing docks it back into the pane.

use std::rc::Rc;

use gpui::{
    App, Bounds, Entity, Focusable, Pixels, PlatformDisplay, Point, Render, Size, Subscription,
    TitlebarOptions, WeakEntity, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, canvas, div, point, prelude::*, px, size,
};

use super::TerminalView;
use crate::editor::FileEditor;

const WIDTH: f32 = 640.;
/// Before the first frame fits the panel to the Composer.
const HEIGHT: f32 = 160.;

pub struct ComposerPanel {
    view: WeakEntity<TerminalView>,
    _subscriptions: [Subscription; 2],
}

/// Opens the panel on `display` (the pane's), typing into `editor`.
pub(super) fn open(
    view: WeakEntity<TerminalView>,
    editor: Entity<FileEditor>,
    display: Option<Rc<dyn PlatformDisplay>>,
    cx: &mut App,
) -> Option<WindowHandle<ComposerPanel>> {
    let pane = view.upgrade()?;
    let area = display
        .clone()
        .or_else(|| cx.primary_display())
        .map(|d| d.visible_bounds())?;
    let size = size(px(WIDTH), px(HEIGHT));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            spotlight(area, size),
            size,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Composer".into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        // A non-activating panel that can become key: typing goes to it
        // while another app stays active.
        kind: WindowKind::PopUp,
        focus: true,
        is_resizable: false,
        is_minimizable: false,
        app_owns_titlebar_drag: true,
        display_id: display.map(|d| d.id()),
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("dev.motty.Motty".into()),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        crate::materials::float_panel(window);
        window.focus(&editor.focus_handle(cx), cx);
        cx.new(|cx| ComposerPanel::new(&pane, window, cx))
    }) {
        Ok(handle) => Some(handle),
        Err(e) => {
            tracing::error!("couldn't open the Composer's panel: {e:#}");
            None
        }
    }
}

pub(super) fn close(panel: WindowHandle<ComposerPanel>, cx: &mut App) {
    let _ = panel.update(cx, |_, window, _| window.remove_window());
}

/// Where a panel of `size` goes on a screen whose visible part is `area`:
/// centered across, its middle a third of the way down, as Spotlight sits.
fn spotlight(area: Bounds<Pixels>, size: Size<Pixels>) -> Point<Pixels> {
    let x = (area.size.width - size.width) / 2.;
    let y = area.size.height / 3. - size.height / 2.;
    point(area.left() + x.max(px(0.)), area.top() + y.max(px(0.)))
}

impl ComposerPanel {
    fn new(pane: &Entity<TerminalView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            view: pane.downgrade(),
            _subscriptions: [
                cx.observe(pane, |_, _, cx| cx.notify()),
                // The pane closed: its Composer goes with it.
                cx.observe_release_in(pane, window, |_, _, window, _| window.remove_window()),
            ],
        }
    }
}

impl Render for ComposerPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let card = self
            .view
            .upgrade()
            .and_then(|view| view.update(cx, |v, cx| v.render_floating_composer(cx)));
        let Some(card) = card else {
            return div().into_any_element();
        };
        // The window takes the card's height, growing down as the draft does.
        let fit = canvas(
            |bounds, window, cx| {
                let height = bounds.size.height;
                if (height - window.viewport_size().height).abs() >= px(0.5) {
                    crate::materials::set_content_height(window, height.as_f32(), cx);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        div()
            .size_full()
            .child(div().relative().w_full().child(card).child(fit))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sits_like_spotlight() {
        // A 1512×945 screen below a 37pt menu bar.
        let area = Bounds::new(point(px(0.), px(37.)), size(px(1512.), px(945.)));
        let at = spotlight(area, size(px(640.), px(160.)));
        assert_eq!(at, point(px(436.), px(37. + 315. - 80.)));
        // In the upper third.
        assert!(at.y + px(160.) <= area.top() + area.size.height / 2.);
    }

    #[test]
    fn stays_on_a_small_screen() {
        let area = Bounds::new(point(px(0.), px(25.)), size(px(600.), px(300.)));
        let at = spotlight(area, size(px(640.), px(240.)));
        assert_eq!(at, point(px(0.), px(25.)));
    }

    #[test]
    fn follows_the_screens_origin() {
        let area = Bounds::new(point(px(80.), px(0.)), size(px(1000.), px(900.)));
        let at = spotlight(area, size(px(600.), px(200.)));
        assert_eq!(at, point(px(280.), px(200.)));
    }
}
