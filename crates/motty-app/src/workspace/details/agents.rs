//! The Agents tab: every coding agent in every window, grouped by project.
//! Agents that need you come first, then working ones, then the rest; each
//! row says for how long, and a click goes to the agent's pane.

use std::path::Path;
use std::time::SystemTime;

use gpui::{AnyElement, Context, FontWeight, Window, div, prelude::*, px};

use super::{INSET, LIST_ROW, PanelColors, Workspace, short_duration};
use crate::agents::Badge;

/// One agent's line.
#[derive(Clone, Debug)]
pub(super) struct AgentRow {
    pub pane: u64,
    pub project: String,
    pub agent: String,
    pub badge: Option<Badge>,
    pub message: Option<String>,
    pub since: SystemTime,
}

/// Most pressing first: waiting, failed, working, done, idle.
pub(super) fn rank(badge: Option<Badge>) -> u8 {
    match badge {
        Some(Badge::Awaiting) => 0,
        Some(Badge::Error) => 1,
        Some(Badge::Running) => 2,
        Some(Badge::Done) => 3,
        _ => 4,
    }
}

/// The project a folder belongs to: the repository it's in (a worktree in
/// `.worktrees/` counts as its main checkout's), else the folder itself.
pub(super) fn project_of(cwd: &Path) -> String {
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.display().to_string())
    };
    // `<repo>/.worktrees/<branch>/…` is `<repo>`.
    let mut parts = cwd.ancestors();
    if let Some(repo) = parts.find(|p| {
        p.parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == crate::workspace::WORKTREES)
    }) && let Some(main) = repo.parent().and_then(Path::parent)
    {
        return name(main);
    }
    cwd.ancestors()
        .find(|p| p.join(".git").exists())
        .map_or_else(|| name(cwd), name)
}

/// What a row says about its state, with how long it has been so.
fn state_line(badge: Option<Badge>, since: SystemTime, now: SystemTime) -> String {
    let age = now
        .duration_since(since)
        .map(short_duration)
        .unwrap_or_default();
    match badge {
        Some(Badge::Awaiting) => format!("waiting {age}"),
        Some(Badge::Error) => format!("failed {age} ago"),
        Some(Badge::Running) => format!("working {age}"),
        Some(Badge::Done) => format!("done {age} ago"),
        _ => format!("idle {age}"),
    }
}

/// Rows grouped by project: projects with the most pressing agent first.
pub(super) fn group(mut rows: Vec<AgentRow>) -> Vec<(String, Vec<AgentRow>)> {
    // In each state, the one that has been so longest first.
    rows.sort_by(|a, b| {
        (rank(a.badge), a.since)
            .cmp(&(rank(b.badge), b.since))
            .then_with(|| a.pane.cmp(&b.pane))
    });
    let mut groups: Vec<(String, Vec<AgentRow>)> = Vec::new();
    for row in rows {
        match groups.iter_mut().find(|(p, _)| *p == row.project) {
            Some((_, list)) => list.push(row),
            None => groups.push((row.project.clone(), vec![row])),
        }
    }
    groups
}

impl Workspace {
    /// This window's agents.
    pub(super) fn agent_rows(&self, cx: &gpui::App) -> Vec<AgentRow> {
        self.panes()
            .filter_map(|(_, view)| {
                let v = view.read(cx);
                let status = v.status.as_ref()?;
                let agent = status.agent.clone()?;
                Some(AgentRow {
                    pane: v.id,
                    project: v
                        .cwd
                        .as_deref()
                        .map_or_else(|| "Other".to_owned(), project_of),
                    agent,
                    badge: status.badge,
                    message: status.message.clone(),
                    since: status.since,
                })
            })
            .collect()
    }

    pub(super) fn render_agents(
        &self,
        window: &mut Window,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // This window's own; the others' through their handles (this one
        // is being drawn, so it can't be read through its handle).
        let mut rows = self.agent_rows(cx);
        let own = window.window_handle().window_id();
        for handle in crate::ipc::workspaces(cx) {
            if handle.window_id() == own {
                continue;
            }
            if let Ok(other) = handle.read(cx) {
                rows.extend(other.agent_rows(cx));
            }
        }
        let mut col = div().flex().flex_col().pt(px(12.));
        if rows.is_empty() {
            return col
                .child(
                    div()
                        .px(px(INSET))
                        .text_color(c.secondary)
                        .child("No agents running"),
                )
                .child(
                    div()
                        .px(px(INSET))
                        .pt(px(4.))
                        .text_size(px(11.5))
                        .text_color(c.tertiary)
                        .child("Coding agents started in any pane show up here."),
                )
                .into_any_element();
        }
        let now = SystemTime::now();
        let theme = self.theme.clone();
        for (gx, (project, list)) in group(rows).into_iter().enumerate() {
            col = col.child(
                div()
                    .px(px(INSET))
                    .pt(px(if gx == 0 { 0. } else { 12. }))
                    .pb(px(4.))
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.tertiary)
                    .child(project.to_uppercase()),
            );
            for row in list {
                let pane = row.pane;
                let icon = match row.badge {
                    Some(badge) => crate::workspace::badge_icon(badge, pane as usize, &theme),
                    None => div()
                        .flex_none()
                        .size(px(6.))
                        .mx(px(3.))
                        .rounded_full()
                        .bg(c.tertiary)
                        .into_any_element(),
                };
                let state = state_line(row.badge, row.since, now);
                let mut card = div()
                    .id(("details-agent", pane as usize))
                    .mx(px(INSET - 8.))
                    .px(px(8.))
                    .py(px(4.))
                    .min_h(px(LIST_ROW))
                    .rounded(px(5.))
                    .flex()
                    .flex_col()
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        // Not from inside this window's update.
                        cx.defer(move |cx| crate::ipc::reveal_pane_by_id(pane, cx));
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(icon)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_color(c.text)
                                    .child(row.agent.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.5))
                                    .text_color(if row.badge == Some(Badge::Awaiting) {
                                        c.warning
                                    } else {
                                        c.tertiary
                                    })
                                    .child(state),
                            ),
                    );
                if let Some(message) = row.message.clone() {
                    card = card.child(
                        div()
                            .pl(px(20.))
                            .text_size(px(11.5))
                            .text_color(c.secondary)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(message),
                    );
                }
                col = col.child(card);
            }
        }
        col.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn row(pane: u64, project: &str, badge: Option<Badge>, age: u64) -> AgentRow {
        AgentRow {
            pane,
            project: project.into(),
            agent: "Claude Code".into(),
            badge,
            message: None,
            since: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 - age),
        }
    }

    #[test]
    fn waiting_agents_lead() {
        let groups = group(vec![
            row(1, "web", None, 5),
            row(2, "api", Some(Badge::Running), 5),
            row(3, "web", Some(Badge::Awaiting), 5),
            row(4, "api", Some(Badge::Running), 50),
        ]);
        let names: Vec<&str> = groups.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, ["web", "api"]);
        let web: Vec<u64> = groups[0].1.iter().map(|r| r.pane).collect();
        assert_eq!(web, [3, 1]);
        // Longest working first.
        let api: Vec<u64> = groups[1].1.iter().map(|r| r.pane).collect();
        assert_eq!(api, [4, 2]);
    }

    #[test]
    fn projects() {
        let dir = std::env::temp_dir().join(format!("motty-agents-{}", std::process::id()));
        let repo = dir.join("shop");
        let tree = repo.join(".worktrees").join("feat-x").join("src");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert_eq!(project_of(&repo.join("src")), "shop");
        assert_eq!(project_of(&tree), "shop");
        assert_eq!(project_of(&dir), dir.file_name().unwrap().to_string_lossy());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn says_for_how_long() {
        let since = SystemTime::UNIX_EPOCH;
        let now = since + Duration::from_secs(120);
        assert_eq!(state_line(Some(Badge::Awaiting), since, now), "waiting 2m");
        assert_eq!(state_line(None, since, now), "idle 2m");
    }
}
