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

/// MIME types and the pasteboard types macOS keeps them as. A type not
/// listed is kept under its MIME name.
const MIME_TYPES: &[(&str, &str)] = &[
    ("text/plain", "public.utf8-plain-text"),
    ("text/html", "public.html"),
    ("text/rtf", "public.rtf"),
    ("application/rtf", "public.rtf"),
    ("image/png", "public.png"),
    ("image/tiff", "public.tiff"),
    ("image/jpeg", "public.jpeg"),
    ("image/gif", "com.compuserve.gif"),
    ("image/svg+xml", "public.svg-image"),
    ("application/pdf", "com.adobe.pdf"),
];

/// The pasteboard type for `mime`, parameters (`;charset=utf-8`) aside.
fn pasteboard_type(mime: &str) -> String {
    let base = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    MIME_TYPES
        .iter()
        .find(|(m, _)| *m == base)
        .map_or(base, |(_, t)| (*t).to_owned())
}

/// The MIME type a pasteboard type stands for, if it has one.
fn mime_type(pasteboard: &str) -> Option<String> {
    match MIME_TYPES.iter().find(|(_, t)| *t == pasteboard) {
        Some((m, _)) => Some((*m).to_owned()),
        None => pasteboard.contains('/').then(|| pasteboard.to_owned()),
    }
}

/// The clipboard's contents as MIME types, for programs that read it by
/// type (OSC 5522). A picture is offered as PNG even when only TIFF is
/// there.
pub fn mime_types() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        let types = mac::types();
        let mut out: Vec<String> = Vec::new();
        for mime in types.iter().filter_map(|t| mime_type(t)) {
            if !out.contains(&mime) {
                out.push(mime);
            }
        }
        if out.iter().any(|m| m == "image/tiff") && !out.iter().any(|m| m == "image/png") {
            out.push("image/png".into());
        }
        out
    }
    #[cfg(not(target_os = "macos"))]
    Vec::new()
}

/// The clipboard's data of type `mime`.
pub fn read_mime(mime: &str) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    return mac::read(&pasteboard_type(mime));
    #[cfg(not(target_os = "macos"))]
    {
        let _ = mime;
        None
    }
}

/// Replaces the clipboard with `items`, each under its MIME type; `false`
/// when the clipboard took none of them.
pub fn write_mimes(items: &[(String, Vec<u8>)]) -> bool {
    #[cfg(target_os = "macos")]
    {
        let items: Vec<(String, &[u8])> = items
            .iter()
            .map(|(mime, data)| (pasteboard_type(mime), data.as_slice()))
            .collect();
        mac::write(&items)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = items;
        false
    }
}

/// A random password for a paste event, used once.
pub fn one_time_password() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_err()
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        bytes = (now.as_nanos() ^ u128::from(std::process::id())).to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
    use objc2_foundation::{NSAttributedString, NSData, NSDictionary, NSRange, NSString};

    use super::Rich;

    /// The pasteboard types on the clipboard.
    pub fn types() -> Vec<String> {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::generalPasteboard();
            board
                .types()
                .map(|types| types.iter().map(|t| t.to_string()).collect())
                .unwrap_or_default()
        })
    }

    /// The clipboard's data of pasteboard type `kind`; PNG converted from
    /// TIFF when that's all there is.
    pub fn read(kind: &str) -> Option<Vec<u8>> {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::generalPasteboard();
            // SAFETY: the type constant is AppKit's static NSString.
            if kind == unsafe { NSPasteboardTypePNG }.to_string() {
                return image(&board);
            }
            let data = board.dataForType(&NSString::from_str(kind))?;
            Some(data.to_vec())
        })
    }

    /// Replaces the clipboard with `items` (pasteboard type, data).
    pub fn write(items: &[(String, &[u8])]) -> bool {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::generalPasteboard();
            board.clearContents();
            let mut any = false;
            for (kind, data) in items {
                any |= board
                    .setData_forType(Some(&NSData::with_bytes(data)), &NSString::from_str(kind));
            }
            any
        })
    }

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

        #[test]
        fn mime_names_map_to_pasteboard_types() {
            assert_eq!(
                crate::pasteboard::pasteboard_type("text/plain;charset=utf-8"),
                "public.utf8-plain-text"
            );
            assert_eq!(
                crate::pasteboard::pasteboard_type("IMAGE/PNG"),
                "public.png"
            );
            assert_eq!(
                crate::pasteboard::pasteboard_type("application/x-motty"),
                "application/x-motty"
            );
            assert_eq!(
                crate::pasteboard::mime_type("public.html").as_deref(),
                Some("text/html")
            );
            assert_eq!(crate::pasteboard::mime_type("com.apple.flat-rtfd"), None);
        }

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
