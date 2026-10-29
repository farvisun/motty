//! The Cocoa Scripting side. Cocoa evaluates a script's object references
//! with key-value coding on the classes `Motty.sdef` names:
//!
//! - the application is GPUI's NSApplication subclass, and windows are its
//!   NSWindow subclass. NSWindow brings the standard window properties
//!   (bounds, visible, miniaturized, close…); the Motty ones (`mottyTabs`,
//!   `mottyName`…) and a window's `objectSpecifier` are added to GPUI's
//!   classes before launch, as a category would.
//! - a tab is a [`TabObject`], which knows the tab by one of its panes and
//!   reads it anew each time, so a reference a script keeps follows the tab.
//! - `do script` is [`DoScriptCommand`].
//!
//! Apple Events arrive from the run loop outside GPUI's callbacks, so each
//! of these updates the app through an `AsyncApp` kept at launch.

use std::cell::RefCell;
use std::ffi::CStr;
use std::ptr;

use gpui::{App, AppContext as _, AsyncApp, WindowHandle, WindowId};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, NSObject, NSObjectProtocol, Sel};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    sel,
};
use objc2_app_kit::{NSApplication, NSView, NSWindow, NSWindowOrderingMode};
use objc2_foundation::{
    NSArray, NSNumber, NSScriptClassDescription, NSScriptCommand, NSScriptObjectSpecifier,
    NSString, NSUniqueIDSpecifier, ns_string,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::{Result, ScriptError, Target};
use crate::workspace::Workspace;

thread_local! {
    static APP: RefCell<Option<AsyncApp>> = const { RefCell::new(None) };
}

/// The element keys of the application and of a window, as in the sdef.
const WINDOWS_KEY: &str = "mottyWindows";
const TABS_KEY: &str = "mottyTabs";

/// Before the app launches: Cocoa reads the dictionary, and looks up the
/// classes it names, as soon as the first Apple Event arrives, which can be
/// the one that launched the app.
pub fn register() {
    let _ = TabObject::class();
    let _ = DoScriptCommand::class();
    // GPUI registers its classes as the program loads.
    for (name, methods) in [
        (c"GPUIApplication", app_methods()),
        (c"GPUIWindow", window_methods()),
    ] {
        match AnyClass::get(name) {
            Some(class) => add_methods(class, &methods),
            None => tracing::warn!("AppleScript: no class {name:?}"),
        }
    }
}

pub fn install(cx: &mut App) {
    APP.with(|app| *app.borrow_mut() = Some(cx.to_async()));
}

/// Runs `f` with the app. GPUI's app is a `RefCell`: an Apple Event that
/// arrives from a nested run loop (a menu or modal panel opened during an
/// update) finds it borrowed, and fails rather than panicking.
fn with_app<R>(f: impl FnOnce(&mut App) -> Result<R>) -> Result<R> {
    let app = APP
        .with(|app| app.borrow().clone())
        .ok_or_else(ScriptError::busy)?;
    if !app_free(&app) {
        return Err(ScriptError::busy());
    }
    app.update(f)
}

/// Whether the app can be updated now. GPUI has no fallible `update`, but
/// updating a window borrows the app fallibly: a window that doesn't exist
/// tells a borrowed app (`BorrowMutError`) from a free one ("not found").
fn app_free(app: &AsyncApp) -> bool {
    let nowhere: gpui::AnyWindowHandle =
        WindowHandle::<Workspace>::new(WindowId::from(u64::MAX)).into();
    match app.clone().update_window(nowhere, |_, _, _| ()) {
        Ok(()) => true,
        Err(e) => {
            e.downcast_ref::<std::cell::BorrowMutError>().is_none()
                && !e.to_string().contains("released")
        }
    }
}

/// Reports `e` as the current command's error.
fn fail(e: ScriptError) {
    if let Some(command) = NSScriptCommand::currentCommand() {
        report(&command, e);
    }
}

fn report(command: &NSScriptCommand, e: ScriptError) {
    command.setScriptErrorNumber(e.number);
    command.setScriptErrorString(Some(&NSString::from_str(&e.message)));
}

/// A getter's answer: the object, autoreleased, or nil with the error
/// reported.
fn answer<T: objc2::Message>(result: Result<Retained<T>>) -> *mut AnyObject {
    match result {
        Ok(object) => Retained::autorelease_return(object).cast(),
        Err(e) => {
            fail(e);
            ptr::null_mut()
        }
    }
}

fn number(value: Option<&AnyObject>) -> Option<i64> {
    let value = value?;
    if let Some(n) = value.downcast_ref::<NSNumber>() {
        return Some(n.as_i64());
    }
    value
        .downcast_ref::<NSString>()?
        .to_string()
        .trim()
        .parse()
        .ok()
}

fn boolean(value: Option<&AnyObject>) -> bool {
    value
        .and_then(|v| v.downcast_ref::<NSNumber>())
        .is_some_and(|n| n.as_bool())
}

/// The window's NSWindow.
fn ns_window(handle: WindowHandle<Workspace>, cx: &mut App) -> Option<Retained<NSWindow>> {
    handle
        .update(cx, |_, window, _| {
            let RawWindowHandle::AppKit(handle) =
                HasWindowHandle::window_handle(window).ok()?.as_raw()
            else {
                return None;
            };
            // SAFETY: GPUI's AppKit handle points at its live NSView, and
            // this is the main thread.
            unsafe { handle.ns_view.cast::<NSView>().as_ref() }.window()
        })
        .ok()
        .flatten()
}

/// Motty's windows, frontmost first, with their NSWindows.
fn ns_windows(cx: &mut App) -> Vec<(WindowHandle<Workspace>, Retained<NSWindow>)> {
    super::windows(cx)
        .into_iter()
        .filter_map(|handle| Some((handle, ns_window(handle, cx)?)))
        .collect()
}

/// The Motty window that `window` is.
fn handle_of(window: &NSWindow, cx: &mut App) -> Result<WindowHandle<Workspace>> {
    super::windows(cx)
        .into_iter()
        .find(|handle| ns_window(*handle, cx).is_some_and(|w| ptr::eq(&*w, window)))
        .ok_or_else(ScriptError::no_window)
}

fn tab_object(pane: u64) -> Option<Retained<TabObject>> {
    Some(TabObject::new(MainThreadMarker::new()?, pane))
}

fn tab_list(panes: Vec<u64>) -> Result<Retained<NSArray<TabObject>>> {
    let tabs: Vec<_> = panes.into_iter().filter_map(tab_object).collect();
    Ok(NSArray::from_retained_slice(&tabs))
}

fn unique_id_specifier(
    container_class: &AnyClass,
    container: Option<&NSScriptObjectSpecifier>,
    key: &NSString,
    id: &NSNumber,
) -> Option<Retained<NSScriptObjectSpecifier>> {
    // SAFETY: a class Cocoa Scripting knows from the sdef.
    let description =
        unsafe { NSScriptClassDescription::classDescriptionForClass(container_class) }?;
    // SAFETY: the unique ID is an NSNumber, as the sdef's integer `id`.
    let specifier = unsafe {
        NSUniqueIDSpecifier::initWithContainerClassDescription_containerSpecifier_key_uniqueID(
            NSUniqueIDSpecifier::alloc(),
            &description,
            container,
            key,
            id,
        )
    };
    Some(Retained::into_super(specifier))
}

/// `tab id N of window id M`, for the tab holding pane N.
fn tab_specifier(pane: u64) -> Option<Retained<NSScriptObjectSpecifier>> {
    let window = with_app(|cx| {
        let (handle, _) = super::find_tab(pane, cx)?;
        ns_window(handle, cx).ok_or_else(ScriptError::no_window)
    })
    .ok()?;
    // SAFETY: the method added to GPUI's windows in `register`.
    let container: Option<Retained<NSScriptObjectSpecifier>> =
        unsafe { msg_send![&*window, objectSpecifier] };
    let container = container?;
    unique_id_specifier(
        NSWindow::class(),
        Some(&container),
        &NSString::from_str(TABS_KEY),
        &NSNumber::new_u64(pane),
    )
}

define_class!(
    /// A scripting tab, known by one of its panes.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MottyScriptTab"]
    #[ivars = u64]
    pub(super) struct TabObject;

    unsafe impl NSObjectProtocol for TabObject {}

    impl TabObject {
        /// `tab id N of window id M`.
        #[unsafe(method_id(objectSpecifier))]
        fn object_specifier(&self) -> Option<Retained<NSScriptObjectSpecifier>> {
            tab_specifier(*self.ivars())
        }

        #[unsafe(method(mottyID))]
        fn id(&self) -> *mut AnyObject {
            answer(self.read(|tab, cx| Ok(NSNumber::new_u64(tab.pane.read(cx).id))))
        }

        #[unsafe(method(mottyRows))]
        fn rows(&self) -> *mut AnyObject {
            answer(self.read(|tab, cx| {
                Ok(NSNumber::new_u16(tab.pane.read(cx).session().size().rows))
            }))
        }

        #[unsafe(method(mottyColumns))]
        fn columns(&self) -> *mut AnyObject {
            answer(self.read(|tab, cx| {
                Ok(NSNumber::new_u16(tab.pane.read(cx).session().size().cols))
            }))
        }

        #[unsafe(method(mottyContents))]
        fn contents(&self) -> *mut AnyObject {
            let pane = *self.ivars();
            answer(with_app(|cx| super::text(pane, false, cx)).map(|t| NSString::from_str(&t)))
        }

        #[unsafe(method(mottyHistory))]
        fn history(&self) -> *mut AnyObject {
            let pane = *self.ivars();
            answer(with_app(|cx| super::text(pane, true, cx)).map(|t| NSString::from_str(&t)))
        }

        #[unsafe(method(mottyBusy))]
        fn busy(&self) -> *mut AnyObject {
            answer(self.read(|tab, cx| {
                Ok(NSNumber::new_bool(tab.pane.read(cx).session().is_busy()))
            }))
        }

        #[unsafe(method(mottyProcesses))]
        fn processes(&self) -> *mut AnyObject {
            answer(self.read(|tab, cx| {
                let names: Vec<_> = tab
                    .pane
                    .read(cx)
                    .session()
                    .processes()
                    .iter()
                    .map(|p| NSString::from_str(&p.name))
                    .collect();
                Ok(NSArray::from_retained_slice(&names))
            }))
        }

        #[unsafe(method(mottyTTY))]
        fn tty(&self) -> *mut AnyObject {
            answer(self.read(|tab, cx| {
                let tty = tab.pane.read(cx).session().tty().unwrap_or_default();
                Ok(NSString::from_str(&tty))
            }))
        }

        #[unsafe(method(mottyWorkingDirectory))]
        fn working_directory(&self) -> *mut AnyObject {
            answer(self.read(|tab, cx| {
                let session = tab.pane.read(cx).session();
                // As the pane's title has it: the shell's report (OSC 7),
                // else the foreground process's.
                let reported = session.shell().cwd.clone();
                let cwd = reported.or_else(|| session.foreground_cwd());
                Ok(NSString::from_str(
                    &cwd.map(|c| c.display().to_string()).unwrap_or_default(),
                ))
            }))
        }

        #[unsafe(method(mottyCustomTitle))]
        fn custom_title(&self) -> *mut AnyObject {
            let pane = *self.ivars();
            answer(with_app(|cx| {
                let (handle, tab) = super::find_tab(pane, cx)?;
                let workspace = handle.read(cx).map_err(|_| ScriptError::no_window())?;
                Ok(NSString::from_str(&workspace.custom_title(tab.ix)))
            }))
        }

        #[unsafe(method(setMottyCustomTitle:))]
        fn set_custom_title(&self, title: Option<&AnyObject>) {
            let pane = *self.ivars();
            let title = title
                .and_then(|t| t.downcast_ref::<NSString>())
                .map(|t| t.to_string())
                .unwrap_or_default();
            if let Err(e) = with_app(|cx| {
                let (handle, tab) = super::find_tab(pane, cx)?;
                handle
                    .update(cx, |workspace, _, cx| workspace.set_custom_title(tab.ix, &title, cx))
                    .map_err(|_| ScriptError::no_window())
            }) {
                fail(e);
            }
        }

        #[unsafe(method(mottySelected))]
        fn selected(&self) -> *mut AnyObject {
            answer(self.read(|tab, _| Ok(NSNumber::new_bool(tab.selected))))
        }

        #[unsafe(method(setMottySelected:))]
        fn set_selected(&self, selected: Option<&AnyObject>) {
            // Like Terminal: false selects nothing else.
            if boolean(selected)
                && let Err(e) = select(*self.ivars())
            {
                fail(e);
            }
        }
    }
);

impl TabObject {
    fn new(mtm: MainThreadMarker, pane: u64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(pane);
        // SAFETY: `NSObject`'s `init` on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }

    /// Reads the tab, wherever it is now.
    fn read<T: objc2::Message>(
        &self,
        f: impl FnOnce(&crate::workspace::ScriptTab, &App) -> Result<Retained<T>>,
    ) -> Result<Retained<T>> {
        let pane = *self.ivars();
        with_app(|cx| {
            let (_, tab) = super::find_tab(pane, cx)?;
            f(&tab, cx)
        })
    }
}

/// Brings the tab holding `pane` forward in its window.
fn select(pane: u64) -> Result<()> {
    with_app(|cx| {
        let (handle, tab) = super::find_tab(pane, cx)?;
        handle
            .update(cx, |workspace, window, cx| {
                workspace.select_tab(tab.ix, window, cx)
            })
            .map_err(|_| ScriptError::no_window())
    })
}

define_class!(
    /// `do script "command" [in tab | window]`.
    #[unsafe(super(NSScriptCommand, NSObject))]
    #[name = "MottyDoScriptCommand"]
    pub(super) struct DoScriptCommand;

    impl DoScriptCommand {
        #[unsafe(method_id(performDefaultImplementation))]
        fn perform(&self) -> Option<Retained<AnyObject>> {
            let command: &NSScriptCommand = self;
            let arguments = command.evaluatedArguments();
            let argument = |key: &NSString| arguments.as_ref().and_then(|a| a.objectForKey(key));
            let text = command
                .directParameter()
                .or_else(|| argument(ns_string!("Command")))
                .and_then(|t| t.downcast::<NSString>().ok())
                .map(|t| t.to_string())
                .unwrap_or_default();
            let target = argument(ns_string!("Target"));
            let result = with_app(|cx| {
                let target = target.as_deref();
                let target = if let Some(tab) = target.and_then(|t| t.downcast_ref::<TabObject>()) {
                    Target::Tab(*tab.ivars())
                } else if let Some(window) = target.and_then(|t| t.downcast_ref::<NSWindow>()) {
                    Target::Window(handle_of(window, cx)?)
                } else {
                    Target::NewWindow
                };
                super::do_script(&text, target, cx)
            });
            match result.and_then(|pane| tab_object(pane).ok_or_else(ScriptError::busy)) {
                Ok(tab) => Some(Retained::into_super(Retained::into_super(tab))),
                Err(e) => {
                    report(command, e);
                    None
                }
            }
        }
    }
);

type Getter = unsafe extern "C-unwind" fn(&AnyObject, Sel) -> *mut AnyObject;
type Setter = unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject);
type Lookup =
    unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject, *mut AnyObject) -> *mut AnyObject;

/// A method to add to a GPUI class: selector, implementation, types.
pub(super) type Method = (Sel, Imp, &'static CStr);

fn getter(sel: Sel, f: Getter) -> Method {
    // SAFETY: called back with the signature the types say.
    (
        sel,
        unsafe { std::mem::transmute::<Getter, Imp>(f) },
        c"@@:",
    )
}

fn setter(sel: Sel, f: Setter) -> Method {
    // SAFETY: as above.
    (
        sel,
        unsafe { std::mem::transmute::<Setter, Imp>(f) },
        c"v@:@",
    )
}

fn lookup(f: Lookup) -> Method {
    // SAFETY: as above.
    (
        sel!(valueWithUniqueID:inPropertyWithKey:),
        unsafe { std::mem::transmute::<Lookup, Imp>(f) },
        c"@@:@@",
    )
}

fn add_methods(class: &AnyClass, methods: &[Method]) {
    for &(sel, imp, types) in methods {
        // SAFETY: `imp` has the signature `types` describes; adding a
        // method a class doesn't define yet only overrides its superclass's.
        let added = unsafe {
            objc2::ffi::class_addMethod(ptr::from_ref(class).cast_mut(), sel, imp, types.as_ptr())
        };
        if !added.as_bool() {
            tracing::warn!(
                "AppleScript: {} already has {sel}",
                class.name().to_string_lossy()
            );
        }
    }
}

/// What the application answers besides NSApplication's own.
pub(super) fn app_methods() -> Vec<Method> {
    vec![
        getter(sel!(mottyName), app_name),
        getter(sel!(mottyVersion), app_version),
        getter(sel!(mottyWindows), app_windows),
        lookup(app_window_with_id),
    ]
}

/// What windows answer besides NSWindow's own.
pub(super) fn window_methods() -> Vec<Method> {
    vec![
        getter(sel!(objectSpecifier), window_specifier),
        getter(sel!(mottyName), window_name),
        getter(sel!(mottyID), window_id),
        getter(sel!(mottyIndex), window_index),
        setter(sel!(setMottyIndex:), set_window_index),
        getter(sel!(mottyFrontmost), window_frontmost),
        setter(sel!(setMottyFrontmost:), set_window_frontmost),
        getter(sel!(mottyTabs), window_tabs),
        getter(sel!(mottySelectedTab), window_selected_tab),
        setter(sel!(setMottySelectedTab:), set_window_selected_tab),
        lookup(window_tab_with_id),
    ]
}

unsafe extern "C-unwind" fn app_name(_: &AnyObject, _: Sel) -> *mut AnyObject {
    answer(Ok(NSString::from_str("Motty")))
}

unsafe extern "C-unwind" fn app_version(_: &AnyObject, _: Sel) -> *mut AnyObject {
    answer(Ok(NSString::from_str(env!("CARGO_PKG_VERSION"))))
}

unsafe extern "C-unwind" fn app_windows(_: &AnyObject, _: Sel) -> *mut AnyObject {
    answer(with_app(|cx| {
        let windows: Vec<_> = ns_windows(cx).into_iter().map(|(_, w)| w).collect();
        Ok(NSArray::from_retained_slice(&windows))
    }))
}

/// `window id N`.
unsafe extern "C-unwind" fn app_window_with_id(
    _: &AnyObject,
    _: Sel,
    id: *mut AnyObject,
    key: *mut AnyObject,
) -> *mut AnyObject {
    // SAFETY: Cocoa passes objects or nil.
    let (id, key) = unsafe { (id.as_ref(), key.as_ref()) };
    if !is_key(key, WINDOWS_KEY) {
        return ptr::null_mut();
    }
    let Some(id) = number(id) else {
        return ptr::null_mut();
    };
    match with_app(|cx| Ok(ns_windows(cx))) {
        Ok(windows) => windows
            .into_iter()
            .find(|(_, w)| w.windowNumber() as i64 == id)
            .map_or(ptr::null_mut(), |(_, w)| {
                Retained::autorelease_return(w).cast()
            }),
        Err(e) => {
            fail(e);
            ptr::null_mut()
        }
    }
}

fn is_key(key: Option<&AnyObject>, name: &str) -> bool {
    key.and_then(|k| k.downcast_ref::<NSString>())
        .is_some_and(|k| k.to_string() == name)
}

fn as_window(this: &AnyObject) -> Result<&NSWindow> {
    this.downcast_ref::<NSWindow>()
        .ok_or_else(ScriptError::no_window)
}

/// `window id N`.
unsafe extern "C-unwind" fn window_specifier(this: &AnyObject, _: Sel) -> *mut AnyObject {
    let Ok(window) = as_window(this) else {
        return ptr::null_mut();
    };
    let specifier = unique_id_specifier(
        NSApplication::class(),
        None,
        &NSString::from_str(WINDOWS_KEY),
        &NSNumber::new_isize(window.windowNumber()),
    );
    specifier.map_or(ptr::null_mut(), |s| Retained::autorelease_return(s).cast())
}

unsafe extern "C-unwind" fn window_name(this: &AnyObject, _: Sel) -> *mut AnyObject {
    answer(with_app(|cx| {
        let handle = handle_of(as_window(this)?, cx)?;
        let workspace = handle.read(cx).map_err(|_| ScriptError::no_window())?;
        Ok(NSString::from_str(&workspace.title(cx)))
    }))
}

unsafe extern "C-unwind" fn window_id(this: &AnyObject, _: Sel) -> *mut AnyObject {
    answer(as_window(this).map(|w| NSNumber::new_isize(w.windowNumber())))
}

/// Where the window is among Motty's, from 1.
fn position(window: &NSWindow, cx: &mut App) -> Result<usize> {
    ns_windows(cx)
        .iter()
        .position(|(_, w)| ptr::eq(&**w, window))
        .map(|i| i + 1)
        .ok_or_else(ScriptError::no_window)
}

unsafe extern "C-unwind" fn window_index(this: &AnyObject, _: Sel) -> *mut AnyObject {
    answer(with_app(|cx| {
        Ok(NSNumber::new_usize(position(as_window(this)?, cx)?))
    }))
}

/// Moves the window to `index` among Motty's; 1 brings it to the front.
unsafe extern "C-unwind" fn set_window_index(this: &AnyObject, _: Sel, index: *mut AnyObject) {
    // SAFETY: Cocoa passes an object or nil.
    let index = number(unsafe { index.as_ref() }).unwrap_or(1).max(1) as usize;
    let result = as_window(this).and_then(|window| {
        let others: Vec<_> = with_app(|cx| Ok(ns_windows(cx)))?
            .into_iter()
            .map(|(_, w)| w)
            .filter(|w| !ptr::eq(&**w, window))
            .collect();
        // Below the window that's to be above it; past the end, last.
        let above = (index > 1)
            .then(|| others.get(index - 2).or(others.last()))
            .flatten();
        match above {
            Some(above) => {
                window.orderWindow_relativeTo(NSWindowOrderingMode::Below, above.windowNumber())
            }
            None => window.makeKeyAndOrderFront(None),
        }
        Ok(())
    });
    if let Err(e) = result {
        fail(e);
    }
}

unsafe extern "C-unwind" fn window_frontmost(this: &AnyObject, _: Sel) -> *mut AnyObject {
    answer(with_app(|cx| {
        Ok(NSNumber::new_bool(position(as_window(this)?, cx)? == 1))
    }))
}

unsafe extern "C-unwind" fn set_window_frontmost(this: &AnyObject, _: Sel, value: *mut AnyObject) {
    // SAFETY: Cocoa passes an object or nil.
    if boolean(unsafe { value.as_ref() })
        && let Ok(window) = as_window(this)
    {
        window.makeKeyAndOrderFront(None);
    }
}

unsafe extern "C-unwind" fn window_tabs(this: &AnyObject, _: Sel) -> *mut AnyObject {
    answer(
        with_app(|cx| {
            let handle = handle_of(as_window(this)?, cx)?;
            super::tabs(handle, cx)
        })
        .and_then(tab_list),
    )
}

unsafe extern "C-unwind" fn window_selected_tab(this: &AnyObject, _: Sel) -> *mut AnyObject {
    answer(
        with_app(|cx| {
            let handle = handle_of(as_window(this)?, cx)?;
            let workspace = handle.read(cx).map_err(|_| ScriptError::no_window())?;
            let tab = workspace.script_selected_tab().ok_or_else(|| {
                ScriptError::new(super::NO_SUCH_OBJECT, "No terminal tab is selected.")
            })?;
            Ok(tab.pane.read(cx).id)
        })
        .and_then(|pane| tab_object(pane).ok_or_else(ScriptError::busy)),
    )
}

unsafe extern "C-unwind" fn set_window_selected_tab(_: &AnyObject, _: Sel, tab: *mut AnyObject) {
    // SAFETY: Cocoa passes an object or nil.
    let tab = unsafe { tab.as_ref() }.and_then(|t| t.downcast_ref::<TabObject>());
    let result = match tab {
        Some(tab) => select(*tab.ivars()),
        None => Err(ScriptError::no_tab()),
    };
    if let Err(e) = result {
        fail(e);
    }
}

/// `tab id N`: the tab holding pane N, whichever of its panes has focus.
unsafe extern "C-unwind" fn window_tab_with_id(
    this: &AnyObject,
    _: Sel,
    id: *mut AnyObject,
    key: *mut AnyObject,
) -> *mut AnyObject {
    // SAFETY: Cocoa passes objects or nil.
    let (id, key) = unsafe { (id.as_ref(), key.as_ref()) };
    if !is_key(key, TABS_KEY) {
        return ptr::null_mut();
    }
    let Some(pane) = number(id).and_then(|id| u64::try_from(id).ok()) else {
        return ptr::null_mut();
    };
    let found = with_app(|cx| {
        let handle = handle_of(as_window(this)?, cx)?;
        let (holder, _) = super::find_tab(pane, cx)?;
        Ok(holder == handle)
    });
    match found {
        Ok(true) => {
            tab_object(pane).map_or(ptr::null_mut(), |t| Retained::autorelease_return(t).cast())
        }
        _ => ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use super::*;

    const SDEF: &str = include_str!("../../resources/Motty.sdef");

    fn cocoa<'a>(node: roxmltree::Node<'a, 'a>, attribute: &str) -> Option<&'a str> {
        node.children()
            .find(|c| c.has_tag_name("cocoa"))?
            .attribute(attribute)
    }

    fn selector(name: &str) -> Sel {
        Sel::register(&CString::new(name).unwrap())
    }

    /// KVC's setter for `key`: `setKey:`.
    fn setter_name(key: &str) -> String {
        let mut chars = key.chars();
        let first = chars.next().unwrap().to_uppercase();
        format!("set{first}{}:", chars.as_str())
    }

    /// Every cocoa key and method the dictionary names is answered: by
    /// AppKit's class, by a method added to GPUI's, or by ours.
    #[test]
    fn dictionary_keys_exist() {
        let _ = TabObject::class();
        let _ = DoScriptCommand::class();
        let options = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(SDEF, options).expect("valid XML");
        let mut checked = 0;
        for class in doc.descendants().filter(|n| n.has_tag_name("class")) {
            let name = cocoa(class, "class").expect("a cocoa class");
            let (runtime, added) = match name {
                "NSApplication" => (NSApplication::class(), app_methods()),
                "NSWindow" => (NSWindow::class(), window_methods()),
                "MottyScriptTab" => (TabObject::class(), Vec::new()),
                other => panic!("unexpected class {other}"),
            };
            let answers = |sel: &str| {
                let sel = selector(sel);
                runtime.responds_to(sel) || added.iter().any(|(s, _, _)| *s == sel)
            };
            for item in class.children().filter(|c| c.is_element()) {
                let mut wanted = Vec::new();
                match item.tag_name().name() {
                    "property" | "contents" => {
                        let key = cocoa(item, "key").expect("a cocoa key");
                        wanted.push(key.to_owned());
                        if item.attribute("access") != Some("r") {
                            wanted.push(setter_name(key));
                        }
                    }
                    "element" => wanted.push(cocoa(item, "key").expect("a cocoa key").to_owned()),
                    "responds-to" => {
                        wanted.push(cocoa(item, "method").expect("a cocoa method").to_owned())
                    }
                    _ => {}
                }
                for sel in wanted {
                    assert!(answers(&sel), "{name} doesn't answer {sel}");
                    checked += 1;
                }
            }
            // Cocoa asks for this to name the objects it returns.
            if name != "NSApplication" {
                assert!(answers("objectSpecifier"), "{name} has no objectSpecifier");
            }
        }
        assert!(checked > 30, "only {checked} keys");
        for command in doc.descendants().filter(|n| n.has_tag_name("command")) {
            let class = cocoa(command, "class").expect("a command class");
            let class = CString::new(class).unwrap();
            assert!(AnyClass::get(&class).is_some(), "no class {class:?}");
        }
    }

    #[test]
    fn element_keys_match() {
        assert!(SDEF.contains(&format!("<cocoa key=\"{WINDOWS_KEY}\"/>")));
        assert!(SDEF.contains(&format!("<cocoa key=\"{TABS_KEY}\"/>")));
    }
}
