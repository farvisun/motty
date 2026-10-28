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
    line_links(&text, &points, cwd, Matcher::Link)
        .into_iter()
        .find(|l| (l.start..=l.end).contains(&point))
}

/// What hint mode labels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Matcher {
    /// URLs, OSC 8 hyperlinks and file paths.
    #[default]
    Link,
    Url,
    Path,
    /// Word-like tokens of at least three characters.
    Word,
    Ip,
    /// Hexadecimal strings of 7 to 64 characters.
    Hash,
}

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w[\w-]+\w").unwrap());
static IP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b|\b(?:[0-9A-Fa-f]{1,4}:){2,7}[0-9A-Fa-f]{1,4}\b")
        .unwrap()
});
static HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9A-Fa-f]{7,64}\b").unwrap());

/// A hint target: its cells and what it resolves to.
#[derive(Clone, Debug, PartialEq)]
pub struct HintTarget {
    pub start: Point,
    pub end: Point,
    /// The text as shown.
    pub text: String,
    /// For links and paths: the URL or resolved path.
    pub link: Option<Target>,
}

/// Targets on screen for `matcher`, top to bottom.
pub fn hint_targets<T>(term: &Term<T>, matcher: Matcher, cwd: Option<&Path>) -> Vec<HintTarget> {
    let grid = term.grid();
    let top = Line(-(grid.display_offset() as i32));
    let bottom = top + (term.screen_lines() as i32 - 1);
    let mut out = Vec::new();
    let mut line = top;
    while line <= bottom {
        let (text, points) = logical_line(term, Point::new(line, Column(0)));
        let last = points.last().map_or(line, |p| p.line);
        let visible = |p: &Point| (top..=bottom).contains(&p.line);
        let mut found: Vec<HintTarget> = Vec::new();
        if matches!(matcher, Matcher::Link | Matcher::Url) {
            found.extend(
                line_hyperlinks(term, &points)
                    .into_iter()
                    .map(|l| HintTarget {
                        text: text_of(&text, &points, l.start, l.end),
                        start: l.start,
                        end: l.end,
                        link: Some(l.target),
                    }),
            );
        }
        let regex = match matcher {
            Matcher::Link | Matcher::Url | Matcher::Path => None,
            Matcher::Word => Some(&*WORD),
            Matcher::Ip => Some(&*IP),
            Matcher::Hash => Some(&*HASH),
        };
        if let Some(regex) = regex {
            for m in regex.find_iter(&text) {
                let (start, end) = span(&text, &points, m.start(), m.len());
                found.push(HintTarget {
                    start,
                    end,
                    text: m.as_str().to_owned(),
                    link: None,
                });
            }
        } else {
            for l in line_links(&text, &points, cwd, matcher) {
                // OSC 8 links win over text found inside them.
                if found.iter().any(|f| f.start <= l.end && l.start <= f.end) {
                    continue;
                }
                found.push(HintTarget {
                    text: text_of(&text, &points, l.start, l.end),
                    start: l.start,
                    end: l.end,
                    link: Some(l.target),
                });
            }
        }
        found.retain(|t| visible(&t.start));
        found.sort_by_key(|t| t.start);
        out.extend(found);
        line = last + 1;
    }
    out
}

/// Cells of the byte range `start..start + len` of a logical line.
fn span(text: &str, points: &[Point], start: usize, len: usize) -> (Point, Point) {
    let first = text[..start].chars().count();
    let last = first + text[start..start + len].chars().count() - 1;
    (points[first], points[last])
}

fn text_of(text: &str, points: &[Point], start: Point, end: Point) -> String {
    text.chars()
        .zip(points)
        .filter(|(_, p)| (start..=end).contains(*p))
        .map(|(c, _)| c)
        .collect()
}

/// URLs and existing paths in a logical line, left to right, not
/// overlapping (URLs first).
fn line_links(text: &str, points: &[Point], cwd: Option<&Path>, matcher: Matcher) -> Vec<Link> {
    let mut out: Vec<Link> = Vec::new();
    if matches!(matcher, Matcher::Link | Matcher::Url) {
        for m in URL.find_iter(text) {
            let url = trim_url(m.as_str());
            let (start, end) = span(text, points, m.start(), url.len());
            out.push(Link {
                start,
                end,
                target: Target::Url(url.to_owned()),
                explicit: false,
            });
        }
    }
    if matches!(matcher, Matcher::Link | Matcher::Path) {
        for m in PATH.find_iter(text) {
            let raw = m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']);
            if raw.is_empty() {
                continue;
            }
            let (start, end) = span(text, points, m.start(), raw.len());
            if out.iter().any(|l| l.start <= end && start <= l.end) {
                continue;
            }
            if let Some(target) = resolve_path(raw, cwd) {
                out.push(Link {
                    start,
                    end,
                    target,
                    explicit: false,
                });
            }
        }
    }
    out.sort_by_key(|l| l.start);
    out
}

/// OSC 8 hyperlinks among `points`, one per run of cells.
fn line_hyperlinks<T>(term: &Term<T>, points: &[Point]) -> Vec<Link> {
    let mut out: Vec<Link> = Vec::new();
    for p in points {
        if out.last().is_some_and(|l| (l.start..=l.end).contains(p)) {
            continue;
        }
        if let Some(link) = hyperlink_at(term, *p) {
            out.push(link);
        }
    }
    out
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
    fn hint_targets_by_matcher() {
        let dir = env!("CARGO_MANIFEST_DIR");
        let t = term("cat src/links.rs\r\nsee https://a.io/x 10.0.0.1\r\ncommit 3f9a2c1d ok");
        let cwd = Some(Path::new(dir));
        let texts = |m| -> Vec<String> {
            hint_targets(&t, m, cwd)
                .into_iter()
                .map(|h| h.text)
                .collect()
        };
        assert_eq!(texts(Matcher::Link), ["src/links.rs", "https://a.io/x"]);
        assert_eq!(texts(Matcher::Url), ["https://a.io/x"]);
        assert_eq!(texts(Matcher::Ip), ["10.0.0.1"]);
        assert_eq!(texts(Matcher::Hash), ["3f9a2c1d"]);
        assert!(texts(Matcher::Word).contains(&"commit".to_owned()));
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
