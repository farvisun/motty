use gpui::{App, KeyBinding, actions};

actions!(
    motty,
    [
        Copy,
        Paste,
        SelectAll,
        Clear,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        Quit,
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        ActivateLastTab,
        ToggleTabsPanel,
    ]
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
        KeyBinding::new("cmd-t", NewTab, Some("Workspace")),
        KeyBinding::new("cmd-w", CloseTab, Some("Workspace")),
        KeyBinding::new("cmd-shift-]", NextTab, Some("Workspace")),
        KeyBinding::new("ctrl-tab", NextTab, Some("Workspace")),
        KeyBinding::new("cmd-shift-[", PreviousTab, Some("Workspace")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("Workspace")),
        KeyBinding::new("cmd-1", ActivateTab1, Some("Workspace")),
        KeyBinding::new("cmd-2", ActivateTab2, Some("Workspace")),
        KeyBinding::new("cmd-3", ActivateTab3, Some("Workspace")),
        KeyBinding::new("cmd-4", ActivateTab4, Some("Workspace")),
        KeyBinding::new("cmd-5", ActivateTab5, Some("Workspace")),
        KeyBinding::new("cmd-6", ActivateTab6, Some("Workspace")),
        KeyBinding::new("cmd-7", ActivateTab7, Some("Workspace")),
        KeyBinding::new("cmd-8", ActivateTab8, Some("Workspace")),
        KeyBinding::new("cmd-9", ActivateLastTab, Some("Workspace")),
        KeyBinding::new("cmd-shift-l", ToggleTabsPanel, Some("Workspace")),
    ]);
}
