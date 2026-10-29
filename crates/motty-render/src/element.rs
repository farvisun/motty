//! The terminal grid as a GPUI element.
//!
//! Each frame takes a short-lived snapshot of the visible grid, then builds:
//! merged background rectangles, one shaped line per contiguous text segment
//! (glyphs snapped to the cell grid with `force_width`), quads for box-drawing
//! characters, and the cursor.

use std::borrow::Cow;
use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Bounds, ElementId, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId,
    LineLayout, PaintQuad, Path, Pixels, ShapedLine, SharedString, Size, StrikethroughStyle, Style,
    TextAlign, TextRun, UnderlineStyle, Window, fill, outline, point, px, relative, size,
};
use motty_term::alacritty_terminal::grid::Dimensions;
use motty_term::alacritty_terminal::index::{Column, Line, Point as GridPoint};
use motty_term::alacritty_terminal::sync::FairMutex;
use motty_term::alacritty_terminal::term::TermMode;
use motty_term::alacritty_terminal::term::cell::Flags;
use motty_term::alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use motty_term::graphics::Graphics;
use motty_term::{GridSize, Terminal};
use motty_theme::{FaceMode, FontBlending, Rgba};

use crate::bidi::{self, BidiCell, LineOrder, VisualMap};
use crate::box_drawing;
use crate::contrast::Contrast;
use crate::images::{self, Images, Layer};
use crate::motion::FrameMotion;
use crate::style::{TerminalStyle, hsla};

/// Geometry of the last painted frame, shared with the view for hit-testing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalLayout {
    /// Top-left of cell (0, 0) in window coordinates, where it's drawn this
    /// frame (scrolling motion moves it).
    pub origin: gpui::Point<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub cols: usize,
    pub rows: usize,
    /// Cursor cell (row, col) in the viewport, when visible; the column
    /// is where it is drawn (rows with right-to-left text are reordered).
    pub cursor: Option<(usize, usize)>,
    /// Cells set to blink (SGR 5) are on screen and `text-blink` is on.
    pub blinking: bool,
}

impl TerminalLayout {
    pub fn cell_bounds(&self, row: usize, col: usize) -> Bounds<Pixels> {
        Bounds::new(
            point(
                self.origin.x + self.cell_width * col as f32,
                self.origin.y + self.line_height * row as f32,
            ),
            size(self.cell_width, self.line_height),
        )
    }

    /// Viewport cell under `pos`, clamped to the grid, plus whether the
    /// pointer is in the right half of the cell (for selection sides).
    pub fn cell_at(&self, pos: gpui::Point<Pixels>) -> (usize, usize, bool) {
        let x = f32::from(pos.x - self.origin.x) / f32::from(self.cell_width);
        let y = f32::from(pos.y - self.origin.y) / f32::from(self.line_height);
        let col = (x.max(0.) as usize).min(self.cols.saturating_sub(1));
        let row = (y.max(0.) as usize).min(self.rows.saturating_sub(1));
        (row, col, x.fract() >= 0.5)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CellMetrics {
    pub cell_width: Pixels,
    pub line_height: Pixels,
    /// Shift from GPUI's centered baseline to ours: the baseline sits one
    /// descent above the cell bottom, rounded down the screen to a device pixel.
    pub text_dy: Pixels,
}

/// The grid that fits a pane of `size` (padding included).
pub fn grid_for(style: &TerminalStyle, window: &mut Window, size: Size<Pixels>) -> GridSize {
    let metrics = measure(style, window);
    let pad = style.padding;
    let inner_w = f32::from(size.width) - pad.left - pad.right;
    let inner_h = f32::from(size.height) - pad.top - pad.bottom;
    GridSize {
        cols: ((inner_w / f32::from(metrics.cell_width)).floor() as u16).max(2),
        rows: ((inner_h / f32::from(metrics.line_height)).floor() as u16).max(1),
        cell_width: f32::from(metrics.cell_width),
        cell_height: f32::from(metrics.line_height),
    }
}

pub fn measure(style: &TerminalStyle, window: &mut Window) -> CellMetrics {
    let scale = window.scale_factor();
    let ts = window.text_system();
    let font_id = ts.resolve_font(&style.font);
    let size = style.font_size;
    let advance = ts
        .advance(font_id, size, 'm')
        .map(|s| s.width)
        .unwrap_or(size * 0.6);
    let ascent = ts.ascent(font_id, size);
    let descent = ts.descent(font_id, size).abs();
    let natural = ascent + descent;
    // The advance stays fractional (JetBrains Mono at 13pt is 7.8pt); glyphs
    // are positioned at col * width so rounding would drift across a line.
    let cell_width = advance;
    let device = |v: f32| (v * scale).ceil() / scale;
    let natural_height = device(f32::from(natural));
    let line_height = px(style
        .cell_height
        .map_or(natural_height, |h| device(h.apply(f32::from(natural)))));
    let centered_baseline = (line_height - natural) / 2. + ascent;
    // At the natural height the baseline sits one descent above the cell
    // bottom; added or removed height is split above and below.
    let extra = f32::from(line_height) - natural_height;
    let baseline =
        px(device(natural_height - f32::from(descent)) + (extra / 2. * scale).floor() / scale);
    CellMetrics {
        cell_width,
        line_height,
        text_dy: baseline - centered_baseline,
    }
}

/// A search match to highlight, in grid coordinates (inclusive).
#[derive(Clone, Copy, Debug)]
pub struct Highlight {
    pub start: GridPoint,
    pub end: GridPoint,
    /// The selected match, drawn in the foreground color.
    pub current: bool,
}

/// Other matches: a marker yellow.
const MATCH_BACKGROUND: u32 = 0xfee382;
/// Text on matches over a dark theme.
const MATCH_DARK_TEXT: u32 = 0x1b1b1b;
/// Hint labels: dark letters on yellow in a dark outline (Otty's).
const LABEL_BACKGROUND: u32 = 0xffd926;
const LABEL_INK: u32 = 0x1a1a1a;

/// How strongly ghost text shows against the foreground.
const GHOST_ALPHA: f32 = 0.4;

type ResizeFn = Box<dyn FnOnce(GridSize, &mut Window, &mut App)>;
type PaintHook = Box<dyn FnOnce(Bounds<Pixels>, &mut Window, &mut App)>;

pub struct TerminalElement {
    term: Arc<FairMutex<Terminal>>,
    style: Rc<TerminalStyle>,
    focused: bool,
    cursor_visible: bool,
    layout_sink: Rc<StdCell<Option<TerminalLayout>>>,
    on_resize: Option<ResizeFn>,
    on_paint: Option<PaintHook>,
    preedit: Option<SharedString>,
    /// An inline suggestion, drawn dim from the cursor on.
    ghost: Option<SharedString>,
    /// Sorted, non-overlapping.
    highlights: Vec<Highlight>,
    /// Links under the pointer or hint targets, underlined (OSC 8 links
    /// always are).
    underlines: Vec<(GridPoint, GridPoint)>,
    /// Hint labels: text drawn over the cells from the point rightwards.
    labels: Vec<(GridPoint, SharedString)>,
    /// Smooth scrolling and the cursor's glide (see [`crate::motion`]).
    motion: FrameMotion,
    /// The off half of the `text-blink` cycle: blinking cells are hidden.
    blink_off: bool,
    /// Where rows were reordered for right-to-left text, for hit-testing.
    visual_map: Option<Rc<RefCell<VisualMap>>>,
    /// Inline images (sixel, iTerm2, kitty), drawn by [`images`].
    graphics: Option<Arc<std::sync::Mutex<Graphics>>>,
}

impl TerminalElement {
    pub fn new(
        term: Arc<FairMutex<Terminal>>,
        style: Rc<TerminalStyle>,
        focused: bool,
        cursor_visible: bool,
        layout_sink: Rc<StdCell<Option<TerminalLayout>>>,
        on_resize: impl FnOnce(GridSize, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            term,
            style,
            focused,
            cursor_visible,
            layout_sink,
            on_resize: Some(Box::new(on_resize)),
            on_paint: None,
            preedit: None,
            ghost: None,
            highlights: Vec::new(),
            underlines: Vec::new(),
            labels: Vec::new(),
            motion: FrameMotion::default(),
            blink_off: false,
            visual_map: None,
            graphics: None,
        }
    }

    /// Where scrolling and the cursor are this frame, between rows and cells.
    pub fn motion(mut self, motion: FrameMotion) -> Self {
        self.motion = motion;
        self
    }

    /// Hides blinking cells (the off half of the blink cycle).
    pub fn blink_off(mut self, off: bool) -> Self {
        self.blink_off = off;
        self
    }

    /// Receives each frame's right-to-left reordering, so a pointer
    /// position can be turned back into a grid column.
    pub fn visual_map(mut self, map: Rc<RefCell<VisualMap>>) -> Self {
        self.visual_map = Some(map);
        self
    }

    /// The session's inline images, to draw with the text.
    pub fn graphics(mut self, graphics: Arc<std::sync::Mutex<Graphics>>) -> Self {
        self.graphics = Some(graphics);
        self
    }

    /// Runs during paint with the element's bounds (e.g. to register an input handler).
    pub fn on_paint(
        mut self,
        f: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_paint = Some(Box::new(f));
        self
    }

    /// Search matches, sorted and non-overlapping.
    pub fn highlights(mut self, highlights: Vec<Highlight>) -> Self {
        self.highlights = highlights;
        self
    }

    /// Underlines the cells of each range (inclusive).
    pub fn underlines(mut self, ranges: Vec<(GridPoint, GridPoint)>) -> Self {
        self.underlines = ranges;
        self
    }

    /// Hint labels, each drawn over the cells from its point rightwards.
    pub fn labels(mut self, labels: Vec<(GridPoint, SharedString)>) -> Self {
        self.labels = labels;
        self
    }

    /// Suggested text, drawn dim from the cursor to the end of its row.
    pub fn ghost(mut self, text: Option<SharedString>) -> Self {
        self.ghost = text.filter(|t| !t.is_empty());
        self
    }

    /// IME composition text, drawn over the cursor.
    pub fn preedit(mut self, text: Option<SharedString>) -> Self {
        self.preedit = text.filter(|t| !t.is_empty());
        self
    }
}

struct TextSegment {
    origin: gpui::Point<Pixels>,
    line: ShapedLine,
    /// Drawn again this far to the right: faux bold, or thickened stems.
    overstrike: Option<Pixels>,
}

impl TextSegment {
    fn paint(&self, line_height: Pixels, window: &mut Window, cx: &mut App) {
        let _ = self
            .line
            .paint(self.origin, line_height, TextAlign::Left, None, window, cx);
        if let Some(dx) = self.overstrike {
            let origin = point(self.origin.x + dx, self.origin.y);
            let _ = self
                .line
                .paint(origin, line_height, TextAlign::Left, None, window, cx);
        }
    }
}

pub struct Prepaint {
    backgrounds: Vec<PaintQuad>,
    segments: Vec<TextSegment>,
    glyph_quads: Vec<PaintQuad>,
    /// Joined arrows' heads and triangles.
    glyph_paths: Vec<(Path<Pixels>, Hsla)>,
    cursor: Option<PaintQuad>,
    cursor_text: Option<TextSegment>,
    preedit: Option<(PaintQuad, PaintQuad, TextSegment)>,
    ghost: Option<TextSegment>,
    line_height: Pixels,
    /// While scrolling between rows, the rows' area, so a row half out of
    /// it doesn't draw over the padding.
    clip: Option<Bounds<Pixels>>,
    images: Images,
}

#[derive(Clone)]
struct SnapCell {
    c: char,
    zerowidth: Option<Vec<char>>,
    fg: Rgba,
    bg: Rgba,
    flags: Flags,
}

struct Snapshot {
    /// Rows from the top one drawn, which may be cut by the top edge.
    rows: Vec<Vec<SnapCell>>,
    /// How much of the first row is above the top edge, in rows.
    shift: f32,
    /// Drawing order of rows holding right-to-left text.
    orders: Vec<Option<LineOrder>>,
    /// Blinking cells are on screen.
    blinking: bool,
    /// The grid's first row is this row of the snapshot (one more
    /// while a smooth scroll shows part of the row above).
    grid_row0: usize,
    cursor: Option<(usize, usize, CursorShape, bool)>,
    /// Cursor cell even when the cursor itself is hidden (for IME placement).
    cursor_cell: Option<(usize, usize)>,
    /// Hint labels on screen: row, column, width in cells.
    labels: Vec<(usize, usize, usize)>,
    images: Images,
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl gpui::Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let style = self.style.clone();
        let metrics = measure(&style, window);
        let pad = style.padding;
        let inner_w = f32::from(bounds.size.width) - pad.left - pad.right;
        let inner_h = f32::from(bounds.size.height) - pad.top - pad.bottom;
        let cols = ((inner_w / f32::from(metrics.cell_width)).floor() as usize).max(2);
        let rows = ((inner_h / f32::from(metrics.line_height)).floor() as usize).max(1);
        // Rows are centered vertically in the leftover space (snapped down to a
        // device pixel); columns stay left-aligned.
        let scale = window.scale_factor();
        let leftover = inner_h - rows as f32 * f32::from(metrics.line_height);
        let top_slack = ((leftover / 2.).max(0.) * scale).floor() / scale;
        let origin = point(
            bounds.origin.x + px(pad.left),
            bounds.origin.y + px(pad.top + top_slack),
        );

        let grid = GridSize {
            cols: cols as u16,
            rows: rows as u16,
            cell_width: f32::from(metrics.cell_width),
            cell_height: f32::from(metrics.line_height),
        };
        let current = {
            let term = self.term.lock();
            (term.columns(), term.screen_lines())
        };
        if current != (cols, rows)
            && let Some(on_resize) = self.on_resize.take()
        {
            on_resize(grid, window, cx);
        }
        let mut snapshot = self.snapshot(&style);
        let lh = metrics.line_height;
        // The snapshot's rows start `shift` of a row above the top edge;
        // the grid's rows (for hit-testing) sit `scroll` rows further down.
        let rows_origin = point(origin.x, origin.y - lh * snapshot.shift);
        let visual = |row: usize, col: usize| match snapshot.orders.get(row + snapshot.grid_row0) {
            Some(Some(order)) => order.to_visual(col),
            _ => col,
        };
        let cursor_cell = snapshot
            .cursor_cell
            .map(|(row, col)| (row, visual(row, col)));
        let layout = TerminalLayout {
            origin: point(origin.x, origin.y + lh * self.motion.scroll),
            cell_width: metrics.cell_width,
            line_height: metrics.line_height,
            cols,
            rows,
            cursor: cursor_cell,
            blinking: snapshot.blinking,
        };
        self.layout_sink.set(Some(layout));
        if let Some(map) = &self.visual_map {
            map.borrow_mut().rows =
                snapshot.orders[snapshot.grid_row0.min(snapshot.orders.len())..].to_vec();
        }
        let faces = Faces::new(&style, window);
        let mut prepaint = build(&snapshot, &style, &faces, metrics, rows_origin, window);
        if self.motion.scroll != 0. {
            prepaint.clip = Some(Bounds::new(
                point(bounds.origin.x, origin.y),
                size(bounds.size.width, lh * rows as f32),
            ));
        }
        if let Some(quad) = prepaint.cursor.as_mut() {
            quad.bounds = moved_cursor(quad.bounds, self.motion, metrics.cell_width);
            // The glyph under a gliding block catches up when it lands.
            if self.motion.cursor_dx != 0. {
                prepaint.cursor_text = None;
            }
        }
        // Images go where the grid's cells do, clipped to the text area.
        prepaint.images = std::mem::take(&mut snapshot.images);
        prepaint.images.set_geometry(
            point(origin.x, origin.y + lh * self.motion.scroll),
            (metrics.cell_width, metrics.line_height),
            Bounds::new(
                origin,
                size(
                    metrics.cell_width * cols as f32,
                    metrics.line_height * rows as f32,
                ),
            ),
        );
        for &(row, col, width) in &snapshot.labels {
            let col = visual(row, col);
            prepaint.glyph_quads.push(outline(
                Bounds::new(
                    point(
                        rows_origin.x + metrics.cell_width * col as f32,
                        rows_origin.y + metrics.line_height * row as f32,
                    ),
                    size(metrics.cell_width * width as f32, metrics.line_height),
                ),
                hsla(Rgba::rgb(LABEL_INK)),
                gpui::BorderStyle::Solid,
            ));
        }
        if let (Some(text), Some((row, col)), None) =
            (self.ghost.clone(), cursor_cell, &self.preedit)
        {
            let room = layout.cols.saturating_sub(col);
            let text: String = text.chars().take(room).collect();
            if !text.is_empty() {
                let fg = style.palette[NamedColor::Foreground as usize];
                let run = TextRun {
                    len: text.len(),
                    font: style.font.clone(),
                    color: hsla(fg.with_alpha(fg.a * GHOST_ALPHA)),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line =
                    window
                        .text_system()
                        .shape_line(text.into(), style.font_size, &[run], None);
                let origin = layout.cell_bounds(row, col).origin;
                prepaint.ghost = Some(TextSegment {
                    origin,
                    line,
                    overstrike: None,
                });
            }
        }
        if let (Some(text), Some((row, col))) = (self.preedit.clone(), cursor_cell) {
            prepaint.preedit = Some(build_preedit(text, &style, &layout, row, col, window));
            prepaint.cursor = None;
            prepaint.cursor_text = None;
        }
        prepaint
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mask = prepaint.clip.map_or(bounds, |clip| clip.intersect(&bounds));
        window.with_content_mask(Some(gpui::ContentMask { bounds: mask }), |window| {
            images::paint(&prepaint.images, Layer::BelowBackground, window, cx);
            for quad in prepaint.backgrounds.drain(..) {
                window.paint_quad(quad);
            }
            images::paint(&prepaint.images, Layer::BelowText, window, cx);
            for seg in &prepaint.segments {
                seg.paint(prepaint.line_height, window, cx);
            }
            for quad in prepaint.glyph_quads.drain(..) {
                window.paint_quad(quad);
            }
            for (path, color) in prepaint.glyph_paths.drain(..) {
                window.paint_path(path, color);
            }
            images::paint(&prepaint.images, Layer::AboveText, window, cx);
            if let Some(seg) = &prepaint.ghost {
                let _ = seg.line.paint(
                    seg.origin,
                    prepaint.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
            if let Some(seg) = &prepaint.cursor_text {
                seg.paint(prepaint.line_height, window, cx);
            }
            if let Some((bg, underline, seg)) = prepaint.preedit.take() {
                window.paint_quad(bg);
                let _ = seg.line.paint(
                    seg.origin,
                    prepaint.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
                window.paint_quad(underline);
            }
        });
        if let Some(hook) = self.on_paint.take() {
            hook(bounds, window, cx);
        }
    }
}

impl TerminalElement {
    fn snapshot(&self, style: &TerminalStyle) -> Snapshot {
        let term = self.term.lock();
        let content = term.renderable_content();
        let colors = content.colors;
        let display_offset = content.display_offset as i32;
        let selection = content.selection;
        let rows_n = term.screen_lines();
        let cols_n = term.columns();
        // The grid line at the top edge, between rows while scrolling; one
        // more row is drawn when the top one is cut.
        let top = -(display_offset as f32 + self.motion.scroll);
        let first = top.floor();
        let shift = top - first;
        let first = first as i32;
        let count = rows_n + usize::from(shift > 0.);
        let mut rows: Vec<Vec<SnapCell>> = Vec::with_capacity(count);
        let opaque_bg = if style.background.a > 0.5 {
            style.background
        } else {
            style.palette[NamedColor::Background as usize].with_alpha(1.)
        };
        let default_fg = style.palette[NamedColor::Foreground as usize];
        let label_cells: std::collections::BTreeMap<GridPoint, char> = self
            .labels
            .iter()
            .flat_map(|(p, text)| {
                text.chars()
                    .enumerate()
                    .map(move |(i, c)| (GridPoint::new(p.line, p.column + i), c))
            })
            .collect();
        let labels = self
            .labels
            .iter()
            .filter_map(|(p, text)| {
                let row = p.line.0 - first;
                (0..count as i32).contains(&row).then_some((
                    row as usize,
                    p.column.0,
                    text.chars().count(),
                ))
            })
            .collect();
        let dark = motty_theme::theme::luminance(opaque_bg) < 0.5;
        let text = &style.text;
        let mut contrast = Contrast::new(text.minimum_contrast);
        let mut blinking = false;

        let grid = term.grid();
        let start = first.max(grid.topmost_line().0);
        let end = (first + count as i32 - 1).min(grid.bottommost_line().0);
        let cells = (start <= end)
            .then(|| {
                grid.iter_from(GridPoint::new(Line(start - 1), grid.last_column()))
                    .take_while(move |c| c.point.line.0 <= end)
            })
            .into_iter()
            .flatten();
        for indexed in cells {
            let row = (indexed.point.line.0 - first) as usize;
            while rows.len() <= row {
                rows.push(Vec::with_capacity(cols_n));
            }
            let cell = indexed.cell;
            let mut flags = cell.flags;
            if !text.underline {
                flags.remove(Flags::ALL_UNDERLINES);
            }
            if text.blink && flags.contains(Flags::BLINK) {
                blinking = true;
                if self.blink_off {
                    flags.insert(Flags::HIDDEN);
                }
            }
            if text.strip_cjk_wrap_pad
                && indexed.point.column.0 + 1 == cols_n
                && crate::wrap_pad::is_wrap_pad(term.grid(), indexed.point)
            {
                flags.remove(Flags::INVERSE);
            }
            if cell.hyperlink().is_some()
                || self
                    .underlines
                    .iter()
                    .any(|(a, b)| (*a..=*b).contains(&indexed.point))
            {
                flags |= Flags::UNDERLINE;
            }
            if let Some(&c) = label_cells.get(&indexed.point) {
                rows[row].push(SnapCell {
                    c,
                    zerowidth: None,
                    fg: Rgba::rgb(LABEL_INK),
                    bg: Rgba::rgb(LABEL_BACKGROUND),
                    flags: Flags::BOLD,
                });
                continue;
            }
            let mut fg = style.resolve(dim_color(cell.fg, flags), colors);
            if flags.contains(Flags::DIM) && !matches!(cell.fg, Color::Named(n) if (n as usize) < 8)
            {
                fg = motty_theme::theme::mix(fg, opaque_bg, 0.34);
            }
            let mut bg = style.resolve(cell.bg, colors);
            if flags.contains(Flags::INVERSE) {
                let bg_opaque = if bg.a < 0.5 { opaque_bg } else { bg };
                (fg, bg) = (bg_opaque, fg);
            }
            let ix = self.highlights.partition_point(|h| h.end < indexed.point);
            if let Some(h) = self.highlights.get(ix)
                && h.start <= indexed.point
            {
                if h.current {
                    (fg, bg) = (opaque_bg, default_fg);
                } else {
                    bg = Rgba::rgb(MATCH_BACKGROUND);
                    if dark {
                        fg = Rgba::rgb(MATCH_DARK_TEXT);
                    }
                }
            }
            if selection.is_some_and(|s| {
                s.contains_cell(&indexed, content.cursor.point, content.cursor.shape)
            }) {
                bg = style.selection_background;
                if let Some(sfg) = style.selection_foreground {
                    fg = sfg;
                }
            }
            if let Some(contrast) = contrast.as_mut() {
                fg = contrast.apply(fg, if bg.a < 0.5 { opaque_bg } else { bg });
            }
            rows[row].push(SnapCell {
                c: cell.c,
                zerowidth: cell.zerowidth().map(<[char]>::to_vec),
                fg,
                bg,
                flags,
            });
        }

        // Images, read with the terminal still locked (terminal, then
        // graphics: the PTY thread's order). Placeholder cells show the
        // image, not their character.
        let images = self.graphics.as_ref().map_or_else(Images::default, |g| {
            let graphics = g.lock().unwrap_or_else(|e| e.into_inner());
            images::collect(&term, &graphics, display_offset as usize, rows_n, cols_n)
        });
        let grid_row0 = (-(display_offset + first)).max(0) as usize;
        for &(row, col) in &images.placeholders {
            if let Some(cell) = rows.get_mut(row + grid_row0).and_then(|r| r.get_mut(col)) {
                cell.c = ' ';
                cell.zerowidth = None;
            }
        }

        let cursor_point = content.cursor.point;
        let cursor_row = cursor_point.line.0 - first;
        let vi = content.mode.contains(TermMode::VI);
        let shape = match content.cursor.shape {
            _ if !self.cursor_visible => CursorShape::Hidden,
            // The vi cursor is a block whatever the program asked for.
            _ if vi => CursorShape::Block,
            CursorShape::Hidden => CursorShape::Hidden,
            _ if !self.focused => CursorShape::HollowBlock,
            shape => shape,
        };
        let cursor = (shape != CursorShape::Hidden
            && (0..count as i32).contains(&cursor_row)
            && (vi || content.mode.contains(TermMode::SHOW_CURSOR)))
        .then(|| {
            let wide = term.grid()
                [GridPoint::new(Line(cursor_point.line.0), Column(cursor_point.column.0))]
            .flags
            .contains(Flags::WIDE_CHAR);
            (cursor_row as usize, cursor_point.column.0, shape, wide)
        });
        let cursor_cell = (0..rows_n as i32)
            .contains(&(cursor_point.line.0 + display_offset))
            .then_some((
                (cursor_point.line.0 + display_offset) as usize,
                cursor_point.column.0,
            ));
        // A program in explicit mode (BDSM) lays out bidi text itself.
        let reorder = text.bidi && !content.mode.contains(TermMode::BIDI_EXPLICIT);
        let orders = rows
            .iter()
            .map(|cells| reorder.then(|| row_order(cells)).flatten())
            .collect();
        Snapshot {
            rows,
            shift,
            grid_row0,
            orders,
            blinking,
            cursor,
            cursor_cell,
            labels,
            images,
        }
    }
}

/// The drawing order of a row with right-to-left text in it.
fn row_order(cells: &[SnapCell]) -> Option<LineOrder> {
    // Nothing below U+0590 reads right to left: most rows stop here.
    if !cells
        .iter()
        .any(|c| c.c >= '\u{0590}' || c.zerowidth.is_some())
    {
        return None;
    }
    let cells: Vec<BidiCell> = cells
        .iter()
        .map(|c| BidiCell {
            c: c.c,
            extra: c.zerowidth.as_deref().unwrap_or_default(),
            spacer: c.flags.contains(Flags::WIDE_CHAR_SPACER),
        })
        .collect();
    bidi::reorder(&cells)
}

/// Maps the 8 normal named colors to their dim variants when DIM is set.
fn dim_color(color: Color, flags: Flags) -> Color {
    const DIMS: [NamedColor; 8] = [
        NamedColor::DimBlack,
        NamedColor::DimRed,
        NamedColor::DimGreen,
        NamedColor::DimYellow,
        NamedColor::DimBlue,
        NamedColor::DimMagenta,
        NamedColor::DimCyan,
        NamedColor::DimWhite,
    ];
    match color {
        Color::Named(n) if flags.contains(Flags::DIM) && (n as usize) < 8 => {
            Color::Named(DIMS[n as usize])
        }
        other => other,
    }
}

/// Per-frame choices of face and overstrike (`text-bold`, `text-italic`,
/// `font-thicken`, `font-blending`).
struct Faces {
    /// Bold cells use the bold face rather than the regular one.
    bold: bool,
    italic: bool,
    /// Bold cells are overstruck this far: synthetic bold, as the family
    /// has no bold face.
    faux_bold: Option<Pixels>,
    /// All text is overstruck this far, darkening stems.
    thicken: Option<Pixels>,
    /// Only text darker than its background is (`perceptual` blending).
    thicken_dark: Option<Pixels>,
    /// What default-background cells sit on.
    backdrop: Rgba,
}

impl Faces {
    fn new(style: &TerminalStyle, window: &mut Window) -> Self {
        let text = &style.text;
        let scale = window.scale_factor();
        // GPUI picks the closest face in the family, so a family without a
        // bold face resolves bold to the regular one.
        let real_bold = text.family_bold.is_some() || {
            let ts = window.text_system();
            ts.resolve_font(&style.faces[1]) != ts.resolve_font(&style.faces[0])
        };
        let synthetic = text.bold == FaceMode::Synthetic && !real_bold;
        // Half a device pixel: GPUI positions glyphs at subpixel offsets,
        // so the overstrike widens stems without doubling them.
        let thin = px(0.5 / scale);
        Self {
            bold: text.bold != FaceMode::Off && !synthetic,
            // GPUI can't slant glyphs, so synthetic italic is the closest
            // face, as with auto.
            italic: text.italic != FaceMode::Off,
            faux_bold: synthetic.then(|| px((f32::from(style.font_size) / 16.).max(1. / scale))),
            thicken: (text.thicken || style.background.a < 1.).then_some(thin),
            thicken_dark: (text.blending == FontBlending::Perceptual).then_some(thin),
            backdrop: if style.background.a > 0.5 {
                style.background
            } else {
                style.palette[NamedColor::Background as usize].with_alpha(1.)
            },
        }
    }

    fn overstrike(&self, cell: &SnapCell) -> Option<Pixels> {
        let luma = |c: Rgba| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
        let mut dx = self.thicken;
        if let Some(thin) = self.thicken_dark {
            let bg = if cell.bg.a < 0.5 {
                self.backdrop
            } else {
                cell.bg
            };
            if luma(cell.fg) < luma(bg) {
                dx = Some(thin);
            }
        }
        match self.faux_bold {
            Some(bold) if cell.flags.contains(Flags::BOLD) => {
                Some(dx.map_or(bold, |d| d.max(bold)))
            }
            _ => dx,
        }
    }
}

#[derive(Clone, PartialEq)]
struct RunStyle {
    fg: Hsla,
    bold: bool,
    italic: bool,
    /// Ligatures off: letters and digits when they only form in symbols.
    plain: bool,
    underline: Option<(bool, f32)>,
    strike: bool,
}

fn run_style(cell: &SnapCell, faces: &Faces, style: &TerminalStyle) -> RunStyle {
    let f = cell.flags;
    let underline = if f.contains(Flags::UNDERCURL) {
        Some((true, 1.))
    } else if f.contains(Flags::DOUBLE_UNDERLINE) {
        Some((false, 2.))
    } else if f.intersects(Flags::UNDERLINE | Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE) {
        Some((false, 1.))
    } else {
        None
    };
    RunStyle {
        fg: hsla(cell.fg),
        bold: faces.bold && f.contains(Flags::BOLD),
        italic: faces.italic && f.contains(Flags::ITALIC),
        plain: style.plain_faces.is_some() && cell.c.is_alphanumeric(),
        underline,
        strike: f.contains(Flags::STRIKEOUT),
    }
}

fn text_run(style: &TerminalStyle, rs: &RunStyle, len: usize) -> TextRun {
    TextRun {
        len,
        font: style.face(rs.bold, rs.italic, rs.plain).clone(),
        color: rs.fg,
        background_color: None,
        underline: rs.underline.map(|(wavy, t)| UnderlineStyle {
            color: Some(rs.fg),
            thickness: px(t),
            wavy,
        }),
        strikethrough: rs.strike.then(|| StrikethroughStyle {
            color: Some(rs.fg),
            thickness: px(1.),
        }),
    }
}

/// Accumulates one shaped line: contiguous narrow cells starting at `start_col`.
struct SegmentBuilder {
    start_col: usize,
    text: String,
    runs: Vec<TextRun>,
    current: Option<(RunStyle, usize)>,
    /// Byte length up to the last cell that has visible content.
    visible_len: usize,
    overstrike: Option<Pixels>,
}

impl SegmentBuilder {
    fn new(start_col: usize, overstrike: Option<Pixels>) -> Self {
        Self {
            start_col,
            text: String::new(),
            runs: Vec::new(),
            current: None,
            visible_len: 0,
            overstrike,
        }
    }

    fn push(&mut self, style: &TerminalStyle, cell: &SnapCell, mut rs: RunStyle) {
        // Spaces never ligate: keep the run going rather than split it.
        if cell.c == ' '
            && let Some((cur, _)) = &self.current
        {
            rs.plain = cur.plain;
        }
        let before = self.text.len();
        self.text.push(cell.c);
        if let Some(zw) = &cell.zerowidth {
            self.text.extend(zw.iter());
        }
        let len = self.text.len() - before;
        match &mut self.current {
            Some((cur, n)) if *cur == rs => *n += len,
            _ => {
                if let Some((cur, n)) = self.current.take() {
                    self.runs.push(text_run(style, &cur, n));
                }
                self.current = Some((rs, len));
            }
        }
        if cell.c != ' '
            || cell
                .flags
                .intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
        {
            self.visible_len = self.text.len();
        }
    }

    /// Shapes the segment, trimming trailing blank cells.
    fn finish(
        mut self,
        style: &TerminalStyle,
        metrics: CellMetrics,
        origin: gpui::Point<Pixels>,
        row: usize,
        force: bool,
        window: &mut Window,
    ) -> Option<TextSegment> {
        if let Some((cur, n)) = self.current.take() {
            self.runs.push(text_run(style, &cur, n));
        }
        if self.visible_len == 0 {
            return None;
        }
        let mut remaining = self.visible_len;
        let mut runs = Vec::with_capacity(self.runs.len());
        for mut run in self.runs {
            if remaining == 0 {
                break;
            }
            run.len = run.len.min(remaining);
            remaining -= run.len;
            runs.push(run);
        }
        self.text.truncate(self.visible_len);
        let line = window.text_system().shape_line(
            SharedString::from(self.text),
            style.font_size,
            &runs,
            force.then_some(metrics.cell_width),
        );
        let origin = point(
            origin.x + metrics.cell_width * self.start_col as f32,
            origin.y + metrics.line_height * row as f32 + metrics.text_dy,
        );
        Some(TextSegment {
            origin,
            line,
            overstrike: self.overstrike,
        })
    }
}

/// A right-to-left run, `cells` in drawing order from visual column
/// `start_col`: shaped in logical order inside RLO…PDF, then its glyphs
/// placed on the run's cells (see [`bidi::place_rtl`]).
#[allow(clippy::too_many_arguments)]
fn rtl_segment(
    style: &TerminalStyle,
    rs: &RunStyle,
    overstrike: Option<Pixels>,
    cells: &[SnapCell],
    start_col: usize,
    metrics: CellMetrics,
    origin: gpui::Point<Pixels>,
    row: usize,
    window: &mut Window,
) -> Option<TextSegment> {
    let decorated = |c: &SnapCell| c.flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT);
    if cells.iter().all(|c| c.c == ' ' && !decorated(c)) {
        return None;
    }
    let mut text = String::with_capacity(cells.len() * 4 + 6);
    text.push(bidi::RLO);
    let mut starts = Vec::with_capacity(cells.len());
    let mut joining = false;
    for cell in cells.iter().rev() {
        starts.push(text.len());
        text.push(cell.c);
        if let Some(zw) = &cell.zerowidth {
            text.extend(zw.iter());
        }
        joining |= bidi::joins(cell.c);
    }
    let text_end = text.len();
    text.push(bidi::PDF);
    let run = text_run(style, rs, text.len());
    let mut line =
        window
            .text_system()
            .shape_line(SharedString::from(text), style.font_size, &[run], None);
    let shaped: Vec<(usize, f32)> = line
        .runs
        .iter()
        .flat_map(|r| r.glyphs.iter())
        .map(|g| (g.index, f32::from(g.position.x)))
        .collect();
    let mut xs = bidi::place_rtl(
        &shaped,
        &starts,
        text_end,
        f32::from(metrics.cell_width),
        f32::from(line.width),
        joining,
    )
    .into_iter();
    // Rebuilt rather than edited: the shaped layout is shared with GPUI's
    // cache. Glyphs go left to right so decorations start at the left.
    let mut runs = Vec::with_capacity(line.runs.len());
    for run in &line.runs {
        let mut glyphs = Vec::with_capacity(run.glyphs.len());
        for glyph in &run.glyphs {
            if let Some(Some(x)) = xs.next() {
                let mut glyph = glyph.clone();
                glyph.position.x = px(x);
                glyphs.push(glyph);
            }
        }
        glyphs.sort_by(|a, b| f32::from(a.position.x).total_cmp(&f32::from(b.position.x)));
        if !glyphs.is_empty() {
            runs.push(gpui::ShapedRun {
                font_id: run.font_id,
                glyphs,
            });
        }
    }
    runs.sort_by(|a, b| {
        f32::from(a.glyphs[0].position.x).total_cmp(&f32::from(b.glyphs[0].position.x))
    });
    let layout = LineLayout {
        font_size: line.font_size,
        width: metrics.cell_width * cells.len() as f32,
        ascent: line.ascent,
        descent: line.descent,
        runs,
        len: line.len,
    };
    *line = Arc::new(layout);
    let origin = point(
        origin.x + metrics.cell_width * start_col as f32,
        origin.y + metrics.line_height * row as f32 + metrics.text_dy,
    );
    Some(TextSegment {
        origin,
        line,
        overstrike,
    })
}

/// Box-drawing and block elements, drawn as quads.
fn is_box(c: char) -> bool {
    ('\u{2500}'..='\u{259F}').contains(&c)
}

fn build(
    snapshot: &Snapshot,
    style: &TerminalStyle,
    faces: &Faces,
    metrics: CellMetrics,
    origin: gpui::Point<Pixels>,
    window: &mut Window,
) -> Prepaint {
    let scale = window.scale_factor();
    let cw = metrics.cell_width;
    let lh = metrics.line_height;
    let default_bg = style.default_background();
    let cell_bounds = |row: usize, col: usize, width: usize| {
        Bounds::new(
            point(origin.x + cw * col as f32, origin.y + lh * row as f32),
            size(cw * width as f32, lh),
        )
    };

    let mut backgrounds = Vec::new();
    let mut segments = Vec::new();
    let mut glyph_quads = Vec::new();
    let mut glyph_paths = Vec::new();
    let tint = |color: Hsla, alpha: f32| Hsla {
        a: color.a * alpha,
        ..color
    };

    for (row, logical_cells) in snapshot.rows.iter().enumerate() {
        // Rows with right-to-left text are drawn in visual order.
        let order = snapshot.orders.get(row).and_then(Option::as_ref);
        let cells: Cow<[SnapCell]> = match order {
            Some(order) => order
                .visual
                .iter()
                .map(|&l| logical_cells[l as usize].clone())
                .collect(),
            None => Cow::Borrowed(logical_cells),
        };
        // Backgrounds: merge horizontal runs of the same non-default color.
        let mut col = 0;
        while col < cells.len() {
            let bg = cells[col].bg;
            if bg == default_bg || bg.a == 0. {
                col += 1;
                continue;
            }
            let start = col;
            while col < cells.len() && cells[col].bg == bg {
                col += 1;
            }
            backgrounds.push(fill(cell_bounds(row, start, col - start), hsla(bg)));
        }

        // Text segments.
        let mut seg: Option<SegmentBuilder> = None;
        let flush = |seg: &mut Option<SegmentBuilder>,
                     segments: &mut Vec<TextSegment>,
                     window: &mut Window| {
            if let Some(s) = seg.take()
                && let Some(t) = s.finish(style, metrics, origin, row, true, window)
            {
                segments.push(t);
            }
        };
        let mut col = 0;
        while col < cells.len() {
            let cell = &cells[col];
            let flags = cell.flags;
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                col += 1;
                continue;
            }
            if flags.contains(Flags::HIDDEN) || (cell.c == ' ' && seg.is_none()) {
                if flags.contains(Flags::HIDDEN) {
                    flush(&mut seg, &mut segments, window);
                }
                col += 1;
                continue;
            }
            let wide = flags.contains(Flags::WIDE_CHAR);
            if style.text.arrow_join && !wide && box_drawing::is_arrow(cell.c) {
                let at = |row: Option<usize>| {
                    row.and_then(|r| snapshot.rows.get(r))
                        .and_then(|r| r.get(col))
                        .map_or(' ', |c| c.c)
                };
                let around = [
                    at(row.checked_sub(1)),
                    cells.get(col + 1).map_or(' ', |c| c.c),
                    at(Some(row + 1)),
                    col.checked_sub(1).map_or(' ', |c| cells[c].c),
                ];
                if let Some((quads, polygons)) = box_drawing::arrow(
                    cell.c,
                    cell_bounds(row, col, 1),
                    style.font_size,
                    scale,
                    around,
                ) {
                    flush(&mut seg, &mut segments, window);
                    let color = hsla(cell.fg);
                    glyph_quads.extend(
                        quads
                            .into_iter()
                            .map(|q| fill(q.bounds, tint(color, q.alpha))),
                    );
                    for polygon in polygons {
                        let mut points = polygon.points.into_iter();
                        if let Some(first) = points.next() {
                            let mut path = Path::new(first);
                            for p in points {
                                path.line_to(p);
                            }
                            glyph_paths.push((path, color));
                        }
                    }
                    col += 1;
                    continue;
                }
            }
            if let Some(quads) =
                box_drawing::quads(cell.c, cell_bounds(row, col, 1), style.font_size, scale)
            {
                flush(&mut seg, &mut segments, window);
                let color = hsla(cell.fg);
                glyph_quads.extend(
                    quads
                        .into_iter()
                        .map(|q| fill(q.bounds, tint(color, q.alpha))),
                );
                col += 1;
                continue;
            }
            if wide {
                flush(&mut seg, &mut segments, window);
                let mut wide = SegmentBuilder::new(col, faces.overstrike(cell));
                wide.push(style, cell, run_style(cell, faces, style));
                if let Some(t) = wide.finish(style, metrics, origin, row, false, window) {
                    segments.push(t);
                }
                col += 1;
                continue;
            }
            if let Some(order) = order
                && order.is_rtl(order.visual[col] as usize)
            {
                // The longest run drawn right to left in one style.
                flush(&mut seg, &mut segments, window);
                let rtl_style = |c: &SnapCell| RunStyle {
                    plain: false,
                    ..run_style(c, faces, style)
                };
                let rs = rtl_style(cell);
                let overstrike = faces.overstrike(cell);
                let level = order.levels[order.visual[col] as usize];
                let mut end = col + 1;
                while let Some(next) = cells.get(end) {
                    let l = order.visual[end] as usize;
                    let joins = order.levels[l] == level
                        && l + 1 == order.visual[end - 1] as usize
                        && !next.flags.intersects(
                            Flags::WIDE_CHAR
                                | Flags::WIDE_CHAR_SPACER
                                | Flags::LEADING_WIDE_CHAR_SPACER
                                | Flags::HIDDEN,
                        )
                        && !is_box(next.c)
                        && rtl_style(next) == rs
                        && faces.overstrike(next) == overstrike;
                    if !joins {
                        break;
                    }
                    end += 1;
                }
                if let Some(t) = rtl_segment(
                    style,
                    &rs,
                    overstrike,
                    &cells[col..end],
                    col,
                    metrics,
                    origin,
                    row,
                    window,
                ) {
                    segments.push(t);
                }
                col = end;
                continue;
            }
            let overstrike = faces.overstrike(cell);
            if seg.as_ref().is_some_and(|s| s.overstrike != overstrike) {
                flush(&mut seg, &mut segments, window);
            }
            seg.get_or_insert_with(|| SegmentBuilder::new(col, overstrike))
                .push(style, cell, run_style(cell, faces, style));
            col += 1;
        }
        flush(&mut seg, &mut segments, window);
    }

    let (cursor, cursor_text) = match snapshot.cursor {
        Some((row, col, shape, wide)) => build_cursor(
            snapshot, style, faces, metrics, origin, row, col, shape, wide, window,
        ),
        None => (None, None),
    };

    Prepaint {
        backgrounds,
        segments,
        glyph_quads,
        glyph_paths,
        cursor,
        cursor_text,
        preedit: None,
        ghost: None,
        line_height: lh,
        clip: None,
        images: Images::default(),
    }
}

/// The cursor's quad, offset along its row and grown about its center.
fn moved_cursor(bounds: Bounds<Pixels>, motion: FrameMotion, cell_width: Pixels) -> Bounds<Pixels> {
    let mut b = bounds;
    b.origin.x += cell_width * motion.cursor_dx;
    if motion.cursor_scale != 1. {
        let center = b.center();
        b.size = size(
            b.size.width * motion.cursor_scale,
            b.size.height * motion.cursor_scale,
        );
        b.origin = point(center.x - b.size.width / 2., center.y - b.size.height / 2.);
    }
    b
}

fn build_preedit(
    text: SharedString,
    style: &TerminalStyle,
    layout: &TerminalLayout,
    row: usize,
    col: usize,
    window: &mut Window,
) -> (PaintQuad, PaintQuad, TextSegment) {
    let fg = style.palette[NamedColor::Foreground as usize];
    let bg = if style.background.a > 0.5 {
        style.background
    } else {
        style.palette[NamedColor::Background as usize].with_alpha(1.)
    };
    let run = TextRun {
        len: text.len(),
        font: style.font.clone(),
        color: hsla(fg),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text, style.font_size, &[run], None);
    let origin = layout.cell_bounds(row, col).origin;
    let width = line.width.max(layout.cell_width);
    let bg_quad = fill(
        Bounds::new(origin, size(width, layout.line_height)),
        hsla(bg),
    );
    let underline = fill(
        Bounds::new(
            point(origin.x, origin.y + layout.line_height - px(1.)),
            size(width, px(1.)),
        ),
        hsla(fg),
    );
    (
        bg_quad,
        underline,
        TextSegment {
            origin,
            line,
            overstrike: None,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn build_cursor(
    snapshot: &Snapshot,
    style: &TerminalStyle,
    faces: &Faces,
    metrics: CellMetrics,
    origin: gpui::Point<Pixels>,
    row: usize,
    col: usize,
    shape: CursorShape,
    wide: bool,
    window: &mut Window,
) -> (Option<PaintQuad>, Option<TextSegment>) {
    let width = if wide { 2. } else { 1. };
    // Drawn where its cell is drawn; `col` stays the logical one.
    let visual = match snapshot.orders.get(row) {
        Some(Some(order)) => order.to_visual(col),
        _ => col,
    };
    let x = origin.x + metrics.cell_width * visual as f32;
    let y = origin.y + metrics.line_height * row as f32;
    let full = Bounds::new(
        point(x, y),
        size(metrics.cell_width * width, metrics.line_height),
    );
    let thickness = px(if window.scale_factor() >= 2. { 2. } else { 1. });
    match shape {
        CursorShape::Block => {
            let quad = fill(full, hsla(style.cursor));
            let cell = snapshot.rows.get(row).and_then(|r| r.get(col));
            let text = cell.filter(|c| c.c != ' ').and_then(|cell| {
                let mut c = cell.clone();
                c.fg = style.cursor_text.unwrap_or(if cell.bg.a > 0. {
                    cell.bg
                } else {
                    style.palette[NamedColor::Background as usize].with_alpha(1.)
                });
                let mut seg = SegmentBuilder::new(visual, faces.overstrike(&c));
                seg.push(style, &c, run_style(&c, faces, style));
                seg.finish(style, metrics, origin, row, !wide, window)
            });
            (Some(quad), text)
        }
        CursorShape::Beam => (
            Some(fill(
                Bounds::new(full.origin, size(thickness, full.size.height)),
                hsla(style.cursor_bar),
            )),
            None,
        ),
        // Otty's underline is twice the bar's thickness; its hollow block
        // strokes at the bar's.
        CursorShape::Underline => (
            Some(fill(
                Bounds::new(
                    point(x, y + metrics.line_height - thickness * 2.),
                    size(full.size.width, thickness * 2.),
                ),
                hsla(style.cursor_underline),
            )),
            None,
        ),
        CursorShape::HollowBlock => (
            Some(
                outline(full, hsla(style.cursor), gpui::BorderStyle::Solid)
                    .border_widths(thickness),
            ),
            None,
        ),
        CursorShape::Hidden => (None, None),
    }
}
