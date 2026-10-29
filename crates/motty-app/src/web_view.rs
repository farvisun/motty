//! A native web view (WKWebView) inside a GPUI window, for web pages and
//! HTML previews.
//!
//! The WKWebView is a subview of GPUI's NSView, so it draws over GPUI's
//! content. Every frame that paints the [`WebView`] moves it to the painted
//! bounds (clipped to the content mask, so scrolling works). A frame that
//! doesn't paint it hides it: the paint keeps a [`Presence`] in GPUI's
//! element state, and GPUI drops element state a frame didn't use as that
//! frame ends.
//!
//! Elsewhere than macOS the entity renders an empty area.

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Bounds, Context, DispatchPhase, ElementId, EntityId, EventEmitter, MouseDownEvent, Pixels,
    Point, Rgba, Task, Window, canvas, div, prelude::*,
};

#[cfg(target_os = "macos")]
use mac::Native;
#[cfg(target_os = "macos")]
pub(crate) use mac::Overlay;
#[cfg(not(target_os = "macos"))]
use stub::Native;

/// How often the page's URL, title and loading state are checked.
const POLL_INTERVAL: Duration = Duration::from_millis(300);

/// Set when the view is created.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WebViewOptions {
    pub javascript: bool,
    /// Private browsing: nothing (cookies, caches, storage) touches disk.
    pub non_persistent: bool,
    /// Identifies as Safari on iPhone, for mobile layouts.
    pub mobile: bool,
    /// The page may talk back: `window.webkit.messageHandlers.motty
    /// .postMessage(string)` arrives as [`WebEvent::Message`]. Only for
    /// Motty's own pages.
    pub messages: bool,
}

impl Default for WebViewOptions {
    fn default() -> Self {
        Self {
            javascript: true,
            non_persistent: false,
            mobile: false,
            messages: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum WebEvent {
    UrlChanged(Option<String>),
    TitleChanged(Option<String>),
    LoadingChanged(bool),
    NavigationChanged {
        can_go_back: bool,
        can_go_forward: bool,
    },
    /// The answer to [`WebView::find`].
    FindResult {
        found: bool,
    },
    /// A string the page posted (see [`WebViewOptions::messages`]).
    Message(String),
}

/// The page state that [`WebEvent`]s report changes of.
#[derive(Clone, Debug, Default, PartialEq)]
struct Snapshot {
    url: Option<String>,
    title: Option<String>,
    loading: bool,
    can_go_back: bool,
    can_go_forward: bool,
}

pub struct WebView {
    native: Option<Rc<Native>>,
    background: Option<Rgba>,
    /// Where it was last drawn, in window coordinates.
    bounds: Rc<std::cell::Cell<Bounds<Pixels>>>,
    snapshot: Snapshot,
    _poll: Task<()>,
}

impl EventEmitter<WebEvent> for WebView {}

impl WebView {
    pub fn new(options: WebViewOptions, cx: &mut Context<Self>) -> Self {
        let (sender, messages) = async_channel::unbounded();
        let native = Native::new(options, sender).map(Rc::new);
        if native.is_none() && cfg!(target_os = "macos") {
            tracing::warn!("could not create a web view");
        }
        if options.messages {
            cx.spawn(async move |this, cx| {
                while let Ok(message) = messages.recv().await {
                    if this
                        .update(cx, |_, cx| cx.emit(WebEvent::Message(message)))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            native,
            background: None,
            bounds: Rc::default(),
            snapshot: Snapshot::default(),
            _poll: poll,
        }
    }

    /// A point on the page as a point in the window, for a menu there.
    pub fn to_window(&self, at: Point<Pixels>) -> Point<Pixels> {
        self.bounds.get().origin + at
    }

    pub fn load_url(&self, url: &str) {
        if let Some(native) = &self.native {
            native.load_url(url);
        }
    }

    /// Shows `html`. Relative links resolve against `base`, a file (its
    /// folder) or a folder, which the page may then read from.
    pub fn load_html(&self, html: &str, base: Option<&Path>) {
        if let Some(native) = &self.native {
            native.load_html(html, base);
        }
    }

    /// Opens a local file, letting it read files under `read_access`
    /// (its own folder when `None`).
    pub fn load_file(&self, path: &Path, read_access: Option<&Path>) {
        if let Some(native) = &self.native {
            native.load_file(path, read_access);
        }
    }

    pub fn back(&self) {
        if let Some(native) = &self.native {
            native.back();
        }
    }

    pub fn forward(&self) {
        if let Some(native) = &self.native {
            native.forward();
        }
    }

    /// `bypass_cache` revalidates everything with the server.
    pub fn reload(&self, bypass_cache: bool) {
        if let Some(native) = &self.native {
            native.reload(bypass_cache);
        }
    }

    pub fn stop(&self) {
        if let Some(native) = &self.native {
            native.stop();
        }
    }

    pub fn url(&self) -> Option<String> {
        self.native.as_ref()?.url()
    }

    pub fn title(&self) -> Option<String> {
        self.native.as_ref()?.title()
    }

    pub fn can_go_back(&self) -> bool {
        self.native.as_ref().is_some_and(|n| n.can_go_back())
    }

    pub fn can_go_forward(&self) -> bool {
        self.native.as_ref().is_some_and(|n| n.can_go_forward())
    }

    pub fn is_loading(&self) -> bool {
        self.native.as_ref().is_some_and(|n| n.is_loading())
    }

    /// Selects the next (or previous) match of `text`, wrapping around, and
    /// reports whether there was one with [`WebEvent::FindResult`].
    pub fn find(&self, text: &str, backwards: bool, cx: &mut Context<Self>) {
        let Some(native) = &self.native else {
            return;
        };
        let (tx, rx) = async_channel::bounded(1);
        native.find(text, backwards, move |found| {
            let _ = tx.try_send(found);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(found) = rx.recv().await {
                let _ = this.update(cx, |_, cx| cx.emit(WebEvent::FindResult { found }));
            }
        })
        .detach();
    }

    /// Page zoom; 1.0 is actual size.
    pub fn set_zoom(&self, zoom: f64) {
        if let Some(native) = &self.native {
            native.set_zoom(zoom);
        }
    }

    /// Runs `script` in the page, ignoring its result.
    pub fn evaluate_js(&self, script: &str) {
        if let Some(native) = &self.native {
            native.evaluate_js(script);
        }
    }

    /// Gives the web view the keyboard. GPUI gets it back when it's clicked
    /// or [`Self::blur_native`] is called.
    pub fn focus_native(&self) {
        if let Some(native) = &self.native {
            native.focus();
        }
    }

    /// Hands the keyboard back to GPUI if the web view has it.
    pub fn blur_native(&self) {
        if let Some(native) = &self.native {
            native.blur();
        }
    }

    /// The page's light or dark appearance (`prefers-color-scheme`).
    pub fn set_appearance(&self, dark: bool) {
        if let Some(native) = &self.native {
            native.set_appearance(dark);
        }
    }

    /// Shown before the page draws and past its edges, instead of white.
    pub fn set_background(&mut self, color: Rgba, cx: &mut Context<Self>) {
        if let Some(native) = &self.native {
            native.set_background(color);
        }
        self.background = Some(color);
        cx.notify();
    }

    /// Hides the web view while a GPUI overlay (a palette, a menu) covers
    /// it, since it would draw over the overlay.
    pub fn set_suppressed(&self, suppressed: bool) {
        if let Some(native) = &self.native {
            native.set_suppressed(suppressed);
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let Some(native) = &self.native else {
            return;
        };
        let now = Snapshot {
            url: native.url(),
            title: native.title(),
            loading: native.is_loading(),
            can_go_back: native.can_go_back(),
            can_go_forward: native.can_go_forward(),
        };
        let was = std::mem::replace(&mut self.snapshot, now.clone());
        if now.url != was.url {
            cx.emit(WebEvent::UrlChanged(now.url));
        }
        if now.title != was.title {
            cx.emit(WebEvent::TitleChanged(now.title));
        }
        if now.loading != was.loading {
            cx.emit(WebEvent::LoadingChanged(now.loading));
        }
        if (now.can_go_back, now.can_go_forward) != (was.can_go_back, was.can_go_forward) {
            cx.emit(WebEvent::NavigationChanged {
                can_go_back: now.can_go_back,
                can_go_forward: now.can_go_forward,
            });
        }
    }
}

impl Drop for WebView {
    fn drop(&mut self) {
        if let Some(native) = &self.native {
            native.detach();
        }
    }
}

impl Render for WebView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let native = self.native.clone();
        let id = cx.entity_id();
        let drawn = self.bounds.clone();
        div()
            .size_full()
            .when_some(self.background, |div, color| div.bg(color))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, _| {
                        drawn.set(bounds);
                        if let Some(native) = native {
                            paint_overlay(native, "web-view", id, bounds, window);
                        }
                    },
                )
                .size_full(),
            )
    }
}

/// A native view drawn over GPUI's, where its element paints (see the
/// module docs): the web view, Quick Look's preview.
pub(crate) trait Overlaid {
    /// Records a paint and returns its number.
    fn painted(&self) -> u64;
    /// A frame ended without paint `paint` being repeated.
    fn unpainted(&self, paint: u64);
    /// Moves the view to `bounds`, showing the `visible` part, both in
    /// GPUI's window coordinates (points, y down).
    fn place(&self, window: &Window, bounds: Bounds<Pixels>, visible: Bounds<Pixels>);
    /// Hands the keyboard back to GPUI if the view has it.
    fn blur(&self);
    /// Hidden while something GPUI draws covers it.
    fn set_suppressed(&self, suppressed: bool);
}

/// Shows `native` over `bounds` for this frame; `name` and `id` tell its
/// element state apart.
pub(crate) fn paint_overlay(
    native: Rc<dyn Overlaid>,
    name: &'static str,
    id: EntityId,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let visible = bounds.intersect(&window.content_mask().bounds);
    window.with_global_id(
        ElementId::NamedInteger(name.into(), id.as_u64()),
        |id, window| {
            window.with_element_state::<Presence, _>(id, |presence, _| {
                let paint = native.painted();
                let presence = match presence {
                    Some(mut presence) => {
                        presence.paint = paint;
                        presence
                    }
                    None => Presence {
                        native: native.clone(),
                        paint,
                    },
                };
                ((), presence)
            })
        },
    );
    native.set_suppressed(covered(window));
    native.place(window, bounds, visible);
    // Clicks on the native view go to it directly; one that reaches GPUI
    // is outside it, so GPUI takes the keyboard back (its NSView doesn't
    // take it on click by itself).
    window.on_mouse_event(move |_: &MouseDownEvent, phase, _, _| {
        if phase == DispatchPhase::Capture {
            native.blur();
        }
    });
}

thread_local! {
    /// Windows with something GPUI draws over their content (a palette).
    static COVERED: std::cell::RefCell<std::collections::HashSet<gpui::WindowId>> =
        std::cell::RefCell::default();
}

/// Hides the window's web views while GPUI draws `covered` overlays, which
/// they would otherwise cover.
pub fn set_covered(window: &Window, covered: bool) {
    let id = window.window_handle().window_id();
    COVERED.with_borrow_mut(|set| {
        if covered {
            set.insert(id);
        } else {
            set.remove(&id);
        }
    });
}

fn covered(window: &Window) -> bool {
    let id = window.window_handle().window_id();
    COVERED.with_borrow(|set| set.contains(&id))
}

/// Lives in GPUI's element state while the view is painted each frame;
/// dropped when a frame ends without painting it, which hides the view.
struct Presence {
    native: Rc<dyn Overlaid>,
    /// The paint that last kept this alive. A newer paint under another
    /// element id (the pane moved in the tree) outlives this one.
    paint: u64,
}

impl Drop for Presence {
    fn drop(&mut self) {
        self.native.unpainted(self.paint);
    }
}

impl Overlaid for Native {
    fn painted(&self) -> u64 {
        Native::painted(self)
    }
    fn unpainted(&self, paint: u64) {
        Native::unpainted(self, paint)
    }
    fn place(&self, window: &Window, bounds: Bounds<Pixels>, visible: Bounds<Pixels>) {
        Native::place(self, window, bounds, visible)
    }
    fn blur(&self) {
        Native::blur(self)
    }
    fn set_suppressed(&self, suppressed: bool) {
        Native::set_suppressed(self, suppressed)
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::Cell;
    use std::path::Path;

    use block2::RcBlock;
    use gpui::{Bounds, Pixels, Rgba, Window};
    use objc2::DefinedClass;
    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSColor, NSView,
    };
    use objc2_foundation::{
        NSNumber, NSObjectNSKeyValueCoding, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest,
        ns_string,
    };
    use objc2_web_kit::{
        WKAudiovisualMediaTypes, WKFindConfiguration, WKFindResult, WKNavigationAction,
        WKScriptMessage, WKScriptMessageHandler, WKUIDelegate, WKUserContentController, WKWebView,
        WKWebViewConfiguration, WKWebpagePreferences, WKWebsiteDataStore, WKWindowFeatures,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::WebViewOptions;

    const MOBILE_USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) \
        AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1";

    define_class!(
        // Opens `target=_blank` links and `window.open` in the same view.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "MottyWebUIDelegate"]
        struct UiDelegate;

        unsafe impl NSObjectProtocol for UiDelegate {}

        unsafe impl WKUIDelegate for UiDelegate {
            #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
            fn create_web_view(
                &self,
                web_view: &WKWebView,
                _configuration: &WKWebViewConfiguration,
                action: &WKNavigationAction,
                _features: &WKWindowFeatures,
            ) -> Option<Retained<WKWebView>> {
                // SAFETY: the action and the web view are valid for the call,
                // which is on the main thread.
                unsafe { web_view.loadRequest(&action.request()) };
                None
            }
        }
    );

    define_class!(
        // Hands what the page posts to `window.webkit.messageHandlers.motty`
        // to the GPUI side.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "MottyWebMessages"]
        #[ivars = async_channel::Sender<String>]
        struct Messages;

        unsafe impl NSObjectProtocol for Messages {}

        unsafe impl WKScriptMessageHandler for Messages {
            #[unsafe(method(userContentController:didReceiveScriptMessage:))]
            fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
                // SAFETY: the message is valid for the call, on the main thread.
                let body = unsafe { message.body() };
                if let Some(text) = body.downcast_ref::<NSString>() {
                    let _ = self.ivars().try_send(text.to_string());
                }
            }
        }
    );

    impl Messages {
        fn new(mtm: MainThreadMarker, sender: async_channel::Sender<String>) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(sender);
            // SAFETY: `NSObject`'s `init` on a freshly allocated object.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// The name pages post to.
    const MESSAGES: &str = "motty";

    /// A native view over GPUI's, inside a view that clips it to the
    /// visible part of its element; hidden while it isn't painted.
    pub struct Overlay {
        clip: Retained<NSView>,
        content: Retained<NSView>,
        /// Counts paints; see [`super::Presence`].
        paints: Cell<u64>,
        painted: Cell<bool>,
        suppressed: Cell<bool>,
        /// Painted bounds have some area on screen.
        on_screen: Cell<bool>,
        detached: Cell<bool>,
    }

    impl Overlay {
        /// Wraps `content`, hidden until it's painted.
        pub fn new(content: Retained<NSView>) -> Self {
            let mtm = MainThreadMarker::from(&*content);
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.));
            let clip = NSView::initWithFrame(mtm.alloc(), frame);
            // Views no longer clip their subviews by default as of macOS 14;
            // before that the setter doesn't exist and they always do.
            if clip.respondsToSelector(sel!(setClipsToBounds:)) {
                clip.setClipsToBounds(true);
            }
            clip.setHidden(true);
            clip.addSubview(&content);
            Self {
                clip,
                content,
                paints: Cell::new(0),
                painted: Cell::new(false),
                suppressed: Cell::new(false),
                on_screen: Cell::new(false),
                detached: Cell::new(false),
            }
        }

        /// Records a paint and returns its number.
        pub fn painted(&self) -> u64 {
            let paint = self.paints.get() + 1;
            self.paints.set(paint);
            self.painted.set(true);
            paint
        }

        /// A frame ended without paint `paint` being repeated.
        pub fn unpainted(&self, paint: u64) {
            if paint == self.paints.get() {
                self.painted.set(false);
                self.update_hidden();
            }
        }

        pub fn set_suppressed(&self, suppressed: bool) {
            self.suppressed.set(suppressed);
            self.update_hidden();
        }

        fn update_hidden(&self) {
            let hidden = self.detached.get()
                || !self.painted.get()
                || self.suppressed.get()
                || !self.on_screen.get();
            if hidden == self.clip.isHidden() {
                return;
            }
            if hidden {
                // Hiding the first responder would pass the keyboard to
                // whatever view is next in the key loop.
                self.blur();
            }
            self.clip.setHidden(hidden);
        }

        /// Moves the view to `bounds`, showing the `visible` part, both in
        /// GPUI's window coordinates (points, y down).
        pub fn place(&self, window: &Window, bounds: Bounds<Pixels>, visible: Bounds<Pixels>) {
            if self.detached.get() {
                return;
            }
            let Some(host) = gpui_view(window) else {
                return;
            };
            let scale = f64::from(window.scale_factor());
            // SAFETY: reading the superview on the main thread.
            let attached = unsafe { self.clip.superview() }.is_some_and(|s| &*s == host);
            if !attached {
                self.clip.removeFromSuperview();
                host.addSubview(&self.clip);
            }
            let host_size = host.bounds().size;
            let host_rect = Rect::new(0., 0., host_size.width, host_size.height);
            let visible = Rect::from(visible).snap(scale);
            let bounds = Rect::from(bounds).snap(scale);
            self.clip
                .setFrame(visible.frame_in(&host_rect, host.isFlipped()));
            self.content
                .setFrame(bounds.frame_in(&visible, self.clip.isFlipped()));
            self.on_screen
                .set(visible.width > 0. && visible.height > 0.);
            self.update_hidden();
        }

        /// Takes the view out of its window for good.
        pub fn detach(&self) {
            self.detached.set(true);
            self.blur();
            self.clip.removeFromSuperview();
        }

        /// Gives the view the keyboard.
        pub fn focus(&self) {
            if let Some(window) = self.content.window() {
                window.makeFirstResponder(Some(&self.content));
            }
        }

        /// Gives the keyboard back to GPUI's view if the view has it.
        pub fn blur(&self) {
            let Some(window) = self.content.window() else {
                return;
            };
            let focused = window.firstResponder().is_some_and(|responder| {
                responder
                    .downcast_ref::<NSView>()
                    .is_some_and(|view| view.isDescendantOf(&self.clip))
            });
            // SAFETY: reading the superview on the main thread.
            if focused && let Some(host) = unsafe { self.clip.superview() } {
                window.makeFirstResponder(Some(&host));
            }
        }
    }

    /// The web view, as an [`Overlay`].
    pub struct Native {
        overlay: Overlay,
        web: Retained<WKWebView>,
        _delegate: Retained<UiDelegate>,
    }

    impl Native {
        pub fn new(options: WebViewOptions, sender: async_channel::Sender<String>) -> Option<Self> {
            let mtm = MainThreadMarker::new()?;
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.));
            // SAFETY: plain WebKit configuration on the main thread; the
            // configuration is copied by the web view's initializer.
            let web = unsafe {
                let config = WKWebViewConfiguration::new(mtm);
                let page = WKWebpagePreferences::new(mtm);
                page.setAllowsContentJavaScript(options.javascript);
                config.setDefaultWebpagePreferences(Some(&page));
                config.setMediaTypesRequiringUserActionForPlayback(WKAudiovisualMediaTypes::All);
                if options.non_persistent {
                    config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
                }
                if options.messages {
                    // The controller keeps the handler until `detach`.
                    let handler = Messages::new(mtm, sender);
                    config.userContentController().addScriptMessageHandler_name(
                        ProtocolObject::from_ref(&*handler),
                        &NSString::from_str(MESSAGES),
                    );
                }
                WKWebView::initWithFrame_configuration(mtm.alloc(), frame, &config)
            };
            if options.mobile {
                // SAFETY: setting a string property on the main thread.
                unsafe { web.setCustomUserAgent(Some(&NSString::from_str(MOBILE_USER_AGENT))) };
            }
            // SAFETY: `NSObject`'s `init` on a freshly allocated delegate.
            let delegate: Retained<UiDelegate> = unsafe { msg_send![UiDelegate::alloc(mtm), init] };
            // SAFETY: the web view holds its delegate weakly; `Native` keeps
            // it alive for as long as the web view.
            unsafe { web.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
            Some(Self {
                overlay: Overlay::new(Retained::into_super(web.clone())),
                web,
                _delegate: delegate,
            })
        }

        pub fn painted(&self) -> u64 {
            self.overlay.painted()
        }

        pub fn unpainted(&self, paint: u64) {
            self.overlay.unpainted(paint);
        }

        pub fn set_suppressed(&self, suppressed: bool) {
            self.overlay.set_suppressed(suppressed);
        }

        pub fn place(&self, window: &Window, bounds: Bounds<Pixels>, visible: Bounds<Pixels>) {
            self.overlay.place(window, bounds, visible);
        }

        /// Takes the web view out of its window for good.
        pub fn detach(&self) {
            self.overlay.detach();
            // SAFETY: plain WebKit calls on the main thread.
            unsafe {
                self.web.stopLoading();
                self.web.setUIDelegate(None);
                // It holds its message handler, which would outlive the view.
                self.web
                    .configuration()
                    .userContentController()
                    .removeScriptMessageHandlerForName(&NSString::from_str(MESSAGES));
            }
        }

        pub fn load_url(&self, url: &str) {
            let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
                tracing::warn!(url, "not a URL");
                return;
            };
            // SAFETY: loading a request on the main thread.
            unsafe { self.web.loadRequest(&NSURLRequest::requestWithURL(&url)) };
        }

        pub fn load_html(&self, html: &str, base: Option<&Path>) {
            let base = base.map(|path| file_url(path, path.is_dir()));
            // SAFETY: loading a string on the main thread.
            unsafe {
                self.web
                    .loadHTMLString_baseURL(&NSString::from_str(html), base.as_deref())
            };
        }

        pub fn load_file(&self, path: &Path, read_access: Option<&Path>) {
            let read_access = read_access.or_else(|| path.parent()).unwrap_or(path);
            let url = file_url(path, false);
            let access = file_url(read_access, read_access.is_dir());
            // SAFETY: loading a file URL on the main thread.
            unsafe { self.web.loadFileURL_allowingReadAccessToURL(&url, &access) };
        }

        pub fn back(&self) {
            // SAFETY: navigation on the main thread.
            unsafe { self.web.goBack() };
        }

        pub fn forward(&self) {
            // SAFETY: navigation on the main thread.
            unsafe { self.web.goForward() };
        }

        pub fn reload(&self, bypass_cache: bool) {
            // SAFETY: navigation on the main thread.
            unsafe {
                if bypass_cache {
                    self.web.reloadFromOrigin();
                } else {
                    self.web.reload();
                }
            }
        }

        pub fn stop(&self) {
            // SAFETY: navigation on the main thread.
            unsafe { self.web.stopLoading() };
        }

        pub fn url(&self) -> Option<String> {
            // SAFETY: reading a property on the main thread.
            let url = unsafe { self.web.URL() }?;
            Some(url.absoluteString()?.to_string())
        }

        pub fn title(&self) -> Option<String> {
            // SAFETY: reading a property on the main thread.
            let title = unsafe { self.web.title() }?.to_string();
            (!title.is_empty()).then_some(title)
        }

        pub fn can_go_back(&self) -> bool {
            // SAFETY: reading a property on the main thread.
            unsafe { self.web.canGoBack() }
        }

        pub fn can_go_forward(&self) -> bool {
            // SAFETY: reading a property on the main thread.
            unsafe { self.web.canGoForward() }
        }

        pub fn is_loading(&self) -> bool {
            // SAFETY: reading a property on the main thread.
            unsafe { self.web.isLoading() }
        }

        pub fn find(&self, text: &str, backwards: bool, done: impl Fn(bool) + 'static) {
            let mtm = MainThreadMarker::from(&*self.web);
            let done = RcBlock::new(move |result: std::ptr::NonNull<WKFindResult>| {
                // SAFETY: WebKit passes a valid result for the call.
                done(unsafe { result.as_ref().matchFound() });
            });
            // SAFETY: WebKit calls the block once, on the main thread.
            unsafe {
                let config = WKFindConfiguration::new(mtm);
                config.setBackwards(backwards);
                config.setWraps(true);
                self.web.findString_withConfiguration_completionHandler(
                    &NSString::from_str(text),
                    Some(&config),
                    &done,
                );
            }
        }

        pub fn set_zoom(&self, zoom: f64) {
            // SAFETY: setting a property on the main thread.
            unsafe { self.web.setPageZoom(zoom) };
        }

        pub fn evaluate_js(&self, script: &str) {
            // SAFETY: no completion handler, so nothing outlives the call.
            unsafe {
                self.web
                    .evaluateJavaScript_completionHandler(&NSString::from_str(script), None)
            };
        }

        pub fn focus(&self) {
            self.overlay.focus();
        }

        pub fn blur(&self) {
            self.overlay.blur();
        }

        pub fn set_appearance(&self, dark: bool) {
            // SAFETY: the appearance names are AppKit constants.
            let name = unsafe {
                if dark {
                    NSAppearanceNameDarkAqua
                } else {
                    NSAppearanceNameAqua
                }
            };
            self.web
                .setAppearance(NSAppearance::appearanceNamed(name).as_deref());
        }

        pub fn set_background(&self, color: Rgba) {
            let color = NSColor::colorWithSRGBRed_green_blue_alpha(
                color.r.into(),
                color.g.into(),
                color.b.into(),
                color.a.into(),
            );
            // SAFETY: `drawsBackground` is WKWebView's private but long-standing
            // KVC key for a transparent web view; it takes a boolean NSNumber.
            unsafe {
                self.web.setUnderPageBackgroundColor(Some(&color));
                self.web.setValue_forKey(
                    Some(&NSNumber::new_bool(false)),
                    ns_string!("drawsBackground"),
                );
            }
        }
    }

    fn file_url(path: &Path, is_dir: bool) -> Retained<NSURL> {
        NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&path.to_string_lossy()), is_dir)
    }

    /// A rectangle in GPUI's window coordinates (points, y down).
    #[derive(Clone, Copy)]
    struct Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    impl Rect {
        fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
            Self {
                x,
                y,
                width,
                height,
            }
        }

        /// Rounds the edges to device pixels.
        fn snap(self, scale: f64) -> Self {
            let round = |v: f64| (v * scale).round() / scale;
            let (x, y) = (round(self.x), round(self.y));
            Self::new(
                x,
                y,
                round(self.x + self.width) - x,
                round(self.y + self.height) - y,
            )
        }

        /// The frame in `parent`'s view, which sits at `parent` in window
        /// coordinates. GPUI's view and plain NSViews aren't flipped: their
        /// origin is at the bottom left.
        fn frame_in(&self, parent: &Rect, flipped: bool) -> NSRect {
            let x = self.x - parent.x;
            let y = if flipped {
                self.y - parent.y
            } else {
                (parent.y + parent.height) - (self.y + self.height)
            };
            NSRect::new(
                NSPoint::new(x, y),
                NSSize::new(self.width.max(0.), self.height.max(0.)),
            )
        }
    }

    impl From<Bounds<Pixels>> for Rect {
        fn from(b: Bounds<Pixels>) -> Self {
            Self::new(
                b.origin.x.into(),
                b.origin.y.into(),
                b.size.width.into(),
                b.size.height.into(),
            )
        }
    }

    /// GPUI's NSView for the window.
    fn gpui_view(window: &Window) -> Option<&NSView> {
        MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
        else {
            return None;
        };
        // SAFETY: GPUI's AppKit handle points at its live NSView, which
        // outlives this borrow of the window, and we are on the main thread.
        Some(unsafe { handle.ns_view.cast::<NSView>().as_ref() })
    }
}

#[cfg(not(target_os = "macos"))]
mod stub {
    use std::path::Path;

    use gpui::{Bounds, Pixels, Rgba, Window};

    use super::WebViewOptions;

    /// No web view outside macOS; never constructed.
    pub enum Native {}

    impl Native {
        pub fn new(_: WebViewOptions, _: async_channel::Sender<String>) -> Option<Self> {
            None
        }
        pub fn painted(&self) -> u64 {
            match *self {}
        }
        pub fn unpainted(&self, _: u64) {
            match *self {}
        }
        pub fn set_suppressed(&self, _: bool) {
            match *self {}
        }
        pub fn place(&self, _: &Window, _: Bounds<Pixels>, _: Bounds<Pixels>) {
            match *self {}
        }
        pub fn detach(&self) {
            match *self {}
        }
        pub fn load_url(&self, _: &str) {
            match *self {}
        }
        pub fn load_html(&self, _: &str, _: Option<&Path>) {
            match *self {}
        }
        pub fn load_file(&self, _: &Path, _: Option<&Path>) {
            match *self {}
        }
        pub fn back(&self) {
            match *self {}
        }
        pub fn forward(&self) {
            match *self {}
        }
        pub fn reload(&self, _: bool) {
            match *self {}
        }
        pub fn stop(&self) {
            match *self {}
        }
        pub fn url(&self) -> Option<String> {
            match *self {}
        }
        pub fn title(&self) -> Option<String> {
            match *self {}
        }
        pub fn can_go_back(&self) -> bool {
            match *self {}
        }
        pub fn can_go_forward(&self) -> bool {
            match *self {}
        }
        pub fn is_loading(&self) -> bool {
            match *self {}
        }
        pub fn find(&self, _: &str, _: bool, _: impl Fn(bool) + 'static) {
            match *self {}
        }
        pub fn set_zoom(&self, _: f64) {
            match *self {}
        }
        pub fn evaluate_js(&self, _: &str) {
            match *self {}
        }
        pub fn focus(&self) {
            match *self {}
        }
        pub fn blur(&self) {
            match *self {}
        }
        pub fn set_appearance(&self, _: bool) {
            match *self {}
        }
        pub fn set_background(&self, _: Rgba) {
            match *self {}
        }
    }
}
