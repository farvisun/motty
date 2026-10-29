//! What GPUI can't do with the menu bar: Minimize and Zoom sent to the key
//! window as AppKit's own (so macOS adds Fill, Center and Move & Resize
//! after them, as in Otty), with Minimize All and Zoom All as their ⌥
//! alternates; and items whose shortcut Otty leaves off the menu (it still
//! works, through the keymap). Run after each `set_menus`.

/// Items shown without their shortcut, as Otty shows them.
const UNSHOWN_KEYS: [&str; 10] = [
    "Send to Chat...",
    "Vi Mode",
    "Move Divider Up",
    "Move Divider Down",
    "Move Divider Left",
    "Move Divider Right",
    "Focus Pane Above",
    "Focus Pane Below",
    "Focus Pane to the Left",
    "Focus Pane to the Right",
];

pub fn patch() {
    #[cfg(target_os = "macos")]
    mac::patch();
}

#[cfg(target_os = "macos")]
mod mac {

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
    use objc2_foundation::NSString;

    define_class!(
        // Zoom All has no AppKit action; this zooms every window.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "MottyWindowMenuTarget"]
        struct Target;

        impl Target {
            #[unsafe(method(zoomAll:))]
            fn zoom_all(&self, _sender: Option<&AnyObject>) {
                let app = NSApplication::sharedApplication(self.mtm());
                for window in app.windows() {
                    if window.isVisible() && window.isZoomable() && !window.isMiniaturized() {
                        window.zoom(None);
                    }
                }
            }
        }
    );

    thread_local! {
        static TARGET: Option<Retained<Target>> = MainThreadMarker::new().map(|mtm| {
            // SAFETY: NSObject's init on a freshly allocated instance.
            unsafe { msg_send![Target::alloc(mtm), init] }
        });
    }

    pub fn patch() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        if let Some(main) = app.mainMenu() {
            hide_keys(&main);
        }
        let Some(menu) = app.windowsMenu() else {
            return;
        };
        retarget(&menu, "Minimize", sel!(performMiniaturize:));
        retarget(&menu, "Zoom", sel!(performZoom:));
        alternate(
            &menu,
            "Minimize",
            "Minimize All",
            sel!(miniaturizeAll:),
            None,
            mtm,
        );
        TARGET.with(|target| {
            let target = target.as_deref().map(|t| t as &AnyObject);
            alternate(&menu, "Zoom", "Zoom All", sel!(zoomAll:), target, mtm);
        });
    }

    fn hide_keys(menu: &NSMenu) {
        for item in menu.itemArray() {
            if let Some(sub) = item.submenu() {
                hide_keys(&sub);
            } else if super::UNSHOWN_KEYS.contains(&item.title().to_string().as_str()) {
                item.setKeyEquivalent(&NSString::from_str(""));
                item.setKeyEquivalentModifierMask(NSEventModifierFlags::empty());
            }
        }
    }

    fn retarget(menu: &NSMenu, title: &str, action: objc2::runtime::Sel) {
        if let Some(item) = menu.itemWithTitle(&NSString::from_str(title)) {
            // SAFETY: a standard NSWindow action, sent along the responder
            // chain to the key window.
            unsafe {
                item.setAction(Some(action));
                item.setTarget(None);
            }
        }
    }

    /// Adds `title` under `of`, shown in its place while ⌥ is held.
    fn alternate(
        menu: &NSMenu,
        of: &str,
        title: &str,
        action: objc2::runtime::Sel,
        target: Option<&AnyObject>,
        mtm: MainThreadMarker,
    ) {
        let index = menu.indexOfItemWithTitle(&NSString::from_str(of));
        if index < 0 || menu.indexOfItemWithTitle(&NSString::from_str(title)) >= 0 {
            return;
        }
        let Some(main) = menu.itemAtIndex(index) else {
            return;
        };
        // SAFETY: the action is NSApplication's or `Target`'s, which lives
        // as long as the thread.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &main.keyEquivalent(),
            )
        };
        item.setKeyEquivalentModifierMask(
            main.keyEquivalentModifierMask() | NSEventModifierFlags::Option,
        );
        item.setAlternate(true);
        // SAFETY: as above.
        unsafe { item.setTarget(target) };
        menu.insertItem_atIndex(&item, index + 1);
    }
}
