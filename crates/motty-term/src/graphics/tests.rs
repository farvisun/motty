//! End-to-end: bytes through the scanner, the VT parser and [`Graphics`],
//! as the PTY thread runs them.

use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use base64::Engine as _;

use super::*;
use crate::GridSize;
use crate::graphemes::{Cluster, Graphemes};

/// A PNG of `w` × `h` red pixels.
pub fn png(w: u32, h: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A kitty command with RGBA pixels of a `w` × `h` image.
fn kitty_rgba(control: &str, w: u32, h: u32) -> Vec<u8> {
    let data = b64(&vec![7; (w * h * 4) as usize]);
    format!("\x1b_Gf=32,s={w},v={h},{control};{data}\x1b\\").into_bytes()
}

struct Harness {
    term: Term<VoidListener>,
    graphics: Graphics,
    scanner: Scanner,
    parser: Processor,
    cluster: Cluster,
    replies: Vec<String>,
}

impl Harness {
    /// A terminal with 10 × 20 pixel cells.
    fn new(cols: u16, rows: u16, history: usize) -> Self {
        let size = GridSize {
            cols,
            rows,
            cell_width: 10.,
            cell_height: 20.,
        };
        let config = Config {
            scrolling_history: history,
            ..Config::default()
        };
        let mut graphics = Graphics::default();
        graphics.set_cell_size(10, 20);
        Self {
            term: Term::new(config, &size, VoidListener),
            graphics,
            scanner: Scanner::default(),
            parser: Processor::new(),
            cluster: Cluster::default(),
            replies: Vec::new(),
        }
    }

    fn parse(&mut self, bytes: &[u8]) {
        self.parser.advance(
            &mut Graphemes {
                term: &mut self.term,
                cluster: &mut self.cluster,
                pointer: &mut None,
                repaint: &mut crate::repaint::Repaint::default(),
                graphics: &mut self.graphics,
            },
            bytes,
        );
    }

    /// Mirrors `PtyLoop::read` and `PtyLoop::image`.
    fn feed(&mut self, input: impl AsRef<[u8]>) {
        let input = input.as_ref();
        let mut pos = 0;
        while pos < input.len() {
            let (used, step) = self.scanner.next(&input[pos..]);
            let bytes = &input[pos..pos + used];
            pos += used;
            match step {
                Step::Pass => self.parse(bytes),
                Step::PassHeld(held) => self.parse(&held),
                Step::Sequence(seq) => {
                    let synced = self.parser.sync_timeout().sync_timeout().is_some();
                    if synced {
                        self.parser.stop_sync(&mut Graphemes {
                            term: &mut self.term,
                            cluster: &mut self.cluster,
                            pointer: &mut None,
                            repaint: &mut crate::repaint::Repaint::default(),
                            graphics: &mut self.graphics,
                        });
                    }
                    if let Work::Decode(job) = self.graphics.begin(seq, &mut self.term) {
                        self.graphics.finish(job.run(), &mut self.term);
                    }
                    self.replies.extend(self.graphics.take_replies());
                    if synced {
                        self.parse(b"\x1b[?2026h");
                    }
                }
                Step::Pending => {}
            }
        }
        self.graphics.prune(&self.term);
    }

    fn take_replies(&mut self) -> Vec<String> {
        std::mem::take(&mut self.replies)
    }

    fn cursor(&self) -> (i32, usize) {
        let p = self.term.grid().cursor.point;
        (p.line.0, p.column.0)
    }

    fn placements(&self) -> &[Placement] {
        self.graphics
            .placements(self.term.mode().contains(TermMode::ALT_SCREEN))
    }

    /// Placements as (screen line, column, cols, rows).
    fn on_screen(&self) -> Vec<(i64, u32, u32, u32)> {
        let pushed = self.term.grid().lines_pushed() as i64;
        self.placements()
            .iter()
            .map(|p| (p.row as i64 - pushed, p.col, p.cols, p.rows))
            .collect()
    }

    fn images(&self) -> usize {
        self.graphics
            .screen(self.term.mode().contains(TermMode::ALT_SCREEN))
            .images
            .len()
    }
}

#[test]
fn kitty_places_at_the_cursor_and_moves_it_after() {
    let mut h = Harness::new(20, 10, 100);
    h.feed("ab");
    // 25 × 45 pixels on 10 × 20 cells: 3 × 3 cells.
    h.feed(kitty_rgba("a=T,i=1", 25, 45));
    assert_eq!(h.on_screen(), vec![(0, 2, 3, 3)]);
    assert_eq!(h.cursor(), (2, 5));
    assert_eq!(h.take_replies(), vec!["\x1b_Gi=1;OK\x1b\\"]);
    // C=1 leaves the cursor; no id, no reply.
    h.feed(kitty_rgba("a=T,C=1", 10, 20));
    assert_eq!(h.cursor(), (2, 5));
    assert!(h.take_replies().is_empty());
    assert_eq!(h.placements().len(), 2);
}

#[test]
fn kitty_sizes_source_rects_offsets_and_z() {
    let mut h = Harness::new(20, 10, 100);
    h.feed(kitty_rgba(
        "a=T,i=1,x=10,y=10,w=20,h=20,X=4,Y=6,z=-5,C=1",
        40,
        40,
    ));
    let p = &h.placements()[0];
    assert_eq!(
        p.src,
        Rect {
            x: 10.,
            y: 10.,
            w: 20.,
            h: 20.
        }
    );
    assert_eq!((p.dest.x, p.dest.y, p.dest.w, p.dest.h), (0.4, 0.3, 2., 1.));
    assert_eq!((p.cols, p.rows, p.z), (3, 2, -5));
    // c and r: fit and centre (letterbox) in 4 × 4 cells = 40 × 80 pixels.
    h.feed("\x1b_Ga=p,i=1,p=2,c=4,r=4,C=1\x1b\\");
    let p = h.placements().last().unwrap();
    assert_eq!((p.cols, p.rows), (4, 4));
    // The whole 40 × 40 image, centred vertically.
    assert_eq!((p.dest.w, p.dest.h, p.dest.y), (4., 2., 1.));
    // c alone keeps the aspect ratio.
    h.feed("\x1b_Ga=p,i=1,p=3,c=2,C=1\x1b\\");
    let p = h.placements().last().unwrap();
    assert_eq!((p.cols, p.rows, p.dest.h), (2, 1, 1.));
}

#[test]
fn kitty_chunked_zlib_and_png_transmissions() {
    let mut h = Harness::new(20, 10, 100);
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut z, &[1u8; 10 * 20 * 3]).unwrap();
    let data = b64(&z.finish().unwrap());
    let (first, rest) = data.split_at(8);
    h.feed(format!(
        "\x1b_Ga=t,i=5,f=24,s=10,v=20,o=z,m=1;{first}\x1b\\"
    ));
    assert!(h.take_replies().is_empty());
    for (i, chunk) in rest.as_bytes().chunks(8).enumerate() {
        let more = usize::from((i + 1) * 8 < rest.len());
        h.feed(format!(
            "\x1b_Gm={more};{}\x1b\\",
            std::str::from_utf8(chunk).unwrap()
        ));
    }
    assert_eq!(h.take_replies(), vec!["\x1b_Gi=5;OK\x1b\\"]);
    assert_eq!(h.images(), 1);
    assert!(h.placements().is_empty());
    // PNG, displayed from the stored id.
    h.feed(format!(
        "\x1b_Ga=t,i=6,f=100,q=1;{}\x1b\\",
        b64(&png(30, 30))
    ));
    h.feed("\x1b_Ga=p,i=6\x1b\\");
    assert_eq!(h.on_screen(), vec![(0, 0, 3, 2)]);
    assert_eq!(h.take_replies(), vec!["\x1b_Gi=6;OK\x1b\\"]);
}

#[test]
fn kitty_queries_errors_and_numbers() {
    let mut h = Harness::new(20, 10, 100);
    // a=q answers without storing.
    h.feed("\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\");
    assert_eq!(h.take_replies(), vec!["\x1b_Gi=31;OK\x1b\\"]);
    assert_eq!(h.images(), 0);
    h.feed("\x1b_Ga=p,i=99\x1b\\");
    assert!(h.take_replies()[0].starts_with("\x1b_Gi=99;ENOENT"));
    h.feed("\x1b_Gi=3,f=24,s=2,v=2;AAAA\x1b\\");
    assert!(h.take_replies()[0].starts_with("\x1b_Gi=3;ENODATA"));
    h.feed("\x1b_Ga=f,i=3;AAAA\x1b\\");
    assert!(h.take_replies()[0].starts_with("\x1b_Gi=3;ENOTSUP"));
    h.feed("\x1b_Gi=4,t=s;L3g=\x1b\\");
    assert!(h.take_replies()[0].starts_with("\x1b_Gi=4;ENOTSUP"));
    // I: the terminal picks the id; `a=p,I=` uses the newest.
    h.feed(kitty_rgba("a=t,I=13", 1, 1));
    let reply = h.take_replies().remove(0);
    assert!(reply.starts_with("\x1b_Gi=") && reply.ends_with(",I=13;OK\x1b\\"));
    h.feed("\x1b_Ga=p,I=13\x1b\\");
    assert_eq!(h.placements().len(), 1);
    // A feature query.
    h.feed("\x1b_Ga=q,i=31,e=T,U=1\x1b\\");
    assert_eq!(
        h.take_replies().pop().unwrap(),
        "\x1b_Gi=31,e=T,U=1;OK\x1b\\"
    );
}

#[test]
fn kitty_same_placement_id_replaces_and_new_data_drops_placements() {
    let mut h = Harness::new(20, 10, 100);
    h.feed(kitty_rgba("a=T,i=1,p=1,C=1", 10, 20));
    h.feed("\x1b[5;5H\x1b_Ga=p,i=1,p=1,C=1\x1b\\");
    assert_eq!(h.on_screen(), vec![(4, 4, 1, 1)]);
    h.feed("\x1b_Ga=p,i=1,p=2,C=1\x1b\\");
    assert_eq!(h.placements().len(), 2);
    // Sending image 1 again deletes its placements.
    h.feed(kitty_rgba("a=t,i=1", 10, 20));
    assert!(h.placements().is_empty());
    assert_eq!(h.images(), 1);
}

#[test]
fn kitty_deletes() {
    let mut h = Harness::new(20, 10, 100);
    let setup = |h: &mut Harness| {
        h.feed("\x1b_Ga=d,d=A\x1b\\");
        h.feed("\x1b[H");
        h.feed(kitty_rgba("a=T,i=1,p=1,C=1", 20, 20));
        h.feed("\x1b[3;5H\x1b_Ga=p,i=1,p=2,z=3,C=1\x1b\\");
        h.feed("\x1b[6;1H");
        h.feed(kitty_rgba("a=T,i=2,C=1", 10, 20));
        h.feed("\x1b[H");
    };
    let count = |h: &mut Harness, cmd: &str| {
        setup(h);
        h.feed(format!("\x1b_Ga=d,{cmd}\x1b\\"));
        (h.placements().len(), h.images())
    };
    assert_eq!(count(&mut h, "d=a"), (0, 2));
    assert_eq!(count(&mut h, "d=A"), (0, 0));
    assert_eq!(count(&mut h, "d=i,i=1"), (1, 2));
    assert_eq!(count(&mut h, "d=i,i=1,p=2"), (2, 2));
    assert_eq!(count(&mut h, "d=I,i=2"), (2, 1));
    // At the cursor (top-left), at a cell, by z, by column, by row.
    assert_eq!(count(&mut h, "d=c"), (2, 2));
    assert_eq!(count(&mut h, "d=p,x=6,y=3"), (2, 2));
    assert_eq!(count(&mut h, "d=q,x=6,y=3,z=0"), (3, 2));
    assert_eq!(count(&mut h, "d=z,z=3"), (2, 2));
    assert_eq!(count(&mut h, "d=x,x=1"), (1, 2));
    assert_eq!(count(&mut h, "d=y,y=6"), (2, 2));
    assert_eq!(count(&mut h, "d=R,x=1,y=2"), (0, 0));
    // An upload in progress is abandoned by a delete.
    h.feed("\x1b_Ga=t,i=9,f=24,s=1,v=1,m=1;AA\x1b\\\x1b_Ga=d\x1b\\\x1b_Gm=0;AA\x1b\\");
    assert!(!h.graphics.screen(false).images.contains_key(&9));
}

#[test]
fn kitty_virtual_placements_for_placeholders() {
    let mut h = Harness::new(20, 10, 100);
    h.feed(kitty_rgba("a=T,U=1,i=42,c=4,r=2,q=2", 20, 20));
    // Virtual placements don't move the cursor or show by themselves.
    assert_eq!(h.cursor(), (0, 0));
    assert!(h.placements().is_empty());
    let (image, v) = h.graphics.virtual_placement(false, 42, 0).unwrap();
    assert_eq!((image.width, v.cols, v.rows), (20, 4, 2));
    // 20 × 20 scaled to fill the 4 × 2 cells (40 × 40 pixels).
    assert_eq!((v.dest.x, v.dest.w, v.dest.h), (0., 4., 2.));
    // The placeholder text reaches the grid as a character with diacritics.
    h.feed("\x1b[38;5;42m\u{10EEEE}\u{305}\u{30D}\u{10EEEE}\x1b[39m");
    let row = &h.term.grid()[Line(0)];
    let cells: Vec<_> = (0..2)
        .filter_map(|c| placeholder::Raw::from_cell(&row[Column(c)]).map(|r| (c, r)))
        .collect();
    let resolved = placeholder::resolve_row(&cells);
    assert_eq!(
        resolved
            .iter()
            .map(|(_, r)| (r.image_id, r.row, r.col))
            .collect::<Vec<_>>(),
        vec![(42, 0, 1), (42, 0, 2)]
    );
    // d=i removes virtual placements too.
    h.feed("\x1b_Ga=d,d=I,i=42\x1b\\");
    assert!(!h.graphics.has_virtual(false));
    assert_eq!(h.images(), 0);
}

#[test]
fn placements_scroll_with_text_and_leave_with_the_scrollback() {
    let mut h = Harness::new(20, 4, 3);
    h.feed("\r\n");
    h.feed(kitty_rgba("a=T,i=1,C=1", 10, 20));
    assert_eq!(h.on_screen(), vec![(1, 0, 1, 1)]);
    h.feed("\r\n\r\n\r\n\r\n");
    // Two lines up the screen (from line 1), then into the scrollback.
    assert_eq!(h.on_screen(), vec![(-1, 0, 1, 1)]);
    h.feed("\r\n\r\n");
    assert_eq!(h.on_screen(), vec![(-3, 0, 1, 1)]);
    // History holds 3 lines: one more and its row is gone.
    h.feed("\r\n");
    assert!(h.placements().is_empty());
    // Image data stays for later placements.
    assert_eq!(h.images(), 1);

    // Clearing the scrollback (ED 3) takes placements there with it.
    h.feed(kitty_rgba("a=T,i=1,C=1", 10, 20));
    h.feed("\r\n\r\n\r\n\r\n\x1b[3J");
    assert!(h.placements().is_empty());
}

#[test]
fn removed_history_rows_shift_newer_placements() {
    let mut g = Graphics::default();
    let mut place = |row: u64| {
        g.main.placements.push(Placement {
            image: Image::new(Pixels {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            }),
            row,
            col: 0,
            cols: 1,
            rows: 1,
            dest: Rect::default(),
            src: Rect::default(),
            z: 0,
            origin: Origin::Kitty {
                image: 1,
                placement: 0,
            },
            stamp: 0,
            erased: Vec::new(),
            erased_count: 0,
        });
    };
    // Images on stable rows 0, 2 and 5.
    place(0);
    place(2);
    place(5);
    // Rows 2 and 3 go: row 0 keeps its name, row 2 goes, row 5 becomes 3.
    g.rows_removed(false, 2, 2);
    let rows: Vec<_> = g.placements(false).iter().map(|p| p.row).collect();
    assert_eq!(rows, vec![0, 3]);
}

#[test]
fn clear_alternate_screen_and_reset() {
    let mut h = Harness::new(20, 4, 100);
    h.feed(kitty_rgba("a=T,i=1", 10, 20));
    // The alternate screen has its own images, and starts empty.
    h.feed("\x1b[?1049h");
    assert!(h.placements().is_empty());
    assert_eq!(h.images(), 0);
    h.feed(kitty_rgba("a=T,i=1", 10, 20));
    h.feed("\x1b[?1049l\x1b[?1049h");
    assert!(h.placements().is_empty());
    h.feed("\x1b[?1049l");
    assert_eq!(h.placements().len(), 1);
    // ED 2 clears kitty placements on screen.
    h.feed("\x1b[2J");
    assert!(h.placements().is_empty());
    h.feed(kitty_rgba("a=T,i=1", 10, 20));
    // RIS clears everything.
    h.feed("\x1bc");
    assert!(h.placements().is_empty());
    assert_eq!(h.images(), 0);
}

/// A 20 × 40 pixel sixel (2 × 2 cells): `!20~` twice per band.
fn sixel(p2: u8) -> String {
    let band = "#1!20~";
    format!("\x1bP0;{p2};0q\"1;1;20;40{band}-{band}-{band}-{band}-{band}-{band}-{band}\x1b\\")
}

#[test]
fn sixel_places_at_cursor_and_moves_below() {
    let mut h = Harness::new(20, 10, 100);
    h.feed("abc");
    h.feed(sixel(1));
    // 42 pixel rows (7 bands) on 20-pixel cells: 3 rows.
    assert_eq!(h.on_screen(), vec![(0, 3, 2, 3)]);
    assert_eq!(h.cursor(), (3, 3));
    assert!(matches!(h.placements()[0].origin, Origin::Sixel));
    assert_eq!(h.placements()[0].z, 0);
}

#[test]
fn text_over_cell_images_wipes_them() {
    let mut h = Harness::new(20, 10, 100);
    h.feed(sixel(0));
    // Writing over one cell wipes it.
    h.feed("\x1b[1;2HX");
    let p = &h.placements()[0];
    assert!(p.is_erased(0, 1) && !p.is_erased(0, 0));
    // Erasing the rest (EL, ECH, ED) removes the image.
    h.feed("\x1b[1;1H\x1b[K\x1b[2;1H\x1b[2X\x1b[3;1H\x1b[J");
    assert!(h.placements().is_empty());
    assert!(!h.graphics.has_cell_images());
    // A new image over an old one replaces it.
    h.feed("\x1b[H");
    h.feed(sixel(1));
    h.feed("\x1b[H");
    h.feed(sixel(1));
    assert_eq!(h.placements().len(), 1);
    // Kitty placements aren't wiped by text.
    h.feed("\x1b[5;1H");
    h.feed(kitty_rgba("a=T,C=1", 10, 20));
    h.feed("XYZ");
    assert_eq!(h.placements().len(), 2);
}

#[test]
fn iterm_inline_images() {
    let mut h = Harness::new(40, 10, 100);
    let image = b64(&png(25, 30));
    h.feed("ab");
    h.feed(format!(
        "\x1b]1337;File=name={};inline=1:{image}\x07",
        b64(b"x.png")
    ));
    // 25 × 30 pixels: 3 × 2 cells; the cursor ends after the bottom-right.
    assert_eq!(h.on_screen(), vec![(0, 2, 3, 2)]);
    assert_eq!(h.cursor(), (1, 5));
    assert!(matches!(h.placements()[0].origin, Origin::Iterm));
    // Width in cells, aspect kept.
    h.feed(format!(
        "\r\n\x1b]1337;File=inline=1;width=10:{image}\x1b\\"
    ));
    let p = h.placements().last().unwrap();
    assert_eq!((p.cols, p.rows, p.dest.w, p.dest.h), (10, 6, 10., 6.));
    // Not inline: a download, ignored.
    h.feed(format!("\x1b]1337;File=name=eA==:{image}\x07"));
    assert_eq!(h.placements().len(), 2);
    // Multipart.
    h.feed("\r\n\x1b]1337;MultipartFile=inline=1\x07");
    for part in image.as_bytes().chunks(12) {
        h.feed(format!(
            "\x1b]1337;FilePart={}\x07",
            std::str::from_utf8(part).unwrap()
        ));
    }
    h.feed("\x1b]1337;FileEnd\x07");
    assert_eq!(h.placements().len(), 3);
}

#[test]
fn xtsmgraphics_is_answered() {
    let mut h = Harness::new(80, 24, 100);
    h.feed("\x1b[?1;1;0S\x1b[?2;1;0S");
    assert_eq!(
        h.take_replies(),
        vec!["\x1b[?1;0;256S", "\x1b[?2;0;800;480S"]
    );
}

#[test]
fn images_in_a_synchronized_update_land_after_the_held_text() {
    let mut h = Harness::new(20, 10, 100);
    h.feed("\x1b[?2026habc");
    // The text is still held back by the update...
    assert_eq!(h.cursor(), (0, 0));
    h.feed(kitty_rgba("a=T,i=1,C=1", 10, 20));
    // ...but goes first, so the image is where the program meant.
    assert_eq!(h.on_screen(), vec![(0, 3, 1, 1)]);
    // The update carries on afterwards.
    h.feed("def");
    assert_eq!(h.cursor(), (0, 3));
    h.feed("\x1b[?2026l");
    assert_eq!(h.cursor(), (0, 6));
    assert_eq!(h.term.grid()[Point::new(Line(0), Column(5))].c, 'f');
    assert_eq!(h.term.columns(), 20);
}

/// Output without images must not get slower with the scanner in front:
/// `cargo test --release -p motty-term scanner_overhead -- --ignored --nocapture`.
#[test]
#[ignore]
fn scanner_overhead() {
    use std::time::Instant;
    let plain: Vec<u8> = "the quick brown fox jumps over the lazy dog 0123456789\r\n"
        .repeat(600_000)
        .into_bytes();
    let colored: Vec<u8> =
        "\x1b[01;34mdir\x1b[0m  \x1b[01;32mrun.sh\x1b[0m  \x1b[38;5;208mnote\x1b[m.txt\r\n"
            .repeat(600_000)
            .into_bytes();
    for (name, input) in [("plain", &plain), ("colored", &colored)] {
        let time = |scan: bool| {
            (0..5)
                .map(|_| {
                    let mut h = Harness::new(120, 40, 10_000);
                    let start = Instant::now();
                    for chunk in input.chunks(0x1_0000) {
                        if scan {
                            h.feed(chunk);
                        } else {
                            h.parse(chunk);
                        }
                    }
                    start.elapsed()
                })
                .min()
                .unwrap()
        };
        let scan_only = (0..5)
            .map(|_| {
                let mut scanner = Scanner::default();
                let start = Instant::now();
                for chunk in input.chunks(0x1_0000) {
                    let mut pos = 0;
                    while pos < chunk.len() {
                        pos += scanner.next(&chunk[pos..]).0;
                    }
                }
                start.elapsed()
            })
            .min()
            .unwrap();
        println!(
            "{name}: scanner alone {:.1} ms",
            scan_only.as_secs_f64() * 1e3
        );
        // Interleaved, best of each, so warm-up and allocator state even out.
        let (mut without, mut with) = (time(false), time(true));
        for _ in 0..2 {
            with = with.min(time(true));
            without = without.min(time(false));
        }
        let mb = input.len() as f64 / 1e6;
        println!(
            "{name}: {mb:.0} MB, parser only {:.1} ms ({:.0} MB/s), with scanner {:.1} ms ({:.0} MB/s), {:+.1}%",
            without.as_secs_f64() * 1e3,
            mb / without.as_secs_f64(),
            with.as_secs_f64() * 1e3,
            mb / with.as_secs_f64(),
            (with.as_secs_f64() / without.as_secs_f64() - 1.) * 100.
        );
    }
}
