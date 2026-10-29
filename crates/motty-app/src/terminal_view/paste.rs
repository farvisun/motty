//! What reaches the shell from the clipboard and from drops: file paths
//! (escaped), a pasted picture (saved to a file), and text, asked about
//! first when it could run something by itself (`clipboard-paste-protection`).

use std::path::PathBuf;

use gpui::{
    AnyElement, App, ClipboardEntry, ClipboardItem, Context, DragMoveEvent, ExternalPaths, Hsla,
    Image, PromptLevel, Window, div, prelude::*, px,
};
use motty_term::alacritty_terminal::term::TermMode;

use super::{TerminalView, shell_escape};
use crate::actions::{DropSplit, EditFiles};

/// Where a drag over the pane would land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DropZone {
    /// The paths are typed at the prompt.
    Center,
    /// A new pane on that side (`left`, `right`, `up`, `down`).
    Edge(&'static str),
}

/// How deep the edge targets reach into the pane, as a share of its size.
const EDGE: f32 = 0.22;

/// The zone under `x, y` in a `w`×`h` pane.
fn zone(x: f32, y: f32, w: f32, h: f32) -> DropZone {
    let edges = [
        (x / w, "left"),
        ((w - x) / w, "right"),
        (y / h, "up"),
        ((h - y) / h, "down"),
    ];
    let (share, side) = edges
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .expect("four edges");
    if share < EDGE {
        DropZone::Edge(side)
    } else {
        DropZone::Center
    }
}

/// Why a paste might do more than it shows; empty when it's harmless.
pub(super) fn paste_risks(text: &str) -> Vec<&'static str> {
    let mut risks = Vec::new();
    let body = text.trim_end_matches(['\n', '\r']);
    if body.contains(['\n', '\r']) {
        risks.push("Several lines: each runs as soon as it's pasted.");
    }
    if body.len() != text.len() {
        risks.push("Ends with a newline: the command runs on paste.");
    }
    if text
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | '`'))
        .any(|w| w == "sudo" || w == "su")
    {
        risks.push("Uses sudo or su: it may run with elevated privileges.");
    }
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        risks.push("Contains control characters: possible terminal escape injection.");
    }
    risks
}

/// The first lines of a paste, for the question.
fn preview(text: &str) -> String {
    const LINES: usize = 6;
    const WIDTH: usize = 80;
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = lines
        .iter()
        .take(LINES)
        .map(|l| {
            let clean: String = l
                .chars()
                .map(|c| if c.is_control() { '·' } else { c })
                .collect();
            match clean.char_indices().nth(WIDTH) {
                Some((i, _)) => format!("{}…", &clean[..i]),
                None => clean,
            }
        })
        .collect();
    if lines.len() > LINES {
        out.push(format!("… {} more lines", lines.len() - LINES));
    }
    out.join("\n")
}

/// Paths as one line of shell words.
fn escaped(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| shell_escape(&p.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

impl TerminalView {
    /// ⌘V: files as their paths, a picture as the path of a saved copy,
    /// text after the paste-protection check.
    pub(super) fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        if let Some(ClipboardEntry::String(s)) = item.entries().first() {
            let text = s.text().clone();
            return self.paste_checked(text, window, cx);
        }
        match clipboard_files(&item) {
            Some(text) if !text.is_empty() => self.paste_text(&text, false, window),
            Some(_) => window.play_system_bell(),
            None => {}
        }
    }

    /// Pastes `text`, first asking when it could run something before
    /// it's seen. Programs taking bracketed paste (with
    /// `clipboard-paste-bracketed-safe`) and full-screen ones aren't asked
    /// about.
    pub(super) fn paste_checked(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only || self.vi.is_some() {
            window.play_system_bell();
            return;
        }
        let mode = self.term_mode();
        let safe = !self.behavior.paste_protection
            || mode.contains(TermMode::ALT_SCREEN)
            || (self.behavior.paste_bracketed_safe && mode.contains(TermMode::BRACKETED_PASTE));
        let risks = if safe { Vec::new() } else { paste_risks(&text) };
        if risks.is_empty() {
            return self.paste_text(&text, false, window);
        }
        let detail = format!(
            "{}\n\n{}",
            risks
                .iter()
                .map(|r| format!("• {r}"))
                .collect::<Vec<_>>()
                .join("\n"),
            preview(&text)
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            "Paste this text?",
            Some(&detail),
            &["Paste Anyway", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update_in(cx, |this, window, _| this.paste_text(&text, false, window));
            }
        })
        .detach();
    }

    pub(super) fn drag_moved(
        &mut self,
        event: &DragMoveEvent<ExternalPaths>,
        cx: &mut Context<Self>,
    ) {
        let b = event.bounds;
        let p = event.event.position;
        let zone = b.contains(&p).then(|| {
            zone(
                f32::from(p.x - b.origin.x),
                f32::from(p.y - b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            )
        });
        if zone != self.drop_zone {
            self.drop_zone = zone;
            self.drop_folder = event.drag(cx).paths().iter().any(|p| p.is_dir());
            cx.notify();
        }
    }

    /// Dropped files: an edge opens a terminal there in the (first)
    /// folder, or the files in editor tabs; the middle types the paths.
    pub(super) fn dropped(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let zone = self.drop_zone.take().unwrap_or(DropZone::Center);
        let paths = paths.paths().to_vec();
        cx.notify();
        match zone {
            DropZone::Edge(side) => {
                let action: Box<dyn gpui::Action> = match paths.iter().find(|p| p.is_dir()) {
                    Some(folder) => Box::new(DropSplit {
                        pane: self.id,
                        side: side.to_owned(),
                        path: folder.clone(),
                    }),
                    None => Box::new(EditFiles { paths }),
                };
                window.dispatch_action(action, cx);
            }
            DropZone::Center => {
                window.focus(&self.focus_handle, cx);
                let text = escaped(&paths);
                self.paste_text(&format!("{text} "), false, window);
            }
        }
    }

    /// The drop targets, while files are dragged over the pane.
    pub(super) fn render_drop_zone(&self, cx: &App) -> Option<AnyElement> {
        let zone = self.drop_zone.filter(|_| cx.has_active_drag())?;
        let folder = self.drop_folder;
        // Otty's green for a terminal, blue for a view.
        let color: Hsla = gpui::rgb(if folder { 0x34c759 } else { 0x0a84ff }).into();
        let (label, side) = match zone {
            DropZone::Center => ("Insert Path", None),
            DropZone::Edge(side) if folder => ("New Terminal Here", Some(side)),
            DropZone::Edge(side) => ("Open in Editor", Some(side)),
        };
        let share = gpui::relative(EDGE);
        let mut target = div()
            .absolute()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .border_2()
            .border_color(color.opacity(0.8))
            .bg(color.opacity(0.14));
        target = match side {
            None => target.inset(px(6.)),
            Some("left") => target.top(px(6.)).bottom(px(6.)).left(px(6.)).w(share),
            Some("right") => target.top(px(6.)).bottom(px(6.)).right(px(6.)).w(share),
            Some("up") => target.left(px(6.)).right(px(6.)).top(px(6.)).h(share),
            Some(_) => target.left(px(6.)).right(px(6.)).bottom(px(6.)).h(share),
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    target.child(
                        div()
                            .px(px(10.))
                            .py(px(4.))
                            .rounded(px(5.))
                            .bg(color)
                            .text_color(gpui::white())
                            .text_size(px(12.))
                            .child(label),
                    ),
                )
                .into_any_element(),
        )
    }
}

/// Files on the clipboard, or a picture (saved to a file), as shell
/// words; empty when the picture couldn't be saved, `None` for text.
pub(crate) fn clipboard_files(item: &ClipboardItem) -> Option<String> {
    match item.entries().first()? {
        ClipboardEntry::ExternalPaths(paths) => Some(escaped(paths.paths())),
        ClipboardEntry::Image(image) => Some(match save_image(image) {
            Ok(path) => escaped(&[path]),
            Err(e) => {
                tracing::warn!("couldn't save the pasted image: {e}");
                String::new()
            }
        }),
        _ => None,
    }
}

/// A picture with no file behind it, written where the shell can read it.
fn save_image(image: &Image) -> std::io::Result<PathBuf> {
    let ext = match image.format {
        gpui::ImageFormat::Png => "png",
        gpui::ImageFormat::Jpeg => "jpg",
        gpui::ImageFormat::Webp => "webp",
        gpui::ImageFormat::Gif => "gif",
        gpui::ImageFormat::Svg => "svg",
        gpui::ImageFormat::Bmp => "bmp",
        gpui::ImageFormat::Tiff => "tiff",
        _ => "img",
    };
    let dir = std::env::temp_dir().join("motty-paste");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{:016x}.{ext}", image.id));
    std::fs::write(&path, &image.bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risks() {
        assert!(paste_risks("ls -la").is_empty());
        assert!(paste_risks("echo sudoku").is_empty());
        assert_eq!(paste_risks("ls\n").len(), 1);
        assert_eq!(paste_risks("cd /\nrm -rf x").len(), 1);
        assert_eq!(paste_risks("cd /\nrm -rf x\n").len(), 2);
        assert!(paste_risks("true && sudo rm x")[0].starts_with("Uses sudo"));
        assert!(paste_risks("echo \u{1b}[31m")[0].starts_with("Contains control"));
    }

    #[test]
    fn zones() {
        assert_eq!(zone(50., 50., 100., 100.), DropZone::Center);
        assert_eq!(zone(5., 50., 100., 100.), DropZone::Edge("left"));
        assert_eq!(zone(50., 95., 100., 100.), DropZone::Edge("down"));
        assert_eq!(zone(98., 2., 100., 100.), DropZone::Edge("right"));
    }

    #[test]
    fn previews() {
        let long = (1..=9)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(preview(&long).ends_with("… 3 more lines"));
        assert_eq!(preview("a\u{7}b"), "a·b");
    }
}
