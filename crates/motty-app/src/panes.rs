//! A tab's split panes: a binary tree of views (terminals, files, folders,
//! web pages), laid out by ratios.
//!
//! Like Otty, panes split their space exactly in half (each keeps the full
//! container padding), and the divider is a hairline drawn over the seam.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Axis, Bounds, Entity, EntityId, FocusHandle, Focusable, Hsla, MouseButton,
    MouseMoveEvent, Pixels, Window, canvas, div, prelude::*, px, relative,
};

use crate::editor::FileEditor;
use crate::terminal_view::TerminalView;
use crate::web_pane::WebPane;

/// Panes never shrink below this along the split axis while dragging.
const MIN_PANE: f32 = 48.;
/// The divider's grab area, centered on the hairline.
const GRAB: f32 = 6.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    /// The half of `rect` a new pane split off in this direction gets.
    pub fn half(self, [x, y, w, h]: [f32; 4]) -> [f32; 4] {
        match self {
            Direction::Left => [x, y, w / 2., h],
            Direction::Right => [x + w / 2., y, w / 2., h],
            Direction::Up => [x, y, w, h / 2.],
            Direction::Down => [x, y + h / 2., w, h / 2.],
        }
    }

    fn axis(self) -> Axis {
        match self {
            Direction::Left | Direction::Right => Axis::Horizontal,
            Direction::Up | Direction::Down => Axis::Vertical,
        }
    }

    /// Whether the new pane goes after (right of, below) the old one.
    fn after(self) -> bool {
        matches!(self, Direction::Right | Direction::Down)
    }
}

/// What a pane shows.
#[derive(Clone, PartialEq)]
pub enum PaneItem {
    Terminal(Entity<TerminalView>),
    File(Entity<FileEditor>),
    Web(Entity<WebPane>),
}

impl PaneItem {
    pub fn terminal(&self) -> Option<&Entity<TerminalView>> {
        match self {
            PaneItem::Terminal(view) => Some(view),
            _ => None,
        }
    }

    pub fn file(&self) -> Option<&Entity<FileEditor>> {
        match self {
            PaneItem::File(editor) => Some(editor),
            _ => None,
        }
    }

    pub fn entity_id(&self) -> EntityId {
        match self {
            PaneItem::Terminal(view) => view.entity_id(),
            PaneItem::File(editor) => editor.entity_id(),
            PaneItem::Web(web) => web.entity_id(),
        }
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            PaneItem::Terminal(view) => view.focus_handle(cx),
            PaneItem::File(editor) => editor.focus_handle(cx),
            PaneItem::Web(web) => web.focus_handle(cx),
        }
    }

    pub fn element(&self) -> AnyElement {
        match self {
            PaneItem::Terminal(view) => view.clone().into_any_element(),
            PaneItem::File(editor) => editor.clone().into_any_element(),
            PaneItem::Web(web) => web.clone().into_any_element(),
        }
    }

    /// What the pane shows, for tab titles and pane lists.
    pub fn title(&self, cx: &App) -> String {
        match self {
            PaneItem::Terminal(view) => view.read(cx).display_title(),
            PaneItem::File(editor) => editor.read(cx).title(),
            PaneItem::Web(web) => web.read(cx).title(),
        }
    }

    /// Where a terminal opened beside it should start.
    pub fn folder(&self, cx: &App) -> Option<std::path::PathBuf> {
        match self {
            PaneItem::Terminal(view) => view.read(cx).cwd.clone(),
            PaneItem::File(editor) => editor.read(cx).folder(),
            PaneItem::Web(_) => None,
        }
    }
}

impl From<Entity<TerminalView>> for PaneItem {
    fn from(view: Entity<TerminalView>) -> Self {
        PaneItem::Terminal(view)
    }
}

impl From<Entity<WebPane>> for PaneItem {
    fn from(web: Entity<WebPane>) -> Self {
        PaneItem::Web(web)
    }
}

impl From<Entity<FileEditor>> for PaneItem {
    fn from(editor: Entity<FileEditor>) -> Self {
        PaneItem::File(editor)
    }
}

impl PartialEq<Entity<TerminalView>> for PaneItem {
    fn eq(&self, view: &Entity<TerminalView>) -> bool {
        self.terminal() == Some(view)
    }
}

/// Leaves are pane items; tests use plain values.
pub enum Pane<T = PaneItem> {
    Leaf(T),
    Split(Split<T>),
}

pub struct Split<T = PaneItem> {
    pub id: usize,
    pub axis: Axis,
    /// The first child's share of the space.
    pub ratio: f32,
    pub first: Box<Pane<T>>,
    pub second: Box<Pane<T>>,
    /// Where the split was last laid out, for dragging its divider.
    pub bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

type DragStart = dyn Fn(usize, &mut gpui::Window, &mut gpui::App);

/// How a pane is drawn: its divider and whether a leaf is dimmed.
pub struct PaneStyle<'a> {
    pub divider: Hsla,
    pub divider_width: f32,
    pub dimmed: &'a dyn Fn(&PaneItem) -> bool,
    pub dim_opacity: f32,
    /// Called with a split's id when its divider is pressed.
    pub on_drag_start: Rc<DragStart>,
    /// `focus-follows-mouse`: called with the pane under a moving pointer.
    pub on_hover: Option<Rc<PaneHover>>,
}

pub type PaneHover = dyn Fn(&PaneItem, &MouseMoveEvent, &mut Window, &mut App);

impl<T: Clone + PartialEq> Pane<T> {
    pub fn leaves(&self) -> Vec<&T> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a T>) {
        match self {
            Pane::Leaf(view) => out.push(view),
            Pane::Split(s) => {
                s.first.collect(out);
                s.second.collect(out);
            }
        }
    }

    pub fn contains(&self, view: &T) -> bool {
        self.leaves().into_iter().any(|v| v == view)
    }

    /// Splits `target` in half, putting `new` on the `direction` side.
    pub fn split(&mut self, target: &T, new: T, direction: Direction, id: usize) -> bool {
        match self {
            Pane::Leaf(view) if view == target => {
                let old = std::mem::replace(self, Pane::Leaf(new.clone()));
                let new = Pane::Leaf(new);
                let (first, second) = if direction.after() {
                    (old, new)
                } else {
                    (new, old)
                };
                *self = Pane::Split(Split {
                    id,
                    axis: direction.axis(),
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                    bounds: Rc::default(),
                });
                true
            }
            Pane::Leaf(_) => false,
            Pane::Split(s) => {
                s.first.split(target, new.clone(), direction, id)
                    || s.second.split(target, new, direction, id)
            }
        }
    }

    /// The tree without `target`; its sibling takes the split's place.
    pub fn without(self, target: &T) -> Option<Self> {
        match self {
            Pane::Leaf(view) => (view != *target).then_some(Pane::Leaf(view)),
            Pane::Split(s) => match (s.first.without(target), s.second.without(target)) {
                (Some(first), Some(second)) => Some(Pane::Split(Split {
                    first: Box::new(first),
                    second: Box::new(second),
                    ..s
                })),
                (first, second) => first.or(second),
            },
        }
    }

    pub fn split_mut(&mut self, id: usize) -> Option<&mut Split<T>> {
        let Pane::Split(s) = self else { return None };
        if s.id == id {
            Some(s)
        } else if s.first.has_split(id) {
            s.first.split_mut(id)
        } else {
            s.second.split_mut(id)
        }
    }

    fn has_split(&self, id: usize) -> bool {
        match self {
            Pane::Leaf(_) => false,
            Pane::Split(s) => s.id == id || s.first.has_split(id) || s.second.has_split(id),
        }
    }

    /// The innermost split along `axis` that holds `leaf`.
    pub fn enclosing_split(&mut self, leaf: &T, axis: Axis) -> Option<&mut Split<T>> {
        let Pane::Split(s) = self else { return None };
        let in_first = s.first.contains(leaf);
        if !in_first && !s.second.contains(leaf) {
            return None;
        }
        let inner = if in_first { &s.first } else { &s.second };
        if inner.has_split_along(leaf, axis) {
            let inner = if in_first {
                &mut s.first
            } else {
                &mut s.second
            };
            return inner.enclosing_split(leaf, axis);
        }
        (s.axis == axis).then_some(s)
    }

    fn has_split_along(&self, leaf: &T, axis: Axis) -> bool {
        match self {
            Pane::Leaf(_) => false,
            Pane::Split(s) => {
                (s.axis == axis && self.contains(leaf))
                    || s.first.has_split_along(leaf, axis)
                    || s.second.has_split_along(leaf, axis)
            }
        }
    }

    /// Gives every pane in a row (or column) of same-axis splits equal space.
    pub fn equalize(&mut self) {
        if let Pane::Split(s) = self {
            s.first.equalize();
            s.second.equalize();
            let (a, b) = (s.first.span(s.axis), s.second.span(s.axis));
            s.ratio = a as f32 / (a + b) as f32;
        }
    }

    /// How many panes this subtree lines up along `axis`.
    fn span(&self, axis: Axis) -> usize {
        match self {
            Pane::Split(s) if s.axis == axis => s.first.span(axis) + s.second.span(axis),
            _ => 1,
        }
    }

    /// Each leaf's rectangle (x, y, w, h) within the unit square.
    fn rects(&self, rect: [f32; 4], out: &mut Vec<(T, [f32; 4])>) {
        match self {
            Pane::Leaf(view) => out.push((view.clone(), rect)),
            Pane::Split(s) => {
                let [x, y, w, h] = rect;
                let (a, b) = match s.axis {
                    Axis::Horizontal => (
                        [x, y, w * s.ratio, h],
                        [x + w * s.ratio, y, w * (1. - s.ratio), h],
                    ),
                    Axis::Vertical => (
                        [x, y, w, h * s.ratio],
                        [x, y + h * s.ratio, w, h * (1. - s.ratio)],
                    ),
                };
                s.first.rects(a, out);
                s.second.rects(b, out);
            }
        }
    }

    /// Where `leaf` sits, as (x, y, w, h) within the unit square.
    pub fn rect_of(&self, leaf: &T) -> Option<[f32; 4]> {
        let mut rects = Vec::new();
        self.rects([0., 0., 1., 1.], &mut rects);
        rects.into_iter().find(|(v, _)| v == leaf).map(|(_, r)| r)
    }

    /// The pane next to `from` in `direction`: of those across that edge
    /// and overlapping it, the one nearest its center.
    pub fn neighbor(&self, from: &T, direction: Direction) -> Option<T> {
        let mut rects = Vec::new();
        self.rects([0., 0., 1., 1.], &mut rects);
        let [fx, fy, fw, fh] = rects.iter().find(|(v, _)| v == from)?.1;
        let (cx, cy) = (fx + fw / 2., fy + fh / 2.);
        const EPS: f32 = 1e-4;
        rects
            .into_iter()
            .filter(|(v, _)| v != from)
            .filter(|(_, [x, y, w, h])| match direction {
                Direction::Left => (x + w - fx).abs() < EPS && *y < fy + fh && y + h > fy,
                Direction::Right => (x - (fx + fw)).abs() < EPS && *y < fy + fh && y + h > fy,
                Direction::Up => (y + h - fy).abs() < EPS && *x < fx + fw && x + w > fx,
                Direction::Down => (y - (fy + fh)).abs() < EPS && *x < fx + fw && x + w > fx,
            })
            .min_by(|(_, a), (_, b)| {
                let d = |[x, y, w, h]: [f32; 4]| match direction.axis() {
                    Axis::Horizontal => (y + h / 2. - cy).abs(),
                    Axis::Vertical => (x + w / 2. - cx).abs(),
                };
                d(*a).total_cmp(&d(*b))
            })
            .map(|(v, _)| v)
    }
}

impl Pane {
    pub fn render(&self, style: &PaneStyle) -> AnyElement {
        match self {
            Pane::Leaf(view) => div()
                .size_full()
                .overflow_hidden()
                .when((style.dimmed)(view), |d| d.opacity(style.dim_opacity))
                .when_some(style.on_hover.clone(), |d, hover| {
                    let view = view.clone();
                    d.on_mouse_move(move |event, window, cx| hover(&view, event, window, cx))
                })
                .child(view.element())
                .into_any_element(),
            Pane::Split(s) => {
                let horizontal = s.axis == Axis::Horizontal;
                let bounds = s.bounds.clone();
                let (id, on_drag_start) = (s.id, style.on_drag_start.clone());
                let width = style.divider_width;
                let hairline = div().absolute().bg(style.divider);
                let hairline = if horizontal {
                    hairline
                        .top_0()
                        .bottom_0()
                        .left(px(-width / 2.))
                        .w(px(width))
                } else {
                    hairline
                        .left_0()
                        .right_0()
                        .top(px(-width / 2.))
                        .h(px(width))
                };
                let grab = div()
                    .id(("divider", s.id))
                    .absolute()
                    .occlude()
                    .cursor(if horizontal {
                        gpui::CursorStyle::ResizeLeftRight
                    } else {
                        gpui::CursorStyle::ResizeUpDown
                    })
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        cx.stop_propagation();
                        on_drag_start(id, window, cx);
                    });
                let grab = if horizontal {
                    grab.top_0().bottom_0().left(px(-GRAB / 2.)).w(px(GRAB))
                } else {
                    grab.left_0().right_0().top(px(-GRAB / 2.)).h(px(GRAB))
                };
                let first = div()
                    .relative()
                    .flex_none()
                    .overflow_hidden()
                    .child(s.first.render(style));
                let first = if horizontal {
                    first.h_full().w(relative(s.ratio))
                } else {
                    first.w_full().h(relative(s.ratio))
                };
                let second = div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(s.second.render(style))
                    .child(hairline)
                    .child(grab);
                let second = if horizontal {
                    second.h_full()
                } else {
                    second.w_full()
                };
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .when(!horizontal, |d| d.flex_col())
                    .child(first)
                    .child(second)
                    .child(
                        canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .into_any_element()
            }
        }
    }
}

impl<T> Split<T> {
    /// Moves the divider by `delta` points (right or down when positive).
    pub fn nudge(&mut self, delta: f32) {
        let Some(b) = self.bounds.get() else { return };
        let len = f32::from(match self.axis {
            Axis::Horizontal => b.size.width,
            Axis::Vertical => b.size.height,
        });
        if len > 2. * MIN_PANE {
            self.ratio = (self.ratio * len + delta).clamp(MIN_PANE, len - MIN_PANE) / len;
        }
    }

    /// Moves the divider to follow the pointer, keeping both sides usable.
    pub fn drag_to(&mut self, position: gpui::Point<Pixels>) {
        let Some(b) = self.bounds.get() else { return };
        let (start, len, at) = match self.axis {
            Axis::Horizontal => (b.origin.x, b.size.width, position.x),
            Axis::Vertical => (b.origin.y, b.size.height, position.y),
        };
        let (len, at) = (f32::from(len), f32::from(at - start));
        if len > 2. * MIN_PANE {
            self.ratio = at.clamp(MIN_PANE, len - MIN_PANE) / len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Pane<u32> {
        // 1 | 2
        //   | -
        //   | 3
        let mut root = Pane::Leaf(1);
        assert!(root.split(&1, 2, Direction::Right, 0));
        assert!(root.split(&2, 3, Direction::Down, 1));
        root
    }

    #[test]
    fn splits_in_reading_order() {
        assert_eq!(tree().leaves(), [&1, &2, &3]);
        let mut root = Pane::Leaf(1);
        root.split(&1, 2, Direction::Left, 0);
        assert_eq!(root.leaves(), [&2, &1]);
    }

    #[test]
    fn neighbors_follow_geometry() {
        let root = tree();
        assert_eq!(root.neighbor(&1, Direction::Right), Some(2));
        assert_eq!(root.neighbor(&3, Direction::Left), Some(1));
        assert_eq!(root.neighbor(&2, Direction::Down), Some(3));
        assert_eq!(root.neighbor(&3, Direction::Up), Some(2));
        assert_eq!(root.neighbor(&1, Direction::Left), None);
    }

    #[test]
    fn removing_a_pane_promotes_its_sibling() {
        let root = tree().without(&2).unwrap();
        assert_eq!(root.leaves(), [&1, &3]);
        assert_eq!(root.neighbor(&1, Direction::Right), Some(3));
        assert!(Pane::Leaf(1).without(&1).is_none());
    }

    #[test]
    fn enclosing_split_is_the_innermost_on_that_axis() {
        let mut root = tree();
        assert_eq!(
            root.enclosing_split(&3, Axis::Vertical).map(|s| s.id),
            Some(1)
        );
        assert_eq!(
            root.enclosing_split(&3, Axis::Horizontal).map(|s| s.id),
            Some(0)
        );
        assert!(root.enclosing_split(&1, Axis::Vertical).is_none());
    }

    #[test]
    fn equalize_shares_a_row_evenly() {
        let mut root = Pane::Leaf(1);
        root.split(&1, 2, Direction::Right, 0);
        root.split(&2, 3, Direction::Right, 1);
        root.equalize();
        let Pane::Split(outer) = &root else { panic!() };
        assert!((outer.ratio - 1. / 3.).abs() < 1e-6);
        assert!(
            root.split_mut(1)
                .is_some_and(|s| (s.ratio - 0.5).abs() < 1e-6)
        );
    }
}
