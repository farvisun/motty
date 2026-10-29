//! The clipboard beyond plain text, which GPUI's clipboard doesn't carry.

/// Puts `html` on the clipboard for apps that paste styled text, with
/// `plain` for the rest.
pub fn write_html(plain: &str, html: &str) {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString};
        use objc2_foundation::NSString;
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        // SAFETY: the type constants are AppKit's static NSStrings.
        let (html_type, text_type) = unsafe { (NSPasteboardTypeHTML, NSPasteboardTypeString) };
        board.setString_forType(&NSString::from_str(html), html_type);
        board.setString_forType(&NSString::from_str(plain), text_type);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (plain, html);
}
