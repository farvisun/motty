//! Links under the pointer: holding ⌘ underlines the URL or path there and
//! shows its target at the bottom left; ⌘-click opens it, ⌘⇧-click reveals
//! a path in Finder (or copies a URL).

use gpui::{
    ClipboardItem, Context, Modifiers, Pixels, Point, SharedString, Window, div, prelude::*, px,
    relative,
};
use motty_render::hsla;
use motty_term::links::{self, Link, Target};
use motty_theme::{Config, LinkClick, LinkShiftClick, Rgba, RightClick};

use super::TerminalView;

#[derive(Clone, Debug, Default)]
pub struct LinkOptions {
    pub detection: bool,
    pub click: LinkClick,
    pub shift_click: LinkShiftClick,
    /// Not a link option, but set with them: `right-click-action`.
    pub right_click: RightClick,
    /// Open With apps for paths, as (name, bundle id).
    pub open_with: Vec<(String, String)>,
}

impl LinkOptions {
    pub fn from_config(config: &Config) -> Self {
        Self {
            detection: config.link_detection,
            click: config.link_cmd_click,
            shift_click: config.link_cmd_shift_click,
            right_click: config.right_click,
            open_with: crate::workspace::open_with_apps(&config.open_with_apps),
        }
    }
}

impl TerminalView {
    pub fn set_link_options(&mut self, options: LinkOptions) {
        self.link_options = options;
    }

    /// Re-checks the link under the pointer, after it moved or ⌘ changed.
    pub(super) fn update_hovered_link(
        &mut self,
        position: Option<Point<Pixels>>,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) {
        if position.is_some() {
            self.pointer = position;
        }
        let link = self
            .pointer
            .filter(|_| modifiers.platform)
            .and_then(|pos| self.link_at(pos));
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
    }

    pub(super) fn link_at(&self, pos: Point<Pixels>) -> Option<Link> {
        let layout = self.layout.get()?;
        // Only over the grid itself.
        let (x, y) = (pos.x - layout.origin.x, pos.y - layout.origin.y);
        if x < px(0.)
            || y < px(0.)
            || x >= layout.cell_width * layout.cols as f32
            || y >= layout.line_height * layout.rows as f32
        {
            return None;
        }
        let (.., point, _) = self.hit(pos)?;
        let term = self.session.term.lock();
        let link = links::link_at(&*term, point, self.cwd.as_deref())?;
        (link.explicit || self.link_options.detection).then_some(link)
    }

    /// ⌘-click (or ⌘⇧-click) on the hovered link; false when there's none.
    pub(super) fn click_link(
        &mut self,
        modifiers: &Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(link) = self.hovered_link.clone().filter(|_| modifiers.platform) else {
            return false;
        };
        match (&link.target, modifiers.shift) {
            (Target::Url(url), true) => {
                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()))
            }
            (Target::Path { path, .. }, true) => match self.link_options.shift_click {
                LinkShiftClick::RevealFinder => cx.reveal_path(path),
                LinkShiftClick::OpenSystemDefault => cx.open_with_system(path),
            },
            (target, false) => match self.link_options.click {
                LinkClick::Open => match target {
                    Target::Url(url) if url.starts_with(crate::url_scheme::SCHEME) => {
                        open_motty_link(url.clone(), window, cx)
                    }
                    Target::Url(url) => cx.open_url(url),
                    Target::Path { path, .. } => cx.open_with_system(path),
                },
                LinkClick::Copy => {
                    cx.write_to_clipboard(ClipboardItem::new_string(target.display()))
                }
                LinkClick::Nothing => {}
            },
        }
        true
    }

    /// The hovered link's target in a dark label at the pane's bottom left,
    /// measured from Otty (28pt tall, 12.5pt in from the edges).
    pub(super) fn render_link_status(&self) -> Option<impl IntoElement> {
        let link = self.hovered_link.as_ref()?;
        Some(
            div()
                .absolute()
                .left(px(12.5))
                .bottom(px(12.5))
                .max_w(relative(0.9))
                .h(px(28.))
                .px(px(14.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .bg(hsla(Rgba::rgb(0x464645)))
                .font(self.style.font.clone())
                .text_size(self.style.font_size)
                .text_color(gpui::white())
                .whitespace_nowrap()
                .overflow_hidden()
                .child(SharedString::from(link.target.display())),
        )
    }
}

/// A `motty://` link in a pane opens here rather than through the system,
/// once this window's update is over (it may focus this very window).
pub(super) fn open_motty_link(url: String, window: &mut Window, cx: &mut Context<TerminalView>) {
    if matches!(
        crate::url_scheme::parse(&url),
        Some(crate::url_scheme::Link::Run { .. })
    ) {
        cx.defer_in(window, move |_, window, cx| {
            crate::url_scheme::open_clicked(&url, window, cx)
        });
    } else {
        cx.defer(move |cx| crate::url_scheme::open(&url, cx));
    }
}
