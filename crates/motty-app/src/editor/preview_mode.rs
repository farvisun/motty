//! Markdown, SVG and HTML files show rendered while read-only (Otty's
//! Preview), in a web view over the text; editing shows their source. An
//! agent's session log shows as its History the same way, until View as ›
//! JSONL.

use std::path::{Path, PathBuf};

use gpui::{AppContext, ClipboardItem, Context, Entity, Pixels, Point, Subscription, Window, px};

use super::FileEditor;
use crate::agents::sessions;
use crate::history_page::{self, PageFont, PageMessage};
use crate::preview::{self, PreviewColors};
use crate::web_view::{WebEvent, WebView, WebViewOptions};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Markdown,
    Svg,
    Html,
    /// An agent session's History.
    Transcript,
}

impl Kind {
    pub fn of(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "md" | "markdown" | "mdown" | "mkd" | "mdx" => Self::Markdown,
            "svg" => Self::Svg,
            "html" | "htm" | "xhtml" => Self::Html,
            _ => return None,
        })
    }
}

/// The rendered page and what it last showed.
pub(super) struct Preview {
    pub view: Entity<WebView>,
    /// The text revision and theme last loaded.
    shown: Option<(u64, bool, [u8; 4])>,
    /// The web view has the keyboard (a History page scrolls by keys).
    keyboard: bool,
    _messages: Option<Subscription>,
}

impl FileEditor {
    /// Whether the pane shows the rendered file rather than its text.
    pub fn previewing(&self) -> bool {
        if self.transcript.is_some() {
            return !self.raw;
        }
        self.read_only && self.path.as_deref().and_then(Kind::of).is_some()
    }

    fn kind(&self) -> Option<Kind> {
        if self.transcript.is_some() {
            return Some(Kind::Transcript);
        }
        self.path.as_deref().and_then(Kind::of)
    }

    /// The page for the current text, made or refreshed as needed.
    pub(super) fn preview_view(&mut self, cx: &mut Context<Self>) -> Option<Entity<WebView>> {
        let path = self.path.clone()?;
        let kind = self.kind()?;
        if self.preview.is_none() {
            let options = WebViewOptions {
                javascript: true,
                non_persistent: true,
                mobile: false,
                // Only Motty's own History page talks back.
                messages: kind == Kind::Transcript,
            };
            let view = cx.new(|cx| WebView::new(options, cx));
            let messages = (kind == Kind::Transcript).then(|| {
                cx.subscribe(&view, |this, _, event: &WebEvent, cx| {
                    if let WebEvent::Message(message) = event {
                        this.on_page_message(message, cx);
                    }
                })
            });
            self.preview = Some(Preview {
                view,
                shown: None,
                keyboard: false,
                _messages: messages,
            });
        }
        let preview = self.preview.as_mut()?;
        let style = &self.style;
        let bg = style.background;
        let theme = (
            style.dark,
            [bg.r, bg.g, bg.b, bg.a].map(|c| (c * 255.) as u8),
        );
        let key = (self.revision, theme.0, theme.1);
        if preview.shown != Some(key) {
            // Same theme, newer text: a History page updates in place.
            let in_place = kind == Kind::Transcript
                && preview
                    .shown
                    .is_some_and(|(_, dark, bg)| (dark, bg) == theme);
            preview.shown = Some(key);
            let colors = PreviewColors::from_style(style);
            let view = preview.view.clone();
            let text = self.buffer.text();
            let font = page_font(style);
            let session = self.transcript.clone();
            view.update(cx, |web, cx| {
                if let Some(session) = session {
                    let messages = sessions::transcript(&session);
                    if in_place {
                        let main = history_page::main_html(&session, &messages, &colors);
                        web.evaluate_js(&history_page::update_script(&main));
                        return;
                    }
                    web.set_appearance(style.dark);
                    web.set_background(motty_render::hsla(bg).to_rgb(), cx);
                    let html = history_page::html(&session, &messages, &colors, &font);
                    let page = page_path(cx.entity_id().as_u64(), "html");
                    // It reads only its own folder: the page and the font.
                    if write(&page, &html) {
                        web.load_file(&page, page.parent());
                    }
                    return;
                }
                web.set_appearance(style.dark);
                web.set_background(motty_render::hsla(bg).to_rgb(), cx);
                load(web, kind, &path, &text, &colors, cx.entity_id().as_u64());
            });
        }
        Some(preview.view.clone())
    }

    /// What the History page reported.
    fn on_page_message(&mut self, message: &str, cx: &mut Context<Self>) {
        match PageMessage::parse(message) {
            Some(PageMessage::Selection(text)) => self.page_selection = text,
            Some(PageMessage::Menu { x, y }) => {
                self.page_menu = Some(gpui::point(px(x), px(y)));
                cx.notify();
            }
            Some(PageMessage::Open(url))
                if url.starts_with("https://") || url.starts_with("http://") =>
            {
                cx.open_url(&url);
            }
            Some(PageMessage::Open(_)) | None => {}
        }
    }

    /// The History's selection, else the whole transcript as text (what
    /// Send to Chat takes).
    pub(super) fn page_text(&self) -> Option<String> {
        if !self.page_selection.trim().is_empty() {
            return Some(self.page_selection.clone());
        }
        let session = self.transcript.as_ref()?;
        let messages = sessions::transcript(session);
        Some(sessions::render(session, &messages, 100))
    }

    /// A History pane's right-click menu at `at` (on the page while it
    /// shows, else in the window): Copy, Send to Chat, and View as.
    pub(super) fn show_page_menu(
        &mut self,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::native_menu::{Entry, Item};
        let Some(session) = &self.transcript else {
            return;
        };
        let previewing = self.previewing();
        let at = match (&self.preview, previewing) {
            (Some(preview), true) => preview.view.read(cx).to_window(at),
            _ => at,
        };
        let selection = if previewing {
            Some(self.page_selection.clone()).filter(|s| !s.is_empty())
        } else {
            self.selected_text()
        };
        let history = format!("{} History", crate::agents::label_for(session.agent));
        let entries: Vec<Entry> = vec![
            Item::new("Copy").enabled(selection.is_some()).into(),
            Item::new("Send to Chat").into(),
            Entry::Separator,
            Entry::Submenu {
                label: "View as".into(),
                entries: vec![
                    Item::new(history).checked(!self.raw).into(),
                    Item::new("JSONL (Syntax Highlight)")
                        .checked(self.raw)
                        .into(),
                ],
            },
        ];
        let editor = cx.entity().downgrade();
        crate::native_menu::popup(
            entries,
            at,
            None,
            window,
            cx,
            move |ix, window, cx| match ix {
                0 => {
                    if let Some(text) = selection {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                }
                1 => window.dispatch_action(Box::new(crate::actions::SendToChat), cx),
                2 | 3 => {
                    let _ = editor.update(cx, |this, cx| this.set_raw(ix == 3, cx));
                }
                _ => {}
            },
        );
    }

    /// View as › JSONL, or back to the History.
    fn set_raw(&mut self, raw: bool, cx: &mut Context<Self>) {
        self.raw = raw;
        if let Some(preview) = &mut self.preview {
            preview.keyboard = false;
        }
        cx.notify();
    }

    /// While a History page shows in the focused pane, its web view takes
    /// the keyboard, so Page Up/Down, Home/End, the arrows and Space scroll
    /// it as in any document window.
    pub(super) fn sync_page_focus(&mut self, focused: bool, cx: &mut Context<Self>) {
        let showing = self.transcript.is_some() && self.previewing();
        let Some(preview) = &mut self.preview else {
            return;
        };
        let want = focused && showing;
        if want != preview.keyboard {
            preview.keyboard = want;
            let web = preview.view.read(cx);
            if want {
                web.focus_native();
            } else {
                web.blur_native();
            }
        }
    }
}

/// The terminal's font for a page; the bundled one is copied beside the
/// page so WebKit can load it.
fn page_font(style: &motty_render::TerminalStyle) -> PageFont {
    let family = style.font.family.to_string();
    let mut faces = String::new();
    if family.eq_ignore_ascii_case(crate::fonts::BUNDLED_FAMILY) {
        let dir = page_path(0, "html").with_file_name("fonts");
        for (name, bytes, weight, italic) in crate::fonts::BUNDLED_FILES {
            let file = dir.join(name);
            if (file.exists() || write_bytes(&file, bytes)) && dir.is_dir() {
                faces.push_str(&format!(
                    "@font-face{{font-family:'{family}';src:url('{}');font-weight:{weight};\
font-style:{}}}",
                    preview::file_url(&file),
                    if italic { "italic" } else { "normal" }
                ));
            }
        }
    }
    PageFont {
        family,
        size: f32::from(style.font_size),
        faces,
    }
}

fn write_bytes(path: &Path, bytes: &[u8]) -> bool {
    path.parent()
        .is_some_and(|d| std::fs::create_dir_all(d).is_ok())
        && std::fs::write(path, bytes).is_ok()
}

/// Loads `text` (the file's current, maybe unsaved, contents) rendered.
fn load(web: &WebView, kind: Kind, path: &Path, text: &str, colors: &PreviewColors, id: u64) {
    let dir = path.parent().unwrap_or(Path::new("/"));
    let html = match kind {
        Kind::Markdown => preview::markdown_html(text, colors, preview::mermaid_js_url()),
        Kind::Svg => preview::svg_html(&preview::file_url(path), colors),
        Kind::Html => {
            // Its own page, scripts and all, read from where it lives.
            let page = page_path(id, "html");
            if write(&page, &with_base(text, dir)) {
                web.load_file(&page, Some(&common_root(&page, dir)));
            }
            return;
        }
        Kind::Transcript => return,
    };
    // A page in the cache folder, so it can read mermaid.js beside it and
    // the file's own images through its <base>.
    let page = page_path(id, "html");
    if write(&page, &with_base(&html, dir)) {
        web.load_file(&page, Some(&common_root(&page, dir)));
    }
}

/// Relative links and images resolve against the file's folder.
fn with_base(html: &str, dir: &Path) -> String {
    let base = format!("<base href=\"{}/\">", preview::file_url(dir));
    match html.find("<head>") {
        Some(at) => {
            let at = at + "<head>".len();
            format!("{}{base}{}", &html[..at], &html[at..])
        }
        None => format!("{base}{html}"),
    }
}

fn page_path(id: u64, ext: &str) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "motty/preview/page-{}-{id}.{ext}",
            std::process::id()
        ))
}

fn write(path: &Path, text: &str) -> bool {
    let ok = path
        .parent()
        .is_some_and(|d| std::fs::create_dir_all(d).is_ok())
        && std::fs::write(path, text).is_ok();
    if !ok {
        tracing::warn!("preview: can't write {}", path.display());
    }
    ok
}

/// The deepest folder holding both `a` and `b`: what the page may read.
fn common_root(a: &Path, b: &Path) -> PathBuf {
    a.ancestors()
        .find(|p| b.starts_with(p))
        .map_or_else(|| PathBuf::from("/"), Path::to_path_buf)
}

impl Drop for Preview {
    fn drop(&mut self) {
        // The page file goes with the pane.
        let _ = std::fs::remove_file(page_path(self.view.entity_id().as_u64(), "html"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_extension() {
        assert_eq!(Kind::of(Path::new("a/README.MD")), Some(Kind::Markdown));
        assert_eq!(Kind::of(Path::new("logo.svg")), Some(Kind::Svg));
        assert_eq!(Kind::of(Path::new("x.htm")), Some(Kind::Html));
        assert_eq!(Kind::of(Path::new("main.rs")), None);
    }

    #[test]
    fn base_goes_first_in_head() {
        let html = with_base(
            "<html><head><title>x</title></head></html>",
            Path::new("/a b"),
        );
        assert!(html.starts_with("<html><head><base href=\"file:///a%20b/\"><title>"));
    }

    #[test]
    fn common_root_of_two_paths() {
        assert_eq!(
            common_root(
                Path::new("/Users/me/Library/Caches/p.html"),
                Path::new("/Users/me/src")
            ),
            PathBuf::from("/Users/me")
        );
        assert_eq!(
            common_root(Path::new("/tmp/x"), Path::new("/Users")),
            PathBuf::from("/")
        );
    }
}
