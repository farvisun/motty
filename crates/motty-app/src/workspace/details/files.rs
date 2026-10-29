//! The Files tab: a tree of the pane's folder, a filter field over it
//! (↑/↓ pick a row, Return opens it, Esc clears) and a menu per entry.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, ClipboardItem, Context, FontWeight, MouseButton, MouseDownEvent, Pixels, Point,
    Task, Window, div, prelude::*, px,
};

use super::{
    FieldId, INSET, Icon, PanelColors, TREE_INDENT, TREE_ROW, Workspace, open_in, open_with_apps,
    tilde, tool_button,
};
use crate::native_menu::{self, Entry, Item};

/// How far a filter looks: entries visited and matches kept.
const FILTER_VISIT: usize = 40_000;
const FILTER_MATCHES: usize = 400;
/// Folders a filter doesn't go into.
const SKIPPED: [&str; 4] = [".git", "node_modules", "target", ".build"];

#[derive(Default)]
pub(in crate::workspace) struct FileTree {
    pub(super) root: Option<PathBuf>,
    expanded: HashSet<PathBuf>,
    children: HashMap<PathBuf, Vec<FileEntry>>,
    show_hidden: bool,
    /// The rows matching the filter, when there's one.
    filtered: Option<Vec<(usize, FileEntry)>>,
    /// The row ↑/↓ moved to.
    selected: Option<usize>,
    filtering: Option<Task<()>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileEntry {
    path: PathBuf,
    name: String,
    dir: bool,
}

impl FileEntry {
    fn new(path: PathBuf, dir: bool) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self { path, name, dir }
    }
}

/// Folders first, then by name.
fn order(a: &FileEntry, b: &FileEntry) -> std::cmp::Ordering {
    b.dir
        .cmp(&a.dir)
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
}

impl FileTree {
    fn entries(&mut self, dir: &Path) -> Vec<FileEntry> {
        if let Some(list) = self.children.get(dir) {
            return list.clone();
        }
        let mut list: Vec<FileEntry> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| FileEntry::new(e.path(), e.path().is_dir()))
            .collect();
        list.sort_by(order);
        self.children.insert(dir.to_path_buf(), list.clone());
        list
    }

    /// The visible rows, depth first: (depth, entry).
    fn rows(&mut self) -> Vec<(usize, FileEntry)> {
        if let Some(rows) = &self.filtered {
            return rows.clone();
        }
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut stack: Vec<(usize, FileEntry)> = self
            .entries(&root)
            .into_iter()
            .rev()
            .map(|e| (0, e))
            .collect();
        while let Some((depth, entry)) = stack.pop() {
            if !self.show_hidden && entry.name.starts_with('.') {
                continue;
            }
            if entry.dir && self.expanded.contains(&entry.path) {
                stack.extend(
                    self.entries(&entry.path)
                        .into_iter()
                        .rev()
                        .map(|e| (depth + 1, e)),
                );
            }
            out.push((depth, entry));
            if out.len() >= 2000 {
                break;
            }
        }
        out
    }

    pub(super) fn set_root(&mut self, root: Option<PathBuf>) {
        if root != self.root {
            self.root = root;
            self.expanded.clear();
            self.children.clear();
            self.filtered = None;
            self.selected = None;
        }
    }
}

/// What a file's menu does.
enum Pick {
    Open(PathBuf),
    OpenWith(String, PathBuf),
    ConfigureOpenWith,
    Reveal(PathBuf),
    Copy(PathBuf),
    Close,
}

impl Workspace {
    pub(super) fn render_files(
        &mut self,
        window: &mut Window,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = self.details.files.rows();
        let files = &self.details.files;
        let hidden = files.show_hidden;
        let filtering = files.filtered.is_some();
        let selected = files.selected;
        let tools = div()
            .px(px(INSET - 8.))
            .pt(px(6.))
            .pb(px(4.))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.secondary)
                    .children(files.root.as_deref().map(tilde)),
            )
            .child(
                tool_button("details-files-up", Icon::Up, c).on_click(cx.listener(
                    |this, _, _, cx| {
                        let parent = this
                            .details
                            .files
                            .root
                            .as_deref()
                            .and_then(Path::parent)
                            .map(Path::to_path_buf);
                        if parent.is_some() {
                            this.details.files.set_root(parent);
                            this.refilter(cx);
                            cx.notify();
                        }
                    },
                )),
            )
            .child(
                tool_button(
                    "details-files-hidden",
                    if hidden { Icon::Eye } else { Icon::EyeSlash },
                    c,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.details.files.show_hidden = !this.details.files.show_hidden;
                    this.refilter(cx);
                    cx.notify();
                })),
            );
        let filter = self.render_field(FieldId::Filter, "Filter", window, c, cx);
        let mut col = div()
            .flex()
            .flex_col()
            .child(tools)
            .child(div().px(px(INSET - 8.)).pb(px(6.)).child(filter));
        if filtering && rows.is_empty() {
            col = col.child(
                div()
                    .px(px(INSET - 8.))
                    .pt(px(4.))
                    .text_color(c.tertiary)
                    .child("No matching files"),
            );
        }
        for (ix, (depth, entry)) in rows.into_iter().enumerate() {
            let open =
                entry.dir && (filtering || self.details.files.expanded.contains(&entry.path));
            let path = entry.path.clone();
            let menu_path = entry.path.clone();
            let dir = entry.dir;
            col = col.child(
                div()
                    .id(("details-file", ix))
                    .mx(px(4.))
                    .pl(px(4. + depth as f32 * TREE_INDENT))
                    .h(px(TREE_ROW))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(6.))
                    .when(selected == Some(ix), |d| d.bg(c.selected))
                    .hover(|s| s.bg(c.hover))
                    .child(div().flex_none().w(px(14.)).when(dir, |d| {
                        d.child(if open {
                            Icon::ChevronDown.draw(c.icon)
                        } else {
                            Icon::ChevronRight.draw(c.icon)
                        })
                    }))
                    .child(if dir {
                        Icon::Folder.draw(c.icon)
                    } else {
                        Icon::File.draw(c.icon)
                    })
                    .child(
                        div()
                            .pl(px(4.))
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(c.text)
                            .child(entry.name),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.details.files.selected = Some(ix);
                        this.open_file_row(path.clone(), dir, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.details.files.selected = Some(ix);
                            this.show_file_menu(menu_path.clone(), e.position, window, cx);
                        }),
                    ),
            );
        }
        col.into_any_element()
    }

    /// A click or Return on a row: a folder opens or closes (or, in
    /// filtered rows, is shown in the tree); a file opens in an editor
    /// tab, or in its app when it isn't text.
    fn open_file_row(
        &mut self,
        path: PathBuf,
        dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let files = &mut self.details.files;
        if dir {
            if files.filtered.is_some() {
                let root = files.root.clone().unwrap_or_default();
                let mut at = path.as_path();
                while at.starts_with(&root) && at != root {
                    files.expanded.insert(at.to_path_buf());
                    at = at.parent().unwrap_or(&root);
                }
                self.details.field(FieldId::Filter).text.clear();
                self.refilter(cx);
            } else if !files.expanded.remove(&path) {
                files.expanded.insert(path);
            }
            cx.notify();
        } else {
            self.open_path(path, window, cx);
        }
    }

    /// A file in an editor tab, when it's text; its app otherwise.
    fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if path.is_dir() {
            cx.open_with_system(&path);
        } else if is_text(&path) {
            self.open_editor(Some(path), window, cx);
        } else {
            cx.open_with_system(&path);
        }
    }

    /// ↑/↓ and Return in the filter field.
    pub(super) fn move_file_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.details.files.rows().len();
        if n == 0 {
            return;
        }
        let files = &mut self.details.files;
        files.selected = Some(match (files.selected, down) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1).min(n - 1),
            (Some(i), false) => i.saturating_sub(1),
        });
        cx.notify();
    }

    pub(super) fn open_selected_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.details.files.rows();
        let pick = match self.details.files.selected {
            Some(i) => rows.get(i),
            // With a filter, Return opens the first match.
            None => rows.iter().find(|(_, e)| !e.dir).or(rows.first()),
        };
        if let Some((_, entry)) = pick.cloned() {
            self.open_file_row(entry.path, entry.dir, window, cx);
        }
    }

    /// Filters the tree by the field's text, in the background.
    pub(super) fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self
            .details
            .field(FieldId::Filter)
            .text
            .trim()
            .to_lowercase();
        let files = &mut self.details.files;
        files.selected = None;
        let Some(root) = files.root.clone().filter(|_| !query.is_empty()) else {
            files.filtered = None;
            files.filtering = None;
            cx.notify();
            return;
        };
        let hidden = files.show_hidden;
        let work = cx.background_spawn(async move {
            let found = search(&root, &query, hidden);
            tree_rows(&root, &found)
        });
        files.filtering = Some(cx.spawn(async move |this, cx| {
            let rows = work.await;
            this.update(cx, |this, cx| {
                let files = &mut this.details.files;
                files.filtered = Some(rows);
                files.filtering = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn show_file_menu(
        &mut self,
        path: PathBuf,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut entries: Vec<Entry> = vec![Item::new("Open").into()];
        let mut picks = vec![Pick::Open(path.clone())];
        let apps = super::apps_for(
            &path.to_string_lossy(),
            &open_with_apps(&self.config.open_with_apps),
        );
        if !apps.is_empty() {
            let mut with = Vec::new();
            for (name, bundle) in apps {
                with.push(Item::new(name).into());
                picks.push(Pick::OpenWith(bundle, path.clone()));
            }
            entries.push(Entry::Submenu {
                label: "Open With".into(),
                entries: with,
            });
        }
        entries.push(Item::new("Configure Open With…").into());
        picks.push(Pick::ConfigureOpenWith);
        entries.push(Entry::Separator);
        entries.push(Item::new("Reveal in Finder").into());
        picks.push(Pick::Reveal(path.clone()));
        entries.push(Item::new("Copy Path").into());
        picks.push(Pick::Copy(path));
        entries.push(Entry::Separator);
        entries.push(Item::new("Close Details Panel").into());
        picks.push(Pick::Close);
        let this = cx.entity().downgrade();
        native_menu::popup(entries, at, None, window, cx, move |ix, window, cx| {
            let Some(pick) = picks.into_iter().nth(ix) else {
                return;
            };
            let _ = this.update(cx, |this, cx| match pick {
                Pick::Open(path) => this.open_path(path, window, cx),
                Pick::OpenWith(bundle, path) => open_in(&bundle, &path),
                Pick::ConfigureOpenWith => this.configure_open_with(window, cx),
                Pick::Reveal(path) => cx.reveal_path(&path),
                Pick::Copy(path) => cx.write_to_clipboard(ClipboardItem::new_string(
                    path.to_string_lossy().into_owned(),
                )),
                Pick::Close => this.toggle_details(cx),
            });
        });
    }
}

/// No NUL bytes near the start: worth opening in the editor.
fn is_text(path: &Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 8192];
    std::fs::File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .is_ok_and(|n| !buf[..n].contains(&0))
}

/// Entries under `root` whose names contain `query` (lowercase), as
/// (path, is a folder), breadth first.
fn search(root: &Path, query: &str, hidden: bool) -> Vec<(PathBuf, bool)> {
    let mut found = Vec::new();
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    let mut visited = 0;
    while let Some(dir) = queue.pop_front() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            visited += 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                continue;
            }
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if name.to_lowercase().contains(query) {
                found.push((entry.path(), is_dir));
                if found.len() >= FILTER_MATCHES {
                    return found;
                }
            }
            if is_dir && !SKIPPED.contains(&name.as_str()) {
                queue.push_back(entry.path());
            }
        }
        if visited >= FILTER_VISIT {
            break;
        }
    }
    found
}

/// The matches as tree rows under `root`, each below its folders.
fn tree_rows(root: &Path, found: &[(PathBuf, bool)]) -> Vec<(usize, FileEntry)> {
    let mut children: HashMap<PathBuf, Vec<FileEntry>> = HashMap::new();
    let mut seen = HashSet::new();
    for (path, dir) in found {
        let mut at = path.clone();
        let mut dir = *dir;
        while at.starts_with(root) && at != root && seen.insert(at.clone()) {
            let Some(parent) = at.parent().map(Path::to_path_buf) else {
                break;
            };
            children
                .entry(parent.clone())
                .or_default()
                .push(FileEntry::new(at, dir));
            at = parent;
            dir = true;
        }
    }
    for list in children.values_mut() {
        list.sort_by(order);
    }
    let mut out = Vec::new();
    let mut stack: Vec<(usize, FileEntry)> = children
        .get(root)
        .into_iter()
        .flatten()
        .rev()
        .map(|e| (0, e.clone()))
        .collect();
    while let Some((depth, entry)) = stack.pop() {
        if let Some(kids) = children.get(&entry.path) {
            stack.extend(kids.iter().rev().map(|e| (depth + 1, e.clone())));
        }
        out.push((depth, entry));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtered_rows_keep_their_folders() {
        let root = Path::new("/r");
        let found = [
            (PathBuf::from("/r/src/main.rs"), false),
            (PathBuf::from("/r/Main.md"), false),
            (PathBuf::from("/r/src/bin/main"), true),
        ];
        let rows: Vec<(usize, String)> = tree_rows(root, &found)
            .into_iter()
            .map(|(d, e)| (d, e.name))
            .collect();
        assert_eq!(
            rows,
            [
                (0, "src".to_owned()),
                (1, "bin".to_owned()),
                (2, "main".to_owned()),
                (1, "main.rs".to_owned()),
                (0, "Main.md".to_owned()),
            ]
        );
    }
}
