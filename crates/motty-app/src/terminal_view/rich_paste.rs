//! Rich paste in the Composer: ⌘V turns styled text (HTML, or RTF through
//! AppKit) into Markdown and a picture into an attachment chip. Files and
//! plain text, and everything on ⇧⌘V, are the text field's to paste.

use std::path::PathBuf;

use gpui::{Context, Window};
use htmd::options::{BulletListMarker, HrStyle, Options};

use super::TerminalView;
use super::composer::Chip;
use crate::actions::Paste;
use crate::pasteboard::{self, Rich};

impl TerminalView {
    /// ⌘V in the Composer, ahead of its text field.
    pub(super) fn composer_paste(
        &mut self,
        _: &Paste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rich) = pasteboard::read_rich() else {
            return;
        };
        match rich {
            Rich::Image(png) => match save_png(&png) {
                Ok(path) => {
                    let chip = Chip::image(&path, self.composer_images() + 1);
                    self.attach(chip, window, cx);
                }
                Err(e) => {
                    tracing::warn!("couldn't save the pasted image: {e}");
                    return;
                }
            },
            Rich::Html(html) => {
                // Otherwise the plain text is the better copy.
                let Some((markdown, editor)) = html_to_markdown(&html)
                    .filter(|md| !md.is_empty())
                    .zip(self.composer.as_ref().map(|c| c.editor.clone()))
                else {
                    return;
                };
                editor.update(cx, |e, cx| e.insert_text(&markdown, cx));
            }
        }
        cx.stop_propagation();
    }
}

/// Styled text as Markdown. A page's head, scripts and styles are left
/// out; lists use `-`, as people type them. `None` when the markup is only
/// `div`s and `span`s, as code copied from an editor (VS Code) is: Markdown
/// has nothing to add and would lose its indentation.
fn html_to_markdown(html: &str) -> Option<String> {
    if !has_structure(html) {
        return None;
    }
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec!["head", "script", "style", "title", "meta"])
        .options(Options {
            bullet_list_marker: BulletListMarker::Dash,
            ul_bullet_spacing: 1,
            ol_number_spacing: 1,
            hr_style: HrStyle::Dashes,
            ..Options::default()
        })
        .build();
    match converter.convert(html) {
        Ok(markdown) => Some(markdown.trim().to_owned()),
        Err(e) => {
            tracing::warn!("couldn't convert the pasted HTML: {e}");
            None
        }
    }
}

/// Whether `html` has an element Markdown can say something about.
fn has_structure(html: &str) -> bool {
    const LAYOUT: [&str; 10] = [
        "html", "head", "body", "meta", "title", "style", "div", "span", "br", "font",
    ];
    html.split('<').skip(1).any(|tag| {
        let name = tag
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        !name.is_empty() && !LAYOUT.contains(&name.as_str())
    })
}

/// A pasted picture, kept in Motty's cache folder under a name from its
/// content, so pasting it again reuses the file.
fn save_png(png: &[u8]) -> std::io::Result<PathBuf> {
    let dir = dirs::cache_dir()
        .ok_or_else(|| std::io::Error::other("no cache folder"))?
        .join("motty/pasted");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.png", &crate::recipes::hash(png)[..16]));
    if !path.exists() {
        std::fs::write(&path, png)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_a_browser_copy() {
        // What Safari and Chrome put on the clipboard.
        let html = "<meta charset='utf-8'><h2 style=\"color: red\">Setup</h2>\
            <p>Run <code>cargo test</code> and <b>read</b> the \
            <a href=\"https://otty.sh/docs\">docs</a>.</p>\
            <ul><li>one</li><li>two</li></ul>\
            <pre><code>let x = 1;\nlet y = 2;</code></pre>";
        assert_eq!(
            html_to_markdown(html).as_deref(),
            Some(
                "## Setup\n\n\
                 Run `cargo test` and **read** the [docs](https://otty.sh/docs).\n\n\
                 - one\n- two\n\n\
                 ```\nlet x = 1;\nlet y = 2;\n```"
            )
        );
    }

    #[test]
    fn leaves_out_the_head() {
        let html = "<html><head><title>Page</title><style>p { color: red }</style>\
            <script>alert(1)</script></head><body><ol><li>first</li><li>second</li></ol>\
            <hr><p><i>end</i></p></body></html>";
        assert_eq!(
            html_to_markdown(html).as_deref(),
            Some("1. first\n2. second\n\n- - -\n\n*end*")
        );
    }

    #[test]
    fn leaves_editor_code_to_the_plain_text() {
        // VS Code's copy: lines as divs, indentation kept by CSS.
        let html = "<meta charset='utf-8'><div style=\"font-family: Menlo; \
            white-space: pre;\"><div><span style=\"color: #569cd6;\">fn</span>\
            <span> main() {</span></div><div><span>    run();</span></div>\
            <div><span>}</span></div></div>";
        assert_eq!(html_to_markdown(html), None);
        assert_eq!(html_to_markdown("<span>just text</span>"), None);
        assert!(has_structure("<DIV><B>bold</B></DIV>"));
        assert!(!has_structure("a < b <br/> </div>"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn converts_rtf_through_appkit() {
        let rtf = br"{\rtf1\ansi\deff0{\fonttbl{\f0 Helvetica;}}
\f0\fs24 Hello {\b bold} and {\i italic}.\par
Second line\par
}";
        let html = pasteboard::rtf_to_html(rtf).expect("AppKit converts RTF");
        // AppKit writes a whole page, its fonts in a style sheet.
        assert_eq!(
            html_to_markdown(&html).as_deref(),
            Some("Hello **bold** and *italic*.\n\nSecond line")
        );
    }
}
