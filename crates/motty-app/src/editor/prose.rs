//! The editor as a text field: no file, no gutter, lines wrapped to the
//! width, a placeholder while empty, and as tall as its text up to a
//! limit. The Composer, the Prompt Queue and Settings' name fields use it.

use std::rc::Rc;

use gpui::{
    AnyElement, Bounds, Context, ElementInputHandler, Hsla, MouseButton, Pixels, Point,
    ScrollEvent, SharedString, TextAlign, Window, WrappedLine, canvas, div, fill, point,
    prelude::*, px, size,
};
use motty_render::{TerminalStyle, hsla};

use super::{FileEditor, Layout, Pos, runs, shape};

pub(super) struct Prose {
    placeholder: SharedString,
    min_lines: usize,
    max_height: Pixels,
    font_size: Option<Pixels>,
}

impl FileEditor {
    /// An empty text field showing `placeholder`.
    pub fn prose(
        placeholder: impl Into<SharedString>,
        style: TerminalStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut editor = Self::new(None, style, cx);
        editor.prose = Some(Prose {
            placeholder: placeholder.into(),
            min_lines: 1,
            max_height: px(240.),
            font_size: None,
        });
        editor
    }

    /// At least `min_lines` tall, scrolling past `max_height`.
    pub fn set_height(&mut self, min_lines: usize, max_height: Pixels) {
        if let Some(p) = &mut self.prose {
            p.min_lines = min_lines.max(1);
            p.max_height = max_height;
        }
    }

    /// Text at this size rather than the terminal's.
    pub fn set_font_size(&mut self, size: Pixels) {
        if let Some(p) = &mut self.prose {
            p.font_size = Some(size);
        }
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = &mut self.prose {
            p.placeholder = placeholder.into();
            cx.notify();
        }
    }

    pub fn text(&self) -> String {
        self.buffer.text()
    }

    /// Selects everything, so typing replaces it.
    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.anchor = Some(super::Pos::default());
        self.cursor = self.buffer.end();
        cx.notify();
    }

    /// Replaces the whole text (an undo step), the cursor at its end.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.push_undo();
        self.buffer = super::Buffer::from_text(text);
        self.cursor = self.buffer.end();
        self.anchor = None;
        self.typing_at = None;
        self.changed(cx);
    }

    fn font_size(&self) -> Pixels {
        self.prose
            .as_ref()
            .and_then(|p| p.font_size)
            .unwrap_or(self.style.font_size)
    }

    fn prose_line_height(&self) -> Pixels {
        (self.font_size() * 1.45).round()
    }

    /// Where `pos` is, from the top of the text.
    fn prose_point(&self, layout: &Layout, pos: Pos) -> Point<Pixels> {
        let lh = layout.line_height;
        match (layout.wrapped.get(pos.line), layout.tops.get(pos.line)) {
            (Some(line), Some(&top)) => {
                let p = line
                    .position_for_index(pos.col.min(line.len()), lh)
                    .unwrap_or_default();
                point(p.x, top + p.y)
            }
            _ => point(px(0.), lh * pos.line as f32),
        }
    }

    /// The position at `y` below the top of the text, nearest `x`.
    fn prose_pos(&self, layout: &Layout, x: Pixels, y: Pixels) -> Pos {
        if y < px(0.) {
            return Pos::default();
        }
        let lh = layout.line_height.max(px(1.));
        for (ix, line) in layout.wrapped.iter().enumerate() {
            let top = layout.tops[ix];
            let height = line.size(lh).height.max(lh);
            if y < top + height {
                let col = line
                    .closest_index_for_position(point(x, y - top), lh)
                    .unwrap_or_else(|i| i);
                return self.buffer.clamp(Pos::new(ix, col));
            }
        }
        self.buffer.end()
    }

    pub(super) fn prose_pos_at(&self, at: Point<Pixels>) -> Pos {
        let layout = self.layout.borrow();
        let x = at.x - layout.bounds.left();
        let y = at.y - layout.bounds.top() + self.scroll.y;
        self.prose_pos(&layout, x, y)
    }

    /// The row `delta` rows away, at the remembered x.
    pub(super) fn prose_vertical(&mut self, delta: isize, _: &mut Window) -> Pos {
        let layout = self.layout.borrow();
        let lh = layout.line_height.max(px(1.));
        let at = self.prose_point(&layout, self.cursor);
        let goal = *self.goal_x.get_or_insert(at.x);
        let y = at.y + lh * delta as f32 + lh / 2.;
        if y >= layout.content_height {
            return self.buffer.end();
        }
        let pos = self.prose_pos(&layout, goal, y);
        drop(layout);
        self.goal_x = Some(goal);
        pos
    }

    pub(super) fn prose_scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let layout = self.layout.borrow();
        let lh = layout.line_height.max(px(1.));
        let max_y = (layout.content_height - layout.bounds.size.height).max(px(0.));
        let dy = event.delta.pixel_delta(lh).y;
        drop(layout);
        let y = (self.scroll.y - dy).clamp(px(0.), max_y);
        if y != self.scroll.y {
            self.scroll.y = y;
            cx.notify();
            cx.stop_propagation();
        }
    }

    /// The caret, for the input method's candidate window.
    pub(super) fn prose_caret(&self) -> Bounds<Pixels> {
        let layout = self.layout.borrow();
        let at = self.prose_point(&layout, self.cursor);
        Bounds::new(
            point(
                layout.bounds.left() + at.x,
                layout.bounds.top() + at.y - self.scroll.y,
            ),
            size(px(2.), layout.line_height),
        )
    }

    pub(super) fn render_prose(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(prose) = &self.prose else {
            return div().into_any_element();
        };
        let lh = self.prose_line_height();
        let content = self.layout.borrow().content_height.max(lh);
        let height = content
            .min(prose.max_height)
            .max(lh * prose.min_lines as f32);
        let style = self.style.clone();
        let fg = hsla(self.foreground());
        let state = ProsePaint {
            lines: self.buffer.clone(),
            cursor: self.cursor,
            selection: self.selection(),
            marked: self.marked.clone(),
            scroll: self.scroll.y,
            reveal: std::mem::take(&mut self.reveal),
            focused: self.focus.is_focused(window),
            style: style.clone(),
            font_size: self.font_size(),
            line_height: lh,
            fg,
            placeholder: prose.placeholder.clone(),
            placeholder_color: fg.opacity(0.4),
            selection_bg: hsla(style.selection_background),
            caret: hsla(motty_theme::Rgba::rgb(0x047afe)),
            layout: self.layout.clone(),
        };
        let entity = cx.entity();
        let focus = self.focus.clone();
        let text = canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let (scroll, resized) = paint_prose(state, bounds, window, cx);
                if scroll.is_some() || resized {
                    entity.update(cx, |this, cx| {
                        if let Some(y) = scroll {
                            this.scroll.y = y;
                        }
                        cx.notify();
                    });
                }
                window.handle_input(&focus, ElementInputHandler::new(bounds, entity.clone()), cx);
            },
        )
        .size_full();

        div()
            .id("prose")
            .key_context("Editor")
            .track_focus(&self.focus)
            .w_full()
            .h(height)
            .overflow_hidden()
            .cursor_text()
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_scroll(cx.listener(Self::on_scroll))
            .child(text)
            .into_any_element()
    }
}

struct ProsePaint {
    lines: super::Buffer,
    cursor: Pos,
    selection: Option<(Pos, Pos)>,
    marked: Option<String>,
    scroll: Pixels,
    reveal: bool,
    focused: bool,
    style: Rc<TerminalStyle>,
    font_size: Pixels,
    line_height: Pixels,
    fg: Hsla,
    placeholder: SharedString,
    placeholder_color: Hsla,
    selection_bg: Hsla,
    caret: Hsla,
    layout: Rc<std::cell::RefCell<Layout>>,
}

/// Paints the wrapped lines; returns a new scroll offset when the cursor
/// had to be brought into view, and whether the text's height changed.
fn paint_prose(
    s: ProsePaint,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut gpui::App,
) -> (Option<Pixels>, bool) {
    let lh = s.line_height;
    let font = &s.style.font;
    // Room for the caret after the last character.
    let width = (bounds.size.width - px(3.)).max(px(10.));
    let mut wrapped: Vec<WrappedLine> = Vec::with_capacity(s.lines.line_count());
    let mut tops = Vec::with_capacity(s.lines.line_count());
    let mut marked_at = None;
    let mut y = px(0.);
    for ix in 0..s.lines.line_count() {
        let mut text = s.lines.line(ix).to_owned();
        // The input method's composition shows at the cursor.
        let marked = (ix == s.cursor.line)
            .then_some(s.marked.as_ref())
            .flatten()
            .map(|m| {
                let col = s.cursor.col.min(text.len());
                text.insert_str(col, m);
                col..col + m.len()
            });
        if marked.is_some() {
            marked_at = marked.clone();
        }
        let runs = runs(&text, font, s.fg, marked);
        let line = window
            .text_system()
            .shape_text(text.into(), s.font_size, &runs, Some(width), None)
            .ok()
            .and_then(|mut l| (!l.is_empty()).then(|| l.remove(0)))
            .unwrap_or_default();
        tops.push(y);
        y += line.size(lh).height.max(lh);
        wrapped.push(line);
    }
    let content_height = y;

    let at = |pos: Pos, wrapped: &[WrappedLine]| {
        let p = wrapped[pos.line]
            .position_for_index(pos.col.min(wrapped[pos.line].len()), lh)
            .unwrap_or_default();
        point(p.x, tops[pos.line] + p.y)
    };
    let caret_col = s.cursor.col + marked_at.as_ref().map_or(0, |r| r.len());
    let caret = at(Pos::new(s.cursor.line, caret_col), &wrapped);

    let mut scroll = s.scroll;
    let mut moved = false;
    if s.reveal {
        if caret.y < scroll {
            scroll = caret.y;
            moved = true;
        } else if caret.y + lh > scroll + bounds.size.height {
            scroll = caret.y + lh - bounds.size.height;
            moved = true;
        }
    }
    let max_scroll = (content_height - bounds.size.height).max(px(0.));
    if scroll > max_scroll {
        scroll = max_scroll;
        moved = true;
    }
    let origin = point(bounds.left(), bounds.top() - scroll);

    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        if s.lines.line_count() == 1 && s.lines.line(0).is_empty() && s.marked.is_none() {
            let hint = shape(
                &s.placeholder,
                font,
                s.font_size,
                s.placeholder_color,
                window,
            );
            let _ = hint.paint(origin, lh, TextAlign::Left, None, window, cx);
        }
        if let Some((a, b)) = s.selection {
            for ix in a.line..=b.line {
                let line = &wrapped[ix];
                let start = if ix == a.line { a.col } else { 0 };
                let end = if ix == b.line { b.col } else { line.len() };
                let p1 = at(Pos::new(ix, start), &wrapped);
                let p2 = at(Pos::new(ix, end), &wrapped);
                // The line break shows as a bit of selection.
                let tail = if ix == b.line {
                    px(0.)
                } else {
                    s.font_size / 2.
                };
                let mut row = p1.y;
                while row <= p2.y {
                    let x1 = if row == p1.y { p1.x } else { px(0.) };
                    let x2 = if row == p2.y { p2.x + tail } else { width };
                    if x2 > x1 {
                        window.paint_quad(fill(
                            Bounds::new(point(origin.x + x1, origin.y + row), size(x2 - x1, lh)),
                            s.selection_bg,
                        ));
                    }
                    row += lh;
                }
            }
        }
        for (ix, line) in wrapped.iter().enumerate() {
            let top = origin.y + tops[ix];
            if top > bounds.bottom() || top + line.size(lh).height < bounds.top() {
                continue;
            }
            let _ = line.paint(point(origin.x, top), lh, TextAlign::Left, None, window, cx);
        }
        if s.focused {
            window.paint_quad(fill(
                Bounds::new(
                    point(origin.x + caret.x, origin.y + caret.y + px(2.)),
                    size(px(2.), lh - px(4.)),
                ),
                s.caret,
            ));
        }
    });

    let mut layout = s.layout.borrow_mut();
    let resized = layout.content_height != content_height;
    *layout = Layout {
        bounds,
        line_height: lh,
        text_left: bounds.left(),
        wrapped,
        tops,
        content_height,
        ..Layout::default()
    };
    (moved.then_some(scroll), resized)
}
