//! Tab rules (Settings › View › Tabs & Title): each tab's alias, icon and
//! title, worked out from what it's showing.

use gpui::{AnyElement, App, Hsla, div, prelude::*, px, svg};
use motty_render::hsla;
use motty_theme::tab_rule::{self, Facts, Look};
use motty_theme::{Rgba, Theme};

use super::{Content, Tab, Workspace};

/// A tab icon's box; Phosphor's marks fill about three quarters of it.
pub(super) const ICON_SIZE: f32 = 16.;
/// How far the mark sits inside its box, and the room after it before the
/// title (Otty: the title starts 20.5pt after the mark's left edge).
pub(super) const ICON_INSET: f32 = 2.;
pub(super) const ICON_GAP: f32 = 6.5;

impl Workspace {
    /// Each tab's look from the rules, before the tabs are drawn.
    pub(super) fn refresh_looks(&mut self, cx: &App) {
        let rules = &self.config.tab_rules;
        let home = dirs::home_dir();
        for (ix, tab) in self.tabs.iter_mut().enumerate() {
            tab.look = if rules.is_empty() {
                Look::default()
            } else {
                tab_rule::resolve(rules, &facts(tab, ix, home.clone(), cx))
            };
        }
    }
}

fn facts(tab: &Tab, ix: usize, home: Option<std::path::PathBuf>, cx: &App) -> Facts {
    let mut f = Facts {
        home,
        index: ix + 1,
        ..Default::default()
    };
    match &tab.content {
        Content::Panes(t) if t.terminal().is_some() => {
            let v = t.terminal().expect("checked").read(cx);
            f.cwd.clone_from(&v.cwd);
            f.command.clone_from(&v.command);
            f.branch.clone_from(&v.branch);
            f.title = v.title.clone().filter(|t| !t.is_empty());
            f.shell.clone_from(&v.process);
            if let Some(status) = &v.status {
                f.agent.clone_from(&status.agent_id);
                f.agent_label.clone_from(&status.agent);
            }
            if let Some(dest) = v.system.ssh.as_deref().filter(|d| !d.is_empty()) {
                match dest.split_once('@') {
                    Some((user, host)) => {
                        f.user = Some(user.to_owned());
                        f.host = Some(host.to_owned());
                    }
                    None => f.host = Some(dest.to_owned()),
                }
            }
        }
        Content::Panes(t) => {
            if let Some(path) = t.focused.file().and_then(|e| e.read(cx).path()) {
                f.file = path.file_name().map(|n| n.to_string_lossy().into_owned());
                f.cwd = path.parent().map(|p| p.to_path_buf());
            }
        }
        Content::Search(_) => {}
    }
    f
}

/// `icon-color`: an ANSI color name off the palette (`red`,
/// `bright-blue`), or `#rrggbb`.
pub(super) fn icon_color(name: Option<&str>, t: &Theme) -> Option<Rgba> {
    let name = name?.trim().to_lowercase();
    const NAMES: [&str; 8] = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    let (base, bright) = match name.strip_prefix("bright-") {
        Some(rest) => (rest, 8),
        None => (name.as_str(), 0),
    };
    if let Some(i) = NAMES.iter().position(|n| *n == base) {
        return Some(t.terminal.palette[i + bright]);
    }
    motty_theme::css::parse_color(&name).ok()
}

/// The rule's icon: one of ours, or an emoji (any other text). Uncolored
/// icons take the tab's text color.
pub(super) fn icon(look: &Look, fallback: Hsla, t: &Theme) -> Option<AnyElement> {
    let name = look.icon.as_deref()?.trim();
    let color = icon_color(look.icon_color.as_deref(), t).map_or(fallback, hsla);
    let el = match crate::tab_icons::path(name) {
        Some(path) => svg()
            .path(path)
            .flex_none()
            .size(px(ICON_SIZE))
            .text_color(color)
            .into_any_element(),
        // An unknown name shows nothing, as in Otty; anything that isn't
        // a plain word is taken as an emoji.
        None if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => return None,
        None => div()
            .flex_none()
            .w(px(ICON_SIZE))
            .flex()
            .justify_center()
            .text_size(px(ICON_SIZE - 3.))
            .child(name.to_owned())
            .into_any_element(),
    };
    Some(el)
}
