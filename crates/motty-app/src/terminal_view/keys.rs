//! Keys for the program: presses, repeats and (when the kitty keyboard
//! protocol asks) releases and modifier keys, encoded by
//! `motty_term::input` with what AppKit adds ([`crate::key_info`]).

use gpui::{Keystroke, ModifiersChangedEvent, Window};
use motty_term::alacritty_terminal::term::TermMode;
use motty_term::input::{self, KeyInput, KeyKind, Mods};

use super::TerminalView;

/// Keys whose press the program got, so it gets their release and no
/// other (a press Motty kept, like a shortcut, has none).
#[derive(Default)]
pub(super) struct KeysDown(Vec<String>);

impl KeysDown {
    const MAX: usize = 16;

    fn press(&mut self, id: String) {
        if !self.0.contains(&id) {
            if self.0.len() == Self::MAX {
                self.0.remove(0);
            }
            self.0.push(id);
        }
    }

    fn release(&mut self, id: &str) -> bool {
        let at = self.0.iter().position(|k| k == id);
        at.map(|i| self.0.remove(i)).is_some()
    }
}

impl TerminalView {
    /// The bytes for a key, or `None` when it isn't the program's. A key
    /// the program locked out (KAM) gives nothing to send. `pass_cmd`: the
    /// config unbinds this ⌘ chord, so it's the program's too.
    pub(super) fn encode_key(
        &mut self,
        ks: &Keystroke,
        kind: KeyKind,
        pass_cmd: bool,
    ) -> Option<Vec<u8>> {
        let native = crate::key_info::current();
        let mut mods = Self::mods(&ks.modifiers);
        if let Some(n) = native {
            // GPUI drops Shift when it's already in the key (`!`).
            mods.shift = n.shift;
            mods.caps_lock = n.caps_lock;
        }
        let n = native.unwrap_or_default();
        let input = KeyInput {
            key: &ks.key,
            key_char: ks.key_char.as_deref(),
            mods,
            kind,
            keypad: n.keypad,
            unshifted: n.unshifted,
            shifted: n.shifted,
            base_layout: n.base_layout,
        };
        let id = native.map_or_else(|| ks.key.clone(), |n| n.code.to_string());
        let option_as_meta = self.option_as_meta;
        let encode = |mode| {
            if pass_cmd && mods.cmd {
                input::encode_cmd(&input, mode, option_as_meta)
            } else {
                input::encode(&input, mode, option_as_meta)
            }
        };
        let Some(mode) = self.session.key_mode() else {
            let mode = *self.session.term.lock().mode();
            return encode(mode).map(|_| Vec::new());
        };
        if kind == KeyKind::Release {
            if !self.keys_down.release(&id) || !mode.contains(TermMode::REPORT_EVENT_TYPES) {
                return None;
            }
            return encode(mode);
        }
        let bytes = encode(mode)?;
        self.keys_down.press(id);
        Some(bytes)
    }

    pub(super) fn on_key_up(
        &mut self,
        event: &gpui::KeyUpEvent,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if !self.focus_handle.is_focused(window)
            || self.hints.is_some()
            || self.vi.is_some()
            || self.read_only
        {
            return;
        }
        let pass_cmd = crate::actions::unbound(&event.keystroke, cx);
        if let Some(bytes) = self.encode_key(&event.keystroke, KeyKind::Release, pass_cmd) {
            self.session.write(bytes);
        }
    }

    /// A modifier key went down or up: reported when the program asked
    /// for every key.
    pub(super) fn report_modifier(&mut self, event: &ModifiersChangedEvent, window: &Window) {
        if !self.focus_handle.is_focused(window)
            || self.hints.is_some()
            || self.vi.is_some()
            || self.read_only
        {
            return;
        }
        let Some(mode) = self
            .session
            .key_mode()
            .filter(|m| m.contains(TermMode::REPORT_ALL_KEYS_AS_ESC))
        else {
            return;
        };
        let Some((key, down)) = crate::key_info::modifier_change() else {
            return;
        };
        let m = &event.modifiers;
        let input = KeyInput {
            key,
            mods: Mods {
                shift: m.shift,
                alt: m.alt,
                ctrl: m.control,
                cmd: m.platform,
                caps_lock: event.capslock.on,
            },
            kind: if down {
                KeyKind::Press
            } else {
                KeyKind::Release
            },
            ..Default::default()
        };
        if let Some(bytes) = input::encode(&input, mode, self.option_as_meta) {
            self.session.write(bytes);
        }
    }
}
