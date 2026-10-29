//! A web page in a pane, as Otty draws it: Close, Back, Forward and Reload,
//! the address, and Share above a private (non-persistent) WKWebView.
//! ⌘[ ⌘] go back and forward, ⌘R reloads, ⌘L edits the address, ⌘F finds.

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use motty_render::{TerminalStyle, hsla};
use motty_term::alacritty_terminal::vte::ansi::NamedColor;
use motty_theme::Rgba;

use crate::editor::{EditorEvent, FileEditor};
use crate::web_view::{WebEvent, WebView, WebViewOptions};

/// The title or address changed.
pub struct WebPaneEvent;

pub struct WebPane {
    focus: FocusHandle,
    web: Entity<WebView>,
    address: Entity<FileEditor>,
    /// The find field, while ⌘F's bar is up, and whether it last matched.
    find: Option<(Entity<FileEditor>, Option<bool>)>,
    url: Option<String>,
    title: Option<String>,
    loading: bool,
    /// The address field had the keyboard on the last render.
    editing: bool,
    can_go_back: bool,
    can_go_forward: bool,
    style: TerminalStyle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<WebPaneEvent> for WebPane {}

impl Focusable for WebPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// What was typed in the address field, as a URL: a web address gets
/// https://, a local one http://, anything else is searched for.
pub fn url_for(typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    if typed.contains("://") || typed.starts_with("about:") || typed.starts_with("data:") {
        return Some(typed.to_owned());
    }
    if typed.starts_with('/') || typed.starts_with("~/") {
        let path = match typed.strip_prefix("~/") {
            Some(rest) => dirs::home_dir()?.join(rest),
            None => typed.into(),
        };
        return Some(crate::preview::file_url(&path));
    }
    let host = typed.split(['/', ':', '?', '#']).next().unwrap_or("");
    let local = host == "localhost"
        || host.parse::<std::net::Ipv4Addr>().is_ok()
        || host.ends_with(".local")
        || host.ends_with(".test");
    if local {
        return Some(format!("http://{typed}"));
    }
    if !typed.contains(char::is_whitespace) && host.contains('.') {
        return Some(format!("https://{typed}"));
    }
    let query: String = form_urlencoded(typed);
    Some(format!("https://duckduckgo.com/?q={query}"))
}

/// The fields type in the system font, not the terminal's.
fn ui_style(style: &TerminalStyle) -> TerminalStyle {
    let mut style = style.clone();
    style.font.family = ".SystemUIFont".into();
    style
}

fn form_urlencoded(text: &str) -> String {
    let mut out = String::new();
    for &b in text.as_bytes() {
        match b {
            b' ' => out.push('+'),
            b if b.is_ascii_alphanumeric() || b"-._~".contains(&b) => out.push(b as char),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl WebPane {
    pub fn new(url: Option<String>, style: TerminalStyle, cx: &mut Context<Self>) -> Self {
        let options = WebViewOptions {
            javascript: true,
            non_persistent: true,
            mobile: false,
            messages: false,
        };
        let web = cx.new(|cx| WebView::new(options, cx));
        let address = cx.new(|cx| {
            let mut field = FileEditor::prose("Search or enter address", ui_style(&style), cx);
            field.set_height(1, px(20.));
            field.set_font_size(px(13.));
            field
        });
        let subscriptions = vec![
            cx.subscribe(&web, Self::on_web_event),
            cx.subscribe(&address, |_, _, _: &EditorEvent, cx| cx.notify()),
        ];
        let mut pane = Self {
            focus: cx.focus_handle(),
            web,
            address,
            find: None,
            url: None,
            title: None,
            loading: false,
            editing: false,
            can_go_back: false,
            can_go_forward: false,
            style,
            _subscriptions: subscriptions,
        };
        pane.apply_style(cx);
        if let Some(url) = url {
            pane.load(&url, cx);
        }
        pane
    }

    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// The page's host, as Otty names the tab; else its title, else New Tab.
    pub fn title(&self) -> String {
        let host = self
            .url
            .as_deref()
            .and_then(|u| u.split("://").nth(1))
            .and_then(|rest| rest.split(['/', '?', '#', ':']).next())
            .filter(|host| !host.is_empty());
        match (host, self.title.as_deref().filter(|t| !t.trim().is_empty())) {
            (Some(host), _) => host.to_owned(),
            (None, Some(title)) => title.to_owned(),
            (None, None) => "New Tab".to_owned(),
        }
    }

    pub fn load(&mut self, url: &str, cx: &mut Context<Self>) {
        self.url = Some(url.to_owned());
        self.web.read(cx).load_url(url);
        self.set_address(url, cx);
        cx.emit(WebPaneEvent);
        cx.notify();
    }

    pub fn set_style(&mut self, style: TerminalStyle, cx: &mut Context<Self>) {
        self.style = style.clone();
        self.address
            .update(cx, |field, cx| field.set_style(ui_style(&style), cx));
        self.apply_style(cx);
    }

    fn apply_style(&mut self, cx: &mut Context<Self>) {
        let dark = self.style.dark;
        let bg = hsla(self.style.background).to_rgb();
        self.web.update(cx, |web, cx| {
            web.set_appearance(dark);
            web.set_background(bg, cx);
        });
    }

    fn set_address(&self, url: &str, cx: &mut Context<Self>) {
        self.address.update(cx, |field, cx| {
            if field.text() != url {
                field.set_text(url, cx);
            }
        });
    }

    fn on_web_event(&mut self, _: Entity<WebView>, event: &WebEvent, cx: &mut Context<Self>) {
        match event {
            WebEvent::UrlChanged(url) => {
                self.url = url.clone();
                // Not while the address is being typed over.
                if let Some(url) = url
                    && !self.editing
                {
                    self.set_address(url, cx);
                }
                cx.emit(WebPaneEvent);
            }
            WebEvent::TitleChanged(title) => {
                self.title = title.clone();
                cx.emit(WebPaneEvent);
            }
            WebEvent::LoadingChanged(loading) => self.loading = *loading,
            WebEvent::NavigationChanged {
                can_go_back,
                can_go_forward,
            } => {
                self.can_go_back = *can_go_back;
                self.can_go_forward = *can_go_forward;
            }
            WebEvent::FindResult { found } => {
                if let Some((_, matched)) = &mut self.find {
                    *matched = Some(*found);
                }
            }
            // Web pages can't post to Motty (`messages` is off).
            WebEvent::Message(_) => {}
        }
        cx.notify();
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        self.web.read(cx).back();
    }

    pub fn forward(&mut self, cx: &mut Context<Self>) {
        self.web.read(cx).forward();
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let web = self.web.read(cx);
        if self.loading {
            web.stop();
        } else {
            web.reload(false);
        }
    }

    fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.web.read(cx).blur_native();
        let focus = self.address.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.address
            .update(cx, |field, cx| field.select_all_text(cx));
    }

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = match &self.find {
            Some((field, _)) => field.clone(),
            None => {
                let style = self.style.clone();
                let field = cx.new(|cx| {
                    let mut field = FileEditor::prose("Find on Page", ui_style(&style), cx);
                    field.set_height(1, px(20.));
                    field.set_font_size(px(13.));
                    field
                });
                self._subscriptions.push(cx.subscribe(
                    &field,
                    |this, field, _: &EditorEvent, cx| {
                        let text = field.read(cx).text();
                        if !text.is_empty() {
                            this.web.update(cx, |web, cx| web.find(&text, false, cx));
                        }
                    },
                ));
                self.find = Some((field.clone(), None));
                field
            }
        };
        self.web.read(cx).blur_native();
        let focus = field.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        window.focus(&self.focus, cx);
        self.web.read(cx).focus_native();
        cx.notify();
    }

    fn on_address_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = &event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "enter" if !m.modified() => {
                let typed = self.address.read(cx).text();
                if let Some(url) = url_for(&typed) {
                    self.load(&url, cx);
                    window.focus(&self.focus, cx);
                    self.web.read(cx).focus_native();
                }
                cx.stop_propagation();
            }
            "escape" if !m.modified() => {
                if let Some(url) = self.url.clone() {
                    self.set_address(&url, cx);
                }
                window.focus(&self.focus, cx);
                self.web.read(cx).focus_native();
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn on_find_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "enter" => {
                if let Some((field, _)) = &self.find {
                    let text = field.read(cx).text();
                    let back = m.shift;
                    self.web.update(cx, |web, cx| web.find(&text, back, cx));
                }
                cx.stop_propagation();
            }
            "escape" => {
                self.close_find(window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn foreground(&self) -> Rgba {
        self.style.palette[NamedColor::Foreground as usize]
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let fg = self.foreground();
        let ink = move |a: f32| {
            let mut c = fg;
            c.a = a;
            hsla(c)
        };
        let icon = |id: &'static str, path: &'static str, tip: &'static str, on: bool| {
            div()
                .id(id)
                .flex_none()
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .when(on, |d| d.hover(move |s| s.bg(ink(0.08))))
                .tooltip(move |_, cx| crate::tooltip::text(tip, cx))
                .child(gpui::svg().path(path).size(px(14.)).text_color(ink(if on {
                    0.8
                } else {
                    0.3
                })))
        };
        let editing = self.editing;
        div()
            .flex_none()
            .h(px(32.))
            .pl(px(10.))
            .pr(px(10.))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                icon("web-close", "icons/x.svg", "Close", true).on_click(cx.listener(
                    |this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        window.defer(cx, |window, cx| {
                            window.dispatch_action(Box::new(crate::actions::CloseTab), cx)
                        });
                    },
                )),
            )
            .child(div().mx(px(4.)).w(px(1.)).h(px(16.)).bg(ink(0.12)))
            .child(
                icon(
                    "web-back",
                    "icons/chevron-left.svg",
                    "Back",
                    self.can_go_back,
                )
                .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
            )
            .child(
                icon(
                    "web-forward",
                    "icons/chevron-right.svg",
                    "Forward",
                    self.can_go_forward,
                )
                .on_click(cx.listener(|this, _, _, cx| this.forward(cx))),
            )
            .child(
                icon(
                    "web-reload",
                    if self.loading {
                        "icons/x.svg"
                    } else {
                        "icons/rotate-cw.svg"
                    },
                    if self.loading { "Stop" } else { "Reload" },
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .child(
                div()
                    .id("web-address")
                    .ml(px(6.))
                    .flex_1()
                    .min_w_0()
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .bg(gpui::black().opacity(if self.style.dark { 0.35 } else { 0.06 }))
                    .when(editing, |d| d.border_1().border_color(ink(0.25)))
                    .capture_key_down(cx.listener(Self::on_address_key))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.web.read(cx).blur_native();
                        }),
                    )
                    .child(div().flex_1().min_w_0().child(self.address.clone())),
            )
            .when_some(self.url.clone(), |d, url| {
                d.child(
                    icon("web-share", "icons/share.svg", "Share", true)
                        .ml(px(4.))
                        .on_click(move |event, window, _| {
                            crate::share::show(
                                window,
                                crate::share::Shared::Url(&url),
                                event.position(),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    fn render_find(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (field, matched) = self.find.as_ref()?;
        let fg = self.foreground();
        let ink = move |a: f32| {
            let mut c = fg;
            c.a = a;
            hsla(c)
        };
        let label: Option<SharedString> = match matched {
            Some(false) => Some("Not found".into()),
            _ => None,
        };
        Some(
            div()
                .flex_none()
                .h(px(30.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(260.))
                        .h(px(22.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .bg(gpui::black().opacity(if self.style.dark { 0.35 } else { 0.06 }))
                        .capture_key_down(cx.listener(Self::on_find_key))
                        .child(div().flex_1().child(field.clone())),
                )
                .children(label.map(|l| div().text_size(px(12.)).text_color(ink(0.5)).child(l)))
                .child(div().flex_1())
                .child(
                    div()
                        .id("web-find-done")
                        .px(px(8.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .text_size(px(12.))
                        .text_color(ink(0.8))
                        .hover(move |s| s.bg(ink(0.08)))
                        .child("Done")
                        .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
                )
                .into_any_element(),
        )
    }
}

impl Render for WebPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The page has the keyboard while the pane does.
        if self.focus.is_focused(window) {
            self.web.read(cx).focus_native();
        }
        self.editing = self.address.read(cx).focus_handle(cx).is_focused(window);
        let toolbar = self.render_toolbar(cx);
        let find = self.render_find(cx);
        div()
            .id("web-pane")
            .key_context("WebPane")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(self.style.background))
            .on_action(cx.listener(|this, _: &crate::actions::GoBack, _, cx| this.back(cx)))
            .on_action(cx.listener(|this, _: &crate::actions::GoForward, _, cx| this.forward(cx)))
            .on_action(cx.listener(|this, _: &crate::actions::ReloadPage, _, cx| this.reload(cx)))
            .on_action(
                cx.listener(|this, _: &crate::actions::FocusAddress, window, cx| {
                    this.focus_address(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::actions::FindOnPage, window, cx| {
                    this.open_find(window, cx)
                }),
            )
            .child(toolbar)
            .children(find)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
                    )
                    .child(self.web.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_addresses() {
        assert_eq!(
            url_for("example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            url_for("localhost:3000/x").as_deref(),
            Some("http://localhost:3000/x")
        );
        assert_eq!(
            url_for("127.0.0.1:8765").as_deref(),
            Some("http://127.0.0.1:8765")
        );
        assert_eq!(url_for("https://a.b/c").as_deref(), Some("https://a.b/c"));
        assert_eq!(
            url_for("rust gpui").as_deref(),
            Some("https://duckduckgo.com/?q=rust+gpui")
        );
        assert_eq!(
            url_for("/tmp/a b.html").as_deref(),
            Some("file:///tmp/a%20b.html")
        );
        assert_eq!(url_for("  "), None);
    }
}
