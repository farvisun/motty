//! The bar above a file pane, as Otty draws it: the Read-Only (or Preview)
//! switch, Send to Chat and Share on the left; the language, whether it's
//! saved and Close on the right.

use gpui::{AnyElement, Context, Hsla, MouseButton, Window, div, prelude::*, px};
use motty_render::hsla;
use motty_theme::Rgba;

use super::{EditorEvent, FileEditor, button_row};

const HEIGHT: f32 = 32.;
/// The switch, measured from Otty: a 45×21 track, a 19pt knob.
const SWITCH_W: f32 = 45.;
const SWITCH_H: f32 = 21.;
const KNOB: f32 = 19.;
/// Icon buttons: 13pt glyphs, 28pt apart.
const ICON: f32 = 14.;
const ICON_BOX: f32 = 24.;

impl FileEditor {
    /// Whether the toolbar has the language dropdown: for an extension more
    /// than one language uses (Otty's `.m`, `.h`, `.v`, `.pl`), or once one
    /// was picked.
    fn shows_language(&self) -> bool {
        self.language_override.is_some()
            || self.path.as_deref().is_some_and(|p| {
                matches!(
                    crate::file_kind::extension(p).as_str(),
                    "m" | "h" | "v" | "pl"
                )
            })
    }

    /// The language the file is colored as; picking one overrides what was
    /// detected, until Auto-detect.
    fn language_dropdown(
        &self,
        ink: impl Fn(f32) -> Hsla + Copy + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.language();
        div()
            .id("editor-language")
            .flex_none()
            .h(px(24.))
            .pl(px(6.))
            .pr(px(4.))
            .flex()
            .items_center()
            .gap(px(3.))
            .rounded(px(5.))
            .text_size(px(12.))
            .text_color(ink(0.6))
            .hover(move |s| s.bg(ink(0.07)))
            .tooltip(|_, cx| crate::tooltip::text("Language", cx))
            .child(current.unwrap_or("Plain Text"))
            .child(
                gpui::svg()
                    .path("icons/chevron-down.svg")
                    .size(px(11.))
                    .text_color(ink(0.5)),
            )
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    use crate::native_menu::{Entry, Item};
                    let languages = super::highlight::languages();
                    let auto = this.language_override.is_none();
                    let mut entries: Vec<Entry> = vec![
                        Item::new("Auto-detect").checked(auto).into(),
                        Entry::Separator,
                        Item::new("Plain Text")
                            .checked(!auto && current.is_none())
                            .into(),
                    ];
                    entries.extend(
                        languages
                            .iter()
                            .map(|&l| Item::new(l).checked(!auto && current == Some(l)).into()),
                    );
                    let editor = cx.entity().downgrade();
                    crate::native_menu::popup(
                        entries,
                        event.position(),
                        None,
                        window,
                        cx,
                        move |ix, _, cx| {
                            let pick = match ix {
                                0 => None,
                                1 => Some(None),
                                ix => Some(languages.get(ix - 2).copied()),
                            };
                            let _ = editor.update(cx, |this, cx| this.set_language(pick, cx));
                        },
                    );
                }),
            )
    }

    pub(super) fn render_toolbar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let fg = self.foreground();
        let ink = move |a: f32| {
            let mut c = fg;
            c.a = a;
            hsla(c)
        };
        let accent = hsla(Rgba::rgb(0x047afe));
        let active = window.is_window_active();
        let icon = |id: &'static str, path: &'static str, tip: &'static str| {
            div()
                .id(id)
                .flex_none()
                .size(px(ICON_BOX))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .hover(move |s| s.bg(ink(0.08)))
                .tooltip(move |_, cx| crate::tooltip::text(tip, cx))
                .child(gpui::svg().path(path).size(px(ICON)).text_color(ink(0.8)))
        };
        let file = self.path.is_some();

        let mut left = div().flex().items_center().gap(px(4.));
        if file && !self.lockable && !self.diff && self.transcript.is_none() {
            let on = self.read_only;
            let enabled = self.can_edit;
            let track = match (on, active) {
                (true, true) => accent,
                _ => ink(0.12),
            };
            left = left.child(
                div()
                    .id("editor-read-only")
                    .flex_none()
                    .mr(px(12.))
                    .w(px(SWITCH_W))
                    .h(px(SWITCH_H))
                    .rounded_full()
                    .bg(track)
                    .relative()
                    .when(!enabled, |d| d.opacity(0.5))
                    .tooltip(|_, cx| crate::tooltip::text("Read Only", cx))
                    .child(
                        div()
                            .absolute()
                            .top(px((SWITCH_H - KNOB) / 2.))
                            .left(px(if on {
                                SWITCH_W - KNOB - (SWITCH_H - KNOB) / 2.
                            } else {
                                (SWITCH_H - KNOB) / 2.
                            }))
                            .size(px(KNOB))
                            .rounded_full()
                            .bg(gpui::white())
                            .shadow_sm(),
                    )
                    .when(enabled, |d| {
                        d.on_click(cx.listener(|this, _, window, cx| {
                            this.set_read_only(!this.read_only, cx);
                            if !this.read_only {
                                window.focus(&this.focus, cx);
                            }
                        }))
                    }),
            );
        }
        if self.lockable {
            let locked = self.read_only;
            left = left.child(
                div()
                    .id("editor-lock")
                    .h(px(24.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .text_color(ink(0.5))
                    .hover(move |s| s.bg(ink(0.07)))
                    .child(
                        gpui::svg()
                            .path(if locked {
                                "icons/lock.svg"
                            } else {
                                "icons/lock-open.svg"
                            })
                            .size(px(13.))
                            .text_color(ink(0.6)),
                    )
                    .child(if locked { "Read Only" } else { "Editing" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.read_only = !this.read_only;
                        cx.notify();
                    })),
            );
        }
        left = left
            .when(!self.diff, |d| {
                d.child(
                    icon("editor-chat", "icons/message-square.svg", "Send to Chat")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::actions::SendToChat), cx)
                        }),
                )
            })
            .when_some(self.path.clone().filter(|_| file), |d, path| {
                d.child(icon("editor-share", "icons/share.svg", "Share").on_click(
                    move |event, window, _| {
                        crate::share::show(
                            window,
                            crate::share::Shared::File(&path),
                            event.position(),
                        )
                    },
                ))
            });
        if self.read_only
            && !self.lockable
            && !self.can_edit
            && !self.diff
            && self.transcript.is_none()
        {
            left = left.child(
                div()
                    .ml(px(6.))
                    .text_size(px(12.))
                    .text_color(ink(0.5))
                    .child("Read Only"),
            );
        }
        left = left.children(self.error.clone().map(|e| {
            div()
                .ml(px(6.))
                .text_size(px(12.))
                .text_color(ink(0.5))
                .child(e)
        }));

        let dirty = self.is_dirty();
        let label = |id: &'static str, glyph: &'static str, text: &'static str, color: Hsla| {
            div()
                .id(id)
                .flex_none()
                .h(px(24.))
                .px(px(4.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(5.))
                .text_size(px(13.))
                .text_color(color)
                .child(gpui::svg().path(glyph).size(px(13.)).text_color(color))
                .child(text)
        };
        let right = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .when(file && self.shows_language(), |d| {
                d.child(self.language_dropdown(ink, cx))
            })
            .when(file && !self.read_only, |d| {
                if dirty {
                    d.child(
                        label("editor-revert", "icons/rotate-cw.svg", "Revert", ink(0.6))
                            .hover(move |s| s.bg(ink(0.07)))
                            .on_click(cx.listener(|this, _, _, cx| this.discard(cx))),
                    )
                    .child(
                        label("editor-save", "icons/check.svg", "Save", accent)
                            .hover(move |s| s.bg(ink(0.07)))
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                } else {
                    d.child(label("editor-saved", "icons/check.svg", "Saved", ink(0.5)))
                }
            })
            .when_some(self.transcript.as_ref(), |d, session| {
                // Continues the session: its live tab if it's still running.
                let resume = crate::actions::ResumeSession {
                    agent: session.agent.to_owned(),
                    id: session.id.clone(),
                    cwd: session.cwd.clone(),
                    fork: false,
                    place: "tab".into(),
                };
                d.child(
                    label(
                        "editor-resume",
                        "icons/circle-play.svg",
                        "Resume",
                        ink(0.85),
                    )
                    .hover(move |s| s.bg(ink(0.07)))
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(Box::new(resume.clone()), cx)
                    }),
                )
            })
            .when(!self.lockable, |d| {
                d.child(
                    label("editor-close", "icons/x.svg", "Close", ink(0.9))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .hover(move |s| s.bg(ink(0.07)))
                        .on_click(cx.listener(|this, _, window, cx| {
                            // ⌘W on this pane: it asks first when unsaved.
                            window.focus(&this.focus, cx);
                            window.defer(cx, |window, cx| {
                                window.dispatch_action(Box::new(crate::actions::CloseTab), cx)
                            });
                        })),
                )
            });

        div()
            .flex_none()
            .h(px(HEIGHT))
            .pl(px(10.))
            .pr(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .child(left)
            .child(right)
            .into_any_element()
    }

    /// Over the text when the file changed on disk under unsaved edits.
    pub(super) fn render_disk_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let fg = self.foreground();
        let ink = move |a: f32| {
            let mut c = fg;
            c.a = a;
            hsla(c)
        };
        div()
            .flex_none()
            .h(px(30.))
            .mx(px(10.))
            .mb(px(4.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(7.))
            .bg(ink(0.06))
            .text_size(px(12.))
            .text_color(ink(0.75))
            .child(
                div()
                    .flex_1()
                    .child(format!("“{}” changed on disk.", self.title())),
            )
            .child(
                button_row("disk-reload", "Reload".into(), ink(0.9), ink(0.08))
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .child(
                button_row("disk-keep", "Keep My Changes".into(), ink(0.9), ink(0.08))
                    .on_click(cx.listener(|this, _, _, cx| this.keep_changes(cx))),
            )
            .into_any_element()
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.read_only = read_only || !self.can_edit;
        self.anchor = None;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }
}
