//! The file editor's text: lines without their breaks, positions as
//! (line, byte column) on grapheme boundaries.

use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    /// Byte offset into the line.
    pub col: usize,
}

impl Pos {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buffer {
    lines: Vec<String>,
    /// The file used CRLF line breaks; saving keeps them.
    crlf: bool,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
            crlf: false,
        }
    }
}

impl Buffer {
    pub fn from_text(text: &str) -> Self {
        let crlf = text.contains("\r\n");
        let lines = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
            .collect();
        Self { lines, crlf }
    }

    pub fn text(&self) -> String {
        self.lines.join(if self.crlf { "\r\n" } else { "\n" })
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, ix: usize) -> &str {
        self.lines.get(ix).map_or("", String::as_str)
    }

    pub fn end(&self) -> Pos {
        let line = self.lines.len() - 1;
        Pos::new(line, self.lines[line].len())
    }

    /// `p` moved into the text and onto a character boundary.
    pub fn clamp(&self, p: Pos) -> Pos {
        if p.line >= self.lines.len() {
            return self.end();
        }
        let line = &self.lines[p.line];
        let mut col = p.col.min(line.len());
        while !line.is_char_boundary(col) {
            col -= 1;
        }
        Pos::new(p.line, col)
    }

    /// Inserts `text` at `at`; returns the position after it.
    pub fn insert(&mut self, at: Pos, text: &str) -> Pos {
        let at = self.clamp(at);
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let tail = self.lines[at.line].split_off(at.col);
        let mut parts = text.split('\n');
        self.lines[at.line].push_str(parts.next().unwrap_or(""));
        let mut line = at.line;
        for part in parts {
            line += 1;
            self.lines.insert(line, part.to_owned());
        }
        let col = self.lines[line].len();
        self.lines[line].push_str(&tail);
        Pos::new(line, col)
    }

    /// Removes the text between `a` and `b` (in either order).
    pub fn delete(&mut self, a: Pos, b: Pos) {
        let (a, b) = order(self.clamp(a), self.clamp(b));
        if a == b {
            return;
        }
        if a.line == b.line {
            self.lines[a.line].replace_range(a.col..b.col, "");
            return;
        }
        let tail = self.lines[b.line][b.col..].to_owned();
        self.lines[a.line].truncate(a.col);
        self.lines[a.line].push_str(&tail);
        self.lines.drain(a.line + 1..=b.line);
    }

    pub fn slice(&self, a: Pos, b: Pos) -> String {
        let (a, b) = order(self.clamp(a), self.clamp(b));
        if a.line == b.line {
            return self.lines[a.line][a.col..b.col].to_owned();
        }
        let mut out = self.lines[a.line][a.col..].to_owned();
        for line in &self.lines[a.line + 1..b.line] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.push_str(&self.lines[b.line][..b.col]);
        out
    }

    /// One character (grapheme) left, onto the previous line's end.
    pub fn prev(&self, p: Pos) -> Pos {
        let p = self.clamp(p);
        if p.col == 0 {
            return match p.line {
                0 => p,
                line => Pos::new(line - 1, self.lines[line - 1].len()),
            };
        }
        let line = &self.lines[p.line][..p.col];
        let col = line
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i);
        Pos::new(p.line, col)
    }

    /// One character (grapheme) right, onto the next line's start.
    pub fn next(&self, p: Pos) -> Pos {
        let p = self.clamp(p);
        let line = &self.lines[p.line];
        if p.col >= line.len() {
            return if p.line + 1 < self.lines.len() {
                Pos::new(p.line + 1, 0)
            } else {
                p
            };
        }
        let step = line[p.col..].graphemes(true).next().map_or(0, str::len);
        Pos::new(p.line, p.col + step)
    }

    /// The start of the word before `p` (⌥←).
    pub fn word_left(&self, p: Pos) -> Pos {
        let p = self.clamp(p);
        if p.col == 0 {
            return self.prev(p);
        }
        let line = &self.lines[p.line][..p.col];
        let col = line
            .split_word_bound_indices()
            .rev()
            .find(|(_, w)| w.chars().any(char::is_alphanumeric))
            .map_or(0, |(i, _)| i);
        Pos::new(p.line, col)
    }

    /// The end of the word after `p` (⌥→).
    pub fn word_right(&self, p: Pos) -> Pos {
        let p = self.clamp(p);
        let line = &self.lines[p.line];
        if p.col >= line.len() {
            return self.next(p);
        }
        let col = line[p.col..]
            .split_word_bound_indices()
            .find(|(_, w)| w.chars().any(char::is_alphanumeric))
            .map_or(line.len(), |(i, w)| p.col + i + w.len());
        Pos::new(p.line, col)
    }

    /// The word around `p`, for a double-click.
    pub fn word_at(&self, p: Pos) -> (Pos, Pos) {
        let p = self.clamp(p);
        let line = &self.lines[p.line];
        for (i, w) in line.split_word_bound_indices() {
            if p.col >= i && p.col < i + w.len() || p.col == line.len() && i + w.len() == p.col {
                return (Pos::new(p.line, i), Pos::new(p.line, i + w.len()));
            }
        }
        (p, p)
    }

    /// Where the line's text starts after its indentation.
    pub fn indent_end(&self, line: usize) -> usize {
        let text = self.line(line);
        text.len() - text.trim_start_matches([' ', '\t']).len()
    }
}

pub fn order(a: Pos, b: Pos) -> (Pos, Pos) {
    if a <= b { (a, b) } else { (b, a) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_text() {
        for text in ["", "a", "a\nb", "a\n", "x\r\ny\r\n"] {
            assert_eq!(Buffer::from_text(text).text(), text);
        }
    }

    #[test]
    fn inserts_and_deletes_across_lines() {
        let mut b = Buffer::from_text("hello world");
        let end = b.insert(Pos::new(0, 5), ",\nbig");
        assert_eq!(b.text(), "hello,\nbig world");
        assert_eq!(end, Pos::new(1, 3));
        assert_eq!(b.slice(Pos::new(0, 3), Pos::new(1, 2)), "lo,\nbi");
        b.delete(Pos::new(1, 2), Pos::new(0, 3));
        assert_eq!(b.text(), "helg world");
    }

    #[test]
    fn moves_by_grapheme() {
        let b = Buffer::from_text("é👍🏽x\ny");
        let p = b.next(Pos::default());
        assert_eq!(p.col, "é".len());
        let p = b.next(p);
        assert_eq!(p.col, "é👍🏽".len());
        assert_eq!(b.prev(p).col, "é".len());
        assert_eq!(b.next(b.end_of(0)), Pos::new(1, 0));
        assert_eq!(b.prev(Pos::new(1, 0)), b.end_of(0));
    }

    #[test]
    fn moves_by_word() {
        let b = Buffer::from_text("let foo_bar = baz();");
        assert_eq!(b.word_right(Pos::new(0, 0)).col, 3);
        assert_eq!(b.word_right(Pos::new(0, 3)).col, 11);
        assert_eq!(b.word_left(Pos::new(0, 20)).col, 14);
        assert_eq!(b.word_left(Pos::new(0, 11)).col, 4);
        assert_eq!(b.word_at(Pos::new(0, 6)), (Pos::new(0, 4), Pos::new(0, 11)));
    }

    #[test]
    fn clamps_to_char_boundaries() {
        let b = Buffer::from_text("é");
        assert_eq!(b.clamp(Pos::new(0, 1)).col, 0);
        assert_eq!(b.clamp(Pos::new(5, 0)), b.end());
    }

    impl Buffer {
        fn end_of(&self, line: usize) -> Pos {
            Pos::new(line, self.line(line).len())
        }
    }
}
