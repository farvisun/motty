//! The Git tab: the repository's branch and changes, a toolbar (Stage
//! All, Commit…), hover actions per file, an inline diff and the
//! submodules. Git runs in the background.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, Context, FontWeight, PromptLevel, SharedString, Window, div, prelude::*, px,
};

use super::{
    FieldId, INSET, LIST_ROW, PanelColors, Workspace, header_text, left_ellipsis, lucide,
    tool_button_svg,
};

/// What `git status` says about a repository.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::workspace) struct GitSummary {
    pub(super) branch: String,
    added: u32,
    removed: u32,
    remote: Option<String>,
    staged: Vec<Change>,
    unstaged: Vec<Change>,
    untracked: Vec<String>,
    submodules: Vec<Submodule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Change {
    status: char,
    path: String,
}

/// A line of `git submodule status`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Submodule {
    path: String,
    /// `-` not initialized, `+` checked out at another commit, `U`
    /// conflicted, ` ` up to date.
    state: char,
    commit: String,
}

/// Which list a changed file is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Staged,
    Unstaged,
    Untracked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiffLine {
    Hunk,
    Added,
    Removed,
    Context,
}

/// The Git tab's own state, besides the summary.
#[derive(Default)]
pub(in crate::workspace) struct GitUi {
    /// The file whose diff shows under its row.
    diff: Option<OpenDiff>,
    /// The row under the pointer, which shows its actions.
    hovered: Option<(Section, String)>,
    /// The commit message field is open.
    pub(super) committing: bool,
    /// What the last git command said when it failed.
    error: Option<String>,
    busy: bool,
}

struct OpenDiff {
    section: Section,
    path: String,
    lines: Vec<(DiffLine, String)>,
}

impl Workspace {
    pub(super) fn render_git(
        &mut self,
        window: &mut Window,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut col = div().flex().flex_col().pt(px(12.));
        let Some(git) = self.details.git.clone() else {
            let msg = if self.details.git_at.is_some() || self.details.cwd.is_none() {
                "Not a git repository"
            } else {
                "Loading…"
            };
            return col
                .child(div().px(px(INSET)).text_color(c.tertiary).child(msg))
                .into_any_element();
        };
        col = col.child(
            div()
                .px(px(INSET))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c.text)
                        .child(git.branch.clone()),
                )
                .when(git.added + git.removed > 0, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .text_color(c.secondary)
                            .child(format!("+{} −{}", git.added, git.removed)),
                    )
                }),
        );
        if let Some(remote) = &git.remote {
            let url = web_url(remote);
            col = col.child(
                div()
                    .id("details-git-remote")
                    .px(px(INSET))
                    .pt(px(2.))
                    .text_size(px(12.))
                    .text_color(c.accent)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .child(url.clone())
                    .when(url.starts_with("http"), |d| {
                        d.on_click(move |_, _, cx| cx.open_url(&url))
                    }),
            );
        }
        col = col.child(self.render_git_toolbar(&git, window, c, cx));
        if let Some(error) = &self.details.git_ui.error {
            col = col.child(
                div()
                    .px(px(INSET))
                    .pb(px(8.))
                    .text_size(px(12.))
                    .text_color(c.danger)
                    .child(error.clone()),
            );
        }
        let sections: [(&str, Section, Vec<Change>); 3] = [
            ("Staged", Section::Staged, git.staged.clone()),
            ("Unstaged", Section::Unstaged, git.unstaged.clone()),
            (
                "Untracked",
                Section::Untracked,
                git.untracked
                    .iter()
                    .map(|p| Change {
                        status: '?',
                        path: p.clone(),
                    })
                    .collect(),
            ),
        ];
        if sections.iter().all(|(_, _, l)| l.is_empty()) {
            col = col.child(
                div()
                    .px(px(INSET))
                    .pt(px(4.))
                    .text_color(c.tertiary)
                    .child("No changes"),
            );
        }
        let mut ix = 0usize;
        for (name, section, list) in sections {
            if list.is_empty() {
                continue;
            }
            col = col.child(header_text(format!("{name} ({})", list.len()), c));
            for change in list {
                ix += 1;
                let path = change.path.clone();
                col = col.child(self.render_change(ix, section, change, c, cx));
                let open = self
                    .details
                    .git_ui
                    .diff
                    .as_ref()
                    .filter(|d| d.section == section && d.path == path);
                if let Some(diff) = open {
                    col = col.child(render_diff(&diff.lines, self.mono_font.clone(), c));
                }
            }
            col = col.child(div().h(px(10.)));
        }
        if !git.submodules.is_empty() {
            col = col.child(header_text(
                format!("Submodules ({})", git.submodules.len()),
                c,
            ));
            for sub in &git.submodules {
                let (note, color) = match sub.state {
                    '-' => ("not initialized", c.tertiary),
                    '+' => ("other commit", c.warning),
                    'U' => ("conflict", c.danger),
                    _ => ("", c.tertiary),
                };
                col = col.child(
                    div()
                        .px(px(INSET))
                        .h(px(LIST_ROW))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(lucide("git-branch", 13., c.icon))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(c.text)
                                .child(left_ellipsis(&sub.path, 30)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.5))
                                .text_color(color)
                                .child(if note.is_empty() {
                                    sub.commit.chars().take(7).collect()
                                } else {
                                    note.to_owned()
                                }),
                        ),
                );
            }
        }
        col.into_any_element()
    }

    /// Stage All and Commit…, or the commit message being typed.
    fn render_git_toolbar(
        &mut self,
        git: &GitSummary,
        window: &mut Window,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = &self.details.git_ui;
        let changes = !git.unstaged.is_empty() || !git.untracked.is_empty();
        let staged = !git.staged.is_empty();
        let rule = div().mx(px(INSET)).h(px(1.)).bg(c.hover);
        if ui.committing {
            let field = self.render_field(FieldId::Commit, "Commit message", window, c, cx);
            return div()
                .flex()
                .flex_col()
                .pt(px(10.))
                .pb(px(8.))
                .gap(px(6.))
                .child(div().px(px(INSET - 8.)).child(field))
                .child(
                    div()
                        .px(px(INSET))
                        .text_size(px(11.5))
                        .text_color(c.tertiary)
                        .child("Return commits the staged changes, Esc cancels"),
                )
                .child(rule)
                .into_any_element();
        }
        let busy = ui.busy;
        let button = |id: &'static str, icon: &'static str, label: &'static str, on: bool| {
            let color = if on { c.accent } else { c.tertiary };
            div()
                .id(id)
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(6.))
                .text_size(px(12.5))
                .text_color(color)
                .when(on, |d| d.hover(|s| s.bg(c.hover)))
                .child(lucide(icon, 13., color))
                .child(label)
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(INSET - 8.))
                    .pt(px(10.))
                    .pb(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        button(
                            "details-git-stage-all",
                            "plus",
                            "Stage All",
                            changes && !busy,
                        )
                        .when(changes && !busy, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| {
                                this.run_git(vec!["add".into(), "-A".into()], cx)
                            }))
                        }),
                    )
                    .child(
                        button(
                            "details-git-commit",
                            "git-commit-horizontal",
                            "Commit…",
                            staged && !busy,
                        )
                        .when(staged && !busy, |d| {
                            d.on_click(cx.listener(|this, _, window, cx| {
                                this.details.git_ui.committing = true;
                                this.focus_field(FieldId::Commit, window, cx);
                            }))
                        }),
                    ),
            )
            .child(rule)
            .child(div().h(px(8.)))
            .into_any_element()
    }

    fn render_change(
        &mut self,
        ix: usize,
        section: Section,
        change: Change,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let color = match change.status {
            'A' | '?' => c.success,
            'D' => c.danger,
            _ => c.warning,
        };
        let key = (section, change.path.clone());
        let hovered = self.details.git_ui.hovered.as_ref() == Some(&key);
        let open = self
            .details
            .git_ui
            .diff
            .as_ref()
            .is_some_and(|d| d.section == section && d.path == change.path);
        let mut actions = div().flex_none().flex().items_center().gap(px(2.));
        if hovered && !self.details.git_ui.busy {
            let path = change.path.clone();
            let file = self.details.cwd.as_ref().map(|r| r.join(&change.path));
            if let Some(file) = file.filter(|f| f.is_file()) {
                actions = actions.child(
                    tool_button_svg(("details-git-open", ix), "file", "Open", c).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.open_editor(Some(file.clone()), window, cx)
                        }),
                    ),
                );
            }
            if section != Section::Staged && change.status != 'U' {
                let path = path.clone();
                actions = actions.child(
                    tool_button_svg(("details-git-discard", ix), "undo-2", "Discard", c).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.confirm_discard(section, path.clone(), window, cx)
                        }),
                    ),
                );
            }
            let (icon, args): (&str, Vec<String>) = match section {
                Section::Staged => (
                    "minus",
                    vec!["restore".into(), "--staged".into(), "--".into(), path],
                ),
                _ => ("plus", vec!["add".into(), "--".into(), path]),
            };
            let label = if section == Section::Staged {
                "Unstage"
            } else {
                "Stage"
            };
            actions = actions.child(
                tool_button_svg(("details-git-stage", ix), icon, label, c)
                    .on_click(cx.listener(move |this, _, _, cx| this.run_git(args.clone(), cx))),
            );
        }
        let hover_key = key.clone();
        let diff_key = key;
        div()
            .id(("details-git", ix))
            .mx(px(INSET - 8.))
            .px(px(8.))
            .h(px(LIST_ROW + 2.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(px(6.))
            .when(open, |d| d.bg(c.selected))
            .hover(|s| s.bg(c.hover))
            .child(
                div()
                    .flex_none()
                    .w(px(10.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color)
                    .child(change.status.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(c.text)
                    .child(left_ellipsis(&change.path, if hovered { 24 } else { 34 })),
            )
            .child(actions)
            .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                let ui = &mut this.details.git_ui;
                if *on {
                    ui.hovered = Some(hover_key.clone());
                } else if ui.hovered.as_ref() == Some(&hover_key) {
                    ui.hovered = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_diff(diff_key.0, diff_key.1.clone(), cx)
            }))
            .into_any_element()
    }

    /// Shows the file's diff under its row, or hides it.
    fn toggle_diff(&mut self, section: Section, path: String, cx: &mut Context<Self>) {
        let ui = &mut self.details.git_ui;
        if ui
            .diff
            .take()
            .is_some_and(|d| d.section == section && d.path == path)
        {
            cx.notify();
            return;
        }
        let Some(dir) = self.details.cwd.clone() else {
            return;
        };
        let work = cx.background_spawn({
            let path = path.clone();
            async move { file_diff(&dir, section, &path) }
        });
        cx.spawn(async move |this, cx| {
            let lines = work.await;
            this.update(cx, |this, cx| {
                this.details.git_ui.diff = Some(OpenDiff {
                    section,
                    path,
                    lines,
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs `git <args>` in the pane's folder, then reads the status again.
    pub(super) fn run_git(&mut self, args: Vec<String>, cx: &mut Context<Self>) {
        let Some(dir) = self.details.cwd.clone() else {
            return;
        };
        let ui = &mut self.details.git_ui;
        ui.busy = true;
        ui.error = None;
        cx.notify();
        let work = cx.background_spawn(async move { git_command(&dir, &args) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                let ui = &mut this.details.git_ui;
                ui.busy = false;
                ui.error = result.err();
                // The open diff is stale now.
                if let Some(d) = ui.diff.take() {
                    this.toggle_diff(d.section, d.path, cx);
                }
                this.details.git_at = None;
                this.refresh_details(true, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Commits the staged changes with the typed message.
    pub(super) fn commit(&mut self, cx: &mut Context<Self>) {
        let message = self.details.field(FieldId::Commit).text.trim().to_owned();
        if message.is_empty() {
            return;
        }
        self.details.field(FieldId::Commit).text.clear();
        self.details.git_ui.committing = false;
        self.run_git(vec!["commit".into(), "-m".into(), message], cx);
    }

    /// Discards a file's changes (deletes an untracked one), once asked.
    fn confirm_discard(
        &mut self,
        section: Section,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (message, detail, args): (String, &str, Vec<String>) = match section {
            Section::Untracked => (
                format!("Delete “{path}”?"),
                "The file isn't tracked by git, so it can't be brought back.",
                vec!["clean".into(), "-f".into(), "--".into(), path],
            ),
            _ => (
                format!("Discard the changes to “{path}”?"),
                "This can't be undone.",
                vec!["restore".into(), "--".into(), path],
            ),
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(detail),
            &["Discard", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            this.update(cx, |this, cx| this.run_git(args, cx)).ok();
        })
        .detach();
    }
}

fn render_diff(lines: &[(DiffLine, String)], font: SharedString, c: &PanelColors) -> AnyElement {
    let mut col = div()
        .mx(px(INSET - 8.))
        .mb(px(6.))
        .py(px(4.))
        .flex()
        .flex_col()
        .rounded(px(6.))
        .bg(c.hover)
        .font_family(font)
        .text_size(px(11.))
        .overflow_hidden();
    if lines.is_empty() {
        col = col.child(
            div()
                .px(px(8.))
                .text_color(c.tertiary)
                .child("No textual changes"),
        );
    }
    for (kind, text) in lines.iter().take(400) {
        let (fg, bg) = match kind {
            DiffLine::Added => (c.success, Some(c.success.opacity(0.12))),
            DiffLine::Removed => (c.danger, Some(c.danger.opacity(0.12))),
            DiffLine::Hunk => (c.accent, None),
            DiffLine::Context => (c.secondary, None),
        };
        col = col.child(
            div()
                .px(px(8.))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_color(fg)
                .when_some(bg, |d, bg| d.bg(bg))
                .child(if text.is_empty() {
                    " ".to_owned()
                } else {
                    text.clone()
                }),
        );
    }
    if lines.len() > 400 {
        col = col.child(
            div()
                .px(px(8.))
                .text_color(c.tertiary)
                .child(format!("{} more lines", lines.len() - 400)),
        );
    }
    col.into_any_element()
}

/// A remote as a browsable URL (`git@github.com:a/b.git` → https).
pub(crate) fn web_url(remote: &str) -> String {
    let r = remote.trim().trim_end_matches(".git");
    if let Some(rest) = r.strip_prefix("git@")
        && let Some((host, path)) = rest.split_once(':')
    {
        return format!("https://{host}/{path}");
    }
    if let Some(rest) = r.strip_prefix("ssh://git@") {
        return format!("https://{rest}");
    }
    r.to_owned()
}

fn git(dir: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(dir).env("GIT_OPTIONAL_LOCKS", "0");
    cmd
}

/// Runs git, giving the first line of what it said when it failed.
fn git_command(dir: &Path, args: &[String]) -> Result<(), String> {
    let out = git(dir)
        .args(args)
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let text = String::from_utf8_lossy(&out.stdout);
    Err(err
        .lines()
        .chain(text.lines())
        .map(|l| {
            l.trim()
                .trim_start_matches("error: ")
                .trim_start_matches("fatal: ")
        })
        .find(|l| !l.is_empty())
        .unwrap_or("git failed")
        .to_owned())
}

pub(super) fn git_summary(dir: &Path) -> Option<GitSummary> {
    let read = |args: &[&str]| -> Option<String> {
        let out = git(dir).args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let status = read(&["status", "--porcelain=v2", "--branch", "-z"])?;
    let mut summary = parse_status(&status);
    if let Some(stat) = read(&["diff", "HEAD", "--numstat"]) {
        (summary.added, summary.removed) = parse_numstat(&stat);
    }
    summary.remote = read(&["remote", "get-url", "origin"])
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let top = read(&["rev-parse", "--show-toplevel"]).map(|s| PathBuf::from(s.trim()));
    if top.is_some_and(|t| t.join(".gitmodules").exists())
        && let Some(out) = read(&["submodule", "status"])
    {
        summary.submodules = parse_submodules(&out);
    }
    Some(summary)
}

/// A file's changes: staged ones against HEAD, unstaged against the
/// index, an untracked file as all new.
fn file_diff(dir: &Path, section: Section, path: &str) -> Vec<(DiffLine, String)> {
    let mut cmd = git(dir);
    cmd.args(["--no-pager", "diff", "--no-color", "--no-ext-diff"]);
    match section {
        Section::Staged => cmd.args(["--cached", "--", path]),
        Section::Unstaged => cmd.args(["--", path]),
        Section::Untracked => cmd.args(["--no-index", "--", "/dev/null", path]),
    };
    // `--no-index` exits 1 when the files differ, which they do.
    cmd.output()
        .map(|out| parse_diff(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

/// `git status --porcelain=v2 --branch -z`.
fn parse_status(out: &str) -> GitSummary {
    let mut s = GitSummary::default();
    let mut fields = out.split('\0');
    while let Some(entry) = fields.next() {
        if let Some(head) = entry.strip_prefix("# branch.head ") {
            s.branch = head.to_owned();
        } else if let Some(path) = entry.strip_prefix("? ") {
            s.untracked.push(path.to_owned());
        } else if entry.starts_with("1 ") || entry.starts_with("2 ") || entry.starts_with("u ") {
            let kind = entry.as_bytes()[0];
            // Ordinary: 8 fields before the path; renamed: 9; unmerged: 10.
            let skip = match kind {
                b'1' => 8,
                b'2' => 9,
                _ => 10,
            };
            let mut parts = entry.splitn(skip + 1, ' ');
            let xy: Vec<char> = parts.nth(1).unwrap_or("..").chars().collect();
            let path = parts.nth(skip - 2).unwrap_or_default().to_owned();
            if kind == b'2' {
                fields.next(); // the original path
            }
            let (x, y) = (
                xy.first().copied().unwrap_or('.'),
                xy.get(1).copied().unwrap_or('.'),
            );
            if kind == b'u' {
                s.unstaged.push(Change { status: 'U', path });
                continue;
            }
            if x != '.' {
                s.staged.push(Change {
                    status: x,
                    path: path.clone(),
                });
            }
            if y != '.' {
                s.unstaged.push(Change { status: y, path });
            }
        }
    }
    if s.branch == "(detached)" {
        s.branch = "detached HEAD".into();
    }
    s
}

fn parse_numstat(out: &str) -> (u32, u32) {
    out.lines().fold((0, 0), |(a, r), line| {
        let mut f = line.split('\t');
        let add = f.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
        let del = f.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
        (a + add, r + del)
    })
}

/// `git submodule status`: `<state><sha> <path> (<describe>)`.
fn parse_submodules(out: &str) -> Vec<Submodule> {
    out.lines()
        .filter_map(|line| {
            let state = line.chars().next()?;
            let mut parts = line[state.len_utf8()..].split_whitespace();
            let commit = parts.next()?.to_owned();
            let path = parts.next()?.to_owned();
            Some(Submodule {
                path,
                state,
                commit,
            })
        })
        .collect()
}

/// A unified diff's hunks, without the file headers.
pub(super) fn parse_diff(out: &str) -> Vec<(DiffLine, String)> {
    let mut lines = Vec::new();
    let mut in_hunk = false;
    for line in out.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            lines.push((DiffLine::Hunk, line.to_owned()));
        } else if line.starts_with("diff ") {
            in_hunk = false;
        } else if !in_hunk {
            continue;
        } else if let Some(rest) = line.strip_prefix('+') {
            lines.push((DiffLine::Added, format!("+{rest}")));
        } else if let Some(rest) = line.strip_prefix('-') {
            lines.push((DiffLine::Removed, format!("-{rest}")));
        } else if !line.starts_with('\\') {
            lines.push((DiffLine::Context, line.to_owned()));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_v2_status() {
        let out = "# branch.oid abc\0# branch.head main\0\
            1 .M N... 100644 100644 100644 aaa bbb src/lib.rs\0\
            1 A. N... 000000 100644 100644 000 ccc new file.txt\0\
            1 MM N... 100644 100644 100644 aaa bbb both.rs\0\
            2 R. N... 100644 100644 100644 aaa bbb R100 renamed.rs\0old.rs\0\
            ? notes.md\0";
        let s = parse_status(out);
        assert_eq!(s.branch, "main");
        let names = |l: &[Change]| -> Vec<(char, String)> {
            l.iter().map(|c| (c.status, c.path.clone())).collect()
        };
        assert_eq!(
            names(&s.staged),
            [
                ('A', "new file.txt".to_owned()),
                ('M', "both.rs".to_owned()),
                ('R', "renamed.rs".to_owned())
            ]
        );
        assert_eq!(
            names(&s.unstaged),
            [('M', "src/lib.rs".to_owned()), ('M', "both.rs".to_owned())]
        );
        assert_eq!(s.untracked, ["notes.md"]);
    }

    #[test]
    fn numstat_and_remotes() {
        assert_eq!(parse_numstat("3\t1\ta\n-\t-\tbin\n10\t0\tb\n"), (13, 1));
        assert_eq!(web_url("git@github.com:a/b.git"), "https://github.com/a/b");
        assert_eq!(web_url("ssh://git@host/a/b"), "https://host/a/b");
    }

    #[test]
    fn submodule_status() {
        let out =
            " 1a2b3c4d vendor/lib (v1.2)\n-5e6f7a8b ext/tool\n+9c0d1e2f deps/x (heads/main)\n";
        let subs = parse_submodules(out);
        assert_eq!(subs.len(), 3);
        assert_eq!((subs[0].state, subs[0].path.as_str()), (' ', "vendor/lib"));
        assert_eq!((subs[1].state, subs[1].commit.as_str()), ('-', "5e6f7a8b"));
        assert_eq!(subs[2].state, '+');
    }

    #[test]
    fn unified_diff() {
        let out = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n\
            @@ -1,3 +1,3 @@\n keep\n-old\n+new\n\\ No newline at end of file\n";
        assert_eq!(
            parse_diff(out),
            [
                (DiffLine::Hunk, "@@ -1,3 +1,3 @@".to_owned()),
                (DiffLine::Context, " keep".to_owned()),
                (DiffLine::Removed, "-old".to_owned()),
                (DiffLine::Added, "+new".to_owned()),
            ]
        );
    }
}
