//! Native backdrop materials behind regions of the window.
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
    Sidebar { width: f32 },
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
    #[cfg(target_os = "macos")]
    views: Vec<objc2::rc::Retained<objc2_app_kit::NSView>>,
}

impl Backdrops {
    /// Installs `wanted` (bottom-most first), replacing what was there when it
    /// changed. Cheap to call every frame.
    pub fn sync(&mut self, window: &Window, wanted: &[Backdrop]) {
        let wanted: Vec<Backdrop> = wanted.iter().copied().filter(|b| b.material != Material::None).collect();
        if wanted == self.installed {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            for view in self.views.drain(..) {
                view.removeFromSuperview();
            }
            match mac::install(window, &wanted) {
                Some(views) => self.views = views,
                None => tracing::warn!("could not install window materials"),
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = window;
        self.installed = wanted;
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2::runtime::AnyClass;
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSGlassEffectView, NSGlassEffectViewStyle, NSView, NSVisualEffectBlendingMode,
        NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::{Backdrop, Region};
    use motty_theme::Material;

    pub fn install(window: &gpui::Window, backdrops: &[Backdrop]) -> Option<Vec<Retained<NSView>>> {
        let mtm = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw() else { return None };
        // SAFETY: GPUI's AppKit handle points at its live NSView, and we are
        // on the main thread.
        let gpui_view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let content = unsafe { gpui_view.superview() }?;
        let bounds = content.bounds();
        let mut views = Vec::new();
        for backdrop in backdrops {
            let (frame, mask) = placement(backdrop.region, bounds);
            let view = make_view(backdrop.material, frame, mtm);
            view.setAutoresizingMask(mask);
            // Each view goes directly below GPUI's, so later ones stack above
            // earlier ones.
            content.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, Some(gpui_view));
            views.push(view);
        }
        Some(views)
    }

    /// Frame in the (unflipped, bottom-left origin) content view.
    fn placement(region: Region, bounds: NSRect) -> (NSRect, NSAutoresizingMaskOptions) {
        match region {
            Region::Window => (
                bounds,
                NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
            ),
            Region::Sidebar { width } => (
                NSRect::new(NSPoint::new(0., 0.), NSSize::new(width as f64, bounds.size.height)),
                NSAutoresizingMaskOptions::ViewHeightSizable | NSAutoresizingMaskOptions::ViewMaxXMargin,
            ),
        }
    }

    fn make_view(material: Material, frame: NSRect, mtm: MainThreadMarker) -> Retained<NSView> {
        // Liquid Glass (macOS 26+) for `glass`, when the system has it.
        if material == Material::Glass && AnyClass::get(c"NSGlassEffectView").is_some() {
            let glass = NSGlassEffectView::initWithFrame(mtm.alloc(), frame);
            glass.setStyle(NSGlassEffectViewStyle::Regular);
            glass.setCornerRadius(0.);
            return Retained::into_super(glass);
        }
        let view = NSVisualEffectView::initWithFrame(mtm.alloc(), frame);
        view.setMaterial(match material {
            Material::VibrancyThin => NSVisualEffectMaterial::HeaderView,
            Material::VibrancyRegular => NSVisualEffectMaterial::Sidebar,
            Material::VibrancyThick => NSVisualEffectMaterial::WindowBackground,
            Material::Glass | Material::None => NSVisualEffectMaterial::UnderWindowBackground,
        });
        view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        view.setState(NSVisualEffectState::FollowsWindowActiveState);
        Retained::into_super(view)
    }
}
