//! Box-drawing and block-element characters rendered as pixel-snapped quads
//! so they tile seamlessly across cells regardless of the font's glyphs.

use gpui::{Bounds, Pixels, point, px, size};

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
    let (x0, y0, x1, y1) = (snap(x0, scale), snap(y0, scale), snap(x1, scale), snap(y1, scale));
    Quad { bounds: Bounds::new(point(px(x0), px(y0)), size(px(x1 - x0), px(y1 - y0))), alpha }
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
    let vert_half = if vert_half == 0. { 0. } else { weight(vert_half as u8) / 2. };
    let horiz_half = left.max(right);
    let horiz_half = if horiz_half == 0 { 0. } else { weight(horiz_half) / 2. };

    let mut out = Vec::with_capacity(4);
    if left > 0 {
        let t = weight(left);
        out.push(rect(x, cy - t / 2., cx + vert_half, cy + t / 2., scale, 1.));
    }
    if right > 0 {
        let t = weight(right);
        out.push(rect(cx - vert_half, cy - t / 2., x + w, cy + t / 2., scale, 1.));
    }
    if up > 0 {
        let t = weight(up);
        out.push(rect(cx - t / 2., y, cx + t / 2., cy + horiz_half, scale, 1.));
    }
    if down > 0 {
        let t = weight(down);
        out.push(rect(cx - t / 2., cy - horiz_half, cx + t / 2., y + h, scale, 1.));
    }
    out
}

fn block(cp: u32, cell: Bounds<Pixels>, scale: f32) -> Vec<Quad> {
    let (x, y) = (f32::from(cell.origin.x), f32::from(cell.origin.y));
    let (w, h) = (f32::from(cell.size.width), f32::from(cell.size.height));
    let r = |fx0: f32, fy0: f32, fx1: f32, fy1: f32| rect(x + w * fx0, y + h * fy0, x + w * fx1, y + h * fy1, scale, 1.);
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
}
