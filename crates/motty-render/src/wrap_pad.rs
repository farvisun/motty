//! `strip-cjk-wrap-pad`: when a wide character doesn't fit in the last
//! column, zsh fills that column with a reverse-video space and wraps the
//! character, leaving a small solid block at the row's right edge. That
//! one cell is hidden; reverse-video bars stay as they are.

use motty_term::alacritty_terminal::grid::{Dimensions, Grid};
use motty_term::alacritty_terminal::index::{Column, Point};
use motty_term::alacritty_terminal::term::cell::{Cell, Flags};

/// Whether the cell at `point` (in the last column) is such a pad: a
/// reverse-video space ending a soft-wrapped row, not continuing a
/// reverse-video run, with a wide character starting the next row.
pub fn is_wrap_pad(grid: &Grid<Cell>, point: Point) -> bool {
    let cell = &grid[point];
    let last = grid.columns() - 1;
    if point.column.0 != last
        || cell.c != ' '
        || !cell.flags.contains(Flags::INVERSE | Flags::WRAPLINE)
        || point.line >= grid.bottommost_line()
    {
        return false;
    }
    let lone = last == 0 || {
        let before = &grid[point.line][Column(last - 1)];
        !before.flags.contains(Flags::INVERSE)
    };
    let next = &grid[Point::new(point.line + 1i32, Column(0))];
    lone && next.flags.contains(Flags::WIDE_CHAR)
}

#[cfg(test)]
mod tests {
    use motty_term::GridSize;
    use motty_term::alacritty_terminal::event::VoidListener;
    use motty_term::alacritty_terminal::index::Line;
    use motty_term::alacritty_terminal::term::{Config, Term};
    use motty_term::alacritty_terminal::vte::ansi::Processor;

    use super::*;

    fn term(bytes: &str) -> Term<VoidListener> {
        let size = GridSize {
            cols: 6,
            rows: 3,
            cell_width: 8.,
            cell_height: 16.,
        };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, bytes.as_bytes());
        term
    }

    fn pad(term: &Term<VoidListener>) -> bool {
        is_wrap_pad(term.grid(), Point::new(Line(0), Column(5)))
    }

    #[test]
    fn the_lone_pad_before_a_wrapped_wide_character() {
        // What zsh writes: five cells, a reverse-video space, then 漢.
        assert!(pad(&term("abcde\x1b[7m \x1b[m漢")));
    }

    #[test]
    fn everything_else_stays() {
        // A reverse-video bar running into the last column.
        assert!(!pad(&term("abcd\x1b[7m  \x1b[m漢")));
        // The row wraps onto narrow text.
        assert!(!pad(&term("abcde\x1b[7m \x1b[mx")));
        // A plain space at the edge.
        assert!(!pad(&term("abcde 漢")));
        // A hard line break: no soft wrap.
        assert!(!pad(&term("abcde\x1b[7m \x1b[m\r\n漢")));
    }
}
