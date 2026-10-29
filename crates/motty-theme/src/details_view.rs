//! `details-view`: the details panel's tabs, in order, as in Otty. A line
//! names a built-in tab (`info`, `outline`, `git`, `files`) or a view of
//! your own: a program in an embedded terminal, or a web page.
//!
//! ```toml
//! details-view = 'id="docker" kind="tui" title="Docker" command="lazydocker"'
//! details-view = 'id="grafana" kind="web" url="http://localhost:3000"'
//! details-view = 'id="git" enabled="false"'
//! ```

use std::path::PathBuf;

/// The built-in tabs' ids, in their default order.
pub const BUILTIN_VIEWS: [&str; 4] = ["info", "outline", "git", "files"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewKind {
    #[default]
    Builtin,
    /// A program in an embedded terminal.
    Tui,
    /// A web page.
    Web,
}

impl ViewKind {
    fn name(self) -> &'static str {
        match self {
            ViewKind::Builtin => "builtin",
            ViewKind::Tui => "tui",
            ViewKind::Web => "web",
        }
    }
}

/// One `details-view` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailsView {
    pub id: String,
    pub kind: ViewKind,
    /// The tab's label; a built-in's own name when empty.
    pub title: Option<String>,
    /// A tab icon name or an emoji.
    pub icon: Option<String>,
    /// `tui`: the command line, with `${…}` variables.
    pub command: Option<String>,
    /// `web`: the page.
    pub url: Option<String>,
    /// `web`: ask for the site's phone layout (on by default).
    pub mobile: bool,
    /// `tui`: where it runs; `~/.config/motty/views/<id>` when empty.
    pub cwd: Option<String>,
    pub enabled: bool,
}

impl DetailsView {
    /// A built-in tab, switched on.
    pub fn builtin(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            kind: ViewKind::Builtin,
            title: None,
            icon: None,
            command: None,
            url: None,
            mobile: true,
            cwd: None,
            enabled: true,
        }
    }

    /// Reads `field="value"` pairs; `\"` escapes a quote inside a value.
    pub fn parse(line: &str) -> Result<Self, String> {
        let mut view = Self::builtin("");
        let mut kind = None;
        for (key, value) in pairs(line)? {
            let text = Some(value.clone()).filter(|v| !v.is_empty());
            match key.as_str() {
                "id" => view.id = value,
                "kind" => {
                    kind = Some(match value.as_str() {
                        "builtin" => ViewKind::Builtin,
                        "tui" => ViewKind::Tui,
                        "web" => ViewKind::Web,
                        _ => return Err(format!("unknown details-view kind {value:?}")),
                    })
                }
                "title" => view.title = text,
                "icon" => view.icon = text,
                "command" => view.command = text,
                "url" => view.url = text,
                "cwd" => view.cwd = text,
                "mobile" => view.mobile = value != "false",
                "enabled" => view.enabled = value != "false",
                _ => return Err(format!("unknown details-view field {key:?}")),
            }
        }
        if view.id.is_empty() {
            return Err("a details view needs an id".into());
        }
        view.kind = kind.unwrap_or(if view.url.is_some() {
            ViewKind::Web
        } else if view.command.is_some() {
            ViewKind::Tui
        } else {
            ViewKind::Builtin
        });
        let builtin = BUILTIN_VIEWS.contains(&view.id.as_str());
        match view.kind {
            ViewKind::Builtin if !builtin => Err(format!("{:?} isn't a built-in view", view.id)),
            ViewKind::Tui if view.command.is_none() => {
                Err(format!("view {:?} needs a command", view.id))
            }
            ViewKind::Web if view.url.is_none() => Err(format!("view {:?} needs a url", view.id)),
            _ => Ok(view),
        }
    }

    /// The line as written to the config file.
    pub fn to_line(&self) -> String {
        let kind = (self.kind != ViewKind::Builtin).then(|| self.kind.name().to_owned());
        let off = |on: bool| (!on).then(|| "false".to_owned());
        let mobile = off(self.mobile || self.kind != ViewKind::Web);
        let fields = [
            ("id", Some(self.id.clone())),
            ("kind", kind),
            ("title", self.title.clone()),
            ("icon", self.icon.clone()),
            ("command", self.command.clone()),
            ("url", self.url.clone()),
            ("mobile", mobile),
            ("cwd", self.cwd.clone()),
            ("enabled", off(self.enabled)),
        ];
        fields
            .iter()
            .filter_map(|(k, v)| {
                let v = v.as_deref()?;
                Some(format!(
                    "{k}=\"{}\"",
                    v.replace('\\', "\\\\").replace('"', "\\\"")
                ))
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Where a program view runs when it names no folder.
    pub fn home(&self) -> PathBuf {
        crate::config::motty_dir().join("views").join(&self.id)
    }
}

/// The panel's tabs: the configured ones in order, then the built-ins not
/// listed (so a new built-in still shows up).
pub fn resolve(configured: &[DetailsView]) -> Vec<DetailsView> {
    let mut out: Vec<DetailsView> = Vec::new();
    for view in configured {
        if !out.iter().any(|v| v.id == view.id) {
            out.push(view.clone());
        }
    }
    for id in BUILTIN_VIEWS {
        if !out.iter().any(|v| v.id == id) {
            out.push(DetailsView::builtin(id));
        }
    }
    out
}

/// A name for a new view that no other one has: `name`, `name-2`, …
pub fn unique_id(name: &str, taken: &[DetailsView]) -> String {
    let base: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    let base = if base.is_empty() { "view".into() } else { base };
    let free = |id: &str| !taken.iter().any(|v| v.id == id) && !BUILTIN_VIEWS.contains(&id);
    if free(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| free(id))
        .unwrap_or(base)
}

/// Replaces every `details-view` line with `views`, in order, where the
/// first one was (or at the end).
pub fn save(views: &[DetailsView]) -> std::io::Result<PathBuf> {
    crate::config::edit(|src| replace_lines(src, views))
}

fn replace_lines(src: &str, views: &[DetailsView]) -> String {
    let lines: String = views
        .iter()
        .map(|v| {
            let line = v.to_line();
            // A TOML literal string can't hold a `'`; fall back to a basic one.
            if line.contains('\'') {
                format!("details-view = {}\n", toml::Value::String(line))
            } else {
                format!("details-view = '{line}'\n")
            }
        })
        .collect();
    let mut out = String::new();
    let mut placed = false;
    for line in src.lines() {
        let key = line
            .trim_start()
            .split(['=', ' '])
            .next()
            .unwrap_or_default();
        if key == "details-view" {
            if !placed {
                out.push_str(&lines);
                placed = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !placed {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(&lines);
    }
    out
}

/// The 0-based line of the config file that defines view `id`.
pub fn line_of(src: &str, id: &str) -> Option<usize> {
    src.lines().position(|line| {
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        if key.trim() != "details-view" {
            return false;
        }
        let value = value.trim();
        let text = toml::from_str::<toml::Table>(&format!("v = {value}"))
            .ok()
            .and_then(|t| t.get("v")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| value.to_owned());
        DetailsView::parse(&text).is_ok_and(|v| v.id == id)
    })
}

fn pairs(line: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut rest = line.trim();
    while !rest.is_empty() {
        let eq = rest
            .find('=')
            .ok_or_else(|| format!("expected field=\"value\" in {line:?}"))?;
        let key = rest[..eq].trim().to_owned();
        let body = rest[eq + 1..]
            .trim_start()
            .strip_prefix('"')
            .ok_or_else(|| format!("{key}: the value needs double quotes"))?;
        let mut value = String::new();
        let mut end = None;
        let mut chars = body.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    if let Some((_, next)) = chars.next() {
                        value.push(next);
                    }
                }
                '"' => {
                    end = Some(i);
                    break;
                }
                c => value.push(c),
            }
        }
        let end = end.ok_or_else(|| format!("{key}: missing closing quote"))?;
        out.push((key, value));
        rest = body[end + 1..].trim_start();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_otty_examples() {
        let v = DetailsView::parse(
            r#"id="lazygit" kind="tui" title="Git" command="lazygit -p \"${cwd}\"""#,
        )
        .unwrap();
        assert_eq!(v.kind, ViewKind::Tui);
        assert_eq!(v.title.as_deref(), Some("Git"));
        assert_eq!(v.command.as_deref(), Some(r#"lazygit -p "${cwd}""#));
        let w =
            DetailsView::parse(r#"id="admin" url="http://localhost:8080" mobile="false""#).unwrap();
        assert_eq!(w.kind, ViewKind::Web);
        assert!(!w.mobile);
        let off = DetailsView::parse(r#"id="git" enabled="false""#).unwrap();
        assert_eq!(off.kind, ViewKind::Builtin);
        assert!(!off.enabled);
        assert!(DetailsView::parse(r#"id="nope""#).is_err());
        assert!(DetailsView::parse(r#"id="x" kind="tui""#).is_err());
        assert!(DetailsView::parse(r#"kind="web" url="http://a""#).is_err());
        assert!(DetailsView::parse(r#"id="x" colour="red""#).is_err());
    }

    #[test]
    fn round_trips_and_resolves() {
        let line = r#"id="top" kind="tui" title="It's top" command="top -o \"cpu\"""#;
        let v = DetailsView::parse(line).unwrap();
        assert_eq!(DetailsView::parse(&v.to_line()).unwrap(), v);
        let out = replace_lines("theme = \"Nord\"\n", std::slice::from_ref(&v));
        let parsed = crate::Config::parse(&out).details_views;
        assert_eq!(parsed, std::slice::from_ref(&v));
        assert_eq!(line_of(&out, "top"), Some(2));
        let tabs = resolve(&[DetailsView::builtin("git"), v.clone()]);
        let ids: Vec<&str> = tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["git", "top", "info", "outline", "files"]);
        let src = "a = 1\ndetails-view = 'id=\"x\" url=\"http://x\"'\nb = 2\ndetails-view = 'id=\"git\"'\n";
        let out = replace_lines(src, &[v]);
        assert_eq!(out.matches("details-view").count(), 1);
        assert!(out.starts_with("a = 1\ndetails-view"));
    }

    #[test]
    fn new_ids() {
        let taken = [DetailsView::parse(r#"id="htop" command="htop""#).unwrap()];
        assert_eq!(unique_id("Docker Stats", &taken), "docker-stats");
        assert_eq!(unique_id("htop", &taken), "htop-2");
        assert_eq!(unique_id("Git", &taken), "git-2");
        assert_eq!(unique_id("  ", &taken), "view");
    }
}
