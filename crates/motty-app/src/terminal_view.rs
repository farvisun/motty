//! One terminal pane: owns the session and translates GPUI input into PTY bytes.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, ScrollDelta, ScrollEvent, SharedString, Task, UTF16Selection, Window,
    div, prelude::*,
};
use hints::HintAction;
use motty_render::{TerminalElement, TerminalLayout, TerminalStyle};
use motty_term::alacritty_terminal::grid::{Dimensions, Scroll};
use motty_term::alacritty_terminal::index::{Column, Point, Side};
use motty_term::alacritty_terminal::selection::{Selection, SelectionType};
use motty_term::alacritty_terminal::term::{TermMode, viewport_to_point};
use motty_term::alacritty_terminal::vte::ansi::Rgb;
use motty_term::input::{self, KeyKind, Mods, MouseAction};
use motty_term::links::Matcher;
use motty_term::search::SearchOptions;
use motty_term::{Session, SessionEvent};
use motty_theme::CursorBlink;

use crate::actions::*;

mod autocomplete;
mod behavior;
mod composer;
mod composer_panel;
mod context_menu;
mod copy_as;
mod find;
mod hints;
mod keys;
mod links;
mod paste;
mod replay;
mod rich_paste;
mod vi;

pub use composer::{Chip, is_pinned as composer_pinned};
pub use links::LinkOptions;
pub(crate) use paste::clipboard_files;
pub use vi::PaneMode;

pub enum TerminalEvent {
    TitleChanged,
    Exited,
    /// A command run from an integrated shell finished.
    CommandFinished {
        exit: Option<i32>,
        duration: Option<std::time::Duration>,
    },
    /// OSC 9 / OSC 777 desktop notification.
    Notification {
        title: Option<String>,
        body: String,
    },
    /// BEL: whether it rings is Sound — Shell Controlled.
    Bell,
    /// OSC 52 copy, subject to `clipboard-write`.
    ClipboardWrite(String),
    /// OSC 52 paste, subject to `clipboard-read`: reply with
    /// [`TerminalView::answer_clipboard`].
    ClipboardRead(std::sync::Arc<dyn Fn(&str) -> String + Send + Sync>),
}

/// A new pane id; unique for the life of the app.
pub fn next_pane_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub struct TerminalView {
    /// Stable id for the control socket (`$MOTTY_PANE_ID` in the shell).
    pub id: u64,
    session: Session,
    style: Rc<TerminalStyle>,
    base_font_size: f32,
    focus_handle: FocusHandle,
    layout: Rc<Cell<Option<TerminalLayout>>>,
    /// Title set by the running program (OSC 0/2).
    pub title: Option<String>,
    /// Shell name, foreground cwd and whether a command is running,
    /// refreshed by [`Self::refresh_process`].
    pub process: Option<String>,
    pub cwd: Option<std::path::PathBuf>,
    pub busy: bool,
    /// Title — Shell Controlled and Mouse — Shell Controlled for this pane's
    /// tab (Notifications & Privileges).
    pub allow_title: bool,
    pub allow_mouse: bool,
    /// The program in the foreground while a command runs (not the shell).
    pub foreground: Option<String>,
    /// Its command line, for `tab-rule` `command` and `${command}`.
    pub command: Option<String>,
    /// The git branch of the folder (a short hash when detached).
    pub branch: Option<String>,
    /// Sudo, SSH and caffeinate while a command runs.
    pub system: motty_term::SystemState,
    /// The agent's state, or how the last command went, for the tab.
    pub status: Option<crate::agents::Status>,
    /// OSC 9;4 progress, or a spinner for a slow command.
    pub progress: Option<motty_term::shell::Progress>,
    /// Selection, clipboard and Secure Keyboard Entry switches.
    behavior: motty_theme::Behavior,
    progress_commands: Vec<String>,
    /// Whether the running command (by prompt row) gets a spinner.
    auto_progress: Option<(u64, bool)>,
    /// A ⇧-arrow selection's anchor and moving end, as cell boundaries.
    kb_select: Option<((i32, i32), (i32, i32))>,
    /// The pane is at a password prompt and asked for Secure Keyboard Entry.
    secret: bool,
    /// Files dragged over the pane: where they'd land, and whether there's
    /// a folder among them.
    drop_zone: Option<paste::DropZone>,
    drop_folder: bool,
    option_as_meta: bool,
    keys_down: keys::KeysDown,
    /// Fractional lines carried between precise (trackpad) scroll events.
    scroll_remainder: f32,
    selecting: bool,
    /// IME composition in progress.
    marked_text: Option<SharedString>,
    /// `None` when the cursor never blinks (`always_off`).
    blink: Option<BlinkState>,
    blink_policy: CursorBlink,
    /// The find bar, while open.
    find: Option<find::Find>,
    /// A row a `motty://` link pointed at, lit for a moment.
    flash: Option<(u64, Task<()>)>,
    /// The last query and options, restored when the bar reopens.
    last_find: (String, SearchOptions),
    link_options: LinkOptions,
    /// The link under the pointer while ⌘ is held.
    hovered_link: Option<motty_term::links::Link>,
    /// Last pointer position over the pane, for ⌘ presses without a move.
    pointer: Option<gpui::Point<Pixels>>,
    /// Vi mode, while on.
    vi: Option<vi::ViState>,
    /// Vi mode shows its keys at the bottom (⌘/ toggles).
    vi_key_hints: bool,
    /// Hint mode, while labels are up.
    hints: Option<hints::HintState>,
    /// Input is refused (output still streams).
    read_only: bool,
    /// The `autocomplete-*` settings.
    autocomplete: motty_theme::Autocomplete,
    /// The inline suggestion at the prompt, if any.
    suggestion: Option<autocomplete::Suggestion>,
    /// The prompt line whose suggestion Esc hid.
    dismissed: Option<String>,
    /// The Composer's draft, open or not.
    composer: Option<composer::Composer>,
    queue: composer::Queue,
    events: async_channel::Receiver<SessionEvent>,
    _events: Task<()>,
    _focus_out: gpui::Subscription,
}

struct BlinkState {
    visible: bool,
    /// Bumped on input so the blink restarts from "visible".
    epoch: usize,
    _task: Task<()>,
}

const BLINK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(530);

impl EventEmitter<TerminalEvent> for TerminalView {}

impl TerminalView {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: u64,
        session: Session,
        events: async_channel::Receiver<SessionEvent>,
        style: TerminalStyle,
        option_as_meta: bool,
        cursor_blink: CursorBlink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let task = Self::event_task(events.clone(), window, cx);
        let focus_handle = cx.focus_handle();
        let focus_out = cx.on_focus_out(&focus_handle, window, |this, _, _, cx| {
            this.release_secret(cx)
        });
        Self {
            id,
            session,
            base_font_size: f32::from(style.font_size),
            style: Rc::new(style),
            focus_handle,
            layout: Rc::new(Cell::new(None)),
            title: None,
            process: None,
            allow_title: true,
            allow_mouse: true,
            foreground: None,
            command: None,
            branch: None,
            system: Default::default(),
            cwd: None,
            busy: false,
            status: None,
            progress: None,
            behavior: Default::default(),
            progress_commands: Vec::new(),
            auto_progress: None,
            kb_select: None,
            secret: false,
            drop_zone: None,
            drop_folder: false,
            option_as_meta,
            keys_down: Default::default(),
            scroll_remainder: 0.,
            selecting: false,
            marked_text: None,
            blink: (cursor_blink != CursorBlink::AlwaysOff).then(|| BlinkState {
                visible: true,
                epoch: 0,
                _task: Self::blink_task(cx),
            }),
            blink_policy: cursor_blink,
            find: None,
            flash: None,
            last_find: Default::default(),
            link_options: LinkOptions::default(),
            hovered_link: None,
            pointer: None,
            vi: None,
            vi_key_hints: true,
            hints: None,
            read_only: false,
            autocomplete: Default::default(),
            suggestion: None,
            dismissed: None,
            composer: None,
            queue: Default::default(),
            events,
            _events: task,
            _focus_out: focus_out,
        }
    }

    /// Handles the session's events in `window`'s context.
    fn event_task(
        events: async_channel::Receiver<SessionEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let alive = this.update_in(cx, |view, window, cx| {
                    view.handle_session_event(event, window, cx)
                });
                if alive.is_err() {
                    break;
                }
            }
        })
    }

    /// The pane moved to another window (Move Tab to New Window): its
    /// events are handled there from now on.
    pub fn attach_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._events = Self::event_task(self.events.clone(), window, cx);
    }

    fn blink_task(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut seen_epoch = 0;
            loop {
                cx.background_executor().timer(BLINK_INTERVAL).await;
                let alive = this.update(cx, |view, cx| {
                    // Programs turn blinking on and off (DECSCUSR, DECSET 12);
                    // the policy decides whether that counts.
                    let wanted = view
                        .blink_policy
                        .resolve(view.session.term.lock().cursor_style().blinking);
                    let Some(blink) = view.blink.as_mut() else {
                        return;
                    };
                    if !wanted || blink.epoch != seen_epoch {
                        // Not blinking, or input happened since the last tick:
                        // stay visible, repainting only if that changes anything.
                        seen_epoch = blink.epoch;
                        if blink.visible {
                            return;
                        }
                        blink.visible = true;
                    } else {
                        blink.visible = !blink.visible;
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
    }

    fn reset_blink(&mut self) {
        if let Some(blink) = self.blink.as_mut() {
            blink.visible = true;
            blink.epoch += 1;
        }
    }

    /// Re-reads the foreground process; returns whether anything changed.
    pub fn refresh_process(&mut self) -> bool {
        let process = self.session.shell_name();
        // The shell's own report (OSC 7) also covers remote shells.
        let cwd = self
            .session
            .shell()
            .cwd
            .clone()
            .or_else(|| self.session.foreground_cwd());
        let busy = self.session.is_busy();
        let (foreground, system) = if busy {
            (
                self.session
                    .foreground_process_name()
                    .filter(|name| Some(name) != process.as_ref()),
                self.session.system_state(),
            )
        } else {
            (None, Default::default())
        };
        let command = foreground
            .as_ref()
            .and_then(|_| self.session.foreground_pid())
            .and_then(motty_term::system::process_args)
            .map(|args| args.join(" "));
        let branch = cwd.as_deref().and_then(git_branch);
        let changed = process != self.process
            || cwd != self.cwd
            || busy != self.busy
            || foreground != self.foreground
            || system != self.system
            || command != self.command
            || branch != self.branch;
        self.foreground = foreground;
        self.command = command;
        self.branch = branch;
        self.system = system;
        let changed = self.refresh_progress() || changed;
        if cwd != self.cwd
            && let Some(dir) = &cwd
        {
            crate::folders::record(dir);
        }
        self.process = process;
        self.cwd = cwd;
        self.busy = busy;
        changed
    }

    /// The program's title, else the cwd with `~` for home.
    pub fn style(&self) -> Rc<TerminalStyle> {
        self.style.clone()
    }

    pub fn display_title(&self) -> String {
        if let Some(t) = self.title.as_deref().filter(|t| !t.is_empty()) {
            return t.to_owned();
        }
        let Some(cwd) = &self.cwd else {
            return "~".into();
        };
        match dirs::home_dir().and_then(|h| cwd.strip_prefix(h).ok().map(|p| p.to_path_buf())) {
            Some(rel) if rel.as_os_str().is_empty() => "~".into(),
            Some(rel) => format!("~/{}", rel.display()),
            None => cwd.display().to_string(),
        }
    }

    pub fn apply_config(
        &mut self,
        option_as_meta: bool,
        cursor_blink: CursorBlink,
        cursor_style: motty_term::alacritty_terminal::vte::ansi::CursorStyle,
        scrollback: usize,
        vt: motty_term::vt::VtOptions,
        cx: &mut Context<Self>,
    ) {
        self.option_as_meta = option_as_meta;
        self.session.set_options(cursor_style, scrollback, vt);
        if cursor_blink != self.blink_policy {
            self.blink_policy = cursor_blink;
            self.blink = (cursor_blink != CursorBlink::AlwaysOff).then(|| BlinkState {
                visible: true,
                epoch: 0,
                _task: Self::blink_task(cx),
            });
        }
        cx.notify();
    }

    /// Input is refused (Read Only mode).
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Writes text to the program as if typed (no bracketed paste).
    pub fn write_input(&mut self, text: &str, cx: &mut Context<Self>) {
        self.session.term.lock().scroll_display(Scroll::Bottom);
        self.session.write(text.as_bytes().to_vec());
        cx.notify();
    }

    pub fn set_style(&mut self, style: TerminalStyle, cx: &mut Context<Self>) {
        self.base_font_size = f32::from(style.font_size);
        self.style = Rc::new(style);
        cx.notify();
    }

    fn handle_session_event(
        &mut self,
        event: SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SessionEvent::Wakeup => {
                self.schedule_find_refresh(cx);
                self.check_secret(window, cx);
                cx.notify();
            }
            SessionEvent::Title(title) => {
                if self.allow_title {
                    self.title = title;
                    cx.emit(TerminalEvent::TitleChanged);
                }
            }
            SessionEvent::Bell => cx.emit(TerminalEvent::Bell),
            SessionEvent::ClipboardStore(text) => cx.emit(TerminalEvent::ClipboardWrite(text)),
            SessionEvent::ClipboardLoad(format) => cx.emit(TerminalEvent::ClipboardRead(format)),
            SessionEvent::ColorRequest(index, format) => {
                let c = self
                    .style
                    .palette
                    .get(index)
                    .copied()
                    .unwrap_or(self.style.background);
                let rgb = Rgb {
                    r: (c.r * 255.).round() as u8,
                    g: (c.g * 255.).round() as u8,
                    b: (c.b * 255.).round() as u8,
                };
                self.session.write(format(rgb).into_bytes());
            }
            SessionEvent::TextAreaSizeRequest(format) => {
                let size = self.session.size();
                let ws = motty_term::alacritty_terminal::event::WindowSize {
                    num_cols: size.cols,
                    num_lines: size.rows,
                    cell_width: size.cell_width as u16,
                    cell_height: size.cell_height as u16,
                };
                self.session.write(format(ws).into_bytes());
            }
            SessionEvent::CursorBlinkingChange => cx.notify(),
            SessionEvent::Exited(_) => cx.emit(TerminalEvent::Exited),
            SessionEvent::CommandFinished { exit, duration } => {
                // Where it ran: the folder before any `cd` it did.
                self.record_command(self.cwd.clone(), exit);
                self.refresh_process();
                self.command_done();
                cx.emit(TerminalEvent::CommandFinished { exit, duration });
            }
            SessionEvent::Notification { title, body } => {
                cx.emit(TerminalEvent::Notification { title, body })
            }
            SessionEvent::ShellChanged => {
                if self.refresh_process() {
                    cx.emit(TerminalEvent::TitleChanged);
                }
            }
            // zsh marks input start again as it accepts a line, so this can't
            // end a command; it lets the next queued prompt go.
            SessionEvent::PromptReady => self.dispatch(window, cx),
        }
    }

    /// Sends typed text (from keys or the IME) to the shell.
    fn send_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if text.is_empty() || self.vi.is_some() {
            return;
        }
        if self.read_only {
            window.play_system_bell();
            return;
        }
        self.reset_blink();
        self.typed();
        self.session.term.lock().scroll_display(Scroll::Bottom);
        self.session.write(text.as_bytes().to_vec());
        cx.notify();
    }

    fn term_mode(&self) -> TermMode {
        *self.session.term.lock().mode()
    }

    fn mods(m: &gpui::Modifiers) -> Mods {
        Mods {
            shift: m.shift,
            alt: m.alt,
            ctrl: m.control,
            cmd: m.platform,
            caps_lock: false,
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keys typed in the Composer or the queue aren't the terminal's.
        if !self.focus_handle.is_focused(window) {
            return;
        }
        if self.hints.is_some() {
            self.hint_key(event, window, cx);
            return;
        }
        if self.vi.is_some() {
            self.vi_key(event, window, cx);
            return;
        }
        if self.suggestion_key(&event.keystroke, cx) {
            self.reset_blink();
            cx.stop_propagation();
            return;
        }
        let ks = &event.keystroke;
        if self.shift_select(ks, cx) {
            cx.stop_propagation();
            return;
        }
        let kind = if event.is_held {
            KeyKind::Repeat
        } else {
            KeyKind::Press
        };
        if let Some(bytes) = self.encode_key(ks, kind) {
            if self.read_only {
                window.play_system_bell();
                cx.stop_propagation();
                return;
            }
            self.reset_blink();
            self.typed();
            self.session.term.lock().scroll_display(Scroll::Bottom);
            self.session.write(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// Viewport cell and grid point under a window position.
    fn hit(&self, pos: gpui::Point<gpui::Pixels>) -> Option<(usize, usize, Point, Side)> {
        let layout = self.layout.get()?;
        let (row, col, right_half) = layout.cell_at(pos);
        let offset = self.session.term.lock().grid().display_offset();
        let point = viewport_to_point(offset, Point::new(row, Column(col)));
        Some((
            row,
            col,
            point,
            if right_half { Side::Right } else { Side::Left },
        ))
    }

    /// Whether the program in the pane gets mouse events. Shift bypasses
    /// application mouse tracking so text can still be selected; so does
    /// read-only mode.
    fn captures_mouse(&self, mods: &gpui::Modifiers) -> bool {
        !mods.shift && !self.read_only && self.reports_mouse(self.term_mode())
    }

    /// The program asked for mouse events and may have them.
    fn reports_mouse(&self, mode: TermMode) -> bool {
        self.allow_mouse && mode.intersects(TermMode::MOUSE_MODE)
    }

    /// Answers an allowed OSC 52 read with the clipboard's text (a refused
    /// one gets no answer, as in xterm).
    pub fn answer_clipboard(&self, format: &(dyn Fn(&str) -> String + Send + Sync), text: &str) {
        self.session.write(format(text).into_bytes());
    }

    fn report_mouse(
        &self,
        button: input::MouseButton,
        action: MouseAction,
        pos: gpui::Point<gpui::Pixels>,
        mods: &gpui::Modifiers,
    ) -> bool {
        if !self.captures_mouse(mods) {
            return false;
        }
        let mode = self.term_mode();
        if let Some((row, col, ..)) = self.hit(pos)
            && let Some(bytes) =
                input::mouse_report(button, action, col, row, Self::mods(mods), mode)
        {
            self.session.write(bytes);
        }
        true
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        if self.control_click(event, window, cx) {
            return;
        }
        if self.click_link(&event.modifiers, window, cx) {
            return;
        }
        if self.report_mouse(
            input::MouseButton::Left,
            MouseAction::Press,
            event.position,
            &event.modifiers,
        ) {
            return;
        }
        let Some((.., point, side)) = self.hit(event.position) else {
            return;
        };
        let ty = match event.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ if event.modifiers.alt => SelectionType::Block,
            _ => SelectionType::Simple,
        };
        self.session.term.lock().selection = Some(Selection::new(ty, point, side));
        self.kb_select = None;
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_hovered_link(Some(event.position), &event.modifiers, cx);
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if !self.selecting {
            self.report_mouse(
                input::MouseButton::Left,
                MouseAction::Drag,
                event.position,
                &event.modifiers,
            );
            return;
        }
        let Some((.., point, side)) = self.hit(event.position) else {
            return;
        };
        if let Some(sel) = self.session.term.lock().selection.as_mut() {
            sel.update(point, side);
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.selecting = false;
            let mut term = self.session.term.lock();
            if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                term.selection = None;
            }
            drop(term);
            self.copy_on_select(cx);
            cx.notify();
        } else {
            self.report_mouse(
                input::MouseButton::Left,
                MouseAction::Release,
                event.position,
                &event.modifiers,
            );
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.layout.get() else {
            return;
        };
        let lines = match event.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => f32::from(p.y) / f32::from(layout.line_height),
        } + self.scroll_remainder;
        let whole = lines.trunc() as i32;
        self.scroll_remainder = lines.fract();
        if whole == 0 {
            return;
        }
        let mode = self.term_mode();
        if self.reports_mouse(mode) && !event.modifiers.shift && !self.read_only {
            let button = if whole > 0 {
                input::MouseButton::WheelUp
            } else {
                input::MouseButton::WheelDown
            };
            for _ in 0..whole.unsigned_abs().min(10) {
                self.report_mouse(button, MouseAction::Press, event.position, &event.modifiers);
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let key: &[u8] = match (whole > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.session
                .write(key.repeat(whole.unsigned_abs() as usize));
        } else {
            self.session
                .term
                .lock()
                .scroll_display(Scroll::Delta(whole));
            cx.notify();
        }
    }

    fn copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            if self.behavior.clear_selection_on_copy && !self.behavior.copy_on_select {
                self.session.term.lock().selection = None;
                self.kb_select = None;
                cx.notify();
            }
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        self.paste_clipboard(window, cx);
    }

    /// Pastes `text` as the program asked for it (bracketed or not), or
    /// bracketed in any case with `force_bracketed`.
    pub fn paste_text(&mut self, text: &str, force_bracketed: bool, window: &mut Window) {
        if self.read_only || self.vi.is_some() {
            window.play_system_bell();
            return;
        }
        let mut mode = self.term_mode();
        if force_bracketed {
            mode.insert(TermMode::BRACKETED_PASTE);
        }
        let bytes = input::paste_bytes(text, mode);
        self.session.term.lock().scroll_display(Scroll::Bottom);
        self.session.write(bytes);
    }

    /// Edit › Paste as › Paste Selection: the selected text, pasted.
    fn paste_selection(&mut self, _: &PasteSelection, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.session.term.lock().selection_to_string();
        match text {
            Some(text) => self.paste_checked(text, window, cx),
            None => window.play_system_bell(),
        }
        cx.notify();
    }

    /// Paste Escaping Special Characters: the clipboard with each character
    /// a shell treats specially escaped, as one word.
    fn paste_escaped(&mut self, _: &PasteEscaped, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            self.paste_text(&shell_escape(&text), false, window);
        }
    }

    fn paste_bracketed(&mut self, _: &PasteBracketed, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            self.paste_text(&text, true, window);
        }
    }

    /// Insert from › File Path…: the chosen files' paths, escaped.
    fn insert_file_path(
        &mut self,
        _: &InsertFilePath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Insert".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let text = paths
                .iter()
                .map(|p| shell_escape(&p.to_string_lossy()))
                .collect::<Vec<_>>()
                .join(" ");
            let _ = this.update_in(cx, |this, window, _| this.paste_text(&text, false, window));
        })
        .detach();
    }

    fn select_all(&mut self, _: &SelectAll, _window: &mut Window, cx: &mut Context<Self>) {
        let mut term = self.session.term.lock();
        let top = term.grid().topmost_line();
        let bottom = term.grid().bottommost_line();
        let last = term.grid().last_column();
        let mut sel = Selection::new(
            SelectionType::Simple,
            Point::new(top, Column(0)),
            Side::Left,
        );
        sel.update(Point::new(bottom, last), Side::Right);
        term.selection = Some(sel);
        cx.notify();
    }

    /// Sends a key's bytes as typed input (back at the bottom, like typing).
    fn send(&mut self, bytes: &'static [u8], cx: &mut Context<Self>) {
        self.session.term.lock().scroll_display(Scroll::Bottom);
        self.session.write(bytes);
        cx.notify();
    }

    fn scroll(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        self.session.term.lock().scroll_display(scroll);
        cx.notify();
    }

    /// Clears scrollback and screen, keeping the prompt line (like ⌘K elsewhere).
    fn clear(&mut self, _: &Clear, _window: &mut Window, cx: &mut Context<Self>) {
        let mut term = self.session.term.lock();
        term.grid_mut().clear_history();
        term.selection = None;
        drop(term);
        // Ask the shell to redraw its prompt at the top.
        self.session.write(b"\x0c".as_slice());
        cx.notify();
    }

    /// Clear Scrollback Above: what scrolled off is gone; the screen stays.
    fn clear_scrollback(
        &mut self,
        _: &ClearScrollbackAbove,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut term = self.session.term.lock();
        term.grid_mut().clear_history();
        term.selection = None;
        drop(term);
        cx.notify();
    }

    fn zoom(&mut self, delta: f32, cx: &mut Context<Self>) {
        let mut style = (*self.style).clone();
        let size = if delta == 0. {
            self.base_font_size
        } else {
            (f32::from(style.font_size) + delta).clamp(6., 72.)
        };
        style.font_size = gpui::px(size);
        self.style = Rc::new(style);
        cx.notify();
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Re-arm before reading so output during this frame schedules another.
        self.session.take_wakeup();
        // Focus survives deactivation: the focused pane of an inactive window
        // shows a hollow cursor, and other panes (unfocused splits) none.
        let focused = self.focus_handle.is_focused(window);
        let active = focused && window.is_window_active();
        let entity = cx.entity().downgrade();
        let input_entity = cx.entity();
        let focus = self.focus_handle.clone();
        if focused {
            self.update_suggestion();
        }
        let element = TerminalElement::new(
            self.session.term.clone(),
            self.style.clone(),
            active,
            focused
                && (!active || self.vi.is_some() || self.blink.as_ref().is_none_or(|b| b.visible)),
            self.layout.clone(),
            move |size, _window, cx| {
                if let Some(view) = entity.upgrade() {
                    view.update(cx, |view, _| view.session.resize(size));
                }
            },
        )
        .preedit(self.marked_text.clone())
        .ghost(focused.then(|| self.ghost_text()).flatten())
        .highlights(self.find_highlights())
        .underlines(self.underlines())
        .labels(self.hint_labels())
        .on_paint(move |bounds, window, cx| {
            window.handle_input(&focus, ElementInputHandler::new(bounds, input_entity), cx);
        });
        let screen = div()
            .id("terminal-screen")
            .relative()
            .flex_1()
            .min_h_0()
            .cursor(if self.hovered_link.is_some() {
                gpui::CursorStyle::PointingHand
            } else {
                self.session
                    .shell()
                    .pointer
                    .map_or(gpui::CursorStyle::IBeam, pointer_style)
            })
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.pointer = None;
                    this.update_hovered_link(None, &gpui::Modifiers::default(), cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_right_mouse_up))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_drag_move(cx.listener(
                |this, event: &gpui::DragMoveEvent<gpui::ExternalPaths>, _, cx| {
                    this.drag_moved(event, cx)
                },
            ))
            .on_drop(
                cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                    this.dropped(paths, window, cx)
                }),
            )
            .child(element)
            .children(self.render_link_status())
            .children(self.render_vi_key_hints())
            .children(self.render_find_bar(cx))
            .children(self.render_drop_zone(cx));
        let root = div().id("terminal");
        self.register_copy_as(root, cx)
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .on_modifiers_changed(cx.listener(
                |this, event: &gpui::ModifiersChangedEvent, window, cx| {
                    this.report_modifier(event, window);
                    this.update_hovered_link(None, &event.modifiers, cx)
                },
            ))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_action(
                cx.listener(|this, _: &ToggleComposer, window, cx| {
                    this.toggle_composer(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &TogglePromptQueue, window, cx| {
                    this.toggle_queue(window, cx)
                }),
            )
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::clear_scrollback))
            .on_action(cx.listener(Self::paste_selection))
            .on_action(cx.listener(Self::paste_escaped))
            .on_action(cx.listener(Self::paste_bracketed))
            .on_action(cx.listener(Self::insert_file_path))
            .on_action(cx.listener(|this, _: &AcceptSuggestion, _, cx| {
                this.accept_suggestion(cx);
            }))
            .on_action(cx.listener(|this, _: &CopyPath, _, cx| {
                if let Some(cwd) = &this.cwd {
                    cx.write_to_clipboard(ClipboardItem::new_string(cwd.display().to_string()));
                }
            }))
            .on_action(cx.listener(|this, _: &RevealInFinder, _, cx| {
                if let Some(cwd) = &this.cwd {
                    cx.reveal_path(cwd);
                }
            }))
            .on_action(cx.listener(|this, a: &SendText, window, cx| {
                if this.read_only {
                    window.play_system_bell();
                } else {
                    this.write_input(&a.text, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CursorLineStart, _, cx| this.send(b"\x01", cx)))
            .on_action(cx.listener(|this, _: &CursorLineEnd, _, cx| this.send(b"\x05", cx)))
            .on_action(cx.listener(|this, _: &CursorWordLeft, _, cx| this.send(b"\x1bb", cx)))
            .on_action(cx.listener(|this, _: &CursorWordRight, _, cx| this.send(b"\x1bf", cx)))
            .on_action(cx.listener(|this, _: &DeleteToLineStart, _, cx| this.send(b"\x15", cx)))
            .on_action(cx.listener(|this, _: &DeleteToLineEnd, _, cx| this.send(b"\x0b", cx)))
            .on_action(cx.listener(|this, _: &DeleteWordLeft, _, cx| this.send(b"\x1b\x7f", cx)))
            .on_action(cx.listener(|this, _: &DeleteWordRight, _, cx| this.send(b"\x1bd", cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.send(b"\x1f", cx)))
            .on_action(cx.listener(|this, _: &ScrollToTop, _, cx| this.scroll(Scroll::Top, cx)))
            .on_action(
                cx.listener(|this, _: &ScrollToBottom, _, cx| this.scroll(Scroll::Bottom, cx)),
            )
            .on_action(cx.listener(|this, _: &ScrollPageUp, _, cx| this.scroll(Scroll::PageUp, cx)))
            .on_action(
                cx.listener(|this, _: &ScrollPageDown, _, cx| this.scroll(Scroll::PageDown, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ScrollLineUp, _, cx| this.scroll(Scroll::Delta(1), cx)),
            )
            .on_action(
                cx.listener(|this, _: &ScrollLineDown, _, cx| this.scroll(Scroll::Delta(-1), cx)),
            )
            .on_action(cx.listener(|this, _: &IncreaseFontSize, _, cx| this.zoom(1., cx)))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, _, cx| this.zoom(-1., cx)))
            .on_action(cx.listener(|this, _: &ResetFontSize, _, cx| this.zoom(0., cx)))
            .on_action(cx.listener(|this, _: &ToggleViMode, _, cx| this.toggle_vi_mode(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleViKeyHints, _, cx| this.toggle_vi_key_hints(cx)),
            )
            .on_action(cx.listener(|this, _: &HintOpen, window, cx| {
                this.start_hints(Matcher::Link, HintAction::Open, window, cx)
            }))
            .on_action(cx.listener(|this, _: &HintCopy, window, cx| {
                this.start_hints(Matcher::Link, HintAction::Copy, window, cx)
            }))
            .on_action(cx.listener(|this, _: &HintPaste, window, cx| {
                this.start_hints(Matcher::Word, HintAction::Paste, window, cx)
            }))
            .on_action(cx.listener(|this, _: &HintReveal, window, cx| {
                this.start_hints(Matcher::Path, HintAction::Reveal, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleReadOnly, _, cx| {
                this.set_read_only(!this.read_only, cx)
            }))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_find(window, cx)))
            .on_action(
                cx.listener(|this, _: &FindNext, window, cx| this.find_step(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FindPrevious, window, cx| this.find_step(false, window, cx)),
            )
            .child(screen)
            .children(self.render_composer(cx))
    }
}

/// IME integration: composed text arrives here instead of as key events, and
/// marked (in-progress) text is shown at the cursor until committed.
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self
            .marked_text
            .as_ref()
            .map_or(0, |t| t.encode_utf16().count());
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.marked_text
            .as_ref()
            .map(|t| 0..t.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = None;
        self.send_text(text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        new_text: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = (!new_text.is_empty()).then(|| SharedString::from(new_text.to_owned()));
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.get()?;
        let (row, col) = layout.cursor?;
        Some(layout.cell_bounds(row, col))
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// `text` with every character a shell treats specially backslash-escaped.
/// The branch checked out in `dir`'s repository, read from `HEAD` (a
/// worktree's `.git` file points at its own); a short hash when detached.
pub fn git_branch(dir: &std::path::Path) -> Option<String> {
    let dot = dir
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|p| p.exists())?;
    let git = if dot.is_file() {
        let text = std::fs::read_to_string(&dot).ok()?;
        let target = text.trim().strip_prefix("gitdir:")?.trim();
        dot.parent()?.join(target)
    } else {
        dot
    };
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => Some(branch.to_owned()),
        None => Some(head.get(..7)?.to_owned()),
    }
}

pub fn shell_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\n' {
            out.push_str("'\n'");
            continue;
        }
        if " \t'\"\\$`!&*()[]{};<>|?#~=%".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The macOS pointer for a shape a program set with OSC 22 (CSS names).
fn pointer_style(icon: motty_term::CursorIcon) -> gpui::CursorStyle {
    use gpui::CursorStyle as S;
    use motty_term::CursorIcon as I;
    match icon {
        I::Text => S::IBeam,
        I::VerticalText => S::IBeamCursorForVerticalLayout,
        I::Pointer => S::PointingHand,
        I::Crosshair | I::Cell => S::Crosshair,
        I::Grab | I::Move | I::AllScroll => S::OpenHand,
        I::Grabbing => S::ClosedHand,
        I::NotAllowed | I::NoDrop => S::OperationNotAllowed,
        I::Alias => S::DragLink,
        I::Copy => S::DragCopy,
        I::ContextMenu => S::ContextualMenu,
        I::EResize => S::ResizeRight,
        I::WResize => S::ResizeLeft,
        I::NResize => S::ResizeUp,
        I::SResize => S::ResizeDown,
        I::EwResize => S::ResizeLeftRight,
        I::NsResize => S::ResizeUpDown,
        I::NwseResize | I::NwResize | I::SeResize => S::ResizeUpLeftDownRight,
        I::NeswResize | I::NeResize | I::SwResize => S::ResizeUpRightDownLeft,
        I::ColResize => S::ResizeColumn,
        I::RowResize => S::ResizeRow,
        _ => S::Arrow,
    }
}
