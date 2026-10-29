//! How a file's lines become rows on screen: tabs expanded to their stops,
//! whitespace shown as glyphs (`editor-visible-whitespace`), and long lines
//! wrapped at the pane's width (`editor-line-wrap`).
//!
//! Text is monospaced, so wrapping counts columns rather than shaping:
//! wide characters take two, tabs run to the next stop.

use std::ops::Range;

use unicode_width::UnicodeWidthChar;

/// Shown for a space, and at the start of a tab, with visible whitespace.
const SPACE: char = '·';
const TAB: char = '→';

/// A character's width in columns, `col` being where it starts.
fn width(c: char, col: usize, tab: usize) -> usize {
    match c {
        '\t' => tab - col % tab,
        c => c.width().unwrap_or(0),
    }
}

/// The columns `text` takes, starting at `col`.
pub fn columns(text: &str, col: usize, tab: usize) -> usize {
    text.chars().fold(col, |at, c| at + width(c, at, tab)) - col
}

/// Where a line's rows after the first start (byte offsets), wrapping at
/// `cols` columns: after the last space that leaves the row at least half
/// full, else wherever the row is full.
pub fn breaks(line: &str, tab: usize, cols: usize) -> Vec<usize> {
    let cols = cols.max(8);
    let mut out = Vec::new();
    let (mut row_col, mut col) = (0, 0);
    let mut space = None;
    for (ix, c) in line.char_indices() {
        let w = width(c, col, tab);
        if row_col + w > cols && row_col > 0 {
            let at = match space {
                Some((at, at_col)) if at_col * 2 >= cols => {
                    row_col -= at_col;
                    at
                }
                _ => {
                    row_col = 0;
                    ix
                }
            };
            out.push(at);
            space = None;
        }
        col += w;
        row_col += w;
        if c == ' ' || c == '\t' {
            space = Some((ix + c.len_utf8(), row_col));
        }
    }
    out
}

/// One row of a line as it's drawn, and the way back to the line's bytes.
pub struct DisplayRow {
    pub text: String,
    /// `(source byte, display byte)` at each character boundary, end
    /// included; both ascend.
    map: Vec<(usize, usize)>,
    /// Display bytes that are whitespace glyphs, drawn faint.
    pub glyphs: Vec<Range<usize>>,
}

impl DisplayRow {
    /// `source` (a row's slice of a line) starting at line column `col`.
    pub fn new(source: &str, col: usize, tab: usize, whitespace: bool) -> Self {
        let mut text = String::with_capacity(source.len());
        let mut map = Vec::with_capacity(source.len() + 1);
        let mut glyphs = Vec::new();
        let mut at = col;
        for (ix, c) in source.char_indices() {
            map.push((ix, text.len()));
            let w = width(c, at, tab);
            at += w;
            match c {
                '\t' => {
                    let start = text.len();
                    if whitespace {
                        text.push(TAB);
                        glyphs.push(start..text.len());
                        text.extend(std::iter::repeat_n(' ', w.saturating_sub(1)));
                    } else {
                        text.extend(std::iter::repeat_n(' ', w));
                    }
                }
                ' ' if whitespace => {
                    let start = text.len();
                    text.push(SPACE);
                    glyphs.push(start..text.len());
                }
                c => text.push(c),
            }
        }
        map.push((source.len(), text.len()));
        Self { text, map, glyphs }
    }

    /// The display byte for source byte `src` (a character boundary).
    pub fn to_display(&self, src: usize) -> usize {
        match self.map.binary_search_by_key(&src, |m| m.0) {
            Ok(i) => self.map[i].1,
            Err(i) => self.map[i.min(self.map.len() - 1)].1,
        }
    }

    /// The source byte for display byte `disp`: inside an expanded tab,
    /// whichever edge is nearer.
    pub fn to_source(&self, disp: usize) -> usize {
        match self.map.binary_search_by_key(&disp, |m| m.1) {
            Ok(i) => self.map[i].0,
            Err(0) => 0,
            Err(i) if i >= self.map.len() => self.map[self.map.len() - 1].0,
            Err(i) => {
                let (before, after) = (self.map[i - 1], self.map[i]);
                if disp - before.1 <= after.1 - disp {
                    before.0
                } else {
                    after.0
                }
            }
        }
    }
}

/// Every line's rows, for the pane's current width.
#[derive(Default)]
pub struct Rows {
    /// The first row of each line, then the total.
    starts: Vec<usize>,
    /// Each line's row starts after its first (bytes).
    breaks: Vec<Vec<usize>>,
}

impl Rows {
    /// `cols`: wrap there, or never with `None`.
    pub fn new<'a>(lines: impl Iterator<Item = &'a str>, tab: usize, cols: Option<usize>) -> Self {
        let mut starts = Vec::new();
        let mut breaks = Vec::new();
        let mut row = 0;
        for line in lines {
            starts.push(row);
            let b = match cols {
                Some(cols) if line.len() > cols / 2 => self::breaks(line, tab, cols),
                _ => Vec::new(),
            };
            row += b.len() + 1;
            breaks.push(b);
        }
        starts.push(row);
        Self { starts, breaks }
    }

    pub fn lines(&self) -> usize {
        self.breaks.len()
    }

    pub fn total(&self) -> usize {
        self.starts.last().copied().unwrap_or(0)
    }

    /// The line on `row`, and which of its rows it is.
    pub fn locate(&self, row: usize) -> (usize, usize) {
        let line = match self.starts.binary_search(&row) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
        .min(self.breaks.len().saturating_sub(1));
        (line, row.saturating_sub(self.starts[line]))
    }

    pub fn first_row(&self, line: usize) -> usize {
        self.starts.get(line).copied().unwrap_or(0)
    }

    /// Row `seg` of `line` as a byte range of `text` (that line).
    pub fn segment(&self, line: usize, seg: usize, text_len: usize) -> Range<usize> {
        let b = self.breaks.get(line).map_or(&[][..], Vec::as_slice);
        let start = if seg == 0 { 0 } else { b[seg - 1] };
        let end = b.get(seg).copied().unwrap_or(text_len);
        start..end
    }

    /// Which of `line`'s rows holds byte `col`: a break belongs to the row
    /// it starts.
    pub fn segment_of(&self, line: usize, col: usize) -> usize {
        self.breaks
            .get(line)
            .map_or(0, |b| b.iter().take_while(|&&at| at <= col).count())
    }

    /// The row `col` of `line` is on.
    pub fn row_of(&self, line: usize, col: usize) -> usize {
        self.first_row(line) + self.segment_of(line, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_run_to_the_next_stop() {
        assert_eq!(columns("a\tb", 0, 4), 5);
        assert_eq!(columns("\t", 2, 4), 2);
        assert_eq!(columns("漢字", 0, 4), 4);
    }

    #[test]
    fn wraps_after_a_space_when_it_leaves_half_a_row() {
        // 10 columns: "hello wor|ld again" breaks after "hello ".
        assert_eq!(breaks("hello world again", 4, 10), [6, 12]);
        // No space in the first half: break mid-word.
        assert_eq!(breaks("abcdefghijklmnop", 4, 10), [10]);
    }

    #[test]
    fn display_maps_both_ways_through_tabs_and_glyphs() {
        let row = DisplayRow::new("a\tb c", 0, 4, true);
        assert_eq!(row.text, "a→  b·c");
        assert_eq!(row.to_display(1), 1);
        assert_eq!(row.to_display(2), 1 + '→'.len_utf8() + 2);
        assert_eq!(row.to_source(row.to_display(4)), 4);
        // Inside the tab's run, the nearer edge.
        assert_eq!(row.to_source(row.to_display(1) + 1), 1);
        assert_eq!(row.glyphs.len(), 2);
        let plain = DisplayRow::new("a\tb", 0, 4, false);
        assert_eq!(plain.text, "a   b");
        assert_eq!(plain.to_source(4), 2);
    }

    #[test]
    fn rows_locate_lines_and_segments() {
        let lines = ["short", "hello world again", ""];
        let rows = Rows::new(lines.iter().copied(), 4, Some(10));
        assert_eq!(rows.total(), 5);
        assert_eq!(rows.locate(0), (0, 0));
        assert_eq!(rows.locate(2), (1, 1));
        assert_eq!(rows.locate(4), (2, 0));
        assert_eq!(rows.segment(1, 1, lines[1].len()), 6..12);
        assert_eq!(rows.row_of(1, 6), 2);
        assert_eq!(rows.row_of(1, 5), 1);
        let flat = Rows::new(lines.iter().copied(), 4, None);
        assert_eq!(flat.total(), 3);
    }
}
