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

/// What the Composer's ⌘V takes from the clipboard instead of its text.
#[derive(Debug, PartialEq)]
pub enum Rich {
    /// A picture copied on its own (a screenshot, Copy Image), as PNG.
    Image(Vec<u8>),
    /// Styled text, as HTML; RTF comes converted by AppKit.
    Html(String),
}

/// The clipboard's picture or styled text. `None` for copied files and
/// plain text, which the plain paste handles.
pub fn read_rich() -> Option<Rich> {
    #[cfg(target_os = "macos")]
    return mac::read_rich();
    #[cfg(not(target_os = "macos"))]
    None
}

/// RTF as HTML, converted by AppKit, as `read_rich` does.
#[cfg(all(test, target_os = "macos"))]
pub fn rtf_to_html(rtf: &[u8]) -> Option<String> {
    mac::rtf_to_html(rtf)
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::AnyThread;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSAttributedStringAppKitDocumentFormats, NSAttributedStringDocumentFormats,
        NSBitmapImageFileType, NSBitmapImageRep, NSDocumentTypeDocumentAttribute,
        NSHTMLTextDocumentType, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypeHTML,
        NSPasteboardTypePNG, NSPasteboardTypeRTF, NSPasteboardTypeString, NSPasteboardTypeTIFF,
    };
    use objc2_foundation::{NSAttributedString, NSData, NSDictionary, NSRange};

    use super::Rich;

    pub fn read_rich() -> Option<Rich> {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::generalPasteboard();
            let types = board.types()?;
            // SAFETY: the type constants are AppKit's static NSStrings.
            let (files, text, html, rtf) = unsafe {
                (
                    NSPasteboardTypeFileURL,
                    NSPasteboardTypeString,
                    NSPasteboardTypeHTML,
                    NSPasteboardTypeRTF,
                )
            };
            // Finder's copy also carries the files' icons and names.
            if types.containsObject(files) {
                return None;
            }
            // A picture with text alongside is a document's preview of it
            // (Word, Pages): the text is what was copied.
            if !types.containsObject(text)
                && let Some(png) = image(&board)
            {
                return Some(Rich::Image(png));
            }
            if let Some(html) = board.stringForType(html) {
                return Some(Rich::Html(html.to_string()));
            }
            let rtf = board.dataForType(rtf)?;
            rtf_to_html(&rtf.to_vec()).map(Rich::Html)
        })
    }

    /// The clipboard's picture as PNG, converting TIFF (what most apps
    /// copy) when there's no PNG.
    fn image(board: &NSPasteboard) -> Option<Vec<u8>> {
        // SAFETY: the type constants are AppKit's static NSStrings.
        if let Some(png) = board.dataForType(unsafe { NSPasteboardTypePNG }) {
            return Some(png.to_vec());
        }
        let tiff = board.dataForType(unsafe { NSPasteboardTypeTIFF })?;
        tiff_to_png(&tiff)
    }

    fn tiff_to_png(tiff: &NSData) -> Option<Vec<u8>> {
        let rep = NSBitmapImageRep::imageRepWithData(tiff)?;
        // SAFETY: an empty dictionary is valid PNG properties.
        let png = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }?;
        Some(png.to_vec())
    }

    pub fn rtf_to_html(rtf: &[u8]) -> Option<String> {
        objc2::rc::autoreleasepool(|_| {
            let data = NSData::with_bytes(rtf);
            // SAFETY: no document attributes are asked for.
            let text = unsafe {
                NSAttributedString::initWithRTF_documentAttributes(
                    NSAttributedString::alloc(),
                    &data,
                    None,
                )
            }?;
            // SAFETY: the key and the value are AppKit's static NSStrings, as
            // the dictionary's types say.
            let attributes = unsafe {
                NSDictionary::<_, AnyObject>::from_slices(
                    &[NSDocumentTypeDocumentAttribute],
                    &[NSHTMLTextDocumentType.as_ref()],
                )
            };
            let range = NSRange::new(0, text.length());
            // SAFETY: the attributes are of the documented types.
            let html =
                unsafe { text.dataFromRange_documentAttributes_error(range, &attributes) }.ok()?;
            String::from_utf8(html.to_vec()).ok()
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// One transparent pixel.
        const PIXEL: [u8; 70] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f, 0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47,
            0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];

        #[test]
        fn copied_tiff_becomes_png() {
            let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(&PIXEL)).unwrap();
            let tiff = rep.TIFFRepresentation().expect("AppKit writes TIFF");
            let png = tiff_to_png(&tiff).expect("AppKit writes PNG");
            assert!(png.starts_with(&PIXEL[..8]), "a PNG signature");
            let back = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(&png)).unwrap();
            assert_eq!((back.pixelsWide(), back.pixelsHigh()), (1, 1));
        }
    }
}
