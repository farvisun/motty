//! Right-click in a terminal: the native context menu (Otty's items, minus
//! the agent ones still to come), or what `right-click-action` says instead.
//! A program that captures the mouse gets the click; ⌃-click always opens
//! the menu.

use std::path::PathBuf;

use gpui::{
    Action, ClipboardItem, Context, FocusHandle, Keystroke, MouseButton, MouseDownEvent,
    MouseUpEvent, Pixels, Point, Window,
};
use motty_term::input::{self, MouseAction};
use motty_term::links::Target;
use motty_theme::RightClick;

use super::TerminalView;
use crate::actions::*;
use crate::native_menu::{self, Entry, Item};

/// What a menu item does once picked.
enum Pick {
    Action(Box<dyn Action>),
    Open(Target),
    Copy(String),
    Reveal(PathBuf),
}

impl TerminalView {
    pub(super) fn on_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let control = event.modifiers.control;
        if !control
            && self.report_mouse(
                input::MouseButton::Right,
                MouseAction::Press,
                event.position,
                &event.modifiers,
            )
        {
            return;
        }
        let action = if control {
            RightClick::ContextMenu
        } else {
            self.link_options.right_click
        };
        let has_selection = self.has_selection();
        let focus = self.focus_handle.clone();
        match action {
            RightClick::ContextMenu => self.show_context_menu(event.position, window, cx),
            RightClick::Copy => focus.dispatch_action(&Copy, window, cx),
            RightClick::Paste => focus.dispatch_action(&Paste, window, cx),
            RightClick::CopyOrPaste if has_selection => {
                focus.dispatch_action(&Copy, window, cx);
                self.session.term.lock().selection = None;
                cx.notify();
            }
            RightClick::CopyOrPaste => focus.dispatch_action(&Paste, window, cx),
            RightClick::Ignore => {}
        }
    }

    pub(super) fn on_right_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.report_mouse(
            input::MouseButton::Right,
            MouseAction::Release,
            event.position,
            &event.modifiers,
        );
    }

    /// ⌃-click (left button) opens the menu too, as everywhere on macOS,
    /// unless a program has the mouse.
    pub(super) fn control_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button != MouseButton::Left
            || !event.modifiers.control
            || event.modifiers.platform
            || self.captures_mouse(&event.modifiers)
        {
            return false;
        }
        self.show_context_menu(event.position, window, cx);
        true
    }

    fn has_selection(&self) -> bool {
        self.session
            .term
            .lock()
            .selection
            .as_ref()
            .is_some_and(|s| !s.is_empty())
    }

    fn show_context_menu(
        &mut self,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self.focus_handle.clone();
        let mut entries: Vec<Entry> = Vec::new();
        let mut picks: Vec<Pick> = Vec::new();
        let mut add = |entries: &mut Vec<Entry>, item: Item, pick: Pick| {
            entries.push(item.into());
            picks.push(pick);
        };

        if let Some(link) = self.link_at(at) {
            match link.target.clone() {
                Target::Url(url) => {
                    add(
                        &mut entries,
                        Item::new("Open Link"),
                        Pick::Open(Target::Url(url.clone())),
                    );
                    add(&mut entries, Item::new("Copy Link"), Pick::Copy(url));
                }
                target @ Target::Path { .. } => {
                    let Target::Path { path, .. } = &target else {
                        unreachable!()
                    };
                    let path = path.clone();
                    add(&mut entries, Item::new("Open"), Pick::Open(target));
                    add(
                        &mut entries,
                        Item::new("Reveal in Finder"),
                        Pick::Reveal(path.clone()),
                    );
                    add(
                        &mut entries,
                        Item::new("Copy Path"),
                        Pick::Copy(path.to_string_lossy().into_owned()),
                    );
                }
            }
            entries.push(Entry::Separator);
        }

        let key = |action: &dyn Action| shortcut(action, &focus, window);
        let has_selection = self.has_selection();
        add(
            &mut entries,
            Item::new("Copy")
                .symbol("doc.on.doc")
                .keystroke(key(&Copy))
                .enabled(has_selection),
            Pick::Action(Box::new(Copy)),
        );
        add(
            &mut entries,
            Item::new("Paste")
                .symbol("doc.on.clipboard")
                .keystroke(key(&Paste))
                .enabled(!self.read_only),
            Pick::Action(Box::new(Paste)),
        );
        entries.push(Entry::Separator);
        add(
            &mut entries,
            Item::new("Select All")
                .symbol("character.textbox")
                .keystroke(key(&SelectAll)),
            Pick::Action(Box::new(SelectAll)),
        );
        add(
            &mut entries,
            Item::new("Find…")
                .symbol("magnifyingglass")
                .keystroke(key(&Find)),
            Pick::Action(Box::new(Find)),
        );
        add(
            &mut entries,
            Item::new("Clear Screen")
                .symbol("eraser")
                .keystroke(key(&Clear)),
            Pick::Action(Box::new(Clear)),
        );
        entries.push(Entry::Separator);

        // The submenu's items count in place, after the ones above.
        let mut split = Vec::new();
        for (title, action) in [
            ("Split Right", Box::new(SplitRight) as Box<dyn Action>),
            ("Split Left", Box::new(SplitLeft)),
            ("Split Down", Box::new(SplitDown)),
            ("Split Up", Box::new(SplitUp)),
        ] {
            split.push(Item::new(title).keystroke(key(&*action)).into());
            picks.push(Pick::Action(action));
        }
        entries.push(Entry::Submenu {
            label: "Split Pane".into(),
            entries: split,
        });
        entries.push(
            Item::new("Read-Only")
                .symbol("lock")
                .keystroke(key(&ToggleReadOnly))
                .checked(self.read_only)
                .into(),
        );
        picks.push(Pick::Action(Box::new(ToggleReadOnly)));

        native_menu::popup(entries, at, None, window, cx, move |ix, window, cx| {
            let Some(pick) = picks.into_iter().nth(ix) else {
                return;
            };
            match pick {
                Pick::Action(action) => focus.dispatch_action(&*action, window, cx),
                Pick::Open(Target::Url(url)) => cx.open_url(&url),
                Pick::Open(Target::Path { path, .. }) => cx.open_with_system(&path),
                Pick::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                Pick::Reveal(path) => cx.reveal_path(&path),
            }
        });
    }
}

/// The first key that runs `action` from the pane.
fn shortcut(action: &dyn Action, focus: &FocusHandle, window: &Window) -> Option<Keystroke> {
    window
        .highest_precedence_binding_for_action_in(action, focus)
        .and_then(|b| b.keystrokes().first().map(|k| k.inner().clone()))
}
