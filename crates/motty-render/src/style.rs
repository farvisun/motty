//! Everything the terminal element needs from the theme and config,
//! pre-converted to GPUI types once per theme change.

use gpui::{Font, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, Rgba as GpuiRgba, px};
use motty_term::alacritty_terminal::term::color::COUNT;
use motty_term::alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use motty_theme::{Rgba, Theme};

pub fn hsla(c: Rgba) -> Hsla {
    GpuiRgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
    .into()
}

fn rgb_to_rgba(c: Rgb) -> Rgba {
    Rgba {
        r: c.r as f32 / 255.,
        g: c.g as f32 / 255.,
        b: c.b as f32 / 255.,
        a: 1.,
    }
}

#[derive(Clone)]
pub struct TerminalStyle {
    pub font: Font,
    pub font_size: Pixels,
    /// Multiplier on the font's natural line height (`1 + adjust-cell-height`).
    pub line_height_scale: f32,
    /// Indexed like alacritty's `Colors`: 0..256 palette, then the named slots.
    pub palette: Box<[Rgba; COUNT]>,
    pub background: Rgba,
    pub cursor: Rgba,
    pub cursor_text: Option<Rgba>,
    pub cursor_bar: Rgba,
    pub cursor_underline: Rgba,
    pub selection_background: Rgba,
    pub selection_foreground: Option<Rgba>,
    pub padding: motty_theme::Edges,
    /// Popovers over the terminal (the find bar).
    pub panel_background: Rgba,
    pub dark: bool,
}

impl TerminalStyle {
    pub fn from_theme(
        theme: &Theme,
        font_family: Option<&str>,
        font_size: Option<f32>,
        ligatures: bool,
    ) -> Self {
        let family = font_family
            .map(str::to_owned)
            .or_else(|| {
                theme
                    .token
                    .font_mono
                    .iter()
                    .find(|f| !f.starts_with('-') && *f != "monospace")
                    .cloned()
            })
            .unwrap_or_else(|| "Menlo".into());
        let features = if ligatures {
            FontFeatures(std::sync::Arc::new(vec![
                ("calt".into(), 1),
                ("liga".into(), 1),
            ]))
        } else {
            FontFeatures::disable_ligatures()
        };
        let font = Font {
            family: family.into(),
            features,
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };

        let t = &theme.terminal;
        let mut palette = Box::new([Rgba::TRANSPARENT; COUNT]);
        palette[..16].copy_from_slice(&t.palette);
        // 6x6x6 color cube.
        for i in 0..216u32 {
            let level = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
            let (r, g, b) = (i / 36, (i / 6) % 6, i % 6);
            palette[16 + i as usize] = Rgba::rgb((level(r) << 16) | (level(g) << 8) | level(b));
        }
        for i in 0..24u32 {
            let v = 8 + i * 10;
            palette[232 + i as usize] = Rgba::rgb((v << 16) | (v << 8) | v);
        }
        // Default background is painted by the container, so cells with the
        // default background stay transparent (needed for vibrancy themes).
        let bg = t.background;
        let opaque_bg = if bg.a > 0.5 {
            bg
        } else {
            theme.panel.background.with_alpha(1.)
        };
        palette[NamedColor::Foreground as usize] = t.foreground;
        palette[NamedColor::Background as usize] = bg;
        palette[NamedColor::Cursor as usize] = theme.cursor.color;
        for i in 0..8 {
            palette[NamedColor::DimBlack as usize + i] =
                motty_theme::theme::mix(t.palette[i], opaque_bg, 0.34);
        }
        palette[NamedColor::BrightForeground as usize] = t.foreground;
        palette[NamedColor::DimForeground as usize] =
            motty_theme::theme::mix(t.foreground, opaque_bg, 0.34);

        Self {
            font,
            font_size: px(font_size.unwrap_or(theme.token.font_size)),
            line_height_scale: 1. + theme.token.adjust_cell_height,
            palette,
            background: bg,
            cursor: theme.cursor.block,
            cursor_text: theme.cursor.block_text,
            cursor_bar: theme.cursor.bar,
            cursor_underline: theme.cursor.underline,
            selection_background: t.selection_background,
            selection_foreground: t.selection_foreground,
            padding: theme.container.padding,
            panel_background: theme.panel.background.with_alpha(1.),
            dark: theme.meta.mode == motty_theme::Mode::Dark,
        }
    }

    /// Resolves a cell color, honoring runtime overrides (OSC 4/10/11).
    pub fn resolve(
        &self,
        color: Color,
        overrides: &motty_term::alacritty_terminal::term::color::Colors,
    ) -> Rgba {
        let index = match color {
            Color::Spec(rgb) => return rgb_to_rgba(rgb),
            Color::Named(n) => n as usize,
            Color::Indexed(i) => i as usize,
        };
        match overrides[index] {
            Some(rgb) => rgb_to_rgba(rgb),
            None => self.palette[index],
        }
    }

    pub fn default_background(&self) -> Rgba {
        self.background
    }
}
