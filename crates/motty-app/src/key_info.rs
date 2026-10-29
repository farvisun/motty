//! What AppKit knows about the key being handled that GPUI doesn't pass
//! on, for the kitty keyboard protocol: keypad keys, the unshifted and
//! shifted characters (GPUI turns ⇧1 into `!` without Shift), the key on a
//! US layout, Caps Lock, and which side's modifier key changed.
//!
//! Read from `NSApp.currentEvent`, which during GPUI's key handlers is the
//! event being handled.

/// The key event being handled.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeKey {
    /// AppKit's virtual key code, the same for press and release.
    pub code: u16,
    pub keypad: bool,
    pub shift: bool,
    pub caps_lock: bool,
    pub unshifted: Option<char>,
    pub shifted: Option<char>,
    pub base_layout: Option<char>,
}

/// The key being pressed or released, if a key event is being handled.
#[cfg(target_os = "macos")]
pub fn current() -> Option<NativeKey> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSEventType};

    let mtm = MainThreadMarker::new()?;
    let event = NSApplication::sharedApplication(mtm).currentEvent()?;
    let ty = event.r#type();
    if ty != NSEventType::KeyDown && ty != NSEventType::KeyUp {
        return None;
    }
    let code = event.keyCode();
    let flags = event.modifierFlags();
    let char_with = |mods| {
        let text = event.charactersByApplyingModifiers(mods)?.to_string();
        let mut chars = text.chars();
        let c = chars.next()?;
        (chars.next().is_none() && !c.is_control() && !is_function_key(c)).then_some(c)
    };
    Some(NativeKey {
        code,
        keypad: is_keypad(code),
        shift: flags.contains(NSEventModifierFlags::Shift),
        caps_lock: flags.contains(NSEventModifierFlags::CapsLock),
        unshifted: char_with(NSEventModifierFlags::empty()),
        shifted: char_with(NSEventModifierFlags::Shift),
        base_layout: us_layout(code),
    })
}

#[cfg(not(target_os = "macos"))]
pub fn current() -> Option<NativeKey> {
    None
}

/// The modifier key whose change is being handled (one of
/// `motty_term::input::MODIFIER_KEYS`), and whether it went down. Caps
/// Lock is "down" while it's on.
#[cfg(target_os = "macos")]
pub fn modifier_change() -> Option<(&'static str, bool)> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEventType};

    let mtm = MainThreadMarker::new()?;
    let event = NSApplication::sharedApplication(mtm).currentEvent()?;
    if event.r#type() != NSEventType::FlagsChanged {
        return None;
    }
    // The device-dependent bits (IOKit's NX_DEVICE*KEYMASK) tell the left
    // key from the right one.
    let (name, mask): (&str, usize) = match event.keyCode() {
        0x38 => ("left_shift", 0x2),
        0x3C => ("right_shift", 0x4),
        0x3B => ("left_control", 0x1),
        0x3E => ("right_control", 0x2000),
        0x3A => ("left_alt", 0x20),
        0x3D => ("right_alt", 0x40),
        0x37 => ("left_super", 0x8),
        0x36 => ("right_super", 0x10),
        0x39 => ("caps_lock", 1 << 16),
        _ => return None,
    };
    Some((name, event.modifierFlags().0 & mask != 0))
}

#[cfg(not(target_os = "macos"))]
pub fn modifier_change() -> Option<(&'static str, bool)> {
    None
}

/// AppKit's private-use characters for arrows, F-keys and the like.
fn is_function_key(c: char) -> bool {
    ('\u{F700}'..='\u{F8FF}').contains(&c)
}

/// kVK_ANSI_Keypad*.
fn is_keypad(code: u16) -> bool {
    matches!(
        code,
        0x41 | 0x43 | 0x45 | 0x4B | 0x4C | 0x4E | 0x51 | 0x52..=0x59 | 0x5B | 0x5C
    )
}

/// What the key types on a US keyboard (kVK_ANSI_*), for the kitty
/// protocol's base layout key.
fn us_layout(code: u16) -> Option<char> {
    // Return and Tab (0x24, 0x30) aren't text keys.
    const KEYS: &[u8; 0x33] = b"asdfhgzxcv\0bqweryt123465=97-80]ou[ip\rlj'k;\\,/nm.\t `";
    match KEYS.get(code as usize) {
        Some(&b) if !b.is_ascii_control() => Some(b as char),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn us_layout_by_key_code() {
        assert_eq!(us_layout(0x00), Some('a'));
        assert_eq!(us_layout(0x08), Some('c'));
        assert_eq!(us_layout(0x0A), None);
        assert_eq!(us_layout(0x12), Some('1'));
        assert_eq!(us_layout(0x16), Some('6'));
        assert_eq!(us_layout(0x17), Some('5'));
        assert_eq!(us_layout(0x1E), Some(']'));
        assert_eq!(us_layout(0x24), None);
        assert_eq!(us_layout(0x27), Some('\''));
        assert_eq!(us_layout(0x2A), Some('\\'));
        assert_eq!(us_layout(0x2F), Some('.'));
        assert_eq!(us_layout(0x31), Some(' '));
        assert_eq!(us_layout(0x32), Some('`'));
        assert_eq!(us_layout(0x60), None);
        assert!(is_keypad(0x4C) && is_keypad(0x5B) && !is_keypad(0x5A));
    }
}
