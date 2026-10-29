//! Box-drawing and block-element characters rendered as pixel-snapped quads
//! so they tile seamlessly across cells regardless of the font's glyphs.
//! Arrows and solid triangles next to a rule are drawn here too, on the
//! rule's centre line, so they meet it (Otty's `arrow-box-drawing-join`).

use gpui::{Bounds, Pixels, Point, point, px, size};

/// Line weights per direction: 0 none, 1 light, 2 heavy. Order: up, right, down, left.
type Arms = [u8; 4];

#[rustfmt::skip]
const LINES_2500: [Option<Arms>; 76] = {
    const fn a(u: u8, r: u8, d: u8, l: u8) -> Option<Arms> { Some([u, r, d, l]) }
    [
        a(0,1,0,1), a(0,2,0,2), a(1,0,1,0), a(2,0,2,0),             // ─ ━ │ ┃
        None, None, None, None, None, None, None, None,             // dashed: font glyphs
        a(0,1,1,0), a(0,2,1,0), a(0,1,2,0), a(0,2,2,0),             // ┌ ┍ ┎ ┏
        a(0,0,1,1), a(0,0,1,2), a(0,0,2,1), a(0,0,2,2),             // ┐ ┑ ┒ ┓
        a(1,1,0,0), a(1,2,0,0), a(2,1,0,0), a(2,2,0,0),             // └ ┕ ┖ ┗
        a(1,0,0,1), a(1,0,0,2), a(2,0,0,1), a(2,0,0,2),             // ┘ ┙ ┚ ┛
        a(1,1,1,0), a(1,2,1,0), a(2,1,1,0), a(1,1,2,0),             // ├ ┝ ┞ ┟
        a(2,1,2,0), a(2,2,1,0), a(1,2,2,0), a(2,2,2,0),             // ┠ ┡ ┢ ┣
        a(1,0,1,1), a(1,0,1,2), a(2,0,1,1), a(1,0,2,1),             // ┤ ┥ ┦ ┧
        a(2,0,2,1), a(2,0,1,2), a(1,0,2,2), a(2,0,2,2),             // ┨ ┩ ┪ ┫
        a(0,1,1,1), a(0,1,1,2), a(0,2,1,1), a(0,2,1,2),             // ┬ ┭ ┮ ┯
        a(0,1,2,1), a(0,1,2,2), a(0,2,2,1), a(0,2,2,2),             // ┰ ┱ ┲ ┳
        a(1,1,0,1), a(1,1,0,2), a(1,2,0,1), a(1,2,0,2),             // ┴ ┵ ┶ ┷
        a(2,1,0,1), a(2,1,0,2), a(2,2,0,1), a(2,2,0,2),             // ┸ ┹ ┺ ┻
        a(1,1,1,1), a(1,1,1,2), a(1,2,1,1), a(1,2,1,2),             // ┼ ┽ ┾ ┿
        a(2,1,1,1), a(1,1,2,1), a(2,1,2,1), a(2,1,1,2),             // ╀ ╁ ╂ ╃
        a(2,2,1,1), a(1,1,2,2), a(1,2,2,1), a(2,2,1,2),             // ╄ ╅ ╆ ╇
        a(1,2,2,2), a(2,1,2,2), a(2,2,2,1), a(2,2,2,2),             // ╈ ╉ ╊ ╋
    ]
};

#[rustfmt::skip]
const LINES_2574: [Arms; 12] = [
    [0,0,0,1], [1,0,0,0], [0,1,0,0], [0,0,1,0],                      // ╴ ╵ ╶ ╷
    [0,0,0,2], [2,0,0,0], [0,2,0,0], [0,0,2,0],                      // ╸ ╹ ╺ ╻
    [0,2,0,1], [1,0,2,0], [0,1,0,2], [2,0,1,0],                      // ╼ ╽ ╾ ╿
];

pub struct Quad {
    pub bounds: Bounds<Pixels>,
    pub alpha: f32,
}

/// Returns `None` if `c` should be drawn by the font.
pub fn quads(c: char, cell: Bounds<Pixels>, font_size: Pixels, scale: f32) -> Option<Vec<Quad>> {
    let cp = c as u32;
    let arms = match cp {
        0x2500..=0x254B => LINES_2500[(cp - 0x2500) as usize]?,
        0x2574..=0x257F => LINES_2574[(cp - 0x2574) as usize],
        0x2580..=0x259F => return Some(block(cp, cell, scale)),
        _ => return None,
    };
    Some(lines(arms, cell, font_size, scale))
}

fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32, scale: f32, alpha: f32) -> Quad {
    let (x0, y0, x1, y1) = (
        snap(x0, scale),
        snap(y0, scale),
        snap(x1, scale),
        snap(y1, scale),
    );
    Quad {
        bounds: Bounds::new(point(px(x0), px(y0)), size(px(x1 - x0), px(y1 - y0))),
        alpha,
    }
}

fn lines(arms: Arms, cell: Bounds<Pixels>, font_size: Pixels, scale: f32) -> Vec<Quad> {
    let (x, y) = (f32::from(cell.origin.x), f32::from(cell.origin.y));
    let (w, h) = (f32::from(cell.size.width), f32::from(cell.size.height));
    // One device pixel minimum; scales with the font like a stroke would.
    let light = (f32::from(font_size) / 13.).max(1. / scale);
    let light = snap(light, scale).max(1. / scale);
    let weight = |k: u8| if k == 2 { light * 2. } else { light };
    let cx = snap(x + w / 2., scale);
    let cy = snap(y + h / 2., scale);
    let [up, right, down, left] = arms;
    // Horizontal arms extend across the vertical stroke so joins are solid.
    let vert_half = up.max(down).min(2) as f32;
    let vert_half = if vert_half == 0. {
        0.
    } else {
        weight(vert_half as u8) / 2.
    };
    let horiz_half = left.max(right);
    let horiz_half = if horiz_half == 0 {
        0.
    } else {
        weight(horiz_half) / 2.
    };

    let mut out = Vec::with_capacity(4);
    if left > 0 {
        let t = weight(left);
        out.push(rect(x, cy - t / 2., cx + vert_half, cy + t / 2., scale, 1.));
    }
    if right > 0 {
        let t = weight(right);
        out.push(rect(
            cx - vert_half,
            cy - t / 2.,
            x + w,
            cy + t / 2.,
            scale,
            1.,
        ));
    }
    if up > 0 {
        let t = weight(up);
        out.push(rect(
            cx - t / 2.,
            y,
            cx + t / 2.,
            cy + horiz_half,
            scale,
            1.,
        ));
    }
    if down > 0 {
        let t = weight(down);
        out.push(rect(
            cx - t / 2.,
            cy - horiz_half,
            cx + t / 2.,
            y + h,
            scale,
            1.,
        ));
    }
    out
}

/// Whether `c` reaches each edge of its cell (up, right, down, left): box
/// rules of any weight, dashed, double or rounded.
pub fn reaches(c: char) -> [bool; 4] {
    let cp = c as u32;
    let on = |a: Arms| a.map(|w| w > 0);
    const H: [bool; 4] = [false, true, false, true];
    const V: [bool; 4] = [true, false, true, false];
    match cp {
        0x2504 | 0x2505 | 0x2508 | 0x2509 | 0x254C | 0x254D => H,
        0x2506 | 0x2507 | 0x250A | 0x250B | 0x254E | 0x254F => V,
        0x2500..=0x254B => LINES_2500[(cp - 0x2500) as usize].map_or([false; 4], on),
        0x2574..=0x257F => on(LINES_2574[(cp - 0x2574) as usize]),
        0x2550 => H,
        0x2551 => V,
        // Double and mixed corners, tees and crosses, then rounded corners.
        0x2552..=0x2554 | 0x256D => [false, true, true, false],
        0x2555..=0x2557 | 0x256E => [false, false, true, true],
        0x2558..=0x255A | 0x2570 => [true, true, false, false],
        0x255B..=0x255D | 0x256F => [true, false, false, true],
        0x255E..=0x2560 => [true, true, true, false],
        0x2561..=0x2563 => [true, false, true, true],
        0x2564..=0x2566 => [false, true, true, true],
        0x2567..=0x2569 => [true, true, false, true],
        0x256A..=0x256C => [true; 4],
        _ => [false; 4],
    }
}

/// A filled convex polygon.
pub struct Polygon {
    pub points: Vec<Point<Pixels>>,
}

/// The way an arrow points (up, right, down, left, as in [`Arms`]) and
/// whether it is a solid triangle.
fn pointing(c: char) -> Option<(usize, bool)> {
    Some(match c {
        '\u{2191}' => (0, false),
        '\u{2192}' => (1, false),
        '\u{2193}' => (2, false),
        '\u{2190}' => (3, false),
        '\u{25B2}' => (0, true),
        '\u{25B6}' => (1, true),
        '\u{25BC}' => (2, true),
        '\u{25C0}' => (3, true),
        _ => return None,
    })
}

/// Whether `c` is one of the arrows or triangles [`arrow`] can draw.
pub fn is_arrow(c: char) -> bool {
    pointing(c).is_some()
}

/// An arrow (← → ↑ ↓) or solid triangle (◀ ▶ ▲ ▼) drawn on the cell's
/// centre line because a rule meets it from behind or ahead: arrows get a
/// rule-weight stem and chevron, triangles a flat back on the edge the
/// rule comes from. `None` (left to the font) when no rule touches it.
/// `around` holds the characters up, right, down and left of the cell.
pub fn arrow(
    c: char,
    cell: Bounds<Pixels>,
    font_size: Pixels,
    scale: f32,
    around: [char; 4],
) -> Option<(Vec<Quad>, Vec<Polygon>)> {
    let (dir, solid) = pointing(c)?;
    let back = (dir + 2) % 4;
    let from_behind = reaches(around[back])[dir];
    let ahead = reaches(around[dir])[back];
    if !from_behind && !ahead {
        return None;
    }
    let (x0, y0) = (f32::from(cell.origin.x), f32::from(cell.origin.y));
    let (w, h) = (f32::from(cell.size.width), f32::from(cell.size.height));
    let cx = snap(x0 + w / 2., scale);
    let cy = snap(y0 + h / 2., scale);
    let vertical = dir % 2 == 0;
    // Length along the arrow, and room across it.
    let (len, across) = if vertical { (h, w) } else { (w, h) };
    // `a` runs from the back edge to the front one; `o` is across the line.
    let at = |a: f32, o: f32| -> (f32, f32) {
        match dir {
            0 => (cx + o, y0 + h - a),
            1 => (x0 + a, cy + o),
            2 => (cx + o, y0 + a),
            _ => (x0 + w - a, cy + o),
        }
    };
    let pt = |a: f32, o: f32| {
        let (x, y) = at(a, o);
        point(px(x), px(y))
    };
    let light = snap((f32::from(font_size) / 13.).max(1. / scale), scale).max(1. / scale);
    if solid {
        let half = if vertical {
            across / 2.
        } else {
            (len * 0.55).min(across / 2.)
        };
        // As deep as it is wide, flush on the side a rule comes from.
        let depth = (half * 2.).min(len);
        let (start, end) = if from_behind {
            (0., depth)
        } else {
            (len - depth, len)
        };
        let points = vec![pt(start, -half), pt(end, 0.), pt(start, half)];
        return Some((Vec::new(), vec![Polygon { points }]));
    }
    let head = (len * 0.45).min(across * 0.42);
    let tip = len - light * 0.6;
    let start = if from_behind { 0. } else { len * 0.12 };
    let (ax, ay) = at(start, -light / 2.);
    let (bx, by) = at(tip - light, light / 2.);
    let stem = rect(ax.min(bx), ay.min(by), ax.max(bx), ay.max(by), scale, 1.);
    // Each chevron arm is a stroke of the rule's weight from the tip back.
    let arm = |side: f32| {
        let (back_a, back_o) = (tip - head, side * head);
        let (da, d_o) = (back_a - tip, back_o);
        let norm = (da * da + d_o * d_o).sqrt().max(f32::EPSILON);
        let (na, no) = (-d_o / norm * light / 2., da / norm * light / 2.);
        Polygon {
            points: vec![
                pt(tip + na, no),
                pt(back_a + na, back_o + no),
                pt(back_a - na, back_o - no),
                pt(tip - na, -no),
            ],
        }
    };
    Some((vec![stem], vec![arm(-1.), arm(1.)]))
}

fn block(cp: u32, cell: Bounds<Pixels>, scale: f32) -> Vec<Quad> {
    let (x, y) = (f32::from(cell.origin.x), f32::from(cell.origin.y));
    let (w, h) = (f32::from(cell.size.width), f32::from(cell.size.height));
    let r = |fx0: f32, fy0: f32, fx1: f32, fy1: f32| {
        rect(
            x + w * fx0,
            y + h * fy0,
            x + w * fx1,
            y + h * fy1,
            scale,
            1.,
        )
    };
    let shade = |alpha: f32| vec![rect(x, y, x + w, y + h, scale, alpha)];
    match cp {
        0x2580 => vec![r(0., 0., 1., 0.5)],
        0x2581..=0x2587 => {
            let eighths = (cp - 0x2580) as f32 / 8.;
            vec![r(0., 1. - eighths, 1., 1.)]
        }
        0x2588 => vec![r(0., 0., 1., 1.)],
        0x2589..=0x258F => {
            let eighths = (0x2590 - cp) as f32 / 8.;
            vec![r(0., 0., eighths, 1.)]
        }
        0x2590 => vec![r(0.5, 0., 1., 1.)],
        0x2591 => shade(0.25),
        0x2592 => shade(0.5),
        0x2593 => shade(0.75),
        0x2594 => vec![r(0., 0., 1., 0.125)],
        0x2595 => vec![r(0.875, 0., 1., 1.)],
        _ => {
            // Quadrants: bit 0 UL, 1 UR, 2 LL, 3 LR.
            let bits = match cp {
                0x2596 => 0b0100,
                0x2597 => 0b1000,
                0x2598 => 0b0001,
                0x2599 => 0b1101,
                0x259A => 0b1001,
                0x259B => 0b0111,
                0x259C => 0b1011,
                0x259D => 0b0010,
                0x259E => 0b0110,
                0x259F => 0b1110,
                _ => 0,
            };
            let mut out = Vec::new();
            if bits & 1 != 0 {
                out.push(r(0., 0., 0.5, 0.5));
            }
            if bits & 2 != 0 {
                out.push(r(0.5, 0., 1., 0.5));
            }
            if bits & 4 != 0 {
                out.push(r(0., 0.5, 0.5, 1.));
            }
            if bits & 8 != 0 {
                out.push(r(0.5, 0.5, 1., 1.));
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(0.)), size(px(8.), px(16.)))
    }

    #[test]
    fn table_spot_checks() {
        assert_eq!(LINES_2500[0x0C], Some([0, 1, 1, 0])); // ┌
        assert_eq!(LINES_2500[0x3C], Some([1, 1, 1, 1])); // ┼
        assert_eq!(LINES_2500[0x4B], Some([2, 2, 2, 2])); // ╋
        assert_eq!(LINES_2500[0x34], Some([1, 1, 0, 1])); // ┴
    }

    #[test]
    fn horizontal_line_spans_cell() {
        let q = quads('─', cell(), px(13.), 2.).unwrap();
        let total: f32 = q.iter().map(|q| f32::from(q.bounds.size.width)).sum();
        assert!(total >= 8.);
        assert!(quads('a', cell(), px(13.), 2.).is_none());
        assert!(quads('┄', cell(), px(13.), 2.).is_none());
    }

    #[test]
    fn full_block_fills_cell() {
        let q = quads('█', cell(), px(13.), 2.).unwrap();
        assert_eq!(q[0].bounds, cell());
    }

    #[test]
    fn rules_reach_their_edges() {
        assert_eq!(reaches('\u{2500}'), [false, true, false, true]);
        assert_eq!(reaches('\u{2503}'), [true, false, true, false]);
        assert_eq!(reaches('\u{2504}'), [false, true, false, true]);
        assert_eq!(reaches('\u{2550}'), [false, true, false, true]);
        assert_eq!(reaches('\u{256D}'), [false, true, true, false]);
        assert_eq!(reaches('\u{256C}'), [true; 4]);
        assert_eq!(reaches('\u{2574}'), [false, false, false, true]);
        assert_eq!(reaches('a'), [false; 4]);
    }

    #[test]
    fn arrows_join_only_next_to_a_rule() {
        let (h, v) = ('\u{2500}', '\u{2502}');
        let none = [' '; 4];
        assert!(arrow('\u{2192}', cell(), px(13.), 2., none).is_none());
        assert!(arrow('x', cell(), px(13.), 2., [h; 4]).is_none());
        // A rule behind (left of a right arrow): the stem starts at the edge.
        let (quads, arms) = arrow('\u{2192}', cell(), px(13.), 2., [' ', ' ', ' ', h]).unwrap();
        assert_eq!(quads[0].bounds.origin.x, px(0.));
        assert_eq!(arms.len(), 2);
        // Centred like the rule: the stem straddles the middle row.
        let mid = f32::from(quads[0].bounds.center().y);
        assert!((mid - 8.).abs() <= 0.5, "{mid}");
        // A vertical rule above doesn't join a right arrow, but a down one.
        assert!(arrow('\u{2192}', cell(), px(13.), 2., [v, ' ', ' ', ' ']).is_none());
        assert!(arrow('\u{2193}', cell(), px(13.), 2., [v, ' ', ' ', ' ']).is_some());
    }

    #[test]
    fn triangles_sit_flush_on_the_rule() {
        let h = '\u{2500}';
        let (_, tri) = arrow('\u{25B6}', cell(), px(13.), 2., [' ', ' ', ' ', h]).unwrap();
        let xs: Vec<f32> = tri[0].points.iter().map(|p| f32::from(p.x)).collect();
        assert_eq!(xs[0], 0.);
        assert_eq!(xs[1], 8.);
        // Pointing at a rule ahead: the tip touches the far edge.
        let up = arrow('\u{25B2}', cell(), px(13.), 2., ['\u{2502}', ' ', ' ', ' ']);
        assert_eq!(f32::from(up.unwrap().1[0].points[1].y), 0.);
    }
}
