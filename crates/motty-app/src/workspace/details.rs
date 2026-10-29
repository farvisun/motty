//! The details panel (⌘⇧R): a column on the right about the focused pane,
//! mirroring the tabs panel on the left. Its tabs, picked from the strip at
//! its top: Info (working directory, open-in actions, the agent, processes,
//! listening ports), Outline (the pane's commands and prompts, or a file's
//! structure), Git (branch, changed files, a diff) and Files (a tree of the
//! working directory), then the views of your own (`details-view`). When
//! the strip can't fit its tabs they collapse into a dropdown; dragging
//! the panel's left edge sets its width.
//!
//! Geometry is measured from Otty's screenshots at 2x; the icons are drawn
//! here in the style of the system's symbols.

mod files;
mod git;
mod outline;
mod views;

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    AnyElement, ClipboardItem, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla,
    KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point, ScrollHandle, SharedString, Task,
    Window, div, prelude::*, px,
};
use motty_render::hsla;
use motty_term::ProcessInfo;
use motty_theme::{DetailsView, ViewKind};

use super::{Workspace, native_label};
use crate::actions::*;
use crate::native_menu::{self, Entry, Item};
use crate::panes::PaneItem;
use crate::terminal_view::TerminalView;
use crate::titlebar::{self, icon, stroke};
pub(crate) use git::web_url;

/// The strip's buttons: 24pt tall, 4pt from the top, from 8pt in; an
/// unselected tab is a 28pt box, the selected one a pill with its name.
const STRIP_TOP: f32 = titlebar::TOP;
const STRIP_LEFT: f32 = 8.;
const TAB_WIDTH: f32 = 28.;
const TAB_GAP: f32 = 4.;
const PILL_PAD: f32 = 7.;
/// Room the panel's toggle takes at the strip's right end.
const STRIP_RIGHT: f32 = titlebar::DETAILS_RIGHT + titlebar::BUTTON + 6.;
/// Where content starts below the window's top.
const CONTENT_TOP: f32 = 44.;
const INSET: f32 = 18.5;
const ROW_HEIGHT: f32 = 28.;
const LIST_ROW: f32 = 22.;
const TREE_ROW: f32 = 25.;
const TREE_INDENT: f32 = 18.;
const SECTION_GAP: f32 = 22.;
/// What dragging the left edge allows, and how wide the grip is.
const DETAILS_MIN: f32 = 180.;
const DETAILS_MAX: f32 = 720.;
const EDGE_GRAB: f32 = 6.;
/// The slowest refreshes run at most this often.
const GIT_INTERVAL: Duration = Duration::from_secs(3);
const PORTS_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum DetailsTab {
    #[default]
    Info,
    Outline,
    Git,
    Files,
}

impl DetailsTab {
    const ALL: [Self; 4] = [Self::Info, Self::Outline, Self::Git, Self::Files];

    fn label(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Outline => "Outline",
            Self::Git => "Git",
            Self::Files => "Files",
        }
    }

    /// Its `details-view` id.
    fn id(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Outline => "outline",
            Self::Git => "git",
            Self::Files => "files",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }
}

#[derive(Default)]
pub(super) struct Details {
    pub visible: bool,
    tab: DetailsTab,
    /// A custom view's id, when one is showing instead of `tab`.
    view: Option<String>,
    /// The folder the data below describes.
    cwd: Option<std::path::PathBuf>,
    pane: Option<gpui::EntityId>,
    processes: Vec<ProcessInfo>,
    ports: Vec<(u16, i32)>,
    ports_at: Option<Instant>,
    git: Option<git::GitSummary>,
    git_at: Option<Instant>,
    git_ui: git::GitUi,
    files: files::FileTree,
    outline: outline::OutlineState,
    views: views::Views,
    fields: [Field; 4],
    pending: Option<Task<()>>,
    scroll: ScrollHandle,
    /// The width while the left edge is dragged.
    drag: Option<f32>,
}

/// The panel's text fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldId {
    Filter,
    Commit,
    ViewName,
    ViewTarget,
}

#[derive(Default)]
struct Field {
    pub text: String,
    focus: Option<FocusHandle>,
}

impl Details {
    fn field(&mut self, id: FieldId) -> &mut Field {
        &mut self.fields[id as usize]
    }
}

impl Workspace {
    pub(super) fn details_width(&self) -> f32 {
        let border = self
            .theme
            .details_panel
            .border
            .as_ref()
            .map_or(0., |b| b.width);
        self.details_inner_width() + border
    }

    /// `details-panel-width`, or where its edge is being dragged.
    fn details_inner_width(&self) -> f32 {
        self.details
            .drag
            .or(self.config.details_panel_width)
            .unwrap_or(self.theme.details_panel.width)
    }

    /// Follows the pointer while the panel's left edge is dragged.
    pub(super) fn drag_details(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.details.drag.is_none() {
            return false;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.finish_details_drag(cx);
            return true;
        }
        let border = self
            .theme
            .details_panel
            .border
            .as_ref()
            .map_or(0., |b| b.width);
        let right = f32::from(window.viewport_size().width - event.position.x) - border;
        self.details.drag = Some(right.clamp(DETAILS_MIN, DETAILS_MAX).round());
        cx.notify();
        true
    }

    /// Saves the dragged width as `details-panel-width`, as Otty does.
    pub(super) fn finish_details_drag(&mut self, cx: &mut Context<Self>) {
        let Some(width) = self.details.drag else {
            return;
        };
        let value = toml::Value::Integer(width as i64);
        match motty_theme::config::save(&[("details-panel-width", Some(value))]) {
            Ok(_) => {
                self.config.details_panel_width = Some(width);
                cx.defer(super::reload_all);
            }
            Err(e) => tracing::warn!("couldn't save details-panel-width: {e}"),
        }
        self.details.drag = None;
        cx.notify();
    }

    pub(super) fn toggle_details(&mut self, cx: &mut Context<Self>) {
        self.details.visible = !self.details.visible;
        self.refresh_details(true, cx);
        cx.notify();
    }

    pub(super) fn show_details_tab(&mut self, tab: DetailsTab, cx: &mut Context<Self>) {
        self.details.visible = true;
        self.details.tab = tab;
        self.details.view = None;
        self.details.views.adding = None;
        self.refresh_details(true, cx);
        cx.notify();
    }

    /// Shows a tab of the strip by its `details-view` id.
    fn select_details(&mut self, id: &str, cx: &mut Context<Self>) {
        match DetailsTab::from_id(id) {
            Some(tab) => self.show_details_tab(tab, cx),
            None => {
                self.details.visible = true;
                self.details.view = Some(id.to_owned());
                self.details.views.adding = None;
                cx.notify();
            }
        }
    }

    /// The id of the tab showing.
    fn details_selected(&self) -> String {
        self.details
            .view
            .clone()
            .unwrap_or_else(|| self.details.tab.id().to_owned())
    }

    /// Re-reads what the panel shows about the focused pane: processes now,
    /// git and ports in the background (at most every few seconds unless
    /// `now`).
    pub(super) fn refresh_details(&mut self, now: bool, cx: &mut Context<Self>) {
        if !self.details.visible {
            return;
        }
        let Some(item) = self.active_panes().map(|p| p.focused.clone()) else {
            return;
        };
        let (cwd, processes) = match &item {
            PaneItem::Terminal(view) => {
                let v = view.read(cx);
                (v.cwd.clone(), v.session().processes())
            }
            other => (other.folder(cx), Vec::new()),
        };
        let builtin = self.details.view.is_none();
        if builtin && self.details.tab == DetailsTab::Outline {
            self.refresh_outline(now, cx);
        }
        let d = &mut self.details;
        let moved = d.cwd != cwd || d.pane != Some(item.entity_id());
        if moved {
            d.cwd = cwd.clone();
            d.pane = Some(item.entity_id());
            d.git = None;
            d.ports.clear();
            d.git_at = None;
            d.ports_at = None;
            d.git_ui = Default::default();
        }
        if d.processes != processes {
            d.processes = processes;
            cx.notify();
        }
        let old_root = d.files.root.clone();
        d.files.set_root(cwd.clone());
        if d.files.root != old_root && !d.field(FieldId::Filter).text.is_empty() {
            self.refilter(cx);
        }
        let d = &mut self.details;
        if d.pending.is_some() {
            return;
        }
        let due =
            |at: Option<Instant>, every| now || moved || at.is_none_or(|t| t.elapsed() >= every);
        // A program view's `${branch}` comes from here too.
        let git = ((builtin && d.tab == DetailsTab::Git || !builtin)
            && due(d.git_at, GIT_INTERVAL))
        .then(|| cwd.clone())
        .flatten();
        let pids: Vec<i32> = d.processes.iter().map(|p| p.pid).collect();
        let ports = builtin
            && d.tab == DetailsTab::Info
            && due(d.ports_at, PORTS_INTERVAL)
            && !pids.is_empty();
        if git.is_none() && !ports {
            return;
        }
        let work = cx.background_spawn(async move {
            let git = git.map(|dir| git::git_summary(&dir));
            let ports = ports.then(|| listening_ports(&pids));
            (git, ports)
        });
        d.pending = Some(cx.spawn(async move |this, cx| {
            let (git, ports) = work.await;
            this.update(cx, |this, cx| {
                let d = &mut this.details;
                d.pending = None;
                if let Some(git) = git {
                    d.git_at = Some(Instant::now());
                    if d.git != git {
                        d.git = git;
                        cx.notify();
                    }
                }
                if let Some(ports) = ports {
                    d.ports_at = Some(Instant::now());
                    if d.ports != ports {
                        d.ports = ports;
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }

    pub(super) fn render_details(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let c = PanelColors {
            text: hsla(t.token.foreground),
            secondary: hsla(t.token.secondary),
            tertiary: hsla(t.token.tertiary),
            accent: hsla(t.token.accent),
            hover: hsla(native_label(t, 0.05, 0.08)),
            selected: hsla(native_label(t, 0.08, 0.12)),
            icon: hsla(native_label(t, 0.5, 0.55)),
            danger: hsla(t.token.danger),
            warning: hsla(t.token.warning),
            success: hsla(t.token.success),
        };
        let view = self.focused_pane();
        let scroll = |body: AnyElement, font: SharedString, handle: &ScrollHandle| {
            div()
                .id("details-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(handle)
                .font_family(font)
                .text_size(px(13.))
                .pb(px(16.))
                .child(body)
                .into_any_element()
        };
        let body = if self.details.views.adding.is_some() {
            let form = self.render_add_view(window, &c, cx);
            scroll(form, self.ui_font.clone(), &self.details.scroll)
        } else if let Some(id) = self.details.view.clone() {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .font_family(self.ui_font.clone())
                .text_size(px(13.))
                .child(self.render_view(&id, window, &c, cx))
                .into_any_element()
        } else {
            let body = match self.details.tab {
                DetailsTab::Info => self.render_info(view.as_ref(), &c, cx),
                DetailsTab::Outline => self.render_outline(view.as_ref(), &c, cx),
                DetailsTab::Git => self.render_git(window, &c, cx),
                DetailsTab::Files => self.render_files(window, &c, cx),
            };
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, e: &MouseDownEvent, window, cx| {
                        this.show_panel_menu(e.position, window, cx)
                    }),
                )
                .child(scroll(body, self.ui_font.clone(), &self.details.scroll))
                .into_any_element()
        };
        let strip = self.render_details_strip(&c, cx);
        let t = &self.theme;
        let side = &t.details_panel;
        let mut panel = div()
            .id("details-panel")
            .relative()
            .w(px(self.details_width()))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(hsla(side.background))
            .child(strip)
            .child(body);
        if let Some(b) = &side.border {
            panel = panel
                .border_l(px(b.width))
                .border_color(hsla(t.border_color(b)));
        }
        panel
            .child(
                // Drag the edge to resize; it straddles the border.
                div()
                    .id("details-edge")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-EDGE_GRAB / 2.))
                    .w(px(EDGE_GRAB))
                    .occlude()
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.details.drag = Some(this.details_inner_width());
                        }),
                    ),
            )
            .into_any_element()
    }

    /// The tabs, or, when they don't fit, the showing one with a ▾ that
    /// lists them all.
    fn render_details_strip(&self, c: &PanelColors, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.details_tabs();
        let selected = self.details_selected();
        let labels: Vec<String> = tabs.iter().map(tab_label).collect();
        let need: f32 = tabs
            .iter()
            .zip(&labels)
            .map(|(t, l)| {
                if t.id == selected {
                    pill_width(l)
                } else {
                    TAB_WIDTH
                }
            })
            .sum::<f32>()
            + TAB_GAP * tabs.len().saturating_sub(1) as f32;
        let room = self.details_width() - STRIP_LEFT - STRIP_RIGHT;
        let strip = div()
            .flex_none()
            .h(px(CONTENT_TOP - 12.))
            .pt(px(STRIP_TOP))
            .pl(px(STRIP_LEFT))
            .flex()
            .items_start()
            .gap(px(TAB_GAP))
            .font_family(self.ui_font.clone())
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    this.show_strip_menu(None, e.position, window, cx)
                }),
            );
        if need > room {
            let ix = tabs.iter().position(|t| t.id == selected).unwrap_or(0);
            let Some(tab) = tabs.get(ix) else {
                return strip.into_any_element();
            };
            let custom = (tab.kind != ViewKind::Builtin).then(|| tab.id.clone());
            let max = (room - PILL_PAD * 2. - 15. - 5. - 16.).max(20.);
            let pill = div()
                .id("details-tabs-menu")
                .h(px(titlebar::BUTTON))
                .min_w_0()
                .px(px(PILL_PAD))
                .flex()
                .items_center()
                .gap(px(5.))
                .rounded(px(titlebar::RADIUS))
                .bg(c.selected)
                .child(strip_icon(tab, c.text))
                .child(
                    div()
                        .max_w(px(max))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(13.))
                        .text_color(c.text)
                        .child(labels[ix].clone()),
                )
                .child(lucide("chevron-down", 11., c.icon))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, e: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.show_tabs_menu(e.position, window, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.show_strip_menu(custom.clone(), e.position, window, cx);
                    }),
                );
            return strip.child(pill).into_any_element();
        }
        let tabs = tabs.into_iter().zip(labels).map(|(tab, label)| {
            let on = tab.id == selected;
            let color = if on { c.text } else { c.icon };
            let id = tab.id.clone();
            let custom = (tab.kind != ViewKind::Builtin).then(|| tab.id.clone());
            div()
                .id(SharedString::from(format!("details-tab-{}", tab.id)))
                .h(px(titlebar::BUTTON))
                .flex()
                .items_center()
                .rounded(px(titlebar::RADIUS))
                .map(|d| {
                    if on {
                        d.px(px(PILL_PAD))
                            .gap(px(5.))
                            .bg(c.selected)
                            .child(strip_icon(&tab, color))
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .text_size(px(13.))
                                    .text_color(color)
                                    .child(label.clone()),
                            )
                    } else {
                        d.w(px(TAB_WIDTH))
                            .justify_center()
                            .hover(|s| s.bg(c.hover))
                            .child(strip_icon(&tab, color))
                            .tooltip(move |_, cx| crate::tooltip::text(label.clone(), cx))
                    }
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.show_strip_menu(custom.clone(), e.position, window, cx);
                    }),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.select_details(&id, cx)))
        });
        strip.children(tabs).into_any_element()
    }

    /// The collapsed strip's list: every tab, then Add View….
    fn show_tabs_menu(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let tabs = self.details_tabs();
        let selected = self.details_selected();
        let mut entries: Vec<Entry> = tabs
            .iter()
            .map(|t| Item::new(tab_label(t)).checked(t.id == selected).into())
            .collect();
        entries.push(Entry::Separator);
        entries.push(Item::new("Add View…").into());
        let ids: Vec<String> = tabs.into_iter().map(|t| t.id).collect();
        let this = cx.entity().downgrade();
        native_menu::popup(entries, at, None, window, cx, move |ix, window, cx| {
            let _ = this.update(cx, |this, cx| match ids.get(ix) {
                Some(id) => this.select_details(id, cx),
                None => this.show_add_view(window, cx),
            });
        });
    }

    /// Right-click on the strip: Add View…, and for a view of your own,
    /// Configure… and Close.
    fn show_strip_menu(
        &mut self,
        custom: Option<String>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut entries: Vec<Entry> = Vec::new();
        if custom.is_some() {
            entries.push(Item::new("Configure…").into());
            entries.push(Item::new("Close").into());
            entries.push(Entry::Separator);
        }
        entries.push(Item::new("Add View…").into());
        let this = cx.entity().downgrade();
        native_menu::popup(entries, at, None, window, cx, move |ix, window, cx| {
            let _ = this.update(cx, |this, cx| match (custom, ix) {
                (Some(id), 0) => this.configure_view(&id, window, cx),
                (Some(id), 1) => this.close_view(&id, cx),
                _ => this.show_add_view(window, cx),
            });
        });
    }

    /// Right-click in the panel: Configure Open With… and Close.
    fn show_panel_menu(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let entries = vec![
            Item::new("Configure Open With…").into(),
            Entry::Separator,
            Item::new("Close Details Panel").into(),
        ];
        let this = cx.entity().downgrade();
        native_menu::popup(entries, at, None, window, cx, move |ix, window, cx| {
            let _ = this.update(cx, |this, cx| match ix {
                0 => this.configure_open_with(window, cx),
                _ => this.toggle_details(cx),
            });
        });
    }

    fn field_focus(&mut self, id: FieldId, cx: &mut Context<Self>) -> FocusHandle {
        self.details.fields[id as usize]
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone()
    }

    fn focus_field(&mut self, id: FieldId, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.field_focus(id, cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Gives focus back to the pane.
    fn refocus_pane(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.active_panes().map(|p| p.focused.clone()) {
            window.focus(&item.focus_handle(cx), cx);
        }
    }

    /// A one-line text field: the caret after the text while focused, the
    /// placeholder while empty.
    fn render_field(
        &mut self,
        id: FieldId,
        placeholder: &'static str,
        window: &mut Window,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self.field_focus(id, cx);
        let focused = focus.is_focused(window);
        // The end of a long text, where the caret is.
        let room = ((self.details_width() - 60.) / 6.5).max(8.) as usize;
        let text = left_cut(&self.details.fields[id as usize].text, room);
        let caret = div().flex_none().w(px(1.5)).h(px(14.)).bg(c.accent);
        div()
            .id(("details-field", id as usize))
            .track_focus(&focus)
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                this.on_field_key(id, e, window, cx)
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.focus_field(id, window, cx);
                }),
            )
            .h(px(24.))
            .px(px(7.))
            .flex()
            .items_center()
            .gap(px(5.))
            .rounded(px(6.))
            .bg(c.hover)
            .border_1()
            .border_color(if focused {
                c.accent
            } else {
                gpui::transparent_black()
            })
            .text_size(px(12.5))
            .when(id == FieldId::Filter, |d| {
                d.child(lucide("search", 12., c.tertiary))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .when(!text.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .whitespace_nowrap()
                                .text_color(c.text)
                                .child(text.clone()),
                        )
                    })
                    .when(focused, |d| d.child(caret))
                    .when(text.is_empty(), |d| {
                        d.child(
                            div()
                                .pl(px(1.))
                                .whitespace_nowrap()
                                .text_color(c.tertiary)
                                .child(placeholder),
                        )
                    }),
            )
            .into_any_element()
    }

    fn on_field_key(
        &mut self,
        id: FieldId,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let mut edited = false;
        if ks.modifiers.platform {
            // ⌘V pastes a line; other ⌘ keys go on to their actions.
            if ks.key != "v" {
                return;
            }
            if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                let line = text.lines().next().unwrap_or_default().to_owned();
                self.details.field(id).text.push_str(&line);
                edited = true;
            }
        }
        cx.stop_propagation();
        match ks.key.as_str() {
            _ if edited => {}
            "escape" => match id {
                FieldId::Filter if self.details.field(id).text.is_empty() => {
                    self.refocus_pane(window, cx)
                }
                FieldId::Filter => {
                    self.details.field(id).text.clear();
                    self.refilter(cx);
                }
                FieldId::Commit => {
                    self.details.git_ui.committing = false;
                    self.details.field(id).text.clear();
                    self.refocus_pane(window, cx);
                }
                FieldId::ViewName | FieldId::ViewTarget => {
                    self.details.views.adding = None;
                    self.refocus_pane(window, cx);
                }
            },
            "enter" => match id {
                FieldId::Filter => self.open_selected_file(window, cx),
                FieldId::Commit => self.commit(cx),
                FieldId::ViewName | FieldId::ViewTarget => self.add_view(cx),
            },
            "up" | "down" if id == FieldId::Filter => {
                self.move_file_selection(ks.key == "down", cx)
            }
            "tab" if id == FieldId::ViewName => self.focus_field(FieldId::ViewTarget, window, cx),
            "tab" if id == FieldId::ViewTarget => self.focus_field(FieldId::ViewName, window, cx),
            _ => edited = super::picker::edit_query(&mut self.details.field(id).text, ks),
        }
        if edited && id == FieldId::Filter {
            self.refilter(cx);
        }
        cx.notify();
    }

    fn render_info(
        &self,
        view: Option<&Entity<TerminalView>>,
        c: &PanelColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let d = &self.details;
        let mut col = div().flex().flex_col().pt(px(12.));
        if let Some(cwd) = &d.cwd {
            col = col.child(header("Working Directory", c)).child(
                div()
                    .px(px(INSET))
                    .pb(px(10.))
                    .font_family(self.mono_font.clone())
                    .text_color(c.text)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(tilde(cwd)),
            );
            let target = view.cloned();
            let mut actions: Vec<(SharedString, Icon, Run)> = vec![
                (
                    "Copy Path".into(),
                    Icon::Copy,
                    dispatch(target.clone(), || Box::new(CopyPath)),
                ),
                (
                    "Reveal in Finder".into(),
                    Icon::Folder,
                    dispatch(target.clone(), || Box::new(RevealInFinder)),
                ),
            ];
            for (name, bundle) in open_with_apps(&self.config.open_with_apps) {
                let path = cwd.clone();
                actions.push((
                    format!("Open in {name}").into(),
                    Icon::External,
                    Box::new(move |_, _| open_in(&bundle, &path)),
                ));
            }
            col = col.child(action_rows("details-action", actions, c));
        }
        if let Some(view) = view {
            col = col.child(render_agent(view, c, cx));
        }
        let now = SystemTime::now();
        col = col
            .child(div().h(px(SECTION_GAP)))
            .child(header("Process", c));
        for p in &d.processes {
            let age = p
                .started
                .and_then(|s| now.duration_since(s).ok())
                .map(short_duration)
                .unwrap_or_default();
            col = col.child(
                div()
                    .px(px(INSET))
                    .h(px(LIST_ROW))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_none().size(px(8.)).rounded_full().bg(c.success))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(c.text)
                            .child(p.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.5))
                            .text_color(c.tertiary)
                            .child(p.pid.to_string()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(c.tertiary)
                            .child(age),
                    ),
            );
        }
        col = col
            .child(div().h(px(SECTION_GAP)))
            .child(header("Ports", c));
        if d.ports.is_empty() {
            col = col.child(
                div()
                    .px(px(INSET))
                    .text_color(c.tertiary)
                    .child("No listening ports"),
            );
        }
        for (ix, &(port, pid)) in d.ports.iter().enumerate() {
            let url = format!("http://localhost:{port}");
            let name = d
                .processes
                .iter()
                .find(|p| p.pid == pid)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            col = col.child(
                div()
                    .id(("details-port", ix))
                    .mx(px(INSET - 8.))
                    .px(px(8.))
                    .h(px(LIST_ROW + 2.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(c.hover))
                    .child(
                        div()
                            .flex_1()
                            .text_color(c.accent)
                            .child(format!(":{port}")),
                    )
                    .child(div().text_size(px(11.5)).text_color(c.tertiary).child(name))
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        col.into_any_element()
    }
}

/// The Info tab's Agent group, for a pane an agent reported from: its
/// name, whether it's working or waiting, its step and its session.
fn render_agent(view: &Entity<TerminalView>, c: &PanelColors, cx: &mut gpui::App) -> AnyElement {
    use crate::agents::{Badge, sessions};
    let Some(status) = view.read(cx).status.clone() else {
        return div().into_any_element();
    };
    let Some(name) = status.agent.clone() else {
        return div().into_any_element();
    };
    let (state, color) = if status.in_turn {
        ("Working", c.accent)
    } else {
        match status.badge {
            Some(Badge::Awaiting) => ("Waiting for you", c.warning),
            Some(Badge::Error) => ("Failed", c.danger),
            _ => ("Waiting for a prompt", c.success),
        }
    };
    let mut col = div()
        .flex()
        .flex_col()
        .child(div().h(px(SECTION_GAP - 10.)))
        .child(header("Agent", c))
        .child(
            div()
                .px(px(INSET))
                .h(px(LIST_ROW))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_none().size(px(8.)).rounded_full().bg(color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c.text)
                        .child(name),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(c.tertiary)
                        .child(state),
                ),
        );
    if let Some(message) = status.message.filter(|m| !m.trim().is_empty()) {
        col = col.child(
            div()
                .px(px(INSET))
                .pl(px(INSET + 16.))
                .pb(px(4.))
                .text_size(px(12.))
                .text_color(c.secondary)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(message.lines().next().unwrap_or_default().to_owned()),
        );
    }
    if let Some(session) = status.session {
        col = col.child(
            div()
                .px(px(INSET))
                .pl(px(INSET + 16.))
                .h(px(LIST_ROW))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .child(div().flex_none().text_color(c.tertiary).child("Session"))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(c.secondary)
                        .child(session.chars().take(8).collect::<String>()),
                ),
        );
        let mut actions: Vec<(SharedString, Icon, Run)> = Vec::new();
        let id = session.clone();
        actions.push((
            "Copy Session ID".into(),
            Icon::Copy,
            Box::new(move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(id.clone()))),
        ));
        if let Some(resume) = status
            .agent_id
            .as_deref()
            .and_then(|a| sessions::resume_command(a, &session))
        {
            actions.push((
                "Copy Resume Command".into(),
                Icon::Copy,
                Box::new(move |_, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(resume.clone()))
                }),
            ));
        }
        col = col.child(action_rows("details-agent-action", actions, c));
    }
    col.into_any_element()
}

/// Info's accent rows that do something.
fn action_rows(
    id: &'static str,
    actions: Vec<(SharedString, Icon, Run)>,
    c: &PanelColors,
) -> impl IntoElement {
    let mut col = div().flex().flex_col();
    for (ix, (label, glyph, run)) in actions.into_iter().enumerate() {
        col = col.child(
            div()
                .id((id, ix))
                .mx(px(INSET - 8.))
                .px(px(8.))
                .h(px(ROW_HEIGHT))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(6.))
                .text_color(c.accent)
                .hover(|s| s.bg(c.hover))
                .child(glyph.draw(c.accent))
                .child(label)
                .on_click(move |_, window, cx| run(window, cx)),
        );
    }
    col
}

/// A tab's name in the strip.
fn tab_label(view: &DetailsView) -> String {
    match (&view.title, DetailsTab::from_id(&view.id)) {
        (Some(title), _) => title.clone(),
        (None, Some(tab)) => tab.label().to_owned(),
        (None, None) => view.id.clone(),
    }
}

/// The selected tab's pill: padding, icon, gap and its name at 13pt.
fn pill_width(label: &str) -> f32 {
    PILL_PAD * 2. + 15. + 5. + label.chars().count() as f32 * 7.2
}

/// A tab's icon: a built-in's own, a view's named icon or emoji, or a
/// terminal or globe.
fn strip_icon(view: &DetailsView, color: Hsla) -> AnyElement {
    if view.kind == ViewKind::Builtin
        && let Some(tab) = DetailsTab::from_id(&view.id)
    {
        return tab_icon(tab, color);
    }
    let named = view
        .icon
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    if let Some(name) = named {
        if let Some(path) = crate::tab_icons::path(name) {
            return gpui::svg()
                .path(path)
                .flex_none()
                .size(px(13.))
                .text_color(color)
                .into_any_element();
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return div()
                .flex_none()
                .w(px(15.))
                .flex()
                .justify_center()
                .text_size(px(11.))
                .child(name.to_owned())
                .into_any_element();
        }
    }
    let fallback = if view.kind == ViewKind::Web {
        "globe"
    } else {
        "terminal"
    };
    lucide(fallback, 13., color)
}

/// One of the bundled Lucide icons.
pub(super) fn lucide(name: &str, size: f32, color: Hsla) -> AnyElement {
    gpui::svg()
        .path(format!("icons/{name}.svg"))
        .flex_none()
        .size(px(size))
        .text_color(color)
        .into_any_element()
}

/// A small icon button with a tooltip.
fn tool_button_svg(
    id: impl Into<gpui::ElementId>,
    icon: &'static str,
    tip: &'static str,
    c: &PanelColors,
) -> gpui::Stateful<gpui::Div> {
    let hover = c.selected;
    div()
        .id(id)
        .flex_none()
        .size(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .hover(move |s| s.bg(hover))
        .child(lucide(icon, 12., c.icon))
        .tooltip(move |_, cx| crate::tooltip::text(tip, cx))
        // Not a click on the row it's in.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// The last `max` characters of `text`, after an ellipsis.
fn left_cut(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(n - max + 1).collect();
    format!("…{tail}")
}

/// What an Info action does when clicked.
type Run = Box<dyn Fn(&mut Window, &mut gpui::App)>;

struct PanelColors {
    text: Hsla,
    secondary: Hsla,
    tertiary: Hsla,
    accent: Hsla,
    hover: Hsla,
    selected: Hsla,
    icon: Hsla,
    danger: Hsla,
    warning: Hsla,
    success: Hsla,
}

fn header(text: &'static str, c: &PanelColors) -> impl IntoElement {
    header_text(text.to_owned(), c)
}

fn header_text(text: String, c: &PanelColors) -> impl IntoElement {
    div()
        .px(px(INSET))
        .pb(px(6.))
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c.tertiary)
        .child(text)
}

fn tool_button(id: &'static str, glyph: Icon, c: &PanelColors) -> gpui::Stateful<gpui::Div> {
    let hover = c.hover;
    div()
        .id(id)
        .flex_none()
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .hover(move |s| s.bg(hover))
        .child(glyph.draw(c.icon))
}

/// Runs `action` in the pane, as its key would.
fn dispatch(
    target: Option<Entity<TerminalView>>,
    action: impl Fn() -> Box<dyn gpui::Action> + 'static,
) -> Run {
    Box::new(move |window, cx| {
        if let Some(target) = &target {
            let focus = target.focus_handle(cx);
            focus.dispatch_action(&*action(), window, cx);
        }
    })
}

/// `open-with-app = "Name|bundle.id"` entries, else the editors found in
/// /Applications.
pub(crate) fn open_with_apps(configured: &[String]) -> Vec<(String, String)> {
    if !configured.is_empty() {
        return configured
            .iter()
            .filter_map(|e| {
                let (name, bundle) = e.split_once('|')?;
                Some((name.trim().to_owned(), bundle.trim().to_owned()))
            })
            .collect();
    }
    [
        ("VS Code", "Visual Studio Code", "com.microsoft.VSCode"),
        ("Cursor", "Cursor", "com.todesktop.230313mzl4w4u92"),
        ("Zed", "Zed", "dev.zed.Zed"),
        ("Xcode", "Xcode", "com.apple.dt.Xcode"),
    ]
    .into_iter()
    .filter(|(_, app, _)| Path::new(&format!("/Applications/{app}.app")).exists())
    .map(|(name, _, bundle)| (name.to_owned(), bundle.to_owned()))
    .collect()
}

pub(crate) fn open_in(bundle: &str, path: &Path) {
    let result = std::process::Command::new("/usr/bin/open")
        .arg("-b")
        .arg(bundle)
        .arg(path)
        .spawn();
    if let Err(e) = result {
        tracing::warn!("couldn't open {} with {bundle}: {e}", path.display());
    }
}

/// The installed apps that can open `target` (a path or a URL), the
/// default first, then `extra` (the configured editors) not already there;
/// as (name, bundle id).
pub(crate) fn apps_for(target: &str, extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut apps = Vec::new();
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSBundle, NSString, NSURL};
        let text = NSString::from_str(target);
        let url = if target.contains("://") {
            NSURL::URLWithString(&text)
        } else {
            Some(NSURL::fileURLWithPath(&text))
        };
        if let Some(url) = url {
            let workspace = NSWorkspace::sharedWorkspace();
            for app in workspace.URLsForApplicationsToOpenURL(&url).iter() {
                let Some(id) = NSBundle::bundleWithURL(&app).and_then(|b| b.bundleIdentifier())
                else {
                    continue;
                };
                let Some(name) = app
                    .lastPathComponent()
                    .map(|n| n.to_string().trim_end_matches(".app").to_owned())
                else {
                    continue;
                };
                let id = id.to_string();
                if !apps.iter().any(|(_, b)| *b == id) {
                    apps.push((name, id));
                }
            }
        }
    }
    for (name, bundle) in extra {
        if !apps.iter().any(|(_, b)| b == bundle) {
            apps.push((name.clone(), bundle.clone()));
        }
    }
    apps
}

pub(super) fn tilde(path: &Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rel) if rel.as_os_str().is_empty() => "~".into(),
        Some(rel) => format!("~/{}", rel.display()),
        None => path.display().to_string(),
    }
}

/// `34s`, `5m`, `2h`, `3d`.
fn short_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3_600 => format!("{}m", s / 60),
        3_600..86_400 => format!("{}h", s / 3_600),
        _ => format!("{}d", s / 86_400),
    }
}

/// The end of a long path, which says more than its start.
fn left_ellipsis(path: &str, max: usize) -> String {
    let n = path.chars().count();
    if n <= max {
        return path.to_owned();
    }
    let tail: String = path.chars().skip(n - (max - 1)).collect();
    match tail.find('/') {
        Some(i) if i < tail.len() - 1 => format!("…{}", &tail[i..]),
        _ => format!("…{tail}"),
    }
}

/// TCP ports the processes listen on, as (port, pid).
pub(crate) fn listening_ports(pids: &[i32]) -> Vec<(u16, i32)> {
    let list = pids
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let Ok(out) = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-Fpn", "-p", &list])
        .output()
    else {
        return Vec::new();
    };
    parse_lsof(&String::from_utf8_lossy(&out.stdout))
}

/// The current user's other listening TCP ports, with their process
/// names: Shell › Ports' Other group. Root-owned system services, Motty
/// itself and `listed` are left out. Nothing is connected to.
pub(crate) fn other_listening_ports(listed: &[i32]) -> Vec<(u16, String)> {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() }.to_string();
    let Ok(out) = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-Fpcn", "-u", &uid])
        .output()
    else {
        return Vec::new();
    };
    let me = std::process::id() as i32;
    parse_lsof_named(&String::from_utf8_lossy(&out.stdout))
        .into_iter()
        .filter(|(_, pid, _)| *pid != me && !listed.contains(pid) && !system_process(*pid))
        .map(|(port, _, name)| (port, name))
        .collect()
}

/// A macOS service (AirPlay's ControlCenter and the like), by where its
/// executable is.
fn system_process(pid: i32) -> bool {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is as large as the call is told it is.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return true;
    }
    let path = String::from_utf8_lossy(&buf[..n as usize]);
    ["/System/", "/usr/libexec/", "/usr/sbin/", "/sbin/"]
        .iter()
        .any(|p| path.starts_with(p))
}

fn parse_lsof_named(out: &str) -> Vec<(u16, i32, String)> {
    let mut pid = 0;
    let mut name = String::new();
    let mut ports: Vec<(u16, i32, String)> = Vec::new();
    for line in out.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().unwrap_or(0);
        } else if let Some(c) = line.strip_prefix('c') {
            name = c.to_owned();
        } else if let Some(addr) = line.strip_prefix('n')
            && let Some(port) = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok())
            && !ports.iter().any(|(p, _, _)| *p == port)
        {
            ports.push((port, pid, name.clone()));
        }
    }
    ports.sort_unstable();
    ports
}

fn parse_lsof(out: &str) -> Vec<(u16, i32)> {
    let mut pid = 0;
    let mut ports = Vec::new();
    for line in out.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().unwrap_or(0);
        } else if let Some(addr) = line.strip_prefix('n')
            && let Some(port) = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok())
            && !ports.iter().any(|&(p, _)| p == port)
        {
            ports.push((port, pid));
        }
    }
    ports.sort_unstable();
    ports
}

#[derive(Clone, Copy)]
enum Icon {
    Copy,
    Folder,
    External,
    File,
    ChevronRight,
    ChevronDown,
    Up,
    Eye,
    EyeSlash,
}

/// A circle from four quarter arcs (k: the bezier handle length).
fn circle(
    p: &mut gpui::PathBuilder,
    at: &dyn Fn(f32, f32) -> gpui::Point<gpui::Pixels>,
    cx: f32,
    cy: f32,
    r: f32,
) {
    let k = r * 0.5523;
    p.move_to(at(cx + r, cy));
    p.cubic_bezier_to(at(cx, cy + r), at(cx + r, cy + k), at(cx + k, cy + r));
    p.cubic_bezier_to(at(cx - r, cy), at(cx - k, cy + r), at(cx - r, cy + k));
    p.cubic_bezier_to(at(cx, cy - r), at(cx - r, cy - k), at(cx - k, cy - r));
    p.cubic_bezier_to(at(cx + r, cy), at(cx + k, cy - r), at(cx + r, cy - k));
    p.close();
}

fn rounded_rect(
    p: &mut gpui::PathBuilder,
    at: &dyn Fn(f32, f32) -> gpui::Point<gpui::Pixels>,
    (x, y, w, h): (f32, f32, f32, f32),
    r: f32,
) {
    let k = r * 0.4477;
    p.move_to(at(x + r, y));
    p.line_to(at(x + w - r, y));
    p.cubic_bezier_to(at(x + w, y + r), at(x + w - k, y), at(x + w, y + k));
    p.line_to(at(x + w, y + h - r));
    p.cubic_bezier_to(
        at(x + w - r, y + h),
        at(x + w, y + h - k),
        at(x + w - k, y + h),
    );
    p.line_to(at(x + r, y + h));
    p.cubic_bezier_to(at(x, y + h - r), at(x + k, y + h), at(x, y + h - k));
    p.line_to(at(x, y + r));
    p.cubic_bezier_to(at(x + r, y), at(x, y + k), at(x + k, y));
    p.close();
}

fn folder_shape(p: &mut gpui::PathBuilder, at: &dyn Fn(f32, f32) -> gpui::Point<gpui::Pixels>) {
    p.move_to(at(1.5, 4.));
    p.line_to(at(1.5, 3.));
    p.cubic_bezier_to(at(2.5, 2.), at(1.5, 2.4), at(1.9, 2.));
    p.line_to(at(5.6, 2.));
    p.line_to(at(7., 3.5));
    p.line_to(at(12.5, 3.5));
    p.cubic_bezier_to(at(13.5, 4.5), at(13.1, 3.5), at(13.5, 3.9));
    p.line_to(at(13.5, 11.));
    p.cubic_bezier_to(at(12.5, 12.), at(13.5, 11.6), at(13.1, 12.));
    p.line_to(at(2.5, 12.));
    p.cubic_bezier_to(at(1.5, 11.), at(1.9, 12.), at(1.5, 11.6));
    p.close();
}

impl Icon {
    fn draw(self, color: Hsla) -> AnyElement {
        const W: f32 = 1.1;
        icon(15., 14., move |o, window| match self {
            Icon::Copy => {
                stroke(o, color, window, W, |p, at| {
                    rounded_rect(p, at, (4.5, 1.5, 8., 9.5), 1.5)
                });
                stroke(o, color, window, W, |p, at| {
                    p.move_to(at(2.5, 4.));
                    p.line_to(at(2.5, 11.));
                    p.cubic_bezier_to(at(4., 12.5), at(2.5, 11.9), at(3.1, 12.5));
                    p.line_to(at(9.5, 12.5));
                });
            }
            Icon::Folder => stroke(o, color, window, W, |p, at| folder_shape(p, at)),
            Icon::External => {
                stroke(o, color, window, W, |p, at| {
                    rounded_rect(p, at, (2., 1.5, 11., 11.), 2.5)
                });
                stroke(o, color, window, W, |p, at| {
                    p.move_to(at(5.5, 9.));
                    p.line_to(at(9.5, 5.));
                    p.move_to(at(6.2, 5.));
                    p.line_to(at(9.5, 5.));
                    p.line_to(at(9.5, 8.3));
                });
            }
            Icon::File => stroke(o, color, window, W, |p, at| {
                p.move_to(at(3., 2.5));
                p.cubic_bezier_to(at(4., 1.5), at(3., 1.9), at(3.4, 1.5));
                p.line_to(at(8.5, 1.5));
                p.line_to(at(12., 5.));
                p.line_to(at(12., 11.5));
                p.cubic_bezier_to(at(11., 12.5), at(12., 12.1), at(11.6, 12.5));
                p.line_to(at(4., 12.5));
                p.cubic_bezier_to(at(3., 11.5), at(3.4, 12.5), at(3., 12.1));
                p.close();
                p.move_to(at(8.5, 1.5));
                p.line_to(at(8.5, 5.));
                p.line_to(at(12., 5.));
            }),
            Icon::ChevronRight => stroke(o, color, window, 1.3, |p, at| {
                p.move_to(at(5.5, 3.5));
                p.line_to(at(9., 7.));
                p.line_to(at(5.5, 10.5));
            }),
            Icon::ChevronDown => stroke(o, color, window, 1.3, |p, at| {
                p.move_to(at(4., 5.5));
                p.line_to(at(7.5, 9.));
                p.line_to(at(11., 5.5));
            }),
            Icon::Up => stroke(o, color, window, 1.2, |p, at| {
                p.move_to(at(7.5, 12.));
                p.line_to(at(7.5, 2.5));
                p.move_to(at(3.5, 6.5));
                p.line_to(at(7.5, 2.5));
                p.line_to(at(11.5, 6.5));
            }),
            Icon::Eye | Icon::EyeSlash => {
                stroke(o, color, window, W, |p, at| {
                    p.move_to(at(1.5, 7.));
                    p.cubic_bezier_to(at(13.5, 7.), at(4., 2.5), at(11., 2.5));
                    p.cubic_bezier_to(at(1.5, 7.), at(11., 11.5), at(4., 11.5));
                    p.close();
                });
                stroke(o, color, window, W, |p, at| circle(p, at, 7.5, 7., 2.));
                if matches!(self, Icon::EyeSlash) {
                    stroke(o, color, window, 1.2, |p, at| {
                        p.move_to(at(2.5, 1.5));
                        p.line_to(at(12.5, 12.5));
                    });
                }
            }
        })
        .into_any_element()
    }
}

/// The strip's icons: info, outline (a bulleted list), git (two arrows
/// in a loop) and files (a folder).
fn tab_icon(tab: DetailsTab, color: Hsla) -> AnyElement {
    const W: f32 = 1.1;
    icon(15., 14., move |o, window| match tab {
        DetailsTab::Info => {
            stroke(o, color, window, W, |p, at| circle(p, at, 7.5, 7., 5.8));
            stroke(o, color, window, 1.3, |p, at| {
                p.move_to(at(7.5, 6.2));
                p.line_to(at(7.5, 10.));
            });
            stroke(o, color, window, 1.6, |p, at| {
                p.move_to(at(7.5, 3.9));
                p.line_to(at(7.5, 4.5));
            });
        }
        DetailsTab::Outline => {
            for y in [3.5, 7., 10.5] {
                stroke(o, color, window, 1.6, |p, at| {
                    p.move_to(at(2., y));
                    p.line_to(at(2.8, y));
                });
                stroke(o, color, window, W, |p, at| {
                    p.move_to(at(5., y));
                    p.line_to(at(13.5, y));
                });
            }
        }
        DetailsTab::Git => {
            stroke(o, color, window, W, |p, at| {
                p.move_to(at(2.5, 7.5));
                p.line_to(at(2.5, 5.5));
                p.cubic_bezier_to(at(4.5, 3.5), at(2.5, 4.4), at(3.4, 3.5));
                p.line_to(at(12., 3.5));
                p.move_to(at(10., 1.5));
                p.line_to(at(12., 3.5));
                p.line_to(at(10., 5.5));
            });
            stroke(o, color, window, W, |p, at| {
                p.move_to(at(12.5, 6.5));
                p.line_to(at(12.5, 8.5));
                p.cubic_bezier_to(at(10.5, 10.5), at(12.5, 9.6), at(11.6, 10.5));
                p.line_to(at(3., 10.5));
                p.move_to(at(5., 8.5));
                p.line_to(at(3., 10.5));
                p.line_to(at(5., 12.5));
            });
        }
        DetailsTab::Files => stroke(o, color, window, W, |p, at| folder_shape(p, at)),
    })
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lsof_names() {
        let out =
            "p664\ncControlCenter\nf9\nn*:7000\nf10\nn*:7000\np900\ncnode\nf3\nn127.0.0.1:5173\n";
        assert_eq!(
            parse_lsof_named(out),
            vec![
                (5173, 900, "node".to_owned()),
                (7000, 664, "ControlCenter".to_owned())
            ]
        );
    }

    #[test]
    fn lsof_ports_and_helpers() {
        let out = "p100\nf5\nn*:3000\nf6\nn[::1]:3000\np200\nf7\nn127.0.0.1:5173\n";
        assert_eq!(parse_lsof(out), [(3000, 100), (5173, 200)]);
        assert_eq!(
            left_ellipsis("docs/user/workspace/file-pane.md", 20),
            "…/file-pane.md"
        );
        assert_eq!(short_duration(Duration::from_secs(125)), "2m");
        assert_eq!(left_cut("abcdef", 4), "…def");
        assert_eq!(left_cut("abc", 4), "abc");
    }

    #[test]
    fn strip_labels() {
        assert_eq!(tab_label(&DetailsView::builtin("git")), "Git");
        let mut v = DetailsView::builtin("docker");
        v.kind = ViewKind::Tui;
        assert_eq!(tab_label(&v), "docker");
        v.title = Some("Docker".into());
        assert_eq!(tab_label(&v), "Docker");
        assert!(pill_width("Outline") > TAB_WIDTH);
    }
}
