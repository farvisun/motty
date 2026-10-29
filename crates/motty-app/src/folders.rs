//! Frequent folders: each folder a pane reports (OSC 7, else the shell's
//! cwd) is recorded and ranked by frecency, zoxide's way: visits, weighted
//! by how recent the last one was. Open Quickly's Folders filter lists them.
//!
//! Only the instance that saves the session records, so test and scripted
//! runs leave the list alone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// When the ranks add up past this, they all shrink and stale ones go.
const MAX_TOTAL: f64 = 10_000.;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Entry {
    rank: f64,
    /// Unix seconds.
    last: u64,
}

/// The list, and the file's modification time when it was read or saved:
/// a newer file (`motty import zoxide`) is read again.
type Loaded = (Option<SystemTime>, HashMap<PathBuf, Entry>);

static DB: Mutex<Option<Loaded>> = Mutex::new(None);

fn modified() -> Option<SystemTime> {
    std::fs::metadata(file()).and_then(|m| m.modified()).ok()
}

fn file() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Motty/folders.json")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn with_db<T>(f: impl FnOnce(&mut HashMap<PathBuf, Entry>) -> T) -> T {
    let mut db = DB.lock().unwrap_or_else(|e| e.into_inner());
    let stamp = modified();
    if db.as_ref().is_none_or(|(read, _)| *read != stamp) {
        let list = std::fs::read(file())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        *db = Some((stamp, list));
    }
    let (_, list) = db.as_mut().expect("just filled");
    f(list)
}

/// Counts a visit to `path` (home doesn't count: it's where shells start).
pub fn record(path: &Path) {
    if !crate::recovery::enabled() || dirs::home_dir().as_deref() == Some(path) {
        return;
    }
    visit(path);
}

/// `motty learn`: counts a visit even from a scripted run, and takes the
/// folder off the ignored list.
pub fn learn(path: &Path) {
    let ignored = with_ignored(|list| {
        let before = list.len();
        list.retain(|p| p != path);
        (list.len() != before).then(|| serde_json::to_vec(list))
    });
    if let Some(snapshot) = ignored {
        save_to(&ignored_file(), snapshot);
    }
    visit(path);
}

fn visit(path: &Path) {
    if is_ignored(path) {
        return;
    }
    let snapshot = with_db(|db| {
        let entry = db.entry(path.to_path_buf()).or_default();
        entry.rank += 1.;
        entry.last = now();
        let total: f64 = db.values().map(|e| e.rank).sum();
        if total > MAX_TOTAL {
            db.retain(|_, e| {
                e.rank *= 0.9;
                e.rank >= 1.
            });
        }
        serde_json::to_vec(db)
    });
    save(snapshot);
}

/// Open Quickly's Forget This Folder.
pub fn forget(path: &Path) {
    let snapshot = with_db(|db| {
        db.remove(path);
        serde_json::to_vec(db)
    });
    save(snapshot);
}

/// Folders `motty ignore` took off the list for good: visits to them
/// aren't counted until `motty learn` brings one back.
static IGNORED: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);

fn ignored_file() -> PathBuf {
    file().with_file_name("folders-ignored.json")
}

fn with_ignored<T>(f: impl FnOnce(&mut Vec<PathBuf>) -> T) -> T {
    let mut list = IGNORED.lock().unwrap_or_else(|e| e.into_inner());
    let list = list.get_or_insert_with(|| {
        std::fs::read(ignored_file())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    });
    f(list)
}

fn is_ignored(path: &Path) -> bool {
    with_ignored(|list| list.iter().any(|p| p == path))
}

/// `motty ignore`: forgets the folder and stops counting visits to it.
pub fn ignore(path: &Path) {
    forget(path);
    let snapshot = with_ignored(|list| {
        if !list.iter().any(|p| p == path) {
            list.push(path.to_path_buf());
        }
        serde_json::to_vec(list)
    });
    save_to(&ignored_file(), snapshot);
}

fn save(snapshot: serde_json::Result<Vec<u8>>) {
    save_to(&file(), snapshot);
}

fn save_to(path: &Path, snapshot: serde_json::Result<Vec<u8>>) {
    let result = snapshot.map_err(std::io::Error::other).and_then(|json| {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)
    });
    // Our own write isn't news.
    if result.is_ok()
        && let Some((read, _)) = DB.lock().unwrap_or_else(|e| e.into_inner()).as_mut()
    {
        *read = modified();
    }
    if let Err(e) = result {
        tracing::warn!("couldn't save frequent folders: {e}");
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
    entry.rank * weight
}

/// Folders that still exist, best first.
pub fn ranked() -> Vec<PathBuf> {
    let now = now();
    let mut list: Vec<(PathBuf, f64)> = with_db(|db| {
        db.iter()
            .map(|(path, e)| (path.clone(), frecency(e, now)))
            .collect()
    });
    list.retain(|(path, _)| path.is_dir());
    list.sort_by(|a, b| b.1.total_cmp(&a.1));
    list.into_iter().map(|(path, _)| path).collect()
}

/// Where the last bare `motty jump` left from, for the next one to go back.
static JUMPED_FROM: Mutex<Option<PathBuf>> = Mutex::new(None);

/// `motty jump [query]` from `cwd`. With a query, the best match (see
/// [`pick`]); without, home, or back from home to where the last jump
/// started.
pub fn jump(query: Option<&str>, cwd: Option<&Path>) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let mut from = JUMPED_FROM.lock().unwrap_or_else(|e| e.into_inner());
    let to = match query.map(str::trim).filter(|q| !q.is_empty()) {
        Some(query) => pick(query, &ranked(), cwd)?,
        None if cwd == Some(home.as_path()) => from.clone().filter(|d| d.is_dir())?,
        None => home,
    };
    *from = cwd.map(Path::to_path_buf);
    Some(to)
}

/// The best of `ranked` (best first) for `query`, Otty's way: an exact
/// folder name, then a name starting with it, then a name holding it, then
/// a path holding it, ignoring case; frecency breaks ties. When `cwd` is
/// among the matches, the one after it, so repeating a jump cycles.
pub fn pick(query: &str, ranked: &[PathBuf], cwd: Option<&Path>) -> Option<PathBuf> {
    let q = query.to_lowercase();
    let tier = |path: &Path| {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name == q {
            Some(0)
        } else if name.starts_with(&q) {
            Some(1)
        } else if name.contains(&q) {
            Some(2)
        } else if path.to_string_lossy().to_lowercase().contains(&q) {
            Some(3)
        } else {
            None
        }
    };
    let mut matches: Vec<(usize, &PathBuf)> =
        ranked.iter().filter_map(|p| Some((tier(p)?, p))).collect();
    // Stable: frecency order holds within a tier.
    matches.sort_by_key(|(tier, _)| *tier);
    let at = cwd.and_then(|cwd| matches.iter().position(|(_, p)| p.as_path() == cwd));
    let next = at.map_or(0, |i| (i + 1) % matches.len());
    matches.get(next).map(|(_, p)| (*p).clone())
}

/// File › Open Recent lists this many files.
const RECENT_FILES: usize = 10;

fn files_file() -> PathBuf {
    file().with_file_name("recent-files.json")
}

fn read_recent_files() -> Vec<PathBuf> {
    std::fs::read(files_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_recent_files(list: &[PathBuf]) {
    let result = serde_json::to_vec(list)
        .map_err(std::io::Error::other)
        .and_then(|json| {
            let path = files_file();
            std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
            std::fs::write(path, json)
        });
    if let Err(e) = result {
        tracing::warn!("couldn't save recent files: {e}");
    }
}

/// A file opened in an editor tab, for File › Open Recent.
pub fn record_file(path: &Path) {
    if !crate::recovery::enabled() {
        return;
    }
    let mut list = read_recent_files();
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(RECENT_FILES);
    write_recent_files(&list);
}

/// Files opened lately that still exist, the latest first.
pub fn recent_files() -> Vec<PathBuf> {
    let mut list = read_recent_files();
    list.retain(|p| p.is_file());
    list
}

/// Open Recent › Clear Menu.
pub fn clear_recent_files() {
    write_recent_files(&[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_visits_outweigh_old_ones() {
        let now = 1_000_000_000;
        let fresh = Entry {
            rank: 2.,
            last: now - 60,
        };
        let stale = Entry {
            rank: 10.,
            last: now - 30 * 86_400,
        };
        assert!(frecency(&fresh, now) > frecency(&stale, now));
    }

    #[test]
    fn jumps_rank_names_before_paths_and_cycle() {
        let ranked: Vec<PathBuf> = [
            "/w/api-docs/src",
            "/w/my-api",
            "/w/Typora-i18n",
            "/w/api-server",
            "/w/Typora",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        let pick = |q, cwd: Option<&str>| pick(q, &ranked, cwd.map(Path::new)).unwrap();
        // A name starting with it beats a better-ranked name holding it,
        // which beats a path holding it.
        assert_eq!(pick("api", None), PathBuf::from("/w/api-server"));
        assert_eq!(pick("typora", None), PathBuf::from("/w/Typora"));
        assert_eq!(
            pick("typora", Some("/w/Typora")),
            PathBuf::from("/w/Typora-i18n")
        );
        assert_eq!(
            pick("typora", Some("/w/Typora-i18n")),
            PathBuf::from("/w/Typora")
        );
        assert_eq!(pick("docs", None), PathBuf::from("/w/api-docs/src"));
        assert!(super::pick("nothing", &ranked, None).is_none());
    }
}
