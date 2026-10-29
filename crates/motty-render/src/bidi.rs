//! Bidirectional text, applied when drawing only: the grid stays in logical
//! order (so copy, paste and find are untouched) and each row containing
//! right-to-left text is reordered with the Unicode Bidirectional
//! Algorithm, with a left-to-right paragraph level so prompts and output
//! keep their left edge.
//!
//! A right-to-left run is shaped by CoreText in logical order (wrapped in
//! RLO…PDF, so its glyphs come out mirrored and right to left whatever
//! CoreText would have guessed) and its glyphs are then placed on the cells
//! that run occupies on screen: one glyph per cell for scripts that don't
//! join (Hebrew), the shaped word kept whole and centred on its cells for
//! those that do (Arabic, Persian), so letters stay connected.

use unicode_bidi::{BidiClass, Level, ParagraphBidiInfo, bidi_class};

/// One cell as the algorithm sees it.
#[derive(Clone, Copy, Debug)]
pub struct BidiCell<'a> {
    pub c: char,
    /// Combining marks and other zero-width characters stored with it.
    pub extra: &'a [char],
    /// The right half of a wide character: it moves with that character.
    pub spacer: bool,
}

/// Whether `c` makes a row need reordering: a strong right-to-left letter
/// or an explicit right-to-left embedding, override or isolate.
#[inline]
pub fn starts_rtl(c: char) -> bool {
    c >= '\u{0590}'
        && matches!(
            bidi_class(c),
            BidiClass::R | BidiClass::AL | BidiClass::RLE | BidiClass::RLO | BidiClass::RLI
        )
}

/// Letters of scripts that join (Arabic, Syriac, N'Ko, …): their runs are
/// kept whole rather than split into cells.
#[inline]
pub fn joins(c: char) -> bool {
    c >= '\u{0600}' && bidi_class(c) == BidiClass::AL
}

/// A row's drawing order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineOrder {
    /// `visual[v]`: the logical column drawn at visual column `v`.
    pub visual: Vec<u16>,
    /// `logical[l]`: the visual column logical column `l` is drawn at.
    pub logical: Vec<u16>,
    /// Resolved embedding level per logical column; odd is right to left.
    pub levels: Vec<u8>,
}

impl LineOrder {
    pub fn to_visual(&self, col: usize) -> usize {
        self.logical.get(col).map_or(col, |&v| v as usize)
    }

    pub fn to_logical(&self, col: usize) -> usize {
        self.visual.get(col).map_or(col, |&l| l as usize)
    }

    /// Whether logical column `col` reads right to left.
    pub fn is_rtl(&self, col: usize) -> bool {
        self.levels.get(col).is_some_and(|l| l % 2 == 1)
    }
}

/// The drawing order of a row, or `None` when it reads left to right as
/// stored (the common case, decided without running the algorithm).
pub fn reorder(cells: &[BidiCell]) -> Option<LineOrder> {
    if !cells
        .iter()
        .any(|c| starts_rtl(c.c) || c.extra.iter().any(|&e| starts_rtl(e)))
    {
        return None;
    }
    // Units: a cell plus the spacer cells after it.
    let mut unit_cells: Vec<(usize, usize)> = Vec::with_capacity(cells.len());
    let mut text = String::with_capacity(cells.len() * 2);
    let mut starts = Vec::with_capacity(cells.len());
    for (i, cell) in cells.iter().enumerate() {
        if cell.spacer
            && let Some(last) = unit_cells.last_mut()
        {
            last.1 += 1;
            continue;
        }
        unit_cells.push((i, 1));
        starts.push(text.len());
        // Paragraph separators would start a new paragraph mid-row.
        let plain = |c: char| {
            if bidi_class(c) == BidiClass::B {
                ' '
            } else {
                c
            }
        };
        text.push(plain(cell.c));
        text.extend(cell.extra.iter().map(|&c| plain(c)));
    }
    let info = ParagraphBidiInfo::new(&text, Some(Level::ltr()));
    let byte_levels = info.reordered_levels(0..text.len());
    let unit_levels: Vec<Level> = starts.iter().map(|&s| byte_levels[s]).collect();
    // Even levels only (digits at level 2 at most): nothing moves.
    if unit_levels.iter().all(|l| l.is_ltr()) {
        return None;
    }
    let order = ParagraphBidiInfo::reorder_visual(&unit_levels);

    let n = cells.len();
    let mut visual = Vec::with_capacity(n);
    let mut levels = vec![0u8; n];
    for (&(first, len), level) in unit_cells.iter().zip(&unit_levels) {
        levels[first..first + len].fill(level.number());
    }
    for &unit in &order {
        let (first, len) = unit_cells[unit];
        // A wide character keeps its spacer on its right.
        visual.extend((first..first + len).map(|c| c as u16));
    }
    let mut logical = vec![0u16; n];
    for (v, &l) in visual.iter().enumerate() {
        logical[l as usize] = v as u16;
    }
    Some(LineOrder {
        visual,
        logical,
        levels,
    })
}

/// Rows' drawing orders from the last frame, for turning a pointer
/// position into a grid column.
#[derive(Clone, Debug, Default)]
pub struct VisualMap {
    pub rows: Vec<Option<LineOrder>>,
}

impl VisualMap {
    /// The logical column drawn at `(row, col)` and whether it reads right
    /// to left (so the halves of the cell swap meaning).
    pub fn logical(&self, row: usize, col: usize) -> (usize, bool) {
        match self.rows.get(row).and_then(Option::as_ref) {
            Some(order) => {
                let l = order.to_logical(col);
                (l, order.is_rtl(l))
            }
            None => (col, false),
        }
    }

    /// The visual column logical `col` of `row` is drawn at.
    pub fn visual(&self, row: usize, col: usize) -> usize {
        match self.rows.get(row).and_then(Option::as_ref) {
            Some(order) => order.to_visual(col),
            None => col,
        }
    }
}

/// Opens a right-to-left run handed to the shaper (RLO).
pub const RLO: char = '\u{202E}';
/// Closes it (PDF).
pub const PDF: char = '\u{202C}';

/// Where each glyph of a right-to-left run goes, as an x offset from the
/// run's left edge on screen; `None` for glyphs to drop (the RLO and PDF
/// wrapped around the text).
///
/// `glyphs` are `(byte index, shaped x)`; `starts[k]` is the byte where
/// logical cell `k` of the run begins (after the RLO); `text_end` is where
/// the PDF begins. The run's cells are drawn mirrored: logical cell `k` of
/// `n` sits at visual cell `n - 1 - k`.
pub fn place_rtl(
    glyphs: &[(usize, f32)],
    starts: &[usize],
    text_end: usize,
    cell_width: f32,
    natural_width: f32,
    joining: bool,
) -> Vec<Option<f32>> {
    let n = starts.len();
    let span = n as f32 * cell_width;
    let cell_of = |index: usize| starts.partition_point(|&s| s <= index).saturating_sub(1);
    let inside = |index: usize| index >= starts.first().copied().unwrap_or(0) && index < text_end;
    if joining {
        // Keep the shaped run whole, squeezed to fit, centred on its cells.
        let scale = if natural_width > span {
            span / natural_width
        } else {
            1.
        };
        let offset = (span - natural_width * scale) / 2.;
        return glyphs
            .iter()
            .map(|&(index, x)| inside(index).then_some(offset + x * scale))
            .collect();
    }
    // Snap to cells: each cell's leftmost glyph to the cell's left edge,
    // marks keeping their offset from it. A cell without glyphs of its own
    // was absorbed into a ligature with the cell before it (logically).
    let mut has_glyph = vec![false; n];
    let mut left = vec![f32::INFINITY; n];
    for &(index, x) in glyphs {
        if inside(index) {
            let k = cell_of(index);
            has_glyph[k] = true;
            left[k] = left[k].min(x);
        }
    }
    // The last logical cell a glyph's cluster covers: drawn leftmost.
    let mut cluster_end = (0..n).collect::<Vec<_>>();
    for k in (0..n.saturating_sub(1)).rev() {
        if !has_glyph[k + 1] {
            cluster_end[k] = cluster_end[k + 1];
        }
    }
    glyphs
        .iter()
        .map(|&(index, x)| {
            inside(index).then(|| {
                let k = cell_of(index);
                let visual = n - 1 - cluster_end[k];
                visual as f32 * cell_width + (x - left[k])
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(text: &str) -> Vec<BidiCell<'static>> {
        text.chars()
            .map(|c| BidiCell {
                c,
                extra: &[],
                spacer: false,
            })
            .collect()
    }

    /// The row as drawn, left to right.
    fn shown(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        match reorder(&cells(text)) {
            Some(order) => order.visual.iter().map(|&l| chars[l as usize]).collect(),
            None => text.to_owned(),
        }
    }

    #[test]
    fn left_to_right_rows_are_left_alone() {
        assert_eq!(reorder(&cells("ls -la ~/src 123 «quoted»")), None);
        // Arabic-Indic digits alone don't reorder either.
        assert_eq!(reorder(&cells("total ١٢٣")), None);
    }

    #[test]
    fn hebrew() {
        assert_eq!(shown("שלום"), "םולש");
        assert_eq!(shown("echo שלום עולם!"), "echo םלוע םולש!");
        // Numbers inside keep reading left to right.
        assert_eq!(shown("גרסה 2.5 חדשה"), "השדח 2.5 הסרג");
    }

    #[test]
    fn arabic() {
        assert_eq!(shown("مرحبا بالعالم"), "ملاعلاب ابحرم");
        assert_eq!(shown("x مرحبا y"), "x ابحرم y");
    }

    #[test]
    fn persian_with_digits() {
        // Extended Arabic-Indic digits after Persian letters read left to
        // right, and the run reads right to left before the Latin word.
        assert_eq!(shown("سلام دنیا ۱۲۳ hello"), "۱۲۳ ایند مالس hello");
        assert_eq!(shown("نسخه 3.14 آماده"), "هدامآ 3.14 هخسن");
    }

    #[test]
    fn mixed_paths_and_punctuation() {
        // A path stays whole and in place.
        assert_eq!(shown("cp ~/فایل.txt /tmp"), "cp ~/لیاف.txt /tmp");
        // Brackets after Hebrew go with it (the shaper mirrors them); at
        // the start of a line they stay with the paragraph.
        assert_eq!(shown("שלום (עולם)"), ")םלוע( םולש");
        assert_eq!(shown("(שלום)"), "(םולש)");
        // Trailing blanks stay at the paragraph level (rule L1).
        let order = reorder(&cells("שלום   ")).unwrap();
        assert_eq!(&order.visual[4..], &[4, 5, 6]);
    }

    #[test]
    fn cursor_and_selection_columns_map_both_ways() {
        let order = reorder(&cells("ab שלום cd")).unwrap();
        // Logical 3 (ש) is drawn at visual 6; the cursor after the word
        // (logical 7, a space) sits where it was.
        assert_eq!(order.to_visual(3), 6);
        assert_eq!(order.to_logical(6), 3);
        assert_eq!(order.to_visual(7), 7);
        assert!(order.is_rtl(3) && !order.is_rtl(0));
        for l in 0..order.visual.len() {
            assert_eq!(order.to_logical(order.to_visual(l)), l);
        }
        let map = VisualMap {
            rows: vec![None, Some(order)],
        };
        assert_eq!(map.logical(0, 3), (3, false));
        assert_eq!(map.logical(1, 6), (3, true));
        assert_eq!(map.visual(1, 3), 6);
        assert_eq!(map.logical(5, 2), (2, false));
    }

    #[test]
    fn wide_characters_keep_their_spacer() {
        let mut row = cells("שלום 漢 ");
        // 漢 is wide: its spacer follows it.
        row.insert(
            6,
            BidiCell {
                c: ' ',
                extra: &[],
                spacer: true,
            },
        );
        let order = reorder(&row).unwrap();
        let v = order.to_visual(5);
        assert_eq!(order.visual[v + 1], 6);
    }

    #[test]
    fn marks_travel_with_their_letter() {
        let marks = ['\u{064E}'];
        let mut row = cells("بب");
        row[0].extra = &marks;
        let order = reorder(&row).unwrap();
        assert_eq!(order.visual, [1, 0]);
    }

    #[test]
    fn non_joining_runs_snap_to_cells() {
        // "שלום" shaped right to left: ם at x 0, ו 7, ל 14, ש 21; byte
        // starts after the 3-byte RLO.
        let starts = [3, 5, 7, 9];
        let glyphs = [(0, 0.), (9, 0.5), (7, 7.5), (5, 14.5), (3, 21.5), (11, 28.)];
        let x = place_rtl(&glyphs, &starts, 11, 8., 28., false);
        // RLO and PDF are dropped; ש (logical 0) lands in the last cell.
        assert_eq!(x, [None, Some(0.), Some(8.), Some(16.), Some(24.), None]);
    }

    #[test]
    fn ligatures_take_their_cells() {
        // لا: one glyph for two cells, at the cluster's leftmost cell.
        let starts = [3, 5];
        let x = place_rtl(&[(3, 0.)], &starts, 7, 8., 7., false);
        assert_eq!(x, [Some(0.)]);
        // A mark keeps its offset from its letter.
        let starts = [3, 7];
        let x = place_rtl(&[(7, 0.), (3, 6.), (5, 7.)], &starts, 9, 8., 14., false);
        assert_eq!(x, [Some(0.), Some(8.), Some(9.)]);
    }

    #[test]
    fn joining_runs_stay_whole_and_centred() {
        let starts = [3, 5, 7, 9];
        // Narrower than its 4 cells (32): centred, joins untouched.
        let x = place_rtl(&[(9, 0.), (3, 20.)], &starts, 11, 8., 24., true);
        assert_eq!(x, [Some(4.), Some(24.)]);
        // Wider: squeezed to the cells.
        let x = place_rtl(&[(9, 0.), (3, 32.)], &starts, 11, 8., 64., true);
        assert_eq!(x, [Some(0.), Some(16.)]);
    }
}
