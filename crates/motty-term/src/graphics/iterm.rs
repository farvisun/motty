//! iTerm2's inline images: `OSC 1337 ; File = args : base64 ST`, and the
//! `MultipartFile` / `FilePart` / `FileEnd` split form. Only inline images
//! are shown; other files (downloads) are read and dropped.

use super::Pixels;
use super::kitty::Payload;

/// A width or height argument.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Size {
    #[default]
    Auto,
    Cells(f32),
    Pixels(f32),
    Percent(f32),
}

impl Size {
    fn parse(value: &str) -> Size {
        let number = |s: &str| s.trim().parse::<f32>().ok().filter(|n| *n > 0.);
        if let Some(n) = value.strip_suffix("px").and_then(number) {
            Size::Pixels(n)
        } else if let Some(n) = value.strip_suffix('%').and_then(number) {
            Size::Percent(n)
        } else if let Some(n) = number(value) {
            Size::Cells(n)
        } else {
            Size::Auto
        }
    }

    /// In pixels, given a cell's size and the screen's along this axis.
    fn pixels(self, cell: f32, screen: f32) -> Option<f32> {
        match self {
            Size::Auto => None,
            Size::Cells(n) => Some(n * cell),
            Size::Pixels(n) => Some(n),
            Size::Percent(p) => Some(screen * p / 100.),
        }
    }
}

/// The arguments before the `:`.
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    pub inline: bool,
    pub name: Option<String>,
    pub size: Option<u64>,
    pub width: Size,
    pub height: Size,
    pub preserve_aspect_ratio: bool,
    pub do_not_move_cursor: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            inline: false,
            name: None,
            size: None,
            width: Size::Auto,
            height: Size::Auto,
            preserve_aspect_ratio: true,
            do_not_move_cursor: false,
        }
    }
}

impl Args {
    pub fn parse(text: &str) -> Args {
        let mut args = Args::default();
        for pair in text.split(';') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            match key {
                "inline" => args.inline = value == "1",
                "name" => {
                    let mut p = Payload::default();
                    if p.push(value.as_bytes()).is_ok()
                        && let Ok(bytes) = p.finish()
                    {
                        args.name = Some(String::from_utf8_lossy(&bytes).into_owned());
                    }
                }
                "size" => args.size = value.parse().ok(),
                "width" => args.width = Size::parse(value),
                "height" => args.height = Size::parse(value),
                "preserveAspectRatio" => args.preserve_aspect_ratio = value != "0",
                "doNotMoveCursor" => args.do_not_move_cursor = value == "1",
                _ => {}
            }
        }
        args
    }

    /// The size to draw an `image_w` × `image_h` image at, in pixels, with
    /// cells of `cell` and a screen of `screen` pixels. Wide images with
    /// an automatic width are shrunk to fit the screen, as iTerm2 does.
    pub fn display_size(
        &self,
        (image_w, image_h): (u32, u32),
        cell: (f32, f32),
        screen: (f32, f32),
    ) -> (f32, f32) {
        let (iw, ih) = (image_w as f32, image_h as f32);
        let w = self.width.pixels(cell.0, screen.0);
        let h = self.height.pixels(cell.1, screen.1);
        let keep = self.preserve_aspect_ratio;
        match (w, h) {
            (None, None) if iw > screen.0 && screen.0 > 0. => (screen.0, ih * screen.0 / iw),
            (None, None) => (iw, ih),
            (Some(w), None) => (w, if keep { ih * w / iw } else { ih }),
            (None, Some(h)) => (if keep { iw * h / ih } else { iw }, h),
            (Some(w), Some(h)) if keep => {
                let scale = (w / iw).min(h / ih);
                (iw * scale, ih * scale)
            }
            (Some(w), Some(h)) => (w, h),
        }
    }
}

/// What an OSC 1337 file sequence asks for.
pub enum Request {
    /// A whole file: show it if inline.
    File(Args, Vec<u8>),
    /// The start of a multipart file.
    Begin(Args),
    /// More of it (base64).
    Part(Vec<u8>),
    /// Its end.
    End,
}

/// Parses the bytes after `1337;`.
pub fn parse(body: &[u8]) -> Option<Request> {
    if let Some(rest) = body.strip_prefix(b"File=") {
        let colon = memchr::memchr(b':', rest).unwrap_or(rest.len());
        let args = Args::parse(&String::from_utf8_lossy(&rest[..colon]));
        let data = rest.get(colon + 1..).unwrap_or_default().to_vec();
        return Some(Request::File(args, data));
    }
    if let Some(rest) = body.strip_prefix(b"MultipartFile=") {
        return Some(Request::Begin(Args::parse(&String::from_utf8_lossy(rest))));
    }
    if let Some(rest) = body.strip_prefix(b"FilePart=") {
        return Some(Request::Part(rest.to_vec()));
    }
    body.starts_with(b"FileEnd").then_some(Request::End)
}

/// Decodes PNG, JPEG, GIF (first frame) and the other formats `image` reads.
pub fn decode(bytes: &[u8]) -> Result<Pixels, String> {
    let image = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    Pixels::from_image(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_arguments() {
        let a = Args::parse(
            "name=aGVsbG8ucG5n;size=1234;width=10;height=50%;preserveAspectRatio=0;inline=1",
        );
        assert_eq!(a.name.as_deref(), Some("hello.png"));
        assert_eq!(a.size, Some(1234));
        assert_eq!((a.width, a.height), (Size::Cells(10.), Size::Percent(50.)));
        assert!(a.inline && !a.preserve_aspect_ratio);
        let d = Args::parse("width=auto;height=200px");
        assert_eq!((d.width, d.height), (Size::Auto, Size::Pixels(200.)));
        assert!(!d.inline && d.preserve_aspect_ratio);
    }

    #[test]
    fn splits_file_sequences() {
        let Some(Request::File(args, data)) = parse(b"File=inline=1;width=2:QUJD") else {
            panic!()
        };
        assert!(args.inline);
        assert_eq!(data, b"QUJD");
        assert!(matches!(parse(b"MultipartFile=inline=1"), Some(Request::Begin(a)) if a.inline));
        assert!(matches!(parse(b"FilePart=QUJD"), Some(Request::Part(d)) if d == b"QUJD"));
        assert!(matches!(parse(b"FileEnd"), Some(Request::End)));
    }

    #[test]
    fn sizes() {
        let cell = (10., 20.);
        let screen = (800., 600.);
        let size = |args: &str, image| Args::parse(args).display_size(image, cell, screen);
        assert_eq!(size("", (100, 50)), (100., 50.));
        // Too wide for the screen: shrunk to fit.
        assert_eq!(size("", (1600, 400)), (800., 200.));
        assert_eq!(size("width=20", (100, 50)), (200., 100.));
        assert_eq!(size("height=100px", (100, 50)), (200., 100.));
        assert_eq!(size("width=50%", (100, 50)), (400., 200.));
        // Both: fit inside, or stretch without aspect ratio.
        assert_eq!(size("width=10;height=10", (100, 50)), (100., 50.));
        assert_eq!(
            size("width=10;height=10;preserveAspectRatio=0", (100, 50)),
            (100., 200.)
        );
        assert_eq!(size("width=5;preserveAspectRatio=0", (100, 50)), (50., 50.));
    }
}
