//! The Dock icon follows every pane: red after a failure
//! (`dock-icon-error-badge`), and a progress bar while a command reports
//! progress (`dock-icon-animate-progress`).

use gpui::{App, Global};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DockState {
    #[default]
    Normal,
    /// A command or agent failed in a tab not yet looked at.
    Error,
    /// Something is working: the percentage, or `None` when unknown.
    Progress(Option<u8>),
}

#[derive(Default)]
struct Shown(DockState);

impl Global for Shown {}

/// Redraws the icon when the state changed.
pub fn set(state: DockState, cx: &mut App) {
    let shown = cx.default_global::<Shown>();
    if shown.0 == state {
        return;
    }
    shown.0 = state;
    #[cfg(target_os = "macos")]
    mac::draw(state);
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSApplication, NSColor, NSCompositingOperation, NSImage, NSRectFillUsingOperation,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    use super::DockState;

    thread_local! {
        /// The icon as the bundle has it, before any drawing.
        static ORIGINAL: std::cell::OnceCell<Option<Retained<NSImage>>> =
            const { std::cell::OnceCell::new() };
    }

    #[expect(
        deprecated,
        reason = "lockFocus draws the Dock icon simply; it's never shown scaled"
    )]
    pub fn draw(state: DockState) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let original = ORIGINAL.with(|o| o.get_or_init(|| app.applicationIconImage()).clone());
        let Some(original) = original else {
            return;
        };
        if state == DockState::Normal {
            // SAFETY: nil restores the bundle's icon.
            unsafe { app.setApplicationIconImage(None) };
            return;
        }
        let size = NSSize::new(512., 512.);
        let full = NSRect::new(NSPoint::new(0., 0.), size);
        let image = NSImage::initWithSize(NSImage::alloc(), size);
        image.lockFocus();
        original.drawInRect_fromRect_operation_fraction(
            full,
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            1.,
        );
        match state {
            DockState::Error => {
                // Tints the icon's own pixels only.
                NSColor::colorWithSRGBRed_green_blue_alpha(1., 0.23, 0.19, 0.6).set();
                NSRectFillUsingOperation(full, NSCompositingOperation::SourceAtop);
            }
            DockState::Progress(pct) => {
                let (x, y, w, h) = (64., 40., 384., 36.);
                NSColor::colorWithSRGBRed_green_blue_alpha(0., 0., 0., 0.55).set();
                NSRectFillUsingOperation(
                    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)),
                    NSCompositingOperation::SourceOver,
                );
                // Unknown progress shows a third of the bar.
                let share = pct.map_or(0.33, |p| f64::from(p.min(100)) / 100.);
                NSColor::colorWithSRGBRed_green_blue_alpha(0.04, 0.52, 1., 1.).set();
                NSRectFillUsingOperation(
                    NSRect::new(
                        NSPoint::new(x + 4., y + 4.),
                        NSSize::new((w - 8.) * share, h - 8.),
                    ),
                    NSCompositingOperation::SourceOver,
                );
            }
            DockState::Normal => {}
        }
        image.unlockFocus();
        // SAFETY: a fully drawn image, kept alive by the app.
        unsafe { app.setApplicationIconImage(Some(&image)) };
    }
}
