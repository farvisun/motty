//! Finder's icons for files and folders (NSWorkspace), drawn once into a
//! PNG and kept: files by extension, folders and the rest by path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{Image, ImageFormat};

/// Icons are drawn at this many points, at 2x.
const SIZE: f64 = 16.;
const SCALE: f64 = 2.;

#[derive(Clone, Hash, PartialEq, Eq)]
enum Key {
    Ext(String),
    Path(PathBuf),
}

/// What has been drawn; `None` when there was nothing to draw.
#[derive(Default)]
pub struct Icons(HashMap<Key, Option<Arc<Image>>>);

impl Icons {
    /// The icon for `path`; `dir` says it's a folder (which may have an
    /// icon of its own, so it isn't shared by extension).
    pub fn get(&mut self, path: &Path, dir: bool) -> Option<Arc<Image>> {
        let key = match path.extension() {
            Some(ext) if !dir => Key::Ext(ext.to_string_lossy().to_lowercase()),
            _ => Key::Path(path.to_path_buf()),
        };
        self.0
            .entry(key)
            .or_insert_with(|| {
                draw(path).map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png)))
            })
            .clone()
    }
}

/// The volume's name for `/` ("Macintosh HD"), else the last component.
pub fn display_name(path: &Path) -> String {
    if let Some(name) = path.file_name() {
        return name.to_string_lossy().into_owned();
    }
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSFileManager, NSString};
        let name = NSFileManager::defaultManager()
            .displayNameAtPath(&NSString::from_str(&path.to_string_lossy()))
            .to_string();
        if !name.is_empty() {
            return name;
        }
    }
    path.display().to_string()
}

/// `path`'s Finder icon as a PNG, `SIZE` points at `SCALE`.
#[cfg(target_os = "macos")]
fn draw(path: &Path) -> Option<Vec<u8>> {
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace,
        NSGraphicsContext, NSWorkspace,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

    let icon =
        NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(&path.to_string_lossy()));
    let px = (SIZE * SCALE) as isize;
    // SAFETY: null planes make AppKit allocate the pixels; the arguments
    // describe 8-bit RGBA, which it accepts.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            px,
            px,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    // Points, so the icon's 2x artwork is the one drawn.
    rep.setSize(NSSize::new(SIZE, SIZE));
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    let rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(SIZE, SIZE));
    icon.drawInRect_fromRect_operation_fraction(
        rect,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    // SAFETY: PNG with no properties is a supported representation.
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(data.to_vec())
}

#[cfg(not(target_os = "macos"))]
fn draw(_: &Path) -> Option<Vec<u8>> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn draws_finder_icons_as_png() {
        let png = draw(Path::new("/")).expect("an icon");
        assert!(png.starts_with(b"\x89PNG"));
        // 32×32 pixels: the IHDR's width and height.
        assert_eq!(&png[16..24], &[0, 0, 0, 32, 0, 0, 0, 32]);
        // Something was drawn: a blank image compresses to ~100 bytes.
        assert!(png.len() > 500, "{} bytes", png.len());
        assert!(!display_name(Path::new("/")).is_empty());
        assert_eq!(display_name(Path::new("/tmp/x.rs")), "x.rs");
    }
}
