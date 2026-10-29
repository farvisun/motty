//! The config files on disk: config.toml, config.local.toml beside it and
//! the files config.toml includes. Which one a key comes from, and edits
//! that leave everything else in them as it was: comments, blank lines and
//! order survive, a key is updated where it sits, a new one is appended, a
//! key under an alias is rewritten under its real name, and a reset key is
//! commented out as `# key = value (reset to default)`.

use std::path::{Path, PathBuf};

use toml::Value;

use crate::config::{Config, layered, motty_dir, otty_dir, parse_entries};
use crate::schema::{self, List};

/// Points Motty (the app and the CLI) at another config.toml.
pub const CONFIG_FILE_ENV: &str = "MOTTY_CONFIG_FILE";

const RESET: &str = "(reset to default)";

/// config.toml and its config.local.toml.
#[derive(Clone, Debug)]
pub struct Files {
    pub main: PathBuf,
    /// Settings for this machine only: never synced, and its keys win.
    pub local: PathBuf,
    /// Otty's config.toml, read as-is while Motty has none of its own.
    fallback: Option<PathBuf>,
}

/// Where a key's value comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// config.local.toml (a value, or a `(reset to default)` line).
    Local(PathBuf, usize),
    Main(PathBuf, usize),
    Include(PathBuf, usize),
    Default,
}

impl Origin {
    /// `path:line`, or `default`.
    pub fn describe(&self) -> String {
        match self {
            Origin::Local(p, n) | Origin::Main(p, n) | Origin::Include(p, n) => {
                format!("{}:{n}", p.display())
            }
            Origin::Default => "default".into(),
        }
    }
}

impl Files {
    /// `$MOTTY_CONFIG_FILE`, else `~/.config/motty/config.toml` (reading
    /// Otty's while Motty has none).
    pub fn current() -> Self {
        match std::env::var_os(CONFIG_FILE_ENV).filter(|v| !v.is_empty()) {
            Some(path) => Self::at(PathBuf::from(path)),
            None => Self {
                fallback: Some(otty_dir().join("config.toml")),
                ..Self::at(motty_dir().join("config.toml"))
            },
        }
    }

    /// The config.toml at `main`, with config.local.toml beside it.
    pub fn at(main: PathBuf) -> Self {
        Self {
            local: main.with_file_name("config.local.toml"),
            main,
            fallback: None,
        }
    }

    /// The config.toml being read and its text: Motty's, else Otty's.
    pub fn source(&self) -> Option<(PathBuf, String)> {
        if let Ok(src) = std::fs::read_to_string(&self.main) {
            return Some((self.main.clone(), src));
        }
        let fallback = self.fallback.as_ref()?;
        let src = std::fs::read_to_string(fallback).ok()?;
        Some((fallback.clone(), src))
    }

    /// The config.local.toml being read: the one beside [`Self::source`].
    fn local_source(&self) -> (PathBuf, String) {
        let path = match self.source() {
            Some((main, _)) if main != self.main => main.with_file_name("config.local.toml"),
            _ => self.local.clone(),
        };
        let src = std::fs::read_to_string(&path).unwrap_or_default();
        (path, src)
    }

    fn dir(path: &Path) -> &Path {
        path.parent().unwrap_or(Path::new("."))
    }

    /// Every entry in effect, in order (includes, config.toml, then
    /// config.local.toml), with what couldn't be read.
    pub fn entries(&self) -> (Vec<(String, Value)>, Vec<String>) {
        let (path, main) = self
            .source()
            .unwrap_or_else(|| (self.main.clone(), String::new()));
        let (_, local) = self.local_source();
        layered(&main, &local, Self::dir(&path), &|p: &Path| {
            std::fs::read_to_string(p).ok()
        })
    }

    /// The config, with `overrides` (from `motty config set --transient`)
    /// in place of the files' entries for the same keys.
    pub fn load(&self, overrides: &[(String, Value)]) -> Config {
        let (mut entries, warnings) = self.entries();
        overlay(&mut entries, overrides);
        let mut config = Config::from_entries((entries, warnings));
        config.path = self.source().map(|(path, _)| path);
        config
    }

    /// The files config.toml includes, in order.
    pub fn includes(&self) -> Vec<PathBuf> {
        let Some((path, src)) = self.source() else {
            return Vec::new();
        };
        parse_entries(&src)
            .0
            .into_iter()
            .filter(|(k, _)| k == "include")
            .filter_map(|(_, v)| v.as_str().map(|p| Self::dir(&path).join(p)))
            .collect()
    }

    /// `key`'s entries in effect: the last value for a single one, every
    /// entry of a list (an inline list's array opened up).
    pub fn values(&self, key: &str) -> Vec<Value> {
        let key = schema::canonical(key);
        let values: Vec<Value> = self
            .entries()
            .0
            .into_iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v)
            .collect();
        list_entries(key, values)
    }

    /// Where `key`'s value comes from.
    pub fn origin(&self, key: &str) -> Origin {
        let key = schema::canonical(key);
        let (local_path, local) = self.local_source();
        if let Some(n) = last_line(&local, key, true) {
            return Origin::Local(local_path, n + 1);
        }
        if let Some((path, src)) = self.source()
            && let Some(n) = last_line(&src, key, false)
        {
            return Origin::Main(path, n + 1);
        }
        for path in self.includes().into_iter().rev() {
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            if let Some(n) = last_line(&src, key, false) {
                return Origin::Include(path, n + 1);
            }
        }
        Origin::Default
    }

    /// Whether writes of `key` go to config.local.toml: its value (or its
    /// reset) already comes from there.
    pub fn is_local(&self, key: &str) -> bool {
        let src = std::fs::read_to_string(&self.local).unwrap_or_default();
        last_line(&src, schema::canonical(key), true).is_some()
    }

    /// Rewrites config.toml through `f`, starting from Otty's (or a header)
    /// when there's none yet.
    pub fn edit_main(&self, f: impl FnOnce(&str) -> String) -> std::io::Result<PathBuf> {
        let src = match std::fs::read_to_string(&self.main) {
            Ok(src) => src,
            Err(_) => match self.source() {
                Some((_, otty)) => format!("# Started from Otty's config.toml.\n{otty}"),
                None => "# Motty settings: https://docs.otty.sh/reference/configuration\n".into(),
            },
        };
        write(&self.main, &f(&src))?;
        Ok(self.main.clone())
    }

    /// Rewrites config.local.toml through `f` (creating it).
    pub fn edit_local(&self, f: impl FnOnce(&str) -> String) -> std::io::Result<PathBuf> {
        let src = std::fs::read_to_string(&self.local).unwrap_or_default();
        write(&self.local, &f(&src))?;
        Ok(self.local.clone())
    }

    /// Rewrites the file `key` is written to (see [`Self::is_local`]).
    pub fn edit_key(&self, key: &str, f: impl FnOnce(&str) -> String) -> std::io::Result<PathBuf> {
        if self.is_local(key) {
            self.edit_local(f)
        } else {
            self.edit_main(f)
        }
    }

    /// Sets `key` to `values` (one value, or a list's entries) in
    /// config.local.toml when `local`, else in the file it comes from.
    pub fn set(&self, key: &str, values: &[Value], local: bool) -> std::io::Result<PathBuf> {
        let key = schema::canonical(key).to_owned();
        let edit = |src: &str| set_values_in(src, &key, values);
        if local || self.is_local(&key) {
            self.edit_local(edit)
        } else {
            self.edit_main(edit)
        }
    }

    /// Takes `key` out of config.local.toml (so config.toml's value applies
    /// again) and, unless `local_only`, resets it in config.toml too.
    pub fn unset(&self, key: &str, local_only: bool) -> std::io::Result<()> {
        let key = schema::canonical(key);
        if std::fs::read_to_string(&self.local).is_ok() {
            self.edit_local(|src| remove_key_in(src, key))?;
        }
        if !local_only && self.source().is_some() {
            self.edit_main(|src| reset_in(src, key))?;
        }
        Ok(())
    }

    /// Settings' writes: each key set, or reset for `None`, in the file its
    /// value comes from (a local reset stays local, as a reset line).
    pub fn save(&self, changes: &[(&str, Option<Value>)]) -> std::io::Result<PathBuf> {
        let (local, main): (Vec<_>, Vec<_>) =
            changes.iter().partition(|(key, _)| self.is_local(key));
        let apply = |src: &str, changes: &[&(&str, Option<Value>)]| {
            let mut src = src.to_owned();
            for (key, value) in changes {
                src = match value {
                    Some(value) => set_in(&src, key, value),
                    None => reset_in(&src, key),
                };
            }
            src
        };
        if !local.is_empty() {
            self.edit_local(|src| apply(src, &local))?;
        }
        // Always written, so a first save creates config.toml.
        self.edit_main(|src| apply(src, &main))
    }
}

/// Something wrong in a config file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub path: PathBuf,
    /// 1-based.
    pub line: usize,
    pub message: String,
}

impl Files {
    /// Unknown keys, unreadable lines and values a key doesn't take, in
    /// the included files, config.toml and config.local.toml.
    pub fn validate(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        for path in self.includes() {
            match std::fs::read_to_string(&path) {
                Ok(src) => out.extend(validate_src(&path, &src)),
                Err(e) => out.push(Problem {
                    path: path.clone(),
                    line: 0,
                    message: format!("can't read the included file: {e}"),
                }),
            }
        }
        if let Some((path, src)) = self.source() {
            out.extend(validate_src(&path, &src));
        }
        let (path, src) = self.local_source();
        out.extend(validate_src(&path, &src));
        out
    }
}

/// [`Files::validate`] for one file's text.
pub fn validate_src(path: &Path, src: &str) -> Vec<Problem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut problem = |line: usize, message: String| {
        out.push(Problem {
            path: path.to_owned(),
            line: line + 1,
            message,
        })
    };
    // Lines that aren't `key = value` at all.
    let stmts = statements(&lines);
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        let inside = stmts.iter().any(|s| (s.start..s.end).contains(&i));
        if !inside && !t.is_empty() && !t.starts_with('#') {
            problem(i, format!("can't read {t:?}"));
        }
    }
    for stmt in stmts {
        let Some(spec) = schema::lookup(&stmt.key) else {
            problem(stmt.start, format!("unknown key {:?}", stmt.key));
            continue;
        };
        let text = lines[stmt.start..stmt.end].join("\n");
        let (entries, warnings) = parse_entries(&text);
        for w in warnings {
            let message = w.split_once(": ").map_or(w.as_str(), |(_, m)| m);
            problem(stmt.start, format!("{}: {message}", spec.key));
        }
        for (_, value) in entries {
            if let Err(e) = schema::check(spec, &value) {
                problem(stmt.start, format!("{}: {e}", spec.key));
            }
        }
    }
    out
}

/// Puts `overrides` in place of `entries`' values for the same keys.
fn overlay(entries: &mut Vec<(String, Value)>, overrides: &[(String, Value)]) {
    if overrides.is_empty() {
        return;
    }
    entries.retain(|(k, _)| !overrides.iter().any(|(o, _)| o == k));
    entries.extend(overrides.iter().cloned());
}

/// `values` read for `key` as its entries (see [`Files::values`]).
pub fn list_entries(key: &str, values: Vec<Value>) -> Vec<Value> {
    let split = |v: Value| -> Vec<Value> {
        match v {
            Value::Array(items) => items,
            Value::String(s) => s
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| Value::String(s.to_owned()))
                .collect(),
            other => vec![other],
        }
    };
    match schema::lookup(key).and_then(|k| k.list) {
        Some(List::Repeated) if key == "autocomplete-history-ignore" => {
            values.into_iter().flat_map(split).collect()
        }
        Some(List::Repeated) => values,
        Some(List::Inline) => values.into_iter().last().map(split).unwrap_or_default(),
        None => values.into_iter().last().into_iter().collect(),
    }
}

fn write(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// One `key = value` statement: its real key and lines `start..end`.
#[derive(Debug)]
struct Stmt {
    key: String,
    start: usize,
    end: usize,
}

/// The key a line assigns, as written.
fn raw_key(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.starts_with('#') || t.is_empty() {
        return None;
    }
    let end = t.find(|c: char| c == '=' || c.is_whitespace())?;
    let (key, rest) = t.split_at(end);
    (!key.is_empty() && rest.trim_start().starts_with('=')).then_some(key)
}

/// The statements of `lines`, grouped as the parser groups them (a value
/// may run on while brackets are open).
fn statements(lines: &[&str]) -> Vec<Stmt> {
    let depth = |s: &str| s.matches('[').count() as i32 - s.matches(']').count() as i32;
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(key) = raw_key(lines[i]) else {
            i += 1;
            continue;
        };
        let start = i;
        if key != "keybind" {
            let mut open = depth(lines[i]);
            while open > 0 && i + 1 < lines.len() {
                i += 1;
                open += depth(lines[i]);
            }
        }
        out.push(Stmt {
            key: schema::canonical(key).to_owned(),
            start,
            end: i + 1,
        });
        i += 1;
    }
    out
}

/// The key a `# key = … (reset to default)` line resets.
fn reset_key(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix('#')?.trim();
    if !rest.ends_with(RESET) {
        return None;
    }
    let (key, _) = rest.split_once('=')?;
    Some(schema::canonical(key.trim()).to_owned())
}

/// The last line (0-based) that sets `key`, or with `resets` also resets it.
fn last_line(src: &str, key: &str, resets: bool) -> Option<usize> {
    let lines: Vec<&str> = src.lines().collect();
    let set = statements(&lines)
        .into_iter()
        .filter(|s| s.key == key)
        .map(|s| s.start)
        .next_back();
    let reset = resets
        .then(|| {
            lines
                .iter()
                .rposition(|l| reset_key(l).as_deref() == Some(key))
        })
        .flatten();
    set.max(reset)
}

fn join(lines: Vec<String>) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// `src` with `key = value` set where the last line for it (or an alias of
/// it) was, else where it was reset, else appended. Comments and every
/// other line stay as they were.
pub fn set_in(src: &str, key: &str, value: &Value) -> String {
    let key = schema::canonical(key);
    match schema::lookup(key).and_then(|k| k.list) {
        Some(List::Repeated) => set_values_in(src, key, std::slice::from_ref(value)),
        _ => replace(src, key, vec![schema::line(key, value)], false),
    }
}

/// `src` with `key` holding `values`: one line for a single value or an
/// inline list, one line per entry for a repeated key (placed where its
/// first line was).
pub fn set_values_in(src: &str, key: &str, values: &[Value]) -> String {
    let key = schema::canonical(key);
    match schema::lookup(key).and_then(|k| k.list) {
        Some(List::Repeated) => {
            let lines = values.iter().map(|v| schema::line(key, v)).collect();
            replace(src, key, lines, true)
        }
        Some(List::Inline) => {
            let line = schema::line(key, &Value::Array(values.to_vec()));
            replace(src, key, vec![line], false)
        }
        None => match values.last() {
            Some(value) => replace(src, key, vec![schema::line(key, value)], false),
            None => reset_in(src, key),
        },
    }
}

/// Puts `new` in place of `key`'s statements: all of them for a list
/// (`every`), the last one otherwise.
fn replace(src: &str, key: &str, new: Vec<String>, every: bool) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let stmts: Vec<Stmt> = statements(&lines)
        .into_iter()
        .filter(|s| s.key == key)
        .collect();
    let drop: Vec<&Stmt> = if every {
        stmts.iter().collect()
    } else {
        stmts.last().into_iter().collect()
    };
    let mut out: Vec<String> = Vec::new();
    let mut placed = false;
    let at_reset = drop
        .is_empty()
        .then(|| {
            let resets = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| reset_key(l).as_deref() == Some(key));
            if every {
                resets.map(|(i, _)| i).next()
            } else {
                resets.map(|(i, _)| i).next_back()
            }
        })
        .flatten();
    let mut i = 0;
    while i < lines.len() {
        if let Some(s) = drop.iter().find(|s| s.start == i) {
            if !placed {
                out.extend(new.iter().cloned());
                placed = true;
            }
            i = s.end;
            continue;
        }
        if at_reset == Some(i) {
            out.extend(new.iter().cloned());
            placed = true;
            i += 1;
            continue;
        }
        out.push(lines[i].to_owned());
        i += 1;
    }
    if !placed {
        while out.last().is_some_and(|l| l.trim().is_empty()) {
            out.pop();
        }
        out.extend(new);
    }
    join(out)
}

/// `src` with `key`'s lines commented out as `# key = value (reset to
/// default)`, under its real name, so the old value stays visible.
pub fn reset_in(src: &str, key: &str) -> String {
    let key = schema::canonical(key);
    let lines: Vec<&str> = src.lines().collect();
    let stmts: Vec<Stmt> = statements(&lines)
        .into_iter()
        .filter(|s| s.key == key)
        .collect();
    if stmts.is_empty() {
        return src.to_owned();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match stmts.iter().find(|s| s.start == i) {
            Some(s) => {
                let text: Vec<&str> = lines[s.start..s.end].iter().map(|l| l.trim()).collect();
                let text = text.join(" ");
                let value = text.split_once('=').map_or("", |(_, v)| v.trim());
                out.push(format!("# {key} = {value} {RESET}"));
                i = s.end;
            }
            None => {
                out.push(lines[i].to_owned());
                i += 1;
            }
        }
    }
    join(out)
}

/// `src` without `key`'s lines and reset lines (any spelling).
pub fn remove_key_in(src: &str, key: &str) -> String {
    let key = schema::canonical(key);
    let lines: Vec<&str> = src.lines().collect();
    let stmts: Vec<Stmt> = statements(&lines)
        .into_iter()
        .filter(|s| s.key == key)
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(s) = stmts.iter().find(|s| s.start == i) {
            i = s.end;
            continue;
        }
        if reset_key(lines[i]).as_deref() != Some(key) {
            out.push(lines[i].to_owned());
        }
        i += 1;
    }
    if out.is_empty() {
        return String::new();
    }
    join(out)
}

/// Applies signed edits (`+entry` adds, or replaces the entry of the same
/// name; `-entry` removes by name or whole entry) to `current`. Unsigned
/// `edits` are the whole new list. Mixing the two is an error.
pub fn edit_list(
    spec: &schema::KeySpec,
    current: &[Value],
    edits: &[String],
) -> Result<Vec<Value>, String> {
    let signed = |e: &String| e.starts_with('+') || e.starts_with('-');
    let n = edits.iter().filter(|e| signed(e)).count();
    if n == 0 {
        return edits.iter().map(|e| schema::parse_text(spec, e)).collect();
    }
    if n != edits.len() {
        return Err(
            "sign every entry (+add, -remove) or none; to store a value starting with + or -, use `motty config edit`"
                .into(),
        );
    }
    let text = |v: &Value| schema::display(v);
    let mut out: Vec<Value> = current.to_vec();
    for edit in edits {
        let (sign, entry) = edit.split_at(1);
        let name = spec.entry_name(entry);
        let same = |v: &Value| {
            let t = text(v);
            t == entry || (name.is_some() && spec.entry_name(&t) == name)
        };
        if sign == "+" {
            let value = schema::parse_text(spec, entry)?;
            match out.iter().position(same) {
                Some(i) => out[i] = value,
                None => out.push(value),
            }
        } else {
            // `-EDITOR` removes `EDITOR=nvim`: the name alone matches.
            let before = out.len();
            out.retain(|v| {
                let t = text(v);
                !(t == entry || spec.entry_name(&t).as_deref() == Some(entry) || same(v))
            });
            if out.len() == before {
                return Err(format!("no entry {entry:?} to remove"));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::String(v.into())
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "motty-store-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn set_updates_in_place_and_rewrites_aliases() {
        let src = "# mine\nshell = \"/bin/zsh\" # login\n\nfont-size = 12\n";
        let out = set_in(src, "command", &s("/bin/fish"));
        assert_eq!(out, "# mine\ncommand = \"/bin/fish\"\n\nfont-size = 12\n");
        // Set by its alias: written under the real name.
        let out = set_in("ligatures = false\n", "ligatures", &s("calt"));
        assert_eq!(out, "font-ligatures = \"calt\"\n");
        // A new key goes at the end.
        let out = set_in("theme = \"Nord\"\n\n\n", "font-size", &Value::Float(14.));
        assert_eq!(out, "theme = \"Nord\"\nfont-size = 14.0\n");
    }

    #[test]
    fn reset_comments_out_and_set_brings_it_back_in_place() {
        let src = "theme = \"Nord\"\ncursor-style = \"bar\"\nfont-size = 12\n";
        let out = reset_in(src, "cursor-style");
        assert_eq!(
            out,
            "theme = \"Nord\"\n# cursor-style = \"bar\" (reset to default)\nfont-size = 12\n"
        );
        assert_eq!(Config::parse(&out).cursor_style, crate::CursorStyle::Block);
        let back = set_in(&out, "cursor-style", &s("underline"));
        assert_eq!(
            back,
            "theme = \"Nord\"\ncursor-style = \"underline\"\nfont-size = 12\n"
        );
        // An alias is reset under the real name; arrays fold onto one line.
        let out = reset_in(
            "scrollback = 5\nprogress-bar-commands = [\n  \"a\",\n]\n",
            "scrollback-lines",
        );
        assert_eq!(
            out,
            "# scrollback-lines = 5 (reset to default)\nprogress-bar-commands = [\n  \"a\",\n]\n"
        );
        let out = reset_in(&out, "progress-bar-commands");
        assert!(out.ends_with("# progress-bar-commands = [ \"a\", ] (reset to default)\n"));
    }

    #[test]
    fn lists_are_written_where_they_were() {
        let src = "env = \"A=1\"\ntheme = \"Nord\"\nenv = \"B=2\"\n";
        let out = set_values_in(src, "env", &[s("A=1"), s("C=3")]);
        assert_eq!(out, "env = \"A=1\"\nenv = \"C=3\"\ntheme = \"Nord\"\n");
        let out = set_values_in(&out, "keybind", &[s("cmd+t=new_tab")]);
        assert!(out.ends_with("keybind = cmd+t=new_tab\n"));
        let out = set_values_in("", "font-ligatures", &[s("off")]);
        assert_eq!(out, "font-ligatures = \"off\"\n");
        let out = set_values_in("", "progress-bar-commands", &[s("make")]);
        assert_eq!(out, "progress-bar-commands = [\"make\"]\n");
        let out = set_values_in("", "tab-rule", &[s("path=\"~\" icon=\"home\"")]);
        assert_eq!(out, "tab-rule = 'path=\"~\" icon=\"home\"'\n");
        assert_eq!(Config::parse(&out).tab_rules.len(), 1);
    }

    #[test]
    fn signed_list_edits() {
        let env = schema::lookup("env").unwrap();
        let current = vec![s("EDITOR=vim"), s("NO_PROXY=localhost,127.0.0.1")];
        let out = edit_list(env, &current, &["+EDITOR=nvim".into()]).unwrap();
        assert_eq!(
            out,
            vec![s("EDITOR=nvim"), s("NO_PROXY=localhost,127.0.0.1")]
        );
        let out = edit_list(env, &out, &["+PAGER=less".into(), "-NO_PROXY".into()]).unwrap();
        assert_eq!(out, vec![s("EDITOR=nvim"), s("PAGER=less")]);
        let out = edit_list(env, &out, &["-EDITOR=nvim".into()]).unwrap();
        assert_eq!(out, vec![s("PAGER=less")]);
        assert!(edit_list(env, &out, &["-NOPE".into()]).is_err());
        // Mixed signs are rejected rather than guessed at.
        let err = edit_list(env, &out, &["+A=1".into(), "B=2".into()]).unwrap_err();
        assert!(err.contains("sign every entry"));
        // Unsigned: the whole list, each entry checked.
        let out = edit_list(env, &current, &["X=1".into(), "Y=2".into()]).unwrap();
        assert_eq!(out, vec![s("X=1"), s("Y=2")]);
        assert!(edit_list(env, &current, &["X".into()]).is_err());
        // Unnamed lists match whole entries.
        let quota = schema::lookup("agent-quota-off").unwrap();
        let out = edit_list(quota, &[s("claude")], &["+codex".into(), "+claude".into()]).unwrap();
        assert_eq!(out, vec![s("claude"), s("codex")]);
        // Keybinds are named by their trigger.
        let kb = schema::lookup("keybind").unwrap();
        let out = edit_list(kb, &[s("cmd+t=new_tab")], &["+cmd+t=split_right".into()]).unwrap();
        assert_eq!(out, vec![s("cmd+t=split_right")]);
    }

    #[test]
    fn validation_finds_unknown_keys_and_bad_values() {
        let src = "# fine\ntheme = \"Nord\"\nfont-sise = 12\ncursor-style = \"triangle\"\n\
                   font-size = big\nscrollback = 5\nenv = NOPE\nkeybind = cmd+t=new_tab\n\
                   what is this\ncursor-color = \"#12345\"\nautocomplete-history-ignore = [\"ssh *\"]\n";
        let problems = validate_src(Path::new("c.toml"), src);
        let got: Vec<(usize, &str)> = problems
            .iter()
            .map(|p| (p.line, p.message.as_str()))
            .collect();
        assert_eq!(got.len(), 6, "{got:#?}");
        assert_eq!(got[0], (9, "can't read \"what is this\""));
        assert_eq!(got[1], (3, "unknown key \"font-sise\""));
        assert!(
            got[2].0 == 4
                && got[2]
                    .1
                    .starts_with("cursor-style: \"triangle\" isn't one of block")
        );
        assert!(got[3].0 == 5 && got[3].1.contains("expected a number"));
        assert!(got[4].0 == 7 && got[4].1.contains("NAME=value"));
        assert!(got[5].0 == 10 && got[5].1.starts_with("cursor-color:"));
        assert!(validate_src(Path::new("c.toml"), "").is_empty());
    }

    #[test]
    fn local_file_wins_and_takes_its_own_writes() {
        let dir = temp_dir("local");
        let files = Files::at(dir.join("config.toml"));
        std::fs::write(
            &files.main,
            "# synced\nfont-size = 12\ntheme = \"Nord\"\ninclude = \"extra.toml\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("extra.toml"), "cursor-style = \"bar\"\n").unwrap();
        std::fs::write(
            &files.local,
            "shell = \"/bin/fish\"\n# theme = \"x\" (reset to default)\n",
        )
        .unwrap();

        assert_eq!(
            files.origin("font-size"),
            Origin::Main(files.main.clone(), 2)
        );
        assert_eq!(
            files.origin("command"),
            Origin::Local(files.local.clone(), 1)
        );
        assert_eq!(files.origin("theme"), Origin::Local(files.local.clone(), 2));
        assert_eq!(
            files.origin("cursor-style"),
            Origin::Include(dir.join("extra.toml"), 1)
        );
        assert_eq!(files.origin("copy-on-select"), Origin::Default);
        let config = files.load(&[]);
        assert_eq!(config.shell.as_deref(), Some("/bin/fish"));
        assert_eq!(config.theme, None);
        assert_eq!(config.cursor_style, crate::CursorStyle::Bar);
        assert_eq!(files.values("font-size"), vec![Value::Integer(12)]);

        // A local key's writes stay local; others go to config.toml.
        files.set("command", &[s("/bin/zsh")], false).unwrap();
        files.set("font-size", &[Value::Float(15.)], false).unwrap();
        let local = std::fs::read_to_string(&files.local).unwrap();
        assert!(local.starts_with("command = \"/bin/zsh\"\n"), "{local}");
        let main = std::fs::read_to_string(&files.main).unwrap();
        assert!(main.contains("font-size = 15.0\n") && main.starts_with("# synced\n"));
        // --local moves a key here.
        files.set("cursor-style", &[s("underline")], true).unwrap();
        assert_eq!(
            files.origin("cursor-style"),
            Origin::Local(files.local.clone(), 3)
        );
        // Settings' reset of a local key stays local, as a reset line.
        files.save(&[("command", None)]).unwrap();
        let local = std::fs::read_to_string(&files.local).unwrap();
        assert!(
            local.starts_with("# command = \"/bin/zsh\" (reset to default)\n"),
            "{local}"
        );
        assert!(files.is_local("command"));

        // unset --local: config.toml's value applies again.
        files.unset("theme", true).unwrap();
        assert_eq!(files.load(&[]).theme.as_deref(), Some("Nord"));
        // unset: gone from both files.
        files.unset("font-size", false).unwrap();
        files.unset("cursor-style", false).unwrap();
        let main = std::fs::read_to_string(&files.main).unwrap();
        assert!(
            main.contains("# font-size = 15.0 (reset to default)\n"),
            "{main}"
        );
        assert_eq!(
            files.origin("cursor-style"),
            Origin::Include(dir.join("extra.toml"), 1)
        );
        assert_eq!(files.load(&[]).font_size, None);

        // Transient values replace the files' ones.
        let config = files.load(&[("font-size".into(), Value::Float(20.))]);
        assert_eq!(config.font_size, Some(20.));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
