//! The Quick Terminal: a Motty window that drops down from the top of the
//! screen on a system-wide shortcut (`quick-terminal-key`, ctrl+` unless
//! set; empty turns it off), from any app. The same shortcut hides it, and
//! it hides when another app or window takes over, unless pinned (View ›
//! Pin Window). It isn't saved with the session.
//!
//! The shortcut is a Carbon hot key, which needs no Accessibility
//! permission.

use std::sync::OnceLock;

use gpui::{App, Bounds, Global, WindowHandle, point, px, size};

use crate::workspace::Workspace;

/// The share of the screen's height it takes.
const HEIGHT: f32 = 0.45;

/// The window, once opened.
#[derive(Default)]
struct Quick(Option<WindowHandle<Workspace>>);

impl Global for Quick {}

/// Hot key presses, from Carbon's handler to the app.
static PRESSES: OnceLock<async_channel::Sender<()>> = OnceLock::new();

/// A shortcut as Carbon takes it: a virtual key code and modifier bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotKey {
    pub code: u32,
    pub modifiers: u32,
}

// Carbon's modifier bits (Events.h).
const CMD: u32 = 1 << 8;
const SHIFT: u32 = 1 << 9;
const OPTION: u32 = 1 << 11;
const CONTROL: u32 = 1 << 12;

/// Reads `ctrl+\``, `cmd+shift+space`, `alt+f12`. `None` for empty or
/// unknown keys, and for a key without a modifier (but F-keys).
pub fn parse(spec: &str) -> Option<HotKey> {
    let mut modifiers = 0;
    let mut key = None;
    for part in spec.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "super" => modifiers |= CMD,
            "shift" => modifiers |= SHIFT,
            "alt" | "opt" | "option" => modifiers |= OPTION,
            "ctrl" | "control" => modifiers |= CONTROL,
            k => key = Some(key_code(k)?),
        }
    }
    // `ctrl+` with a backquote typed as the key itself splits oddly.
    if key.is_none() && spec.trim_end().ends_with("+`") {
        key = key_code("`");
    }
    let (code, function) = key?;
    (modifiers != 0 || function).then_some(HotKey { code, modifiers })
}

/// A key name's virtual key code (US layout), and whether it's an F-key.
fn key_code(name: &str) -> Option<(u32, bool)> {
    const LETTERS: [u32; 26] = [
        0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F,
        0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
    ];
    const DIGITS: [u32; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
    const F_KEYS: [u32; 12] = [
        0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F,
    ];
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_lowercase() {
            return Some((LETTERS[(c as u8 - b'a') as usize], false));
        }
        if let Some(d) = c.to_digit(10) {
            return Some((DIGITS[d as usize], false));
        }
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<usize>().ok())
        && (1..=12).contains(&n)
    {
        return Some((F_KEYS[n - 1], true));
    }
    Some((
        match name {
            "`" | "grave" | "backquote" => 0x32,
            "space" => 0x31,
            "escape" | "esc" => 0x35,
            "tab" => 0x30,
            "return" | "enter" => 0x24,
            "-" | "minus" => 0x1B,
            "=" | "equal" => 0x18,
            "\\" | "backslash" => 0x2A,
            ";" | "semicolon" => 0x29,
            "'" | "quote" => 0x27,
            "," | "comma" => 0x2B,
            "." | "period" => 0x2F,
            "/" | "slash" => 0x2C,
            _ => return None,
        },
        false,
    ))
}

/// Starts listening for the shortcut presses; [`register`] sets it.
pub fn install(cx: &mut App) {
    let (tx, rx) = async_channel::unbounded::<()>();
    if PRESSES.set(tx).is_err() {
        return;
    }
    cx.spawn(async move |cx| {
        while rx.recv().await.is_ok() {
            cx.update(toggle);
        }
    })
    .detach();
}

/// Sets the shortcut from `quick-terminal-key`; an empty one turns it off.
pub fn register(spec: &str) {
    let key = parse(spec);
    if key.is_none() && !spec.trim().is_empty() {
        tracing::warn!("quick-terminal-key: can't read {spec:?}");
    }
    #[cfg(target_os = "macos")]
    mac::register(key);
    #[cfg(not(target_os = "macos"))]
    let _ = key;
}

/// Shows the Quick Terminal (opening it the first time), or hides it when
/// it's in front.
pub fn toggle(cx: &mut App) {
    let existing = cx.try_global::<Quick>().and_then(|q| q.0);
    if let Some(handle) = existing {
        let shown = handle.update(cx, |workspace, window, cx| {
            if window.is_window_active() {
                crate::materials::hide(window);
            } else {
                show(workspace, window, cx);
            }
        });
        if shown.is_ok() {
            return;
        }
        // Closed since: open a new one.
    }
    match open(cx) {
        Ok(handle) => cx.set_global(Quick(Some(handle))),
        Err(e) => tracing::error!("couldn't open the Quick Terminal: {e:#}"),
    }
}

/// Where it drops down: the top of the main screen, full width.
fn frame(cx: &App) -> Bounds<gpui::Pixels> {
    let screen = cx
        .primary_display()
        .map(|d| d.bounds())
        .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(1440.), px(900.))));
    Bounds::new(
        screen.origin,
        size(screen.size.width, screen.size.height * HEIGHT),
    )
}

fn show(_: &mut Workspace, window: &mut gpui::Window, cx: &mut gpui::Context<Workspace>) {
    cx.activate(true);
    window.activate_window();
}

fn open(cx: &mut App) -> anyhow::Result<WindowHandle<Workspace>> {
    let config = crate::load_config();
    let handle = crate::open_window_at(config, frame(cx), cx)?;
    handle.update(cx, |workspace, window, cx| {
        workspace.quick = true;
        crate::materials::quick_terminal(window);
        cx.activate(true);
        window.activate_window();
    })?;
    Ok(handle)
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::sync::Mutex;

    use super::{HotKey, PRESSES};

    #[repr(C)]
    struct EventTypeSpec {
        class: u32,
        kind: u32,
    }

    #[repr(C)]
    struct EventHotKeyId {
        signature: u32,
        id: u32,
    }

    type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn GetApplicationEventTarget() -> *mut c_void;
        fn InstallEventHandler(
            target: *mut c_void,
            handler: Handler,
            count: u32,
            types: *const EventTypeSpec,
            user_data: *mut c_void,
            out: *mut *mut c_void,
        ) -> i32;
        fn RegisterEventHotKey(
            code: u32,
            modifiers: u32,
            id: EventHotKeyId,
            target: *mut c_void,
            options: u32,
            out: *mut *mut c_void,
        ) -> i32;
        fn UnregisterEventHotKey(hot_key: *mut c_void) -> i32;
    }

    /// `'keyb'` and kEventHotKeyPressed.
    const KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
    const HOT_KEY_PRESSED: u32 = 5;
    /// `'mtqt'`: ours.
    const SIGNATURE: u32 = u32::from_be_bytes(*b"mtqt");

    /// The registered hot key (an EventHotKeyRef), as an address.
    static CURRENT: Mutex<Option<usize>> = Mutex::new(None);
    static HANDLER: std::sync::Once = std::sync::Once::new();

    extern "C" fn pressed(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> i32 {
        if let Some(tx) = PRESSES.get() {
            let _ = tx.try_send(());
        }
        0
    }

    pub fn register(key: Option<HotKey>) {
        HANDLER.call_once(|| {
            let spec = EventTypeSpec {
                class: KEYBOARD,
                kind: HOT_KEY_PRESSED,
            };
            // SAFETY: the application target lives as long as the app;
            // the handler is a plain function and the spec is read here.
            let status = unsafe {
                InstallEventHandler(
                    GetApplicationEventTarget(),
                    pressed,
                    1,
                    &spec,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status != 0 {
                tracing::warn!("Quick Terminal: InstallEventHandler failed ({status})");
            }
        });
        let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = current.take() {
            // SAFETY: a hot key this module registered and still holds.
            unsafe { UnregisterEventHotKey(old as *mut c_void) };
        }
        let Some(key) = key else {
            return;
        };
        let mut out = std::ptr::null_mut();
        let id = EventHotKeyId {
            signature: SIGNATURE,
            id: 1,
        };
        // SAFETY: plain Carbon call; `out` receives the new hot key.
        let status = unsafe {
            RegisterEventHotKey(
                key.code,
                key.modifiers,
                id,
                GetApplicationEventTarget(),
                0,
                &mut out,
            )
        };
        if status == 0 && !out.is_null() {
            *current = Some(out as usize);
        } else {
            tracing::warn!("Quick Terminal: the shortcut is taken ({status})");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_shortcuts() {
        assert_eq!(
            parse("ctrl+`"),
            Some(HotKey {
                code: 0x32,
                modifiers: CONTROL
            })
        );
        assert_eq!(
            parse("cmd+shift+space"),
            Some(HotKey {
                code: 0x31,
                modifiers: CMD | SHIFT
            })
        );
        assert_eq!(parse("alt+t").map(|k| k.code), Some(0x11));
        assert_eq!(parse("f12").map(|k| k.modifiers), Some(0));
        // A plain key would take it from every app; unknown keys are off.
        assert_eq!(parse("t"), None);
        assert_eq!(parse("ctrl+nope"), None);
        assert_eq!(parse(""), None);
    }
}
