//! What the tabs panel does besides switching tabs: a tab's right-click
//! menu (rename, move, mark, dividers), the filter button's Group and
//! Order, dragging tabs into place, Move Tab to New Window and ⇧⌘U.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui::{
    App, Context, FocusHandle, KeyDownEvent, Pixels, Point, Render, Window, div, prelude::*, px,
};

use super::picker::edit_query;
use super::{Content, Tab, Workspace, days_ago, tab_title};
use crate::agents::Badge;
use crate::native_menu::{self, Entry, Item};

/// A name given with Rename Tab.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabName {
    pub text: String,
    /// "Prefix": shown before the title, which still follows the program.
    pub prefix: bool,
}

/// The title a tab shows, with its given name.
pub(super) fn named(name: Option<&TabName>, title: String) -> String {
    match name {
        Some(n) if n.prefix => format!("{}: {title}", n.text),
        Some(n) => n.text.clone(),
        None => title,
    }
}

/// The tabs panel's Group setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Grouping {
    #[default]
    None,
    /// By the git repository (or folder) of each tab's working directory.
    Project,
    /// Today, Yesterday, Earlier, by when each tab was last used.
    Date,
}

/// The tabs panel's Order setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Order {
    #[default]
    Created,
    Updated,
    /// As dragged (or moved with Move Up / Move Down).
    Manual,
}

impl Grouping {
    /// How it's saved with the window.
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Project => "project",
            Self::Date => "date",
        }
    }

    pub(super) fn from_key(key: &str) -> Self {
        match key {
            "project" => Self::Project,
            "date" => Self::Date,
            _ => Self::None,
        }
    }
}

impl Order {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Created => "",
            Self::Updated => "updated",
            Self::Manual => "manual",
        }
    }

    pub(super) fn from_key(key: &str) -> Self {
        match key {
            "updated" => Self::Updated,
            "manual" => Self::Manual,
            _ => Self::Created,
        }
    }
}

/// A tab's name being typed in its card.
pub(super) struct Rename {
    pub ix: usize,
    pub text: String,
    pub focus: FocusHandle,
}

/// A group of tabs in the panel, under its heading.
pub(super) struct Group {
    pub heading: Option<String>,
    /// Where the heading's "+" opens a tab.
    pub folder: Option<PathBuf>,
    pub tabs: Vec<usize>,
}

/// What dragging a tab card carries.
#[derive(Clone, Copy)]
pub(super) struct TabDrag {
    pub ix: usize,
}

/// The card under the pointer while a tab is dragged.
pub(super) struct DragPreview {
    pub title: String,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.))
            .py(px(6.))
            .rounded(px(7.))
            .bg(gpui::hsla(0., 0., 0.5, 0.25))
            .text_size(px(13.))
            .opacity(0.9)
            .child(self.title.clone())
    }
}

enum Pick {
    Rename,
    NewWindow,
    Up,
    Down,
    Mark(Badge),
    ClearBadge,
    Divider(bool),
    About,
    Close,
    Group(Grouping),
    Order(Order),
    RemoveDividers,
    Action(Box<dyn gpui::Action>),
}

/// Menu rows and what each one does, counted as `native_menu` counts.
struct Menu {
    entries: Vec<Entry>,
    picks: Vec<Pick>,
}

impl Menu {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            picks: Vec::new(),
        }
    }

    fn item(&mut self, item: Item, pick: Pick) {
        self.entries.push(item.into());
        self.picks.push(pick);
    }

    fn sub(&mut self, label: &str, items: Vec<(Item, Pick)>) {
        let mut entries = Vec::new();
        for (item, pick) in items {
            entries.push(item.into());
            self.picks.push(pick);
        }
        self.entries.push(Entry::Submenu {
            label: label.to_owned().into(),
            entries,
        });
    }

    fn separator(&mut self) {
        self.entries.push(Entry::Separator);
    }
}

impl Workspace {
    /// Right-click on a tab card.
    pub(super) fn show_tab_menu(
        &mut self,
        ix: usize,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let badge = self.tab_summary(tab, cx).badge.is_some();
        let manual_spot = self.grouping == Grouping::None;
        let mut m = Menu::new();
        m.item(Item::new("Rename Tab…"), Pick::Rename);
        m.item(
            Item::new("Move Tab to New Window").enabled(self.tabs.len() > 1),
            Pick::NewWindow,
        );
        m.item(Item::new("Move Up").enabled(ix > 0), Pick::Up);
        m.item(
            Item::new("Move Down").enabled(ix + 1 < self.tabs.len()),
            Pick::Down,
        );
        m.separator();
        m.sub(
            "Mark Tab",
            vec![
                (Item::new("Unread"), Pick::Mark(Badge::Finished)),
                (Item::new("Processing"), Pick::Mark(Badge::Running)),
                (Item::new("Failed"), Pick::Mark(Badge::Error)),
            ],
        );
        m.item(Item::new("Clear Badge").enabled(badge), Pick::ClearBadge);
        m.separator();
        if tab.divider {
            m.item(Item::new("Remove Divider Below"), Pick::Divider(false));
        } else {
            m.item(
                Item::new("Insert Divider Below").enabled(manual_spot),
                Pick::Divider(true),
            );
        }
        m.item(Item::new("About This Tab…"), Pick::About);
        m.separator();
        m.item(Item::new("Close Tab"), Pick::Close);
        let picks = m.picks;
        let this = cx.entity().downgrade();
        native_menu::popup(
            m.entries,
            at,
            None,
            window,
            cx,
            move |picked, window, cx| {
                let Some(pick) = picks.into_iter().nth(picked) else {
                    return;
                };
                let _ = this.update(cx, |this, cx| this.run_tab_pick(ix, pick, window, cx));
            },
        );
    }

    /// A click on the window's title: what Otty's title menu offers.
    pub(super) fn show_title_menu(
        &mut self,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ix = self.active;
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let badge = self.tab_summary(tab, cx).badge.is_some();
        let terminal = tab.terms().is_some();
        let mut m = Menu::new();
        m.item(Item::new("Rename Tab…"), Pick::Rename);
        m.item(Item::new("About This Tab…"), Pick::About);
        m.item(Item::new("Clear Badge").enabled(badge), Pick::ClearBadge);
        m.separator();
        m.item(
            Item::new("Copy Path").enabled(terminal),
            Pick::Action(Box::new(crate::actions::CopyPath)),
        );
        m.item(
            Item::new("Reveal in Finder").enabled(terminal),
            Pick::Action(Box::new(crate::actions::RevealInFinder)),
        );
        m.separator();
        m.item(
            Item::new("Details Panel").checked(self.details.visible),
            Pick::Action(Box::new(crate::actions::ToggleDetailsPanel)),
        );
        let picks = m.picks;
        let this = cx.entity().downgrade();
        native_menu::popup(
            m.entries,
            at,
            None,
            window,
            cx,
            move |picked, window, cx| {
                let Some(pick) = picks.into_iter().nth(picked) else {
                    return;
                };
                let _ = this.update(cx, |this, cx| this.run_tab_pick(ix, pick, window, cx));
            },
        );
    }

    /// The filter button at the top of the tabs panel (or a right-click on
    /// its empty part).
    pub(super) fn show_filter_menu(
        &mut self,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut m = Menu::new();
        m.sub(
            "Group",
            [
                ("No Grouping", Grouping::None),
                ("By Project", Grouping::Project),
                ("By Date", Grouping::Date),
            ]
            .into_iter()
            .map(|(label, g)| (Item::new(label).checked(self.grouping == g), Pick::Group(g)))
            .collect(),
        );
        m.sub(
            "Order",
            [
                ("Created Time", Order::Created),
                ("Updated Time", Order::Updated),
                ("Manual", Order::Manual),
            ]
            .into_iter()
            .map(|(label, o)| (Item::new(label).checked(self.order == o), Pick::Order(o)))
            .collect(),
        );
        m.separator();
        m.item(
            Item::new("Insert Divider").enabled(self.grouping == Grouping::None),
            Pick::Divider(true),
        );
        m.item(
            Item::new("Remove All Dividers").enabled(self.tabs.iter().any(|t| t.divider)),
            Pick::RemoveDividers,
        );
        let picks = m.picks;
        let active = self.active;
        let this = cx.entity().downgrade();
        native_menu::popup(
            m.entries,
            at,
            None,
            window,
            cx,
            move |picked, window, cx| {
                let Some(pick) = picks.into_iter().nth(picked) else {
                    return;
                };
                let _ = this.update(cx, |this, cx| this.run_tab_pick(active, pick, window, cx));
            },
        );
    }

    fn run_tab_pick(&mut self, ix: usize, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        match pick {
            Pick::Rename => self.start_rename(ix, window, cx),
            Pick::NewWindow => self.move_tab_to_new_window(ix, window, cx),
            Pick::Up => self.move_tab(ix, ix.wrapping_sub(1), window, cx),
            Pick::Down => self.move_tab(ix, ix + 1, window, cx),
            Pick::Mark(badge) => {
                self.tabs[ix].mark = Some(badge);
                cx.notify();
            }
            Pick::ClearBadge => self.clear_badge(ix, cx),
            Pick::Divider(on) => {
                self.tabs[ix].divider = on;
                cx.notify();
            }
            Pick::About => self.about_tab(ix, window, cx),
            Pick::Close => self.close_asking(super::closing::Closing::Tab(ix), window, cx),
            Pick::Group(g) => {
                self.grouping = g;
                cx.notify();
            }
            Pick::Order(o) => {
                self.order = o;
                cx.notify();
            }
            Pick::Action(action) => window.dispatch_action(action, cx),
            Pick::RemoveDividers => {
                for tab in &mut self.tabs {
                    tab.divider = false;
                }
                cx.notify();
            }
        }
    }

    /// Rename Tab: the card's title becomes a text field.
    pub(super) fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let text = match &tab.name {
            Some(n) => n.text.clone(),
            None => tab_title(tab, cx),
        };
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Clicking elsewhere keeps what was typed.
        cx.on_focus_out(&focus, window, |this, _, window, cx| {
            this.finish_rename(true, window, cx)
        })
        .detach();
        self.sidebar_visible = true;
        self.inbox = false;
        self.renaming = Some(Rename { ix, text, focus });
        cx.notify();
    }

    pub(super) fn on_rename_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        match ks.key.as_str() {
            "enter" => self.finish_rename(true, window, cx),
            "escape" => self.finish_rename(false, window, cx),
            _ => {
                if let Some(r) = self.renaming.as_mut()
                    && edit_query(&mut r.text, ks)
                {
                    cx.notify();
                }
            }
        }
        cx.stop_propagation();
    }

    /// Ends renaming, keeping the name (an empty one goes back to the
    /// title) or not.
    pub(super) fn finish_rename(
        &mut self,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = self.renaming.take() else {
            return;
        };
        if keep && let Some(tab) = self.tabs.get_mut(rename.ix) {
            let text = rename.text.trim();
            tab.name = (!text.is_empty()).then(|| TabName {
                text: text.to_owned(),
                prefix: false,
            });
        }
        if rename.focus.is_focused(window) {
            self.activate(self.active, window, cx);
        }
        cx.notify();
    }

    /// Moves the tab at `from` to `to` (Move Up/Down, dragging); the panel
    /// keeps the order from then on.
    pub(super) fn move_tab(
        &mut self,
        from: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let active = self.tabs.len().min(self.active);
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        // The active tab stays active wherever it went.
        self.active = moved_index(active, from, to);
        self.order = Order::Manual;
        let _ = window;
        cx.notify();
    }

    pub(super) fn clear_badge(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(ix) else {
            return;
        };
        tab.mark = None;
        if let Some(terms) = tab.terms() {
            for view in terms.root.leaves() {
                view.update(cx, |v, _| {
                    if let Some(s) = &mut v.status {
                        s.badge = None;
                    }
                });
            }
        }
        cx.notify();
    }

    /// ⇧⌘U: the next tab (after this one, wrapping) with a badge that asks
    /// to be looked at.
    pub(super) fn show_next_unread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len();
        let next = (1..n).map(|d| (self.active + d) % n).find(|&ix| {
            self.tab_summary(&self.tabs[ix], cx)
                .badge
                .is_some_and(|b| b != Badge::Running)
        });
        match next {
            Some(ix) => self.activate(ix, window, cx),
            None => window.play_system_bell(),
        }
    }

    /// What's in a tab: its panes' ids, programs and folders.
    fn about_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let tab = &self.tabs[ix];
        let title = tab_title(tab, cx);
        let detail = match &tab.content {
            Content::Terminals(terms) => terms
                .root
                .leaves()
                .iter()
                .map(|v| {
                    let v = v.read(cx);
                    format!(
                        "Pane {} · {} · {}",
                        v.id,
                        v.process.as_deref().unwrap_or("shell"),
                        v.cwd
                            .as_deref()
                            .map(|c| c.display().to_string())
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Content::Editor(e) => e
                .read(cx)
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "Untitled".into()),
        };
        let ids = match &tab.content {
            Content::Terminals(terms) => terms
                .root
                .leaves()
                .iter()
                .map(|v| v.read(cx).id.to_string())
                .collect::<Vec<_>>()
                .join(" "),
            Content::Editor(_) => String::new(),
        };
        let answer = window.prompt(
            gpui::PromptLevel::Info,
            &title,
            Some(&detail),
            &["OK", "Copy Pane ID"],
            cx,
        );
        cx.spawn(async move |_, cx| {
            if answer.await == Ok(1) {
                cx.update(|cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(ids));
                });
            }
        })
        .detach();
    }

    /// Takes the tab, panes running, into a window of its own.
    pub(super) fn move_tab_to_new_window(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.len() < 2 || ix >= self.tabs.len() {
            return window.play_system_bell();
        }
        let tab = self.tabs.remove(ix);
        // This window stops following its panes; the new one picks them up.
        match &tab.content {
            Content::Terminals(t) => {
                for view in t.root.leaves() {
                    self.pane_subscriptions.remove(&view.entity_id());
                }
            }
            Content::Editor(e) => {
                self.pane_subscriptions.remove(&e.entity_id());
            }
        }
        let next = if self.active > ix {
            self.active - 1
        } else {
            self.active.min(self.tabs.len() - 1)
        };
        self.activate(next, window, cx);
        let config = self.config.clone();
        if let Err(e) = crate::open_window_with_tab(config, MovedTab(tab), cx) {
            tracing::error!("couldn't move the tab: {e:#}");
        }
    }

    /// Takes in a tab moved from another window.
    pub(crate) fn adopt(&mut self, moved: MovedTab, window: &mut Window, cx: &mut Context<Self>) {
        let tab = moved.0;
        match &tab.content {
            Content::Terminals(t) => {
                for view in t.root.leaves() {
                    view.update(cx, |v, cx| v.attach_window(window, cx));
                    self.watch_terminal(view, window, cx);
                }
            }
            Content::Editor(e) => self.watch_editor(e, cx),
        }
        self.tabs.push(tab);
        self.activate(self.tabs.len() - 1, window, cx);
    }

    /// The panel's groups, each in the chosen order.
    pub(super) fn sidebar_groups(&self, cx: &App) -> Vec<Group> {
        let mut order: Vec<usize> = (0..self.tabs.len()).collect();
        match self.order {
            Order::Manual => {}
            Order::Created => order.sort_by_key(|&ix| self.tabs[ix].created),
            Order::Updated => order.sort_by_key(|&ix| std::cmp::Reverse(self.tabs[ix].updated)),
        }
        match self.grouping {
            Grouping::None => vec![Group {
                heading: None,
                folder: None,
                tabs: order,
            }],
            Grouping::Date => {
                let now = SystemTime::now();
                let today = days_ago(now, now);
                let mut groups: Vec<Group> = ["Today", "Yesterday", "Earlier"]
                    .into_iter()
                    .map(|h| Group {
                        heading: Some(h.into()),
                        folder: None,
                        tabs: Vec::new(),
                    })
                    .collect();
                for ix in order {
                    let age = today - days_ago(self.tabs[ix].updated, now);
                    groups[age.clamp(0, 2) as usize].tabs.push(ix);
                }
                groups.retain(|g| !g.tabs.is_empty());
                groups
            }
            Grouping::Project => {
                let mut groups: Vec<Group> = Vec::new();
                for ix in order {
                    let folder = self.tab_folder(&self.tabs[ix], cx);
                    let root = folder.as_deref().map(project_root);
                    let heading = match &root {
                        Some(root) => project_name(root),
                        None => "Other".into(),
                    };
                    match groups
                        .iter_mut()
                        .find(|g| g.heading.as_deref() == Some(&heading))
                    {
                        Some(g) => g.tabs.push(ix),
                        None => groups.push(Group {
                            heading: Some(heading),
                            folder: root,
                            tabs: vec![ix],
                        }),
                    }
                }
                groups
            }
        }
    }

    fn tab_folder(&self, tab: &Tab, cx: &App) -> Option<PathBuf> {
        match &tab.content {
            Content::Terminals(t) => t.focused.read(cx).cwd.clone(),
            Content::Editor(e) => e.read(cx).folder(),
        }
    }

    /// The tab card while it's being renamed: a text field with a caret.
    pub(super) fn rename_field(&self, rename: &Rename, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = &self.theme;
        div()
            .id("rename-tab")
            .track_focus(&rename.focus)
            .on_key_down(cx.listener(Self::on_rename_key))
            .flex_1()
            .min_w_0()
            .h(px(22.))
            .px(px(6.))
            .ml(px(-6.))
            .flex()
            .items_center()
            .rounded(px(5.))
            .bg(motty_render::hsla(t.tabs.active.background))
            .border(px(1.5))
            .border_color(motty_render::hsla(motty_theme::Rgba::rgb(0x047afe)))
            .overflow_hidden()
            .child(div().flex_none().child(rename.text.clone()))
            .child(
                div()
                    .flex_none()
                    .w(px(1.5))
                    .h(px(15.))
                    .bg(motty_render::hsla(motty_theme::Rgba::rgb(0x047afe))),
            )
            .into_any_element()
    }
}

/// A tab on its way to another window.
pub(crate) struct MovedTab(pub(super) Tab);

/// Where index `ix` ends up after the item at `from` moves to `to`.
fn moved_index(ix: usize, from: usize, to: usize) -> usize {
    if ix == from {
        to
    } else if from < ix && ix <= to {
        ix - 1
    } else if to <= ix && ix < from {
        ix + 1
    } else {
        ix
    }
}

/// The repository a folder is in, else the folder itself.
fn project_root(folder: &Path) -> PathBuf {
    folder
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(folder)
        .to_path_buf()
}

fn project_name(root: &Path) -> String {
    if dirs::home_dir().as_deref() == Some(root) {
        return "~".into();
    }
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_follow_a_move() {
        // [a b c d], b (1) → 3: [a c d b].
        assert_eq!(moved_index(1, 1, 3), 3);
        assert_eq!(moved_index(2, 1, 3), 1);
        assert_eq!(moved_index(3, 1, 3), 2);
        assert_eq!(moved_index(0, 1, 3), 0);
        // d (3) → 0: [d a b c].
        assert_eq!(moved_index(0, 3, 0), 1);
        assert_eq!(moved_index(3, 3, 0), 0);
    }

    #[test]
    fn names_replace_or_prefix() {
        let n = TabName {
            text: "api".into(),
            prefix: false,
        };
        assert_eq!(named(Some(&n), "~/src".into()), "api");
        let p = TabName {
            text: "api".into(),
            prefix: true,
        };
        assert_eq!(named(Some(&p), "vim".into()), "api: vim");
        assert_eq!(named(None, "zsh".into()), "zsh");
    }
}
