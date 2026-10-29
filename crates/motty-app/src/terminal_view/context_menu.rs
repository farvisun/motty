//! Right-click in a terminal: the native context menu (Otty's items), or
//! what `right-click-action` says instead.
//! On a path or URL it also offers Open With (the apps macOS lists for
//! it), Change Directory Here and Open in Motty; on a command's rows,
//! About This Line.
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
    OpenWith(String, PathBuf),
    Cd(PathBuf),
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
        fn add(entries: &mut Vec<Entry>, picks: &mut Vec<Pick>, item: Item, pick: Pick) {
            entries.push(item.into());
            picks.push(pick);
        }

        if let Some(link) = self.link_at(at) {
            // Otty's order: open it, Open With, copy, reveal, then the
            // terminal's own places for it.
            let (open, target, copy) = match &link.target {
                Target::Url(url) => ("Open Link", url.clone(), "Copy URL"),
                Target::Path { path, .. } if path.is_dir() => (
                    "Open Folder",
                    path.to_string_lossy().into_owned(),
                    "Copy Path",
                ),
                Target::Path { path, .. } => (
                    "Open File",
                    path.to_string_lossy().into_owned(),
                    "Copy Path",
                ),
            };
            add(
                &mut entries,
                &mut picks,
                Item::new(open),
                Pick::Open(link.target.clone()),
            );
            let apps = crate::workspace::apps_for(&target, &self.link_options.open_with);
            if !apps.is_empty() {
                let mut with = Vec::new();
                for (name, bundle) in apps {
                    with.push(Item::new(name).into());
                    picks.push(Pick::OpenWith(bundle, PathBuf::from(&target)));
                }
                entries.push(Entry::Submenu {
                    label: "Open With".into(),
                    entries: with,
                });
            }
            add(
                &mut entries,
                &mut picks,
                Item::new(copy),
                Pick::Copy(target.clone()),
            );
            if let Target::Path { path, .. } = &link.target {
                let path = path.clone();
                add(
                    &mut entries,
                    &mut picks,
                    Item::new("Reveal in Finder"),
                    Pick::Reveal(path.clone()),
                );
                let folder = if path.is_dir() {
                    Some(path.clone())
                } else {
                    path.parent().map(PathBuf::from)
                };
                if let Some(folder) = folder {
                    entries.push(Entry::Separator);
                    let idle = !self.read_only && !self.session.is_busy();
                    add(
                        &mut entries,
                        &mut picks,
                        Item::new("Change Directory Here").enabled(idle),
                        Pick::Cd(folder.clone()),
                    );
                    // A folder opens as a shell there; a file in an editor
                    // tab (Motty has no file panes in splits yet).
                    let mut here = Vec::new();
                    if path.is_dir() {
                        here.push(Item::new("Current Pane").enabled(idle).into());
                        picks.push(Pick::Cd(folder.clone()));
                        here.push(Item::new("New Tab").into());
                        picks.push(Pick::Action(Box::new(OpenFolder {
                            path: folder.clone(),
                        })));
                        here.push(Item::new("Split").into());
                        picks.push(Pick::Action(Box::new(DropSplit {
                            pane: self.id,
                            side: "right".into(),
                            path: folder,
                        })));
                    } else {
                        here.push(Item::new("New Tab").into());
                        picks.push(Pick::Action(Box::new(EditFiles {
                            paths: vec![path.clone()],
                        })));
                    }
                    entries.push(Entry::Submenu {
                        label: "Open in Motty".into(),
                        entries: here,
                    });
                }
            }
            entries.push(Entry::Separator);
        }

        let key = |action: &dyn Action| shortcut(action, &focus, window);
        let has_selection = self.has_selection();
        add(
            &mut entries,
            &mut picks,
            Item::new("Copy")
                .symbol("doc.on.doc")
                .keystroke(key(&Copy))
                .enabled(has_selection),
            Pick::Action(Box::new(Copy)),
        );
        add(
            &mut entries,
            &mut picks,
            Item::new("Paste")
                .symbol("doc.on.clipboard")
                .keystroke(key(&Paste))
                .enabled(!self.read_only),
            Pick::Action(Box::new(Paste)),
        );
        if has_selection {
            add(
                &mut entries,
                &mut picks,
                Item::new("Save Selection to Recipe…").symbol("book"),
                Pick::Action(Box::new(SaveSelectionToRecipe)),
            );
        }
        entries.push(Entry::Separator);
        add(
            &mut entries,
            &mut picks,
            Item::new("Send to Chat")
                .symbol("bubble.left")
                .keystroke(key(&SendToChat)),
            Pick::Action(Box::new(SendToChat)),
        );
        add(
            &mut entries,
            &mut picks,
            Item::new("Composer")
                .symbol("square.and.pencil")
                .keystroke(key(&ToggleComposer)),
            Pick::Action(Box::new(ToggleComposer)),
        );
        entries.push(Entry::Separator);
        add(
            &mut entries,
            &mut picks,
            Item::new("Select All")
                .symbol("character.textbox")
                .keystroke(key(&SelectAll)),
            Pick::Action(Box::new(SelectAll)),
        );
        add(
            &mut entries,
            &mut picks,
            Item::new("Find…")
                .symbol("magnifyingglass")
                .keystroke(key(&Find)),
            Pick::Action(Box::new(Find)),
        );
        add(
            &mut entries,
            &mut picks,
            Item::new("Clear Screen")
                .symbol("eraser")
                .keystroke(key(&Clear)),
            Pick::Action(Box::new(Clear)),
        );
        entries.push(Entry::Separator);

        if let Some(block) = self.block_at(at) {
            let mut about = Vec::new();
            let text = block.command.unwrap_or_else(|| "Command".into());
            let label = match &block.ran_at {
                Some(at) => format!("{} — {at}", truncate(&text, 50)),
                None => truncate(&text, 60),
            };
            about.push(Item::new(label).enabled(false).into());
            picks.push(Pick::Copy(String::new()));
            about.push(Item::new(block.status).enabled(false).into());
            picks.push(Pick::Copy(String::new()));
            about.push(Entry::Separator);
            about.push(Item::new("Copy Command").enabled(text != "Command").into());
            picks.push(Pick::Copy(text));
            let output = block.output.unwrap_or_default();
            about.push(Item::new("Copy Output").enabled(!output.is_empty()).into());
            picks.push(Pick::Copy(output));
            entries.push(Entry::Submenu {
                label: "About This Line".into(),
                entries: about,
            });
            entries.push(Entry::Separator);
        }

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

        let view = cx.entity();
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
                Pick::OpenWith(bundle, path) => crate::workspace::open_in(&bundle, &path),
                Pick::Cd(folder) => {
                    let line = format!("cd {}\r", super::shell_escape(&folder.to_string_lossy()));
                    view.update(cx, |v, cx| v.write_input(&line, cx));
                }
            }
        });
    }
}

/// The command under a right-click, for About This Line.
struct BlockInfo {
    command: Option<String>,
    /// `14:32:05`: when it started.
    ran_at: Option<String>,
    status: String,
    output: Option<String>,
}

impl TerminalView {
    /// The shell-integration block whose rows hold `at`.
    fn block_at(&self, at: Point<Pixels>) -> Option<BlockInfo> {
        let (.., point, _) = self.hit(at)?;
        let row = {
            let term = self.session.term.lock();
            u64::try_from(term.grid().lines_pushed() as i64 + i64::from(point.line.0)).ok()?
        };
        let shell = self.session.shell();
        let block = shell.blocks.iter().rev().find(|b| b.prompt_row <= row)?;
        // Only rows of a command that ran, not the prompt being typed at.
        block.started?;
        let status = if block.finished.is_none() {
            "Running".to_owned()
        } else {
            let took = block.duration().map(took).unwrap_or_default();
            match block.exit {
                Some(0) => format!("Succeeded {took}"),
                Some(code) => format!("Exited with {code} {took}"),
                None => "Finished".to_owned(),
            }
            .trim()
            .to_owned()
        };
        let block = block.clone();
        drop(shell);
        Some(BlockInfo {
            command: self.session.block_command(&block),
            ran_at: block.started.map(crate::workspace::clock_time),
            status,
            output: self.session.block_output(&block),
        })
    }
}

/// `in 1.2s`, `in 3m 4s`.
fn took(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("in {:.1}s", d.as_secs_f32())
    } else {
        format!("in {}m {}s", secs / 60, secs % 60)
    }
}

fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_owned(),
    }
}

/// The first key that runs `action` from the pane.
fn shortcut(action: &dyn Action, focus: &FocusHandle, window: &Window) -> Option<Keystroke> {
    window
        .highest_precedence_binding_for_action_in(action, focus)
        .and_then(|b| b.keystrokes().first().map(|k| k.inner().clone()))
}
