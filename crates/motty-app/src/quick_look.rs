//! A tab that previews a file with Quick Look, as Otty does for PDFs and
//! other files that are neither text nor images: a PDF's pages, an office
//! document, or the file's big icon for a binary.
//!
//! The preview (QLPreviewView) is a native view over GPUI's, placed and
//! hidden as the web view is (see [`crate::web_view::paint_overlay`]).
//! Elsewhere than macOS the pane is empty.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, Pixels, Window, canvas, div,
    prelude::*, px,
};
use motty_render::{TerminalStyle, hsla};
use motty_term::alacritty_terminal::vte::ansi::NamedColor;

use crate::file_kind::extension;
use crate::file_toolbar::{self, Ink};
use crate::web_view::{Overlaid, paint_overlay};

#[cfg(target_os = "macos")]
use mac::Native;
#[cfg(not(target_os = "macos"))]
use stub::Native;

pub enum QuickLookEvent {
    /// The path changed, so the title did.
    Changed,
}

pub struct QuickLookView {
    focus: FocusHandle,
    path: PathBuf,
    style: Rc<TerminalStyle>,
    native: Option<Rc<Native>>,
}

impl EventEmitter<QuickLookEvent> for QuickLookView {}

impl Focusable for QuickLookView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl QuickLookView {
    pub fn new(path: PathBuf, style: TerminalStyle, cx: &mut Context<Self>) -> Self {
        let native = Native::new(&path, style.dark).map(Rc::new);
        if native.is_none() && cfg!(target_os = "macos") {
            tracing::warn!("could not create a Quick Look preview");
        }
        Self {
            focus: cx.focus_handle(),
            path,
            style: Rc::new(style),
            native,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn title(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }

    /// Where a new terminal opened from here should start.
    pub fn folder(&self) -> Option<PathBuf> {
        self.path.parent().map(Path::to_path_buf)
    }

    /// Previews `path` instead (the file was renamed or replaced).
    pub fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(native) = &self.native {
            native.set_path(&path);
        }
        if path != self.path {
            self.path = path;
            cx.emit(QuickLookEvent::Changed);
        }
        cx.notify();
    }

    pub fn set_style(&mut self, style: TerminalStyle, cx: &mut Context<Self>) {
        if let Some(native) = &self.native {
            native.set_appearance(style.dark);
        }
        self.style = Rc::new(style);
        cx.notify();
    }

    fn render_toolbar(&self) -> gpui::AnyElement {
        let ink = Ink(self.style.palette[NamedColor::Foreground as usize]);
        let pdf = extension(&self.path) == "pdf";
        let left = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .when(pdf, |d| d.child(file_toolbar::send_to_chat(ink)))
            .child(file_toolbar::share(self.path.clone(), ink));
        file_toolbar::bar(left, file_toolbar::close(self.focus.clone(), ink))
    }
}

impl Drop for QuickLookView {
    fn drop(&mut self) {
        if let Some(native) = &self.native {
            native.detach();
        }
    }
}

impl Render for QuickLookView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let native = self.native.clone();
        let id = cx.entity_id();
        div()
            .id("quick-look")
            .key_context("QuickLook")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(self.style.background))
            .child(self.render_toolbar())
            .child(
                div().flex_1().min_h_0().child(
                    canvas(
                        |_, _, _| {},
                        move |bounds: Bounds<Pixels>, (), window, _| {
                            if let Some(native) = native {
                                paint_overlay(native, "quick-look", id, bounds, window);
                            }
                        },
                    )
                    .size_full(),
                ),
            )
    }
}

impl Overlaid for Native {
    fn painted(&self) -> u64 {
        Native::painted(self)
    }
    fn unpainted(&self, paint: u64) {
        Native::unpainted(self, paint)
    }
    fn place(&self, window: &Window, bounds: Bounds<Pixels>, visible: Bounds<Pixels>) {
        Native::place(self, window, bounds, visible)
    }
    fn blur(&self) {
        Native::blur(self)
    }
    fn set_suppressed(&self, suppressed: bool) {
        Native::set_suppressed(self, suppressed)
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::Path;

    use gpui::{Bounds, Pixels, Window};
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSURL};
    use objc2_quick_look_ui::{QLPreviewView, QLPreviewViewStyle};

    use crate::web_view::Overlay;

    /// The preview, as an [`Overlay`].
    pub struct Native {
        overlay: Overlay,
        view: Retained<QLPreviewView>,
    }

    impl Native {
        pub fn new(path: &Path, dark: bool) -> Option<Self> {
            let mtm = MainThreadMarker::new()?;
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.));
            // SAFETY: creating a view on the main thread.
            let view = unsafe {
                QLPreviewView::initWithFrame_style(mtm.alloc(), frame, QLPreviewViewStyle::Normal)
            }?;
            // Closed in `detach` rather than with its window: the tab may
            // move to another window.
            // SAFETY: setting a property on the main thread.
            unsafe { view.setShouldCloseWithWindow(false) };
            let native = Self {
                overlay: Overlay::new(Retained::into_super(view.clone())),
                view,
            };
            native.set_path(path);
            native.set_appearance(dark);
            Some(native)
        }

        pub fn set_path(&self, path: &Path) {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            // SAFETY: a file URL is a valid preview item (NSURL conforms to
            // QLPreviewItem), which the view retains; on the main thread.
            unsafe {
                self.view
                    .setPreviewItem(Some(ProtocolObject::from_ref(&*url)))
            };
        }

        pub fn painted(&self) -> u64 {
            self.overlay.painted()
        }

        pub fn unpainted(&self, paint: u64) {
            self.overlay.unpainted(paint);
        }

        pub fn set_suppressed(&self, suppressed: bool) {
            self.overlay.set_suppressed(suppressed);
        }

        pub fn place(&self, window: &Window, bounds: Bounds<Pixels>, visible: Bounds<Pixels>) {
            self.overlay.place(window, bounds, visible);
        }

        pub fn blur(&self) {
            self.overlay.blur();
        }

        /// Takes the preview out of its window and closes it for good.
        pub fn detach(&self) {
            self.overlay.detach();
            // SAFETY: closing the view on the main thread; it isn't used
            // after this.
            unsafe { self.view.close() };
        }

        pub fn set_appearance(&self, dark: bool) {
            // SAFETY: the appearance names are AppKit constants.
            let name = unsafe {
                if dark {
                    NSAppearanceNameDarkAqua
                } else {
                    NSAppearanceNameAqua
                }
            };
            self.view
                .setAppearance(NSAppearance::appearanceNamed(name).as_deref());
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod stub {
    use std::path::Path;

    use gpui::{Bounds, Pixels, Window};

    /// No Quick Look outside macOS; never constructed.
    pub enum Native {}

    impl Native {
        pub fn new(_: &Path, _: bool) -> Option<Self> {
            None
        }
        pub fn set_path(&self, _: &Path) {
            match *self {}
        }
        pub fn painted(&self) -> u64 {
            match *self {}
        }
        pub fn unpainted(&self, _: u64) {
            match *self {}
        }
        pub fn set_suppressed(&self, _: bool) {
            match *self {}
        }
        pub fn place(&self, _: &Window, _: Bounds<Pixels>, _: Bounds<Pixels>) {
            match *self {}
        }
        pub fn blur(&self) {
            match *self {}
        }
        pub fn detach(&self) {
            match *self {}
        }
        pub fn set_appearance(&self, _: bool) {
            match *self {}
        }
    }
}
