//! Command palette (⇧⌘P): every action, with or without a shortcut, found
//! by typing. ↩ runs the selected one in the pane that had focus, ⌘↩ runs it
//! and keeps the palette open, Esc closes it. Otty's 560pt card, see
//! [`super::picker`].

use gpui::{
    Action, ClipboardItem, Context, Entity, FocusHandle, KeyDownEvent, Keystroke, MouseButton,
    ScrollHandle, SharedString, Window, div, prelude::*, px,
};
use motty_render::hsla;
use motty_theme::{Mode, Rgba};

use super::Workspace;
use super::picker::{self, Colors, ROW_GAP, ROW_HEIGHT, ROW_INSET, ROW_RADIUS, edit_query, score};
use crate::actions::*;
use crate::terminal_view::{PaneMode, TerminalView};

const WIDTH: f32 = 560.;
/// Where row text starts (Otty's glyphs start at 42.5).
const TEXT_LEFT: f32 = 41.5;
const LIST_MAX_HEIGHT: f32 = 320.;
const KEYCAP_WIDTH: f32 = 21.;
const KEYCAP_HEIGHT: f32 = 20.;
const CHECK: u32 = 0x2b5b38;

pub(super) struct Palette {
    focus: FocusHandle,
    query: String,
    /// Index into the filtered commands.
    selected: usize,
    /// The pane commands run in; None over an editor tab.
    target: Option<Entity<TerminalView>>,
    /// Where focus goes back to, and where commands are dispatched from.
    back: FocusHandle,
    scroll: ScrollHandle,
}

struct Command {
    group: &'static str,
    title: SharedString,
    action: Box<dyn Action>,
    checked: bool,
}

impl Command {
    fn new(group: &'static str, title: impl Into<SharedString>, action: impl Action) -> Self {
        Self {
            group,
            title: title.into(),
            action: Box::new(action),
            checked: false,
        }
    }

    fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
}

/// Everything the palette offers, in display order. `target` is the
/// focused terminal pane; without one (an editor tab) its commands are left
/// out.
fn commands(ws: &Workspace, target: Option<&TerminalView>) -> Vec<Command> {
    let mut list = Vec::new();
    if target.is_some_and(|t| t.cwd.is_some()) {
        list.extend([
            Command::new("Working Directory", "Copy Path", CopyPath),
            Command::new("Working Directory", "Reveal in Finder", RevealInFinder),
        ]);
    }
    list.extend([
        Command::new("View", "Toggle Tabs Panel", ToggleTabsPanel).checked(ws.sidebar_visible),
        Command::new("View", "Toggle Inbox", ToggleInbox).checked(ws.inbox),
        Command::new("View", "Toggle Details Panel", ToggleDetailsPanel)
            .checked(ws.details.visible),
        Command::new("View", "Details: Info", DetailsInfo),
        Command::new("View", "Details: Outline", DetailsOutline),
        Command::new("View", "Details: Git", DetailsGit),
        Command::new("View", "Details: Files", DetailsFiles),
        Command::new("View", "Toggle Full Screen", ToggleFullScreen),
        Command::new("View", "Increase Font Size", IncreaseFontSize),
        Command::new("View", "Decrease Font Size", DecreaseFontSize),
        Command::new("View", "Reset Font Size", ResetFontSize),
        Command::new("Go", "Open Quickly", OpenQuickly),
        Command::new("Go", "Open Files Quickly", OpenFilesQuickly),
        Command::new("Go", "Find in All Tabs", GlobalSearch),
        Command::new("Go", "Search Agent Conversations", SearchAgentConversations),
        Command::new("Go", "Show Next Unread Tab", ShowNextUnread),
        Command::new("File", "New File Editor", NewFileEditor),
        Command::new("File", "Open File…", OpenFile),
        Command::new("File", "Open Config Folder", OpenConfigFolder),
        Command::new("Tab", "New Tab", NewTab),
        Command::new("Tab", "Duplicate Tab", DuplicateTab),
        Command::new("Tab", "Close Tab", CloseTab),
        Command::new("Tab", "Reopen Closed Tab", ReopenClosedTab),
        Command::new("Tab", "Next Tab", NextTab),
        Command::new("Tab", "Previous Tab", PreviousTab),
        Command::new("Tab", "Rename Tab", RenameTab),
        Command::new("Tab", "Set Tab Prefix", SetTabPrefix),
        Command::new("Tab", "Move Tab Up", MoveTabUp),
        Command::new("Tab", "Move Tab Down", MoveTabDown),
        Command::new("Tab", "Move Tab to New Window", MoveTabToNewWindow),
        Command::new("Tab", "Mark Tab: Unread", MarkTabUnread),
        Command::new("Tab", "Mark Tab: Processing", MarkTabProcessing),
        Command::new("Tab", "Mark Tab: Failed", MarkTabFailed),
        Command::new("Tab", "Clear Badge", ClearBadge),
        Command::new("Window", "New Window", NewWindow),
        Command::new("Window", "Close Window", CloseWindow),
        Command::new("Window", "Close All", CloseAll),
        Command::new("Window", "Minimize", Minimize),
        Command::new("Window", "Minimize All", MinimizeAll),
        Command::new("Window", "Zoom", ZoomWindow),
        Command::new("Window", "Pin Window", PinWindow).checked(ws.pinned),
        Command::new("Window", "Merge All Windows", MergeAllWindows),
    ]);
    if let Some(target) = target {
        terminal_commands(target, &mut list);
    }
    list.extend([
        Command::new("Edit", "Secure Keyboard Entry", ToggleSecureKeyboardEntry),
        Command::new("Settings", "Settings…", OpenSettings),
        Command::new("Settings", "Open Config File", OpenConfigFile),
        Command::new("Settings", "Reload Config", ReloadConfig),
        Command::new("Motty", "Welcome Guide", ShowWelcome),
        Command::new("Motty", "Quit Motty", Quit),
    ]);
    merge_groups(list)
}

/// Commands about the focused terminal pane.
fn terminal_commands(target: &TerminalView, list: &mut Vec<Command>) {
    let mode = target.mode();
    list.extend([
        Command::new("Go", "Jump To", JumpTo),
        Command::new("File", "Export Text…", ExportText),
        Command::new("Pane", "Split Right", SplitRight),
        Command::new("Pane", "Split Left", SplitLeft),
        Command::new("Pane", "Split Down", SplitDown),
        Command::new("Pane", "Split Up", SplitUp),
        Command::new("Pane", "Equalize Splits", EqualizeSplits),
        Command::new("Pane", "Zoom Split", ZoomSplit),
        Command::new("Pane", "Focus Next Pane", FocusNextPane),
        Command::new("Pane", "Focus Previous Pane", FocusPreviousPane),
        Command::new("Terminal", "Find", Find),
        Command::new("Terminal", "Use Selection for Find", UseSelectionForFind),
        Command::new("Terminal", "Clear Screen", Clear),
        Command::new("Terminal", "Clear Scrollback Above", ClearScrollbackAbove),
        Command::new("Terminal", "Reset Terminal", ResetTerminal),
        Command::new("Terminal", "Reload Terminal View", ReloadTerminalView),
    ]);
    // A program other than the shell is in the foreground.
    if target.busy
        && let Some(program) = &target.process
    {
        list.push(Command::new(
            "Terminal",
            format!("Quit {program}"),
            QuitProgram,
        ));
    }
    list.extend([
        Command::new("Terminal", "Copy", Copy),
        Command::new("Terminal", "Copy with Style", CopyWithStyle),
        Command::new("Terminal", "Copy as ANSI", CopyAsAnsi),
        Command::new("Terminal", "Copy Screen", CopyScreen),
        Command::new("Terminal", "Paste", Paste),
        Command::new("Terminal", "Paste Escaped", PasteEscaped),
        Command::new("Terminal", "Paste Bracketed", PasteBracketed),
        Command::new("Terminal", "Paste Selection", PasteSelection),
        Command::new("Terminal", "Insert File Path…", InsertFilePath),
        Command::new("Terminal", "Select All", SelectAll),
        Command::new("Terminal", "Scroll to Top", ScrollToTop),
        Command::new("Terminal", "Scroll to Bottom", ScrollToBottom),
        Command::new("Terminal", "Vi Mode", ToggleViMode)
            .checked(matches!(mode, Some(PaneMode::Vi { .. }))),
        Command::new("Terminal", "Mark Mode", MarkMode),
        Command::new("Terminal", "Read-Only Mode", ToggleReadOnly)
            .checked(mode == Some(PaneMode::ReadOnly)),
        Command::new("Terminal", "Hints: Open Link", HintOpen),
        Command::new("Terminal", "Hints: Copy Link", HintCopy),
        Command::new("Terminal", "Hints: Paste Word", HintPaste),
        Command::new("Terminal", "Hints: Reveal Path", HintReveal),
        Command::new("Agent", "Composer", ToggleComposer),
        Command::new("Agent", "Prompt Queue", TogglePromptQueue),
        Command::new("Agent", "Send to Chat", SendToChat),
        Command::new("Agent", "Agent History", AgentHistory),
    ]);
    if let Some(status) = &target.status
        && let (Some(agent), Some(id)) = (status.agent_id.as_deref(), status.session.as_deref())
    {
        list.push(Command::new(
            "Agent",
            "View Session History",
            ViewSessionHistory,
        ));
        if crate::agents::sessions::fork_command(agent, id).is_some() {
            let label = crate::agents::sessions::fork_label(agent);
            for (place, action) in [
                ("New Tab", Box::new(ForkSessionInNewTab) as Box<dyn Action>),
                ("New Window", Box::new(ForkSessionInNewWindow)),
                ("Split Right", Box::new(ForkSessionRight)),
                ("Split Down", Box::new(ForkSessionDown)),
            ] {
                list.push(Command {
                    group: "Agent",
                    title: format!("{label} in {place}").into(),
                    action,
                    checked: false,
                });
            }
        }
    }
}

/// Puts each group's commands together, groups in first-appearance order,
/// so no header shows twice.
fn merge_groups(list: Vec<Command>) -> Vec<Command> {
    let mut groups: Vec<(&'static str, Vec<Command>)> = Vec::new();
    for c in list {
        match groups.iter_mut().find(|(g, _)| *g == c.group) {
            Some((_, v)) => v.push(c),
            None => groups.push((c.group, vec![c])),
        }
    }
    groups.into_iter().flat_map(|(_, v)| v).collect()
}

/// The commands shown for `query`: all of them grouped, or the matches
/// best first.
fn filter(commands: Vec<Command>, query: &str) -> Vec<Command> {
    if query.trim().is_empty() {
        return commands;
    }
    let mut scored: Vec<(i32, Command)> = commands
        .into_iter()
        .filter_map(|c| {
            let title = score(query, &c.title);
            let full = score(query, &format!("{}: {}", c.group, c.title)).map(|s| s - 2);
            Some((title.max(full)?, c))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(_, c)| c).collect()
}

/// Key caps for a binding, modifiers in macOS order.
pub(crate) fn keycaps(keystroke: &Keystroke) -> Vec<SharedString> {
    let m = &keystroke.modifiers;
    let mut caps: Vec<SharedString> = [
        (m.control, "⌃"),
        (m.alt, "⌥"),
        (m.shift, "⇧"),
        (m.platform, "⌘"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, glyph)| glyph.into())
    .collect();
    let key = match keystroke.key.as_str() {
        "enter" => "↩".into(),
        "escape" => "⎋".into(),
        "backspace" => "⌫".into(),
        "delete" => "⌦".into(),
        "tab" => "⇥".into(),
        "space" => "Space".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "home" => "↖".into(),
        "end" => "↘".into(),
        "pageup" => "⇞".into(),
        "pagedown" => "⇟".into(),
        key => key.to_uppercase(),
    };
    caps.push(key.into());
    caps
}

impl Workspace {
    /// ⇧⌘P opens the palette over the focused pane (or closes it).
    pub(super) fn toggle_palette(
        &mut self,
        _: &CommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette.is_some() {
            return self.close_palette(window, cx);
        }
        let Some(back) = self.active_focus(cx) else {
            return;
        };
        let target = self.focused_pane();
        self.open_quickly = None;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.palette = Some(Palette {
            focus,
            query: String::new(),
            selected: 0,
            target,
            back,
            scroll: ScrollHandle::new(),
        });
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(palette) = self.palette.take() {
            window.focus(&palette.back, cx);
        }
        cx.notify();
    }

    fn palette_commands(&self, cx: &Context<Self>) -> Vec<Command> {
        let Some(palette) = &self.palette else {
            return Vec::new();
        };
        filter(
            commands(self, palette.target.as_ref().map(|t| t.read(cx))),
            &palette.query,
        )
    }

    fn edit_palette_query(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut String)) {
        if let Some(palette) = self.palette.as_mut() {
            edit(&mut palette.query);
            palette.selected = 0;
            palette.scroll.scroll_to_item(0);
            cx.notify();
        }
    }

    fn move_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.palette_commands(cx).len();
        let grouped = self
            .palette
            .as_ref()
            .is_some_and(|p| p.query.trim().is_empty());
        let headers = if grouped {
            self.palette_headers_before(cx)
        } else {
            Vec::new()
        };
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        if n == 0 {
            return;
        }
        palette.selected = (palette.selected as isize + delta).rem_euclid(n as isize) as usize;
        // Headers are children of the list too.
        let child = palette.selected + headers.get(palette.selected).copied().unwrap_or(0);
        palette.scroll.scroll_to_item(child);
        cx.notify();
    }

    /// For each grouped command, how many headers come before it.
    fn palette_headers_before(&self, cx: &Context<Self>) -> Vec<usize> {
        let mut out = Vec::new();
        let mut group = "";
        let mut headers = 0;
        for c in self.palette_commands(cx) {
            if c.group != group {
                group = c.group;
                headers += 1;
            }
            out.push(headers);
        }
        out
    }

    /// Runs the `ix`th shown command in the target pane; `keep` leaves the
    /// palette open.
    fn run_palette_command(
        &mut self,
        ix: usize,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = self.palette_commands(cx).into_iter().nth(ix) else {
            return;
        };
        let Some(target) = self.palette.as_ref().map(|p| p.back.clone()) else {
            return;
        };
        if !keep {
            self.close_palette(window, cx);
        }
        // Dispatched from the pane, so both its actions and the window's
        // reach it; deferred, since the window's handlers need `self`.
        let action = command.action;
        cx.defer_in(window, move |_, window, cx| {
            target.dispatch_action(&*action, window, cx);
        });
    }

    fn on_palette_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        match ks.key.as_str() {
            "enter" => {
                let ix = self.palette.as_ref().map_or(0, |p| p.selected);
                self.run_palette_command(ix, m.platform, window, cx);
            }
            _ if m.platform => return,
            "escape" => self.close_palette(window, cx),
            "up" => self.move_palette_selection(-1, cx),
            "down" => self.move_palette_selection(1, cx),
            "p" | "k" if m.control => self.move_palette_selection(-1, cx),
            "n" | "j" if m.control => self.move_palette_selection(1, cx),
            _ => {
                if let Some(p) = self.palette.as_mut()
                    && edit_query(&mut p.query, ks)
                {
                    p.selected = 0;
                    p.scroll.scroll_to_item(0);
                    cx.notify();
                }
            }
        }
        cx.stop_propagation();
    }

    pub(super) fn render_palette(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let palette = self.palette.as_ref()?;
        let t = &self.theme;
        let c = Colors::new(t);
        let target = palette.back.clone();
        let commands = self.palette_commands(cx);
        let grouped = palette.query.trim().is_empty();
        let selected = palette.selected.min(commands.len().saturating_sub(1));

        let mut children: Vec<gpui::AnyElement> = Vec::new();
        let mut group = "";
        for (ix, command) in commands.iter().enumerate() {
            if grouped && command.group != group {
                let first = group.is_empty();
                group = command.group;
                children.push(picker::header(group, TEXT_LEFT, first, &c).into_any_element());
            }
            let caps = window
                .highest_precedence_binding_for_action_in(&*command.action, &target)
                .and_then(|b| b.keystrokes().first().map(|k| keycaps(k.inner())))
                .unwrap_or_default();
            children.push(
                div()
                    .id(ix)
                    .flex_none()
                    .mx(px(ROW_INSET))
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .rounded(px(ROW_RADIUS))
                    .when(ix == selected, |d| d.bg(c.selected))
                    .child(
                        div()
                            .w(px(TEXT_LEFT - ROW_INSET))
                            .flex_none()
                            .pl(px(10.5))
                            .text_color(hsla(if t.meta.mode == Mode::Dark {
                                Rgba::rgb(0x6fbf7f)
                            } else {
                                Rgba::rgb(CHECK)
                            }))
                            .when(command.checked, |d| d.child("✓")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c.text)
                            .child(command.title.clone()),
                    )
                    .child(div().flex_none().pr(px(13.)).flex().gap(px(5.)).children(
                        caps.into_iter().map(|cap| {
                            div()
                                .min_w(px(KEYCAP_WIDTH))
                                .h(px(KEYCAP_HEIGHT))
                                .px(px(4.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.))
                                .border(px(0.5))
                                .border_color(c.keycap_border)
                                .bg(c.keycap)
                                .text_size(px(11.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(c.keycap_text)
                                .child(cap)
                        }),
                    ))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.palette.as_mut()
                            && p.selected != ix
                        {
                            p.selected = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_palette_command(ix, false, window, cx)
                    }))
                    .into_any_element(),
            );
        }
        if commands.is_empty() {
            children.push(
                div()
                    .h(px(ROW_HEIGHT))
                    .pl(px(TEXT_LEFT))
                    .flex()
                    .items_center()
                    .text_color(c.placeholder)
                    .child("No matching commands")
                    .into_any_element(),
            );
        }

        let search = picker::search_row(&palette.query, "Search for commands...", &c);

        let card = picker::card(WIDTH, &c)
            .id("command-palette")
            .key_context("CommandPalette")
            .track_focus(&palette.focus)
            .on_key_down(cx.listener(Self::on_palette_key))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .map(|t| t.replace(['\n', '\r'], " "));
                if let Some(text) = text {
                    this.edit_palette_query(cx, |q| q.push_str(&text));
                }
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                if let Some(p) = &this.palette {
                    cx.write_to_clipboard(ClipboardItem::new_string(p.query.clone()));
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(search)
            .child(div().flex_none().h(px(1.)).bg(c.separator))
            .child(
                div()
                    .id("palette-list")
                    .max_h(px(LIST_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&palette.scroll)
                    .pb(px(ROW_INSET))
                    .when(!grouped, |d| d.pt(px(6.)))
                    .flex()
                    .flex_col()
                    .gap(px(ROW_GAP))
                    .children(children),
            );

        // A click outside the card closes it.
        Some(
            div()
                .id("palette-scrim")
                .absolute()
                .inset_0()
                .px(px(16.))
                .pt(px(picker::top(window)))
                .flex()
                .flex_col()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
                )
                .child(card),
        )
    }
}
