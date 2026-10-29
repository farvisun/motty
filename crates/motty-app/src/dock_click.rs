//! A Dock click while windows are open. GPUI only reports one when no
//! window is open (`on_reopen`); Otty also uses it to go to the next tab
//! that failed, so the app delegate's handler is wrapped to see both.

use gpui::App;

pub fn install(cx: &mut App) {
    #[cfg(target_os = "macos")]
    mac::install(cx);
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::sync::OnceLock;

    use gpui::{App, AsyncApp};
    use objc2::runtime::{AnyObject, Bool, Imp, Sel};
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::NSApplication;

    type Reopen = unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject, Bool) -> Bool;

    /// GPUI's handler, which opens nothing itself but runs `on_reopen` when
    /// no window is open.
    static ORIGINAL: OnceLock<Reopen> = OnceLock::new();

    thread_local! {
        static APP: RefCell<Option<AsyncApp>> = const { RefCell::new(None) };
    }

    pub fn install(cx: &mut App) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() else {
            return;
        };
        let object: &AnyObject = delegate.as_ref();
        let Some(method) = object
            .class()
            .instance_method(sel!(applicationShouldHandleReopen:hasVisibleWindows:))
        else {
            return;
        };
        APP.with(|app| *app.borrow_mut() = Some(cx.to_async()));
        // SAFETY: `reopen` has the signature of the method it replaces, and
        // the old implementation is only called back with that signature.
        let old = unsafe { method.set_implementation(std::mem::transmute::<Reopen, Imp>(reopen)) };
        let _ = ORIGINAL.set(unsafe { std::mem::transmute::<Imp, Reopen>(old) });
    }

    unsafe extern "C-unwind" fn reopen(
        this: &AnyObject,
        sel: Sel,
        sender: *mut AnyObject,
        visible: Bool,
    ) -> Bool {
        if visible.as_bool() {
            APP.with(|app| {
                if let Some(app) = app.borrow().as_ref() {
                    let _ = app.update(crate::workspace::show_next_failing);
                }
            });
            return Bool::YES;
        }
        match ORIGINAL.get() {
            // SAFETY: GPUI's implementation of this same method.
            Some(original) => unsafe { original(this, sel, sender, visible) },
            None => Bool::YES,
        }
    }
}
