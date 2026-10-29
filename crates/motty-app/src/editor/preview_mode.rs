//! Markdown, SVG and HTML files show rendered while read-only (Otty's
//! Preview), in a web view over the text; editing shows their source.

use std::path::{Path, PathBuf};

use gpui::{AppContext, Context, Entity};

use super::FileEditor;
use crate::preview::{self, PreviewColors};
use crate::web_view::{WebView, WebViewOptions};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Markdown,
    Svg,
    Html,
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
}

impl FileEditor {
    /// Whether the pane shows the rendered file rather than its text.
    pub fn previewing(&self) -> bool {
        self.read_only && self.name.is_none() && self.path.as_deref().and_then(Kind::of).is_some()
    }

    /// The page for the current text, made or refreshed as needed.
    pub(super) fn preview_view(&mut self, cx: &mut Context<Self>) -> Option<Entity<WebView>> {
        let path = self.path.clone()?;
        let kind = Kind::of(&path)?;
        let preview = self.preview.get_or_insert_with(|| {
            let options = WebViewOptions {
                javascript: true,
                non_persistent: true,
                mobile: false,
            };
            Preview {
                view: cx.new(|cx| WebView::new(options, cx)),
                shown: None,
            }
        });
        let style = &self.style;
        let bg = style.background;
        let key = (
            self.revision,
            style.dark,
            [bg.r, bg.g, bg.b, bg.a].map(|c| (c * 255.) as u8),
        );
        if preview.shown != Some(key) {
            preview.shown = Some(key);
            let colors = PreviewColors::from_style(style);
            let view = preview.view.clone();
            let text = self.buffer.text();
            view.update(cx, |web, cx| {
                web.set_appearance(style.dark);
                web.set_background(motty_render::hsla(bg).to_rgb(), cx);
                load(web, kind, &path, &text, &colors, cx.entity_id().as_u64());
            });
        }
        Some(preview.view.clone())
    }
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
