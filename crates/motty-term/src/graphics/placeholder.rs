//! Kitty's Unicode placeholders: `U+10EEEE` cells that show part of an
//! image, the image id in the foreground colour, the placement id in the
//! underline colour, and row / column / id's top byte as diacritics.

use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::vte::ansi::Color;

/// The placeholder character.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// The diacritics that number rows and columns: index = value
/// (kitty's `rowcolumn-diacritics.txt`).
#[rustfmt::skip]
const DIACRITICS: [u32; 297] = [
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A,
    0x034B, 0x034C, 0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365,
    0x0366, 0x0367, 0x0368, 0x0369, 0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F,
    0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592, 0x0593, 0x0594, 0x0595, 0x0597,
    0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1, 0x05A8, 0x05A9,
    0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615,
    0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6,
    0x06D7, 0x06D8, 0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2,
    0x06E4, 0x06E7, 0x06E8, 0x06EB, 0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736,
    0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743, 0x0745, 0x0747, 0x0749, 0x074A,
    0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3, 0x0816, 0x0817,
    0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951,
    0x0953, 0x0954, 0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD,
    0x193A, 0x1A17, 0x1A75, 0x1A76, 0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C,
    0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71, 0x1B72, 0x1B73, 0x1CD0, 0x1CD1,
    0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4, 0x1DC5, 0x1DC6,
    0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
    0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF,
    0x1DE0, 0x1DE1, 0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1,
    0x20D4, 0x20D5, 0x20D6, 0x20D7, 0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0,
    0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2, 0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6,
    0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE, 0x2DEF, 0x2DF0,
    0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1,
    0xA8E0, 0xA8E1, 0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9,
    0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED, 0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2,
    0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1, 0xFE20, 0xFE21, 0xFE22, 0xFE23,
    0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186, 0x1D187, 0x1D188, 0x1D189,
    0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
];

/// The number a row/column diacritic stands for.
pub fn diacritic(c: char) -> Option<u32> {
    DIACRITICS.binary_search(&(c as u32)).ok().map(|i| i as u32)
}

/// A placeholder cell as stored: what the inheritance rules compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raw {
    /// The low 24 (true colour) or 8 (256 colours) bits of the image id.
    pub fg: u32,
    /// The placement id, 0 for any.
    pub underline: u32,
    pub row: Option<u32>,
    pub col: Option<u32>,
    pub msb: Option<u32>,
}

impl Raw {
    /// Reads a grid cell, if it's a placeholder.
    pub fn from_cell(cell: &Cell) -> Option<Raw> {
        if cell.c != PLACEHOLDER {
            return None;
        }
        let mut marks = cell.zerowidth().unwrap_or_default().iter();
        let mut next = || marks.next().and_then(|&c| diacritic(c));
        let (row, col, msb) = (next(), next(), next());
        Some(Raw {
            fg: color_id(cell.fg),
            underline: cell.underline_color().map_or(0, color_id),
            row,
            col,
            msb,
        })
    }
}

/// The number a colour carries: the RGB value or the palette index.
fn color_id(color: Color) -> u32 {
    match color {
        Color::Spec(rgb) => u32::from_be_bytes([0, rgb.r, rgb.g, rgb.b]),
        Color::Indexed(i) => i.into(),
        Color::Named(_) => 0,
    }
}

/// Which part of which image a placeholder cell shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub image_id: u32,
    pub placement_id: u32,
    pub row: u32,
    pub col: u32,
}

/// Resolves a screen row's placeholder cells (column, cell) left to right,
/// filling omitted diacritics from the cell to the left as kitty's rules say.
pub fn resolve_row(cells: &[(usize, Raw)]) -> Vec<(usize, Resolved)> {
    let mut out: Vec<(usize, Resolved)> = Vec::with_capacity(cells.len());
    let mut prev: Option<(usize, Raw, Resolved)> = None;
    for &(x, raw) in cells {
        // The cell just left, if it's a placeholder for the same image.
        let left = prev
            .filter(|(px, p, _)| px + 1 == x && p.fg == raw.fg && p.underline == raw.underline)
            .map(|(_, _, r)| (r, r.image_id >> 24));
        let (row, col, msb) = match (raw.row, raw.col, raw.msb, left) {
            (None, _, _, Some((l, msb))) => (l.row, l.col + 1, msb),
            (Some(row), None, _, Some((l, msb))) if l.row == row => (row, l.col + 1, msb),
            (Some(row), Some(col), None, Some((l, msb))) if l.row == row && l.col + 1 == col => {
                (row, col, msb)
            }
            (row, col, msb, _) => (row.unwrap_or(0), col.unwrap_or(0), msb.unwrap_or(0)),
        };
        let resolved = Resolved {
            image_id: (msb << 24) | raw.fg,
            placement_id: raw.underline,
            row,
            col,
        };
        out.push((x, resolved));
        prev = Some((x, raw, resolved));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(n: usize) -> char {
        char::from_u32(DIACRITICS[n]).unwrap()
    }

    fn raw(fg: u32, row: Option<u32>, col: Option<u32>, msb: Option<u32>) -> Raw {
        Raw {
            fg,
            underline: 0,
            row,
            col,
            msb,
        }
    }

    #[test]
    fn table_is_sorted_for_lookup() {
        assert!(DIACRITICS.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(diacritic('\u{305}'), Some(0));
        assert_eq!(diacritic('\u{30D}'), Some(1));
        assert_eq!(diacritic('\u{30E}'), Some(2));
        assert_eq!(diacritic(d(296)), Some(296));
        assert_eq!(diacritic('a'), None);
    }

    #[test]
    fn explicit_diacritics() {
        let row = resolve_row(&[
            (0, raw(42, Some(1), Some(0), Some(2))),
            (1, raw(42, Some(1), Some(1), Some(2))),
        ]);
        assert_eq!(
            row[1].1,
            Resolved {
                image_id: 42 + (2 << 24),
                placement_id: 0,
                row: 1,
                col: 1
            }
        );
    }

    #[test]
    fn omitted_diacritics_follow_the_cell_to_the_left() {
        // Only the first cell numbers its row; the rest count on.
        let row = resolve_row(&[
            (3, raw(42, Some(1), Some(0), Some(5))),
            (4, raw(42, None, None, None)),
            (5, raw(42, Some(1), None, None)),
            (6, raw(42, Some(1), Some(3), None)),
        ]);
        let cols: Vec<_> = row
            .iter()
            .map(|(_, r)| (r.row, r.col, r.image_id >> 24))
            .collect();
        assert_eq!(cols, vec![(1, 0, 5), (1, 1, 5), (1, 2, 5), (1, 3, 5)]);
    }

    #[test]
    fn no_inheritance_across_images_or_gaps() {
        let row = resolve_row(&[
            (0, raw(42, Some(2), Some(4), None)),
            (1, raw(43, None, None, None)),
            (3, raw(43, None, None, None)),
        ]);
        assert_eq!((row[1].1.row, row[1].1.col), (0, 0));
        assert_eq!((row[2].1.row, row[2].1.col), (0, 0));
    }

    #[test]
    fn reads_cells() {
        use alacritty_terminal::vte::ansi::Rgb;
        let mut cell = Cell {
            c: PLACEHOLDER,
            fg: Color::Spec(Rgb { r: 0, g: 1, b: 2 }),
            ..Cell::default()
        };
        cell.push_zerowidth(d(3));
        cell.push_zerowidth(d(4));
        cell.set_underline_color(Some(Color::Indexed(7)));
        let raw = Raw::from_cell(&cell).unwrap();
        assert_eq!(raw.fg, 0x102);
        assert_eq!(raw.underline, 7);
        assert_eq!((raw.row, raw.col, raw.msb), (Some(3), Some(4), None));
        assert!(Raw::from_cell(&Cell::default()).is_none());
    }
}
