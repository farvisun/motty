//! The terminal grid as a GPUI element.
//!
//! Each frame takes a short-lived snapshot of the visible grid, then builds:
//! merged background rectangles, one shaped line per contiguous text segment
//! (glyphs snapped to the cell grid with `force_width`), quads for box-drawing
//! characters, and the cursor.

use std::cell::Cell as StdCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Bounds, ElementId, FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, PaintQuad, Pixels, ShapedLine, SharedString, Size, StrikethroughStyle,
    Style, TextAlign, TextRun, UnderlineStyle, Window, fill, outline, point, px, relative, size,
};
use motty_term::alacritty_terminal::grid::Dimensions;
use motty_term::alacritty_terminal::index::{Column, Line, Point as GridPoint};
use motty_term::alacritty_terminal::sync::FairMutex;
use motty_term::alacritty_terminal::term::TermMode;
use motty_term::alacritty_terminal::term::cell::Flags;
use motty_term::alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use motty_term::{GridSize, Terminal};
use motty_theme::Rgba;

use crate::box_drawing;
use crate::style::{TerminalStyle, hsla};

/// Geometry of the last painted frame, shared with the view for hit-testing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalLayout {
    /// Top-left of cell (0, 0) in window coordinates.
    pub origin: gpui::Point<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub cols: usize,
    pub rows: usize,
    /// Cursor cell (row, col) in the viewport, when visible.
    pub cursor: Option<(usize, usize)>,
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
    let line_height = px((f32::from(natural) * style.line_height_scale * scale).ceil() / scale);
    let centered_baseline = (line_height - natural) / 2. + ascent;
    let baseline = px((f32::from(line_height - descent) * scale).ceil() / scale);
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
        }
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
}

pub struct Prepaint {
    backgrounds: Vec<PaintQuad>,
    segments: Vec<TextSegment>,
    glyph_quads: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    cursor_text: Option<TextSegment>,
    preedit: Option<(PaintQuad, PaintQuad, TextSegment)>,
    ghost: Option<TextSegment>,
    line_height: Pixels,
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
    rows: Vec<Vec<SnapCell>>,
    cursor: Option<(usize, usize, CursorShape, bool)>,
    /// Cursor cell even when the cursor itself is hidden (for IME placement).
    cursor_cell: Option<(usize, usize)>,
    /// Hint labels on screen: row, column, width in cells.
    labels: Vec<(usize, usize, usize)>,
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
        let snapshot = self.snapshot(&style);
        let layout = TerminalLayout {
            origin,
            cell_width: metrics.cell_width,
            line_height: metrics.line_height,
            cols,
            rows,
            cursor: snapshot.cursor_cell,
        };
        self.layout_sink.set(Some(layout));
        let mut prepaint = build(&snapshot, &style, metrics, origin, window);
        for &(row, col, width) in &snapshot.labels {
            prepaint.glyph_quads.push(outline(
                Bounds::new(
                    point(
                        origin.x + metrics.cell_width * col as f32,
                        origin.y + metrics.line_height * row as f32,
                    ),
                    size(metrics.cell_width * width as f32, metrics.line_height),
                ),
                hsla(Rgba::rgb(LABEL_INK)),
                gpui::BorderStyle::Solid,
            ));
        }
        if let (Some(text), Some((row, col)), None) =
            (self.ghost.clone(), snapshot.cursor_cell, &self.preedit)
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
                prepaint.ghost = Some(TextSegment { origin, line });
            }
        }
        if let (Some(text), Some((row, col))) = (self.preedit.clone(), snapshot.cursor_cell) {
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
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for quad in prepaint.backgrounds.drain(..) {
                window.paint_quad(quad);
            }
            for seg in &prepaint.segments {
                let _ = seg.line.paint(
                    seg.origin,
                    prepaint.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            for quad in prepaint.glyph_quads.drain(..) {
                window.paint_quad(quad);
            }
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
                let _ = seg.line.paint(
                    seg.origin,
                    prepaint.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
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
        let mut rows: Vec<Vec<SnapCell>> = Vec::with_capacity(rows_n);
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
                let row = p.line.0 + display_offset;
                (0..rows_n as i32).contains(&row).then_some((
                    row as usize,
                    p.column.0,
                    text.chars().count(),
                ))
            })
            .collect();
        let dark = motty_theme::theme::luminance(opaque_bg) < 0.5;

        for indexed in content.display_iter {
            let row = (indexed.point.line.0 + display_offset) as usize;
            if row >= rows_n {
                break;
            }
            while rows.len() <= row {
                rows.push(Vec::with_capacity(cols_n));
            }
            let cell = indexed.cell;
            let mut flags = cell.flags;
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
            rows[row].push(SnapCell {
                c: cell.c,
                zerowidth: cell.zerowidth().map(<[char]>::to_vec),
                fg,
                bg,
                flags,
            });
        }

        let cursor_point = content.cursor.point;
        let cursor_row = cursor_point.line.0 + display_offset;
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
            && (0..rows_n as i32).contains(&cursor_row)
            && (vi || content.mode.contains(TermMode::SHOW_CURSOR)))
        .then(|| {
            let wide = term.grid()
                [GridPoint::new(Line(cursor_point.line.0), Column(cursor_point.column.0))]
            .flags
            .contains(Flags::WIDE_CHAR);
            (cursor_row as usize, cursor_point.column.0, shape, wide)
        });
        let cursor_cell = (0..rows_n as i32)
            .contains(&cursor_row)
            .then_some((cursor_row as usize, cursor_point.column.0));
        Snapshot {
            rows,
            cursor,
            cursor_cell,
            labels,
        }
    }
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

#[derive(Clone, PartialEq)]
struct RunStyle {
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: Option<(bool, f32)>,
    strike: bool,
}

fn run_style(cell: &SnapCell) -> RunStyle {
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
        bold: f.contains(Flags::BOLD),
        italic: f.contains(Flags::ITALIC),
        underline,
        strike: f.contains(Flags::STRIKEOUT),
    }
}

fn text_run(style: &TerminalStyle, rs: &RunStyle, len: usize) -> TextRun {
    let mut font = style.font.clone();
    if rs.bold {
        font.weight = FontWeight::BOLD;
    }
    if rs.italic {
        font.style = FontStyle::Italic;
    }
    TextRun {
        len,
        font,
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
}

impl SegmentBuilder {
    fn new(start_col: usize) -> Self {
        Self {
            start_col,
            text: String::new(),
            runs: Vec::new(),
            current: None,
            visible_len: 0,
        }
    }

    fn push(&mut self, style: &TerminalStyle, cell: &SnapCell, rs: RunStyle) {
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
        Some(TextSegment { origin, line })
    }
}

fn build(
    snapshot: &Snapshot,
    style: &TerminalStyle,
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

    for (row, cells) in snapshot.rows.iter().enumerate() {
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
        for (col, cell) in cells.iter().enumerate() {
            let flags = cell.flags;
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            if flags.contains(Flags::HIDDEN) || (cell.c == ' ' && seg.is_none()) {
                if flags.contains(Flags::HIDDEN) {
                    flush(&mut seg, &mut segments, window);
                }
                continue;
            }
            if let Some(quads) =
                box_drawing::quads(cell.c, cell_bounds(row, col, 1), style.font_size, scale)
            {
                flush(&mut seg, &mut segments, window);
                let color = hsla(cell.fg);
                glyph_quads.extend(quads.into_iter().map(|q| {
                    fill(
                        q.bounds,
                        Hsla {
                            a: color.a * q.alpha,
                            ..color
                        },
                    )
                }));
                continue;
            }
            if flags.contains(Flags::WIDE_CHAR) {
                flush(&mut seg, &mut segments, window);
                let mut wide = SegmentBuilder::new(col);
                wide.push(style, cell, run_style(cell));
                if let Some(t) = wide.finish(style, metrics, origin, row, false, window) {
                    segments.push(t);
                }
                continue;
            }
            seg.get_or_insert_with(|| SegmentBuilder::new(col))
                .push(style, cell, run_style(cell));
        }
        flush(&mut seg, &mut segments, window);
    }

    let (cursor, cursor_text) = match snapshot.cursor {
        Some((row, col, shape, wide)) => build_cursor(
            snapshot, style, metrics, origin, row, col, shape, wide, window,
        ),
        None => (None, None),
    };

    Prepaint {
        backgrounds,
        segments,
        glyph_quads,
        cursor,
        cursor_text,
        preedit: None,
        ghost: None,
        line_height: lh,
    }
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
    (bg_quad, underline, TextSegment { origin, line })
}

#[allow(clippy::too_many_arguments)]
fn build_cursor(
    snapshot: &Snapshot,
    style: &TerminalStyle,
    metrics: CellMetrics,
    origin: gpui::Point<Pixels>,
    row: usize,
    col: usize,
    shape: CursorShape,
    wide: bool,
    window: &mut Window,
) -> (Option<PaintQuad>, Option<TextSegment>) {
    let width = if wide { 2. } else { 1. };
    let x = origin.x + metrics.cell_width * col as f32;
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
                let mut seg = SegmentBuilder::new(col);
                seg.push(style, &c, run_style(&c));
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
