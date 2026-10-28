//! Links under the pointer: OSC 8 hyperlinks, URLs and file paths
//! (with `:line:column`), found in the logical line (wrapped rows joined).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use regex::Regex;

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Url(String),
    Path {
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },
}

impl Target {
    /// What the status tooltip shows.
    pub fn display(&self) -> String {
        match self {
            Target::Url(url) => url.clone(),
            Target::Path { path, line, column } => {
                let mut s = path.display().to_string();
                if let Some(l) = line {
                    s.push_str(&format!(":{l}"));
                    if let Some(c) = column {
                        s.push_str(&format!(":{c}"));
                    }
                }
                s
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// Grid range, inclusive.
    pub start: Point,
    pub end: Point,
    pub target: Target,
    /// An OSC 8 hyperlink (always underlined).
    pub explicit: bool,
}

/// Any `scheme://…` URL, and `mailto:`.
static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:[a-zA-Z][a-zA-Z0-9+.-]*://|mailto:)[^\s<>"'`]+"#).unwrap());

/// Absolute, `~`, `./`, `../` and relative paths with a slash, or a file
/// name with an extension, each optionally followed by `:line[:column]`.
static PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:~|\.{1,2})?/[^\s:'"`<>|()\[\]{}]+(?::\d+(?::\d+)?)?|[\w.@+-]+(?:/[^\s:'"`<>|()\[\]{}]+)+(?::\d+(?::\d+)?)?|[\w@+-][\w.@+-]*\.[A-Za-z0-9]+(?::\d+(?::\d+)?)?"#,
    )
    .unwrap()
});

/// The link at `point`: an OSC 8 hyperlink, else a URL, else a path that
/// exists (relative ones under `cwd`).
pub fn link_at<T>(term: &Term<T>, point: Point, cwd: Option<&Path>) -> Option<Link> {
    if let Some(link) = hyperlink_at(term, point) {
        return Some(link);
    }
    let (text, points) = logical_line(term, point);
    let at = points.iter().position(|p| *p == point)?;
    let byte_at = text.char_indices().nth(at)?.0;
    // Cells of the byte range `start..start + len`.
    let span = |start: usize, len: usize| {
        let first = text[..start].chars().count();
        let last = first + text[start..start + len].chars().count() - 1;
        (points[first], points[last])
    };
    for m in URL.find_iter(&text) {
        let url = trim_url(m.as_str());
        if (m.start()..m.start() + url.len()).contains(&byte_at) {
            let (start, end) = span(m.start(), url.len());
            return Some(Link {
                start,
                end,
                target: Target::Url(url.to_owned()),
                explicit: false,
            });
        }
    }
    for m in PATH.find_iter(&text) {
        let raw = m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if !(m.start()..m.start() + raw.len()).contains(&byte_at) {
            continue;
        }
        let target = resolve_path(raw, cwd)?;
        let (start, end) = span(m.start(), raw.len());
        return Some(Link {
            start,
            end,
            target,
            explicit: false,
        });
    }
    None
}

/// Drops trailing punctuation, and a closing bracket without its opener.
fn trim_url(url: &str) -> &str {
    let mut url = url;
    loop {
        let Some(last) = url.chars().last() else {
            return url;
        };
        let unbalanced = |open: char, close: char| {
            last == close && url.matches(open).count() < url.matches(close).count()
        };
        if ".,;:!?'\"".contains(last)
            || unbalanced('(', ')')
            || unbalanced('[', ']')
            || unbalanced('{', '}')
        {
            url = &url[..url.len() - last.len_utf8()];
        } else {
            return url;
        }
    }
}

/// `path[:line[:column]]`, if the file exists.
fn resolve_path(raw: &str, cwd: Option<&Path>) -> Option<Target> {
    let mut parts = raw.splitn(3, ':');
    let path = parts.next()?;
    let line = parts.next().and_then(|s| s.parse().ok());
    let column = parts.next().and_then(|s| s.parse().ok());
    let path = if let Some(rest) = path.strip_prefix("~/") {
        dirs_home()?.join(rest)
    } else if path == "~" {
        dirs_home()?
    } else if path.starts_with('/') {
        PathBuf::from(path)
    } else {
        cwd?.join(path)
    };
    path.exists().then_some(Target::Path { path, line, column })
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The OSC 8 hyperlink covering `point`, extended over neighbouring cells
/// with the same link (across wrapped rows).
fn hyperlink_at<T>(term: &Term<T>, point: Point) -> Option<Link> {
    let grid = term.grid();
    let link = grid[point].hyperlink()?;
    let same = |p: Point| grid[p].hyperlink().as_ref() == Some(&link);
    let last_col = grid.last_column();
    let (top, bottom) = (grid.topmost_line(), grid.bottommost_line());
    let prev = |p: Point| match p.column.0 {
        0 if p.line > top => Some(Point::new(p.line - 1, last_col)),
        0 => None,
        c => Some(Point::new(p.line, Column(c - 1))),
    };
    let next = |p: Point| {
        if p.column < last_col {
            Some(Point::new(p.line, p.column + 1))
        } else if p.line < bottom {
            Some(Point::new(p.line + 1, Column(0)))
        } else {
            None
        }
    };
    let mut start = point;
    while let Some(p) = prev(start).filter(|p| same(*p)) {
        start = p;
    }
    let mut end = point;
    while let Some(p) = next(end).filter(|p| same(*p)) {
        end = p;
    }
    Some(Link {
        start,
        end,
        target: Target::Url(link.uri().to_owned()),
        explicit: true,
    })
}

/// The text of the wrapped line through `point`, and each char's cell.
fn logical_line<T>(term: &Term<T>, point: Point) -> (String, Vec<Point>) {
    let grid = term.grid();
    let last_col = grid.last_column();
    let wraps = |line: Line| grid[line][last_col].flags.contains(Flags::WRAPLINE);
    let mut first = point.line;
    while first > grid.topmost_line() && wraps(first - 1) {
        first -= 1;
    }
    let mut last = point.line;
    while last < grid.bottommost_line() && wraps(last) {
        last += 1;
    }
    let mut text = String::new();
    let mut points = Vec::new();
    let mut line = first;
    while line <= last {
        for col in 0..grid.columns() {
            let cell = &grid[line][Column(col)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            text.push(cell.c);
            points.push(Point::new(line, Column(col)));
        }
        line += 1;
    }
    (text, points)
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::{Config, test::TermSize};
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;

    fn term(text: &str) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &TermSize::new(30, 4), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, text.as_bytes());
        term
    }

    fn at(line: i32, col: usize) -> Point {
        Point::new(Line(line), Column(col))
    }

    #[test]
    fn urls_trim_punctuation_and_wrap() {
        let t = term("see (https://x.io/a_(b)).\r\n");
        let link = link_at(&t, at(0, 10), None).unwrap();
        assert_eq!(link.target, Target::Url("https://x.io/a_(b)".into()));
        assert_eq!((link.start, link.end), (at(0, 5), at(0, 22)));
        // 40 chars on a 30-column screen wrap onto the next row.
        let t = term("open https://example.com/aaaaaaaaaaaaaaaaaa now");
        let link = link_at(&t, at(1, 2), None).unwrap();
        assert_eq!(
            link.target,
            Target::Url("https://example.com/aaaaaaaaaaaaaaaaaa".into())
        );
        assert_eq!(link_at(&t, at(0, 1), None), None);
    }

    #[test]
    fn paths_with_positions_must_exist() {
        let dir = env!("CARGO_MANIFEST_DIR");
        let t = term("err src/links.rs:12:5: oops\r\n");
        let link = link_at(&t, at(0, 6), Some(Path::new(dir))).unwrap();
        assert_eq!(
            link.target,
            Target::Path {
                path: Path::new(dir).join("src/links.rs"),
                line: Some(12),
                column: Some(5)
            }
        );
        assert_eq!(link.end, at(0, 20));
        assert_eq!(link_at(&t, at(0, 6), Some(Path::new("/nonexistent"))), None);
    }

    #[test]
    fn osc8_hyperlinks() {
        let t = term("a \x1b]8;;https://motty.dev\x1b\\link\x1b]8;;\x1b\\ b");
        let link = link_at(&t, at(0, 3), None).unwrap();
        assert!(link.explicit);
        assert_eq!((link.start, link.end), (at(0, 2), at(0, 5)));
        assert_eq!(link.target, Target::Url("https://motty.dev".into()));
    }
}
