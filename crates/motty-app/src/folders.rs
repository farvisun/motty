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

static DB: Mutex<Option<HashMap<PathBuf, Entry>>> = Mutex::new(None);

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
    let db = db.get_or_insert_with(|| {
        std::fs::read(file())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    });
    f(db)
}

/// Counts a visit to `path` (home doesn't count: it's where shells start).
pub fn record(path: &Path) {
    if !crate::recovery::enabled() || dirs::home_dir().as_deref() == Some(path) {
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
    let result = snapshot.map_err(std::io::Error::other).and_then(|json| {
        let path = file();
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)
    });
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
}
