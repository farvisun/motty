//! Sixel (`DCS P1 ; P2 ; P3 q … ST`) decoding and XTSMGRAPHICS replies.
//!
//! Supported: colour introducers in RGB and HLS, repeat (`!`), raster
//! attributes (`"` aspect and size), graphics CR/NL (`$`/`-`), the VT340
//! default palette, and P2 = 1 leaving untouched pixels transparent. Every
//! image starts from the default palette (xterm's private colour registers).

use super::Pixels;

/// Colour registers reported by XTSMGRAPHICS (and used, wrapping, by `#`).
pub const COLOR_REGISTERS: u32 = 256;
/// Largest sixel image, either way, in pixels.
pub const MAX_SIDE: usize = 8192;

/// The parameters of `DCS P1 ; P2 ; P3 q`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Params {
    /// P2 = 1: pixels the image doesn't set stay transparent; otherwise
    /// they're painted in colour register 0.
    pub transparent: bool,
}

impl Params {
    pub fn new(params: &[u32]) -> Self {
        Self {
            transparent: params.get(1) == Some(&1),
        }
    }
}

/// Decodes sixel data; `None` if it draws nothing.
pub fn decode(params: Params, data: &[u8]) -> Option<Pixels> {
    let mut d = Decoder::new();
    d.run(data);
    d.finish(params)
}

struct Decoder {
    palette: [u32; COLOR_REGISTERS as usize],
    color: u32,
    /// Packed RGBA (`0` = untouched, as every colour is opaque).
    pixels: Vec<u32>,
    stride: usize,
    rows: usize,
    x: usize,
    /// Top of the current band of six pixel rows.
    y: usize,
    /// Extent drawn so far.
    max_x: usize,
    max_y: usize,
    /// From the raster attributes.
    aspect: usize,
    raster: (usize, usize),
}

impl Decoder {
    fn new() -> Self {
        let palette = default_palette();
        Self {
            color: palette[0],
            palette,
            pixels: Vec::new(),
            stride: 0,
            rows: 0,
            x: 0,
            y: 0,
            max_x: 0,
            max_y: 0,
            aspect: 1,
            raster: (0, 0),
        }
    }

    fn run(&mut self, data: &[u8]) {
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            i += 1;
            match b {
                b'?'..=b'~' => self.sixel(b - b'?', 1),
                b'!' => {
                    let (n, used) = number(&data[i..]);
                    i += used;
                    if let Some(&s @ b'?'..=b'~') = data.get(i) {
                        i += 1;
                        self.sixel(s - b'?', (n as usize).clamp(1, MAX_SIDE));
                    }
                }
                b'#' => {
                    let (p, used) = params(&data[i..]);
                    i += used;
                    self.color_introducer(&p);
                }
                b'"' => {
                    let (p, used) = params(&data[i..]);
                    i += used;
                    self.raster_attributes(&p);
                }
                b'$' => self.x = 0,
                b'-' => {
                    self.x = 0;
                    self.y += 6;
                }
                _ => {}
            }
        }
    }

    fn color_introducer(&mut self, p: &[u32]) {
        let Some(&index) = p.first() else {
            return;
        };
        let index = (index % COLOR_REGISTERS) as usize;
        if p.len() >= 5 {
            let (x, y, z) = (p[2], p[3], p[4]);
            let rgb = match p[1] {
                1 => Some(hls(x.min(360), y.min(100), z.min(100))),
                2 => Some([percent(x), percent(y), percent(z)]),
                _ => None,
            };
            if let Some([r, g, b]) = rgb {
                self.palette[index] = pack(r, g, b);
            }
        }
        self.color = self.palette[index];
    }

    fn raster_attributes(&mut self, p: &[u32]) {
        let pan = p.first().copied().unwrap_or(1).max(1);
        let pad = p.get(1).copied().unwrap_or(1).max(1);
        // Pixels are pan/pad times as tall as wide.
        self.aspect = ((pan + pad / 2) / pad).clamp(1, 10) as usize;
        let w = p.get(2).copied().unwrap_or(0) as usize;
        let h = p.get(3).copied().unwrap_or(0) as usize;
        self.raster = (w.min(MAX_SIDE), h.min(MAX_SIDE));
        if self.pixels.is_empty() && w > 0 {
            self.grow(self.raster.0, self.raster.1.max(6));
        }
    }

    /// Draws the six-pixel column `bits` at the cursor `n` times.
    fn sixel(&mut self, bits: u8, n: usize) {
        let x = self.x;
        self.x += n;
        if bits == 0 || x >= MAX_SIDE || self.y + 6 > MAX_SIDE {
            return;
        }
        let end = (x + n).min(MAX_SIDE);
        self.grow(end, self.y + 6);
        for k in 0..6 {
            if bits & (1 << k) == 0 {
                continue;
            }
            let row = (self.y + k) * self.stride;
            self.pixels[row + x..row + end].fill(self.color);
            self.max_y = self.max_y.max(self.y + k + 1);
        }
        self.max_x = self.max_x.max(end);
    }

    /// Makes room for `w` × `h` pixels, at least doubling when it grows.
    fn grow(&mut self, w: usize, h: usize) {
        if w > self.stride {
            let stride = w.max(self.stride * 2).min(MAX_SIDE);
            let mut pixels = vec![0; stride * self.rows];
            for row in 0..self.rows {
                pixels[row * stride..row * stride + self.stride]
                    .copy_from_slice(&self.pixels[row * self.stride..(row + 1) * self.stride]);
            }
            self.pixels = pixels;
            self.stride = stride;
        }
        if h > self.rows {
            let rows = h.max(self.rows * 2).min(MAX_SIDE);
            self.pixels.resize(self.stride * rows, 0);
            self.rows = rows;
        }
    }

    fn finish(mut self, params: Params) -> Option<Pixels> {
        let width = self.max_x.max(self.raster.0);
        let height = self.max_y.max(self.raster.1);
        if width == 0 || height == 0 {
            return None;
        }
        self.grow(width, height);
        let background = if params.transparent {
            0
        } else {
            self.palette[0]
        };
        let tall = self.aspect;
        let mut rgba = Vec::with_capacity(width * height * tall * 4);
        for row in 0..height {
            let start = rgba.len();
            for &px in &self.pixels[row * self.stride..row * self.stride + width] {
                let px = if px == 0 { background } else { px };
                rgba.extend_from_slice(&px.to_be_bytes());
            }
            for _ in 1..tall {
                rgba.extend_from_within(start..start + width * 4);
            }
        }
        Some(Pixels {
            width: width as u32,
            height: (height * tall) as u32,
            rgba,
        })
    }
}

fn pack(r: u8, g: u8, b: u8) -> u32 {
    u32::from_be_bytes([r, g, b, 0xff])
}

fn percent(p: u32) -> u8 {
    ((p.min(100) * 255 + 50) / 100) as u8
}

/// DEC HLS (hue 0 = blue, 120 = red, 240 = green; percentages) to RGB.
fn hls(h: u32, l: u32, s: u32) -> [u8; 3] {
    let (l, s) = (l as f32 / 100., s as f32 / 100.);
    let h = ((h + 240) % 360) as f32;
    let c = (1. - (2. * l - 1.).abs()) * s;
    let x = c * (1. - ((h / 60.) % 2. - 1.).abs());
    let m = l - c / 2.;
    let (r, g, b) = match h as u32 / 60 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    let to = |v: f32| ((v + m) * 255.).round().clamp(0., 255.) as u8;
    [to(r), to(g), to(b)]
}

/// The VT340's sixteen colours, then xterm's 256-colour cube and greys.
fn default_palette() -> [u32; COLOR_REGISTERS as usize] {
    const VT340: [[u32; 3]; 16] = [
        [0, 0, 0],
        [20, 20, 80],
        [80, 13, 13],
        [20, 80, 20],
        [80, 20, 80],
        [20, 80, 80],
        [80, 80, 20],
        [53, 53, 53],
        [26, 26, 26],
        [33, 33, 60],
        [60, 26, 26],
        [33, 60, 33],
        [60, 33, 60],
        [33, 60, 60],
        [60, 60, 33],
        [80, 80, 80],
    ];
    let mut palette = [0; COLOR_REGISTERS as usize];
    for (i, [r, g, b]) in VT340.into_iter().enumerate() {
        palette[i] = pack(percent(r), percent(g), percent(b));
    }
    let level = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
    for (v, color) in palette[16..232].iter_mut().enumerate() {
        *color = pack(level(v / 36), level(v / 6 % 6), level(v % 6));
    }
    for (i, color) in palette[232..].iter_mut().enumerate() {
        let g = (8 + i * 10) as u8;
        *color = pack(g, g, g);
    }
    palette
}

/// A decimal number at the start of `bytes` (0 if none) and its length.
fn number(bytes: &[u8]) -> (u32, usize) {
    let n = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    let value = bytes[..n].iter().fold(0u32, |acc, &d| {
        acc.saturating_mul(10).saturating_add(u32::from(d - b'0'))
    });
    (value, n)
}

/// `;`-separated numbers at the start of `bytes` and their length.
fn params(bytes: &[u8]) -> (Vec<u32>, usize) {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let (n, used) = number(&bytes[i..]);
        out.push(n);
        i += used;
        if bytes.get(i) == Some(&b';') {
            i += 1;
        } else {
            return (out, i);
        }
    }
}

/// The reply to XTSMGRAPHICS (`CSI ? Pi ; Pa ; Pv S`), given the text
/// area in pixels.
pub fn xtsmgraphics(params: &[u32], area: (u32, u32), registers: &mut u32) -> String {
    let item = params.first().copied().unwrap_or(0);
    let action = params.get(1).copied().unwrap_or(0);
    let max_area = (MAX_SIDE as u32, MAX_SIDE as u32);
    match (item, action) {
        // Colour registers: read, reset, set, read the maximum.
        (1, 1) => format!("\x1b[?1;0;{registers}S"),
        (1, 2) => {
            *registers = COLOR_REGISTERS;
            format!("\x1b[?1;0;{registers}S")
        }
        (1, 3) => {
            *registers = params
                .get(2)
                .copied()
                .unwrap_or(COLOR_REGISTERS)
                .clamp(1, COLOR_REGISTERS);
            format!("\x1b[?1;0;{registers}S")
        }
        (1, 4) => format!("\x1b[?1;0;{COLOR_REGISTERS}S"),
        // Sixel geometry: what fits on screen, or the decoder's limit.
        (2, 1..=3) => format!("\x1b[?2;0;{};{}S", area.0, area.1),
        (2, 4) => format!("\x1b[?2;0;{};{}S", max_area.0, max_area.1),
        (1 | 2, _) => format!("\x1b[?{item};2;0S"),
        // No ReGIS.
        _ => format!("\x1b[?{item};1;0S"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(p: &Pixels, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * p.width + x) * 4) as usize;
        p.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn rgb_and_hls_colours() {
        // Register 1 red by RGB; register 2 blue by HLS (hue 0 is blue).
        let p = decode(Params::default(), b"#1;2;100;0;0#1~#2;1;0;50;100#2~").unwrap();
        assert_eq!((p.width, p.height), (2, 6));
        assert_eq!(px(&p, 0, 0), [255, 0, 0, 255]);
        assert_eq!(px(&p, 1, 5), [0, 0, 255, 255]);
        // HLS hue 120 is red, 240 green.
        assert_eq!(hls(120, 50, 100), [255, 0, 0]);
        assert_eq!(hls(240, 50, 100), [0, 255, 0]);
        assert_eq!(hls(0, 100, 0), [255, 255, 255]);
    }

    #[test]
    fn repeat_bands_and_carriage_return() {
        // `!5@` sets the top pixel of five columns; `$` goes back over the
        // same band, `-` starts the next.
        let p = decode(Params::default(), b"#0;2;0;100;0!5@$#0;2;0;0;100A-~").unwrap();
        assert_eq!((p.width, p.height), (5, 12));
        // `A` (bit 1) drew blue under the green row, with register 0 redefined.
        assert_eq!(px(&p, 4, 0), [0, 255, 0, 255]);
        assert_eq!(px(&p, 0, 1), [0, 0, 255, 255]);
        assert_eq!(px(&p, 0, 11), [0, 0, 255, 255]);
    }

    #[test]
    fn raster_size_and_transparency() {
        let data = b"\"1;1;4;8#3~";
        // Untouched pixels: transparent with P2 = 1, register 0 (black)
        // otherwise. The raster size wins over the drawn extent.
        let clear = decode(Params::new(&[0, 1, 0]), data).unwrap();
        assert_eq!((clear.width, clear.height), (4, 8));
        assert_eq!(px(&clear, 3, 7), [0, 0, 0, 0]);
        assert_eq!(px(&clear, 0, 0), [51, 204, 51, 255]);
        let filled = decode(Params::new(&[0, 0, 0]), data).unwrap();
        assert_eq!(px(&filled, 3, 7), [0, 0, 0, 255]);
    }

    #[test]
    fn raster_aspect_stretches_rows() {
        let p = decode(Params::default(), b"\"2;1#1@").unwrap();
        assert_eq!((p.width, p.height), (1, 2));
        assert_eq!(px(&p, 0, 1), px(&p, 0, 0));
    }

    #[test]
    fn grows_past_the_raster_hint() {
        let p = decode(Params::default(), b"\"1;1;2;6#1!3~-!4~").unwrap();
        assert_eq!((p.width, p.height), (4, 12));
        assert_eq!(px(&p, 2, 0), px(&p, 3, 11));
        assert!(decode(Params::default(), b"#1;2;1;2;3").is_none());
    }

    #[test]
    fn vt340_palette() {
        let p = default_palette();
        assert_eq!(p[2].to_be_bytes(), [204, 33, 33, 255]);
        assert_eq!(p[16].to_be_bytes(), [0, 0, 0, 255]);
        assert_eq!(p[231].to_be_bytes(), [255, 255, 255, 255]);
        assert_eq!(p[255].to_be_bytes(), [238, 238, 238, 255]);
    }

    #[test]
    fn xtsmgraphics_replies() {
        let mut regs = COLOR_REGISTERS;
        assert_eq!(
            xtsmgraphics(&[1, 1], (800, 600), &mut regs),
            "\x1b[?1;0;256S"
        );
        assert_eq!(
            xtsmgraphics(&[1, 3, 16], (800, 600), &mut regs),
            "\x1b[?1;0;16S"
        );
        assert_eq!(
            xtsmgraphics(&[1, 4], (800, 600), &mut regs),
            "\x1b[?1;0;256S"
        );
        assert_eq!(
            xtsmgraphics(&[2, 1], (800, 600), &mut regs),
            "\x1b[?2;0;800;600S"
        );
        assert_eq!(xtsmgraphics(&[3, 1], (800, 600), &mut regs), "\x1b[?3;1;0S");
        assert_eq!(xtsmgraphics(&[1, 9], (800, 600), &mut regs), "\x1b[?1;2;0S");
    }
}
