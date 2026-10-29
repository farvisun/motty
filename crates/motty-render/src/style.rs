//! Everything the terminal element needs from the theme and config,
//! pre-converted to GPUI types once per theme change.

use gpui::{Font, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, Rgba as GpuiRgba, px};
use motty_term::alacritty_terminal::term::color::COUNT;
use motty_term::alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use motty_theme::{CellHeight, Rgba, TextPrefs, Theme};

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
    /// Regular, bold, italic and bold italic (indexed by `bold | italic << 1`).
    pub faces: [Font; 4],
    /// The same faces with ligatures off, for letters and digits when
    /// ligatures only form in symbol runs; `None` when that changes nothing.
    pub plain_faces: Option<[Font; 4]>,
    pub font_size: Pixels,
    /// `adjust-cell-height` (config, else the theme's); `None` keeps the
    /// font's natural line height.
    pub cell_height: Option<CellHeight>,
    /// How text is drawn: bidi, contrast, faces, blinking, joins.
    pub text: TextPrefs,
    ligatures: bool,
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
        let font = Font {
            family: family.into(),
            features: ligature_features(ligatures, true),
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

        let token = theme.token.adjust_cell_height;
        let mut style = Self {
            faces: faces(&font, &TextPrefs::default()),
            plain_faces: None,
            font,
            font_size: px(font_size.unwrap_or(theme.token.font_size)),
            cell_height: (token != 0.).then_some(CellHeight::Percent(token * 100.)),
            text: TextPrefs::default(),
            ligatures,
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
        };
        style.set_text(TextPrefs::default());
        style
    }

    /// Applies the config's text-rendering keys.
    pub fn with_text(mut self, text: TextPrefs) -> Self {
        self.set_text(text);
        self
    }

    fn set_text(&mut self, text: TextPrefs) {
        if text.cell_height.is_some() {
            self.cell_height = text.cell_height;
        }
        self.font.features = ligature_features(self.ligatures, text.discretionary_ligatures);
        self.faces = faces(&self.font, &text);
        self.plain_faces = (self.ligatures && !text.ligatures_alphabet).then(|| {
            let mut plain = self.font.clone();
            plain.features = no_ligatures();
            faces(&plain, &text)
        });
        self.text = text;
    }

    /// The face for a cell's bold and italic flags.
    pub fn face(&self, bold: bool, italic: bool, plain: bool) -> &Font {
        let faces = match &self.plain_faces {
            Some(p) if plain => p,
            _ => &self.faces,
        };
        &faces[bold as usize | (italic as usize) << 1]
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

/// `calt` and `liga` (`dlig` too at the discretionary level), or none.
fn ligature_features(on: bool, discretionary: bool) -> FontFeatures {
    if !on {
        return FontFeatures::disable_ligatures();
    }
    let mut tags = vec![("calt".into(), 1), ("liga".into(), 1)];
    if discretionary {
        tags.push(("dlig".into(), 1));
    }
    FontFeatures(std::sync::Arc::new(tags))
}

fn no_ligatures() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![
        ("calt".into(), 0),
        ("liga".into(), 0),
        ("dlig".into(), 0),
    ]))
}

/// Regular, bold, italic and bold italic of `font`, with the families
/// `font-family-bold` and friends name.
fn faces(font: &Font, text: &TextPrefs) -> [Font; 4] {
    let face = |bold: bool, italic: bool, family: Option<&String>| {
        let mut f = font.clone();
        if bold {
            f.weight = FontWeight::BOLD;
        }
        if italic {
            f.style = FontStyle::Italic;
        }
        if let Some(family) = family {
            f.family = family.clone().into();
        }
        f
    };
    [
        face(false, false, None),
        face(true, false, text.family_bold.as_ref()),
        face(false, true, text.family_italic.as_ref()),
        face(
            true,
            true,
            text.family_bold_italic
                .as_ref()
                .or(text.family_bold.as_ref()),
        ),
    ]
}
