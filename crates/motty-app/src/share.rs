//! The Share button: macOS's share menu (NSSharingServicePicker) for a file
//! or a link, below the point that was clicked.

use std::path::Path;

use gpui::{Pixels, Point, Window};

/// What to share.
#[allow(dead_code)] // Url: the web pane's Share.
pub enum Shared<'a> {
    File(&'a Path),
    Url(&'a str),
}

#[cfg(target_os = "macos")]
pub fn show(window: &Window, item: Shared, at: Point<Pixels>) {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSSharingServicePicker, NSView};
    use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSURL};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: GPUI's AppKit handle points at its live NSView, which outlives
    // this borrow of the window, and we are on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let url: Option<Retained<NSURL>> = match item {
        Shared::File(path) => Some(NSURL::fileURLWithPath(&NSString::from_str(
            &path.to_string_lossy(),
        ))),
        Shared::Url(link) => NSURL::URLWithString(&NSString::from_str(link)),
    };
    let Some(url) = url else { return };
    let items: Retained<NSArray<AnyObject>> = NSArray::from_retained_slice(&[url.into()]);
    // SAFETY: the picker is created and shown on the main thread with a live
    // view; AppKit keeps it alive while its menu is up.
    unsafe {
        let picker =
            NSSharingServicePicker::initWithItems(mtm.alloc::<NSSharingServicePicker>(), &items);
        let height = view.bounds().size.height;
        let y = if view.isFlipped() {
            f64::from(at.y)
        } else {
            height - f64::from(at.y)
        };
        let rect = NSRect::new(NSPoint::new(f64::from(at.x), y), NSSize::new(1., 1.));
        picker.showRelativeToRect_ofView_preferredEdge(rect, view, NSRectEdge::MinY);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn show(_: &Window, _: Shared, _: Point<Pixels>) {}
