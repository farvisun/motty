//! Bundled fonts, user font folders, and choosing the terminal font family.

use std::borrow::Cow;

use gpui::App;
use motty_theme::{Config, Theme};

const BUNDLED: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/jetbrains-mono/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/jetbrains-mono/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/fonts/jetbrains-mono/JetBrainsMono-Italic.ttf"),
    include_bytes!("../assets/fonts/jetbrains-mono/JetBrainsMono-BoldItalic.ttf"),
];

/// The bundled family, and its files as a web page loads them: name,
/// bytes, CSS weight and whether it's italic.
pub const BUNDLED_FAMILY: &str = "JetBrains Mono";
pub const BUNDLED_FILES: [(&str, &[u8], u16, bool); 4] = [
    ("JetBrainsMono-Regular.ttf", BUNDLED[0], 400, false),
    ("JetBrainsMono-Bold.ttf", BUNDLED[1], 700, false),
    ("JetBrainsMono-Italic.ttf", BUNDLED[2], 400, true),
    ("JetBrainsMono-BoldItalic.ttf", BUNDLED[3], 700, true),
];

/// Registers bundled fonts plus any files in `~/.config/{motty,otty}/fonts`.
pub fn load(cx: &mut App) {
    let mut fonts: Vec<Cow<'static, [u8]>> = BUNDLED.iter().map(|b| Cow::Borrowed(*b)).collect();
    for dir in [
        motty_theme::config::motty_dir(),
        motty_theme::config::otty_dir(),
    ] {
        let Ok(entries) = std::fs::read_dir(dir.join("fonts")) else {
            continue;
        };
        for path in entries.flatten().map(|e| e.path()) {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            if matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc" | "otc"))
                && let Ok(bytes) = std::fs::read(&path)
            {
                fonts.push(Cow::Owned(bytes));
            }
        }
    }
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("failed to register fonts: {e:#}");
    }
}

/// First installed family among the config's `font-family` and the theme's
/// `font-mono` list; Menlo is always present on macOS.
pub fn terminal_family(config: &Config, theme: &Theme, cx: &App) -> String {
    let available = cx.text_system().all_font_names();
    config
        .font_family
        .iter()
        .chain(theme.token.font_mono.iter())
        .find(|name| available.iter().any(|a| a.eq_ignore_ascii_case(name)))
        .cloned()
        .unwrap_or_else(|| "Menlo".into())
}

/// First available family in a CSS-style `font-ui` list. The CSS
/// system-font keywords all mean the platform UI font.
pub fn ui_family(list: &[String], cx: &App) -> String {
    let available = cx.text_system().all_font_names();
    for name in list {
        match name.as_str() {
            "-apple-system" | "BlinkMacSystemFont" | "system-ui" | "SF Pro" | "SF Pro Text"
            | "sans-serif" => {
                return ".SystemUIFont".into();
            }
            other if available.iter().any(|a| a.eq_ignore_ascii_case(other)) => {
                return other.to_owned();
            }
            _ => {}
        }
    }
    ".SystemUIFont".into()
}
