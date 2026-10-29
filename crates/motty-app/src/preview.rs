//! Previews of files as HTML pages for a WebView, as Otty shows them:
//! GitHub-flavored Markdown with code colored from the terminal palette and
//! Mermaid diagrams drawn by a bundled mermaid.js, and SVG with pinch zoom.
//!
//! Pages carry a Content-Security-Policy that only runs Motty's own scripts,
//! and raw HTML in Markdown is reduced to harmless tags and attributes.

#![allow(dead_code)]

use std::fmt::Write;
use std::hash::{BuildHasher, RandomState};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use motty_render::TerminalStyle;
use motty_term::alacritty_terminal::vte::ansi::NamedColor;
use motty_theme::Rgba;
use motty_theme::theme::mix;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::highlight::{Highlighter, palette_color, token_bold};

/// Mermaid 11.17.2 (MIT, see `assets/preview/LICENSE.mermaid`).
pub const MERMAID_JS: &str = include_str!("../assets/preview/mermaid.min.js");

/// A `file://` URL of mermaid.js in the cache folder, written once per run
/// when missing or changed, for pages loaded with a file base URL.
pub fn mermaid_js_url() -> Option<&'static str> {
    static URL: OnceLock<Option<String>> = OnceLock::new();
    URL.get_or_init(|| {
        let path = dirs::cache_dir()?.join("motty/preview/mermaid.min.js");
        if std::fs::read(&path).ok().as_deref() != Some(MERMAID_JS.as_bytes())
            && let Err(e) = write_atomically(&path, MERMAID_JS.as_bytes())
        {
            tracing::warn!("preview: can't write {}: {e}", path.display());
            return None;
        }
        Some(file_url(&path))
    })
    .as_deref()
}

fn write_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp: PathBuf = dir.join(format!(".mermaid-{}.tmp", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

/// A `file://` URL for an absolute path.
pub fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for &b in path.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            url.push(b as char);
        } else {
            let _ = write!(url, "%{b:02X}");
        }
    }
    url
}

/// The theme's colors, as a preview page uses them.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewColors {
    pub background: Rgba,
    pub foreground: Rgba,
    pub muted: Rgba,
    /// Links.
    pub accent: Rgba,
    pub code_background: Rgba,
    pub border: Rgba,
    pub palette: [Rgba; 16],
    pub dark: bool,
}

impl PreviewColors {
    pub fn from_style(style: &TerminalStyle) -> Self {
        let background = style.background.over(style.panel_background);
        let foreground = style.palette[NamedColor::Foreground as usize];
        let mut palette = [Rgba::TRANSPARENT; 16];
        palette.copy_from_slice(&style.palette[..16]);
        Self {
            background,
            foreground,
            muted: mix(foreground, background, 0.4),
            accent: palette[4],
            code_background: mix(background, foreground, if style.dark { 0.07 } else { 0.05 }),
            border: mix(background, foreground, 0.18),
            palette,
            dark: style.dark,
        }
    }
}

fn hex(c: Rgba) -> String {
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    escape_into(&mut out, s);
    out
}

fn escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

fn nonce() -> String {
    let state = RandomState::new();
    format!(
        "{:016x}{:016x}",
        state.hash_one(std::process::id()),
        state.hash_one(std::time::SystemTime::now())
    )
}

/// The page around `body`: the theme's colors, the shared styles, and a
/// policy that only runs scripts carrying `nonce`.
fn page(colors: &PreviewColors, nonce: &str, css: &str, body: &str) -> String {
    let c = colors;
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; \
img-src * data: blob: file:; media-src * file:; font-src data: file:; \
style-src 'unsafe-inline'; script-src 'nonce-{nonce}'\">\
<style>:root{{color-scheme:{scheme};--bg:{bg};--fg:{fg};--muted:{muted};--accent:{accent};\
--code-bg:{code};--border:{border};--yellow:{yellow};--green:{green};--red:{red};--magenta:{magenta}}}\
{BASE_CSS}{css}</style></head>\n<body>{body}</body></html>\n",
        scheme = if c.dark { "dark" } else { "light" },
        bg = hex(c.background),
        fg = hex(c.foreground),
        muted = hex(c.muted),
        accent = hex(c.accent),
        code = hex(c.code_background),
        border = hex(c.border),
        yellow = hex(c.palette[3]),
        green = hex(c.palette[2]),
        red = hex(c.palette[1]),
        magenta = hex(c.palette[5]),
    )
}

const BASE_CSS: &str = "\
html,body{margin:0;background:var(--bg);color:var(--fg)}\
body{font:16px/1.6 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;\
-webkit-font-smoothing:antialiased;-webkit-text-size-adjust:100%}";

const MARKDOWN_CSS: &str = "\
main{box-sizing:border-box;max-width:1000px;margin:0 auto;padding:56px 40px 96px;word-wrap:break-word}\
main>:first-child{margin-top:0}\
p,ul,ol,dl,table,pre,blockquote,details,.footnote-definition{margin:0 0 16px}\
h1,h2,h3,h4,h5,h6{margin:28px 0 16px;font-weight:700;line-height:1.25}\
h1{font-size:28px;padding-bottom:.3em;border-bottom:1px solid var(--border)}\
h2{font-size:22px;padding-bottom:.3em;border-bottom:1px solid var(--border)}\
h3{font-size:18px}h4{font-size:16px}h5{font-size:14px}h6{font-size:13.5px;color:var(--muted)}\
a{color:var(--accent);text-decoration:underline;text-underline-offset:2px}\
strong{font-weight:700}del{color:var(--muted)}\
hr{height:1px;margin:24px 0;border:0;background:var(--border)}\
img{max-width:100%;box-sizing:content-box}\
ul,ol{padding-left:2em}li+li{margin-top:4px}li>p{margin:8px 0}\
li:has(>input[type=checkbox]){list-style:none}\
li>input[type=checkbox]{margin:0 .4em 0 -1.4em;vertical-align:middle}\
code,kbd,pre,samp{font-family:ui-monospace,SFMono-Regular,'SF Mono',Menlo,monospace}\
code{font-size:85%;padding:.2em .4em;border-radius:6px;background:var(--code-bg)}\
pre{font-size:13.5px;line-height:1.5;padding:16px;border-radius:8px;background:var(--code-bg);\
overflow:auto;tab-size:4}\
pre code{font-size:100%;padding:0;border-radius:0;background:none;white-space:pre}\
kbd{font-size:12px;padding:2px 5px;border:1px solid var(--border);border-bottom-width:2px;\
border-radius:5px;background:var(--code-bg)}\
blockquote{margin-left:0;padding:0 1em;color:var(--muted);border-left:3px solid var(--muted)}\
blockquote[class^=markdown-alert]{color:var(--fg);border-left-color:var(--accent)}\
.markdown-alert-tip{border-left-color:var(--green)!important}\
.markdown-alert-important{border-left-color:var(--magenta)!important}\
.markdown-alert-warning{border-left-color:var(--yellow)!important}\
.markdown-alert-caution{border-left-color:var(--red)!important}\
table{display:block;width:max-content;max-width:100%;overflow:auto;border-collapse:collapse;border-spacing:0}\
th,td{padding:6px 13px;border:1px solid var(--border)}th{font-weight:600}\
tr:nth-child(2n) td{background:var(--code-bg)}\
.footnote-definition{font-size:14px;color:var(--muted)}\
.footnote-definition p{display:inline}.footnote-definition-label{margin-right:.4em}\
sup.footnote-reference{font-size:75%}\
pre.mermaid{font-size:13.5px}\
pre.mermaid[data-processed]{background:none;padding:0;text-align:center;overflow:visible}\
pre.mermaid svg{max-width:100%;height:auto}";

/// A Markdown file as a complete HTML page. Mermaid blocks become diagrams
/// when `mermaid_js_url` is given, and stay as their source otherwise.
pub fn markdown_html(source: &str, colors: &PreviewColors, mermaid_js_url: Option<&str>) -> String {
    let nonce = nonce();
    let lines = LineIndex::new(source);
    let mut slugs = Slugs::default();
    let mut has_mermaid = false;
    let mut events: Vec<Event> = Vec::new();
    let mut raw_html = String::new();
    let mut parser = Parser::new_ext(source, options()).into_offset_iter();

    while let Some((event, range)) = parser.next() {
        if let Event::Html(html) = &event {
            raw_html.push_str(html);
            continue;
        }
        if !raw_html.is_empty() {
            events.push(Event::Html(sanitize_html(&raw_html).into()));
            raw_html.clear();
        }
        match event {
            Event::InlineHtml(html) => events.push(Event::InlineHtml(sanitize_html(&html).into())),
            Event::Start(Tag::Heading { level, id, .. }) => {
                let mut inner = Vec::new();
                let mut text = String::new();
                for (event, _) in parser.by_ref() {
                    match &event {
                        Event::End(TagEnd::Heading(_)) => break,
                        Event::Text(t) | Event::Code(t) => text.push_str(t),
                        _ => {}
                    }
                    inner.push(safe_event(event));
                }
                let slug = slugs.next(id.as_deref().unwrap_or(&text));
                let mut html = String::new();
                pulldown_cmark::html::push_html(&mut html, inner.into_iter());
                let n = level as u8;
                events.push(Event::Html(
                    format!(
                        "<h{n} id=\"{}\" data-line=\"{}\">{html}</h{n}>\n",
                        escape(&slug),
                        lines.line(range.start)
                    )
                    .into(),
                ));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let mut code = String::new();
                for (event, _) in parser.by_ref() {
                    match event {
                        Event::End(TagEnd::CodeBlock) => break,
                        Event::Text(t) => code.push_str(&t),
                        _ => {}
                    }
                }
                let lang = match &kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next(),
                    CodeBlockKind::Indented => None,
                };
                let html = if lang.is_some_and(|l| l.eq_ignore_ascii_case("mermaid")) {
                    has_mermaid = true;
                    format!("<pre class=\"mermaid\">{}</pre>\n", escape(code.trim_end()))
                } else {
                    code_block(&code, lang, colors)
                };
                events.push(Event::Html(html.into()));
            }
            event => events.push(safe_event(event)),
        }
    }
    if !raw_html.is_empty() {
        events.push(Event::Html(sanitize_html(&raw_html).into()));
    }

    let mut body = String::from("<main class=\"markdown\">\n");
    pulldown_cmark::html::push_html(&mut body, events.into_iter());
    body.push_str("</main>\n");
    if let (true, Some(url)) = (has_mermaid, mermaid_js_url) {
        body.push_str(&mermaid_scripts(url, colors, &nonce));
    }
    page(colors, &nonce, MARKDOWN_CSS, &body)
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_GFM
}

/// Links and images keep only safe URLs.
fn safe_event(event: Event) -> Event {
    match event {
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: if safe_url(&dest_url, false) {
                dest_url
            } else {
                CowStr::from("#")
            },
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: if safe_url(&dest_url, true) {
                dest_url
            } else {
                CowStr::from("")
            },
            title,
            id,
        }),
        Event::InlineHtml(html) => Event::InlineHtml(sanitize_html(&html).into()),
        Event::Html(html) => Event::Html(sanitize_html(&html).into()),
        event => event,
    }
}

/// A fenced code block, colored like the editor.
fn code_block(code: &str, lang: Option<&str>, colors: &PreviewColors) -> String {
    let code = code.strip_suffix('\n').unwrap_or(code);
    let lines: Vec<&str> = code.split('\n').collect();
    let mut highlighter = Highlighter::new(lang);
    let spans = highlighter.highlight_lines(&lines, 0..lines.len());
    let mut html = String::from("<pre");
    if let Some(name) = highlighter.language() {
        let _ = write!(html, " data-lang=\"{}\"", escape(name));
    }
    html.push_str("><code>");
    for (i, (line, spans)) in lines.iter().zip(&spans).enumerate() {
        if i > 0 {
            html.push('\n');
        }
        for span in spans {
            let text = &line[span.range.clone()];
            let color = palette_color(
                span.token,
                &colors.palette,
                colors.foreground,
                colors.code_background,
            );
            let bold = token_bold(span.token);
            if color.is_none() && !bold {
                escape_into(&mut html, text);
                continue;
            }
            html.push_str("<span style=\"");
            if let Some((color, italic)) = color {
                let _ = write!(html, "color:{}", hex(color));
                if italic {
                    html.push_str(";font-style:italic");
                }
            }
            if bold {
                html.push_str(";font-weight:700");
            }
            html.push_str("\">");
            escape_into(&mut html, text);
            html.push_str("</span>");
        }
    }
    html.push_str("</code></pre>\n");
    html
}

fn mermaid_scripts(url: &str, colors: &PreviewColors, nonce: &str) -> String {
    let c = colors;
    let accent = c.palette[4];
    let variables = serde_json::json!({
        "darkMode": c.dark,
        "background": hex(c.background),
        "fontFamily": "-apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif",
        "fontSize": "15px",
        "textColor": hex(c.foreground),
        "primaryColor": hex(mix(c.code_background, accent, 0.12)),
        "primaryTextColor": hex(c.foreground),
        "primaryBorderColor": hex(accent),
        "secondaryColor": hex(mix(c.code_background, c.palette[6], 0.12)),
        "secondaryTextColor": hex(c.foreground),
        "secondaryBorderColor": hex(c.palette[6]),
        "tertiaryColor": hex(c.code_background),
        "tertiaryTextColor": hex(c.foreground),
        "tertiaryBorderColor": hex(c.border),
        "mainBkg": hex(mix(c.code_background, accent, 0.12)),
        "nodeBorder": hex(accent),
        "lineColor": hex(c.muted),
        "clusterBkg": hex(c.code_background),
        "clusterBorder": hex(c.border),
        "titleColor": hex(c.foreground),
        "edgeLabelBackground": hex(c.background),
        "noteBkgColor": hex(mix(c.background, c.palette[3], 0.18)),
        "noteTextColor": hex(c.foreground),
        "noteBorderColor": hex(c.palette[3]),
        "actorBkg": hex(mix(c.code_background, accent, 0.12)),
        "actorBorder": hex(accent),
        "actorTextColor": hex(c.foreground),
        "signalColor": hex(c.foreground),
        "signalTextColor": hex(c.foreground),
    });
    format!(
        "<script nonce=\"{nonce}\" src=\"{url}\"></script>\n\
<script nonce=\"{nonce}\">\n\
(async () => {{\n\
  if (!window.mermaid) return;\n\
  mermaid.initialize({{startOnLoad: false, theme: 'base', securityLevel: 'strict',\n\
    suppressErrorRendering: true, themeVariables: {variables}}});\n\
  for (const block of document.querySelectorAll('pre.mermaid')) {{\n\
    const source = block.textContent;\n\
    try {{\n\
      await mermaid.run({{nodes: [block]}});\n\
    }} catch (e) {{\n\
      block.removeAttribute('data-processed');\n\
      block.textContent = source;\n\
      block.title = String(e && e.message || e);\n\
    }}\n\
  }}\n\
}})();\n\
</script>\n",
        url = escape(url),
    )
}

/// An SVG file as a page that centers it and zooms with a pinch or
/// ⌘/Ctrl + scroll; a double click resets the zoom.
pub fn svg_html(svg_path_url: &str, colors: &PreviewColors) -> String {
    let nonce = nonce();
    let body = format!(
        "<div id=\"stage\"><img id=\"svg\" src=\"{}\" alt=\"\"></div>\n\
<script nonce=\"{nonce}\">\n{SVG_JS}</script>\n",
        escape(svg_path_url)
    );
    page(colors, &nonce, SVG_CSS, &body)
}

const SVG_CSS: &str = "\
html,body{height:100%;overflow:auto}\
#stage{box-sizing:border-box;min-width:100%;min-height:100%;display:flex;\
align-items:center;justify-content:center;padding:24px;width:max-content}\
#svg{display:block;max-width:none;user-select:none;-webkit-user-drag:none}";

const SVG_JS: &str = r#"(() => {
  const img = document.getElementById('svg');
  let base = 1, zoom = 1, w = 0, h = 0;
  const apply = () => {
    img.style.width = w * base * zoom + 'px';
    img.style.height = h * base * zoom + 'px';
  };
  const fit = () => {
    w = img.naturalWidth || innerWidth * 0.8;
    h = img.naturalHeight || innerHeight * 0.8;
    base = Math.min(1, (innerWidth - 48) / w, (innerHeight - 48) / h);
    if (!(base > 0)) base = 1;
    apply();
  };
  const zoomAt = (factor, x, y) => {
    const next = Math.min(32, Math.max(0.05, zoom * factor));
    const ratio = next / zoom;
    const sx = scrollX + x, sy = scrollY + y;
    zoom = next;
    apply();
    scrollTo(sx * ratio - x, sy * ratio - y);
  };
  img.addEventListener('load', fit);
  if (img.complete) fit();
  addEventListener('resize', fit);
  addEventListener('wheel', (e) => {
    if (!e.ctrlKey && !e.metaKey) return;
    e.preventDefault();
    zoomAt(Math.exp(-e.deltaY * 0.01), e.clientX, e.clientY);
  }, {passive: false});
  let start = 1;
  addEventListener('gesturestart', (e) => { e.preventDefault(); start = zoom; });
  addEventListener('gesturechange', (e) => {
    e.preventDefault();
    zoomAt(start * e.scale / zoom, e.clientX, e.clientY);
  });
  addEventListener('dblclick', () => { zoom = 1; apply(); });
})();
"#;

/// The headings of a Markdown file for an outline: level, text, and the
/// 0-based line each starts on.
pub fn markdown_headings(source: &str) -> Vec<(u8, String, usize)> {
    let lines = LineIndex::new(source);
    let mut headings = Vec::new();
    let mut current: Option<(HeadingLevel, String, usize)> = None;
    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some((level, String::new(), lines.line(range.start)));
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, text, _)) = &mut current {
                    text.push_str(&t);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, text, line)) = current.take() {
                    headings.push((level as u8, text.trim().to_string(), line));
                }
            }
            _ => {}
        }
    }
    headings
}

/// The `<h1>`–`<h6>` headings of an HTML file, best effort.
pub fn html_headings(source: &str) -> Vec<(u8, String, usize)> {
    let lines = LineIndex::new(source);
    let lower = source.to_ascii_lowercase();
    let mut headings = Vec::new();
    let mut at = 0;
    while let Some(found) = lower[at..].find('<') {
        let start = at + found;
        let rest = &lower[start..];
        if let Some(skip) = ["<!--", "<script", "<style"]
            .iter()
            .zip(["-->", "</script", "</style"])
            .find(|(open, _)| rest.starts_with(**open))
            .map(|(_, close)| close)
        {
            at = lower[start..]
                .find(skip)
                .map_or(lower.len(), |n| start + n + skip.len());
            continue;
        }
        let bytes = rest.as_bytes();
        let level = match bytes {
            [b'<', b'h', d @ b'1'..=b'6', next, ..]
                if next.is_ascii_whitespace() || *next == b'>' || *next == b'/' =>
            {
                d - b'0'
            }
            _ => {
                at = start + 1;
                continue;
            }
        };
        let Some(open_end) = rest.find('>').map(|n| start + n + 1) else {
            break;
        };
        let close = format!("</h{level}");
        let Some(close_at) = lower[open_end..].find(&close).map(|n| open_end + n) else {
            break;
        };
        let text = html_text(&source[open_end..close_at]);
        if !text.is_empty() {
            headings.push((level, text, lines.line(start)));
        }
        at = close_at + close.len();
    }
    headings
}

/// An HTML fragment's text: tags dropped, common entities decoded,
/// whitespace collapsed.
fn html_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    decode_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest[1..].find(';').filter(|&n| n <= 10).and_then(|n| {
            let name = &rest[1..1 + n];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, n + 2))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Tags raw HTML in Markdown may use; anything else shows as text.
const ALLOWED_TAGS: &[&str] = &[
    "a",
    "abbr",
    "b",
    "bdi",
    "bdo",
    "blockquote",
    "br",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "picture",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "small",
    "source",
    "span",
    "strike",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "tt",
    "u",
    "ul",
    "var",
    "wbr",
];

const ALLOWED_ATTRIBUTES: &[&str] = &[
    "align", "alt", "colspan", "datetime", "dir", "height", "href", "id", "lang", "media", "name",
    "open", "rowspan", "span", "src", "srcset", "start", "title", "type", "valign", "width",
];

/// Raw HTML with only allowed tags and attributes kept; other tags are
/// escaped, comments dropped.
fn sanitize_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |n| &comment[n + 3..]);
            continue;
        }
        match clean_tag(rest) {
            Some((len, Some(tag))) => {
                out.push_str(&tag);
                rest = &rest[len..];
            }
            Some((len, None)) => {
                escape_into(&mut out, &rest[..len]);
                rest = &rest[len..];
            }
            None => {
                out.push_str("&lt;");
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parses the tag `s` starts with: its length and, when allowed, the tag
/// rebuilt with only safe attributes. `None` when it isn't a tag.
fn clean_tag(s: &str) -> Option<(usize, Option<String>)> {
    let bytes = s.as_bytes();
    let mut i = 1;
    let closing = bytes.get(i) == Some(&b'/');
    if closing {
        i += 1;
    }
    let name_start = i;
    while bytes.get(i).is_some_and(|b| b.is_ascii_alphanumeric()) {
        i += 1;
    }
    if i == name_start || !bytes[name_start].is_ascii_alphabetic() {
        return None;
    }
    let name = s[name_start..i].to_ascii_lowercase();
    let mut attributes = Vec::new();
    let end = loop {
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        match bytes.get(i)? {
            b'>' => break i + 1,
            b'/' if bytes.get(i + 1) == Some(&b'>') => break i + 2,
            b'/' => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let attr_start = i;
        while bytes
            .get(i)
            .is_some_and(|b| !b.is_ascii_whitespace() && !b"\"'>/=".contains(b))
        {
            i += 1;
        }
        if i == attr_start {
            return None;
        }
        let attr = s[attr_start..i].to_ascii_lowercase();
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        let mut value = None;
        if bytes.get(i) == Some(&b'=') {
            i += 1;
            while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
                i += 1;
            }
            let (start, stop) = match bytes.get(i)? {
                q @ (b'"' | b'\'') => {
                    let close = s[i + 1..].find(*q as char)? + i + 1;
                    (i + 1, close)
                }
                _ => {
                    let mut j = i;
                    while bytes
                        .get(j)
                        .is_some_and(|b| !b.is_ascii_whitespace() && *b != b'>')
                    {
                        j += 1;
                    }
                    (i, j)
                }
            };
            value = Some(decode_entities(&s[start..stop]));
            i = if bytes.get(stop).is_some_and(|b| *b == b'"' || *b == b'\'') {
                stop + 1
            } else {
                stop
            };
        }
        attributes.push((attr, value));
    };
    if !ALLOWED_TAGS.contains(&name.as_str()) {
        return Some((end, None));
    }
    let mut tag = format!("<{}{name}", if closing { "/" } else { "" });
    if !closing {
        for (attr, value) in attributes {
            if !ALLOWED_ATTRIBUTES.contains(&attr.as_str()) {
                continue;
            }
            let value = match (attr.as_str(), value) {
                ("href", Some(v)) => safe_url(&v, false).then_some(v),
                ("src", Some(v)) => safe_url(&v, true).then_some(v),
                ("srcset", Some(v)) => v.split(',').all(|c| safe_url(c.trim(), true)).then_some(v),
                (_, v) => Some(v.unwrap_or_default()),
            };
            if let Some(value) = value {
                let _ = write!(tag, " {attr}=\"{}\"", escape(&value));
            }
        }
    }
    tag.push('>');
    Some((end, Some(tag)))
}

/// Whether `url` is relative or uses a scheme a preview may follow.
fn safe_url(url: &str, image: bool) -> bool {
    let squeezed: String = url
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .take(16)
        .collect::<String>()
        .to_ascii_lowercase();
    let scheme = squeezed
        .find(':')
        .filter(|&n| !squeezed[..n].contains(['/', '?', '#']))
        .map(|n| &squeezed[..n]);
    match scheme {
        None => true,
        Some("http" | "https" | "file") => true,
        Some("mailto") => !image,
        Some("data") => image && squeezed.starts_with("data:image/"),
        Some(_) => false,
    }
}

/// 0-based line numbers for byte offsets.
struct LineIndex(Vec<usize>);

impl LineIndex {
    fn new(source: &str) -> Self {
        Self(
            source
                .bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i)
                .collect(),
        )
    }

    fn line(&self, offset: usize) -> usize {
        self.0.partition_point(|&nl| nl < offset)
    }
}

/// GitHub-style heading anchors, numbered when repeated.
#[derive(Default)]
struct Slugs(std::collections::HashMap<String, usize>);

impl Slugs {
    fn next(&mut self, text: &str) -> String {
        let slug: String = text
            .trim()
            .to_lowercase()
            .chars()
            .filter_map(|c| match c {
                ' ' => Some('-'),
                c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
                _ => None,
            })
            .collect();
        let seen = self.0.entry(slug.clone()).or_insert(0);
        *seen += 1;
        match *seen {
            1 => slug,
            n => format!("{slug}-{}", n - 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors() -> PreviewColors {
        let mut palette = [Rgba::TRANSPARENT; 16];
        for (i, c) in palette.iter_mut().enumerate() {
            *c = Rgba::rgb(0x0f0f0f * (i as u32 + 1));
        }
        palette[5] = Rgba::rgb(0xbb9af7);
        PreviewColors {
            background: Rgba::rgb(0x1a1b26),
            foreground: Rgba::rgb(0xc0caf5),
            muted: Rgba::rgb(0x787c99),
            accent: Rgba::rgb(0x7aa2f7),
            code_background: Rgba::rgb(0x24283b),
            border: Rgba::rgb(0x3b4261),
            palette,
            dark: true,
        }
    }

    #[test]
    fn renders_tables_and_task_lists() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n\n~~old~~ text[^1]\n\n[^1]: note\n";
        let html = markdown_html(md, &colors(), None);
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<table>") && html.contains("<td>1</td>"));
        assert!(html.contains("type=\"checkbox\""));
        assert!(html.contains("checked"));
        assert!(html.contains("<del>old</del>"));
        assert!(html.contains("footnote-definition"));
        assert!(html.contains("--accent:#7aa2f7"));
    }

    #[test]
    fn colors_code_blocks() {
        let md = "```rust\nfn main() { let x = 1 < 2; }\n```\n";
        let html = markdown_html(md, &colors(), None);
        assert!(html.contains("data-lang=\"Rust\""));
        assert!(
            html.contains("<span style=\"color:#bb9af7\">fn</span>"),
            "{html}"
        );
        assert!(html.contains("&lt;"));
        let plain = markdown_html("```\n<b>&\n```\n", &colors(), None);
        assert!(plain.contains("<pre><code>&lt;b&gt;&amp;</code></pre>"));
    }

    #[test]
    fn mermaid_blocks() {
        let md = "```mermaid\ngraph TD; A-->B\n```\n";
        let html = markdown_html(md, &colors(), Some("file:///tmp/mermaid.min.js"));
        assert!(html.contains("<pre class=\"mermaid\">graph TD; A--&gt;B</pre>"));
        assert!(html.contains("src=\"file:///tmp/mermaid.min.js\""));
        assert!(html.contains("suppressErrorRendering: true"));
        let nonce = html
            .split("'nonce-")
            .nth(1)
            .unwrap()
            .split('\'')
            .next()
            .unwrap();
        assert!(html.contains(&format!("<script nonce=\"{nonce}\" src=")));

        let without = markdown_html(
            "# Title\n\n```rust\nfn x() {}\n```\n",
            &colors(),
            Some("m.js"),
        );
        assert!(!without.contains("<script"));
        let offline = markdown_html(md, &colors(), None);
        assert!(offline.contains("class=\"mermaid\"") && !offline.contains("<script"));
    }

    #[test]
    fn headings_have_ids_and_lines() {
        let md = "# Intro\n\ntext\n\n## Intro\n\n```sh\n# not a heading\n```\n\nSetup `x`\n-----\n### Custom {#here}\n";
        let html = markdown_html(md, &colors(), None);
        assert!(
            html.contains("<h1 id=\"intro\" data-line=\"0\">Intro</h1>"),
            "{html}"
        );
        assert!(html.contains("<h2 id=\"intro-1\" data-line=\"4\">"));
        assert!(html.contains("<h2 id=\"setup-x\" data-line=\"10\">Setup <code>x</code></h2>"));
        assert!(html.contains("<h3 id=\"here\""));
        assert_eq!(
            markdown_headings(md),
            vec![
                (1, "Intro".into(), 0),
                (2, "Intro".into(), 4),
                (2, "Setup x".into(), 10),
                (3, "Custom".into(), 12),
            ]
        );
    }

    #[test]
    fn escapes_unsafe_html() {
        let md = "Hi <script>alert(1)</script> there\n\n<div onclick=\"x()\" align=\"center\"><img src=\"javascript:alert(1)\" alt=\"a\"><img src=\"logo.png\" width=\"40\"></div>\n\n[link](javascript:alert(1)) and 1 < 2 & <b>bold</b>\n\n<style>body{}</style>\n";
        let html = markdown_html(md, &colors(), None);
        let body = html.split("<body>").nth(1).unwrap();
        assert!(!body.contains("<script"), "{body}");
        assert!(body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(body.contains("<div align=\"center\">"));
        assert!(!body.contains("onclick") && !body.contains("javascript:"));
        assert!(body.contains("<img src=\"logo.png\" width=\"40\">"));
        assert!(body.contains("<img alt=\"a\">"));
        assert!(body.contains("<a href=\"#\">link</a>"));
        assert!(body.contains("1 &lt; 2 &amp; <b>bold</b>"));
        assert!(!body.contains("<style"));
    }

    #[test]
    fn html_heading_outline() {
        let html = "<html>\n<!-- <h1>no</h1> -->\n<h1 class=\"t\">Hello <b>&amp; bye</b></h1>\n<script>'<h2>x</h2>'</script>\n<H2>\n  Two\n</H2>\n<h7>x</h7>";
        assert_eq!(
            html_headings(html),
            vec![(1, "Hello & bye".into(), 2), (2, "Two".into(), 4)]
        );
    }

    #[test]
    fn svg_page() {
        let html = svg_html("file:///a%20b/x.svg", &colors());
        assert!(html.contains("<img id=\"svg\" src=\"file:///a%20b/x.svg\""));
        assert!(html.contains("gesturechange"));
        assert_eq!(
            file_url(Path::new("/a b/ü.svg")),
            "file:///a%20b/%C3%BC.svg"
        );
    }

    #[test]
    fn colors_from_terminal_style() {
        let source = motty_theme::config::builtin_source("Tokyo Night").unwrap();
        let theme = motty_theme::theme::parse(source).unwrap().theme;
        let style = TerminalStyle::from_theme(&theme, None, None, false);
        let colors = PreviewColors::from_style(&style);
        assert!(colors.dark);
        assert_eq!(colors.background.a, 1.);
        assert_eq!(colors.accent, style.palette[4]);
    }

    #[test]
    fn bundled_mermaid() {
        assert!(MERMAID_JS.len() > 1_000_000);
        assert!(MERMAID_JS.contains("globalThis[\"mermaid\"]"));
    }
}
