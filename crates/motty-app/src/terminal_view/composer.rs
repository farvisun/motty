//! The Composer (⇧⌘E) and the Prompt Queue (⇧⌘M), at the bottom of the
//! pane. The Composer is a roomy editor for a long prompt or command:
//! ⌘↩ sends it, ⌥⌘↩ queues it, ⎋ closes it and keeps the draft. The queue
//! sends one prompt at a time: an agent's when its turn ends, a shell's
//! when it's back at an empty prompt.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, BoxShadow, Context, Entity, Focusable, Hsla, KeyDownEvent, MouseButton,
    SharedString, Window, div, point, prelude::*, px,
};
use motty_render::hsla;
use motty_theme::Rgba;

use super::TerminalView;
use super::find::Colors;
use crate::actions::Paste;
use crate::editor::FileEditor;
use crate::titlebar::{icon, stroke};

const INSET: f32 = 8.;
const RADIUS: f32 = 10.;
const BUTTON: f32 = 22.;
const ROW: f32 = 26.;
/// The Composer grows this tall before it scrolls.
const MAX_HEIGHT: f32 = 220.;
const FONT_SIZE: f32 = 13.;
/// A queued prompt that shows no sign of running is taken as done after this.
const TURN_GRACE: Duration = Duration::from_secs(5);
/// Return follows the text after this, so a TUI doesn't take it as pasted.
const RETURN_DELAY: Duration = Duration::from_millis(60);
const ACCENT: u32 = 0x047afe;

/// The Composer stays on screen when its pane's tab isn't the active one.
#[derive(Default)]
struct Pinned(bool);

impl gpui::Global for Pinned {}

pub fn is_pinned(cx: &gpui::App) -> bool {
    cx.try_global::<Pinned>().is_some_and(|p| p.0)
}

/// Context from Send to Chat, or a pasted picture, sent ahead of the draft.
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    /// What it is: `Selection`, `Output of cargo test`.
    pub label: String,
    /// The context; a picture's file.
    pub text: String,
    /// A picture, sent as its path for the agent to read.
    pub image: bool,
}

impl Chip {
    /// The `n`th picture pasted into the draft, saved at `path`.
    pub(super) fn image(path: &std::path::Path, n: usize) -> Self {
        Self {
            label: format!("Image {n}"),
            text: path.to_string_lossy().into_owned(),
            image: true,
        }
    }
}

pub(super) struct Composer {
    pub(super) editor: Entity<FileEditor>,
    pub(super) open: bool,
    chips: Vec<Chip>,
}

#[derive(Default)]
pub(super) struct Queue {
    pub(super) items: Vec<String>,
    /// Where prompts are typed (⇧⌘M); kept while hidden.
    input: Option<Entity<FileEditor>>,
    bar: bool,
    /// The item being edited in the bar; sending waits meanwhile.
    editing: Option<usize>,
    /// When the last item went out: the next waits for its turn to end.
    sent: Option<Instant>,
    /// Recipe commands typed one at a time, each at an empty prompt.
    pub(super) typed: Vec<String>,
    /// Commands a recipe ran here, which saving a recipe leaves out.
    pub(super) replayed: Vec<String>,
}

/// The chips as fenced blocks (pictures as their paths), then the draft.
fn compose(chips: &[Chip], draft: &str) -> String {
    let mut text = String::new();
    for chip in chips {
        if chip.image {
            text.push_str(&format!("{}\n\n", super::shell_escape(&chip.text)));
            continue;
        }
        let body = chip.text.trim_end_matches('\n');
        text.push_str(&format!("{}:\n```\n{body}\n```\n\n", chip.label));
    }
    text.push_str(draft.trim_end());
    text.trim().to_owned()
}

/// Queued from the Composer: each non-blank line is a prompt of its own,
/// unless context goes with them.
fn queue_items(chips: &[Chip], draft: &str) -> Vec<String> {
    if !chips.is_empty() {
        let text = compose(chips, draft);
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![text]
        };
    }
    draft
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

impl TerminalView {
    fn text_field(&self, placeholder: &str, cx: &mut Context<Self>) -> Entity<FileEditor> {
        let style = (*self.style).clone();
        let placeholder = placeholder.to_owned();
        cx.new(|cx| {
            let mut field = FileEditor::prose(placeholder, style, cx);
            field.set_font_size(px(FONT_SIZE));
            field
        })
    }

    /// ⇧⌘E: opens the Composer (draft and all), or closes it.
    pub(super) fn toggle_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer.as_ref().is_some_and(|c| c.open) {
            return self.close_composer(window, cx);
        }
        self.open_composer(window, cx);
    }

    pub fn open_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer.is_none() {
            let editor = self.text_field("Write a prompt or a command…", cx);
            editor.update(cx, |e, _| e.set_height(2, px(MAX_HEIGHT)));
            self.composer = Some(Composer {
                editor,
                open: false,
                chips: Vec::new(),
            });
        }
        if let Some(c) = &mut self.composer {
            c.open = true;
            window.focus(&c.editor.focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// ⎋: closes it; the draft stays for next time.
    fn close_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = &mut self.composer {
            c.open = false;
        }
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Pictures among the chips.
    pub(super) fn composer_images(&self) -> usize {
        self.composer
            .as_ref()
            .map_or(0, |c| c.chips.iter().filter(|chip| chip.image).count())
    }

    /// What Send to Chat takes from here: the selection, else the last
    /// command with its output.
    pub fn chat_context(&self) -> Option<Chip> {
        if let Some(text) = self.selected_text() {
            return Some(Chip {
                label: format!("Selection from {}", self.display_title()),
                text,
                image: false,
            });
        }
        let block = self.session.shell().last_finished()?.clone();
        let command = self.session.block_command(&block)?;
        let output = self.session.block_output(&block).unwrap_or_default();
        Some(Chip {
            label: format!("Output of `{}`", truncate(&command, 40)),
            text: format!("$ {command}\n{}", output.trim_end()),
            image: false,
        })
    }

    pub fn composer_open(&self) -> bool {
        self.composer.as_ref().is_some_and(|c| c.open)
    }

    /// Send to Chat: `chip` goes above the draft, the Composer opens.
    pub fn attach(&mut self, chip: Chip, window: &mut Window, cx: &mut Context<Self>) {
        self.open_composer(window, cx);
        if let Some(c) = &mut self.composer
            && !c.chips.contains(&chip)
        {
            c.chips.push(chip);
        }
        cx.notify();
    }

    /// ⌘↩ sends the draft; ⌥⌘↩ (`queue`) puts it on the queue.
    fn send_composer(&mut self, queue: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = &mut self.composer else {
            return;
        };
        let draft = c.editor.read(cx).text();
        let items = if queue {
            queue_items(&c.chips, &draft)
        } else {
            let text = compose(&c.chips, &draft);
            if text.is_empty() {
                Vec::new()
            } else {
                vec![text]
            }
        };
        if items.is_empty() {
            return window.play_system_bell();
        }
        c.chips.clear();
        c.open = false;
        c.editor.update(cx, |e, cx| e.set_text("", cx));
        window.focus(&self.focus_handle, cx);
        if queue {
            self.enqueue(items, window, cx);
        } else {
            self.submit(&items[0], window, cx);
        }
        cx.notify();
    }

    fn on_composer_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        match ks.key.as_str() {
            "enter" if m.platform && !m.shift && !m.control => {
                self.send_composer(m.alt, window, cx)
            }
            "escape" if !m.modified() => self.close_composer(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Types `text` and presses Return, as a person would; several lines
    /// go as one paste.
    pub fn submit(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return window.play_system_bell();
        }
        let text = text.trim_end_matches(['\n', '\r']);
        if text.contains('\n') {
            self.paste_text(text, false, window);
        } else {
            self.write_input(text, cx);
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RETURN_DELAY).await;
            let _ = this.update(cx, |this, cx| this.write_input("\r", cx));
        })
        .detach();
    }

    // The queue

    /// ⇧⌘M: shows the bar to type prompts into, or hides it.
    pub(super) fn toggle_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.queue.bar {
            self.queue.bar = false;
            self.queue.editing = None;
            window.focus(&self.focus_handle, cx);
        } else {
            self.show_queue_bar(window, cx);
        }
        cx.notify();
    }

    fn show_queue_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = match &self.queue.input {
            Some(input) => input.clone(),
            None => {
                let input = self.text_field("Queue a prompt or a command — ↩ to add", cx);
                self.queue.input = Some(input.clone());
                input
            }
        };
        self.queue.bar = true;
        window.focus(&input.focus_handle(cx), cx);
    }

    /// Adds prompts to the end of the queue; one may go out right away.
    pub fn enqueue(&mut self, items: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.queue.items.extend(items);
        self.dispatch(window, cx);
        cx.notify();
    }

    pub fn queued(&self) -> &[String] {
        &self.queue.items
    }

    /// Queued prompts, plus one sent that the pane hasn't finished with.
    pub fn pending(&self) -> usize {
        self.queue.items.len() + usize::from(self.queue.sent.is_some())
    }

    /// Whether the pane would take the next prompt now.
    fn ready_for_queue(&self) -> bool {
        if self.queue.editing.is_some() || self.read_only || self.vi.is_some() {
            return false;
        }
        if let Some(s) = self.status.as_ref().filter(|s| s.agent.is_some()) {
            return !s.in_turn;
        }
        // A shell at an empty prompt; full-screen programs hold the queue.
        self.session
            .prompt_input()
            .is_some_and(|line| line.trim().is_empty())
    }

    /// Sends the next prompt if the pane is ready and the last one is done.
    pub fn dispatch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.queue.typed.is_empty() && self.queue.sent.is_none() && self.ready_for_queue() {
            let next = self.queue.typed.remove(0);
            self.write_input(&next, cx);
            return;
        }
        if self.queue.items.is_empty() || self.queue.sent.is_some() || !self.ready_for_queue() {
            return;
        }
        let item = self.queue.items.remove(0);
        self.send_tracked(&item, window, cx);
    }

    /// Sends `text` now, holding the queue (and `motty wait`) until the
    /// pane is done with it.
    pub fn send_tracked(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let sent = Instant::now();
        self.queue.sent = Some(sent);
        self.submit(text, window, cx);
        // A prompt that starts nothing (an agent's own /command, an empty
        // shell line) never reports an end.
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(TURN_GRACE).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.queue.sent == Some(sent) && this.ready_for_queue() {
                    this.turn_ended(window, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// A shell command finished; the next prompt goes once the shell is
    /// back at its prompt. An agent in the pane reports its own turns.
    pub fn command_done(&mut self) {
        if self.status.as_ref().is_none_or(|s| s.agent.is_none()) {
            self.queue.sent = None;
        }
    }

    /// The agent's turn or the shell's command is over: the next may go.
    pub fn turn_ended(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.queue.sent = None;
        self.dispatch(window, cx);
    }

    fn send_queued_now(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.queue.items.len() {
            let item = self.queue.items.remove(ix);
            self.send_tracked(&item, window, cx);
        }
    }

    fn edit_queued(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.queue.items.get(ix).cloned() else {
            return;
        };
        self.show_queue_bar(window, cx);
        self.queue.editing = Some(ix);
        if let Some(input) = &self.queue.input {
            input.update(cx, |e, cx| {
                e.set_text(&text, cx);
                e.set_placeholder("Edit the prompt — ↩ to save, ⎋ to cancel", cx);
            });
        }
        cx.notify();
    }

    fn remove_queued(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.queue.items.len() {
            self.queue.items.remove(ix);
            match self.queue.editing {
                Some(e) if e == ix => self.queue.editing = None,
                Some(e) if e > ix => self.queue.editing = Some(e - 1),
                _ => {}
            }
            cx.notify();
        }
    }

    fn on_queue_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        let Some(input) = self.queue.input.clone() else {
            return;
        };
        match ks.key.as_str() {
            // ⌥↩ and ⇧↩ break the line.
            "enter" if !m.modified() => {
                let text = input.read(cx).text().trim().to_owned();
                match self.queue.editing.take() {
                    Some(ix) if ix < self.queue.items.len() => {
                        if text.is_empty() {
                            self.queue.items.remove(ix);
                        } else {
                            self.queue.items[ix] = text;
                        }
                    }
                    _ if text.is_empty() => return,
                    _ => self.queue.items.push(text),
                }
                input.update(cx, |e, cx| {
                    e.set_text("", cx);
                    e.set_placeholder("Queue a prompt or a command — ↩ to add", cx);
                });
                self.dispatch(window, cx);
            }
            "escape" if !m.modified() => {
                if self.queue.editing.take().is_some() {
                    input.update(cx, |e, cx| {
                        e.set_text("", cx);
                        e.set_placeholder("Queue a prompt or a command — ↩ to add", cx);
                    });
                    self.dispatch(window, cx);
                } else {
                    self.queue.bar = false;
                    window.focus(&self.focus_handle, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    // Drawing

    /// The queue and the Composer, stacked under the terminal.
    pub fn render_composer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let composer = self.composer.as_ref().filter(|c| c.open);
        let queue = (!self.queue.items.is_empty() || self.queue.bar).then_some(&self.queue);
        if composer.is_none() && queue.is_none() {
            return None;
        }
        let c = Colors::new(self.style.panel_background, self.style.dark);
        let accent = hsla(Rgba::rgb(ACCENT));
        Some(
            div()
                .id("composer-area")
                .flex_none()
                .px(px(INSET))
                .pb(px(INSET))
                .flex()
                .flex_col()
                .gap(px(6.))
                .cursor(gpui::CursorStyle::Arrow)
                // Clicks here aren't the terminal's.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .children(queue.map(|q| self.render_queue(q, &c, cx)))
                .children(composer.map(|composer| self.render_card(composer, &c, accent, cx)))
                .into_any_element(),
        )
    }

    fn render_queue(&self, q: &Queue, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let waiting = !self.ready_for_queue() || q.sent.is_some();
        let status = match (&self.status, waiting) {
            _ if q.editing.is_some() => "Paused while editing",
            (Some(s), true) if s.agent.is_some() => "Waiting for the agent",
            (_, true) => "Waiting for the prompt",
            _ => "Ready",
        };
        let rows = q.items.iter().enumerate().map(|(ix, item)| {
            let line = item.lines().next().unwrap_or_default().to_owned();
            let more = item.lines().count() > 1;
            div()
                .id(("queued", ix))
                .h(px(ROW))
                .px(px(4.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(px(6.))
                .when(q.editing == Some(ix), |d| d.bg(c.toggle_border))
                .child(
                    button(("queue-send", ix), send_now_icon(c.secondary), c).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.send_queued_now(ix, window, cx)
                        }),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(c.text)
                        .child(SharedString::from(if more {
                            format!("{line} …")
                        } else {
                            line
                        })),
                )
                .child(
                    button(("queue-edit", ix), pencil_icon(c.secondary), c).on_click(
                        cx.listener(move |this, _, window, cx| this.edit_queued(ix, window, cx)),
                    ),
                )
                .child(
                    button(("queue-remove", ix), trash_icon(c.secondary), c)
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_queued(ix, cx))),
                )
        });
        card(c)
            .id("prompt-queue")
            .child(
                div()
                    .h(px(ROW))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(11.5))
                    .text_color(c.secondary)
                    .child(format!("Queue · {}", q.items.len()))
                    .child(div().flex().items_center().gap(px(6.)).child(status).when(
                        q.bar,
                        |d| {
                            d.child(button("queue-close", close_icon(c.secondary), c).on_click(
                                cx.listener(|this, _, window, cx| this.toggle_queue(window, cx)),
                            ))
                        },
                    )),
            )
            .when(!q.items.is_empty(), |d| {
                d.child(
                    div()
                        .id("queue-rows")
                        .max_h(px(ROW * 5.))
                        .overflow_y_scroll()
                        .px(px(4.))
                        .children(rows),
                )
            })
            .when_some(q.input.clone().filter(|_| q.bar), |d, input| {
                d.child(
                    div()
                        .mx(px(8.))
                        .mb(px(8.))
                        .mt(px(2.))
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .border(px(0.5))
                        .border_color(c.border)
                        .capture_key_down(cx.listener(Self::on_queue_key))
                        .child(input),
                )
            })
            .into_any_element()
    }

    fn render_card(
        &self,
        composer: &Composer,
        c: &Colors,
        accent: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pinned = is_pinned(cx);
        let agent = self.status.as_ref().and_then(|s| s.agent.clone());
        let title = match agent {
            Some(agent) => format!("Composer · {agent}"),
            None => "Composer".to_owned(),
        };
        let chips = composer.chips.iter().enumerate().map(|(ix, chip)| {
            let lines = chip.text.lines().count();
            let label = if chip.image {
                chip.label.clone()
            } else {
                format!(
                    "{} · {lines} line{}",
                    chip.label,
                    if lines == 1 { "" } else { "s" }
                )
            };
            div()
                .id(("chip", ix))
                .max_w(px(320.))
                .h(px(22.))
                .pl(px(8.))
                .pr(px(2.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(px(6.))
                .border(px(0.5))
                .border_color(accent.opacity(0.35))
                .bg(accent.opacity(0.1))
                .text_size(px(11.5))
                .text_color(c.text)
                .child(div().min_w_0().truncate().child(SharedString::from(label)))
                .child(
                    div()
                        .id(("chip-remove", ix))
                        .flex_none()
                        .rounded(px(4.))
                        .hover(|d| d.bg(c.toggle_border))
                        .child(crate::titlebar::small_close(c.secondary))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(c) = &mut this.composer
                                && ix < c.chips.len()
                            {
                                c.chips.remove(ix);
                                cx.notify();
                            }
                        })),
                )
        });
        let header = div()
            .h(px(ROW + 2.))
            .pl(px(12.))
            .pr(px(6.))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(11.5))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(c.secondary)
            .child(SharedString::from(title))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        button("composer-queue", queue_icon(c.secondary), c).on_click(
                            cx.listener(|this, _, window, cx| this.send_composer(true, window, cx)),
                        ),
                    )
                    .child(
                        button("composer-pin", pin_icon(c.secondary, pinned), c)
                            .when(pinned, |d| d.bg(c.toggle_border))
                            .on_click(|_, _, cx| {
                                cx.set_global(Pinned(!is_pinned(cx)));
                                cx.refresh_windows();
                            }),
                    )
                    .child(
                        button("composer-close", close_icon(c.secondary), c).on_click(
                            cx.listener(|this, _, window, cx| this.close_composer(window, cx)),
                        ),
                    ),
            );
        let footer = div()
            .h(px(ROW + 4.))
            .pl(px(12.))
            .pr(px(6.))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(11.))
            .text_color(c.secondary)
            .child("⌘↩ Send   ⌥⌘↩ Queue   Esc Close")
            .child(
                div()
                    .id("composer-send")
                    .h(px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .bg(accent)
                    .text_color(gpui::white())
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .hover(|d| d.opacity(0.9))
                    .child("Send")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.send_composer(false, window, cx)),
                    ),
            );
        card(c)
            .id("composer")
            .capture_key_down(cx.listener(Self::on_composer_key))
            .capture_action(cx.listener(|this, action: &Paste, window, cx| {
                this.composer_paste(action, window, cx)
            }))
            .child(header)
            .when(!composer.chips.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(10.))
                        .pb(px(4.))
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .children(chips),
                )
            })
            .child(div().px(px(12.)).py(px(2.)).child(composer.editor.clone()))
            .child(footer)
            .into_any_element()
    }
}

fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_owned(),
    }
}

fn card(c: &Colors) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(RADIUS))
        .border(px(0.5))
        .border_color(c.border)
        .bg(c.card)
        .shadow(vec![BoxShadow {
            color: c.shadow.opacity(0.5),
            offset: point(px(0.), px(2.)),
            blur_radius: px(8.),
            spread_radius: px(0.),
            inset: false,
        }])
}

fn button(
    id: impl Into<gpui::ElementId>,
    glyph: impl IntoElement,
    c: &Colors,
) -> gpui::Stateful<gpui::Div> {
    let hover = c.toggle_border;
    div()
        .id(id)
        .flex_none()
        .w(px(BUTTON))
        .h(px(BUTTON))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .hover(move |d| d.bg(hover))
        .child(glyph)
}

/// ↳: send this one now.
fn send_now_icon(color: Hsla) -> impl IntoElement {
    icon(BUTTON, BUTTON, move |o, window| {
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(7.5, 6.5));
            p.line_to(at(7.5, 13.));
            p.line_to(at(15., 13.));
        });
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(12., 10.));
            p.line_to(at(15., 13.));
            p.line_to(at(12., 16.));
        });
    })
}

fn pencil_icon(color: Hsla) -> impl IntoElement {
    icon(BUTTON, BUTTON, move |o, window| {
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(7., 15.));
            p.line_to(at(7., 13.));
            p.line_to(at(13.5, 6.5));
            p.line_to(at(15.5, 8.5));
            p.line_to(at(9., 15.));
            p.close();
        });
    })
}

fn trash_icon(color: Hsla) -> impl IntoElement {
    icon(BUTTON, BUTTON, move |o, window| {
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(6.5, 7.5));
            p.line_to(at(15.5, 7.5));
        });
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(9.5, 7.5));
            p.line_to(at(9.5, 6.));
            p.line_to(at(12.5, 6.));
            p.line_to(at(12.5, 7.5));
        });
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(7.5, 7.5));
            p.line_to(at(8.2, 16.));
            p.line_to(at(13.8, 16.));
            p.line_to(at(14.5, 7.5));
        });
    })
}

/// Three lines and an arrow into them: add to the queue.
fn queue_icon(color: Hsla) -> impl IntoElement {
    icon(BUTTON, BUTTON, move |o, window| {
        for y in [7., 11., 15.] {
            stroke(o, color, window, 1.2, |p, at| {
                p.move_to(at(10., y));
                p.line_to(at(16., y));
            });
        }
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(5., 9.));
            p.line_to(at(7.5, 11.));
            p.line_to(at(5., 13.));
        });
    })
}

fn pin_icon(color: Hsla, on: bool) -> impl IntoElement {
    icon(BUTTON, BUTTON, move |o, window| {
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(8.5, 5.5));
            p.line_to(at(13.5, 5.5));
            p.line_to(at(12.5, 10.));
            p.line_to(at(14.5, 12.));
            p.line_to(at(7.5, 12.));
            p.line_to(at(9.5, 10.));
            p.close();
        });
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(11., 12.));
            p.line_to(at(11., 16.5));
        });
        if on {
            stroke(o, color, window, 2.2, |p, at| {
                p.move_to(at(10., 8.));
                p.line_to(at(12., 8.));
            });
        }
    })
}

fn close_icon(color: Hsla) -> impl IntoElement {
    icon(BUTTON, BUTTON, move |o, window| {
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(7.5, 7.5));
            p.line_to(at(14.5, 14.5));
        });
        stroke(o, color, window, 1.2, |p, at| {
            p.move_to(at(14.5, 7.5));
            p.line_to(at(7.5, 14.5));
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_chips_before_the_draft() {
        let chip = Chip {
            label: "Output of cargo test".into(),
            text: "ok\n".into(),
            image: false,
        };
        assert_eq!(
            compose(std::slice::from_ref(&chip), "why?\n"),
            "Output of cargo test:\n```\nok\n```\n\nwhy?"
        );
        assert_eq!(compose(&[], "  \n"), "");
    }

    #[test]
    fn sends_pictures_as_their_paths() {
        let picture = Chip::image(std::path::Path::new("/tmp/a b.png"), 1);
        assert_eq!(picture.label, "Image 1");
        assert_eq!(
            compose(&[picture], "what is this?"),
            "/tmp/a\\ b.png\n\nwhat is this?"
        );
    }

    #[test]
    fn queues_each_line() {
        assert_eq!(
            queue_items(&[], "make\n\n  make test \n"),
            ["make", "make test"]
        );
        let chip = Chip {
            label: "Selection".into(),
            text: "x".into(),
            image: false,
        };
        assert_eq!(queue_items(&[chip], "a\nb").len(), 1);
    }
}
