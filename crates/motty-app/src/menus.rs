//! The menu bar. Items run the same actions as the key bindings (GPUI shows
//! each one's shortcut from the keymap and greys out items the focused
//! window can't run), so a rebinding shows up here once the menus are set
//! again.

use gpui::{App, Menu, MenuItem, OsAction, SystemMenuType};

use crate::actions::*;

pub fn set(cx: &mut App) {
    cx.set_menus([
        Menu::new("Motty").items([
            MenuItem::action("About Motty", About),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::action("Welcome Guide", ShowWelcome),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Motty", HideApp),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Motty", Quit),
        ]),
        Menu::new("Shell").items([
            MenuItem::action("New Window", NewWindow),
            MenuItem::action("New Tab", NewTab),
            MenuItem::separator(),
            MenuItem::action("Split Right", SplitRight),
            MenuItem::action("Split Down", SplitDown),
            MenuItem::submenu(Menu::new("Split").items([
                MenuItem::action("Split Left", SplitLeft),
                MenuItem::action("Split Up", SplitUp),
                MenuItem::separator(),
                MenuItem::action("Equalize Splits", EqualizeSplits),
                MenuItem::action("Zoom Split", ZoomSplit),
            ])),
            MenuItem::separator(),
            MenuItem::action("Close", CloseTab),
            MenuItem::action("Close Window", CloseWindow),
            MenuItem::action("Reopen Closed Tab", ReopenClosedTab),
            MenuItem::separator(),
            MenuItem::action("Copy Path", CopyPath),
            MenuItem::action("Reveal in Finder", RevealInFinder),
            MenuItem::separator(),
            MenuItem::action("Vi Mode", ToggleViMode),
            MenuItem::submenu(Menu::new("Hint Mode").items([
                MenuItem::action("Open Link", HintOpen),
                MenuItem::action("Copy Link", HintCopy),
                MenuItem::action("Paste Word", HintPaste),
                MenuItem::action("Reveal Path", HintReveal),
            ])),
            MenuItem::action("Read-Only", ToggleReadOnly),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action("Find…", Find),
            MenuItem::separator(),
            MenuItem::action("Clear Screen", Clear),
        ]),
        Menu::new("View").items([
            MenuItem::action("Command Palette…", CommandPalette),
            MenuItem::action("Open Quickly…", OpenQuickly),
            MenuItem::action("Jump To…", JumpTo),
            MenuItem::action("Open Files Quickly…", OpenFilesQuickly),
            MenuItem::separator(),
            MenuItem::action("Tabs Panel", ToggleTabsPanel),
            MenuItem::action("Details Panel", ToggleDetailsPanel),
            MenuItem::submenu(Menu::new("Details").items([
                MenuItem::action("Info", DetailsInfo),
                MenuItem::action("Outline", DetailsOutline),
                MenuItem::action("Git", DetailsGit),
                MenuItem::action("Files", DetailsFiles),
            ])),
            MenuItem::separator(),
            MenuItem::action("Bigger", IncreaseFontSize),
            MenuItem::action("Smaller", DecreaseFontSize),
            MenuItem::action("Actual Size", ResetFontSize),
            MenuItem::separator(),
            MenuItem::action("Scroll to Top", ScrollToTop),
            MenuItem::action("Scroll to Bottom", ScrollToBottom),
            MenuItem::separator(),
            MenuItem::action("Toggle Full Screen", ToggleFullScreen),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", ZoomWindow),
            MenuItem::separator(),
            MenuItem::action("Show Next Tab", NextTab),
            MenuItem::action("Show Previous Tab", PreviousTab),
            MenuItem::action("Next Pane", FocusNextPane),
            MenuItem::action("Previous Pane", FocusPreviousPane),
            MenuItem::separator(),
        ]),
        Menu::new("Help").items([MenuItem::action("Motty Help", OpenHelp)]),
    ]);
}

/// The standard About panel, with the bundle's name, version and icon.
pub fn show_about() {
    #[cfg(target_os = "macos")]
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        app.orderFrontStandardAboutPanel(None);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}
