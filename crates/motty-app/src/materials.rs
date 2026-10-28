//! Native backdrop materials behind regions of the window, and the theme's
//! light or dark appearance for native chrome.
//!
//! GPUI offers one window-wide blur. Otty themes put materials on single
//! regions (`[window]`, `[sidebar]`, `[titlebar]`), so Motty inserts its own
//! AppKit effect views into the window's content view, below GPUI's Metal
//! view, the same way GPUI adds its blur view. GPUI then paints those regions
//! with transparent or tinted backgrounds and the material shows through.

use gpui::Window;
use motty_theme::Material;

/// Where a backdrop sits, pinned with autoresizing so it follows live resizes
/// without a round trip through GPUI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Region {
    Window,
    /// Full-height column on the left edge, `width` points wide.
    Sidebar {
        width: f32,
    },
    /// Full-height column on the right edge (the details panel).
    DetailsPanel {
        width: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Backdrop {
    pub region: Region,
    pub material: Material,
}

/// The backdrop views installed in one window.
#[derive(Default)]
pub struct Backdrops {
    installed: Vec<Backdrop>,
    /// Appearance last applied; `None` until the first sync.
    dark: Option<bool>,
    #[cfg(target_os = "macos")]
    views: Vec<objc2::rc::Retained<objc2_app_kit::NSView>>,
}

impl Backdrops {
    /// Installs `wanted` (bottom-most first), replacing what was there when it
    /// changed. Cheap to call every frame.
    /// `dark` gives the materials and the traffic lights the dark appearance.
    pub fn sync(&mut self, window: &Window, wanted: &[Backdrop], dark: bool) {
        let wanted: Vec<Backdrop> = wanted
            .iter()
            .copied()
            .filter(|b| b.material != Material::None)
            .collect();
        if wanted == self.installed && Some(dark) == self.dark {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            mac::set_titlebar_appearance(window, dark);
            for view in self.views.drain(..) {
                view.removeFromSuperview();
            }
            match mac::install(window, &wanted, dark) {
                Some(views) => self.views = views,
                None => tracing::warn!("could not install window materials"),
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = window;
        self.installed = wanted;
        self.dark = Some(dark);
    }
}

/// Floats the window over other apps' without making Motty the active
/// app, so it draws (and can be captured) while the keyboard stays where it
/// was. An occluded window doesn't draw, and ordering front alone stays
/// behind the active app's windows.
pub fn show_inactive(window: &Window) {
    #[cfg(target_os = "macos")]
    mac::show_inactive(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// View › Pin Window: keeps the window above other apps' windows.
pub fn set_pinned(window: &Window, pinned: bool) {
    #[cfg(target_os = "macos")]
    mac::set_pinned(window, pinned);
    #[cfg(not(target_os = "macos"))]
    let _ = (window, pinned);
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2::runtime::AnyClass;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSAutoresizingMaskOptions, NSGlassEffectView, NSGlassEffectViewStyle, NSView,
        NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
        NSVisualEffectView, NSWindowButton, NSWindowOrderingMode,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::{Backdrop, Region};
    use motty_theme::Material;

    /// GPUI's NSView for the window.
    fn gpui_view(window: &gpui::Window) -> Option<&NSView> {
        MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
        else {
            return None;
        };
        // SAFETY: GPUI's AppKit handle points at its live NSView, which
        // outlives this borrow of the window, and we are on the main thread.
        Some(unsafe { handle.ns_view.cast::<NSView>().as_ref() })
    }

    pub fn set_pinned(window: &gpui::Window, pinned: bool) {
        if let Some(ns_window) = gpui_view(window).and_then(|v| v.window()) {
            ns_window.setLevel(if pinned {
                objc2_app_kit::NSFloatingWindowLevel
            } else {
                objc2_app_kit::NSNormalWindowLevel
            });
        }
    }

    pub fn show_inactive(window: &gpui::Window) {
        if let Some(ns_window) = gpui_view(window).and_then(|v| v.window()) {
            ns_window.setLevel(objc2_app_kit::NSFloatingWindowLevel);
            ns_window.orderFrontRegardless();
        }
    }

    /// Gives the traffic lights the theme's appearance, as Otty does, by
    /// setting it on their container. The window keeps the system appearance,
    /// so GPUI still sees system light/dark changes.
    pub fn set_titlebar_appearance(window: &gpui::Window, dark: bool) {
        let Some(ns_window) = gpui_view(window).and_then(|v| v.window()) else {
            return;
        };
        let Some(close) = ns_window.standardWindowButton(NSWindowButton::CloseButton) else {
            return;
        };
        if let Some(container) = unsafe { close.superview() } {
            set_appearance(&container, dark);
        }
    }

    pub fn install(
        window: &gpui::Window,
        backdrops: &[Backdrop],
        dark: bool,
    ) -> Option<Vec<Retained<NSView>>> {
        let mtm = MainThreadMarker::new()?;
        let gpui_view = gpui_view(window)?;
        let content = unsafe { gpui_view.superview() }?;
        let bounds = content.bounds();
        let mut views = Vec::new();
        for backdrop in backdrops {
            let (frame, mask) = placement(backdrop.region, bounds);
            let view = make_view(backdrop.material, dark, frame, mtm);
            view.setAutoresizingMask(mask);
            // Each view goes directly below GPUI's, so later ones stack above
            // earlier ones.
            content.addSubview_positioned_relativeTo(
                &view,
                NSWindowOrderingMode::Below,
                Some(gpui_view),
            );
            views.push(view);
        }
        Some(views)
    }

    /// Frame in the (unflipped, bottom-left origin) content view.
    fn placement(region: Region, bounds: NSRect) -> (NSRect, NSAutoresizingMaskOptions) {
        match region {
            Region::Window => (
                bounds,
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            ),
            Region::Sidebar { width } => (
                NSRect::new(
                    NSPoint::new(0., 0.),
                    NSSize::new(width as f64, bounds.size.height),
                ),
                NSAutoresizingMaskOptions::ViewHeightSizable
                    | NSAutoresizingMaskOptions::ViewMaxXMargin,
            ),
            Region::DetailsPanel { width } => (
                NSRect::new(
                    NSPoint::new(bounds.size.width - width as f64, 0.),
                    NSSize::new(width as f64, bounds.size.height),
                ),
                NSAutoresizingMaskOptions::ViewHeightSizable
                    | NSAutoresizingMaskOptions::ViewMinXMargin,
            ),
        }
    }

    /// Materials follow the theme's mode rather than the system appearance.
    fn set_appearance(view: &NSView, dark: bool) {
        // SAFETY: the appearance names are AppKit constants.
        let name = unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        view.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
    }

    fn make_view(
        material: Material,
        dark: bool,
        frame: NSRect,
        mtm: MainThreadMarker,
    ) -> Retained<NSView> {
        // Liquid Glass (macOS 26+) for `glass`, when the system has it; its
        // regular style matches Otty's glass exactly over a solid backdrop.
        if material == Material::Glass && AnyClass::get(c"NSGlassEffectView").is_some() {
            let glass = NSGlassEffectView::initWithFrame(mtm.alloc(), frame);
            glass.setStyle(NSGlassEffectViewStyle::Regular);
            set_appearance(&glass, dark);
            glass.setCornerRadius(0.);
            return Retained::into_super(glass);
        }
        let view = NSVisualEffectView::initWithFrame(mtm.alloc(), frame);
        // Measured against Otty over a solid backdrop: thin and regular blur
        // identically to the sidebar material, in light and dark. No Otty
        // theme uses thick, so its heavier window material is a guess.
        view.setMaterial(match material {
            Material::VibrancyThick => NSVisualEffectMaterial::WindowBackground,
            _ => NSVisualEffectMaterial::Sidebar,
        });
        set_appearance(&view, dark);
        view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        view.setState(NSVisualEffectState::FollowsWindowActiveState);
        Retained::into_super(view)
    }
}
