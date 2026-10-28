//! Find in scrollback: every match of a literal or regex query, as stable
//! rows so the list stays valid while output scrolls.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Direction, Line, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::search::{RegexIter, RegexSearch};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

/// A match from `start` to `end` inclusive, each (stable row, column); a
/// stable row is `Grid::lines_pushed + line`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SearchMatch {
    pub start: (u64, usize),
    pub end: (u64, usize),
}

impl SearchMatch {
    /// The match in grid coordinates, if its rows are still in the grid.
    pub fn to_points<T>(self, term: &Term<T>) -> Option<(Point, Point)> {
        let pushed = term.grid().lines_pushed() as i64;
        let line = |row: u64| {
            let line = row as i64 - pushed;
            (line >= term.grid().topmost_line().0 as i64
                && line <= term.grid().bottommost_line().0 as i64)
                .then_some(Line(line as i32))
        };
        Some((
            Point::new(line(self.start.0)?, self.start.1.into()),
            Point::new(line(self.end.0)?, self.end.1.into()),
        ))
    }
}

/// The regex for `query` under `options`.
pub fn pattern(query: &str, options: SearchOptions) -> String {
    let body = if options.regex {
        query.to_owned()
    } else {
        escape(query)
    };
    // Overrides alacritty's smart case. ASCII word boundaries: its DFAs
    // can't do Unicode ones.
    let case = if options.case_sensitive {
        "(?-i)"
    } else {
        "(?i)"
    };
    if options.whole_word {
        format!(r"{case}(?-u:\b)(?:{body})(?-u:\b)")
    } else {
        format!("{case}(?:{body})")
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if r"\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Matches top to bottom, at most `limit`. Errors on an invalid regex.
pub fn find_all<T>(
    term: &Term<T>,
    query: &str,
    options: SearchOptions,
    limit: usize,
) -> Result<Vec<SearchMatch>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut regex = RegexSearch::new(&pattern(query, options)).map_err(|e| e.to_string())?;
    let grid = term.grid();
    let start = Point::new(grid.topmost_line(), 0.into());
    let end = Point::new(grid.bottommost_line(), grid.last_column());
    let pushed = grid.lines_pushed() as i64;
    let stable = |p: Point| ((pushed + p.line.0 as i64) as u64, p.column.0);
    Ok(
        RegexIter::new(start, end, Direction::Right, term, &mut regex)
            .take(limit)
            .map(|m| SearchMatch {
                start: stable(*m.start()),
                end: stable(*m.end()),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::{Config, Term, test::TermSize};
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;

    fn find(text: &str, query: &str, options: SearchOptions) -> Vec<SearchMatch> {
        let mut term = Term::new(Config::default(), &TermSize::new(20, 4), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, text.as_bytes());
        find_all(&term, query, options, usize::MAX).unwrap()
    }

    #[test]
    fn literal_case_and_words() {
        let text = "Doc docs a.c\r\nabc doc\r\n";
        let any = SearchOptions::default();
        assert_eq!(find(text, "doc", any).len(), 3);
        let case = SearchOptions {
            case_sensitive: true,
            ..any
        };
        assert_eq!(find(text, "Doc", case).len(), 1);
        let word = SearchOptions {
            whole_word: true,
            ..any
        };
        assert_eq!(
            find(text, "doc", word),
            vec![
                SearchMatch {
                    start: (0, 0),
                    end: (0, 2)
                },
                SearchMatch {
                    start: (1, 4),
                    end: (1, 6)
                },
            ]
        );
        // `.` is literal unless regex is on.
        assert_eq!(find(text, "a.c", any).len(), 1);
        let regex = SearchOptions { regex: true, ..any };
        assert_eq!(find(text, "a.c", regex).len(), 2);
    }

    #[test]
    fn rows_stay_stable_as_output_scrolls() {
        // Six lines into a four-line screen: two scroll into history.
        let found = find(
            "x1\r\nx2\r\nx3\r\nx4\r\nx5\r\nx6",
            "x6",
            SearchOptions::default(),
        );
        assert_eq!(found[0].start, (5, 0));
    }
}
