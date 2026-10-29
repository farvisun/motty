//! `tab-rule`: per project, agent, host or file, a tab's alias, icon and
//! title template, as in Otty.
//!
//! ```toml
//! tab-rule = 'path="~/Work/otty" alias="Otty" icon="terminal" title="${alias} · ${branch}"'
//! ```

use std::path::{Path, PathBuf};

/// One `tab-rule` line: match conditions (all that are set must match)
/// and what the rule gives a matching tab.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabRule {
    pub path: Option<String>,
    pub command: Option<String>,
    pub agent: Option<String>,
    pub host: Option<String>,
    pub file: Option<String>,
    pub alias: Option<String>,
    pub icon: Option<String>,
    pub icon_color: Option<String>,
    pub title: Option<String>,
}

/// What a tab is showing, for matching rules and filling in titles.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub cwd: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// The foreground command line while one runs.
    pub command: Option<String>,
    /// The bound agent's id (`claude`), for matching…
    pub agent: Option<String>,
    /// …and its name (`Claude Code`), for `${agent}`.
    pub agent_label: Option<String>,
    pub user: Option<String>,
    pub host: Option<String>,
    /// A file or folder pane's name; terminals have none.
    pub file: Option<String>,
    pub branch: Option<String>,
    /// Whatever title the program set.
    pub title: Option<String>,
    pub shell: Option<String>,
    /// The tab's position in the window, from 1.
    pub index: usize,
}

/// What the rules decided; each part comes from the first rule that
/// matches and sets it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub alias: Option<String>,
    pub icon: Option<String>,
    pub icon_color: Option<String>,
    pub title: Option<String>,
}

impl TabRule {
    /// Reads `field="value"` pairs; `\"` escapes a quote inside a value.
    pub fn parse(line: &str) -> Result<Self, String> {
        let mut rule = Self::default();
        let mut rest = line.trim();
        while !rest.is_empty() {
            let eq = rest
                .find('=')
                .ok_or_else(|| format!("expected field=\"value\" in {line:?}"))?;
            let key = rest[..eq].trim();
            let after = rest[eq + 1..].trim_start();
            let body = after
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
            let slot = match key {
                "path" => &mut rule.path,
                "command" => &mut rule.command,
                "agent" => &mut rule.agent,
                "host" => &mut rule.host,
                "file" => &mut rule.file,
                "alias" => &mut rule.alias,
                "icon" => &mut rule.icon,
                "icon-color" => &mut rule.icon_color,
                "title" => &mut rule.title,
                _ => return Err(format!("unknown tab-rule field {key:?}")),
            };
            *slot = Some(value).filter(|v| !v.is_empty());
            rest = body[end + 1..].trim_start();
        }
        if rule.alias.is_none() && rule.icon.is_none() && rule.title.is_none() {
            return Err("a tab rule needs an alias, icon or title".into());
        }
        Ok(rule)
    }

    /// The line as written to the config file.
    pub fn to_line(&self) -> String {
        let fields = [
            ("path", &self.path),
            ("command", &self.command),
            ("agent", &self.agent),
            ("host", &self.host),
            ("file", &self.file),
            ("alias", &self.alias),
            ("icon", &self.icon),
            ("icon-color", &self.icon_color),
            ("title", &self.title),
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

    /// The rule's condition as one phrase, for lists (`~/Work/otty`,
    /// `ssh *`, `claude`).
    pub fn condition(&self) -> Option<(&'static str, &str)> {
        [
            ("path", &self.path),
            ("command", &self.command),
            ("agent", &self.agent),
            ("host", &self.host),
            ("file", &self.file),
        ]
        .into_iter()
        .find_map(|(k, v)| Some((k, v.as_deref()?)))
    }

    pub fn matches(&self, f: &Facts) -> bool {
        let home = f.home.as_deref();
        let ok = |pattern: &Option<String>, value: Option<&str>| match pattern {
            None => true,
            Some(p) => value.is_some_and(|v| glob(p, v)),
        };
        let path_ok = match &self.path {
            None => true,
            Some(p) => f
                .cwd
                .as_deref()
                .is_some_and(|cwd| path_matches(p, cwd, home)),
        };
        path_ok
            && ok(&self.command, f.command.as_deref())
            && ok(&self.agent, f.agent.as_deref())
            && ok(&self.host, f.host.as_deref())
            && ok(&self.file, f.file.as_deref())
    }
}

/// Each of alias, icon and title from the first matching rule that sets
/// it; the title template filled in.
pub fn resolve(rules: &[TabRule], facts: &Facts) -> Look {
    let mut look = Look::default();
    let mut template = None;
    for rule in rules.iter().filter(|r| r.matches(facts)) {
        if look.alias.is_none() {
            look.alias.clone_from(&rule.alias);
        }
        if look.icon.is_none() && rule.icon.is_some() {
            look.icon.clone_from(&rule.icon);
            look.icon_color.clone_from(&rule.icon_color);
        }
        if template.is_none() {
            template.clone_from(&rule.title);
        }
    }
    look.title = template
        .map(|t| expand(&t, |name| variable(name, facts, look.alias.as_deref())))
        .filter(|t| !t.is_empty());
    look
}

/// A title variable's value; empty and unknown ones have none.
fn variable(name: &str, f: &Facts, alias: Option<&str>) -> Option<String> {
    let cwd = || f.cwd.as_deref();
    let v = match name {
        "alias" => alias.map(str::to_owned),
        "cwd" => cwd().map(|c| tilde(c, f.home.as_deref())),
        "folder" => cwd().map(|c| match c.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => c.display().to_string(),
        }),
        "user" => f.user.clone(),
        "host" => f.host.clone(),
        "agent" => f.agent_label.clone().or_else(|| f.agent.clone()),
        "branch" => f.branch.clone(),
        "command" => f.command.clone(),
        "title" => f.title.clone(),
        "shell" => f.shell.clone(),
        "file" => f.file.clone(),
        "index" => Some(f.index.to_string()),
        _ => None,
    };
    v.filter(|v| !v.is_empty())
}

/// `~/…` for paths under home.
pub fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rel) if rel.as_os_str().is_empty() => "~".into(),
        Some(rel) => format!("~/{}", rel.display()),
        None => path.display().to_string(),
    }
}

enum Part {
    Text(String),
    /// Punctuation and spaces between words, dropped when it ends up
    /// stranded next to a variable with nothing to show.
    Sep(String),
    Var(Option<String>),
}

/// Fills in `${name}` and `${a|b|'text'}`; a variable with no value goes,
/// and so does a separator it leaves stranded.
pub fn expand(template: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        literal(&rest[..start], &mut parts);
        let Some(len) = rest[start..].find('}') else {
            literal(&rest[start..], &mut parts);
            rest = "";
            break;
        };
        let chain = &rest[start + 2..start + len];
        let v = chain.split('|').find_map(|link| {
            let link = link.trim();
            match link.strip_prefix('\'').and_then(|l| l.strip_suffix('\'')) {
                Some(text) => Some(text.to_owned()),
                None => value(link),
            }
        });
        parts.push(Part::Var(v.filter(|v| !v.is_empty())));
        rest = &rest[start + len + 1..];
    }
    literal(rest, &mut parts);

    let mut keep = vec![true; parts.len()];
    for i in 0..parts.len() {
        if !matches!(parts[i], Part::Var(None)) {
            continue;
        }
        keep[i] = false;
        let left = (0..i).rev().find(|&j| keep[j]);
        let right = (i + 1..parts.len()).find(|&j| keep[j]);
        let is_sep = |j: Option<usize>| j.is_some_and(|j| matches!(parts[j], Part::Sep(_)));
        match (left, right) {
            // `a · ${b} · c` keeps one separator.
            (l, r) if is_sep(l) && is_sep(r) => keep[r.unwrap()] = false,
            (l, None) if is_sep(l) => keep[l.unwrap()] = false,
            (None, r) if is_sep(r) => keep[r.unwrap()] = false,
            _ => {}
        }
    }
    let mut out = String::new();
    for (part, keep) in parts.into_iter().zip(keep) {
        if keep {
            match part {
                Part::Text(s) | Part::Sep(s) | Part::Var(Some(s)) => out.push_str(&s),
                Part::Var(None) => {}
            }
        }
    }
    out.trim().to_owned()
}

/// Splits literal text into its words and the separators around them.
fn literal(text: &str, parts: &mut Vec<Part>) {
    if text.is_empty() {
        return;
    }
    let first = text.find(|c: char| c.is_alphanumeric());
    let last = text.rfind(|c: char| c.is_alphanumeric());
    let (Some(first), Some(last)) = (first, last) else {
        parts.push(Part::Sep(text.to_owned()));
        return;
    };
    let end = last + text[last..].chars().next().map_or(1, char::len_utf8);
    if first > 0 {
        parts.push(Part::Sep(text[..first].to_owned()));
    }
    parts.push(Part::Text(text[first..end].to_owned()));
    if end < text.len() {
        parts.push(Part::Sep(text[end..].to_owned()));
    }
}

/// `pattern` against `path`: `~` is home, `*` spans `/`, and a pattern
/// without `*` also covers everything below it.
fn path_matches(pattern: &str, path: &Path, home: Option<&Path>) -> bool {
    let expanded = match (pattern.strip_prefix('~'), home) {
        (Some(rest), Some(home)) => format!("{}{rest}", home.display()),
        _ => pattern.to_owned(),
    };
    let expanded = expanded.trim_end_matches('/');
    let path = path.to_string_lossy();
    if expanded.contains('*') || expanded.contains('?') {
        return glob(expanded, &path);
    }
    let (p, e) = (path.to_lowercase(), expanded.to_lowercase());
    p == e || p.strip_prefix(&e).is_some_and(|r| r.starts_with('/')) || e.is_empty()
}

/// Case-insensitive glob: `*` is any run of characters (`/` included),
/// `?` any one.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            cwd: Some("/Users/me/Work/otty/src".into()),
            home: Some("/Users/me".into()),
            shell: Some("zsh".into()),
            index: 2,
            ..Default::default()
        }
    }

    #[test]
    fn parses_and_writes_rules() {
        let line =
            r#"path="~/Work/otty" alias="Otty" icon="terminal" title="${alias} · ${branch}""#;
        let rule = TabRule::parse(line).unwrap();
        assert_eq!(rule.path.as_deref(), Some("~/Work/otty"));
        assert_eq!(rule.title.as_deref(), Some("${alias} · ${branch}"));
        assert_eq!(TabRule::parse(&rule.to_line()).unwrap(), rule);
        let quoted = TabRule::parse(r#"command="echo \"hi\"" title="x""#).unwrap();
        assert_eq!(quoted.command.as_deref(), Some("echo \"hi\""));
        assert!(TabRule::parse(r#"path="~/x""#).is_err());
        assert!(TabRule::parse(r#"colour="red" title="x""#).is_err());
        assert!(TabRule::parse(r#"title="x"#).is_err());
    }

    #[test]
    fn paths_cover_their_subtree() {
        let home = Some(Path::new("/Users/me"));
        let cwd = Path::new("/Users/me/Work/otty/src");
        assert!(path_matches("~/Work/otty", cwd, home));
        assert!(path_matches("~/work/OTTY/", cwd, home));
        assert!(!path_matches("~/Work/ot", cwd, home));
        assert!(path_matches("~/Work/*", cwd, home));
        assert!(path_matches("*/src", cwd, home));
        assert!(!path_matches("~/Play/*", cwd, home));
    }

    #[test]
    fn globs() {
        assert!(glob("ssh *", "ssh db-01"));
        assert!(!glob("ssh *", "sshd"));
        assert!(glob("*.md", "README.MD"));
        assert!(glob("a?c", "abc"));
        assert!(glob("*", ""));
    }

    #[test]
    fn stranded_separators_go() {
        let v = |name: &str| match name {
            "folder" => Some("otty".to_owned()),
            "alias" => Some("Otty".to_owned()),
            _ => None,
        };
        assert_eq!(expand("${folder} · ${branch}", v), "otty");
        assert_eq!(expand("${branch} · ${folder}", v), "otty");
        assert_eq!(expand("${alias} · ${branch} · ${folder}", v), "Otty · otty");
        assert_eq!(expand("Typora: ${title}", v), "Typora");
        assert_eq!(expand("Typora: ${title|folder}", v), "Typora: otty");
        assert_eq!(expand("${title|file|'Untitled'}", v), "Untitled");
        assert_eq!(expand("${title|Untitled}", v), "");
        assert_eq!(expand("[${alias}]", v), "[Otty]");
        assert_eq!(expand("plain", v), "plain");
        assert_eq!(expand("${unclosed", v), "${unclosed");
    }

    #[test]
    fn each_part_comes_from_the_first_rule_that_sets_it() {
        let rules: Vec<TabRule> = [
            r#"path="~/Work/otty" title="${alias}: ${title|folder}""#,
            r#"path="~/Work/*" alias="Work" icon="folder" icon-color="blue""#,
            r#"command="ssh *" icon="server" title="${user}@${host}""#,
            r#"title="${shell} ${index}""#,
        ]
        .iter()
        .map(|l| TabRule::parse(l).unwrap())
        .collect();
        let look = resolve(&rules, &facts());
        assert_eq!(look.alias.as_deref(), Some("Work"));
        assert_eq!(look.icon.as_deref(), Some("folder"));
        assert_eq!(look.icon_color.as_deref(), Some("blue"));
        assert_eq!(look.title.as_deref(), Some("Work: src"));

        let elsewhere = Facts {
            cwd: Some("/tmp".into()),
            command: Some("ssh admin@db".into()),
            user: Some("admin".into()),
            host: Some("db".into()),
            ..facts()
        };
        let look = resolve(&rules, &elsewhere);
        assert_eq!(look.icon.as_deref(), Some("server"));
        assert_eq!(look.title.as_deref(), Some("admin@db"));

        let plain = Facts {
            cwd: Some("/tmp".into()),
            ..facts()
        };
        assert_eq!(resolve(&rules, &plain).title.as_deref(), Some("zsh 2"));
    }

    #[test]
    fn file_rules_skip_terminals() {
        let rule = TabRule::parse(r#"file="*.md" icon="markdown""#).unwrap();
        assert!(!rule.matches(&facts()));
        let file = Facts {
            file: Some("README.md".into()),
            ..facts()
        };
        assert!(rule.matches(&file));
    }
}
