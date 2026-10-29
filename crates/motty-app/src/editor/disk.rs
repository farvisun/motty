//! The file on disk: reloading it (⌘R), noticing when something else
//! changes it, and keeping unsaved edits through a crash.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui::{Context, Window};

use super::{Buffer, FileEditor};
use crate::actions::ReloadFile;

/// How often the file is checked, and unsaved edits stashed.
const POLL: Duration = Duration::from_secs(1);

pub(super) fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Where a file's unsaved edits wait: Application Support/Motty/unsaved.
fn stash_path(path: &Path) -> PathBuf {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hash);
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Motty/unsaved")
        .join(format!("{:016x}.txt", hash.finish()))
}

impl FileEditor {
    pub(super) fn toggle_read_only(&mut self, cx: &mut Context<Self>) {
        if self.name.is_none() && !self.lockable && self.path.is_some() {
            self.set_read_only(!self.read_only, cx);
        }
    }

    pub(super) fn reload_action(&mut self, _: &ReloadFile, _: &mut Window, cx: &mut Context<Self>) {
        self.reload(cx);
    }

    /// Reads the file again, dropping unsaved edits (they stay undoable).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let read_only = self.read_only;
        self.push_undo();
        self.load(path);
        self.read_only = read_only || !self.can_edit;
        self.cursor = self.buffer.clamp(self.cursor);
        self.anchor = None;
        self.disk_changed = false;
        self.rehighlight();
        self.changed(cx);
    }

    /// Keep the edits, and stop asking until the file changes again.
    pub(super) fn keep_changes(&mut self, cx: &mut Context<Self>) {
        self.disk_changed = false;
        cx.notify();
    }

    /// Picks up edits a crash left unsaved, then watches the file.
    pub fn restore_unsaved(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.path.clone()
            && self.can_edit
            && let Ok(text) = std::fs::read_to_string(stash_path(&path))
            && text != self.saved
        {
            self.buffer = Buffer::from_text(&text);
            self.revision += 1;
            self.read_only = false;
            self.rehighlight();
        }
        self.watch_disk(cx);
    }

    /// Checks the file every second: an untouched pane reloads, one with
    /// edits offers to; unsaved edits are stashed as they change.
    fn watch_disk(&mut self, cx: &mut Context<Self>) {
        self.watching = Some(cx.spawn(async move |this, cx| {
            let mut stashed = None;
            loop {
                cx.background_executor().timer(POLL).await;
                let Ok(()) = this.update(cx, |this, cx| {
                    let Some(path) = this.path.clone() else {
                        return;
                    };
                    let now = modified(&path);
                    if now.is_some() && now != this.mtime {
                        this.mtime = now;
                        if this.is_dirty() {
                            this.disk_changed = true;
                            cx.notify();
                        } else {
                            this.reload(cx);
                        }
                    }
                    if stashed != Some(this.revision) {
                        stashed = Some(this.revision);
                        this.stash(&path);
                    }
                }) else {
                    return;
                };
            }
        }));
    }

    /// Writes the unsaved text aside, or clears it once saved.
    fn stash(&self, path: &Path) {
        let stash = stash_path(path);
        if self.is_dirty() {
            let text = self.buffer.text();
            let _ = stash
                .parent()
                .map(std::fs::create_dir_all)
                .and_then(Result::ok)
                .map(|()| std::fs::write(&stash, text));
        } else {
            let _ = std::fs::remove_file(stash);
        }
    }

    /// Clears the stash when the pane closes: its edits were saved or let go.
    pub(super) fn clear_stash(&self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(stash_path(path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stash_is_per_file() {
        let a = stash_path(Path::new("/a/x.rs"));
        assert_ne!(a, stash_path(Path::new("/a/y.rs")));
        assert_eq!(a, stash_path(Path::new("/a/x.rs")));
        assert!(a.to_string_lossy().contains("Motty/unsaved/"));
    }
}
