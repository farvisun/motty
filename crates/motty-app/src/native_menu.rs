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
    /// A section title (`WORKING DIRECTORY`); not an item.
    Header(SharedString),
    /// A greyed-out line with an SF Symbol drawn in it (a folder's path);
    /// not an item.
    Info {
        label: SharedString,
        symbol: &'static str,
    },
    /// Rename Tab's Name/Prefix switch, reset button and text field, as
    /// the first row of Otty's title menu; not an item.
    Name(NameField),
}

/// The tab's name as the field shows it, and as it's left when the menu
/// closes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NameField {
    pub text: String,
    /// "Prefix" rather than "Name".
    pub prefix: bool,
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
    popup_named(
        entries,
        position,
        selected,
        None,
        window,
        cx,
        |picked, _, window, cx| on_close(picked, window, cx),
    );
}

/// [`popup_then`] for a menu with an [`Entry::Name`] field: `on_close`
/// also gets the field's value when it was changed (or reset to empty).
/// `dark` draws it dark or light rather than as the system is.
pub fn popup_named(
    entries: Vec<Entry>,
    position: Point<Pixels>,
    selected: Option<usize>,
    dark: Option<bool>,
    window: &mut Window,
    cx: &mut App,
    on_close: impl FnOnce(Option<usize>, Option<NameField>, &mut Window, &mut App) + 'static,
) {
    #[cfg(target_os = "macos")]
    mac::popup(entries, position, selected, dark, window, cx, on_close);
    #[cfg(not(target_os = "macos"))]
    on_close(None, None, window, cx);
    #[cfg(not(target_os = "macos"))]
    let _ = (entries, position, selected, dark);
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::{Cell, RefCell};

    use gpui::{App, Keystroke, Pixels, Point, Window};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSAttributedStringAttachmentConveniences, NSAutoresizingMaskOptions, NSButton, NSColor,
        NSControlStateValueOn, NSEventModifierFlags, NSFont, NSFontAttributeName,
        NSForegroundColorAttributeName, NSImage, NSMenu, NSMenuItem, NSSegmentSwitchTracking,
        NSSegmentedControl, NSTextAttachment, NSTextField, NSTextFieldBezelStyle, NSView,
    };
    use objc2_foundation::{
        NSArray, NSAttributedString, NSMutableAttributedString, NSPoint, NSRange, NSRect, NSSize,
        NSString,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::{Entry, NameField};

    struct Ivars {
        picked: Cell<isize>,
        /// The name field's controls, read when the menu closes.
        name: RefCell<Option<(Retained<NSSegmentedControl>, Retained<NSTextField>)>>,
        reset: Cell<bool>,
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

            /// Return in the name field: done.
            #[unsafe(method(nameEntered:))]
            fn name_entered(&self, sender: &NSView) {
                close_menu(sender);
            }

            /// The reset button: back to the automatic title.
            #[unsafe(method(resetName:))]
            fn reset_name(&self, sender: &NSView) {
                self.ivars().reset.set(true);
                close_menu(sender);
            }

            #[unsafe(method(nameMode:))]
            fn name_mode(&self, sender: &NSSegmentedControl) {
                if let Some((_, field)) = self.ivars().name.borrow().as_ref() {
                    field.setPlaceholderString(Some(&NSString::from_str(placeholder(
                        sender.selectedSegment() == 1,
                    ))));
                }
            }
        }
    );

    fn close_menu(view: &NSView) {
        // SAFETY: the item belongs to the menu being tracked, which outlives
        // this action.
        if let Some(menu) = view
            .enclosingMenuItem()
            .and_then(|item| unsafe { item.menu() })
        {
            menu.cancelTracking();
        }
    }

    fn placeholder(prefix: bool) -> &'static str {
        if prefix { "Prefix" } else { "Tab name" }
    }

    impl Target {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(Ivars {
                picked: Cell::new(-1),
                name: RefCell::new(None),
                reset: Cell::new(false),
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    pub(super) fn popup(
        entries: Vec<Entry>,
        position: Point<Pixels>,
        selected: Option<usize>,
        dark: Option<bool>,
        window: &mut Window,
        cx: &mut App,
        on_close: impl FnOnce(Option<usize>, Option<NameField>, &mut Window, &mut App) + 'static,
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
            let (picked, name) = run(view, &entries, position, selected, dark);
            cx.update(|cx| {
                let _ = window_handle.update(cx, |_, window, cx| {
                    on_close(picked, name, window, cx);
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
        dark: Option<bool>,
    ) -> (Option<usize>, Option<NameField>) {
        let Some(mtm) = MainThreadMarker::new() else {
            return (None, None);
        };
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
        if let Some(dark) = dark {
            // SAFETY: the appearance names are AppKit constants.
            let name = unsafe {
                if dark {
                    NSAppearanceNameDarkAqua
                } else {
                    NSAppearanceNameAqua
                }
            };
            menu.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
        }
        // GPUI's view is flipped relative to AppKit's: y grows downward.
        let height = view.bounds().size.height;
        let at = NSPoint::new(
            f32::from(position.x) as f64,
            height - f32::from(position.y) as f64,
        );
        menu.popUpMenuPositioningItem_atLocation_inView(positioning.as_deref(), at, Some(view));
        let picked = target.ivars().picked.get();
        let name = target
            .ivars()
            .name
            .borrow()
            .as_ref()
            .and_then(|(mode, field)| {
                let before = entries.iter().find_map(|e| match e {
                    Entry::Name(n) => Some(n.clone()),
                    _ => None,
                })?;
                let after = if target.ivars().reset.get() {
                    NameField::default()
                } else {
                    NameField {
                        text: field.stringValue().to_string(),
                        prefix: mode.selectedSegment() == 1,
                    }
                };
                (after != before).then_some(after)
            });
        ((picked >= 0).then_some(picked as usize), name)
    }

    /// Otty's title menu's first row: Name | Prefix, a reset button, and
    /// the text field.
    fn name_view(spec: &NameField, target: &Target, mtm: MainThreadMarker) -> Retained<NSView> {
        const W: f64 = 280.;
        const H: f64 = 74.;
        let rect =
            |x: f64, y: f64, w: f64, h: f64| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., W, H));
        // The menu widens the row to fit its longest item; the field and
        // the reset button follow.
        view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let labels = NSArray::from_retained_slice(&[
            NSString::from_str("Name"),
            NSString::from_str("Prefix"),
        ]);
        let mode = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &labels,
                NSSegmentSwitchTracking::SelectOne,
                Some(target as &AnyObject),
                Some(sel!(nameMode:)),
                mtm,
            )
        };
        mode.setFrame(rect(16., H - 8. - 24., 140., 24.));
        mode.setWidth_forSegment(68., 0);
        mode.setWidth_forSegment(68., 1);
        mode.setSelectedSegment(if spec.prefix { 1 } else { 0 });
        view.addSubview(&mode);

        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("arrow.uturn.backward"),
            Some(&NSString::from_str("Reset")),
        ) {
            let reset = unsafe {
                NSButton::buttonWithImage_target_action(
                    &image,
                    Some(target as &AnyObject),
                    Some(sel!(resetName:)),
                    mtm,
                )
            };
            reset.setBordered(false);
            reset.setFrame(rect(W - 16. - 22., H - 8. - 23., 22., 22.));
            reset.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            view.addSubview(&reset);
        }

        let field = NSTextField::textFieldWithString(&NSString::from_str(&spec.text), mtm);
        field.setPlaceholderString(Some(&NSString::from_str(placeholder(spec.prefix))));
        field.setBezeled(true);
        field.setBezelStyle(NSTextFieldBezelStyle::RoundedBezel);
        field.setFont(Some(&NSFont::systemFontOfSize(13.)));
        field.setFrame(rect(15., 8., W - 30., 28.));
        field.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        unsafe {
            field.setTarget(Some(target as &AnyObject));
            field.setAction(Some(sel!(nameEntered:)));
        }
        view.addSubview(&field);
        *target.ivars().name.borrow_mut() = Some((mode, field));
        view
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
                Entry::Header(title) => {
                    menu.addItem(&NSMenuItem::sectionHeaderWithTitle(
                        &NSString::from_str(title),
                        mtm,
                    ));
                }
                Entry::Info { label, symbol } => {
                    let item = NSMenuItem::new(mtm);
                    item.setEnabled(false);
                    item.setAttributedTitle(Some(&info_title(label, symbol)));
                    menu.addItem(&item);
                }
                Entry::Name(spec) => {
                    let item = NSMenuItem::new(mtm);
                    item.setView(Some(&name_view(spec, target, mtm)));
                    menu.addItem(&item);
                }
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

    /// `label` after `symbol`, in the menu's font and the secondary label
    /// colour. Menus here don't draw items' images, so the symbol goes into
    /// the title.
    fn info_title(label: &str, symbol: &str) -> Retained<NSAttributedString> {
        let text = NSMutableAttributedString::new();
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            None,
        ) {
            let attachment = NSTextAttachment::new();
            attachment.setImage(Some(&image));
            text.appendAttributedString(&NSAttributedString::attributedStringWithAttachment(
                &attachment,
            ));
            text.appendAttributedString(&NSAttributedString::from_nsstring(&NSString::from_str(
                "  ",
            )));
        }
        text.appendAttributedString(&NSAttributedString::from_nsstring(&NSString::from_str(
            label,
        )));
        let range = NSRange::new(0, text.length());
        // SAFETY: the values have the types these keys take.
        unsafe {
            text.addAttribute_value_range(NSFontAttributeName, &NSFont::menuFontOfSize(0.), range);
            text.addAttribute_value_range(
                NSForegroundColorAttributeName,
                &NSColor::secondaryLabelColor(),
                range,
            );
        }
        Retained::into_super(text)
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
