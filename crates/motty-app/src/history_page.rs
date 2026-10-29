//! The History view's page: an agent session's transcript as Otty shows it.
//! A header with the session's title, file and folder, then one section per
//! turn: your prompt on a band, the agent's work folded under a one-line
//! summary (`› Claude · Edit, Read · 52 chars`), and its answer. Text is in
//! the terminal's font, with Markdown kept light: list markers stay, inline
//! code is colored rather than boxed.
//!
//! The page talks back through `window.webkit.messageHandlers.motty` (see
//! [`crate::web_view::WebViewOptions::messages`]): right-clicks, the
//! selection and link clicks arrive as [`PageMessage`]s.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::Path;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};

use crate::agents::sessions::{Message, Role, Session};
use crate::preview::{self, PreviewColors, escape, escape_into};

/// Lines of a tool's output shown when its turn is unfolded.
const OUTPUT_LINES: usize = 12;

/// The terminal font, as the page uses it.
pub struct PageFont {
    pub family: String,
    pub size: f32,
    /// `@font-face` rules for a font the system doesn't have installed
    /// (Motty's bundled one), pointing at copies on disk.
    pub faces: String,
}

/// One prompt and what the agent did about it.
#[derive(Debug, PartialEq)]
pub struct Turn<'a> {
    /// `None` for what an agent logged before the first prompt.
    pub prompt: Option<&'a Message>,
    /// Thinking, tool calls and their output, and any text before the
    /// answer, in order.
    pub work: Vec<&'a Message>,
    /// The last thing it said.
    pub answer: Option<&'a Message>,
}

/// `messages` in turns: each starts at a prompt.
pub fn turns(messages: &[Message]) -> Vec<Turn<'_>> {
    let mut turns: Vec<Turn> = Vec::new();
    for m in messages {
        if m.role == Role::User || turns.is_empty() {
            turns.push(Turn {
                prompt: (m.role == Role::User).then_some(m),
                work: Vec::new(),
                answer: None,
            });
            if m.role == Role::User {
                continue;
            }
        }
        let turn = turns.last_mut().expect("a turn was pushed");
        turn.work.push(m);
    }
    for turn in &mut turns {
        if let Some(ix) = turn.work.iter().rposition(|m| m.role == Role::Assistant) {
            turn.answer = Some(turn.work.remove(ix));
        }
    }
    turns
}

/// The folded work's one-line summary: the agent, the tools it used (by
/// name, sorted) and how much it wrote along the way.
pub fn summary(agent: &str, work: &[&Message]) -> Option<String> {
    if work.is_empty() {
        return None;
    }
    let tools: BTreeSet<&str> = work
        .iter()
        .filter(|m| m.role == Role::Tool)
        .filter_map(|m| m.text.strip_prefix("⏺ "))
        .map(|t| t.split("  ").next().unwrap_or(t).trim())
        .collect();
    let chars: usize = work
        .iter()
        .filter(|m| matches!(m.role, Role::Assistant | Role::Reasoning))
        .map(|m| m.text.chars().count())
        .sum();
    let mut parts = vec![short_name(agent)];
    if !tools.is_empty() {
        parts.push(tools.into_iter().collect::<Vec<_>>().join(", "));
    }
    if chars > 0 {
        parts.push(format!("{chars} chars"));
    }
    Some(parts.join(" · "))
}

/// `Claude` for Claude Code, as the summary line names the agent.
fn short_name(agent: &str) -> String {
    let label = crate::agents::label_for(agent);
    ["Code", "CLI"]
        .iter()
        .find_map(|suffix| label.strip_suffix(suffix).map(str::trim_end))
        .unwrap_or(&label)
        .to_owned()
}

/// The page's `<main>`: everything that changes as the session grows.
pub fn main_html(session: &Session, messages: &[Message], colors: &PreviewColors) -> String {
    let mut out = String::from("<main>\n<header>");
    let file = session
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let _ = write!(
        out,
        "<h1>{}</h1><div class=\"meta\">{}</div>",
        escape(&session.title),
        escape(&file)
    );
    if let Some(cwd) = &session.cwd {
        let _ = write!(
            out,
            "<div class=\"meta\">{}</div>",
            escape(&cwd.display().to_string())
        );
    }
    out.push_str("</header>\n");
    for (n, turn) in turns(messages).iter().enumerate() {
        let _ = write!(out, "<section class=\"turn\" id=\"t{n}\">");
        if let Some(prompt) = turn.prompt {
            out.push_str("<div class=\"who\"><span class=\"dot\"></span>You");
            if let Some(time) = prompt.time.as_deref().and_then(local_time) {
                let _ = write!(out, " · {time}");
            }
            out.push_str("</div><div class=\"prompt\">");
            escape_into(&mut out, &prompt.text);
            out.push_str("</div>");
        }
        if let Some(line) = summary(session.agent, &turn.work) {
            let _ = write!(
                out,
                "<details id=\"w{n}\"><summary>{}</summary><div class=\"work\">",
                escape(&line)
            );
            for m in &turn.work {
                work_item(m, colors, &mut out);
            }
            out.push_str("</div></details>");
        }
        if let Some(answer) = turn.answer {
            out.push_str("<div class=\"answer\">");
            markdown(&answer.text, colors, &mut out);
            out.push_str("</div>");
        }
        out.push_str("</section>\n");
    }
    out.push_str("</main>");
    out
}

/// The whole page: [`main_html`] in the theme's colors and the terminal's
/// font, with the script that reports clicks and the selection.
pub fn html(
    session: &Session,
    messages: &[Message],
    colors: &PreviewColors,
    font: &PageFont,
) -> String {
    let nonce = preview::nonce();
    let css = format!(
        "{faces}:root{{--mono:'{family}',ui-monospace,Menlo,monospace;--size:{size}px;\
--band:{band};--code:{code}}}{CSS}",
        faces = font.faces,
        family = font.family.replace('\'', ""),
        size = font.size,
        band = preview::hex(mix(colors.background, colors.foreground, 0.07)),
        code = preview::hex(colors.palette[4]),
    );
    let body = format!(
        "{}\n<script nonce=\"{nonce}\">{SCRIPT}</script>",
        main_html(session, messages, colors)
    );
    preview::page(colors, &nonce, &css, &body)
}

fn mix(a: motty_theme::Rgba, b: motty_theme::Rgba, t: f32) -> motty_theme::Rgba {
    motty_theme::Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.,
    }
}

fn work_item(m: &Message, colors: &PreviewColors, out: &mut String) {
    match m.role {
        Role::Reasoning => {
            out.push_str("<div class=\"reasoning\">");
            escape_into(out, &m.text);
            out.push_str("</div>");
        }
        Role::Tool => match m.text.strip_prefix("⎿ ") {
            Some(output) => tool_output(output, out),
            None => {
                let call = m.text.strip_prefix("⏺ ").unwrap_or(&m.text);
                let (name, rest) = call.split_once("  ").unwrap_or((call, ""));
                out.push_str("<div class=\"tool\"><b>");
                escape_into(out, name);
                out.push_str("</b>");
                if !rest.is_empty() {
                    out.push_str("  ");
                    escape_into(out, rest.lines().next().unwrap_or_default());
                }
                out.push_str("</div>");
            }
        },
        Role::Assistant | Role::User => {
            out.push_str("<div class=\"text\">");
            markdown(&m.text, colors, out);
            out.push_str("</div>");
        }
    }
}

fn tool_output(output: &str, out: &mut String) {
    let lines: Vec<&str> = output.trim_matches('\n').lines().collect();
    out.push_str("<pre class=\"output\">");
    if lines.iter().all(|l| l.trim().is_empty()) {
        out.push_str("(no output)");
    }
    for (i, line) in lines.iter().take(OUTPUT_LINES).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        escape_into(out, line.trim_end());
    }
    if lines.len() > OUTPUT_LINES {
        let _ = write!(out, "\n… {} more lines", lines.len() - OUTPUT_LINES);
    }
    out.push_str("</pre>");
}

/// `text` with Markdown kept light, as Otty shows an agent's reply: list
/// markers and heading hashes stay, inline code and links are colored,
/// fenced code is highlighted. HTML in it shows as text.
pub fn markdown(text: &str, colors: &PreviewColors, out: &mut String) {
    // Item markers of the lists we're in: `None` for bullets, else the
    // next number.
    let mut lists: Vec<Option<u64>> = Vec::new();
    // A list item's first paragraph runs on from its marker, without a
    // `<p>` of its own; one bool per open paragraph says if it has one.
    let mut item_start = false;
    let mut paragraphs: Vec<bool> = Vec::new();
    let mut parser = Parser::new_ext(text, preview::options());
    while let Some(event) = parser.next() {
        if !matches!(event, Event::Start(Tag::Paragraph | Tag::Item)) {
            item_start = false;
        }
        match event {
            Event::Start(Tag::Paragraph) => {
                let tagged = !std::mem::take(&mut item_start);
                if tagged {
                    out.push_str("<p>");
                }
                paragraphs.push(tagged);
            }
            Event::End(TagEnd::Paragraph) => {
                if paragraphs.pop().unwrap_or(false) {
                    out.push_str("</p>");
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let hashes = "#".repeat(heading_depth(level));
                let _ = write!(
                    out,
                    "<p class=\"h\"><span class=\"mark\">{hashes} </span><b>"
                );
            }
            Event::End(TagEnd::Heading(_)) => out.push_str("</b></p>"),
            Event::Start(Tag::List(start)) => {
                lists.push(start);
                out.push_str("<div class=\"list\">");
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
                out.push_str("</div>");
            }
            Event::Start(Tag::Item) => {
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => "- ".to_owned(),
                };
                let _ = write!(
                    out,
                    "<div class=\"li\"><span class=\"mark\">{marker}</span>"
                );
                item_start = true;
            }
            Event::End(TagEnd::Item) => out.push_str("</div>"),
            Event::TaskListMarker(done) => {
                out.push_str(if done { "[x] " } else { "[ ] " });
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let mut code = String::new();
                for event in parser.by_ref() {
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
                out.push_str(&preview::code_block(&code, lang, colors));
            }
            Event::Start(Tag::BlockQuote(_)) => out.push_str("<blockquote>"),
            Event::End(TagEnd::BlockQuote(_)) => out.push_str("</blockquote>"),
            Event::Start(Tag::Strong) => out.push_str("<b>"),
            Event::End(TagEnd::Strong) => out.push_str("</b>"),
            Event::Start(Tag::Emphasis) => out.push_str("<i>"),
            Event::End(TagEnd::Emphasis) => out.push_str("</i>"),
            Event::Start(Tag::Strikethrough) => out.push_str("<s>"),
            Event::End(TagEnd::Strikethrough) => out.push_str("</s>"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                let _ = write!(out, "<a href=\"{}\">", escape(&dest_url));
            }
            Event::End(TagEnd::Link) => out.push_str("</a>"),
            Event::Start(Tag::Image { dest_url, .. }) => {
                let _ = write!(out, "<a href=\"{}\">", escape(&dest_url));
            }
            Event::End(TagEnd::Image) => out.push_str("</a>"),
            Event::Start(Tag::Table(_)) => out.push_str("<table>"),
            Event::End(TagEnd::Table) => out.push_str("</table>"),
            Event::Start(Tag::TableHead) => out.push_str("<tr class=\"head\">"),
            Event::End(TagEnd::TableHead) => out.push_str("</tr>"),
            Event::Start(Tag::TableRow) => out.push_str("<tr>"),
            Event::End(TagEnd::TableRow) => out.push_str("</tr>"),
            Event::Start(Tag::TableCell) => out.push_str("<td>"),
            Event::End(TagEnd::TableCell) => out.push_str("</td>"),
            Event::Code(code) => {
                out.push_str("<span class=\"code\">");
                escape_into(out, &code);
                out.push_str("</span>");
            }
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => escape_into(out, &t),
            Event::SoftBreak => out.push('\n'),
            Event::HardBreak => out.push_str("<br>"),
            Event::Rule => out.push_str("<hr>"),
            _ => {}
        }
    }
}

fn heading_depth(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// An RFC 3339 timestamp as the local `12:42:03`, with the date in front
/// when it isn't today's.
pub fn local_time(stamp: &str) -> Option<String> {
    let secs = parse_rfc3339(stamp)?;
    let local = |secs: i64| {
        let t = secs as libc::time_t;
        // SAFETY: localtime_r writes into the struct we pass.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            tm
        }
    };
    let tm = local(secs);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let today = local(now);
    let clock = format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec);
    if (tm.tm_year, tm.tm_yday) == (today.tm_year, today.tm_yday) {
        return Some(clock);
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS.get(tm.tm_mon as usize)?;
    Some(if tm.tm_year == today.tm_year {
        format!("{month} {} {clock}", tm.tm_mday)
    } else {
        format!("{month} {} {}, {clock}", tm.tm_mday, tm.tm_year + 1900)
    })
}

/// Seconds since the epoch of `2026-10-29T09:12:03.000Z` (or `+03:30`).
fn parse_rfc3339(stamp: &str) -> Option<i64> {
    let b = stamp.as_bytes();
    let num = |r: std::ops::Range<usize>| stamp.get(r)?.parse::<i64>().ok();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, s) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &stamp[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest.as_bytes().first() {
        None | Some(b'Z' | b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let hm = &rest[1..];
            let (oh, om) = hm.split_once(':').unwrap_or((hm.get(..2)?, hm.get(2..)?));
            let minutes = oh.parse::<i64>().ok()? * 60 + om.parse::<i64>().ok()?;
            if *sign == b'+' {
                minutes * 60
            } else {
                -minutes * 60
            }
        }
        _ => return None,
    };
    Some(days_from_civil(y, mo, d)? * 86_400 + h * 3600 + mi * 60 + s - offset)
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// What the page reports.
#[derive(Debug, PartialEq)]
pub enum PageMessage {
    /// A right-click at this point of the page.
    Menu { x: f32, y: f32 },
    /// The selected text changed.
    Selection(String),
    /// A link was clicked.
    Open(String),
}

impl PageMessage {
    pub fn parse(message: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(message).ok()?;
        if let Some(url) = v["open"].as_str() {
            return Some(Self::Open(url.to_owned()));
        }
        if let Some([x, y]) = v["menu"].as_array().map(Vec::as_slice) {
            return Some(Self::Menu {
                x: x.as_f64()? as f32,
                y: y.as_f64()? as f32,
            });
        }
        v["selection"]
            .as_str()
            .map(|s| Self::Selection(s.to_owned()))
    }
}

/// The script that refreshes `<main>` in place as the session grows:
/// unfolded turns stay unfolded, and a page scrolled to its end follows it.
pub fn update_script(main: &str) -> String {
    format!(
        "window.mottyUpdate && mottyUpdate({})",
        serde_json::Value::String(main.to_owned())
    )
}

/// Where a session's page lives, beside other previews.
pub fn is_session_log(path: &Path) -> bool {
    matches!(crate::file_kind::extension(path).as_str(), "jsonl" | "json")
}

const SCRIPT: &str = "\
const post=m=>{try{webkit.messageHandlers.motty.postMessage(JSON.stringify(m))}catch(e){}};\
document.addEventListener('contextmenu',e=>{e.preventDefault();\
post({selection:String(getSelection())});post({menu:[e.clientX,e.clientY]})});\
let pending;document.addEventListener('selectionchange',()=>{clearTimeout(pending);\
pending=setTimeout(()=>post({selection:String(getSelection())}),120)});\
document.addEventListener('click',e=>{const a=e.target.closest('a[href]');\
if(a){e.preventDefault();post({open:a.href})}});\
window.mottyUpdate=html=>{\
const open=[...document.querySelectorAll('details[open]')].map(d=>d.id);\
const end=innerHeight+scrollY>=document.documentElement.scrollHeight-4,y=scrollY;\
document.querySelector('main').outerHTML=html;\
for(const id of open){const d=document.getElementById(id);if(d)d.open=true}\
scrollTo(0,end?document.documentElement.scrollHeight:y)};";

const CSS: &str = "\
main{box-sizing:border-box;padding:16px 16px 48px;font:var(--size)/1.55 var(--mono);\
word-wrap:break-word}\
header{margin-bottom:22px}\
h1{margin:6px 0 4px;font:700 calc(var(--size) + 2px)/1.3 var(--mono)}\
.meta,.who,summary{font-family:-apple-system,BlinkMacSystemFont,sans-serif;font-size:11.5px;\
color:var(--muted)}\
.meta{line-height:1.5}\
.who{display:flex;align-items:center;gap:7px;margin:22px 0 8px}\
.dot{width:7px;height:7px;border-radius:50%;background:var(--muted)}\
.prompt{margin:0 -16px;padding:13px 16px;background:var(--band);white-space:pre-wrap}\
details{margin:12px 0 10px}\
summary{list-style:none;cursor:default;user-select:none}\
summary::-webkit-details-marker{display:none}\
summary::before{content:'›';display:inline-block;width:14px;transition:transform .12s}\
details[open]>summary::before{transform:rotate(90deg)}\
.work{margin:8px 0 4px 6px;padding-left:12px;border-left:1px solid var(--border)}\
.work>*{margin:0 0 8px}\
.reasoning{font-style:italic;color:var(--muted);white-space:pre-wrap}\
.tool{white-space:pre-wrap}\
pre.output{margin:-4px 0 8px;padding:0 0 0 14px;font:inherit;font-size:calc(var(--size) - 1px);\
color:var(--muted);white-space:pre-wrap;background:none}\
.answer{margin-top:12px}\
.answer p,.text p,.answer .h,.text .h{margin:0 0 .8em;white-space:pre-wrap}\
.list{margin:0 0 .8em}.list .list{margin:0 0 0 2ch}\
.li{white-space:pre-wrap}\
.mark{color:var(--muted)}\
.code{color:var(--code)}\
a{color:var(--accent);text-decoration:underline;text-underline-offset:2px}\
blockquote{margin:0 0 .8em;padding-left:1em;color:var(--muted);border-left:2px solid var(--border)}\
pre{margin:0 0 .8em;padding:10px 12px;border-radius:6px;background:var(--code-bg);\
font:inherit;font-size:calc(var(--size) - .5px);overflow:auto}\
pre code{font:inherit;padding:0;background:none}\
table{border-collapse:collapse;margin:0 0 .8em}td{padding:3px 10px;border:1px solid var(--border)}\
tr.head td{font-weight:700}\
hr{height:1px;border:0;background:var(--border);margin:1em 0}";

#[cfg(test)]
mod tests {
    use super::*;

    fn m(role: Role, text: &str) -> Message {
        Message {
            role,
            text: text.to_owned(),
            time: None,
        }
    }

    #[test]
    fn turns_fold_the_work_and_keep_the_answer() {
        let messages = [
            m(Role::User, "Add a flag"),
            m(Role::Reasoning, "hmm"),
            m(Role::Assistant, "I'll look first."),
            m(Role::Tool, "⏺ Read  src/main.rs"),
            m(Role::Tool, "⎿ fn main() {}"),
            m(Role::Tool, "⏺ Edit  src/main.rs"),
            m(Role::Assistant, "Done."),
            m(Role::User, "Thanks"),
            m(Role::Assistant, "Sure."),
        ];
        let turns = turns(&messages);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].prompt.unwrap().text, "Add a flag");
        assert_eq!(turns[0].answer.unwrap().text, "Done.");
        assert_eq!(turns[0].work.len(), 5);
        assert_eq!(
            summary("claude", &turns[0].work).as_deref(),
            Some("Claude · Edit, Read · 19 chars")
        );
        assert!(turns[1].work.is_empty());
        assert_eq!(summary("claude", &turns[1].work), None);
    }

    #[test]
    fn work_before_any_prompt_gets_a_turn() {
        let messages = [m(Role::Assistant, "Resumed."), m(Role::User, "Go on")];
        let turns = turns(&messages);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].prompt, None);
        assert_eq!(turns[0].answer.unwrap().text, "Resumed.");
    }

    #[test]
    fn agent_names_lose_their_suffix() {
        assert_eq!(short_name("claude"), "Claude");
        assert_eq!(short_name("codex"), "Codex");
    }

    fn colors() -> PreviewColors {
        let c = motty_theme::Rgba::rgb(0x808080);
        PreviewColors {
            background: c,
            foreground: c,
            muted: c,
            accent: c,
            code_background: c,
            border: c,
            palette: [c; 16],
            dark: true,
        }
    }

    #[test]
    fn markdown_stays_light() {
        let mut out = String::new();
        markdown(
            "Done. `--verbose` now **prints**.\n\n- `src/main.rs`: new field\n- second\n\n1. one\n2. two\n\n## Next <b>x</b>",
            &colors(),
            &mut out,
        );
        assert_eq!(
            out,
            "<p>Done. <span class=\"code\">--verbose</span> now <b>prints</b>.</p>\
<div class=\"list\"><div class=\"li\"><span class=\"mark\">- </span><span class=\"code\">src/main.rs</span>: new field</div>\
<div class=\"li\"><span class=\"mark\">- </span>second</div></div>\
<div class=\"list\"><div class=\"li\"><span class=\"mark\">1. </span>one</div>\
<div class=\"li\"><span class=\"mark\">2. </span>two</div></div>\
<p class=\"h\"><span class=\"mark\">## </span><b>Next &lt;b&gt;x&lt;/b&gt;</b></p>"
        );
    }

    #[test]
    fn timestamps() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339("2026-10-29T09:12:03.000Z"),
            Some(1_793_265_123)
        );
        assert_eq!(
            parse_rfc3339("2026-10-29T12:42:03+03:30"),
            parse_rfc3339("2026-10-29T09:12:03Z")
        );
        assert_eq!(parse_rfc3339("yesterday"), None);
    }

    #[test]
    fn page_messages() {
        assert_eq!(
            PageMessage::parse(r#"{"menu":[10,20.5]}"#),
            Some(PageMessage::Menu { x: 10., y: 20.5 })
        );
        assert_eq!(
            PageMessage::parse(r#"{"selection":"abc"}"#),
            Some(PageMessage::Selection("abc".into()))
        );
        assert_eq!(
            PageMessage::parse(r#"{"open":"https://x.dev"}"#),
            Some(PageMessage::Open("https://x.dev".into()))
        );
        assert_eq!(PageMessage::parse("nope"), None);
    }

    #[test]
    fn page_escapes_the_session() {
        let session = Session {
            agent: "claude",
            id: "abc".into(),
            path: "/x/abc.jsonl".into(),
            cwd: Some("/work".into()),
            title: "<script>".into(),
            modified: std::time::SystemTime::UNIX_EPOCH,
        };
        let main = main_html(&session, &[m(Role::User, "a < b")], &colors());
        assert!(main.contains("<h1>&lt;script&gt;</h1>"));
        assert!(main.contains("<div class=\"prompt\">a &lt; b</div>"));
        assert!(main.contains("<div class=\"meta\">/work</div>"));
    }
}
