use gpui::{App, KeyBinding, actions};

actions!(
    motty,
    [Copy, Paste, SelectAll, Clear, IncreaseFontSize, DecreaseFontSize, ResetFontSize, Quit]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-c", Copy, Some("Terminal")),
        KeyBinding::new("cmd-v", Paste, Some("Terminal")),
        KeyBinding::new("cmd-a", SelectAll, Some("Terminal")),
        KeyBinding::new("cmd-k", Clear, Some("Terminal")),
        KeyBinding::new("cmd-=", IncreaseFontSize, Some("Terminal")),
        KeyBinding::new("cmd-+", IncreaseFontSize, Some("Terminal")),
        KeyBinding::new("cmd--", DecreaseFontSize, Some("Terminal")),
        KeyBinding::new("cmd-0", ResetFontSize, Some("Terminal")),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}
