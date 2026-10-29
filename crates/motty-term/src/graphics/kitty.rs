//! The kitty graphics protocol (`ESC _ G <control> ; <payload> ESC \`):
//! command parsing, chunked uploads, loading pixel data and replies.
//! What a command does to the screen is in the parent module.

use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine as _;

use super::Pixels;

/// Largest image side kitty accepts.
pub const MAX_SIDE: u32 = 10_000;
/// Largest decompressed or file payload.
const MAX_DATA: usize = 400 << 20;

/// A parsed command. Fields are named after what the keys mean for
/// transmission and display; delete reuses `x`, `y` and `z`.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    /// `a`: t, T, q, p, d, f, a, c.
    pub action: u8,
    /// `q`: 1 hides OK replies, 2 errors too.
    pub quiet: u8,
    /// `e`: the action a feature query asks about.
    pub feature: Option<u8>,
    /// `f`: 24, 32 or 100.
    pub format: u32,
    /// `t`: d, f, t or s.
    pub medium: u8,
    /// `o`: z for zlib.
    pub compression: Option<u8>,
    /// `s`, `v`: pixel size of raw data.
    pub width: u32,
    pub height: u32,
    /// `S`, `O`: how much of a file to read, from where.
    pub size: u32,
    pub offset: u32,
    /// `i`, `I`, `p`.
    pub id: u32,
    pub number: u32,
    pub placement: u32,
    /// `m`: more chunks follow.
    pub more: bool,
    /// `x`, `y`, `w`, `h`: source rectangle (delete: cell).
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// `X`, `Y`: pixel offset in the first cell.
    pub cell_x: u32,
    pub cell_y: u32,
    /// `c`, `r`: cells to fit the image in.
    pub cols: u32,
    pub rows: u32,
    /// `C=1`: leave the cursor where it is.
    pub keep_cursor: bool,
    /// `U=1`: a virtual placement for Unicode placeholders.
    pub virtual_placement: bool,
    /// `z`.
    pub z: i32,
    /// `P`, `Q`: a parent placement (unsupported).
    pub parent: u32,
    /// `d`: what to delete.
    pub delete: u8,
    /// The keys as sent, for feature-query replies.
    pub keys: Vec<(u8, String)>,
}

impl Default for Command {
    fn default() -> Self {
        Self {
            action: b't',
            quiet: 0,
            feature: None,
            format: 32,
            medium: b'd',
            compression: None,
            width: 0,
            height: 0,
            size: 0,
            offset: 0,
            id: 0,
            number: 0,
            placement: 0,
            more: false,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            cell_x: 0,
            cell_y: 0,
            cols: 0,
            rows: 0,
            keep_cursor: false,
            virtual_placement: false,
            z: 0,
            parent: 0,
            delete: b'a',
            keys: Vec::new(),
        }
    }
}

/// Splits a command body (after `G`) into control data and payload.
pub fn split(body: &[u8]) -> (&[u8], &[u8]) {
    match memchr::memchr(b';', body) {
        Some(i) => (&body[..i], &body[i + 1..]),
        None => (body, &[]),
    }
}

/// Parses control data. On a bad value the command is still returned,
/// so the error can be addressed to its image.
pub fn parse(control: &[u8]) -> (Command, Result<(), String>) {
    let mut cmd = Command::default();
    let mut result = Ok(());
    for pair in control.split(|&b| b == b',') {
        if pair.is_empty() {
            continue;
        }
        let (Some(&key), Some(b'='), value) = (pair.first(), pair.get(1), pair.get(2..)) else {
            result = Err(format!(
                "EINVAL:malformed key {:?}",
                String::from_utf8_lossy(pair)
            ));
            continue;
        };
        let value = String::from_utf8_lossy(value.unwrap_or_default()).into_owned();
        if let Err(e) = cmd.set(key, &value) {
            result = Err(e);
        }
        cmd.keys.push((key, value));
    }
    if result.is_ok() && cmd.id != 0 && cmd.number != 0 {
        result = Err("EINVAL:both i and I given".into());
    }
    (cmd, result)
}

impl Command {
    fn set(&mut self, key: u8, value: &str) -> Result<(), String> {
        let bad = || format!("EINVAL:bad value for {}: {value:?}", key as char);
        let char = || match value.as_bytes() {
            [c] => Ok(*c),
            _ => Err(bad()),
        };
        let uint = || value.parse::<u32>().map_err(|_| bad());
        match key {
            b'a' => self.action = char()?,
            b'q' => self.quiet = uint()?.min(2) as u8,
            b'e' => self.feature = Some(char()?),
            b'f' => self.format = uint()?,
            b't' => self.medium = char()?,
            b'o' => self.compression = Some(char()?),
            b's' => self.width = uint()?,
            b'v' => self.height = uint()?,
            b'S' => self.size = uint()?,
            b'O' => self.offset = uint()?,
            b'i' => self.id = uint()?,
            b'I' => self.number = uint()?,
            b'p' => self.placement = uint()?,
            b'm' => self.more = uint()? == 1,
            b'x' => self.x = uint()?,
            b'y' => self.y = uint()?,
            b'w' => self.w = uint()?,
            b'h' => self.h = uint()?,
            b'X' => self.cell_x = uint()?,
            b'Y' => self.cell_y = uint()?,
            b'c' => self.cols = uint()?,
            b'r' => self.rows = uint()?,
            b'C' => self.keep_cursor = uint()? == 1,
            b'U' => self.virtual_placement = uint()? == 1,
            b'z' => self.z = value.parse::<i32>().map_err(|_| bad())?,
            b'P' | b'Q' => self.parent = self.parent.max(uint()?),
            b'H' | b'V' => {
                value.parse::<i32>().map_err(|_| bad())?;
            }
            b'd' => self.delete = char()?,
            // Hints and animation keys: accepted, unused.
            b'N' => {
                uint()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Whether this only continues a chunked upload (`m` and `q`, maybe
    /// `a=f` for animation frames).
    pub fn is_continuation(&self) -> bool {
        self.keys
            .iter()
            .all(|(k, _)| matches!(k, b'm' | b'q' | b'a'))
            && matches!(self.action, b't' | b'T' | b'q' | b'f')
    }
}

/// The reply to a command, or `None` when it gets none: no `i`/`I`, or
/// silenced with `q`.
pub fn reply(cmd: &Command, id: u32, msg: &str) -> Option<String> {
    if cmd.id == 0 && cmd.number == 0 {
        return None;
    }
    let ok = msg == "OK";
    if (ok && cmd.quiet >= 1) || cmd.quiet >= 2 {
        return None;
    }
    let mut out = format!("\x1b_Gi={id}");
    if cmd.number != 0 {
        out.push_str(&format!(",I={}", cmd.number));
    }
    if cmd.placement != 0 && id != 0 {
        out.push_str(&format!(",p={}", cmd.placement));
    }
    out.push(';');
    out.push_str(msg);
    out.push_str("\x1b\\");
    Some(out)
}

/// The reply to a feature query (`a=q,e=…`): the keys from the query this
/// terminal supports with those values.
pub fn feature_reply(cmd: &Command) -> Option<String> {
    if cmd.id == 0 || cmd.quiet >= 1 {
        return None;
    }
    let action = cmd.feature?;
    let mut out = format!("\x1b_Gi={}", cmd.id);
    if matches!(action, b't' | b'T' | b'q' | b'p' | b'd') {
        out.push_str(&format!(",e={}", action as char));
        for (key, value) in &cmd.keys {
            if matches!(key, b'a' | b'i' | b'I' | b'm' | b'q' | b'e') {
                continue;
            }
            if supports(action, *key, value) {
                out.push_str(&format!(",{}={value}", *key as char));
            }
        }
    }
    out.push_str(";OK\x1b\\");
    Some(out)
}

/// Whether `key=value` (not its default) is supported for `action`.
fn supports(action: u8, key: u8, value: &str) -> bool {
    let transmit = matches!(action, b't' | b'T' | b'q');
    let display = matches!(action, b'T' | b'p');
    let nonzero = value.parse::<i64>().is_ok_and(|v| v != 0);
    match key {
        b'f' if transmit => matches!(value, "24" | "100"),
        b't' if transmit => matches!(value, "f" | "t"),
        b'o' if transmit => value == "z",
        b's' | b'v' | b'S' | b'O' | b'N' if transmit => nonzero,
        b'p' => (display || action == b'd') && nonzero,
        b'x' | b'y' | b'w' | b'h' | b'X' | b'Y' | b'c' | b'r' | b'z' if display => nonzero,
        b'C' | b'U' if display => value == "1",
        b'd' if action == b'd' => matches!(
            value.as_bytes().first().map(u8::to_ascii_lowercase),
            Some(b'a' | b'i' | b'n' | b'c' | b'p' | b'q' | b'r' | b'x' | b'y' | b'z')
        ),
        b'x' | b'y' | b'z' if action == b'd' => true,
        _ => false,
    }
}

/// Base64 payload, decoded a chunk at a time: every chunk but the last is
/// a multiple of four characters, and some clients pad each one.
#[derive(Default)]
pub struct Payload {
    pub data: Vec<u8>,
    /// Characters left over from a chunk that wasn't a multiple of four.
    rest: Vec<u8>,
    pub too_big: bool,
}

impl Payload {
    pub fn push(&mut self, chunk: &[u8]) -> Result<(), String> {
        let mut text: Vec<u8> = std::mem::take(&mut self.rest);
        text.extend(chunk.iter().filter(|b| !b.is_ascii_whitespace()));
        // Padding ends a quantum; keep whole quanta.
        let whole = if text.contains(&b'=') {
            text.len()
        } else {
            text.len() / 4 * 4
        };
        self.rest = text.split_off(whole);
        if self.too_big || self.data.len() + whole / 4 * 3 > MAX_DATA {
            self.too_big = true;
            return Ok(());
        }
        let engine = base64::engine::GeneralPurpose::new(
            &base64::alphabet::STANDARD,
            base64::engine::GeneralPurposeConfig::new()
                .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
                .with_decode_allow_trailing_bits(true),
        );
        engine
            .decode_vec(&text, &mut self.data)
            .map_err(|e| format!("EINVAL:bad base64: {e}"))
    }

    pub fn finish(mut self) -> Result<Vec<u8>, String> {
        if !self.rest.is_empty() {
            let rest = std::mem::take(&mut self.rest);
            let mut pad = rest;
            while !pad.len().is_multiple_of(4) {
                pad.push(b'=');
            }
            self.push(&pad)?;
        }
        if self.too_big {
            return Err("EFBIG:payload too large".into());
        }
        Ok(self.data)
    }
}

/// Image data to load off the terminal lock.
pub struct Load {
    pub cmd: Command,
    pub data: Vec<u8>,
}

impl Load {
    /// Reads, inflates and decodes the image.
    pub fn run(self) -> Result<Pixels, String> {
        let cmd = &self.cmd;
        let bytes = match cmd.medium {
            b'd' => self.data,
            b'f' | b't' => read_file(cmd, &self.data)?,
            b's' => return Err("ENOTSUP:shared memory is not supported".into()),
            m => return Err(format!("EINVAL:unknown transmission medium {}", m as char)),
        };
        let bytes = match cmd.compression {
            None => bytes,
            Some(b'z') => {
                let limit = match cmd.format {
                    24 | 32 => cmd.width as usize * cmd.height as usize * (cmd.format as usize / 8),
                    _ => MAX_DATA,
                };
                inflate(&bytes, limit)?
            }
            Some(c) => return Err(format!("EINVAL:unknown compression {}", c as char)),
        };
        decode(cmd, bytes)
    }
}

fn inflate(bytes: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .take(limit as u64)
        .read_to_end(&mut out)
        .map_err(|e| format!("EINVAL:zlib: {e}"))?;
    Ok(out)
}

/// RGB, RGBA or PNG bytes to pixels.
pub fn decode(cmd: &Command, bytes: Vec<u8>) -> Result<Pixels, String> {
    let pixels = match cmd.format {
        24 | 32 => {
            let (w, h) = (cmd.width, cmd.height);
            if w == 0 || h == 0 {
                return Err("EINVAL:missing image size (s, v)".into());
            }
            if w > MAX_SIDE || h > MAX_SIDE {
                return Err("EFBIG:image too large".into());
            }
            let bpp = cmd.format as usize / 8;
            let need = w as usize * h as usize * bpp;
            if bytes.len() < need {
                return Err(format!(
                    "ENODATA:insufficient image data: {} < {need}",
                    bytes.len()
                ));
            }
            let rgba = if bpp == 4 {
                let mut bytes = bytes;
                bytes.truncate(need);
                bytes
            } else {
                let mut out = Vec::with_capacity(w as usize * h as usize * 4);
                for px in bytes[..need].as_chunks::<3>().0 {
                    out.extend_from_slice(&[px[0], px[1], px[2], 0xff]);
                }
                out
            };
            Pixels {
                width: w,
                height: h,
                rgba,
            }
        }
        100 => {
            let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                .map_err(|e| format!("EBADPNG:{e}"))?;
            Pixels::from_image(image)?
        }
        f => return Err(format!("EINVAL:unsupported format {f}")),
    };
    Ok(pixels)
}

/// The one error every failure to read a file gets, so programs can't
/// probe for files through it (as kitty does).
const FILE_ERROR: &str = "EBADF:Failed to read image file";

/// Reads `t=f` / `t=t` data: a regular file outside system directories;
/// a temporary file is deleted after, if it's in a temporary directory.
fn read_file(cmd: &Command, path: &[u8]) -> Result<Vec<u8>, String> {
    let fail = |why: &str| {
        tracing::debug!(path = %String::from_utf8_lossy(path), "graphics file: {why}");
        FILE_ERROR.to_string()
    };
    let path = PathBuf::from(std::str::from_utf8(path).map_err(|_| fail("not UTF-8"))?);
    if !path.is_absolute() {
        return Err(fail("relative path"));
    }
    let real = path.canonicalize().map_err(|e| fail(&e.to_string()))?;
    if ["/proc", "/sys", "/dev"]
        .iter()
        .any(|dir| real.starts_with(dir))
    {
        return Err(fail("system directory"));
    }
    let meta = std::fs::metadata(&real).map_err(|e| fail(&e.to_string()))?;
    if !meta.is_file() {
        return Err(fail("not a regular file"));
    }
    let mut file = std::fs::File::open(&real).map_err(|e| fail(&e.to_string()))?;
    if cmd.offset > 0 {
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(cmd.offset.into()))
            .map_err(|e| fail(&e.to_string()))?;
    }
    let limit = if cmd.size > 0 {
        cmd.size as usize
    } else {
        MAX_DATA
    };
    let mut bytes = Vec::new();
    file.take(limit as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| fail(&e.to_string()))?;
    if cmd.medium == b't' && is_temporary(&real) {
        let _ = std::fs::remove_file(&real);
    }
    if cmd.size > 0 && bytes.len() < cmd.size as usize {
        return Err(fail("shorter than S"));
    }
    Ok(bytes)
}

/// Whether kitty's rules let a `t=t` file be deleted: in a temporary
/// directory, with `tty-graphics-protocol` in its path.
fn is_temporary(real: &Path) -> bool {
    if !real.to_string_lossy().contains("tty-graphics-protocol") {
        return false;
    }
    let mut dirs: Vec<PathBuf> = vec!["/tmp".into(), "/var/tmp".into(), "/dev/shm".into()];
    dirs.push(std::env::temp_dir());
    if let Some(dir) = std::env::var_os("TMPDIR") {
        dirs.push(dir.into());
    }
    dirs.iter()
        .filter_map(|d| d.canonicalize().ok())
        .any(|d| real.starts_with(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(control: &str) -> Command {
        let (cmd, result) = parse(control.as_bytes());
        result.unwrap();
        cmd
    }

    #[test]
    fn parses_every_key() {
        let c = cmd(
            "a=T,q=2,f=24,t=f,o=z,s=10,v=20,S=30,O=40,i=5,p=6,m=1,x=1,y=2,w=3,h=4,X=5,Y=6,c=7,r=8,C=1,U=1,z=-9,d=I,N=1",
        );
        assert_eq!(c.action, b'T');
        assert_eq!(c.quiet, 2);
        assert_eq!((c.format, c.medium, c.compression), (24, b'f', Some(b'z')));
        assert_eq!((c.width, c.height, c.size, c.offset), (10, 20, 30, 40));
        assert_eq!((c.id, c.number, c.placement), (5, 0, 6));
        assert!(c.more && c.keep_cursor && c.virtual_placement);
        assert_eq!((c.x, c.y, c.w, c.h), (1, 2, 3, 4));
        assert_eq!((c.cell_x, c.cell_y, c.cols, c.rows), (5, 6, 7, 8));
        assert_eq!((c.z, c.delete), (-9, b'I'));
        assert_eq!(c.keys.len(), 25);
        // Defaults.
        let d = cmd("");
        assert_eq!(
            (d.action, d.format, d.medium, d.delete),
            (b't', 32, b'd', b'a')
        );
    }

    #[test]
    fn rejects_bad_values_and_both_ids() {
        let (c, r) = parse(b"i=7,s=abc");
        assert_eq!(c.id, 7);
        assert!(r.unwrap_err().starts_with("EINVAL"));
        assert!(parse(b"i=1,I=2").1.is_err());
        assert!(parse(b"a=toolong").1.is_err());
    }

    #[test]
    fn replies_follow_ids_and_quiet() {
        assert_eq!(
            reply(&cmd("i=31"), 31, "OK").as_deref(),
            Some("\x1b_Gi=31;OK\x1b\\")
        );
        assert_eq!(
            reply(&cmd("I=13"), 99, "OK").as_deref(),
            Some("\x1b_Gi=99,I=13;OK\x1b\\")
        );
        assert_eq!(
            reply(&cmd("i=1,p=7"), 1, "ENOENT:x").as_deref(),
            Some("\x1b_Gi=1,p=7;ENOENT:x\x1b\\")
        );
        // No id: no reply. q=1 hides OK, q=2 errors too.
        assert_eq!(reply(&cmd("a=T"), 0, "OK"), None);
        assert_eq!(reply(&cmd("i=1,q=1"), 1, "OK"), None);
        assert!(reply(&cmd("i=1,q=1"), 1, "EINVAL:x").is_some());
        assert_eq!(reply(&cmd("i=1,q=2"), 1, "EINVAL:x"), None);
    }

    #[test]
    fn feature_query_echoes_supported_keys() {
        let c = cmd("a=q,i=31,e=T,U=1,C=1,f=8");
        assert_eq!(
            feature_reply(&c).as_deref(),
            Some("\x1b_Gi=31,e=T,U=1,C=1;OK\x1b\\")
        );
        // Animation frames aren't supported: no `e` in the reply.
        let c = cmd("a=q,i=32,e=f,r=1");
        assert_eq!(feature_reply(&c).as_deref(), Some("\x1b_Gi=32;OK\x1b\\"));
    }

    #[test]
    fn chunks_decode_with_or_without_padding() {
        let mut p = Payload::default();
        // "hello world!" split across chunks, one of them not a multiple of 4.
        p.push(b"aGVs").unwrap();
        p.push(b"bG8gd29y").unwrap();
        p.push(b"bGQ").unwrap();
        p.push(b"h").unwrap();
        assert_eq!(p.finish().unwrap(), b"hello world!");
        let mut p = Payload::default();
        p.push(b"aGk=").unwrap();
        p.push(b"aGk").unwrap();
        assert_eq!(p.finish().unwrap(), b"hihi");
    }

    #[test]
    fn loads_rgb_rgba_zlib_and_png() {
        let rgb = Load {
            cmd: cmd("f=24,s=2,v=1"),
            data: vec![1, 2, 3, 4, 5, 6],
        };
        assert_eq!(rgb.run().unwrap().rgba, vec![1, 2, 3, 255, 4, 5, 6, 255]);
        let short = Load {
            cmd: cmd("f=32,s=2,v=2"),
            data: vec![0; 15],
        };
        assert!(short.run().unwrap_err().starts_with("ENODATA"));

        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut z, &[9, 8, 7, 6]).unwrap();
        let zlib = Load {
            cmd: cmd("f=32,s=1,v=1,o=z"),
            data: z.finish().unwrap(),
        };
        assert_eq!(zlib.run().unwrap().rgba, vec![9, 8, 7, 6]);

        let png = Load {
            cmd: cmd("f=100"),
            data: super::super::tests::png(3, 2),
        };
        let p = png.run().unwrap();
        assert_eq!((p.width, p.height), (3, 2));
        assert!(
            Load {
                cmd: cmd("t=s"),
                data: b"/x".to_vec()
            }
            .run()
            .unwrap_err()
            .starts_with("ENOTSUP")
        );
    }

    #[test]
    fn reads_files_and_deletes_only_temporary_ones() {
        let dir =
            std::env::temp_dir().join(format!("tty-graphics-protocol-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("img.rgba");
        std::fs::write(&file, [1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        let path = file.to_string_lossy().into_owned().into_bytes();
        let load = |control: &str| Load {
            cmd: cmd(control),
            data: path.clone(),
        };
        // Offset and size pick the second pixel.
        assert_eq!(
            load("t=f,s=1,v=1,O=4,S=4").run().unwrap().rgba,
            vec![5, 6, 7, 8]
        );
        assert!(file.exists());
        assert!(load("t=t,s=1,v=1").run().is_ok());
        assert!(!file.exists());
        // Missing files, directories and devices all get the same error.
        assert_eq!(load("t=f,s=1,v=1").run().unwrap_err(), FILE_ERROR);
        for path in [dir.to_str().unwrap(), "/dev/null", "relative"] {
            let load = Load {
                cmd: cmd("t=f,s=1,v=1"),
                data: path.as_bytes().to_vec(),
            };
            assert_eq!(load.run().unwrap_err(), FILE_ERROR);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
