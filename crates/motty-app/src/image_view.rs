//! A tab that shows an image, as Otty's does: fitted to the pane (never
//! past actual size), zoomed by pinching, ⌘-scrolling, ⌘+ / ⌘- / ⌘0, a
//! double click or the toolbar's menu, panned by scrolling or dragging, and
//! ← / → to the folder's other images.
//!
//! GPUI decodes the file (PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, SVG); what
//! it can't, HEIC above all, macOS converts first. Actual size is a point
//! per pixel.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Asset, Bounds, Context, CursorStyle, EventEmitter, FocusHandle, Focusable,
    ImageAssetLoader, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, PathBuilder,
    PinchEvent, Pixels, Point, RenderImage, Resource, ScrollEvent, Size, Task, Window, canvas, div,
    img, point, prelude::*, px, size,
};
use motty_render::{TerminalStyle, hsla};
use motty_term::alacritty_terminal::vte::ansi::NamedColor;
use motty_theme::Rgba;

use crate::actions::{DecreaseFontSize, IncreaseFontSize, ResetFontSize};
use crate::file_kind::{IMAGE_EXTENSIONS, extension};
use crate::file_toolbar::{self, Ink};

/// Zoom stops for ⌘+ and ⌘-.
const STEPS: [f32; 19] = [
    0.05, 0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1., 1.25, 1.5, 2., 3., 4., 6., 8., 12., 16., 24., 32.,
];
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 32.;
/// The zoom menu's items; `None` fits.
const MENU: [Option<f32>; 5] = [None, Some(0.5), Some(1.), Some(2.), Some(4.)];
/// Scrolling this far with ⌘ held zooms by e.
const SCROLL_PER_E: f32 = 200.;

pub enum ImageEvent {
    /// The path changed (← / →), so the title did.
    Changed,
}

pub struct ImageView {
    focus: FocusHandle,
    path: PathBuf,
    style: Rc<TerminalStyle>,
    /// The decoded image; the last one stays up while the next loads.
    image: Option<Arc<RenderImage>>,
    error: Option<String>,
    /// `None` fits the image in the pane.
    zoom: Option<f32>,
    /// The image's center, from the pane's.
    pan: Point<Pixels>,
    /// The image area, as last painted.
    viewport: Rc<Cell<Bounds<Pixels>>>,
    /// The zoom button, as last painted, for its menu.
    zoom_button: Rc<Cell<Bounds<Pixels>>>,
    /// Where the pointer was while dragging the image.
    drag: Option<Point<Pixels>>,
    _load: Task<()>,
}

impl EventEmitter<ImageEvent> for ImageView {}

impl Focusable for ImageView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ImageView {
    pub fn new(path: PathBuf, style: TerminalStyle, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| {
            if let Some(image) = this.image.take() {
                cx.drop_image(image, None);
            }
        })
        .detach();
        let mut view = Self {
            focus: cx.focus_handle(),
            path: PathBuf::new(),
            style: Rc::new(style),
            image: None,
            error: None,
            zoom: None,
            pan: Point::default(),
            viewport: Rc::default(),
            zoom_button: Rc::default(),
            drag: None,
            _load: Task::ready(()),
        };
        view.load(path, cx);
        view
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn title(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }

    /// Where a new terminal opened from here should start.
    pub fn folder(&self) -> Option<PathBuf> {
        self.path.parent().map(Path::to_path_buf)
    }

    pub fn set_style(&mut self, style: TerminalStyle, cx: &mut Context<Self>) {
        self.style = Rc::new(style);
        cx.notify();
    }

    /// Shows `path` instead, fitted once it's decoded.
    pub fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if path != self.path {
            self.load(path, cx);
            cx.emit(ImageEvent::Changed);
            cx.notify();
        }
    }

    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.path = path.clone();
        let heic = matches!(extension(&path).as_str(), "heic" | "heif");
        let decode = ImageAssetLoader::load(Resource::Path(path.clone().into()), cx);
        let svg = cx.svg_renderer();
        let decoded = cx.background_executor().spawn(async move {
            let decoded = if heic {
                Err(String::new())
            } else {
                decode.await.map_err(|e| e.to_string())
            };
            decoded.or_else(|e| {
                let tiff = convert(&path).ok_or(e)?;
                gpui::Image::from_bytes(gpui::ImageFormat::Tiff, tiff)
                    .to_image_data(svg)
                    .map_err(|e| e.to_string())
            })
        });
        self._load = cx.spawn(async move |this, cx| {
            let decoded = decoded.await;
            let _ = this.update(cx, |this, cx| this.loaded(decoded, cx));
        });
    }

    fn loaded(&mut self, decoded: Result<Arc<RenderImage>, String>, cx: &mut Context<Self>) {
        let old = match decoded {
            Ok(image) => {
                self.error = None;
                self.image.replace(image)
            }
            Err(e) => {
                tracing::warn!(path = %self.path.display(), "can't show the image: {e}");
                self.error = Some(format!("Couldn't open {}", self.title()));
                self.image.take()
            }
        };
        if let Some(old) = old {
            cx.drop_image(old, None);
        }
        self.zoom = None;
        self.pan = Point::default();
        cx.notify();
    }

    /// The image's size at 100%.
    fn natural(&self) -> Size<Pixels> {
        self.image.as_ref().map_or(Size::default(), |image| {
            let s = image.size(0);
            size(px(s.width.0 as f32), px(s.height.0 as f32))
        })
    }

    /// Fits the image in the pane, without enlarging it.
    fn fit(&self) -> f32 {
        let natural = self.natural();
        let pane = self.viewport.get().size;
        if natural.width <= px(0.) || natural.height <= px(0.) {
            return 1.;
        }
        (pane.width / natural.width)
            .min(pane.height / natural.height)
            .clamp(MIN_ZOOM, 1.)
    }

    fn scale(&self) -> f32 {
        self.zoom.unwrap_or_else(|| self.fit())
    }

    /// Zooms to `zoom` (`None` fits), keeping the point under `anchor`
    /// (window coordinates; the pane's center when `None`) in place.
    fn set_zoom(
        &mut self,
        zoom: Option<f32>,
        anchor: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let old = self.scale();
        self.zoom = zoom.map(|z| z.clamp(MIN_ZOOM, MAX_ZOOM));
        if self.zoom.is_none() {
            self.pan = Point::default();
        } else {
            let center = self.viewport.get().center();
            let a = anchor.map_or(Point::default(), |p| p - center);
            let ratio = self.scale() / old;
            self.pan = a - (a - self.pan) * ratio;
        }
        self.clamp_pan();
        cx.notify();
    }

    fn zoom_by(&mut self, factor: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        if factor.is_finite() && factor > 0. {
            self.set_zoom(Some(self.scale() * factor), anchor, cx);
        }
    }

    fn zoom_step(&mut self, larger: bool, cx: &mut Context<Self>) {
        let now = self.scale();
        let next = if larger {
            STEPS.iter().copied().find(|&z| z > now * 1.001)
        } else {
            STEPS.iter().rev().copied().find(|&z| z < now * 0.999)
        };
        if let Some(z) = next {
            self.set_zoom(Some(z), None, cx);
        }
    }

    /// Keeps the image over the pane: centered along an axis it fits in,
    /// its edges no further in than the pane's along one it doesn't.
    fn clamp_pan(&mut self) {
        let natural = self.natural();
        let pane = self.viewport.get().size;
        let scale = self.scale();
        let slack = |image: Pixels, pane: Pixels| ((image * scale - pane) / 2.).max(px(0.));
        let (sx, sy) = (
            slack(natural.width, pane.width),
            slack(natural.height, pane.height),
        );
        self.pan = point(self.pan.x.clamp(-sx, sx), self.pan.y.clamp(-sy, sy));
    }

    /// ← / →: the folder's previous or next image.
    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(folder) = self.path.parent() else {
            return;
        };
        if let Some(next) = neighbor(&images_in(folder), &self.path, forward) {
            self.open(next, cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        let cmd = m.platform && !m.control && !m.alt;
        match ks.key.as_str() {
            "left" if !m.modified() => self.step(false, cx),
            "right" if !m.modified() => self.step(true, cx),
            "=" | "+" if cmd => self.zoom_step(true, cx),
            "-" if cmd => self.zoom_step(false, cx),
            "0" if cmd => self.set_zoom(Some(1.), None, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(20.));
        if event.modifiers.platform {
            let factor = (f32::from(delta.y) / SCROLL_PER_E).exp();
            return self.zoom_by(factor, Some(event.position), cx);
        }
        self.pan += delta;
        self.clamp_pan();
        cx.notify();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        if event.click_count == 2 {
            // Double click: fit ↔ actual size, around the click.
            let zoom = match self.zoom {
                None => Some(1.),
                Some(_) => None,
            };
            self.set_zoom(zoom, Some(event.position), cx);
        } else {
            self.drag = Some(event.position);
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(from) = self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        self.drag = Some(event.position);
        self.pan += event.position - from;
        self.clamp_pan();
        cx.notify();
    }

    /// The zoom menu, under its button.
    fn zoom_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let button = self.zoom_button.get();
        let at = point(button.left(), button.bottom() + px(2.));
        let items = MENU
            .iter()
            .map(|&zoom| {
                let label = zoom.map_or("Fit".into(), percent);
                crate::native_menu::Item::new(label)
                    .checked(zoom == self.zoom)
                    .into()
            })
            .collect();
        let entity = cx.entity();
        crate::native_menu::popup(items, at, None, window, cx, move |ix, _, cx| {
            entity.update(cx, |this, cx| this.set_zoom(MENU[ix], None, cx));
        });
    }

    fn foreground(&self) -> Rgba {
        self.style.palette[NamedColor::Foreground as usize]
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ink = Ink(self.foreground());
        let button = self.zoom_button.clone();
        let zoom = div()
            .id("image-zoom")
            .relative()
            .flex_none()
            .h(px(24.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(5.))
            .text_size(px(13.))
            .text_color(ink.at(0.85))
            .hover(move |s| s.bg(ink.at(0.07)))
            .child(percent(self.scale()))
            .child(triangle(ink.at(0.75)))
            .child(
                canvas(move |bounds, _, _| button.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .on_click(cx.listener(|this, _, window, cx| this.zoom_menu(window, cx)));
        let left = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(zoom)
            .child(
                div()
                    .flex_none()
                    .ml(px(4.))
                    .mr(px(2.5))
                    .w(px(1.))
                    .h(px(16.))
                    .bg(ink.at(0.15)),
            )
            .child(file_toolbar::send_to_chat(ink))
            .child(file_toolbar::share(self.path.clone(), ink));
        file_toolbar::bar(left, file_toolbar::close(self.focus.clone(), ink))
    }
}

impl Render for ImageView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.clamp_pan();
        let ink = Ink(self.foreground());
        let toolbar = self.render_toolbar(cx);
        let pane = self.viewport.get().size;
        let scale = self.scale();
        let natural = self.natural();
        let (w, h) = (natural.width * scale, natural.height * scale);
        let pannable = w > pane.width + px(0.5) || h > pane.height + px(0.5);

        let viewport = self.viewport.clone();
        let id = cx.entity_id();
        let measure = canvas(
            move |bounds, _, cx| {
                if viewport.get() != bounds {
                    viewport.set(bounds);
                    cx.notify(id);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let content = match (&self.image, &self.error) {
            (_, Some(error)) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.))
                .text_color(ink.at(0.5))
                .child(error.clone())
                .into_any_element(),
            (Some(image), None) if pane.width > px(0.) => img(image.clone())
                .id("image")
                .absolute()
                .left((pane.width - w) / 2. + self.pan.x)
                .top((pane.height - h) / 2. + self.pan.y)
                .w(w)
                .h(h)
                .into_any_element(),
            _ => div().into_any_element(),
        };

        div()
            .id("image-view")
            .key_context("ImageView")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(self.style.background))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|this, _: &IncreaseFontSize, _, cx| this.zoom_step(true, cx)))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, _, cx| this.zoom_step(false, cx)))
            .on_action(
                cx.listener(|this, _: &ResetFontSize, _, cx| this.set_zoom(Some(1.), None, cx)),
            )
            .child(toolbar)
            .child(
                div()
                    .id("image-area")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .when(pannable, |d| {
                        d.cursor(if self.drag.is_some() {
                            CursorStyle::ClosedHand
                        } else {
                            CursorStyle::OpenHand
                        })
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.drag = None;
                            cx.notify();
                        }),
                    )
                    .on_scroll(cx.listener(Self::on_scroll))
                    .on_pinch(cx.listener(|this, e: &PinchEvent, _, cx| {
                        this.zoom_by(1. + e.delta, Some(e.position), cx)
                    }))
                    .child(measure)
                    .child(content),
            )
    }
}

/// "100%".
fn percent(zoom: f32) -> String {
    format!("{}%", (zoom * 100.).round())
}

/// The zoom button's ▾, 6×3.5pt.
fn triangle(color: gpui::Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let mut path = PathBuilder::fill();
            path.move_to(b.origin);
            path.line_to(point(b.right(), b.top()));
            path.line_to(point(b.center().x, b.bottom()));
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .flex_none()
    .w(px(6.))
    .h(px(3.5))
}

/// Whether ← / → stop at `path`: the viewer's images, and SVGs.
fn steppable(path: &Path) -> bool {
    let ext = extension(path);
    ext == "svg" || IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// The images in `folder`, by name.
fn images_in(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| e.path())
        .filter(|p| steppable(p))
        .collect();
    files.sort_by_key(|p| sort_key(p));
    files
}

/// Names compare ignoring case, then by case so the order is total.
fn sort_key(path: &Path) -> (String, String) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (name.to_lowercase(), name)
}

/// The file after (or before) `current` in `files`, sorted by
/// [`sort_key`]; `current` needn't be among them (it was deleted).
fn neighbor(files: &[PathBuf], current: &Path, forward: bool) -> Option<PathBuf> {
    let key = sort_key(current);
    if forward {
        files.iter().find(|f| sort_key(f) > key).cloned()
    } else {
        files.iter().rev().find(|f| sort_key(f) < key).cloned()
    }
}

/// The image macOS decodes from `path`, as TIFF, for formats GPUI can't
/// decode (HEIC).
#[cfg(target_os = "macos")]
fn convert(path: &Path) -> Option<Vec<u8>> {
    use objc2::AnyThread;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSString;

    // A background thread has no autorelease pool of its own.
    objc2::rc::autoreleasepool(|_| {
        let file = NSString::from_str(&path.to_string_lossy());
        let image = NSImage::initWithContentsOfFile(NSImage::alloc(), &file)?;
        Some(image.TIFFRepresentation()?.to_vec())
    })
}

#[cfg(not(target_os = "macos"))]
fn convert(_: &Path) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = names.iter().map(|n| Path::new("/pics").join(n)).collect();
        files.sort_by_key(|p| sort_key(p));
        files
    }

    #[test]
    fn sorts_ignoring_case() {
        let files = paths(&["b.png", "A.png", "c.JPG", "a.png"]);
        let names: Vec<_> = files.iter().map(|p| p.file_name().unwrap()).collect();
        assert_eq!(names, ["A.png", "a.png", "b.png", "c.JPG"]);
    }

    #[test]
    fn steps_without_wrapping() {
        let files = paths(&["a.png", "B.jpg", "c.gif"]);
        let at = |n: &str| Path::new("/pics").join(n);
        assert_eq!(neighbor(&files, &at("B.jpg"), true), Some(at("c.gif")));
        assert_eq!(neighbor(&files, &at("B.jpg"), false), Some(at("a.png")));
        assert_eq!(neighbor(&files, &at("c.gif"), true), None);
        assert_eq!(neighbor(&files, &at("a.png"), false), None);
    }

    #[test]
    fn steps_from_a_deleted_file() {
        let files = paths(&["a.png", "c.png"]);
        let gone = Path::new("/pics/b.png");
        assert_eq!(
            neighbor(&files, gone, true),
            Some(Path::new("/pics/c.png").into())
        );
        assert_eq!(
            neighbor(&files, gone, false),
            Some(Path::new("/pics/a.png").into())
        );
    }

    #[test]
    fn steps_over_svgs_not_text() {
        assert!(steppable(Path::new("x.svg")));
        assert!(steppable(Path::new("x.HEIC")));
        assert!(!steppable(Path::new("x.txt")));
        assert!(!steppable(Path::new("x.pdf")));
    }

    #[test]
    fn percent_rounds() {
        assert_eq!(percent(1.), "100%");
        assert_eq!(percent(0.333), "33%");
    }
}
