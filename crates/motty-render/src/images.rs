//! Inline images in the terminal element: what to draw this frame
//! ([`collect`], under the terminal lock) and drawing it ([`paint`]) in
//! three layers around the text, as kitty stacks them by z-index.
//!
//! Each image is uploaded once and kept as an `Arc<RenderImage>` (so its
//! sprite-atlas tile is reused) in a small cache with a memory cap. Images
//! still on screen stay whatever their size; others go oldest first once
//! over the cap, and ones the terminal deleted go after a short grace (a
//! frame may still be using their tile).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use gpui::{App, Bounds, Corners, Pixels, RenderImage, Window, point, px, size};
use motty_term::alacritty_terminal::Term;
use motty_term::alacritty_terminal::grid::Dimensions;
use motty_term::alacritty_terminal::index::{Column, Line};
use motty_term::alacritty_terminal::term::TermMode;
use motty_term::graphics::placeholder::{self, PLACEHOLDER};
use motty_term::graphics::{Graphics, Image, Rect};

/// Decoded images kept for drawing beyond those on screen.
const CACHE_BYTES: usize = 64 << 20;
/// Bigger images are scaled down for the GPU.
const MAX_SIDE: u32 = 4096;
/// How long an image stays after it was last drawn before it may go.
const GRACE: Duration = Duration::from_millis(500);
/// kitty: z-indices below this go under cell backgrounds.
const BELOW_BACKGROUND: i32 = i32::MIN / 2;

/// Where an image is drawn relative to the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Under non-default cell backgrounds.
    BelowBackground,
    /// Over backgrounds, under text.
    BelowText,
    AboveText,
}

impl Layer {
    fn of(z: i32) -> Layer {
        if z < BELOW_BACKGROUND {
            Layer::BelowBackground
        } else if z < 0 {
            Layer::BelowText
        } else {
            Layer::AboveText
        }
    }
}

/// One image (or part of it) to draw, in cells from the top-left of the
/// viewport.
struct Draw {
    image: Arc<Image>,
    /// The part shown.
    clip: Rect,
    /// Where the whole image would be.
    full: Rect,
    layer: Layer,
}

/// What to draw this frame.
#[derive(Default)]
pub struct Images {
    draws: Vec<Draw>,
    /// Viewport cells holding Unicode placeholders, drawn without text.
    pub placeholders: Vec<(usize, usize)>,
    /// Cell (0, 0) and a cell's size, and the text area to clip to.
    origin: gpui::Point<Pixels>,
    cell: (Pixels, Pixels),
    area: Bounds<Pixels>,
}

impl Images {
    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }

    /// Places the frame: cell (0, 0) at `origin`, clipped to `area`.
    pub fn set_geometry(
        &mut self,
        origin: gpui::Point<Pixels>,
        cell: (Pixels, Pixels),
        area: Bounds<Pixels>,
    ) {
        self.origin = origin;
        self.cell = cell;
        self.area = area;
    }
}

/// The whole image's rectangle when `src` of it is shown in `dest`.
fn full_rect(dest: Rect, src: Rect, image: &Image) -> Rect {
    let sx = dest.w / src.w.max(1.);
    let sy = dest.h / src.h.max(1.);
    Rect {
        x: dest.x - src.x * sx,
        y: dest.y - src.y * sy,
        w: image.width as f32 * sx,
        h: image.height as f32 * sy,
    }
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let w = (a.x + a.w).min(b.x + b.w) - x;
    let h = (a.y + a.h).min(b.y + b.h) - y;
    (w > 0. && h > 0.).then_some(Rect { x, y, w, h })
}

/// Collects the images in a viewport of `rows` × `cols` scrolled back by
/// `display_offset`. Rows just outside it are included, for views that
/// draw a partly scrolled-in row.
pub fn collect<T>(
    term: &Term<T>,
    graphics: &Graphics,
    display_offset: usize,
    rows: usize,
    cols: usize,
) -> Images {
    let alt = term.mode().contains(TermMode::ALT_SCREEN);
    let pushed = term.grid().lines_pushed() as i64;
    // Stable row at the top of the viewport.
    let top = pushed - display_offset as i64;
    let mut images = Images::default();
    let mut order = Vec::new();
    for p in graphics.placements(alt) {
        let vr = p.row as i64 - top;
        if vr + i64::from(p.rows) < -1 || vr > rows as i64 {
            continue;
        }
        let (x, y) = (p.col as f32, vr as f32);
        let dest = Rect {
            x: x + p.dest.x,
            y: y + p.dest.y,
            ..p.dest
        };
        let full = full_rect(dest, p.src, &p.image);
        let key = (p.z, p.origin_id(), p.stamp);
        let mut push = |clip: Rect| {
            if let Some(clip) = intersect(clip, dest) {
                order.push((key, images.draws.len()));
                images.draws.push(Draw {
                    image: p.image.clone(),
                    clip,
                    full,
                    layer: Layer::of(p.z),
                });
            }
        };
        if !p.any_erased() {
            push(dest);
            continue;
        }
        // Only the cells text hasn't written over, a run at a time.
        for r in 0..p.rows {
            let mut c = 0;
            while c < p.cols {
                if p.is_erased(r, c) {
                    c += 1;
                    continue;
                }
                let start = c;
                while c < p.cols && !p.is_erased(r, c) {
                    c += 1;
                }
                push(Rect {
                    x: x + start as f32,
                    y: y + r as f32,
                    w: (c - start) as f32,
                    h: 1.,
                });
            }
        }
    }
    if graphics.has_virtual(alt) {
        placeholders(
            term,
            graphics,
            alt,
            display_offset,
            rows,
            cols,
            &mut images,
            &mut order,
        );
    }
    // Lower z first; equal z by image id, then age.
    order.sort_by_key(|(key, _)| *key);
    let mut draws: Vec<Option<Draw>> = images.draws.into_iter().map(Some).collect();
    images.draws = order
        .into_iter()
        .filter_map(|(_, i)| draws[i].take())
        .collect();
    images
}

type Order = Vec<((i32, u32, u64), usize)>;

/// Images shown through Unicode placeholder cells.
#[allow(clippy::too_many_arguments)]
fn placeholders<T>(
    term: &Term<T>,
    graphics: &Graphics,
    alt: bool,
    display_offset: usize,
    rows: usize,
    cols: usize,
    images: &mut Images,
    order: &mut Order,
) {
    let grid = term.grid();
    let cols = cols.min(grid.columns());
    for vr in 0..rows.min(grid.screen_lines()) {
        let row = &grid[Line(vr as i32 - display_offset as i32)];
        let cells: Vec<_> = (0..cols)
            .filter(|&c| row[Column(c)].c == PLACEHOLDER)
            .filter_map(|c| placeholder::Raw::from_cell(&row[Column(c)]).map(|raw| (c, raw)))
            .collect();
        for (col, cell) in placeholder::resolve_row(&cells) {
            images.placeholders.push((vr, col));
            let Some((image, v)) =
                graphics.virtual_placement(alt, cell.image_id, cell.placement_id)
            else {
                continue;
            };
            if cell.row >= v.rows || cell.col >= v.cols {
                continue;
            }
            // The placement's box, with this cell at (cell.col, cell.row).
            let dest = Rect {
                x: col as f32 - cell.col as f32 + v.dest.x,
                y: vr as f32 - cell.row as f32 + v.dest.y,
                ..v.dest
            };
            let full = full_rect(dest, v.src, image);
            let cell_rect = Rect {
                x: col as f32,
                y: vr as f32,
                w: 1.,
                h: 1.,
            };
            let Some(clip) = intersect(cell_rect, dest) else {
                continue;
            };
            // Runs of cells of the same image draw as one.
            if let Some(last) = images.draws.last_mut()
                && Arc::ptr_eq(&last.image, image)
                && last.full == full
                && last.clip.y == clip.y
                && (last.clip.x + last.clip.w - clip.x).abs() < 1e-3
            {
                last.clip.w += clip.w;
                continue;
            }
            order.push(((v.z, cell.image_id, 0), images.draws.len()));
            images.draws.push(Draw {
                image: image.clone(),
                clip,
                full,
                layer: Layer::of(v.z),
            });
        }
    }
}

/// Draws the images of `layer`.
pub fn paint(images: &Images, layer: Layer, window: &mut Window, cx: &mut App) {
    if images.draws.is_empty() {
        return;
    }
    let (cw, lh) = images.cell;
    let to_px = |r: Rect| {
        Bounds::new(
            point(images.origin.x + cw * r.x, images.origin.y + lh * r.y),
            size(cw * r.w, lh * r.h),
        )
    };
    let now = Instant::now();
    CACHE.with_borrow_mut(|cache| {
        for draw in images.draws.iter().filter(|d| d.layer == layer) {
            let clip = to_px(draw.clip).intersect(&images.area);
            if clip.size.width <= px(0.) || clip.size.height <= px(0.) {
                continue;
            }
            let Some(image) = cache.get(&draw.image, now) else {
                continue;
            };
            let _ = window.paint_image(clip, to_px(draw.full), Corners::default(), image, 0, false);
        }
        if layer == Layer::AboveText {
            cache.sweep(now, window, cx);
        }
    });
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

struct Entry {
    image: Arc<RenderImage>,
    /// Gone once the terminal drops the image.
    source: Weak<Image>,
    bytes: usize,
    used: Instant,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<u64, Entry>,
}

impl Cache {
    fn get(&mut self, image: &Arc<Image>, now: Instant) -> Option<Arc<RenderImage>> {
        if let Some(entry) = self.entries.get_mut(&image.serial) {
            entry.used = now;
            return Some(entry.image.clone());
        }
        let render = Arc::new(to_render_image(image)?);
        let bytes = render.as_bytes(0).map_or(0, <[u8]>::len);
        self.entries.insert(
            image.serial,
            Entry {
                image: render.clone(),
                source: Arc::downgrade(image),
                bytes,
                used: now,
            },
        );
        Some(render)
    }

    /// Drops deleted images, then the least recently drawn over the cap.
    fn sweep(&mut self, now: Instant, window: &mut Window, cx: &mut App) {
        let idle = |e: &Entry| now.duration_since(e.used) >= GRACE;
        let mut gone: Vec<u64> = self
            .entries
            .iter()
            .filter(|(_, e)| idle(e) && e.source.strong_count() == 0)
            .map(|(k, _)| *k)
            .collect();
        let mut total: usize = self.entries.values().map(|e| e.bytes).sum();
        total -= gone.iter().map(|k| self.entries[k].bytes).sum::<usize>();
        if total > CACHE_BYTES {
            let mut old: Vec<_> = self
                .entries
                .iter()
                .filter(|(k, e)| idle(e) && !gone.contains(k))
                .map(|(k, e)| (e.used, *k, e.bytes))
                .collect();
            old.sort();
            for (_, k, bytes) in old {
                if total <= CACHE_BYTES {
                    break;
                }
                total -= bytes;
                gone.push(k);
            }
        }
        for k in gone {
            if let Some(entry) = self.entries.remove(&k) {
                cx.drop_image(entry.image, Some(window));
            }
        }
    }
}

/// RGBA to the BGRA gpui draws, scaled down past [`MAX_SIDE`].
fn to_render_image(image: &Image) -> Option<RenderImage> {
    let mut buffer = image::RgbaImage::from_raw(image.width, image.height, image.rgba.clone())?;
    if image.width > MAX_SIDE || image.height > MAX_SIDE {
        let scale = MAX_SIDE as f32 / image.width.max(image.height) as f32;
        let w = ((image.width as f32 * scale) as u32).max(1);
        let h = ((image.height as f32 * scale) as u32).max(1);
        buffer = image::imageops::resize(&buffer, w, h, image::imageops::FilterType::Triangle);
    }
    for px in buffer.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Some(RenderImage::new([image::Frame::new(buffer)]))
}

#[cfg(test)]
mod tests {
    use motty_term::GridSize;
    use motty_term::alacritty_terminal::event::VoidListener;
    use motty_term::alacritty_terminal::term::Config;
    use motty_term::alacritty_terminal::vte::ansi::{Attr, Color, Handler};
    use motty_term::graphics::{Sequence, Work};

    use super::*;

    /// A 20 × 6 terminal with 10 × 20 pixel cells, and its images.
    fn term() -> (Term<VoidListener>, Graphics) {
        let size = GridSize {
            cols: 20,
            rows: 6,
            cell_width: 10.,
            cell_height: 20.,
        };
        let mut graphics = Graphics::default();
        graphics.set_cell_size(10, 20);
        (Term::new(Config::default(), &size, VoidListener), graphics)
    }

    /// Runs a kitty command with `w` × `h` RGBA pixels.
    fn kitty(term: &mut Term<VoidListener>, g: &mut Graphics, control: &str, w: u32, h: u32) {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(vec![9; (w * h * 4) as usize]);
        let body = format!("f=32,s={w},v={h},{control};{data}").into_bytes();
        if let Work::Decode(job) = g.begin(Sequence::Kitty(body), term) {
            g.finish(job.run(), term);
        }
    }

    fn clips(images: &Images) -> Vec<(f32, f32, f32, f32)> {
        images
            .draws
            .iter()
            .map(|d| (d.clip.x, d.clip.y, d.clip.w, d.clip.h))
            .collect()
    }

    #[test]
    fn placements_follow_the_viewport() {
        let (mut term, mut g) = term();
        term.goto(1, 2);
        kitty(&mut term, &mut g, "a=T,C=1", 20, 20);
        let images = collect(&term, &g, 0, 6, 20);
        assert_eq!(clips(&images), vec![(2., 1., 2., 1.)]);
        // Scrolled up: lines pushed into history, the view back on them.
        for _ in 0..10 {
            term.linefeed();
        }
        assert!(collect(&term, &g, 0, 6, 20).is_empty());
        let pushed = term.grid().lines_pushed() as usize;
        let images = collect(&term, &g, pushed, 6, 20);
        assert_eq!(clips(&images), vec![(2., 1., 2., 1.)]);
    }

    #[test]
    fn wiped_cells_leave_runs_and_z_picks_the_layer() {
        let (mut term, mut g) = term();
        // A 3 × 2 cell sixel-like image: via a sixel sequence.
        let band = "#1!30~";
        let data = format!("\"1;1;30;40{band}-{band}-{band}-{band}-{band}-{band}-{band}");
        let seq = Sequence::Sixel {
            params: vec![0, 1],
            data: data.into_bytes(),
        };
        if let Work::Decode(job) = g.begin(seq, &mut term) {
            g.finish(job.run(), &mut term);
        }
        g.wipe(&term, 0..1, 1..2);
        let images = collect(&term, &g, 0, 6, 20);
        assert_eq!(
            clips(&images)[..3],
            [(0., 0., 1., 1.), (2., 0., 1., 1.), (0., 1., 3., 1.)]
        );
        assert!(images.draws.iter().all(|d| d.layer == Layer::AboveText));
        kitty(&mut term, &mut g, "a=T,z=-1,C=1", 10, 20);
        let images = collect(&term, &g, 0, 6, 20);
        // Lower z sorts first.
        assert_eq!(images.draws[0].layer, Layer::BelowText);
    }

    #[test]
    fn placeholders_draw_their_part_of_the_image() {
        let (mut term, mut g) = term();
        kitty(&mut term, &mut g, "a=T,U=1,i=42,c=2,r=1,q=2", 20, 20);
        term.goto(3, 5);
        term.terminal_attribute(Attr::Foreground(Color::Indexed(42)));
        term.input(PLACEHOLDER);
        term.input('\u{305}');
        term.input(PLACEHOLDER);
        let images = collect(&term, &g, 0, 6, 20);
        assert_eq!(images.placeholders, vec![(3, 5), (3, 6)]);
        // The 20 × 20 image fills the 2 × 1 box (20 × 20 pixels); the two
        // cells draw as one.
        assert_eq!(clips(&images), vec![(5., 3., 2., 1.)]);
    }

    #[test]
    fn layers_follow_kitty_z() {
        assert_eq!(Layer::of(0), Layer::AboveText);
        assert_eq!(Layer::of(-1), Layer::BelowText);
        assert_eq!(Layer::of(-1_073_741_824), Layer::BelowText);
        assert_eq!(Layer::of(-1_073_741_825), Layer::BelowBackground);
    }

    #[test]
    fn source_rectangles_map_to_the_whole_image() {
        let image = Image {
            serial: 0,
            width: 40,
            height: 20,
            rgba: Vec::new(),
        };
        // The right half shown in 2 × 1 cells at (3, 4): the whole image
        // spans 4 cells starting one image-half to the left.
        let dest = Rect {
            x: 3.,
            y: 4.,
            w: 2.,
            h: 1.,
        };
        let src = Rect {
            x: 20.,
            y: 0.,
            w: 20.,
            h: 20.,
        };
        assert_eq!(
            full_rect(dest, src, &image),
            Rect {
                x: 1.,
                y: 4.,
                w: 4.,
                h: 1.
            }
        );
        assert_eq!(
            intersect(
                dest,
                Rect {
                    x: 4.,
                    y: 0.,
                    w: 9.,
                    h: 9.
                }
            ),
            Some(Rect {
                x: 4.,
                y: 4.,
                w: 1.,
                h: 1.
            })
        );
        assert_eq!(
            intersect(
                dest,
                Rect {
                    x: 9.,
                    y: 0.,
                    w: 1.,
                    h: 1.
                }
            ),
            None
        );
    }
}
