//! Commands run at a prompt, remembered per folder for inline suggestions.
//!
//! Each finished command an integrated shell reports is kept with the
//! folder it ran in, ranked by frecency (how often, weighted by how
//! recently). Typing at a prompt then suggests the rest of the line: the
//! folder's own commands first, any folder's when it has none.
//!
//! Commands that turn out not to exist (exit 127) aren't kept, nor ones
//! typed with a leading space (shells' "don't remember this" convention,
//! which Motty's own `cd` and `ssh` typing also uses).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use motty_theme::Autocomplete;
use serde::{Deserialize, Serialize};

/// Least-used entries go past this many.
const MAX_ENTRIES: usize = 5_000;
/// A suggestion is shown whole only when it scores this much above the next.
const LEAD: f64 = 1.5;
/// Lines longer than this aren't worth suggesting.
const MAX_LEN: usize = 1_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    /// Empty when the folder wasn't known.
    cwd: String,
    command: String,
    count: f64,
    /// Unix seconds.
    last: u64,
}

static DB: Mutex<Option<Vec<Entry>>> = Mutex::new(None);

fn file() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Motty/history.json")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn with_db<T>(f: impl FnOnce(&mut Vec<Entry>) -> T) -> T {
    let mut db = DB.lock().unwrap_or_else(|e| e.into_inner());
    let db = db.get_or_insert_with(|| {
        std::fs::read(file())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    });
    f(db)
}

fn cwd_key(cwd: Option<&Path>) -> String {
    cwd.map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether `command` should be remembered at all.
pub fn worth_keeping(command: &str, exit: Option<i32>, options: &Autocomplete) -> bool {
    let trimmed = command.trim();
    options.learn
        && !trimmed.is_empty()
        && trimmed.len() <= MAX_LEN
        && !command.starts_with(' ')
        && exit != Some(127)
        && !(options.skip_secrets && looks_secret(trimmed))
        && !options
            .history_ignore
            .iter()
            .any(|glob| glob_match(glob, trimmed))
}

/// Remembers `command`, run in `cwd`.
pub fn record(cwd: Option<&Path>, command: &str, exit: Option<i32>, options: &Autocomplete) {
    if !worth_keeping(command, exit, options) {
        // A command that's gone (or now ignored) stops being suggested.
        if options.learn && !command.trim().is_empty() {
            let command = command.trim();
            with_db(|db| db.retain(|e| e.command != command));
        }
        return;
    }
    let command = command.trim().to_owned();
    let cwd = cwd_key(cwd);
    let json = with_db(|db| {
        let now = now();
        match db.iter_mut().find(|e| e.cwd == cwd && e.command == command) {
            Some(e) => {
                e.count += 1.;
                e.last = now;
            }
            None => db.push(Entry {
                cwd,
                command,
                count: 1.,
                last: now,
            }),
        }
        if db.len() > MAX_ENTRIES {
            db.sort_by(|a, b| frecency(b, now).total_cmp(&frecency(a, now)));
            db.truncate(MAX_ENTRIES * 9 / 10);
        }
        serde_json::to_vec(db)
    });
    let result = json.map_err(std::io::Error::other).and_then(|json| {
        let path = file();
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)
    });
    if let Err(e) = result {
        tracing::warn!("couldn't save command history: {e}");
    }
}

/// Forgets every remembered command.
pub fn clear() {
    with_db(Vec::clear);
    if let Err(e) = std::fs::remove_file(file())
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("couldn't remove the command history: {e}");
    }
}

fn frecency(entry: &Entry, now: u64) -> f64 {
    let age = now.saturating_sub(entry.last);
    let weight = match age {
        0..3_600 => 4.,
        3_600..86_400 => 2.,
        86_400..604_800 => 0.5,
        _ => 0.25,
    };
    entry.count * weight
}

/// The rest of the line to suggest after `typed`, if one clearly leads.
pub fn suggest(cwd: Option<&Path>, typed: &str) -> Option<String> {
    if typed.trim().is_empty() {
        return None;
    }
    let cwd = cwd_key(cwd);
    let now = now();
    let ranked = with_db(|db| {
        let here: Vec<(&str, f64)> = db
            .iter()
            .filter(|e| e.cwd == cwd && e.command.len() > typed.len())
            .filter(|e| e.command.starts_with(typed))
            .map(|e| (e.command.as_str(), frecency(e, now)))
            .collect();
        let mut ranked: Vec<(String, f64)> = if here.is_empty() {
            let mut all: HashMap<&str, f64> = HashMap::new();
            for e in db
                .iter()
                .filter(|e| e.command.len() > typed.len() && e.command.starts_with(typed))
            {
                *all.entry(&e.command).or_default() += frecency(e, now);
            }
            all.into_iter().map(|(c, s)| (c.to_owned(), s)).collect()
        } else {
            here.into_iter().map(|(c, s)| (c.to_owned(), s)).collect()
        };
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked
    });
    lead(typed, &ranked)
}

/// The whole rest of the leader when it's far enough ahead; otherwise the
/// next words the close contenders share.
fn lead(typed: &str, ranked: &[(String, f64)]) -> Option<String> {
    let (top, score) = ranked.first()?;
    let rest = |c: &str| c[typed.len()..].to_owned();
    if ranked.get(1).is_none_or(|(_, s)| *score >= s * LEAD) {
        return Some(rest(top));
    }
    let close: Vec<String> = ranked
        .iter()
        .take_while(|(_, s)| s * LEAD > *score)
        .map(|(c, _)| rest(c))
        .collect();
    let first: Vec<char> = close[0].chars().collect();
    let mut shared = first.len();
    for other in &close[1..] {
        shared = shared.min(
            first
                .iter()
                .zip(other.chars())
                .take_while(|(a, b)| **a == *b)
                .count(),
        );
    }
    // Back off to where every contender's word ends.
    let ends_word = |k: usize| {
        close
            .iter()
            .all(|c| c.chars().nth(k).is_none_or(char::is_whitespace))
    };
    let k = (1..=shared).rev().find(|&k| ends_word(k))?;
    let words: String = first[..k].iter().collect();
    (!words.trim().is_empty()).then_some(words)
}

/// A command that seems to carry a password, token or key.
fn looks_secret(command: &str) -> bool {
    let lower = command.to_lowercase();
    const WORDS: [&str; 9] = [
        "password",
        "passwd",
        "token",
        "secret",
        "api_key",
        "apikey",
        "api-key",
        "bearer ",
        "private_key",
    ];
    const PREFIXES: [&str; 7] = [
        "sk-",
        "ghp_",
        "gho_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "akia",
    ];
    WORDS.iter().any(|w| lower.contains(w))
        || lower
            .split(|c: char| c.is_whitespace() || c == '=' || c == '"' || c == '\'')
            .any(|word| word.len() >= 16 && PREFIXES.iter().any(|p| word.starts_with(p)))
}

/// `*` (any run) and `?` (any one character) glob matching.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
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

    fn ranked(list: &[(&str, f64)]) -> Vec<(String, f64)> {
        list.iter().map(|(c, s)| (c.to_string(), *s)).collect()
    }

    #[test]
    fn a_clear_leader_is_suggested_whole() {
        let r = ranked(&[("cargo test -p motty-app", 8.), ("cargo build", 2.)]);
        assert_eq!(lead("cargo t", &r).as_deref(), Some("est -p motty-app"));
    }

    #[test]
    fn close_contenders_share_only_their_next_word() {
        let r = ranked(&[("nvs 22 && npx install-electron", 5.), ("nvs 22", 4.)]);
        assert_eq!(lead("nvs ", &r).as_deref(), Some("22"));
        let r = ranked(&[("git status", 3.), ("git stash", 3.)]);
        assert_eq!(lead("git s", &r), None);
    }

    #[test]
    fn what_is_kept() {
        let opts = Autocomplete::default();
        assert!(worth_keeping("ls -la", Some(0), &opts));
        assert!(worth_keeping("npm run dev", Some(130), &opts));
        assert!(!worth_keeping("lss", Some(127), &opts));
        assert!(!worth_keeping(" cd -- '/tmp'", Some(0), &opts));
        let opts = Autocomplete {
            skip_secrets: true,
            history_ignore: vec!["ssh *".into(), "export *TOKEN*".into()],
            ..Autocomplete::default()
        };
        assert!(!worth_keeping("ssh prod", Some(0), &opts));
        assert!(!worth_keeping("export GH_TOKEN=abc", Some(0), &opts));
        assert!(!worth_keeping(
            "curl -H 'Authorization: Bearer x' api",
            Some(0),
            &opts
        ));
        assert!(!worth_keeping(
            "tool --key sk-abcdefghijklmnopqrstu",
            Some(0),
            &opts
        ));
        assert!(worth_keeping("git push", Some(0), &opts));
    }

    #[test]
    fn globs() {
        assert!(glob_match("ssh *", "ssh host"));
        assert!(!glob_match("ssh *", "sshd"));
        assert!(glob_match("*TOKEN*", "export TOKEN=1"));
        assert!(glob_match("a?c", "abc"));
    }
}
