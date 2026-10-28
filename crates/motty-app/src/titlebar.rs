//! The titlebar buttons Otty shows while the pointer is over the window: a
//! new-tab split button (with a menu chevron), notifications and the tabs
//! panel toggle next to the traffic lights or at the sidebar's edge, the
//! title's "more" ellipsis, and the details panel toggle on the right.
//!
//! Geometry is measured from Otty at 2x. Icons are drawn as vector shapes in
//! the style of the system's symbols; none of Otty's assets are used.

use gpui::{App, Bounds, Hsla, IntoElement, PathBuilder, Pixels, Point, Styled, Window, canvas, div, point, prelude::*, px};

/// Buttons are square boxes this tall, 4pt below the window's top.
pub const BUTTON: f32 = 24.;
pub const TOP: f32 = 4.;
pub const GAP: f32 = 4.;
/// The new-tab split button: a plus box and a chevron box.
pub const SPLIT: f32 = 38.;
/// Split button, notifications and the tabs panel toggle.
pub const GROUP: f32 = SPLIT + GAP + BUTTON + GAP + BUTTON;
/// With the sidebar hidden the group starts here, after the traffic lights.
pub const COLLAPSED_LEFT: f32 = 82.;
/// The details toggle's box ends this far from the window's right edge.
pub const DETAILS_RIGHT: f32 = 14.;
/// The ellipsis starts this far after the title.
pub const ELLIPSIS_GAP: f32 = 6.5;
pub const RADIUS: f32 = 6.;
/// The title's highlight reaches this far left of the title, and this far
/// right of its end (covering the ellipsis and 8pt after it).
pub const TITLE_PAD_LEFT: f32 = 8.;
pub const TITLE_PAD_RIGHT: f32 = ELLIPSIS_GAP + ELLIPSIS_WIDTH + 7.5;
const ELLIPSIS_WIDTH: f32 = 3. * 1.75 + 2. * 2.5;

const STROKE: f32 = 1.2;

/// A canvas the size of `w`×`h` that paints with its top-left as origin.
fn icon(w: f32, h: f32, paint: impl FnOnce(Point<Pixels>, &mut Window) + 'static) -> impl IntoElement {
    canvas(|_, _, _| {}, move |bounds: Bounds<Pixels>, _, window: &mut Window, _: &mut App| paint(bounds.origin, window))
        .w(px(w))
        .h(px(h))
}

fn at(o: Point<Pixels>, x: f32, y: f32) -> Point<Pixels> {
    point(o.x + px(x), o.y + px(y))
}

fn stroke(
    o: Point<Pixels>,
    color: Hsla,
    window: &mut Window,
    width: f32,
    draw: impl FnOnce(&mut PathBuilder, &dyn Fn(f32, f32) -> Point<Pixels>),
) {
    let mut path = PathBuilder::stroke(px(width));
    draw(&mut path, &|x, y| at(o, x, y));
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// 10.5pt arms centered at (11.75, 12.25) in its 24pt box, as one shape so
/// the translucent color doesn't double where the arms cross.
pub fn plus(color: Hsla) -> impl IntoElement {
    icon(BUTTON, BUTTON, move |o, window| {
        let (cx, cy, arm, h) = (11.75, 12.25, 5.25, STROKE / 2.);
        let mut path = PathBuilder::fill();
        path.add_polygon(
            &[
                (cx - h, cy - arm),
                (cx + h, cy - arm),
                (cx + h, cy - h),
                (cx + arm, cy - h),
                (cx + arm, cy + h),
                (cx + h, cy + h),
                (cx + h, cy + arm),
                (cx - h, cy + arm),
                (cx - h, cy + h),
                (cx - arm, cy + h),
                (cx - arm, cy - h),
                (cx - h, cy - h),
            ]
            .map(|(x, y)| at(o, x, y)),
            true,
        );
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    })
}

/// A 7pt-wide chevron in the split button's 14pt-wide right part.
pub fn chevron(color: Hsla) -> impl IntoElement {
    icon(SPLIT - BUTTON, BUTTON, move |o, window| {
        stroke(o, color, window, 1., |p, at| {
            p.move_to(at(3.5, 10.75));
            p.line_to(at(7., 13.5));
            p.line_to(at(10.5, 10.75));
        });
    })
}

/// A 12pt-wide bell with a round dome and a cup-shaped clapper.
pub fn bell(color: Hsla) -> impl IntoElement {
    icon(BUTTON, BUTTON, move |o, window| {
        stroke(o, color, window, 1.1, |p, at| {
            p.move_to(at(6.4, 16.2));
            p.line_to(at(17.6, 16.2));
            p.line_to(at(16.2, 14.1));
            p.line_to(at(16.2, 10.4));
            p.cubic_bezier_to(at(12., 5.7), at(16.2, 7.6), at(14.4, 5.7));
            p.cubic_bezier_to(at(7.8, 10.4), at(9.6, 5.7), at(7.8, 7.6));
            p.line_to(at(7.8, 14.1));
            p.close();
        });
        stroke(o, color, window, 1.1, |p, at| {
            p.move_to(at(10.2, 16.8));
            p.cubic_bezier_to(at(13.8, 16.8), at(10.3, 19.3), at(13.7, 19.3));
        });
    })
}

/// A 12pt panel outline with a 2pt bar inside, on the left for the tabs
/// panel and on the right (lighter) for the details panel.
pub fn panel(color: Hsla, bar: Hsla, right: bool) -> impl IntoElement {
    let bar_x = if right { 13.25 } else { 7.75 };
    div()
        .relative()
        .size(px(BUTTON))
        .child(div().absolute().left(px(6.)).top(px(6.)).size(px(12.)).rounded(px(3.)).border(px(1.2)).border_color(color))
        .child(div().absolute().left(px(bar_x)).top(px(8.5)).w(px(2.75)).h(px(7.5)).rounded(px(0.75)).bg(bar))
}

/// Three dots, 1.75pt across on a 4.25pt pitch.
pub fn ellipsis(color: Hsla) -> impl IntoElement {
    let dot = || div().flex_none().size(px(1.75)).rounded_full().bg(color);
    div().flex().items_center().gap(px(2.5)).child(dot()).child(dot()).child(dot())
}
