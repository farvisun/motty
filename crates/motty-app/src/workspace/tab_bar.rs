//! The horizontal tab bar (`window-layout = tabs-top | tabs-bottom`): a
//! flat strip of tabs that share the width, up to 200pt each, then the new
//! tab button, and the notifications bell at the far end. Measured from
//! Otty's Night and Paper themes.

use gpui::{AnyElement, Context, FontWeight, MouseButton, div, prelude::*, px};
use motty_render::hsla;
use motty_theme::IconPlacement;

use super::{
    Badge, ToggleInbox, Workspace, badge_icon, native_label, progress_icon, tab_border, tab_look,
    tabs, titlebar,
};

/// A tab's widest; with many tabs they shrink, down to [`MIN_TAB`].
const MAX_TAB: f32 = 200.;
const MIN_TAB: f32 = 64.;
/// Between tabs.
const TAB_GAP: f32 = 2.;
/// The close button's box at a tab's right, 18pt from its edge to center.
const CLOSE_BOX: f32 = 14.;
const CLOSE_RIGHT: f32 = 11.;

impl Workspace {
    /// The sidebar is drawn: in its layout while shown, and in a tab bar
    /// layout only for Inbox.
    pub(super) fn sidebar_shown(&self) -> bool {
        if self.config.window_layout.horizontal() {
            self.inbox
        } else {
            self.sidebar_visible
        }
    }

    pub(super) fn tab_bar_shown(&self) -> bool {
        self.config.window_layout.horizontal() && self.sidebar_visible
    }

    /// The terminal card's margin, which themes can set per layout.
    pub(super) fn container_margin(&self) -> motty_theme::Edges {
        let c = &self.theme.container;
        match self.config.window_layout {
            motty_theme::WindowLayout::SidebarLeft => c.margin,
            motty_theme::WindowLayout::TabsTop => c.margin_tabs_top,
            motty_theme::WindowLayout::TabsBottom => c.margin_tabs_bottom,
        }
    }

    /// The bar's height, padding included.
    pub(super) fn tab_bar_height(&self) -> f32 {
        self.theme.tab_bar.height
    }

    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let bar = &t.tab_bar;
        let icon = hsla(native_label(t, 0.5, 0.55));
        let highlight = hsla(native_label(t, 0.08, 0.1));
        let tabs = (0..self.tabs.len()).map(|ix| self.render_bar_tab(ix, cx));
        let button = move |id: &'static str| {
            div()
                .id(id)
                .flex_none()
                .size(px(titlebar::BUTTON))
                .flex()
                .rounded(px(titlebar::RADIUS))
                .hover(move |s| s.bg(highlight))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        let top = self.config.window_layout == motty_theme::WindowLayout::TabsTop;
        div()
            .id("tab-bar")
            .flex_none()
            .h(px(bar.height))
            .pl(px(bar.padding.left))
            .pr(px(bar.padding.right))
            .flex()
            .items_center()
            .gap(px(4.))
            .when_some(bar.background, |d, bg| d.bg(hsla(bg)))
            // A border on the side that faces the terminal.
            .when_some(
                bar.border_bottom.clone().filter(|b| b.width > 0.),
                |d, b| {
                    let color = hsla(t.border_color(&b));
                    if top {
                        d.border_b(px(b.width)).border_color(color)
                    } else {
                        d.border_t(px(b.width)).border_color(color)
                    }
                },
            )
            // Double-clicking the empty strip zooms, as a titlebar does.
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(TAB_GAP))
                    .children(tabs),
            )
            .child(
                button("bar-new-tab")
                    .child(titlebar::plus(icon))
                    .on_click(cx.listener(|this, _, window, cx| this.open_tab(window, cx))),
            )
            .child(div().flex_1())
            .child(
                button("bar-notifications")
                    .when(self.inbox, |d| d.bg(highlight))
                    .child(titlebar::bell(icon))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_inbox(&ToggleInbox, window, cx)
                    })),
            )
            .into_any_element()
    }

    fn render_bar_tab(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let tab_styles = &t.tab_bar.tabs;
        let active = ix == self.active;
        let style = if active {
            &tab_styles.active
        } else {
            &tab_styles.normal
        };
        let hover_bg = tab_styles.hover.background;
        let height = style.height.unwrap_or(28.);
        let summary = self.tab_summary(&self.tabs[ix], cx);
        let badge = summary
            .badge
            .filter(|b| !(active && matches!(b, Badge::Finished | Badge::Done)));
        let separate = self.config.tab_icon_placement == IconPlacement::Separate;
        let mut rule_icon = tab_look::icon(&self.tabs[ix].look, hsla(style.foreground), t);
        let status = match (badge, summary.progress) {
            (Some(badge), _) if badge.needs_you() => Some(badge_icon(badge, ix, t)),
            (_, Some(progress)) => Some(progress_icon(progress, ix, t)),
            (Some(badge), None) => Some(badge_icon(badge, ix, t)),
            (None, None) if separate => None,
            (None, None) => rule_icon.take(),
        };
        let lead = rule_icon.filter(|_| separate);
        let renaming = self.renaming.as_ref().filter(|r| r.ix == ix && !r.divider);
        let title = match renaming {
            Some(rename) => self.rename_field(rename, cx),
            None => div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(summary.title.clone())
                .into_any_element(),
        };
        let close_color = hsla(style.foreground.with_alpha(0.6));
        let group = gpui::SharedString::from(format!("bar-tab-{ix}"));
        let close = div()
            .id(("bar-tab-close", ix))
            .flex_none()
            .size(px(CLOSE_BOX))
            .rounded(px(3.))
            .hover(|s| s.bg(hsla(style.foreground.with_alpha(0.1))))
            .when(!active, |d| {
                d.invisible().group_hover(group.clone(), |s| s.visible())
            })
            .child(titlebar::small_close(close_color))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| this.close_tab_at(ix, window, cx)));
        let card = div()
            .id(("bar-tab", ix))
            .group(group)
            .relative()
            .flex_shrink(1.)
            .w(px(MAX_TAB))
            .min_w(px(MIN_TAB))
            .h(px(height))
            .pl(px(style.padding.left))
            .pr(px(CLOSE_RIGHT - (CLOSE_BOX - 10.) / 2.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(style.radius))
            .bg(hsla(style.background))
            .when(style.background.a >= 1., |d| {
                d.shadow(super::shadows(&style.shadow))
            })
            .text_color(hsla(style.foreground))
            .text_size(px(style.font_size))
            .font_weight(FontWeight(style.font_weight))
            .when(!active, |d| d.hover(move |s| s.bg(hsla(hover_bg))))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    if this
                        .renaming
                        .as_ref()
                        .is_some_and(|r| r.ix == ix && !r.divider)
                    {
                        return;
                    }
                    this.activate(ix, window, cx);
                    if event.click_count == 2 {
                        this.start_rename(ix, window, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.close_tab_at(ix, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.show_tab_menu(ix, event.position, window, cx);
                }),
            )
            .on_drag(tabs::TabDrag { ix }, {
                let title = summary.title.clone();
                let weak = cx.entity().downgrade();
                move |_, _, _, cx| {
                    if let Some(this) = weak.upgrade() {
                        this.update(cx, |this, _| this.tab_drag_started(ix));
                    }
                    let title = title.clone();
                    cx.new(|_| tabs::DragPreview { title })
                }
            })
            .drag_over::<super::pane_drag::PaneDrag>(move |s, _, _, _| s.bg(hsla(hover_bg)))
            .on_drop(
                cx.listener(move |this, drag: &super::pane_drag::PaneDrag, window, cx| {
                    cx.stop_propagation();
                    this.drop_on_tab(drag.item.clone(), ix, window, cx)
                }),
            )
            .drag_over::<tabs::TabDrag>(move |s, _, _, _| s.bg(hsla(hover_bg)))
            .on_drop(cx.listener(move |this, drag: &tabs::TabDrag, window, cx| {
                this.move_tab(drag.ix, ix, window, cx)
            }))
            .tooltip({
                let this = cx.entity().downgrade();
                move |_, cx| {
                    let card = this.upgrade().and_then(|ws| ws.read(cx).hover_card(ix, cx));
                    match card {
                        Some(card) => cx.new(|_| card).into(),
                        None => cx.new(|_| gpui::Empty).into(),
                    }
                }
            })
            .children(lead)
            .child(title)
            .children(status.map(|s| {
                div()
                    .flex_none()
                    .font_family(self.mono_font.clone())
                    .text_size(px(super::LABEL_FONT_SIZE))
                    .child(s)
            }))
            .child(close);
        card.children(
            style
                .border
                .as_ref()
                .and_then(|b| tab_border(t, b, style.radius)),
        )
        .into_any_element()
    }
}
