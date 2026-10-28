//! Secure Keyboard Entry: while it's on, macOS keeps other apps from
//! reading keystrokes. Edit › Secure Keyboard Entry turns it on by hand;
//! with `auto-secure-input` a password prompt in the focused pane turns it
//! on until it's answered. Like Terminal, only while Motty is active.

use gpui::{App, Global};

#[derive(Default)]
struct State {
    manual: bool,
    auto: bool,
    active: bool,
    /// What macOS was last told.
    on: bool,
}

impl Global for State {}

/// Edit › Secure Keyboard Entry.
pub fn toggle(cx: &mut App) {
    let state = cx.default_global::<State>();
    state.manual = !state.manual;
    apply(cx);
}

/// A focused pane's password prompt started (or ended).
pub fn set_auto(on: bool, cx: &mut App) {
    cx.default_global::<State>().auto = on;
    apply(cx);
}

/// Motty became (or stopped being) the active app.
pub fn set_active(active: bool, cx: &mut App) {
    cx.default_global::<State>().active = active;
    apply(cx);
}

/// Turned on by hand (the menu item's check mark).
pub fn manual(cx: &App) -> bool {
    cx.try_global::<State>().is_some_and(|s| s.manual)
}

/// On right now (the titlebar pill).
pub fn is_on(cx: &App) -> bool {
    cx.try_global::<State>().is_some_and(|s| s.on)
}

fn apply(cx: &mut App) {
    let state = cx.default_global::<State>();
    let want = (state.manual || state.auto) && state.active;
    if want == state.on {
        return;
    }
    state.on = want;
    #[cfg(target_os = "macos")]
    mac::set(want);
    cx.refresh_windows();
    cx.defer(crate::menus::refresh);
}

#[cfg(target_os = "macos")]
mod mac {
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn EnableSecureEventInput() -> i32;
        fn DisableSecureEventInput() -> i32;
    }

    /// Calls are counted by macOS, so each enable gets one disable.
    pub fn set(on: bool) {
        // SAFETY: plain Carbon calls with no arguments.
        let status = unsafe {
            if on {
                EnableSecureEventInput()
            } else {
                DisableSecureEventInput()
            }
        };
        if status != 0 {
            tracing::warn!("Secure Keyboard Entry {on}: status {status}");
        }
    }
}
