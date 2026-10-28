//! Native pop-up menus (AppKit `NSMenu`): the terminal's right-click menu
//! and the settings window's pop-up buttons, so they look and behave like
//! every other Mac menu.
//!
//! AppKit tracks a pop-up menu in a nested run loop. It's run from a
//! foreground task, outside any GPUI update, so GPUI keeps drawing and
//! handling events while the menu is open; the pick is applied afterwards.

use gpui::{App, Keystroke, Pixels, Point, SharedString, Window};

/// One row of a menu.
pub enum Entry {
    Item(Item),
    Submenu {
        label: SharedString,
        entries: Vec<Entry>,
    },
    Separator,
}

pub struct Item {
    pub label: SharedString,
    /// An SF Symbol shown before the label.
    pub symbol: Option<&'static str>,
    /// The shortcut shown on the right (pressing it picks the item too).
    pub keystroke: Option<Keystroke>,
    pub checked: bool,
    pub enabled: bool,
}

impl Item {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            symbol: None,
            keystroke: None,
            checked: false,
            enabled: true,
        }
    }

    pub fn symbol(mut self, name: &'static str) -> Self {
        self.symbol = Some(name);
        self
    }

    pub fn keystroke(mut self, keystroke: Option<Keystroke>) -> Self {
        self.keystroke = keystroke;
        self
    }

    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
}

impl From<Item> for Entry {
    fn from(item: Item) -> Self {
        Entry::Item(item)
    }
}

/// Shows `entries` with its top left at `position` (window coordinates)
/// and calls `on_pick` with the chosen item's index, counting items (not
/// separators or submenus) in order, submenus' items in place. `selected`
/// puts that item under the pointer, as a pop-up button does.
pub fn popup(
    entries: Vec<Entry>,
    position: Point<Pixels>,
    selected: Option<usize>,
    window: &mut Window,
    cx: &mut App,
    on_pick: impl FnOnce(usize, &mut Window, &mut App) + 'static,
) {
    popup_then(
        entries,
        position,
        selected,
        window,
        cx,
        |picked, window, cx| {
            if let Some(ix) = picked {
                on_pick(ix, window, cx);
            }
        },
    );
}

/// [`popup`], calling `on_close` once the menu is gone, with the pick if
/// there was one.
pub fn popup_then(
    entries: Vec<Entry>,
    position: Point<Pixels>,
    selected: Option<usize>,
    window: &mut Window,
    cx: &mut App,
    on_close: impl FnOnce(Option<usize>, &mut Window, &mut App) + 'static,
) {
    #[cfg(target_os = "macos")]
    mac::popup(entries, position, selected, window, cx, on_close);
    #[cfg(not(target_os = "macos"))]
    on_close(None, window, cx);
    #[cfg(not(target_os = "macos"))]
    let _ = (entries, position, selected);
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::Cell;

    use gpui::{App, Keystroke, Pixels, Point, Window};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSControlStateValueOn, NSEventModifierFlags, NSImage, NSMenu, NSMenuItem, NSView,
    };
    use objc2_foundation::{NSPoint, NSString};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::Entry;

    struct Ivars {
        picked: Cell<isize>,
    }

    define_class!(
        // Receives the chosen item's action and keeps its tag.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "MottyMenuTarget"]
        #[ivars = Ivars]
        struct Target;

        impl Target {
            #[unsafe(method(pick:))]
            fn pick(&self, sender: &NSMenuItem) {
                self.ivars().picked.set(sender.tag());
            }
        }
    );

    impl Target {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(Ivars {
                picked: Cell::new(-1),
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    pub(super) fn popup(
        entries: Vec<Entry>,
        position: Point<Pixels>,
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut App,
        on_close: impl FnOnce(Option<usize>, &mut Window, &mut App) + 'static,
    ) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        let view = handle.ns_view.as_ptr() as usize;
        let window_handle = Window::window_handle(window);
        cx.spawn(async move |cx| {
            let picked = run(view, &entries, position, selected);
            cx.update(|cx| {
                let _ = window_handle.update(cx, |_, window, cx| {
                    on_close(picked, window, cx);
                    // The tracking loop swallowed the mouse-up; draw again so
                    // the window picks up where it was.
                    window.refresh();
                });
            });
        })
        .detach();
    }

    fn run(
        view: usize,
        entries: &[Entry],
        position: Point<Pixels>,
        selected: Option<usize>,
    ) -> Option<usize> {
        let mtm = MainThreadMarker::new()?;
        // SAFETY: the pointer is GPUI's NSView for a window that's still open:
        // this runs on the main thread right after the click that asked for
        // the menu, before the window could be closed.
        let view: &NSView = unsafe { &*(view as *const NSView) };
        let target = Target::new(mtm);
        let mut count = 0;
        let mut positioning = None;
        let menu = build(
            entries,
            &target,
            mtm,
            &mut count,
            selected,
            &mut positioning,
        );
        // GPUI's view is flipped relative to AppKit's: y grows downward.
        let height = view.bounds().size.height;
        let at = NSPoint::new(
            f32::from(position.x) as f64,
            height - f32::from(position.y) as f64,
        );
        menu.popUpMenuPositioningItem_atLocation_inView(positioning.as_deref(), at, Some(view));
        let picked = target.ivars().picked.get();
        (picked >= 0).then_some(picked as usize)
    }

    fn build(
        entries: &[Entry],
        target: &Target,
        mtm: MainThreadMarker,
        count: &mut usize,
        selected: Option<usize>,
        positioning: &mut Option<Retained<NSMenuItem>>,
    ) -> Retained<NSMenu> {
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        for entry in entries {
            match entry {
                Entry::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
                Entry::Submenu { label, entries } => {
                    let item = NSMenuItem::new(mtm);
                    item.setTitle(&NSString::from_str(label));
                    let sub = build(entries, target, mtm, count, selected, positioning);
                    item.setSubmenu(Some(&sub));
                    menu.addItem(&item);
                }
                Entry::Item(spec) => {
                    let item = NSMenuItem::new(mtm);
                    item.setTitle(&NSString::from_str(&spec.label));
                    item.setTag(*count as isize);
                    item.setEnabled(spec.enabled);
                    if spec.checked {
                        item.setState(NSControlStateValueOn);
                    }
                    if let Some(name) = spec.symbol
                        && let Some(image) =
                            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                                &NSString::from_str(name),
                                None,
                            )
                    {
                        item.setImage(Some(&image));
                    }
                    if let Some(ks) = &spec.keystroke
                        && let Some(key) = native_key(ks)
                    {
                        item.setKeyEquivalent(&NSString::from_str(&key));
                        item.setKeyEquivalentModifierMask(modifier_mask(ks));
                    }
                    if spec.enabled {
                        unsafe {
                            item.setTarget(Some(target as &AnyObject));
                            item.setAction(Some(sel!(pick:)));
                        }
                    }
                    if selected == Some(*count) {
                        *positioning = Some(item.clone());
                    }
                    *count += 1;
                    menu.addItem(&item);
                }
            }
        }
        menu
    }

    fn modifier_mask(ks: &Keystroke) -> NSEventModifierFlags {
        let m = &ks.modifiers;
        let mut mask = NSEventModifierFlags::empty();
        for (on, flag) in [
            (m.platform, NSEventModifierFlags::Command),
            (m.control, NSEventModifierFlags::Control),
            (m.alt, NSEventModifierFlags::Option),
            (m.shift, NSEventModifierFlags::Shift),
        ] {
            if on {
                mask |= flag;
            }
        }
        mask
    }

    /// The key as AppKit's key equivalents spell it.
    fn native_key(ks: &Keystroke) -> Option<String> {
        let c = match ks.key.as_str() {
            "enter" => '\r',
            "tab" => '\t',
            "space" => ' ',
            "backspace" => '\u{8}',
            "escape" => '\u{1b}',
            "delete" => '\u{7f}',
            "up" => '\u{f700}',
            "down" => '\u{f701}',
            "left" => '\u{f702}',
            "right" => '\u{f703}',
            "home" => '\u{f729}',
            "end" => '\u{f72b}',
            "pageup" => '\u{f72c}',
            "pagedown" => '\u{f72d}',
            key if key.chars().count() == 1 => return Some(key.to_owned()),
            _ => return None,
        };
        Some(c.to_string())
    }
}
