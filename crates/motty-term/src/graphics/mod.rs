//! Inline images: the kitty graphics protocol, iTerm2's `OSC 1337 ; File=`
//! and Sixel.
//!
//! [`scan::Scanner`] cuts the image sequences out of the PTY stream (vte
//! drops APC and alacritty ignores sixel DCS); the PTY thread hands each one
//! to [`Graphics::begin`] at its exact place in the stream, so the cursor is
//! where the program left it. Decoding runs off the terminal lock
//! ([`Decode::run`]), then [`Graphics::finish`] places the image and moves
//! the cursor.
//!
//! Placements are anchored to stable rows (`Grid::lines_pushed` + screen
//! line), so they scroll with the text and go when their rows leave the
//! scrollback ([`Graphics::prune`]). Main and alternate screens keep their
//! own images, as in kitty. Sixel and iTerm2 images behave like cell
//! content: text written or erased over a cell wipes that part of the
//! image. Kitty placements are a layer of their own: only deletes, a full
//! clear (ED 2), entering the alternate screen and RIS remove them. Scroll
//! regions that don't start at the top move text without moving images.

pub mod iterm;
pub mod kitty;
pub mod placeholder;
pub mod scan;
pub mod sixel;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Column;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Handler;

pub use scan::{Scanner, Sequence, Step};

/// Decoded image data kept per terminal (both screens). Over it, images
/// without placements go first, then the oldest.
pub const QUOTA: usize = 320 << 20;

/// Decoded RGBA pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Pixels {
    fn from_image(image: image::DynamicImage) -> Result<Pixels, String> {
        if image.width() > kitty::MAX_SIDE || image.height() > kitty::MAX_SIDE {
            return Err("EFBIG:image too large".into());
        }
        let image = image.into_rgba8();
        Ok(Pixels {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        })
    }
}

/// An image to draw, shared by its placements and the renderer.
#[derive(Debug)]
pub struct Image {
    /// Unique in the process: the renderer's cache key.
    pub serial: u64,
    pub width: u32,
    pub height: u32,
    /// RGBA, straight alpha.
    pub rgba: Vec<u8>,
}

impl Image {
    fn new(pixels: Pixels) -> Arc<Image> {
        static SERIAL: AtomicU64 = AtomicU64::new(1);
        Arc::new(Image {
            serial: SERIAL.fetch_add(1, Ordering::Relaxed),
            width: pixels.width,
            height: pixels.height,
            rgba: pixels.rgba,
        })
    }

    fn bytes(&self) -> usize {
        self.rgba.len()
    }
}

/// A rectangle: in cells for where an image goes, in image pixels for
/// which part of it shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Which protocol made a placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Kitty { image: u32, placement: u32 },
    Sixel,
    Iterm,
}

/// An image shown on the grid.
#[derive(Debug)]
pub struct Placement {
    pub image: Arc<Image>,
    /// Stable row of the top cell: `Grid::lines_pushed` + screen line.
    pub row: u64,
    pub col: u32,
    /// The cells it covers.
    pub cols: u32,
    pub rows: u32,
    /// Where it's drawn, in cells from the top-left of (`row`, `col`).
    pub dest: Rect,
    /// The part of the image shown, in its pixels.
    pub src: Rect,
    pub z: i32,
    pub origin: Origin,
    /// Creation order, for stacking equal z-indices.
    pub stamp: u64,
    /// Cells text has written over, row-major (empty: none).
    erased: Vec<bool>,
    erased_count: usize,
}

impl Placement {
    /// Whether text has wiped the cell `row`, `col` (relative to the top-left).
    pub fn is_erased(&self, row: u32, col: u32) -> bool {
        !self.erased.is_empty() && self.erased[(row * self.cols + col) as usize]
    }

    pub fn any_erased(&self) -> bool {
        self.erased_count > 0
    }

    /// The kitty image id (0 for sixel and iTerm2), which orders images of
    /// equal z-index.
    pub fn origin_id(&self) -> u32 {
        self.kitty_image().unwrap_or(0)
    }

    fn is_cell_image(&self) -> bool {
        !matches!(self.origin, Origin::Kitty { .. })
    }

    fn kitty_image(&self) -> Option<u32> {
        match self.origin {
            Origin::Kitty { image, .. } => Some(image),
            _ => None,
        }
    }

    fn intersects(&self, rows: &Range<u64>, cols: &Range<u32>) -> bool {
        self.row < rows.end
            && rows.start < self.row + u64::from(self.rows)
            && self.col < cols.end
            && cols.start < self.col + self.cols
    }

    /// Wipes the cells in `rows` × `cols`; true once every cell is gone.
    fn erase(&mut self, rows: &Range<u64>, cols: &Range<u32>) -> bool {
        if !self.intersects(rows, cols) {
            return false;
        }
        if self.erased.is_empty() {
            self.erased = vec![false; (self.rows * self.cols) as usize];
        }
        let r0 = (rows.start.max(self.row) - self.row) as u32;
        let r1 = (rows.end.min(self.row + u64::from(self.rows)) - self.row) as u32;
        let c0 = cols.start.max(self.col) - self.col;
        let c1 = cols.end.min(self.col + self.cols) - self.col;
        for r in r0..r1 {
            for c in c0..c1 {
                let cell = &mut self.erased[(r * self.cols + c) as usize];
                if !*cell {
                    *cell = true;
                    self.erased_count += 1;
                }
            }
        }
        self.erased_count == self.erased.len()
    }
}

/// A kitty virtual placement (`U=1`): shown wherever Unicode placeholders
/// for its image are.
#[derive(Clone, Debug, PartialEq)]
pub struct Virtual {
    pub placement: u32,
    /// The box the placeholder cells make up.
    pub cols: u32,
    pub rows: u32,
    /// Where the image goes in that box, in cells.
    pub dest: Rect,
    pub src: Rect,
    pub z: i32,
}

struct KittyImage {
    image: Arc<Image>,
    number: u32,
    /// Whether the program named it (`i` or `I`); unnamed images take no
    /// placement ids.
    named: bool,
    /// Creation and last use, for `I` lookups and eviction.
    created: u64,
    used: u64,
    virtuals: Vec<Virtual>,
}

/// One screen's images.
#[derive(Default)]
struct Screen {
    images: HashMap<u32, KittyImage>,
    placements: Vec<Placement>,
}

impl Screen {
    fn remove_image(&mut self, id: u32) {
        self.images.remove(&id);
        self.placements.retain(|p| p.kitty_image() != Some(id));
    }

    fn wipe(&mut self, rows: &Range<u64>, cols: &Range<u32>) {
        self.placements
            .retain_mut(|p| !p.is_cell_image() || !p.erase(rows, cols));
    }

    /// Removes kitty placements matching `pred`; returns their images.
    fn delete(&mut self, mut pred: impl FnMut(&Placement) -> bool) -> Vec<u32> {
        let mut touched = Vec::new();
        self.placements.retain(|p| match p.kitty_image() {
            Some(id) if pred(p) => {
                touched.push(id);
                false
            }
            _ => true,
        });
        touched
    }

    /// Frees images that nothing shows any more.
    fn free(&mut self, ids: &[u32]) {
        for id in ids {
            let placed = self.placements.iter().any(|p| p.kitty_image() == Some(*id));
            if !placed && self.images.get(id).is_some_and(|i| i.virtuals.is_empty()) {
                self.images.remove(id);
            }
        }
    }

    fn newest(&self, number: u32) -> Option<u32> {
        self.images
            .iter()
            .filter(|(_, i)| i.number == number)
            .max_by_key(|(_, i)| i.created)
            .map(|(id, _)| *id)
    }
}

/// A kitty upload arriving in chunks (`m=1`).
struct Upload {
    cmd: kitty::Command,
    payload: kitty::Payload,
}

/// Images and placements of one terminal.
pub struct Graphics {
    main: Screen,
    alt: Screen,
    /// A cell in pixels, as programs are told (`TIOCGWINSZ`).
    cell: (u32, u32),
    upload: Option<Upload>,
    multipart: Option<(iterm::Args, kitty::Payload)>,
    next_id: u32,
    clock: u64,
    /// Sixel and iTerm2 placements on either screen: text output only
    /// looks for them when there are some.
    cell_images: usize,
    /// XTSMGRAPHICS colour registers.
    registers: u32,
    replies: Vec<String>,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            main: Screen::default(),
            alt: Screen::default(),
            cell: (8, 16),
            upload: None,
            multipart: None,
            next_id: 1 << 31,
            clock: 0,
            cell_images: 0,
            registers: sixel::COLOR_REGISTERS,
            replies: Vec::new(),
        }
    }
}

/// Work left after [`Graphics::begin`].
pub enum Work {
    Done,
    /// Run [`Decode::run`] (without the terminal lock), then
    /// [`Graphics::finish`].
    Decode(Decode),
}

/// Image data to decode.
pub struct Decode(Job);

enum Job {
    Kitty(kitty::Load),
    Sixel(sixel::Params, Vec<u8>),
    /// Base64 (`File=`) or already decoded (multipart) file contents.
    Iterm(iterm::Args, Result<Vec<u8>, Vec<u8>>),
}

/// A decoded image, for [`Graphics::finish`].
pub struct Decoded(Loaded);

enum Loaded {
    Kitty(kitty::Command, Result<Pixels, String>),
    Sixel(Option<Pixels>),
    Iterm(iterm::Args, Result<Pixels, String>),
}

impl Decode {
    pub fn run(self) -> Decoded {
        Decoded(match self.0 {
            Job::Kitty(load) => Loaded::Kitty(load.cmd.clone(), load.run()),
            Job::Sixel(params, data) => Loaded::Sixel(sixel::decode(params, &data)),
            Job::Iterm(args, data) => {
                let bytes = match data {
                    Ok(base64) => {
                        let mut p = kitty::Payload::default();
                        p.push(&base64).and_then(|_| p.finish())
                    }
                    Err(bytes) => Ok(bytes),
                };
                Loaded::Iterm(args, bytes.and_then(|b| iterm::decode(&b)))
            }
        })
    }
}

/// Where the cursor goes after an image.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Move {
    Stay,
    /// Just right of the bottom-right cell (kitty, iTerm2).
    AfterRight,
    /// Under the bottom-left cell (sixel).
    Below,
}

fn is_alt<T>(term: &Term<T>) -> bool {
    term.mode().contains(TermMode::ALT_SCREEN)
}

/// The stable row and column of the cursor.
fn cursor_cell<T>(term: &Term<T>) -> (u64, u32) {
    let grid = term.grid();
    let point = grid.cursor.point;
    (
        grid.lines_pushed() + point.line.0.max(0) as u64,
        point.column.0 as u32,
    )
}

impl Graphics {
    /// Sets the cell size programs are told, in pixels.
    pub fn set_cell_size(&mut self, width: u16, height: u16) {
        if width > 0 && height > 0 {
            self.cell = (width.into(), height.into());
        }
    }

    /// Replies for the PTY, oldest first.
    pub fn take_replies(&mut self) -> Vec<String> {
        std::mem::take(&mut self.replies)
    }

    /// Placements on the main or alternate screen.
    pub fn placements(&self, alt: bool) -> &[Placement] {
        &self.screen(alt).placements
    }

    pub fn has_virtual(&self, alt: bool) -> bool {
        self.screen(alt)
            .images
            .values()
            .any(|i| !i.virtuals.is_empty())
    }

    /// The virtual placement placeholders for `image_id` / `placement_id`
    /// (0: any) refer to.
    pub fn virtual_placement(
        &self,
        alt: bool,
        image_id: u32,
        placement_id: u32,
    ) -> Option<(&Arc<Image>, &Virtual)> {
        let image = self.screen(alt).images.get(&image_id)?;
        let v = image
            .virtuals
            .iter()
            .find(|v| placement_id == 0 || v.placement == placement_id)?;
        Some((&image.image, v))
    }

    /// Whether text output has sixel or iTerm2 images to wipe.
    #[inline]
    pub fn has_cell_images(&self) -> bool {
        self.cell_images > 0
    }

    fn screen(&self, alt: bool) -> &Screen {
        if alt { &self.alt } else { &self.main }
    }

    fn screen_mut(&mut self, alt: bool) -> &mut Screen {
        if alt { &mut self.alt } else { &mut self.main }
    }

    fn recount(&mut self) {
        self.cell_images = [&self.main, &self.alt]
            .iter()
            .map(|s| s.placements.iter().filter(|p| p.is_cell_image()).count())
            .sum();
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn cell_f32(&self) -> (f32, f32) {
        (self.cell.0 as f32, self.cell.1 as f32)
    }

    /// Text written or erased over screen `lines` × `cols` wipes the sixel
    /// and iTerm2 images there.
    pub fn wipe<T>(&mut self, term: &Term<T>, lines: Range<i32>, cols: Range<usize>) {
        if !self.has_cell_images() || lines.is_empty() || cols.is_empty() {
            return;
        }
        let pushed = term.grid().lines_pushed();
        let rows = pushed + lines.start.max(0) as u64..pushed + lines.end.max(0) as u64;
        let cols = cols.start as u32..cols.end as u32;
        self.screen_mut(is_alt(term)).wipe(&rows, &cols);
        self.recount();
    }

    /// ED 2: kitty placements on screen go (sixel and iTerm2 images are
    /// wiped with their cells by the caller).
    pub fn clear_screen<T>(&mut self, term: &Term<T>) {
        let pushed = term.grid().lines_pushed();
        let rows = pushed..pushed + term.screen_lines() as u64;
        self.screen_mut(is_alt(term))
            .delete(|p| p.intersects(&rows, &(0..u32::MAX)));
    }

    /// Entering the alternate screen starts it without images.
    pub fn clear_alt(&mut self) {
        self.alt.placements.clear();
        self.recount();
    }

    /// RIS: everything goes.
    pub fn reset(&mut self) {
        let keep = (self.cell, self.next_id);
        *self = Graphics::default();
        (self.cell, self.next_id) = keep;
    }

    /// Drops placements whose rows have left the active screen's scrollback.
    pub fn prune<T>(&mut self, term: &Term<T>) {
        let screen = self.screen_mut(is_alt(term));
        if screen.placements.is_empty() {
            return;
        }
        let grid = term.grid();
        let top = grid.lines_pushed() as i64 - grid.history_size() as i64;
        let before = screen.placements.len();
        screen
            .placements
            .retain(|p| p.row as i64 + i64::from(p.rows) > top);
        if screen.placements.len() != before {
            self.recount();
        }
    }

    /// `count` history rows from stable row `first` were removed from the
    /// grid and every newer row renamed `count` lower (as
    /// `Grid::remove_history_lines` does): placements on newer rows follow
    /// their text, those on the removed rows go. Call it with the stable
    /// row taken before the removal.
    pub fn rows_removed(&mut self, alt: bool, first: u64, count: u64) {
        let end = first + count;
        self.screen_mut(alt).placements.retain_mut(|p| {
            if p.row + u64::from(p.rows) <= first {
                true
            } else if p.row >= end {
                p.row -= count;
                true
            } else {
                false
            }
        });
        self.recount();
    }

    /// Handles a sequence as far as it can under the terminal lock.
    pub fn begin<T: EventListener>(&mut self, seq: Sequence, term: &mut Term<T>) -> Work {
        match seq {
            Sequence::Kitty(body) => self.kitty(&body, term),
            Sequence::Sixel { params, data } => {
                Work::Decode(Decode(Job::Sixel(sixel::Params::new(&params), data)))
            }
            Sequence::Iterm(body) => self.iterm(&body),
            Sequence::Xtsm(params) => {
                let area = (
                    self.cell.0 * term.columns() as u32,
                    self.cell.1 * term.screen_lines() as u32,
                );
                let reply = sixel::xtsmgraphics(&params, area, &mut self.registers);
                self.replies.push(reply);
                Work::Done
            }
        }
    }

    /// Stores and places a decoded image.
    pub fn finish<T: EventListener>(&mut self, decoded: Decoded, term: &mut Term<T>) {
        match decoded.0 {
            Loaded::Kitty(cmd, result) => self.kitty_loaded(cmd, result, term),
            Loaded::Sixel(Some(pixels)) => {
                let size = (pixels.width as f32, pixels.height as f32);
                self.place_cell_image(Image::new(pixels), size, Origin::Sixel, Move::Below, term);
            }
            Loaded::Sixel(None) => {}
            Loaded::Iterm(args, Ok(pixels)) => {
                let (cw, ch) = self.cell_f32();
                let screen = (cw * term.columns() as f32, ch * term.screen_lines() as f32);
                let size = args.display_size((pixels.width, pixels.height), (cw, ch), screen);
                let movement = if args.do_not_move_cursor {
                    Move::Stay
                } else {
                    Move::AfterRight
                };
                self.place_cell_image(Image::new(pixels), size, Origin::Iterm, movement, term);
            }
            Loaded::Iterm(_, Err(e)) => tracing::debug!("iTerm2 image: {e}"),
        }
        self.recount();
    }

    fn reply(&mut self, cmd: &kitty::Command, id: u32, msg: &str) {
        if let Some(reply) = kitty::reply(cmd, id, msg) {
            self.replies.push(reply);
        }
    }

    fn kitty<T: EventListener>(&mut self, body: &[u8], term: &mut Term<T>) -> Work {
        let (control, payload) = kitty::split(body);
        let (cmd, parsed) = kitty::parse(control);
        if let Some(mut upload) = self.upload.take() {
            // Later chunks carry only `m` (and `q`); anything else abandons
            // the upload.
            if parsed.is_ok() && cmd.is_continuation() {
                if let Err(e) = upload.payload.push(payload) {
                    self.reply(&upload.cmd, upload.cmd.id, &e);
                    return Work::Done;
                }
                if cmd.more {
                    self.upload = Some(upload);
                    return Work::Done;
                }
                let mut first = upload.cmd;
                if cmd.keys.iter().any(|(k, _)| *k == b'q') {
                    first.quiet = cmd.quiet;
                }
                return self.load(first, upload.payload);
            }
        }
        if let Err(e) = parsed {
            self.reply(&cmd, cmd.id, &e);
            return Work::Done;
        }
        match cmd.action {
            b'd' => self.delete(&cmd, term),
            b'p' => {
                let id = if cmd.id != 0 {
                    Some(cmd.id)
                } else {
                    self.screen(is_alt(term)).newest(cmd.number)
                };
                match id {
                    Some(id) if cmd.id != 0 || cmd.number != 0 => self.put(&cmd, id, term),
                    _ => self.reply(&cmd, 0, "ENOENT:no such image"),
                }
            }
            b'q' if cmd.feature.is_some() => {
                if let Some(reply) = kitty::feature_reply(&cmd) {
                    self.replies.push(reply);
                }
            }
            b't' | b'T' | b'q' => {
                let mut p = kitty::Payload::default();
                if let Err(e) = p.push(payload) {
                    self.reply(&cmd, cmd.id, &e);
                } else if cmd.more {
                    self.upload = Some(Upload { cmd, payload: p });
                } else {
                    return self.load(cmd, p);
                }
            }
            b'f' | b'a' | b'c' => {
                self.reply(&cmd, cmd.id, "ENOTSUP:animation is not supported");
            }
            a => self.reply(
                &cmd,
                cmd.id,
                &format!("EINVAL:unknown action {}", a as char),
            ),
        }
        Work::Done
    }

    fn load(&mut self, cmd: kitty::Command, payload: kitty::Payload) -> Work {
        match payload.finish() {
            Ok(data) => Work::Decode(Decode(Job::Kitty(kitty::Load { cmd, data }))),
            Err(e) => {
                self.reply(&cmd, cmd.id, &e);
                Work::Done
            }
        }
    }

    fn kitty_loaded<T: EventListener>(
        &mut self,
        cmd: kitty::Command,
        result: Result<Pixels, String>,
        term: &mut Term<T>,
    ) {
        let pixels = match result {
            Ok(p) if p.rgba.len() > QUOTA => Err("EFBIG:image larger than the quota".into()),
            other => other,
        };
        let pixels = match pixels {
            Ok(p) => p,
            Err(e) => return self.reply(&cmd, cmd.id, &e),
        };
        // A query only checks the image can be loaded.
        if cmd.action == b'q' {
            return self.reply(&cmd, cmd.id, "OK");
        }
        let alt = is_alt(term);
        let id = if cmd.id != 0 {
            cmd.id
        } else {
            self.new_id(alt)
        };
        let now = self.tick();
        let screen = self.screen_mut(alt);
        // Sending an id again replaces the image and drops its placements.
        screen.remove_image(id);
        screen.images.insert(
            id,
            KittyImage {
                image: Image::new(pixels),
                number: cmd.number,
                named: cmd.id != 0 || cmd.number != 0,
                created: now,
                used: now,
                virtuals: Vec::new(),
            },
        );
        if cmd.action == b'T' {
            self.put(&cmd, id, term);
        } else {
            self.reply(&cmd, id, "OK");
        }
        self.enforce_quota();
    }

    /// An id no image on the screen has, for unnamed images and `I`.
    fn new_id(&mut self, alt: bool) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1 << 31);
            if !self.screen(alt).images.contains_key(&id) {
                return id;
            }
        }
    }

    /// `a=p` (and the display half of `a=T`).
    fn put<T: EventListener>(&mut self, cmd: &kitty::Command, id: u32, term: &mut Term<T>) {
        let alt = is_alt(term);
        let cell = self.cell_f32();
        let now = self.tick();
        let Some(image) = self.screen_mut(alt).images.get_mut(&id) else {
            return self.reply(cmd, id, "ENOENT:no such image");
        };
        image.used = now;
        if cmd.parent != 0 {
            return self.reply(cmd, id, "ENOTSUP:relative placements are not supported");
        }
        let Some(geo) = geometry(cmd, image.image.width, image.image.height, cell) else {
            return self.reply(cmd, id, "EINVAL:the source rectangle is empty");
        };
        let placement = if image.named { cmd.placement } else { 0 };
        if cmd.virtual_placement {
            if placement != 0 {
                image.virtuals.retain(|v| v.placement != placement);
            }
            image.virtuals.push(Virtual {
                placement,
                cols: geo.cols,
                rows: geo.rows,
                dest: geo.fitted,
                src: geo.src,
                z: cmd.z,
            });
            return self.reply(cmd, id, "OK");
        }
        let image = image.image.clone();
        let screen = self.screen_mut(alt);
        let origin = Origin::Kitty {
            image: id,
            placement,
        };
        if placement != 0 {
            screen.placements.retain(|p| p.origin != origin);
        }
        let (row, col) = cursor_cell(term);
        screen.placements.push(Placement {
            image,
            row,
            col,
            cols: geo.cols,
            rows: geo.rows,
            dest: geo.dest,
            src: geo.src,
            z: cmd.z,
            origin,
            stamp: now,
            erased: Vec::new(),
            erased_count: 0,
        });
        if !cmd.keep_cursor {
            move_cursor(term, Move::AfterRight, col, geo.cols, geo.rows);
        }
        self.reply(cmd, id, "OK");
    }

    /// `a=d`.
    fn delete<T>(&mut self, cmd: &kitty::Command, term: &Term<T>) {
        let alt = is_alt(term);
        let pushed = term.grid().lines_pushed();
        let screen_rows = pushed..pushed + term.screen_lines() as u64;
        let (cursor_row, cursor_col) = cursor_cell(term);
        // `x`/`y` are 1-based screen cells.
        let row_of = |y: u32| pushed + u64::from(y.max(1) - 1);
        let col_of = |x: u32| x.max(1) - 1;
        let one = |n: u64| n..n + 1;
        let all_cols = 0..u32::MAX;
        let screen = self.screen_mut(alt);
        let touched = match cmd.delete.to_ascii_lowercase() {
            b'a' => screen.delete(|p| p.intersects(&screen_rows, &all_cols)),
            b'i' | b'n' => {
                let id = if cmd.delete.eq_ignore_ascii_case(&b'i') {
                    Some(cmd.id)
                } else {
                    screen.newest(cmd.number)
                };
                let Some(id) = id.filter(|id| *id != 0) else {
                    return;
                };
                let placement = cmd.placement;
                if let Some(image) = screen.images.get_mut(&id) {
                    image
                        .virtuals
                        .retain(|v| placement != 0 && v.placement != placement);
                }
                let mut touched = screen.delete(|p| {
                    p.origin
                        == Origin::Kitty {
                            image: id,
                            placement,
                        }
                        || (placement == 0 && p.kitty_image() == Some(id))
                });
                touched.push(id);
                touched
            }
            b'c' => {
                screen.delete(|p| p.intersects(&one(cursor_row), &(cursor_col..cursor_col + 1)))
            }
            b'p' => {
                let (row, col) = (row_of(cmd.y), col_of(cmd.x));
                screen.delete(|p| p.intersects(&one(row), &(col..col + 1)))
            }
            b'q' => {
                let (row, col) = (row_of(cmd.y), col_of(cmd.x));
                screen.delete(|p| p.z == cmd.z && p.intersects(&one(row), &(col..col + 1)))
            }
            b'r' => {
                let ids = cmd.x..=cmd.y;
                for (id, image) in screen.images.iter_mut() {
                    if ids.contains(id) {
                        image.virtuals.clear();
                    }
                }
                let mut touched =
                    screen.delete(|p| p.kitty_image().is_some_and(|id| ids.contains(&id)));
                touched.extend(screen.images.keys().filter(|id| ids.contains(id)));
                touched
            }
            b'x' => {
                let col = col_of(cmd.x);
                screen.delete(|p| p.intersects(&(0..u64::MAX), &(col..col + 1)))
            }
            b'y' => screen.delete(|p| p.intersects(&one(row_of(cmd.y)), &all_cols)),
            b'z' => screen.delete(|p| p.z == cmd.z),
            // Animation frames: there are none.
            _ => Vec::new(),
        };
        if cmd.delete.is_ascii_uppercase() {
            screen.free(&touched);
        }
    }

    fn iterm(&mut self, body: &[u8]) -> Work {
        match iterm::parse(body) {
            Some(iterm::Request::File(args, data)) if args.inline => {
                return Work::Decode(Decode(Job::Iterm(args, Ok(data))));
            }
            Some(iterm::Request::Begin(args)) => {
                self.multipart = Some((args, kitty::Payload::default()));
            }
            Some(iterm::Request::Part(data)) => {
                if let Some((_, payload)) = &mut self.multipart
                    && payload.push(&data).is_err()
                {
                    self.multipart = None;
                }
            }
            Some(iterm::Request::End) => {
                if let Some((args, payload)) = self.multipart.take()
                    && args.inline
                    && let Ok(bytes) = payload.finish()
                {
                    return Work::Decode(Decode(Job::Iterm(args, Err(bytes))));
                }
            }
            // Downloads aren't supported.
            _ => {}
        }
        Work::Done
    }

    /// Places a sixel or iTerm2 image of `size` pixels at the cursor.
    fn place_cell_image<T: EventListener>(
        &mut self,
        image: Arc<Image>,
        (w, h): (f32, f32),
        origin: Origin,
        movement: Move,
        term: &mut Term<T>,
    ) {
        if image.bytes() > QUOTA {
            return;
        }
        let (cw, ch) = self.cell_f32();
        let (row, col) = cursor_cell(term);
        let room = (term.columns() as u32).saturating_sub(col).max(1);
        let cols = ((w / cw).ceil() as u32).clamp(1, room);
        let rows = ((h / ch).ceil() as u32).max(1);
        let stamp = self.tick();
        let screen = self.screen_mut(is_alt(term));
        // A new image over an old one replaces it there.
        screen.wipe(&(row..row + u64::from(rows)), &(col..col + cols));
        screen.placements.push(Placement {
            src: Rect {
                x: 0.,
                y: 0.,
                w: image.width as f32,
                h: image.height as f32,
            },
            image,
            row,
            col,
            cols,
            rows,
            dest: Rect {
                x: 0.,
                y: 0.,
                w: w / cw,
                h: h / ch,
            },
            z: 0,
            origin,
            stamp,
            erased: Vec::new(),
            erased_count: 0,
        });
        self.enforce_quota();
        move_cursor(term, movement, col, cols, rows);
    }

    fn used_bytes(&self) -> usize {
        [&self.main, &self.alt]
            .iter()
            .map(|s| {
                let kitty: usize = s.images.values().map(|i| i.image.bytes()).sum();
                let cells: usize = s
                    .placements
                    .iter()
                    .filter(|p| p.is_cell_image())
                    .map(|p| p.image.bytes())
                    .sum();
                kitty + cells
            })
            .sum()
    }

    /// Over the quota, drops kitty images nothing shows, then the oldest
    /// images; never the one just stored or placed.
    fn enforce_quota(&mut self) {
        enum Victim {
            Image(u32),
            Placement(usize),
        }
        let newest = self.clock;
        while self.used_bytes() > QUOTA {
            // Ordered by (shown, last use).
            let mut victim: Option<((bool, u64), bool, Victim)> = None;
            let mut consider = |key: (bool, u64), alt: bool, v: Victim| {
                if key.1 != newest && victim.as_ref().is_none_or(|(k, ..)| key < *k) {
                    victim = Some((key, alt, v));
                }
            };
            for alt in [false, true] {
                let screen = self.screen(alt);
                for (id, image) in &screen.images {
                    let shown = !image.virtuals.is_empty()
                        || screen
                            .placements
                            .iter()
                            .any(|p| p.kitty_image() == Some(*id));
                    consider((shown, image.used), alt, Victim::Image(*id));
                }
                for (i, p) in screen.placements.iter().enumerate() {
                    if p.is_cell_image() {
                        consider((true, p.stamp), alt, Victim::Placement(i));
                    }
                }
            }
            let Some((_, alt, victim)) = victim else {
                break;
            };
            let screen = self.screen_mut(alt);
            match victim {
                Victim::Image(id) => screen.remove_image(id),
                Victim::Placement(i) => {
                    screen.placements.remove(i);
                }
            }
        }
        self.recount();
    }
}

/// Where a kitty placement goes.
struct Geometry {
    src: Rect,
    /// In cells from the placement's top-left, with the `X`/`Y` offsets.
    dest: Rect,
    /// The same without the offsets, for virtual placements.
    fitted: Rect,
    cols: u32,
    rows: u32,
}

/// Lays out a kitty placement of a `width` × `height` image with cells
/// of `cell` pixels.
fn geometry(cmd: &kitty::Command, width: u32, height: u32, cell: (f32, f32)) -> Option<Geometry> {
    let x = cmd.x.min(width);
    let y = cmd.y.min(height);
    let w = if cmd.w == 0 {
        width - x
    } else {
        cmd.w.min(width - x)
    };
    let h = if cmd.h == 0 {
        height - y
    } else {
        cmd.h.min(height - y)
    };
    if w == 0 || h == 0 {
        return None;
    }
    let (cw, ch) = cell;
    let (w, h) = (w as f32, h as f32);
    let off_x = (cmd.cell_x as f32).min(cw - 1.);
    let off_y = (cmd.cell_y as f32).min(ch - 1.);
    // The size drawn, and the box it's centred in (letterboxed when both
    // c and r are given).
    let (dw, dh, cols, rows) = match (cmd.cols, cmd.rows) {
        (0, 0) => (
            w,
            h,
            ((off_x + w) / cw).ceil() as u32,
            ((off_y + h) / ch).ceil() as u32,
        ),
        (c, 0) => {
            let dw = c as f32 * cw;
            let dh = dw * h / w;
            (dw, dh, c, (dh / ch).ceil() as u32)
        }
        (0, r) => {
            let dh = r as f32 * ch;
            let dw = dh * w / h;
            (dw, dh, (dw / cw).ceil() as u32, r)
        }
        (c, r) => {
            let (bw, bh) = (c as f32 * cw, r as f32 * ch);
            let scale = (bw / w).min(bh / h);
            (w * scale, h * scale, c, r)
        }
    };
    let pad_x = ((cols as f32 * cw - dw) / 2.).max(0.);
    let pad_y = ((rows as f32 * ch - dh) / 2.).max(0.);
    let (pad_x, pad_y) = if cmd.cols != 0 && cmd.rows != 0 {
        (pad_x, pad_y)
    } else {
        (0., 0.)
    };
    let fitted = Rect {
        x: pad_x / cw,
        y: pad_y / ch,
        w: dw / cw,
        h: dh / ch,
    };
    Some(Geometry {
        src: Rect {
            x: x as f32,
            y: y as f32,
            w,
            h,
        },
        dest: Rect {
            x: fitted.x + off_x / cw,
            y: fitted.y + off_y / ch,
            ..fitted
        },
        fitted,
        cols: cols.max(1),
        rows: rows.max(1),
    })
}

/// Moves the cursor past an image placed at `col` covering `cols` × `rows`,
/// scrolling like line feeds do.
fn move_cursor<T: EventListener>(
    term: &mut Term<T>,
    movement: Move,
    col: u32,
    cols: u32,
    rows: u32,
) {
    let feeds = match movement {
        Move::Stay => return,
        Move::AfterRight => rows.saturating_sub(1),
        Move::Below => rows,
    };
    for _ in 0..feeds {
        term.linefeed();
    }
    let last = term.columns().saturating_sub(1);
    let cursor = &mut term.grid_mut().cursor;
    let x = match movement {
        Move::Below => col as usize,
        _ => (col + cols) as usize,
    };
    cursor.point.column = Column(x.min(last));
    // Past the edge: the next character wraps, as after text.
    cursor.input_needs_wrap = x > last;
}

#[cfg(test)]
pub(crate) mod tests;
