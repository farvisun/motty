//! The folder pane's tree, without drawing: folders' listings, which are
//! expanded, and the rows that makes once hidden files and the Find text
//! are applied.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One file or folder in a listing.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub dir: bool,
}

impl Entry {
    /// A dot-file, shown only with the eye on.
    pub fn hidden(&self) -> bool {
        self.name.starts_with('.')
    }
}

/// A folder as last read: its entries (or why it couldn't be), and its
/// modification time then, to notice changes.
#[derive(Clone, Debug)]
pub struct Listing {
    pub entries: Result<Vec<Entry>, String>,
    pub mtime: Option<SystemTime>,
}

impl Listing {
    pub fn read(dir: &Path) -> Self {
        let mtime = modified(dir);
        let entries = std::fs::read_dir(dir)
            .map(|list| {
                let mut entries: Vec<Entry> = list
                    .flatten()
                    .map(|e| Entry {
                        name: e.file_name().to_string_lossy().into_owned(),
                        // Follows links, so a link to a folder opens like one.
                        dir: e.path().is_dir(),
                        path: e.path(),
                    })
                    .collect();
                sort(&mut entries);
                entries
            })
            .map_err(|e| describe(&e));
        Self { entries, mtime }
    }

    /// Whether the folder changed on disk since it was read.
    pub fn stale(&self, dir: &Path) -> bool {
        modified(dir) != self.mtime
    }
}

/// Why a folder can't be listed, as the pane says it.
fn describe(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            "You don't have permission to see this folder's contents.".into()
        }
        std::io::ErrorKind::NotFound => "This folder doesn't exist.".into(),
        std::io::ErrorKind::NotADirectory => "This isn't a folder.".into(),
        _ => e.to_string(),
    }
}

fn modified(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}

/// Folders first, then by name, ignoring case.
pub fn sort(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Whether `name` has `query` in it, ignoring case; an empty query matches.
pub fn matches(name: &str, query: &str) -> bool {
    query.is_empty() || name.to_lowercase().contains(&query.to_lowercase())
}

/// A row on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub depth: usize,
    pub entry: Entry,
    /// An expanded folder.
    pub open: bool,
}

/// The folders under `root` that should be read: the root and every
/// expanded folder whose parent is shown.
pub fn wanted(
    root: &Path,
    lists: &HashMap<PathBuf, Listing>,
    expanded: &HashSet<PathBuf>,
    hidden: bool,
) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let mut ix = 0;
    while ix < out.len() {
        if let Some(Ok(entries)) = lists.get(&out[ix]).map(|l| &l.entries) {
            let open = entries
                .iter()
                .filter(|e| e.dir && (hidden || !e.hidden()) && expanded.contains(&e.path))
                .map(|e| e.path.clone());
            out.extend(open);
        }
        ix += 1;
    }
    out
}

/// The rows under `root`, depth first. With a `query`, only entries whose
/// names match it are kept, with the folders above them; folders that
/// haven't been expanded aren't searched.
pub fn flatten(
    root: &Path,
    lists: &HashMap<PathBuf, Listing>,
    expanded: &HashSet<PathBuf>,
    hidden: bool,
    query: &str,
) -> Vec<Row> {
    fn walk(
        dir: &Path,
        depth: usize,
        lists: &HashMap<PathBuf, Listing>,
        expanded: &HashSet<PathBuf>,
        hidden: bool,
        query: &str,
        out: &mut Vec<Row>,
    ) -> bool {
        let Some(Ok(entries)) = lists.get(dir).map(|l| &l.entries) else {
            return false;
        };
        let mut any = false;
        for entry in entries.iter().filter(|e| hidden || !e.hidden()) {
            let open = entry.dir && expanded.contains(&entry.path);
            let mut below = Vec::new();
            let found = open
                && walk(
                    &entry.path,
                    depth + 1,
                    lists,
                    expanded,
                    hidden,
                    query,
                    &mut below,
                );
            if found || matches(&entry.name, query) {
                out.push(Row {
                    depth,
                    entry: entry.clone(),
                    open,
                });
                out.append(&mut below);
                any = true;
            }
        }
        any
    }
    let mut out = Vec::new();
    walk(root, 0, lists, expanded, hidden, query, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, dir: bool) -> Entry {
        let path = PathBuf::from(path);
        Entry {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            dir,
        }
    }

    fn listing(entries: Vec<Entry>) -> Listing {
        Listing {
            entries: Ok(entries),
            mtime: None,
        }
    }

    fn names(rows: &[Row]) -> Vec<(usize, &str)> {
        rows.iter()
            .map(|r| (r.depth, r.entry.name.as_str()))
            .collect()
    }

    fn lists() -> HashMap<PathBuf, Listing> {
        HashMap::from([
            (
                PathBuf::from("/r"),
                listing(vec![
                    entry("/r/docs", true),
                    entry("/r/src", true),
                    entry("/r/.git", true),
                    entry("/r/README.md", false),
                ]),
            ),
            (
                PathBuf::from("/r/src"),
                listing(vec![
                    entry("/r/src/main.rs", false),
                    entry("/r/src/lib.rs", false),
                ]),
            ),
            (
                PathBuf::from("/r/docs"),
                listing(vec![entry("/r/docs/guide.md", false)]),
            ),
        ])
    }

    #[test]
    fn sorts_folders_first_ignoring_case() {
        let mut v = vec![
            entry("/b.txt", false),
            entry("/Zed", true),
            entry("/a.txt", false),
            entry("/apps", true),
            entry("/C.txt", false),
        ];
        sort(&mut v);
        let order: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(order, ["apps", "Zed", "a.txt", "b.txt", "C.txt"]);
    }

    #[test]
    fn matches_ignoring_case() {
        assert!(matches("README.md", ""));
        assert!(matches("README.md", "readme"));
        assert!(matches("main.rs", "N.R"));
        assert!(!matches("main.rs", "lib"));
    }

    #[test]
    fn flattens_expanded_folders_depth_first() {
        let expanded = HashSet::from([PathBuf::from("/r/src")]);
        let rows = flatten(Path::new("/r"), &lists(), &expanded, false, "");
        assert_eq!(
            names(&rows),
            [
                (0, "docs"),
                (0, "src"),
                (1, "main.rs"),
                (1, "lib.rs"),
                (0, "README.md")
            ]
        );
        assert!(!rows[0].open);
        assert!(rows[1].open);
    }

    #[test]
    fn hides_dot_files_unless_asked() {
        let rows = flatten(Path::new("/r"), &lists(), &HashSet::new(), true, "");
        assert!(rows.iter().any(|r| r.entry.name == ".git"));
        let rows = flatten(Path::new("/r"), &lists(), &HashSet::new(), false, "");
        assert!(!rows.iter().any(|r| r.entry.name == ".git"));
    }

    #[test]
    fn filters_keeping_parents_of_matches() {
        let expanded = HashSet::from([PathBuf::from("/r/src"), PathBuf::from("/r/docs")]);
        let rows = flatten(Path::new("/r"), &lists(), &expanded, false, "MAIN");
        assert_eq!(names(&rows), [(0, "src"), (1, "main.rs")]);
        // A matching folder keeps its own place even with nothing below.
        let rows = flatten(Path::new("/r"), &lists(), &expanded, false, "doc");
        assert_eq!(names(&rows), [(0, "docs")]);
        // Collapsed folders aren't searched.
        let rows = flatten(Path::new("/r"), &lists(), &HashSet::new(), false, "main");
        assert!(rows.is_empty());
    }

    #[test]
    fn wants_root_and_shown_expanded_folders() {
        let expanded = HashSet::from([
            PathBuf::from("/r/src"),
            PathBuf::from("/r/.git"),
            PathBuf::from("/elsewhere"),
        ]);
        let want = wanted(Path::new("/r"), &lists(), &expanded, false);
        assert_eq!(want, [PathBuf::from("/r"), PathBuf::from("/r/src")]);
        let want = wanted(Path::new("/r"), &lists(), &expanded, true);
        assert!(want.contains(&PathBuf::from("/r/.git")));
    }

    #[test]
    fn reads_a_real_folder() {
        let dir = std::env::temp_dir().join(format!("motty-folder-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("b.txt"), "").unwrap();
        std::fs::write(dir.join("A.txt"), "").unwrap();
        let listing = Listing::read(&dir);
        let names: Vec<String> = listing
            .entries
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["sub", "A.txt", "b.txt"]);
        assert!(Listing::read(&dir.join("missing")).entries.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
