//! Recipes: a tab or window's layout (and optionally the commands run in
//! each pane), a list of commands, or a text snippet, saved to
//! `~/.config/motty/recipes/` or exported as a `.ottyrecipe`-style TOML
//! file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::recovery::{PaneState, TabState};

pub const EXTENSION: &str = "mottyrecipe";
/// Files Otty exports open too.
pub const OTTY_EXTENSION: &str = "ottyrecipe";
/// Commands kept per pane.
pub const MAX_COMMANDS: usize = 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub recipe: Meta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Layout>,
    /// A Commands recipe's commands, for the focused pane.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<Snippet>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub name: String,
    #[serde(default = "one")]
    pub version: u32,
    pub scope: Scope,
    /// Commands written under `[recipe]`; read, then moved to the top.
    #[serde(default, skip_serializing)]
    pub commands: Vec<String>,
}

fn one() -> u32 {
    1
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Tab,
    Window,
    Commands,
    Snippet,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::Tab => "Tab",
            Scope::Window => "Window",
            Scope::Commands => "Commands",
            Scope::Snippet => "Snippet",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub tabs: Vec<TabLayout>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TabLayout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub panes: Vec<PaneLayout>,
}

/// A pane; each after the first splits what came before it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneLayout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// `right`, `down`, `left` or `up` of the previous pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<String>,
    /// The new pane's share of what it splits, 0–1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    /// How many of the panes before it the split takes in (Motty's own;
    /// one, the previous pane, when left out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snippet {
    /// What's typed at the prompt to expand it.
    pub alias: String,
    pub text: String,
}

impl Recipe {
    pub fn parse(src: &str) -> Result<Self, String> {
        let mut recipe: Recipe = toml::from_str(src).map_err(|e| e.message().to_owned())?;
        if recipe.commands.is_empty() {
            recipe.commands = std::mem::take(&mut recipe.recipe.commands);
        }
        match recipe.recipe.scope {
            Scope::Tab | Scope::Window
                if recipe
                    .window
                    .as_ref()
                    .is_none_or(|w| w.tabs.iter().all(|t| t.panes.is_empty())) =>
            {
                Err("the recipe has no panes".into())
            }
            Scope::Snippet if recipe.snippet.is_none() => Err("the snippet has no text".into()),
            Scope::Commands if recipe.commands.is_empty() => {
                Err("the recipe has no commands".into())
            }
            _ => Ok(recipe),
        }
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Every command it would run, with where: `(tab, pane, command)`.
    pub fn all_commands(&self) -> Vec<(usize, usize, String)> {
        let mut out: Vec<_> = self.commands.iter().map(|c| (0, 0, c.clone())).collect();
        for (t, tab) in self.window.iter().flat_map(|w| w.tabs.iter()).enumerate() {
            for (p, pane) in tab.panes.iter().enumerate() {
                out.extend(pane.commands.iter().map(|c| (t, p, c.clone())));
            }
        }
        out
    }

    /// A line for menus and lists: scope and what's in it.
    pub fn summary(&self) -> String {
        let commands = self.all_commands().len();
        let mut s = self.recipe.scope.label().to_owned();
        if let Some(w) = &self.window {
            let panes: usize = w.tabs.iter().map(|t| t.panes.len()).sum();
            if self.recipe.scope == Scope::Window {
                s.push_str(&format!(" · {} tabs", w.tabs.len()));
            }
            s.push_str(&format!(
                " · {panes} pane{}",
                if panes == 1 { "" } else { "s" }
            ));
        }
        if let Some(snippet) = &self.snippet {
            s.push_str(&format!(" · {}", snippet.alias));
        }
        if commands > 0 {
            s.push_str(&format!(
                " · {commands} command{}",
                if commands == 1 { "" } else { "s" }
            ));
        }
        s
    }
}

/// Where portable paths point when the recipe opens.
#[derive(Clone, Debug, Default)]
pub struct Bases {
    pub current: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// The folder holding the recipe file.
    pub recipe: Option<PathBuf>,
}

impl Bases {
    pub fn new(current: Option<PathBuf>, recipe_file: Option<&Path>) -> Self {
        Self {
            current,
            home: dirs::home_dir(),
            recipe: recipe_file.and_then(Path::parent).map(Path::to_path_buf),
        }
    }

    fn expand(&self, path: &str) -> PathBuf {
        for (var, base) in [
            ("{{current_folder}}", &self.current),
            ("{{home}}", &self.home),
            ("{{recipe_folder}}", &self.recipe),
        ] {
            if let Some(rest) = path.strip_prefix(var)
                && let Some(base) = base
            {
                return base.join(rest.trim_start_matches('/'));
            }
        }
        match path.strip_prefix("~/") {
            Some(rest) => self.home.clone().unwrap_or_default().join(rest),
            None => PathBuf::from(path),
        }
    }
}

/// Which variable a saved path's prefix becomes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Portable {
    #[default]
    Off,
    CurrentFolder,
    Home,
    RecipeFolder,
}

impl Portable {
    fn shorten(self, path: &Path, bases: &Bases) -> String {
        let (var, base) = match self {
            Portable::Off => return path.display().to_string(),
            Portable::CurrentFolder => ("{{current_folder}}", &bases.current),
            Portable::Home => ("{{home}}", &bases.home),
            Portable::RecipeFolder => ("{{recipe_folder}}", &bases.recipe),
        };
        match base.as_deref().and_then(|b| path.strip_prefix(b).ok()) {
            Some(rest) if rest.as_os_str().is_empty() => var.to_owned(),
            Some(rest) => format!("{var}/{}", rest.display()),
            None => path.display().to_string(),
        }
    }
}

/// A tab's layout from its saved state, with each pane's commands (left
/// to right, as the leaves go).
pub fn tab_layout(
    state: &TabState,
    title: Option<String>,
    commands: &[Vec<String>],
    portable: Portable,
    bases: &Bases,
) -> TabLayout {
    fn walk(pane: &PaneState, split: Option<(String, f32, usize)>, out: &mut Vec<PaneLayout>) {
        match pane {
            PaneState::Leaf { cwd, .. } => out.push(PaneLayout {
                cwd: cwd.as_ref().map(|c| c.display().to_string()),
                split: split.as_ref().map(|s| s.0.clone()),
                size: split.as_ref().map(|s| (s.1 * 100.).round() / 100.),
                of: split.and_then(|s| (s.2 > 1).then_some(s.2)),
                commands: Vec::new(),
            }),
            PaneState::Split {
                vertical,
                ratio,
                first,
                second,
            } => {
                let before = out.len();
                walk(first, split, out);
                let covered = out.len() - before;
                let dir = if *vertical { "down" } else { "right" };
                walk(second, Some((dir.into(), 1. - ratio, covered)), out);
            }
        }
    }
    let mut panes = Vec::new();
    walk(&state.root, None, &mut panes);
    for (pane, commands) in panes.iter_mut().zip(commands) {
        pane.commands = commands.clone();
        if let Some(cwd) = &pane.cwd {
            pane.cwd = Some(portable.shorten(Path::new(cwd), bases));
        }
    }
    TabLayout { title, panes }
}

/// A tab to open from `layout`, and each leaf's commands.
pub fn tab_state(layout: &TabLayout, bases: &Bases) -> (TabState, Vec<Vec<String>>) {
    // Leaves in the order made; each new one splits the subtree holding
    // the last `of` of them.
    let leaf = |p: &PaneLayout| PaneState::Leaf {
        cwd: p.cwd.as_deref().map(|c| bases.expand(c)),
        scrollback: None,
        agent: None,
        queue: Vec::new(),
    };
    let first = layout.panes.first().map(leaf).unwrap_or(PaneState::Leaf {
        cwd: None,
        scrollback: None,
        agent: None,
        queue: Vec::new(),
    });
    let mut root = first;
    for (made, pane) in (1..).zip(layout.panes.iter().skip(1)) {
        let of = pane.of.unwrap_or(1).clamp(1, made);
        let dir = pane.split.as_deref().unwrap_or("right");
        let size = pane.size.unwrap_or(0.5).clamp(0.05, 0.95);
        let new = leaf(pane);
        split_last(&mut root, made, of, dir, size, new);
    }
    let state = TabState {
        root,
        focused: 0,
        zoomed: false,
        divider: false,
        divider_label: String::new(),
        name: layout.title.clone().filter(|t| !t.is_empty()),
        name_prefix: false,
    };
    (
        state,
        layout.panes.iter().map(|p| p.commands.clone()).collect(),
    )
}

fn leaf_count(p: &PaneState) -> usize {
    match p {
        PaneState::Leaf { .. } => 1,
        PaneState::Split { first, second, .. } => leaf_count(first) + leaf_count(second),
    }
}

/// Splits the smallest subtree of `node` (which holds `total` leaves)
/// that ends with its last leaf and holds at least `of` leaves.
fn split_last(node: &mut PaneState, total: usize, of: usize, dir: &str, size: f32, new: PaneState) {
    if total > of
        && let PaneState::Split { second, .. } = node
    {
        let n = leaf_count(second);
        if n >= of {
            return split_last(second, n, of, dir, size, new);
        }
    }
    let old = std::mem::replace(
        node,
        PaneState::Leaf {
            cwd: None,
            scrollback: None,
            agent: None,
            queue: Vec::new(),
        },
    );
    let vertical = matches!(dir, "down" | "up");
    let (first, second, ratio) = if matches!(dir, "left" | "up") {
        (new, old, size)
    } else {
        (old, new, 1. - size)
    };
    *node = PaneState::Split {
        vertical,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    };
}

// Saved recipes

pub fn dir() -> PathBuf {
    motty_theme::config::motty_dir().join("recipes")
}

/// A saved recipe and its id (the file's stem).
#[derive(Clone, Debug)]
pub struct Saved {
    pub id: String,
    pub recipe: Recipe,
    pub updated: std::time::SystemTime,
}

/// Saved recipes, most recently updated first.
pub fn list() -> Vec<Saved> {
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return Vec::new();
    };
    let mut out: Vec<Saved> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| {
            let path = e.path();
            let recipe = Recipe::parse(&std::fs::read_to_string(&path).ok()?).ok()?;
            Some(Saved {
                id: path.file_stem()?.to_string_lossy().into_owned(),
                recipe,
                updated: e.metadata().and_then(|m| m.modified()).ok()?,
            })
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.updated));
    out
}

/// Finds a saved recipe by id, or by name ignoring case.
pub fn find(key: &str) -> Option<Saved> {
    let all = list();
    all.iter()
        .find(|s| s.id == key)
        .or_else(|| {
            all.iter()
                .find(|s| s.recipe.recipe.name.eq_ignore_ascii_case(key))
        })
        .cloned()
}

/// Saves `recipe` under a new id made from its name; returns the id.
pub fn save(recipe: &Recipe) -> std::io::Result<String> {
    let dir = dir();
    std::fs::create_dir_all(&dir)?;
    let base = slug(&recipe.recipe.name);
    let mut id = base.clone();
    let mut n = 2;
    while dir.join(format!("{id}.toml")).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    std::fs::write(dir.join(format!("{id}.toml")), recipe.to_toml())?;
    forget_snippets();
    Ok(id)
}

pub fn delete(id: &str) -> std::io::Result<()> {
    std::fs::remove_file(dir().join(format!("{id}.toml")))?;
    forget_snippets();
    Ok(())
}

fn slug(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    if s.is_empty() {
        "recipe".into()
    } else {
        s.into()
    }
}

// Snippets

fn snippet_cache() -> &'static Mutex<Option<HashMap<String, String>>> {
    static CACHE: OnceLock<Mutex<Option<HashMap<String, String>>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn forget_snippets() {
    if let Ok(mut cache) = snippet_cache().lock() {
        *cache = None;
    }
}

/// The snippet `alias` expands to, if there's one.
pub fn snippet_for(alias: &str) -> Option<String> {
    let mut cache = snippet_cache().lock().ok()?;
    let map = cache.get_or_insert_with(|| {
        list()
            .into_iter()
            .filter_map(|s| s.recipe.snippet)
            .map(|s| (s.alias, s.text))
            .collect()
    });
    map.get(alias).cloned()
}

/// A snippet's text with `{{clipboard}}`, `{{date}}` and `{{time}}`
/// filled in, and how far the cursor goes back for `{{cursor}}` (in
/// characters from the end).
pub fn expand_snippet(text: &str, clipboard: &str, now: (String, String)) -> (String, usize) {
    let (date, time) = now;
    let filled = text
        .replace("{{clipboard}}", clipboard)
        .replace("{{date}}", &date)
        .replace("{{time}}", &time);
    match filled.split_once("{{cursor}}") {
        Some((before, after)) => {
            let after = after.replace("{{cursor}}", "");
            (format!("{before}{after}"), after.chars().count())
        }
        None => (filled, 0),
    }
}

/// Today as `YYYY-MM-DD` and now as `HH:mm`, local time.
pub fn date_time() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as libc::time_t);
    // SAFETY: localtime_r writes into the struct we pass.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    };
    (
        format!(
            "{}-{:02}-{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday
        ),
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min),
    )
}

// Trusted recipe files

fn trust_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Motty/trusted_recipes.json")
}

pub fn hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn is_trusted(hash: &str) -> bool {
    std::fs::read_to_string(trust_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .is_some_and(|list| list.iter().any(|h| h == hash))
}

pub fn trust(hash: &str) {
    let path = trust_path();
    let mut list: Vec<String> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if !list.iter().any(|h| h == hash) {
        list.push(hash.to_owned());
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(
        &path,
        serde_json::to_string_pretty(&list).unwrap_or_default(),
    ) {
        tracing::warn!("couldn't save trusted recipes: {e}");
    }
}

/// Commands that hand the keyboard to another shell: replay waits after
/// one of these.
pub fn hands_off(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    let first = words.first().copied().unwrap_or_default();
    let first = first.rsplit('/').next().unwrap_or(first);
    match first {
        "ssh" | "mosh" | "telnet" | "su" | "nix-shell" => true,
        "sudo" => words.iter().any(|w| matches!(*w, "-i" | "-s" | "su")),
        "poetry" | "pipenv" => words.get(1) == Some(&"shell"),
        "tmux" | "screen" | "zellij" => {
            words.len() == 1
                || words
                    .iter()
                    .any(|w| matches!(*w, "attach" | "attach-session" | "a" | "-r" | "new"))
        }
        "docker" | "podman" | "kubectl" => {
            (words.contains(&"exec") || words.contains(&"run"))
                && words.iter().any(|w| matches!(*w, "-it" | "-ti" | "-i"))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OTTY: &str = r#"
[recipe]
name = "deploy-prod-debug"
version = 1
scope = "window"

[[window.tabs]]
title = "API"
[[window.tabs.panes]]
cwd = "{{current_folder}}/api"
commands = ["tail -F log/prod.log"]
[[window.tabs.panes]]
cwd = "{{current_folder}}/api"
split = "right"
size = 0.5
commands = ["make deploy"]

[[window.tabs]]
title = "Web"
[[window.tabs.panes]]
cwd = "{{current_folder}}/web"
commands = ["npm run preview"]
"#;

    fn bases() -> Bases {
        Bases {
            current: Some("/work".into()),
            home: Some("/Users/me".into()),
            recipe: None,
        }
    }

    #[test]
    fn reads_otty_files() {
        let r = Recipe::parse(OTTY).unwrap();
        assert_eq!(r.recipe.scope, Scope::Window);
        assert_eq!(r.all_commands().len(), 3);
        let (state, commands) = tab_state(&r.window.as_ref().unwrap().tabs[0], &bases());
        assert_eq!(state.name.as_deref(), Some("API"));
        assert_eq!(commands[1], vec!["make deploy".to_owned()]);
        let PaneState::Split {
            vertical, first, ..
        } = &state.root
        else {
            panic!("not split")
        };
        assert!(!vertical);
        let PaneState::Leaf { cwd, .. } = first.as_ref() else {
            panic!()
        };
        assert_eq!(cwd.as_deref(), Some(Path::new("/work/api")));
        assert!(Recipe::parse("[recipe]\nname='x'\nscope='tab'\n").is_err());
        let under = Recipe::parse("[recipe]\nname='x'\nscope='commands'\ncommands=['ls']\n");
        assert_eq!(under.unwrap().commands, vec!["ls".to_owned()]);
    }

    #[test]
    fn layouts_round_trip() {
        let leaf = |cwd: &str| PaneState::Leaf {
            cwd: Some(cwd.into()),
            scrollback: None,
            agent: None,
            queue: Vec::new(),
        };
        // (A | B) over C, then C split right into C | D.
        let root = PaneState::Split {
            vertical: true,
            ratio: 0.75,
            first: Box::new(PaneState::Split {
                vertical: false,
                ratio: 0.5,
                first: Box::new(leaf("/Users/me/a")),
                second: Box::new(leaf("/Users/me/b")),
            }),
            second: Box::new(PaneState::Split {
                vertical: false,
                ratio: 0.25,
                first: Box::new(leaf("/Users/me/c")),
                second: Box::new(leaf("/Users/me/d")),
            }),
        };
        let state = TabState {
            root,
            focused: 0,
            zoomed: false,
            divider: false,
            divider_label: String::new(),
            name: None,
            name_prefix: false,
        };
        let commands = vec![vec![], vec!["ls".into()], vec![], vec![]];
        let layout = tab_layout(&state, None, &commands, Portable::Home, &bases());
        assert_eq!(layout.panes[0].cwd.as_deref(), Some("{{home}}/a"));
        assert_eq!(layout.panes[2].of, Some(2));
        assert_eq!(layout.panes[2].split.as_deref(), Some("down"));
        let (back, cmds) = tab_state(&layout, &bases());
        assert_eq!(back.root, state.root);
        assert_eq!(cmds, commands);
    }

    #[test]
    fn snippets_and_handoffs() {
        let now = ("2026-10-29".to_owned(), "14:05".to_owned());
        assert_eq!(
            expand_snippet("git checkout {{cursor}}", "", now.clone()),
            ("git checkout ".into(), 0)
        );
        assert_eq!(
            expand_snippet("echo {{cursor}} {{date}}", "", now.clone()),
            ("echo  2026-10-29".into(), 11)
        );
        assert_eq!(
            expand_snippet("{{clipboard}} at {{time}}", "x", now).0,
            "x at 14:05"
        );
        assert!(hands_off("ssh prod"));
        assert!(hands_off("tmux attach -t main"));
        assert!(hands_off("docker exec -it web sh"));
        assert!(!hands_off("docker ps"));
        assert!(!hands_off("sudo make install"));
        assert!(!hands_off("make deploy"));
        assert_eq!(slug("Dev workspace!"), "dev-workspace");
    }
}
