//! `motty://` navigation links: `motty://window/<sel>`, `motty://tab/<sel>`
//! and `motty://pane/<sel>[?line=<n>]`, the selectors being those of
//! [`crate::select`]. `line` is a pane's stable row: rows scrolled into
//! history since it started plus the screen row, so 0 is the oldest line
//! still kept until the scrollback overflows (the same rows shell
//! integration marks commands with).

use std::fmt;

pub const SCHEME: &str = "motty://";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Window(String),
    Tab(String),
    Pane { pane: String, line: Option<u64> },
}

impl fmt::Display for Link {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (kind, sel) = match self {
            Link::Window(s) => ("window", s),
            Link::Tab(s) => ("tab", s),
            Link::Pane { pane, .. } => ("pane", pane),
        };
        write!(f, "{SCHEME}{kind}/{}", encode(sel))?;
        if let Link::Pane {
            line: Some(line), ..
        } = self
        {
            write!(f, "?line={line}")?;
        }
        Ok(())
    }
}

/// A pane's link, as `motty pane link` prints it.
pub fn pane(id: u64, line: Option<u64>) -> String {
    Link::Pane {
        pane: id.to_string(),
        line,
    }
    .to_string()
}

/// A navigation link. A `line` that isn't a non-negative number is dropped,
/// so the link still focuses its pane.
pub fn parse(url: &str) -> Result<Link, String> {
    let rest = url
        .trim()
        .strip_prefix(SCHEME)
        .ok_or_else(|| format!("not a {SCHEME} link: {url}"))?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (kind, sel) = path
        .split_once('/')
        .ok_or_else(|| format!("no selector in {url}"))?;
    let sel = decode(sel.trim_end_matches('/'));
    if sel.is_empty() {
        return Err(format!("no selector in {url}"));
    }
    Ok(match kind {
        "window" => Link::Window(sel),
        "tab" => Link::Tab(sel),
        "pane" => Link::Pane {
            pane: sel,
            line: query
                .split('&')
                .find_map(|kv| kv.strip_prefix("line="))
                .and_then(|n| n.parse().ok()),
        },
        other => return Err(format!("unknown link kind {other:?} in {url}")),
    })
}

/// Percent-encodes what a selector may hold beyond plain characters.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(pane(12, Some(1204)), "motty://pane/12?line=1204");
        assert_eq!(pane(12, None), "motty://pane/12");
        assert_eq!(
            parse("motty://pane/12?line=1204"),
            Ok(Link::Pane {
                pane: "12".into(),
                line: Some(1204)
            })
        );
        let title = Link::Window("title:my api".into());
        assert_eq!(title.to_string(), "motty://window/title:my%20api");
        assert_eq!(parse(&title.to_string()), Ok(title));
        assert_eq!(parse("motty://tab/t_3"), Ok(Link::Tab("t_3".into())));
    }

    #[test]
    fn bad_lines_are_dropped_and_bad_links_refused() {
        for url in ["motty://pane/p_4?line=-3", "motty://pane/p_4?line=x"] {
            assert_eq!(
                parse(url),
                Ok(Link::Pane {
                    pane: "p_4".into(),
                    line: None
                })
            );
        }
        assert!(parse("otty://pane/1").is_err());
        assert!(parse("motty://pane/").is_err());
        assert!(parse("motty://run/ls").is_err());
    }
}
