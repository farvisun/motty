//! `--window`, `--tab` and `--pane` selectors, as the CLI and `motty://`
//! links spell them. Parsing is here; the app resolves them against its
//! windows (`motty_app::ipc::resolve_window` and friends).
//!
//! - window: `w_<id>`, a 1-based index (frontmost first), `current`,
//!   `last` (the window used before the current one) or `title:<pattern>`
//!   (case-insensitive; `*` and `?` are wildcards, else a substring).
//! - tab: `t_<id>`, a 1-based index in its window, or `current`.
//! - pane: `p_<id>` or a bare id (what `$MOTTY_PANE_ID` holds),
//!   `index:<n>` (1-based, in reading order within its tab) or `current`.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowSel {
    Id(u64),
    Index(usize),
    Current,
    Last,
    Title(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabSel {
    Id(u64),
    Index(usize),
    Current,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneSel {
    Id(u64),
    Index(usize),
    Current,
}

/// A selector that isn't one of the forms above.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadSelector {
    pub kind: &'static str,
    pub text: String,
}

impl fmt::Display for BadSelector {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let forms = match self.kind {
            "window" => "w_<id>, an index, current, last or title:<pattern>",
            "tab" => "t_<id>, an index or current",
            _ => "an id, p_<id>, index:<n> or current",
        };
        write!(
            f,
            "bad {} selector {:?} (use {forms})",
            self.kind, self.text
        )
    }
}

impl std::error::Error for BadSelector {}

fn index(s: &str) -> Option<usize> {
    s.parse().ok().filter(|&n| n > 0)
}

impl WindowSel {
    pub fn parse(s: &str) -> Result<Self, BadSelector> {
        let s = s.trim();
        let bad = || BadSelector {
            kind: "window",
            text: s.to_owned(),
        };
        Ok(match s {
            "current" => Self::Current,
            "last" => Self::Last,
            _ if s.starts_with("title:") => Self::Title(s["title:".len()..].to_owned()),
            _ => match s.strip_prefix("w_") {
                Some(id) => Self::Id(id.parse().map_err(|_| bad())?),
                None => Self::Index(index(s).ok_or_else(bad)?),
            },
        })
    }
}

impl fmt::Display for WindowSel {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Id(id) => write!(f, "w_{id}"),
            Self::Index(n) => write!(f, "{n}"),
            Self::Current => f.write_str("current"),
            Self::Last => f.write_str("last"),
            Self::Title(p) => write!(f, "title:{p}"),
        }
    }
}

impl TabSel {
    pub fn parse(s: &str) -> Result<Self, BadSelector> {
        let s = s.trim();
        let bad = || BadSelector {
            kind: "tab",
            text: s.to_owned(),
        };
        Ok(match s {
            "current" => Self::Current,
            _ => match s.strip_prefix("t_") {
                Some(id) => Self::Id(id.parse().map_err(|_| bad())?),
                None => Self::Index(index(s).ok_or_else(bad)?),
            },
        })
    }
}

impl PaneSel {
    pub fn parse(s: &str) -> Result<Self, BadSelector> {
        let s = s.trim();
        let bad = || BadSelector {
            kind: "pane",
            text: s.to_owned(),
        };
        Ok(match s {
            "current" | "focused" => Self::Current,
            _ if s.starts_with("index:") => {
                Self::Index(index(&s["index:".len()..]).ok_or_else(bad)?)
            }
            _ => Self::Id(
                s.strip_prefix("p_")
                    .unwrap_or(s)
                    .parse()
                    .map_err(|_| bad())?,
            ),
        })
    }
}

/// `title:` patterns: `*` and `?` wildcards over the whole title, else a
/// substring; case is ignored either way.
pub fn title_matches(pattern: &str, title: &str) -> bool {
    let (p, t) = (pattern.to_lowercase(), title.to_lowercase());
    if !p.contains(['*', '?']) {
        return t.contains(&p);
    }
    fn glob(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => (0..=t.len()).any(|i| glob(rest, &t[i..])),
            Some(('?', rest)) => !t.is_empty() && glob(rest, &t[1..]),
            Some((c, rest)) => t.first() == Some(c) && glob(rest, &t[1..]),
        }
    }
    let (p, t): (Vec<char>, Vec<char>) = (p.chars().collect(), t.chars().collect());
    glob(&p, &t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows() {
        assert_eq!(WindowSel::parse("w_12"), Ok(WindowSel::Id(12)));
        assert_eq!(WindowSel::parse("2"), Ok(WindowSel::Index(2)));
        assert_eq!(WindowSel::parse("current"), Ok(WindowSel::Current));
        assert_eq!(WindowSel::parse("last"), Ok(WindowSel::Last));
        assert_eq!(
            WindowSel::parse("title:api*"),
            Ok(WindowSel::Title("api*".into()))
        );
        assert!(WindowSel::parse("0").is_err());
        assert!(WindowSel::parse("w_x").is_err());
        assert!(WindowSel::parse("t_1").is_err());
        assert_eq!(WindowSel::Id(3).to_string(), "w_3");
    }

    #[test]
    fn tabs_and_panes() {
        assert_eq!(TabSel::parse("t_7"), Ok(TabSel::Id(7)));
        assert_eq!(TabSel::parse("1"), Ok(TabSel::Index(1)));
        assert!(TabSel::parse("first").is_err());
        assert_eq!(PaneSel::parse("42"), Ok(PaneSel::Id(42)));
        assert_eq!(PaneSel::parse("p_42"), Ok(PaneSel::Id(42)));
        assert_eq!(PaneSel::parse("index:2"), Ok(PaneSel::Index(2)));
        assert_eq!(PaneSel::parse("current"), Ok(PaneSel::Current));
        assert!(PaneSel::parse("index:0").is_err());
        assert!(PaneSel::parse("-1").is_err());
        let e = PaneSel::parse("x").unwrap_err().to_string();
        assert!(e.contains("bad pane selector"), "{e}");
    }

    #[test]
    fn titles() {
        assert!(title_matches("API", "my api server"));
        assert!(title_matches("api*", "API — zsh"));
        assert!(!title_matches("api*", "the api"));
        assert!(title_matches("*s?rver", "web server"));
        assert!(!title_matches("x", "abc"));
    }
}
