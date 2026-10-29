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
use crate::native_menu::{self, Entry, Item, NameField};

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
    /// Set Tab Prefix: the name goes before the title.
    pub prefix: bool,
    /// The label of the divider below the tab, not the tab.
    pub divider: bool,
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
    /// The Inbox in place of the tabs.
    Inbox,
    Order(Order),
    RemoveDividers,
    Action(Box<dyn gpui::Action>),
    OpenIn(String, PathBuf),
    Privilege(Privilege),
    DefaultPrivileges,
    FocusPane(gpui::Entity<crate::terminal_view::TerminalView>),
    RenameDivider,
    /// A row that only shows something.
    Nothing,
}

/// The title menu's Notifications & Privileges switches, in Otty's
/// Settings › Shell order: banners, sounds, badges, agents, then what a
/// program may control.
#[derive(Clone, Copy, Debug)]
pub(super) enum Privilege {
    NotifyFinish,
    NotifyError,
    NotifyWatch,
    NotifyShell,
    SoundOnError,
    SoundShell,
    ExitStatus,
    Processing,
    Complete,
    Awaiting,
    NotifyComplete,
    NotifyAwaiting,
    Caffeinate,
    TitleShell,
    ClipboardShell,
    MouseShell,
}

impl Privilege {
    pub(super) const ALL: [Privilege; 16] = [
        Privilege::NotifyFinish,
        Privilege::NotifyError,
        Privilege::NotifyWatch,
        Privilege::NotifyShell,
        Privilege::SoundOnError,
        Privilege::SoundShell,
        Privilege::ExitStatus,
        Privilege::Processing,
        Privilege::Complete,
        Privilege::Awaiting,
        Privilege::NotifyComplete,
        Privilege::NotifyAwaiting,
        Privilege::Caffeinate,
        Privilege::TitleShell,
        Privilege::ClipboardShell,
        Privilege::MouseShell,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Privilege::NotifyFinish => "Notify on Finish",
            Privilege::NotifyError => "Notify on Error",
            Privilege::NotifyWatch => "Notify on Watch Finish",
            Privilege::NotifyShell => "Notification \u{2014} Shell Controlled",
            Privilege::SoundOnError => "Sound on Error Exit",
            Privilege::SoundShell => "Sound \u{2014} Shell Controlled",
            Privilege::ExitStatus => "Badge Exit Status",
            Privilege::Processing => "Badge While Processing",
            Privilege::Complete => "Badge When Task Completes",
            Privilege::Awaiting => "Badge When Awaiting Input",
            Privilege::NotifyComplete => "Notify When Task Completes",
            Privilege::NotifyAwaiting => "Notify When Awaiting Input",
            Privilege::Caffeinate => "Prevent Sleep While Processing",
            Privilege::TitleShell => "Title \u{2014} Shell Controlled",
            Privilege::ClipboardShell => "Clipboard \u{2014} Shell Controlled",
            Privilege::MouseShell => "Mouse \u{2014} Shell Controlled",
        }
    }

    pub(super) fn field(self, b: &mut motty_theme::Badges) -> &mut bool {
        match self {
            Privilege::NotifyFinish => &mut b.notify_finish,
            Privilege::NotifyError => &mut b.notify_error,
            Privilege::NotifyWatch => &mut b.notify_watch,
            Privilege::NotifyShell => &mut b.notify_shell,
            Privilege::SoundOnError => &mut b.sound_on_error,
            Privilege::SoundShell => &mut b.sound_shell,
            Privilege::ExitStatus => &mut b.exit_status,
            Privilege::Processing => &mut b.agent_processing,
            Privilege::Complete => &mut b.agent_complete,
            Privilege::Awaiting => &mut b.agent_awaiting,
            Privilege::NotifyComplete => &mut b.notify_complete,
            Privilege::NotifyAwaiting => &mut b.notify_awaiting,
            Privilege::Caffeinate => &mut b.caffeinate,
            Privilege::TitleShell => &mut b.title_shell,
            Privilege::ClipboardShell => &mut b.clipboard_shell,
            Privilege::MouseShell => &mut b.mouse_shell,
        }
    }

    pub(super) fn get(self, b: &motty_theme::Badges) -> bool {
        let mut b = *b;
        *self.field(&mut b)
    }
}

/// The speaker and bell on Notifications & Privileges: whether programs may
/// ring the bell, and whether anything may post a banner.
pub(super) fn privilege_symbols(b: &motty_theme::Badges) -> Vec<&'static str> {
    let sound = b.sound_shell || b.sound_on_error;
    let banner = b.notify_shell || b.notify_finish || b.notify_error;
    vec![
        if sound {
            "speaker.wave.2"
        } else {
            "speaker.slash"
        },
        if banner { "bell" } else { "bell.slash" },
    ]
}

/// `~/code/motty/`: a folder as the title menu shows it.
fn folder_label(path: &Path) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let shown = match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    };
    if shown.ends_with('/') {
        shown
    } else {
        format!("{shown}/")
    }
}

thread_local! {
    static LAST_SIDEBAR: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// The tabs panel as last toggled in any window: `auto-hide-tabs-panel =
/// default` opens new windows that way.
pub(super) fn last_sidebar() -> Option<bool> {
    LAST_SIDEBAR.with(|l| l.get())
}

pub(super) fn note_sidebar(visible: bool) {
    LAST_SIDEBAR.with(|l| l.set(Some(visible)));
}

impl Workspace {
    /// `auto-hide-tabs-panel = auto`: hidden with one tab, shown with
    /// more, as tabs open and close (the toggle holds until then).
    pub(super) fn auto_hide_sidebar(&mut self) {
        if self.config.auto_hide_tabs_panel != motty_theme::AutoHide::Auto {
            return;
        }
        let count = self.tabs.len();
        if self.auto_hide_count != Some(count) {
            self.auto_hide_count = Some(count);
            self.sidebar_visible = count > 1 || self.inbox;
        }
    }
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

    fn status(&mut self, label: &str, symbols: Vec<&'static str>, items: Vec<(Item, Pick)>) {
        let mut entries = Vec::new();
        for (item, pick) in items {
            entries.push(item.into());
            self.picks.push(pick);
        }
        self.entries.push(Entry::Status {
            label: label.to_owned().into(),
            symbols,
            entries,
        });
    }

    fn separator(&mut self) {
        self.entries.push(Entry::Separator);
    }
}

impl Workspace {
    /// Notifications & Privileges for `tab`, as the title menu and the
    /// tab's context menu show it.
    fn privileges_menu(&self, tab: &super::Tab, m: &mut Menu) {
        let current = tab.privileges.unwrap_or(self.config.badges);
        let mut privileges: Vec<(Item, Pick)> = Privilege::ALL
            .into_iter()
            .map(|p| {
                (
                    Item::new(p.label()).checked(p.get(&current)),
                    Pick::Privilege(p),
                )
            })
            .collect();
        privileges.push((
            Item::new("Use Settings Defaults").enabled(tab.privileges.is_some()),
            Pick::DefaultPrivileges,
        ));
        m.status(
            "Notifications & Privileges",
            privilege_symbols(&current),
            privileges,
        );
    }

    /// Right-click in the gap below tab `ix`: a divider there.
    pub(super) fn show_gap_menu(
        &mut self,
        ix: usize,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        native_menu::popup(
            vec![Item::new("Insert Divider").into()],
            at,
            None,
            window,
            cx,
            move |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.run_tab_pick(ix, Pick::Divider(true), window, cx)
                });
            },
        );
    }

    /// Right-click on a divider.
    fn show_divider_menu(
        &mut self,
        ix: usize,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picks = [Pick::RenameDivider, Pick::Divider(false)];
        let this = cx.entity().downgrade();
        native_menu::popup(
            vec![
                Item::new("Rename Divider…").into(),
                Item::new("Remove Divider").into(),
            ],
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

    /// The divider below tab `ix`: a hairline, after its label when it has
    /// one. Double-click to edit the label.
    pub(super) fn render_divider(&self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        use super::{DIVIDER_INSET, DIVIDER_SPACE, TAB_GAP};
        let t = &self.theme;
        let line = div()
            .flex_1()
            .h(px(1.))
            .bg(motty_render::hsla(super::native_label(t, 0.1, 0.12)));
        let label = self
            .tabs
            .get(ix)
            .map(|t| t.divider_label.clone())
            .unwrap_or_default();
        let renaming = self.renaming.as_ref().filter(|r| r.divider && r.ix == ix);
        // The row is taller than the line, so it's easy to hit; the
        // margins keep the line where it was.
        const PAD: f32 = 3.;
        div()
            .id(("divider", ix))
            .mt(px(TAB_GAP + DIVIDER_SPACE - PAD))
            .mb(px(DIVIDER_SPACE - PAD))
            .py(px(PAD))
            .mx(px(DIVIDER_INSET))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(11.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(motty_render::hsla(t.token.tertiary))
            .map(|d| match renaming {
                Some(rename) => d.child(self.rename_field(rename, cx)),
                None if label.is_empty() => d.child(line),
                None => d
                    .child(div().flex_none().max_w(px(160.)).truncate().child(label))
                    .child(line),
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    if event.click_count == 2 {
                        this.start_divider_rename(ix, window, cx);
                    }
                }),
            )
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.show_divider_menu(ix, event.position, window, cx);
                }),
            )
            .into_any_element()
    }

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
        if tab.terms().is_some() {
            self.privileges_menu(tab, &mut m);
        }
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

    /// A click on the window's title: Otty's title menu, with the tab's
    /// name field on top, under the title.
    pub(super) fn show_title_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.active;
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let focus = window
            .focused(cx)
            .unwrap_or_else(|| self.focus_handle.clone());
        let key = |action: &dyn gpui::Action| {
            window
                .highest_precedence_binding_for_action_in(action, &focus)
                .and_then(|b| b.keystrokes().first().map(|k| k.inner().clone()))
        };
        let act = |label: &str, action: Box<dyn gpui::Action>| {
            let keystroke = key(&*action);
            (
                Item::new(label.to_owned()).keystroke(keystroke),
                Pick::Action(action),
            )
        };
        let view = tab.terms().map(|t| t.focused.clone());
        let cwd = view.as_ref().and_then(|v| v.read(cx).cwd.clone());

        let mut m = Menu::new();
        m.entries.push(Entry::Name(NameField {
            text: tab
                .name
                .as_ref()
                .map(|n| n.text.clone())
                .unwrap_or_default(),
            prefix: tab.name.as_ref().is_some_and(|n| n.prefix),
        }));
        m.separator();
        if let Some(cwd) = &cwd {
            m.entries.push(Entry::Header("WORKING DIRECTORY".into()));
            m.entries.push(Entry::Info {
                label: folder_label(cwd).into(),
                symbol: "folder.fill",
            });
            let (item, pick) = act("Copy Path", Box::new(crate::actions::CopyPath));
            m.item(item, pick);
            let (item, pick) = act("Reveal in Finder", Box::new(crate::actions::RevealInFinder));
            m.item(item, pick);
            let apps = super::open_with_apps(&self.config.open_with_apps);
            m.sub(
                "Open in",
                apps.into_iter()
                    .map(|(name, bundle)| (Item::new(name), Pick::OpenIn(bundle, cwd.clone())))
                    .collect(),
            );
            m.separator();
            // Otty leaves Git out of a folder that isn't a repository.
            if let Some((branch, web)) = crate::menus::git(cx) {
                let mut git = Vec::new();
                {
                    let typed = |label: &str, line: &str, run: bool| {
                        (
                            Item::new(label.to_owned()),
                            Pick::Action(Box::new(crate::actions::TypeCommand::new(line, run))),
                        )
                    };
                    git.push((Item::new(branch).enabled(false), Pick::Nothing));
                    git.push(typed("Commit…", "git commit -m ", false));
                    git.push(typed("Create Branch…", "git switch -c ", false));
                    git.push(typed("Fetch…", "git fetch", true));
                    git.push(typed("Pull…", "git pull", true));
                    git.push(typed("Push…", "git push", true));
                    if let Some(url) = web {
                        git.push((
                            Item::new("Open Remote URL"),
                            Pick::Action(Box::new(crate::actions::OpenUrl { url })),
                        ));
                    }
                }
                m.sub("Git", git);
            }
            m.separator();
        }
        if tab.terms().is_some() {
            self.privileges_menu(tab, &mut m);
            m.separator();
            if self.focused_session(cx).is_some() {
                let (item, pick) = act(
                    "Open History View",
                    Box::new(crate::actions::ViewSessionHistory),
                );
                m.item(item, pick);
                m.separator();
            }
            // A split tab lists its panes, the focused one checked.
            if let Some(terms) = tab.terms()
                && terms.root.leaves().len() > 1
            {
                let panes = terms
                    .root
                    .leaves()
                    .into_iter()
                    .map(|pane| {
                        let v = pane.read(cx);
                        let program = v.process.clone().unwrap_or_else(|| "shell".into());
                        let label = format!("{program} — {}", v.display_title());
                        (
                            Item::new(label).checked(*pane == terms.focused),
                            Pick::FocusPane(pane.clone()),
                        )
                    })
                    .collect();
                m.sub("Panes", panes);
            }
            use crate::actions::*;
            m.sub(
                "Split View",
                vec![
                    act("Split Right", Box::new(SplitRight)),
                    act("Split Left", Box::new(SplitLeft)),
                    act("Split Down", Box::new(SplitDown)),
                    act("Split Up", Box::new(SplitUp)),
                    act("Equalize Splits", Box::new(EqualizeSplits)),
                    act("Zoom Split", Box::new(ZoomSplit)),
                ],
            );
        }
        let mut about = Vec::new();
        if let Some(v) = &view {
            let v = v.read(cx);
            about.push((
                Item::new(format!("Pane {}", v.id)).enabled(false),
                Pick::Nothing,
            ));
            about.push((
                Item::new(v.process.clone().unwrap_or_else(|| "shell".into())).enabled(false),
                Pick::Nothing,
            ));
            about.push((
                Item::new("Copy Pane ID"),
                Pick::Action(Box::new(crate::actions::CopyText {
                    text: v.id.to_string(),
                })),
            ));
        }
        about.push((Item::new("About This Tab…"), Pick::About));
        m.sub("About this Pane", about);
        m.separator();
        {
            use crate::actions::*;
            for (item, pick) in [
                act("Find", Box::new(Find)),
                act("Find in All Tabs", Box::new(GlobalSearch)),
                act("Jump to", Box::new(JumpTo)),
                act("Command Palette", Box::new(CommandPalette)),
            ] {
                m.item(item, pick);
            }
        }

        // Under the title, its left edge just outside the title's.
        let title = self.title_bounds.get();
        let at = gpui::point(title.left() - px(7.), title.bottom() + px(6.));
        let picks = m.picks;
        let this = cx.entity().downgrade();
        self.title_menu_open = true;
        cx.notify();
        let dark = self.theme.meta.mode == motty_theme::Mode::Dark;
        native_menu::popup_named(
            m.entries,
            at,
            None,
            Some(dark),
            window,
            cx,
            move |picked, name, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.title_menu_open = false;
                    if let Some(name) = name
                        && let Some(tab) = this.tabs.get_mut(ix)
                    {
                        let text = name.text.trim();
                        tab.name = (!text.is_empty()).then(|| TabName {
                            text: text.to_owned(),
                            prefix: name.prefix,
                        });
                    }
                    if let Some(pick) = picked.and_then(|p| picks.into_iter().nth(p)) {
                        this.run_tab_pick(ix, pick, window, cx);
                    }
                    cx.notify();
                });
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
        // Otty's: one list in three sections, each item with its symbol.
        let mut m = Menu::new();
        m.entries.push(Entry::Header("GROUP".into()));
        for (label, symbol, g) in [
            ("No Grouping", "list.bullet", Grouping::None),
            ("By Project", "folder", Grouping::Project),
            ("By Date", "calendar", Grouping::Date),
        ] {
            let on = !self.inbox && self.grouping == g;
            m.item(Item::new(label).symbol(symbol).checked(on), Pick::Group(g));
        }
        m.item(
            Item::new("Inbox").symbol("bell").checked(self.inbox),
            Pick::Inbox,
        );
        m.separator();
        m.entries.push(Entry::Header("ORDER".into()));
        for (label, symbol, o) in [
            ("Created Time", "clock", Order::Created),
            ("Updated Time", "clock.arrow.circlepath", Order::Updated),
        ] {
            m.item(
                Item::new(label).symbol(symbol).checked(self.order == o),
                Pick::Order(o),
            );
        }
        m.separator();
        m.entries.push(Entry::Header("DIVIDER".into()));
        m.item(
            Item::new("Insert Divider")
                .symbol("plus")
                .enabled(self.grouping == Grouping::None && !self.inbox),
            Pick::Divider(true),
        );
        m.item(
            Item::new("Remove All Dividers").symbol("trash"),
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
                if !on {
                    self.tabs[ix].divider_label.clear();
                }
                cx.notify();
            }
            Pick::RenameDivider => self.start_divider_rename(ix, window, cx),
            Pick::About => self.about_tab(ix, window, cx),
            Pick::FocusPane(view) => {
                if ix == self.active {
                    self.focus_pane(view, window, cx);
                }
            }
            Pick::Close => self.close_asking(super::closing::Closing::Tab(ix), window, cx),
            Pick::Group(g) => {
                self.grouping = g;
                self.inbox = false;
                cx.notify();
            }
            Pick::Inbox => {
                self.inbox = true;
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
            Pick::OpenIn(bundle, path) => super::open_in(&bundle, &path),
            Pick::Privilege(p) => {
                let mut b = self.tabs[ix].privileges.unwrap_or(self.config.badges);
                let on = p.field(&mut b);
                *on = !*on;
                self.tabs[ix].privileges = Some(b);
                self.sync_shell_privileges(cx);
                cx.defer(crate::menus::refresh);
            }
            Pick::DefaultPrivileges => {
                self.tabs[ix].privileges = None;
                self.sync_shell_privileges(cx);
                cx.defer(crate::menus::refresh);
            }
            Pick::Nothing => {}
        }
    }

    /// Rename Tab: the card's title becomes a text field.
    pub(super) fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.start_naming(ix, false, window, cx);
    }

    /// Rename Tab, or Set Tab Prefix (`prefix`): a name shown before the
    /// title, which still follows the program.
    pub(super) fn start_naming(
        &mut self,
        ix: usize,
        prefix: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let text = match &tab.name {
            Some(n) if n.prefix == prefix => n.text.clone(),
            _ if prefix => String::new(),
            _ => tab_title(tab, cx),
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
        self.renaming = Some(Rename {
            ix,
            text,
            prefix,
            divider: false,
            focus,
        });
        cx.notify();
    }

    /// Edits the label of the divider below tab `ix` in place.
    pub(super) fn start_divider_rename(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let text = tab.divider_label.clone();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.on_focus_out(&focus, window, |this, _, window, cx| {
            this.finish_rename(true, window, cx)
        })
        .detach();
        self.renaming = Some(Rename {
            ix,
            text,
            prefix: false,
            divider: true,
            focus,
        });
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
        if keep && rename.divider {
            if let Some(tab) = self.tabs.get_mut(rename.ix) {
                tab.divider_label = rename.text.trim().to_owned();
            }
        } else if keep && let Some(tab) = self.tabs.get_mut(rename.ix) {
            let text = rename.text.trim();
            tab.name = (!text.is_empty()).then(|| TabName {
                text: text.to_owned(),
                prefix: rename.prefix,
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
            Content::Search(s) => s
                .read(cx)
                .folder()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        };
        let ids = match &tab.content {
            Content::Terminals(terms) => terms
                .root
                .leaves()
                .iter()
                .map(|v| v.read(cx).id.to_string())
                .collect::<Vec<_>>()
                .join(" "),
            Content::Editor(_) | Content::Search(_) => String::new(),
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
        self.unwatch(&tab);
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

    /// This window stops following a tab's panes; the one it moves to
    /// picks them up.
    fn unwatch(&mut self, tab: &Tab) {
        match &tab.content {
            Content::Terminals(t) => {
                for view in t.root.leaves() {
                    self.pane_subscriptions.remove(&view.entity_id());
                }
            }
            Content::Editor(e) => {
                self.pane_subscriptions.remove(&e.entity_id());
            }
            Content::Search(s) => {
                self.pane_subscriptions.remove(&s.entity_id());
            }
        }
    }

    /// Window › Merge All Windows: every other window's tabs come here and
    /// those windows close.
    pub(super) fn merge_all_windows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let here = window.window_handle();
        let others: Vec<gpui::WindowHandle<Workspace>> = cx
            .windows()
            .into_iter()
            .filter(|w| *w != here)
            .filter_map(|w| w.downcast::<Workspace>())
            .collect();
        if others.is_empty() {
            return window.play_system_bell();
        }
        let active = self.active;
        for handle in others {
            let moved = handle
                .update(cx, |other, window, _| {
                    let tabs = std::mem::take(&mut other.tabs);
                    for tab in &tabs {
                        other.unwatch(tab);
                    }
                    window.remove_window();
                    tabs
                })
                .unwrap_or_default();
            for tab in moved {
                self.adopt(MovedTab(tab), window, cx);
            }
        }
        self.activate(active, window, cx);
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
            Content::Search(s) => self.watch_search(s, window, cx),
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
            Content::Search(s) => s.read(cx).folder(),
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
