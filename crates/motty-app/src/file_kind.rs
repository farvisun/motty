//! Which pane a file opens in: the image viewer, the text editor or a
//! Quick Look preview.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// How much of a file is read to tell text from binary.
const SNIFF: u64 = 8192;

/// Files the image viewer shows. SVG isn't one: the editor previews it.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "heic", "heif", "ico",
];

/// Files that are binary whatever their first bytes look like: documents,
/// archives and disk images, media, fonts, compiled code and databases.
/// (Not `.key`: more often a PEM key than a Keynote deck.)
const BINARY_EXTENSIONS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "pages", "numbers", "odt", "ods", "odp",
    "epub", "zip", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "tar", "dmg", "iso", "pkg", "jar",
    "mp3", "m4a", "aac", "wav", "aif", "aiff", "flac", "ogg", "opus", "mp4", "m4v", "mov", "avi",
    "mkv", "webm", "psd", "icns", "avif", "ttf", "otf", "woff", "woff2", "o", "a", "so", "dylib",
    "dll", "exe", "class", "wasm", "pyc", "bin", "db", "sqlite", "sqlite3",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// Shown by [`crate::image_view::ImageView`].
    Image,
    /// Opened in the editor (also what a missing or unreadable file gets,
    /// so the editor can say why).
    Text,
    /// PDFs and other binary files, shown by
    /// [`crate::quick_look::QuickLookView`].
    QuickLook,
}

/// What `path` is, by its extension, else by whether its first 8 KiB have
/// a NUL byte in them.
pub fn kind_of(path: &Path) -> FileKind {
    let ext = extension(path);
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        FileKind::Image
    } else if BINARY_EXTENSIONS.contains(&ext.as_str()) || has_nul(path) {
        FileKind::QuickLook
    } else {
        FileKind::Text
    }
}

/// `path`'s extension, lowercased; empty when it has none.
pub fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn has_nul(path: &Path) -> bool {
    let mut head = Vec::new();
    File::open(path)
        .and_then(|f| f.take(SNIFF).read_to_end(&mut head))
        .is_ok_and(|_| head.contains(&0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A file in a folder of its own, removed when dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str, bytes: &[u8]) -> Self {
            let dir =
                std::env::temp_dir().join(format!("motty-file-kind-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            if let Some(dir) = self.0.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    #[test]
    fn images_by_extension() {
        for name in [
            "a.png", "b.JPG", "c.jpeg", "d.gif", "e.webp", "f.HEIC", "g.tiff",
        ] {
            assert_eq!(kind_of(Path::new(name)), FileKind::Image, "{name}");
        }
    }

    #[test]
    fn svg_is_text() {
        let f = Temp::new("logo.svg", b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>");
        assert_eq!(kind_of(&f.0), FileKind::Text);
    }

    #[test]
    fn known_binaries_by_extension() {
        // Not read: a PDF is Quick Look's even if it doesn't exist.
        assert_eq!(kind_of(Path::new("/nowhere/x.pdf")), FileKind::QuickLook);
        assert_eq!(kind_of(Path::new("/nowhere/x.ZIP")), FileKind::QuickLook);
        assert_eq!(kind_of(Path::new("/nowhere/x.docx")), FileKind::QuickLook);
    }

    #[test]
    fn sniffs_nul_bytes() {
        let bin = Temp::new("blob", b"\x7fELF\x02\x01\x01\0\0\0");
        assert_eq!(kind_of(&bin.0), FileKind::QuickLook);
        let text = Temp::new("notes.txt", "héllo\nworld\n".as_bytes());
        assert_eq!(kind_of(&text.0), FileKind::Text);
    }

    #[test]
    fn nul_past_the_sniffed_head_is_text() {
        let mut bytes = vec![b'a'; SNIFF as usize];
        bytes.push(0);
        let f = Temp::new("long.log", &bytes);
        assert_eq!(kind_of(&f.0), FileKind::Text);
    }

    #[test]
    fn missing_or_empty_is_text() {
        assert_eq!(kind_of(Path::new("/nowhere/notes")), FileKind::Text);
        let f = Temp::new("empty", b"");
        assert_eq!(kind_of(&f.0), FileKind::Text);
    }
}
