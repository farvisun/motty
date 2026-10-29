//! Search in Files: ripgrep's own walker and searcher over a folder, so
//! `.gitignore`, hidden files and binaries are treated as `rg` treats
//! them.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;

/// Where a search stops collecting.
pub const MAX_MATCHES: usize = 5000;
/// How much of a long line is kept around its match.
const BEFORE: usize = 60;
const LINE_MAX: usize = 300;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// Space-separated globs: only these files.
    pub include: String,
    /// Space-separated globs: not these.
    pub exclude: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileMatch {
    pub path: PathBuf,
    /// 0-based.
    pub line: usize,
    /// The line (or the part of a long one around the match), trimmed.
    pub text: String,
    /// The match in `text`.
    pub cols: Range<usize>,
    /// The match in the whole line, for the preview.
    pub line_cols: Range<usize>,
}

/// Searches the files under `folder`, handing over each file's matches as
/// they're found. Returns whether it stopped at [`MAX_MATCHES`].
pub fn search(
    folder: &Path,
    query: &Query,
    cancel: &AtomicBool,
    mut found: impl FnMut(Vec<FileMatch>),
) -> Result<bool, String> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!query.case_sensitive)
        .word(query.whole_word)
        .fixed_strings(!query.regex)
        .line_terminator(Some(b'\n'))
        .build(&query.text)
        .map_err(|e| e.to_string())?;
    let mut overrides = OverrideBuilder::new(folder);
    for glob in query.include.split_whitespace() {
        overrides.add(glob).map_err(|e| e.to_string())?;
    }
    for glob in query.exclude.split_whitespace() {
        overrides
            .add(&format!("!{}", glob.trim_start_matches('!')))
            .map_err(|e| e.to_string())?;
    }
    let overrides = overrides.build().map_err(|e| e.to_string())?;
    let walk = WalkBuilder::new(folder)
        .overrides(overrides)
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .build();
    let mut total = 0;
    for entry in walk {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let mut matches = Vec::new();
        let result = searcher.search_path(
            &matcher,
            path,
            Lossy(|number, line| {
                let line = line.trim_end_matches(['\n', '\r']);
                let mut ranges = Vec::new();
                let _ = matcher.find_iter(line.as_bytes(), |m| {
                    ranges.push(m.start()..m.end());
                    true
                });
                for range in ranges {
                    if total >= MAX_MATCHES {
                        break;
                    }
                    let (text, cols) = excerpt(line, range.clone());
                    matches.push(FileMatch {
                        path: path.to_path_buf(),
                        line: number.saturating_sub(1) as usize,
                        text,
                        cols,
                        line_cols: range,
                    });
                    total += 1;
                }
                Ok(total < MAX_MATCHES && !cancel.load(Ordering::Relaxed))
            }),
        );
        if let Err(e) = result {
            tracing::debug!("search in {}: {e}", path.display());
        }
        if !matches.is_empty() {
            found(matches);
        }
        if total >= MAX_MATCHES {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The line without its indent, or a window of a long one around
/// `range`, and where the match is in what's kept.
fn excerpt(line: &str, range: Range<usize>) -> (String, Range<usize>) {
    let indent = line.len() - line.trim_start().len();
    let mut start = indent.min(range.start);
    if line.len() - start > LINE_MAX {
        start = start.max(range.start.saturating_sub(BEFORE));
    }
    while !line.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = line.len().min(start.max(range.end) + LINE_MAX);
    end = end.max(range.end);
    while !line.is_char_boundary(end) {
        end += 1;
    }
    let mut text = String::new();
    if start > indent {
        text.push('…');
    }
    let shift = text.len();
    text.push_str(line[start..end].trim_end());
    let cols = range.start - start + shift..range.end - start + shift;
    (text, cols)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("motty-file-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn main() {\n    let Foo = foo;\n}\n").unwrap();
        std::fs::write(dir.join("b.txt"), "food\nnothing\n").unwrap();
        std::fs::write(dir.join("c.lock"), "foo\n").unwrap();
        std::fs::write(dir.join("bin"), b"foo\0bar").unwrap();
        dir
    }

    fn run(dir: &Path, query: Query) -> Vec<(String, usize, String)> {
        let mut out = Vec::new();
        search(dir, &query, &AtomicBool::new(false), |m| {
            for m in m {
                let name = m.path.strip_prefix(dir).unwrap().display().to_string();
                out.push((name, m.line, m.text[m.cols].to_owned()));
            }
        })
        .unwrap();
        out
    }

    #[test]
    fn finds_and_narrows() {
        let dir = folder();
        let q = |text: &str| Query {
            text: text.into(),
            ..Query::default()
        };
        // Case-insensitive and literal by default; binaries skipped.
        let all = run(&dir, q("foo"));
        assert_eq!(all.len(), 4, "{all:?}");
        assert!(all.iter().all(|(name, ..)| name != "bin"));
        assert!(all.contains(&("src/a.rs".into(), 1, "Foo".into())));
        let case = run(
            &dir,
            Query {
                case_sensitive: true,
                ..q("Foo")
            },
        );
        assert_eq!(case, vec![("src/a.rs".into(), 1, "Foo".into())]);
        let word = run(
            &dir,
            Query {
                whole_word: true,
                exclude: "*.lock".into(),
                ..q("foo")
            },
        );
        assert_eq!(word.len(), 2, "{word:?}");
        let only = run(
            &dir,
            Query {
                include: "*.txt".into(),
                ..q("foo")
            },
        );
        assert_eq!(only, vec![("b.txt".into(), 0, "foo".into())]);
        // `.` is literal unless it's a regex.
        assert!(run(&dir, q("f.o")).is_empty());
        let regex = run(
            &dir,
            Query {
                regex: true,
                ..q("f.od")
            },
        );
        assert_eq!(regex, vec![("b.txt".into(), 0, "food".into())]);
        assert!(
            search(
                &dir,
                &Query {
                    regex: true,
                    ..q("(")
                },
                &AtomicBool::new(false),
                |_| {}
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn excerpts() {
        assert_eq!(excerpt("    let x = 1;", 8..9), ("let x = 1;".into(), 4..5));
        let long = format!("{}needle{}", "a".repeat(500), "b".repeat(500));
        let (text, cols) = excerpt(&long, 500..506);
        assert_eq!(&text[cols], "needle");
        assert!(text.starts_with('…') && text.len() < 400);
    }
}
