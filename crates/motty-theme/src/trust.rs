//! Commands a repository asks Motty to run (`.motty/worktree.toml`'s
//! scripts, `.motty/layouts/`) run only once the user approved that file as
//! it is now: an approval is for the file's path and the SHA-256 of its
//! contents, so any change asks again.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default, rename = "file")]
    files: Vec<Entry>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    sha256: String,
}

/// Where approvals are kept: beside the session, not in the config, so a
/// synced config doesn't approve anything on another Mac.
pub fn store_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_default()
        .join("Motty")
        .join("trusted.toml")
}

fn digest(contents: &[u8]) -> String {
    Sha256::digest(contents)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn load(store: &Path) -> Store {
    std::fs::read_to_string(store)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// Whether the user approved `path` with exactly these contents.
pub fn is_trusted(path: &Path, contents: &[u8]) -> bool {
    is_trusted_in(&store_path(), path, contents)
}

/// Approves `path` with these contents, replacing an older approval of it.
pub fn trust(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    trust_in(&store_path(), path, contents)
}

fn is_trusted_in(store: &Path, path: &Path, contents: &[u8]) -> bool {
    let entry = Entry {
        path: key(path),
        sha256: digest(contents),
    };
    load(store).files.contains(&entry)
}

fn trust_in(store: &Path, path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let path = key(path);
    let mut all = load(store);
    all.files.retain(|e| e.path != path);
    all.files.push(Entry {
        path,
        sha256: digest(contents),
    });
    if let Some(dir) = store.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string(&all).map_err(std::io::Error::other)?;
    std::fs::write(store, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_is_for_the_contents() {
        let dir = std::env::temp_dir().join(format!("motty-trust-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("trusted.toml");
        let file = dir.join("worktree.toml");
        std::fs::write(&file, "setup = \"make\"").unwrap();
        assert!(!is_trusted_in(&store, &file, b"setup = \"make\""));
        trust_in(&store, &file, b"setup = \"make\"").unwrap();
        assert!(is_trusted_in(&store, &file, b"setup = \"make\""));
        // Changed contents ask again; approving them replaces the old entry.
        assert!(!is_trusted_in(&store, &file, b"setup = \"rm -rf ~\""));
        trust_in(&store, &file, b"setup = \"make test\"").unwrap();
        assert!(!is_trusted_in(&store, &file, b"setup = \"make\""));
        assert_eq!(load(&store).files.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
