//! Moving panes by hand, as Otty does: a capsule on each pane's top edge
//! picks it up (a click fans out Split Right, Split Bottom and Zoom), and
//! where it's dropped decides what happens: a pane's edge splits that pane,
//! its middle swaps the two, a tab merges it in, the tab list's empty space
//! makes it a tab, and outside the window, a window.

use std::collections::HashMap;

use gpui::{
    AnyElement, App, Bounds, Context, DragMoveEvent, EntityId, MouseButton, MouseUpEvent, Pixels,
    Point, Window, div, prelude::*, px, relative,
};
use motty_render::hsla;
use motty_theme::Rgba;

use super::tabs::DragPreview;
use super::{Content, Panes, Tab, Workspace};
use crate::panes::{Direction, Pane, PaneItem};

/// What dragging a pane's capsule carries.
#[derive(Clone)]
pub(super) struct PaneDrag {
    pub item: PaneItem,
}

/// Where over a pane a dragged one would land.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Zone {
    Edge(Direction),
    /// The inner square: swap.
    Center,
}

/// The zone at `at` within `bounds`: the middle half swaps, else the
/// nearest edge.
fn zone_at(bounds: Bounds<Pixels>, at: Point<Pixels>) -> Zone {
    let x = ((at.x - bounds.left()) / bounds.size.width).clamp(0., 1.);
    let y = ((at.y - bounds.top()) / bounds.size.height).clamp(0., 1.);
    if (0.25..=0.75).contains(&x) && (0.25..=0.75).contains(&y) {
        return Zone::Center;
    }
    let edges = [
        (x, Direction::Left),
        (1. - x, Direction::Right),
        (y, Direction::Up),
        (1. - y, Direction::Down),
    ];
    let (_, d) = edges
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .expect("four edges");
    Zone::Edge(d)
}

/// The pane drag's state between frames.
#[derive(Default)]
pub(super) struct PaneDragState {
    /// The pane whose capsule's actions are fanned out.
    menu: Option<EntityId>,
    /// The pane under a drag, and the zone it would land in.
    target: Option<(EntityId, Zone)>,
    /// A pane or tab being dragged, for a drop outside the window.
    dragging: Option<Dragged>,
}

#[derive(Clone)]
enum Dragged {
    Pane(PaneItem),
    Tab(usize),
}

impl Workspace {
    /// Each pane's capsule, its actions, and the drop zones while a pane is
    /// being dragged; drawn over the pane.
    pub(super) fn pane_overlays(
        &self,
        leaves: &[PaneItem],
        cx: &mut Context<Self>,
    ) -> HashMap<EntityId, AnyElement> {
        let dragging = match &self.pane_drag.dragging {
            Some(Dragged::Pane(item)) if cx.has_active_drag() => Some(item.clone()),
            _ => None,
        };
        let fg = self.theme.token.foreground;
        let ink = move |a: f32| {
            let mut c = fg;
            c.a = a;
            hsla(c)
        };
        let blue = hsla(Rgba::rgb(0x047afe));
        let green = hsla(Rgba::rgb(0x30d158));
        leaves
            .iter()
            .map(|item| {
                let id = item.entity_id();
                let menu = self.pane_drag.menu == Some(id);
                let handle = div()
                    .id(("pane-handle", id.as_u64()))
                    .group("pane-handle")
                    .absolute()
                    .top_0()
                    .left(relative(0.5))
                    .ml(px(-40.))
                    .w(px(80.))
                    .h(px(12.))
                    .flex()
                    .justify_center()
                    .pt(px(3.))
                    .cursor_grab()
                    .child(
                        div()
                            .w(px(36.))
                            .h(px(5.))
                            .rounded_full()
                            .bg(ink(0.35))
                            .invisible()
                            .group_hover("pane-handle", |s| s.visible())
                            .when(menu, |d| d.visible()),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let open = this.pane_drag.menu == Some(id);
                        this.pane_drag.menu = (!open).then_some(id);
                        cx.notify();
                    }))
                    .on_drag(PaneDrag { item: item.clone() }, {
                        let weak = cx.entity().downgrade();
                        let title = item.title(cx);
                        move |drag: &PaneDrag, _, _, cx| {
                            let item = drag.item.clone();
                            if let Some(this) = weak.upgrade() {
                                this.update(cx, |this, _| {
                                    this.pane_drag.menu = None;
                                    this.pane_drag.dragging = Some(Dragged::Pane(item));
                                });
                            }
                            let title = title.clone();
                            cx.new(|_| DragPreview { title })
                        }
                    });
                let mut overlay = div().absolute().inset_0().child(handle);
                if menu {
                    overlay = overlay.child(self.render_pane_actions(item, ink, cx));
                }
                if dragging.as_ref().is_some_and(|d| d != item) {
                    let zone = self
                        .pane_drag
                        .target
                        .filter(|(t, _)| *t == id)
                        .map(|(_, z)| z);
                    overlay = overlay.child(
                        div()
                            .id(("pane-drop", id.as_u64()))
                            .absolute()
                            .inset_0()
                            .on_drag_move::<PaneDrag>(cx.listener(
                                move |this, event: &DragMoveEvent<PaneDrag>, _, cx| {
                                    let at = event.event.position;
                                    let here = event.bounds.contains(&at);
                                    let target = here.then(|| (id, zone_at(event.bounds, at)));
                                    let was = this.pane_drag.target;
                                    if here && was != target {
                                        this.pane_drag.target = target;
                                        cx.notify();
                                    } else if !here && was.is_some_and(|(t, _)| t == id) {
                                        this.pane_drag.target = None;
                                        cx.notify();
                                    }
                                },
                            ))
                            .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                                let zone = this
                                    .pane_drag
                                    .target
                                    .filter(|(t, _)| *t == id)
                                    .map_or(Zone::Center, |(_, z)| z);
                                this.drop_on_pane(drag.item.clone(), id, zone, window, cx);
                            }))
                            .children(zone.map(|zone| {
                                let shade = div().absolute().rounded(px(6.));
                                match zone {
                                    Zone::Center => shade
                                        .left(relative(0.25))
                                        .top(relative(0.25))
                                        .size(relative(0.5))
                                        .bg(green.opacity(0.25))
                                        .border_2()
                                        .border_color(green.opacity(0.7)),
                                    Zone::Edge(d) => {
                                        let s = shade.bg(blue.opacity(0.22)).border_2();
                                        let s = s.border_color(blue.opacity(0.7));
                                        match d {
                                            Direction::Left => {
                                                s.left_0().top_0().bottom_0().w(relative(0.5))
                                            }
                                            Direction::Right => {
                                                s.right_0().top_0().bottom_0().w(relative(0.5))
                                            }
                                            Direction::Up => {
                                                s.left_0().right_0().top_0().h(relative(0.5))
                                            }
                                            Direction::Down => {
                                                s.left_0().right_0().bottom_0().h(relative(0.5))
                                            }
                                        }
                                    }
                                }
                            })),
                    );
                }
                (id, overlay.into_any_element())
            })
            .collect()
    }

    /// Split Right, Split Bottom and Zoom, fanned out under the capsule.
    fn render_pane_actions(
        &self,
        item: &PaneItem,
        ink: impl Fn(f32) -> gpui::Hsla + Copy + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = |name: &'static str, icon: &'static str, tip: &'static str| {
            div()
                .id(name)
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_default()
                .hover(move |s| s.bg(ink(0.12)))
                .tooltip(move |_, cx| crate::tooltip::text(tip, cx))
                .child(gpui::svg().path(icon).size(px(14.)).text_color(ink(0.85)))
        };
        let act =
            |item: &PaneItem, what: fn(&mut Workspace, &mut Window, &mut Context<Workspace>)| {
                let item = item.clone();
                cx.listener(
                    move |this: &mut Workspace, _: &gpui::ClickEvent, window, cx| {
                        this.pane_drag.menu = None;
                        this.focus_pane(item.clone(), window, cx);
                        what(this, window, cx);
                    },
                )
            };
        let bg = self.theme.window.background;
        div()
            .absolute()
            .top(px(14.))
            .left(relative(0.5))
            .ml(px(-46.))
            .p(px(3.))
            .flex()
            .gap(px(2.))
            .rounded(px(9.))
            .bg(hsla(bg))
            .border_1()
            .border_color(ink(0.12))
            .shadow_md()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                button(
                    "pane-split-right",
                    "icons/square-split-horizontal.svg",
                    "Split Right",
                )
                .on_click(act(item, |this, window, cx| {
                    let _ = this.split_pane(Direction::Right, None, None, true, window, cx);
                })),
            )
            .child(
                button(
                    "pane-split-down",
                    "icons/square-split-vertical.svg",
                    "Split Bottom",
                )
                .on_click(act(item, |this, window, cx| {
                    let _ = this.split_pane(Direction::Down, None, None, true, window, cx);
                })),
            )
            .child(
                button("pane-zoom", "icons/maximize-2.svg", "Zoom")
                    .on_click(act(item, |this, _, cx| this.toggle_zoom(cx))),
            )
            .into_any_element()
    }

    /// Notes a tab drag, so a drop outside the window can tear it off.
    pub(super) fn tab_drag_started(&mut self, ix: usize) {
        self.pane_drag.dragging = Some(Dragged::Tab(ix));
    }

    /// A button released outside the window ends a drag there: the pane or
    /// tab goes to a window of its own. (Mouse listeners go in at paint.)
    pub(super) fn watch_tear_off(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        gpui::canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let weak = weak.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx: &mut App| {
                    if phase != gpui::DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    let Some(this) = weak.upgrade() else { return };
                    let size = window.viewport_size();
                    let at = event.position;
                    let outside =
                        at.x < px(0.) || at.y < px(0.) || at.x > size.width || at.y > size.height;
                    this.update(cx, |this, cx| {
                        let dragged = this.pane_drag.dragging.take();
                        this.pane_drag.target = None;
                        if !outside {
                            return cx.notify();
                        }
                        match dragged {
                            Some(Dragged::Pane(item)) => this.tear_off_pane(item, window, cx),
                            Some(Dragged::Tab(ix)) => this.move_tab_to_new_window(ix, window, cx),
                            None => {}
                        }
                    });
                });
            },
        )
        .absolute()
        .size_0()
        .into_any_element()
    }

    /// Takes `item` out of its tab without ending it; a tab left empty goes.
    fn take_pane(&mut self, item: &PaneItem) -> bool {
        let Some(ix) = self.tab_of(item) else {
            return false;
        };
        let Some(tab) = self.tabs[ix].panes_mut() else {
            return false;
        };
        if tab.root.leaves().len() == 1 {
            self.tabs.remove(ix);
            if self.active > ix || self.active >= self.tabs.len() {
                self.active = self.active.saturating_sub(1);
            }
            return true;
        }
        if tab.focused == *item {
            let near = [
                Direction::Left,
                Direction::Up,
                Direction::Right,
                Direction::Down,
            ]
            .into_iter()
            .find_map(|d| tab.root.neighbor(item, d));
            tab.focused = near.unwrap_or_else(|| tab.root.leaves()[0].clone());
        }
        tab.zoomed = false;
        let root = std::mem::replace(&mut tab.root, Pane::Leaf(tab.focused.clone()));
        tab.root = root.without(item).expect("another pane remains");
        true
    }

    /// Whether `item` is the only pane of the only tab.
    fn last_pane(&self, item: &PaneItem) -> bool {
        self.tabs.len() == 1 && self.tab_of(item).is_some_and(|ix| self.leaf_count(ix) == 1)
    }

    fn leaf_count(&self, ix: usize) -> usize {
        self.tabs
            .get(ix)
            .and_then(|t| t.panes())
            .map_or(0, |p| p.root.leaves().len())
    }

    fn drop_on_pane(
        &mut self,
        item: PaneItem,
        target: EntityId,
        zone: Zone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_drag.target = None;
        self.pane_drag.dragging = None;
        if item.entity_id() == target {
            return;
        }
        let Some(target_item) = self
            .tabs
            .iter()
            .filter_map(|t| t.panes())
            .flat_map(|p| p.root.leaves())
            .find(|l| l.entity_id() == target)
            .cloned()
        else {
            return;
        };
        match zone {
            Zone::Center => {
                let (Some(a), Some(b)) = (self.tab_of(&item), self.tab_of(&target_item)) else {
                    return;
                };
                for ix in [a, b] {
                    if let Some(p) = self.tabs[ix].panes_mut() {
                        p.root.swap(&item, &target_item);
                        if p.focused == item {
                            p.focused = target_item.clone();
                        } else if p.focused == target_item {
                            p.focused = item.clone();
                        }
                    }
                    if a == b {
                        break;
                    }
                }
            }
            Zone::Edge(direction) => {
                if !self.take_pane(&item) {
                    return;
                }
                let Some(ix) = self.tab_of(&target_item) else {
                    return;
                };
                let id = self.next_split;
                self.next_split += 1;
                if let Some(p) = self.tabs[ix].panes_mut() {
                    p.root.split(&target_item, item.clone(), direction, id);
                }
                self.active = ix;
            }
        }
        self.focus_pane(item, window, cx);
        cx.notify();
    }

    /// A pane dropped on tab `ix` joins it, split to the right.
    pub(super) fn drop_on_tab(
        &mut self,
        item: PaneItem,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_drag.dragging = None;
        if self.tab_of(&item) == Some(ix) || self.tabs.get(ix).and_then(|t| t.panes()).is_none() {
            return;
        }
        let Some(target) = self.tabs[ix].panes().map(|p| p.focused.clone()) else {
            return;
        };
        if !self.take_pane(&item) {
            return;
        }
        let Some(ix) = self.tab_of(&target) else {
            return;
        };
        let id = self.next_split;
        self.next_split += 1;
        if let Some(p) = self.tabs[ix].panes_mut() {
            p.root.split(&target, item.clone(), Direction::Right, id);
        }
        self.activate(ix, window, cx);
        self.focus_pane(item, window, cx);
    }

    /// A pane dropped on the tab list's empty space becomes a tab.
    pub(super) fn drop_as_tab(
        &mut self,
        item: PaneItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_drag.dragging = None;
        let alone = self
            .tab_of(&item)
            .is_some_and(|ix| self.leaf_count(ix) == 1);
        if alone || !self.take_pane(&item) {
            return;
        }
        self.tabs
            .push(Tab::holding(Content::Panes(Panes::new(item))));
        self.activate(self.tabs.len() - 1, window, cx);
    }

    fn tear_off_pane(&mut self, item: PaneItem, window: &mut Window, cx: &mut Context<Self>) {
        if self.last_pane(&item) || !self.take_pane(&item) {
            return window.play_system_bell();
        }
        self.tabs
            .push(Tab::holding(Content::Panes(Panes::new(item))));
        let ix = self.tabs.len() - 1;
        self.move_tab_to_new_window(ix, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    #[test]
    fn zones_split_at_the_edges_and_swap_in_the_middle() {
        let b = Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.)));
        assert_eq!(zone_at(b, point(px(50.), px(50.))), Zone::Center);
        assert_eq!(
            zone_at(b, point(px(5.), px(50.))),
            Zone::Edge(Direction::Left)
        );
        assert_eq!(
            zone_at(b, point(px(95.), px(40.))),
            Zone::Edge(Direction::Right)
        );
        assert_eq!(
            zone_at(b, point(px(50.), px(10.))),
            Zone::Edge(Direction::Up)
        );
        assert_eq!(
            zone_at(b, point(px(60.), px(90.))),
            Zone::Edge(Direction::Down)
        );
    }
}
