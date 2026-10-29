//! The Outline tab: a map of the focused pane. A terminal lists its
//! commands, and an agent's prompts; a file pane its headings (Markdown,
//! HTML), symbols (code), top-level keys (JSON, YAML, TOML) or the files
//! a diff changes. Each entry has an icon for its kind in the gutter.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui::{AnyElement, Context, Entity, Focusable, FontWeight, Task, div, prelude::*, px};

use super::{INSET, LIST_ROW, PanelColors, ROW_HEIGHT, Workspace, lucide, short_duration, tilde};
use crate::agents::sessions;
use crate::editor::FileEditor;
use crate::terminal_view::TerminalView;

/// How often an agent's prompts are looked for again.
const PROMPTS_INTERVAL: Duration = Duration::from_secs(5);
/// Files bigger than this aren't outlined.
const MAX_FILE: u64 = 4 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Command,
    Prompt,
    Heading,
    Function,
    Type,
    Key,
    File,
}

impl Kind {
    fn icon(self) -> &'static str {
        match self {
            Kind::Command => "square-chevron-right",
            Kind::Prompt => "message-square",
            Kind::Heading => "hash",
            Kind::Function => "square-function",
            Kind::Type => "box",
            Kind::Key => "key-round",
            Kind::File => "file-diff",
        }
    }
}

/// One line of a file's outline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub kind: Kind,
    pub text: String,
    /// Nesting, from 0.
    pub level: u8,
    /// 0-based.
    pub line: usize,
}

#[derive(Default)]
pub(in crate::workspace) struct OutlineState {
    /// The focused file's outline, for the version on disk.
    file: Option<(PathBuf, Option<SystemTime>, Vec<Entry>)>,
    /// The agent session's prompts: (session, its file's mtime, prompts).
    prompts: Option<(String, Option<SystemTime>, Vec<String>)>,
    prompts_at: Option<Instant>,
    pending: Option<Task<()>>,
}

impl Workspace {
    /// Reads the focused file's outline when it changed on disk, and an
    /// agent pane's prompts every few seconds.
    pub(super) fn refresh_outline(&mut self, now: bool, cx: &mut Context<Self>) {
        if self.details.outline.pending.is_some() {
            return;
        }
        if let Some(editor) = self.focused_editor() {
            let Some(path) = editor.read(cx).path().map(Path::to_path_buf) else {
                return;
            };
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            let fresh = self
                .details
                .outline
                .file
                .as_ref()
                .is_some_and(|(p, m, _)| *p == path && *m == modified);
            if fresh && !now {
                return;
            }
            let work = cx.background_spawn({
                let path = path.clone();
                async move {
                    let big = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE);
                    let source = if big {
                        String::new()
                    } else {
                        std::fs::read_to_string(&path).unwrap_or_default()
                    };
                    file_outline(&path, &source)
                }
            });
            self.details.outline.pending = Some(cx.spawn(async move |this, cx| {
                let entries = work.await;
                this.update(cx, |this, cx| {
                    let o = &mut this.details.outline;
                    o.pending = None;
                    o.file = Some((path, modified, entries));
                    cx.notify();
                })
                .ok();
            }));
            return;
        }
        let Some(view) = self.focused_pane() else {
            return;
        };
        let Some(session) = view
            .read(cx)
            .status
            .as_ref()
            .filter(|s| s.agent.is_some())
            .and_then(|s| s.session.clone())
        else {
            return;
        };
        let o = &mut self.details.outline;
        let other = o.prompts.as_ref().is_none_or(|(s, _, _)| *s != session);
        let due = o.prompts_at.is_none_or(|t| t.elapsed() >= PROMPTS_INTERVAL);
        if !(now || other || due) {
            return;
        }
        o.prompts_at = Some(Instant::now());
        let seen = o
            .prompts
            .as_ref()
            .filter(|(s, _, _)| *s == session)
            .map(|(_, m, _)| *m);
        let work = cx.background_spawn(async move { agent_prompts(&session, seen) });
        o.pending = Some(cx.spawn(async move |this, cx| {
            let found = work.await;
            this.update(cx, |this, cx| {
                let o = &mut this.details.outline;
                o.pending = None;
                if let Some(found) = found {
                    o.prompts = Some(found);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// The focused pane, when it's a file.
    pub(super) fn focused_editor(&self) -> Option<Entity<FileEditor>> {
        self.active_panes()?.focused.file().cloned()
    }

    pub(super) fn render_outline(
        &self,
        view: Option<&Entity<TerminalView>>,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut col = div().flex().flex_col().pt(px(8.));
        if let Some(editor) = self.focused_editor() {
            return self.render_file_outline(editor, c, cx);
        }
        let Some(view) = view else {
            return col.into_any_element();
        };
        let (blocks, commands) = {
            let session = view.read(cx).session();
            let blocks: Vec<_> = session
                .shell()
                .blocks
                .iter()
                .rev()
                .take(300)
                .cloned()
                .collect();
            let commands: Vec<Option<String>> =
                blocks.iter().map(|b| session.block_command(b)).collect();
            (blocks, commands)
        };
        if let Some(cwd) = &self.details.cwd {
            col = col.child(
                div()
                    .px(px(INSET - 8.))
                    .h(px(LIST_ROW))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.secondary)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(format!("{}/", tilde(cwd))),
                    ),
            );
        }
        let mut any = false;
        for (ix, (block, command)) in blocks.iter().zip(commands).enumerate().rev() {
            let Some(command) = command else { continue };
            any = true;
            let ago = block
                .started
                .map(|s| format!("{} ago", short_duration(s.elapsed())))
                .unwrap_or_default();
            let failed = block.exit.is_some_and(|e| e != 0);
            let row = block.prompt_row;
            let target = view.clone();
            col = col.child(
                outline_row(("details-outline", ix), Kind::Command, 0, c)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(if failed { c.danger } else { c.text })
                            .child(command.lines().next().unwrap_or_default().to_owned()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(c.tertiary)
                            .child(ago),
                    )
                    .on_click(move |_, window, cx| {
                        target.update(cx, |v, cx| v.reveal_row(row, cx));
                        window.focus(&target.focus_handle(cx), cx);
                    }),
            );
        }
        let agent = view
            .read(cx)
            .status
            .as_ref()
            .and_then(|s| Some((s.agent.clone()?, s.session.clone()?)));
        let prompts = self
            .details
            .outline
            .prompts
            .as_ref()
            .filter(|(s, _, _)| agent.as_ref().is_some_and(|(_, a)| a == s));
        if let (Some((name, _)), Some((_, _, prompts))) = (&agent, prompts) {
            col = col.child(
                div()
                    .px(px(INSET - 8.))
                    .pt(px(if any { 12. } else { 4. }))
                    .h(px(LIST_ROW + 8.))
                    .flex()
                    .items_end()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.secondary)
                    .child(format!("{name} Prompts")),
            );
            if prompts.is_empty() {
                col = col.child(
                    div()
                        .px(px(INSET - 8.))
                        .pt(px(4.))
                        .text_color(c.tertiary)
                        .child("No prompts yet"),
                );
            }
            for (ix, prompt) in prompts.iter().enumerate() {
                let target = view.clone();
                any = true;
                col = col.child(
                    outline_row(("details-prompt", ix), Kind::Prompt, 0, c)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_color(c.text)
                                .child(prompt.clone()),
                        )
                        .on_click(move |_, window, cx| {
                            window.focus(&target.focus_handle(cx), cx);
                        }),
                );
            }
        }
        if !any {
            col = col.child(
                div()
                    .px(px(INSET - 8.))
                    .pt(px(6.))
                    .text_color(c.tertiary)
                    .child("No commands yet"),
            );
        }
        col.into_any_element()
    }

    fn render_file_outline(
        &self,
        editor: Entity<FileEditor>,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut col = div().flex().flex_col().pt(px(8.));
        let path = editor.read(cx).path().map(Path::to_path_buf);
        let entries = self
            .details
            .outline
            .file
            .as_ref()
            .filter(|(p, _, _)| Some(p) == path.as_ref())
            .map(|(_, _, e)| e.clone());
        if let Some(path) = &path {
            col = col.child(
                div()
                    .px(px(INSET - 8.))
                    .h(px(LIST_ROW))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.secondary)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    ),
            );
        }
        let Some(entries) = entries else {
            let msg = if path.is_some() {
                "Loading…"
            } else {
                "Not saved yet"
            };
            return col
                .child(
                    div()
                        .px(px(INSET - 8.))
                        .pt(px(6.))
                        .text_color(c.tertiary)
                        .child(msg),
                )
                .into_any_element();
        };
        if entries.is_empty() {
            col = col.child(
                div()
                    .px(px(INSET - 8.))
                    .pt(px(6.))
                    .text_color(c.tertiary)
                    .child("No outline for this file"),
            );
        }
        let base = entries.iter().map(|e| e.level).min().unwrap_or(0);
        for (ix, entry) in entries.into_iter().enumerate() {
            let target = editor.clone();
            let line = entry.line;
            col = col.child(
                outline_row(("details-symbol", ix), entry.kind, entry.level - base, c)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(c.text)
                            .when(entry.kind == Kind::Heading && entry.level == base, |d| {
                                d.font_weight(FontWeight::MEDIUM)
                            })
                            .child(entry.text),
                    )
                    .on_click(move |_, window, cx| {
                        target.update(cx, |e, cx| e.show_match(line, 0..0, cx));
                        window.focus(&target.focus_handle(cx), cx);
                    }),
            );
        }
        col.into_any_element()
    }
}

/// A row with its kind's icon in the gutter, indented by `level`.
fn outline_row(
    id: impl Into<gpui::ElementId>,
    kind: Kind,
    level: u8,
    c: &PanelColors,
) -> gpui::Stateful<gpui::Div> {
    let icon = match kind {
        Kind::Function => c.accent,
        Kind::Type => c.warning,
        _ => c.icon,
    };
    div()
        .id(id)
        .mx(px(4.))
        .pl(px(INSET - 12. + f32::from(level.min(6)) * 12.))
        .pr(px(INSET - 12.))
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(6.))
        .hover(|s| s.bg(c.hover))
        .child(lucide(kind.icon(), 13., icon))
}

/// The user's prompts in an agent's session, unless its file is still
/// as `seen`; with the session id and the file's mtime. A session that
/// isn't found has none, so it isn't looked for again until it's due.
fn agent_prompts(
    session: &str,
    seen: Option<Option<SystemTime>>,
) -> Option<(String, Option<SystemTime>, Vec<String>)> {
    // Matched by id alone: Grok writes Claude Code's sessions.
    let Some(found) = sessions::list(&sessions::Roots::current(), 40)
        .into_iter()
        .find(|s| s.id == session || s.path.to_string_lossy() == session)
    else {
        return Some((session.to_owned(), None, Vec::new()));
    };
    let modified = std::fs::metadata(&found.path)
        .and_then(|m| m.modified())
        .ok();
    if seen == Some(modified) {
        return None;
    }
    let prompts = sessions::transcript(&found)
        .into_iter()
        .filter(|m| m.role == sessions::Role::User)
        .filter_map(|m| {
            let line = m.text.lines().find(|l| !l.trim().is_empty())?.trim();
            Some(line.chars().take(200).collect())
        })
        .collect();
    Some((session.to_owned(), modified, prompts))
}

/// A file's outline, by its kind.
pub(super) fn file_outline(path: &Path, source: &str) -> Vec<Entry> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let headings = |list: Vec<(u8, String, usize)>| {
        list.into_iter()
            .map(|(level, text, line)| Entry {
                kind: Kind::Heading,
                text,
                level: level.saturating_sub(1),
                line,
            })
            .collect()
    };
    match ext.as_str() {
        "md" | "markdown" | "mdx" => headings(crate::preview::markdown_headings(source)),
        "html" | "htm" | "xhtml" => headings(crate::preview::html_headings(source)),
        "json" | "yaml" | "yml" | "toml" => top_keys(source, &ext),
        "diff" | "patch" => diff_files(source),
        _ => symbols(source),
    }
}

/// The keys at the top level of a JSON, YAML or TOML file (TOML tables
/// too).
fn top_keys(source: &str, ext: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let key = |line: usize, text: &str| Entry {
        kind: Kind::Key,
        text: text.trim().trim_matches('"').trim_matches('\'').to_owned(),
        level: 0,
        line,
    };
    match ext {
        "json" => {
            // Keys at depth one: track brackets outside strings.
            let mut depth = 0i32;
            for (ix, line) in source.lines().enumerate() {
                let mut chars = line.char_indices().peekable();
                let mut in_str = false;
                let mut start = 0;
                while let Some((i, ch)) = chars.next() {
                    match ch {
                        '\\' if in_str => {
                            chars.next();
                        }
                        '"' if !in_str => {
                            in_str = true;
                            start = i + 1;
                        }
                        '"' => {
                            in_str = false;
                            let rest = line[i + 1..].trim_start();
                            if depth == 1 && rest.starts_with(':') {
                                out.push(key(ix, &line[start..i]));
                            }
                        }
                        '{' | '[' if !in_str => depth += 1,
                        '}' | ']' if !in_str => depth -= 1,
                        _ => {}
                    }
                }
            }
        }
        "toml" => {
            for (ix, line) in source.lines().enumerate() {
                let t = line.trim();
                if t.starts_with('[') {
                    let name = t.trim_start_matches('[').split(']').next().unwrap_or("");
                    out.push(Entry {
                        kind: Kind::Type,
                        ..key(ix, name)
                    });
                } else if !line.starts_with([' ', '\t', '#'])
                    && let Some((k, _)) = line.split_once('=')
                    && !out.iter().any(|e| e.kind == Kind::Type)
                {
                    out.push(key(ix, k));
                }
            }
        }
        _ => {
            for (ix, line) in source.lines().enumerate() {
                if line.starts_with([' ', '\t', '#', '-']) || line.trim().is_empty() {
                    continue;
                }
                if let Some((k, _)) = line.split_once(':') {
                    out.push(key(ix, k));
                }
            }
        }
    }
    out
}

/// The files a unified diff changes.
fn diff_files(source: &str) -> Vec<Entry> {
    source
        .lines()
        .enumerate()
        .filter_map(|(ix, line)| {
            let path = line.strip_prefix("+++ ")?.trim();
            let path = path.strip_prefix("b/").unwrap_or(path);
            (path != "/dev/null").then(|| Entry {
                kind: Kind::File,
                text: path.to_owned(),
                level: 0,
                line: ix,
            })
        })
        .collect()
}

/// Functions and types declared at the start of a line, in the common
/// languages' words for them; nested ones (by indent) a level down.
fn symbols(source: &str) -> Vec<Entry> {
    const FUNCTIONS: [&str; 5] = ["fn ", "def ", "func ", "function ", "fun "];
    const TYPES: [&str; 10] = [
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "impl<",
        "class ",
        "interface ",
        "protocol ",
        "extension ",
        "type ",
    ];
    const MODIFIERS: [&str; 16] = [
        "pub(crate) ",
        "pub(super) ",
        "pub ",
        "export default ",
        "export ",
        "async ",
        "unsafe ",
        "const ",
        "static ",
        "private ",
        "public ",
        "internal ",
        "fileprivate ",
        "final ",
        "abstract ",
        "override ",
    ];
    let mut out = Vec::new();
    for (ix, line) in source.lines().enumerate() {
        let body = line.trim_start();
        let indent = line.len() - body.len();
        if indent > 8 {
            continue;
        }
        let mut rest = body;
        while let Some(m) = MODIFIERS.iter().find(|m| rest.starts_with(**m)) {
            rest = &rest[m.len()..];
        }
        let (kind, word) = if let Some(w) = FUNCTIONS.iter().find(|w| rest.starts_with(**w)) {
            (Kind::Function, *w)
        } else if let Some(w) = TYPES.iter().find(|w| rest.starts_with(**w)) {
            (Kind::Type, *w)
        } else {
            continue;
        };
        let after = &rest[word.len()..];
        // `impl<T> Foo<T>`: the type, without the parameters.
        let after = match word {
            "impl<" => after.split_once('>').map_or(after, |(_, r)| r),
            _ => after,
        };
        let name: String = if kind == Kind::Type && word.starts_with("impl") {
            // `impl Foo for Bar`, up to its body.
            format!(
                "impl {}",
                after.split(['{', ';']).next().unwrap_or("").trim()
            )
        } else {
            after
                .trim_start()
                .chars()
                .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '$'))
                .collect()
        };
        if name.is_empty() || name == "impl " {
            continue;
        }
        out.push(Entry {
            kind,
            text: name,
            level: u8::from(indent > 0),
            line: ix,
        });
        if out.len() >= 1000 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(entries: &[Entry]) -> Vec<(Kind, &str, u8)> {
        entries
            .iter()
            .map(|e| (e.kind, e.text.as_str(), e.level))
            .collect()
    }

    #[test]
    fn code_symbols() {
        let src = "use x;\npub struct Foo {\n}\nimpl Foo {\n    pub fn new() -> Self {\n        fn inner() {}\n    }\n}\nasync function load(a) {}\nclass Bar:\n    def go(self):\n";
        assert_eq!(
            texts(&symbols(src)),
            [
                (Kind::Type, "Foo", 0),
                (Kind::Type, "impl Foo", 0),
                (Kind::Function, "new", 1),
                (Kind::Function, "inner", 1),
                (Kind::Function, "load", 0),
                (Kind::Type, "Bar", 0),
                (Kind::Function, "go", 1),
            ]
        );
    }

    #[test]
    fn keys_and_diffs() {
        let json = "{\n  \"name\": \"x\",\n  \"deps\": {\"a\": 1},\n  \"list\": [\"b\"]\n}";
        let keys: Vec<String> = top_keys(json, "json").into_iter().map(|e| e.text).collect();
        assert_eq!(keys, ["name", "deps", "list"]);
        let yaml = "# c\nname: x\nsteps:\n  - run: y\nenv:\n  A: 1\n";
        let keys: Vec<String> = top_keys(yaml, "yaml").into_iter().map(|e| e.text).collect();
        assert_eq!(keys, ["name", "steps", "env"]);
        let toml = "title = \"t\"\n[package]\nname = \"p\"\n[[bin]]\n";
        assert_eq!(
            texts(&top_keys(toml, "toml")),
            [
                (Kind::Key, "title", 0),
                (Kind::Type, "package", 0),
                (Kind::Type, "bin", 0)
            ]
        );
        let diff = "diff --git a/x b/x\n--- a/x\n+++ b/src/x.rs\n@@\n+++ /dev/null\n";
        assert_eq!(texts(&diff_files(diff)), [(Kind::File, "src/x.rs", 0)]);
    }

    #[test]
    fn markdown_outline_by_extension() {
        let md = "# Title\n\ntext\n\n## Part\n";
        let entries = file_outline(Path::new("README.md"), md);
        assert_eq!(
            texts(&entries),
            [(Kind::Heading, "Title", 0), (Kind::Heading, "Part", 1)]
        );
        assert_eq!(entries[1].line, 4);
    }
}
