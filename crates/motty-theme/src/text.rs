//! The text-rendering keys: faces for bold and italic, ligature reach,
//! cell height, blending, bidirectional text, contrast and the widths of
//! East-Asian-Ambiguous characters.

use std::ops::RangeInclusive;

/// `text-bold` / `text-italic`: how bold or italic cells find their face.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FaceMode {
    /// Drawn upright and at normal weight.
    Off,
    /// The family's own face, else the closest one the system finds.
    #[default]
    Auto,
    /// Only the primary family's own face.
    PrimaryOnly,
    /// The real face when there is one, else faked from the regular one.
    Synthetic,
}

impl FaceMode {
    pub fn parse(value: &toml::Value) -> Option<Self> {
        match (value.as_str(), value.as_bool()) {
            (Some("off"), _) | (_, Some(false)) => Some(Self::Off),
            (Some("auto"), _) | (_, Some(true)) => Some(Self::Auto),
            (Some("primary-only" | "primary_only"), _) => Some(Self::PrimaryOnly),
            (Some("synthetic"), _) => Some(Self::Synthetic),
            _ => None,
        }
    }
}

/// `font-blending`: how glyph edges are composited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontBlending {
    #[default]
    SrgbOver,
    MacosLike,
    Linear,
    Perceptual,
}

impl FontBlending {
    pub fn parse(value: &toml::Value) -> Option<Self> {
        match value.as_str()? {
            "srgb-over" | "srgb" | "default" => Some(Self::SrgbOver),
            "macos-like" | "native" => Some(Self::MacosLike),
            "linear" => Some(Self::Linear),
            "perceptual" => Some(Self::Perceptual),
            _ => None,
        }
    }
}

/// `adjust-cell-height`, in Ghostty's syntax.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CellHeight {
    /// `N` or `Npx`: points added to the font's natural height.
    Points(f32),
    /// `N%`: the natural height scaled by `1 + N/100`.
    Percent(f32),
}

impl CellHeight {
    pub fn parse(value: &toml::Value) -> Option<Self> {
        if let Some(n) = value.as_integer() {
            return Some(Self::Points(n as f32));
        }
        if let Some(n) = value.as_float() {
            return Some(Self::Points(n as f32));
        }
        let text = value.as_str()?.trim();
        let number = |s: &str| s.trim().parse::<f32>().ok().filter(|n| n.is_finite());
        if let Some(p) = text.strip_suffix('%') {
            return number(p).map(Self::Percent);
        }
        number(text.strip_suffix("px").unwrap_or(text)).map(Self::Points)
    }

    /// The natural height `natural` (points) adjusted; never below a third
    /// of it, so a large negative value can't collapse the rows.
    pub fn apply(self, natural: f32) -> f32 {
        let height = match self {
            Self::Points(p) => natural + p,
            Self::Percent(p) => natural * (1. + p / 100.),
        };
        height.max(natural / 3.)
    }
}

/// The Unicode blocks `widen-ambiguous` takes, by name.
pub const AMBIGUOUS_BLOCKS: [(&str, RangeInclusive<char>); 8] = [
    ("enclosed-alphanumerics", '\u{2460}'..='\u{24FF}'),
    ("number-forms", '\u{2150}'..='\u{218F}'),
    ("math-operators", '\u{2200}'..='\u{22FF}'),
    ("misc-technical", '\u{2300}'..='\u{23FF}'),
    ("misc-symbols", '\u{2600}'..='\u{26FF}'),
    ("dingbats", '\u{2700}'..='\u{27BF}'),
    ("arrows", '\u{2190}'..='\u{21FF}'),
    ("geometric-shapes", '\u{25A0}'..='\u{25FF}'),
];

/// The code points of a `widen-ambiguous` block; only its
/// East-Asian-Ambiguous ones are widened.
pub fn ambiguous_block(name: &str) -> Option<RangeInclusive<char>> {
    AMBIGUOUS_BLOCKS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, r)| r.clone())
}

/// Settings › Appearance › Text and the rendering knobs in Advanced.
#[derive(Clone, Debug, PartialEq)]
pub struct TextPrefs {
    /// `bidi`: reorder right-to-left text when drawing.
    pub bidi: bool,
    /// `minimum-contrast`: WCAG ratio text keeps against its background
    /// (1 leaves colors alone).
    pub minimum_contrast: f32,
    pub bold: FaceMode,
    pub italic: FaceMode,
    /// `text-underline`: draw SGR underlines (links are underlined anyway).
    pub underline: bool,
    /// `text-blink`: SGR 5/6 cells blink rather than stay steady.
    pub blink: bool,
    /// `font-ligatures = dlig` (the default): discretionary ligatures too.
    pub discretionary_ligatures: bool,
    /// `font-ligatures-alphabet`: ligatures inside letters and digits too.
    pub ligatures_alphabet: bool,
    /// `adjust-cell-height`; the theme's token when unset.
    pub cell_height: Option<CellHeight>,
    pub family_bold: Option<String>,
    pub family_italic: Option<String>,
    pub family_bold_italic: Option<String>,
    pub blending: FontBlending,
    /// `font-thicken`: darken stems whatever the blending.
    pub thicken: bool,
    /// `widen-ambiguous` block names, in file order.
    pub widen_ambiguous: Vec<String>,
    /// `strip-cjk-wrap-pad`: hide zsh's reverse-video pad before a wrapped
    /// wide character.
    pub strip_cjk_wrap_pad: bool,
    /// `arrow-box-drawing-join`: arrows and triangles meet box rules.
    pub arrow_join: bool,
}

impl Default for TextPrefs {
    fn default() -> Self {
        Self {
            bidi: true,
            minimum_contrast: 1.,
            bold: FaceMode::Auto,
            italic: FaceMode::Auto,
            underline: true,
            blink: false,
            discretionary_ligatures: true,
            ligatures_alphabet: false,
            cell_height: None,
            family_bold: None,
            family_italic: None,
            family_bold_italic: None,
            blending: FontBlending::SrgbOver,
            thicken: false,
            widen_ambiguous: Vec::new(),
            strip_cjk_wrap_pad: false,
            arrow_join: true,
        }
    }
}

impl TextPrefs {
    /// The code point ranges `widen-ambiguous` names.
    pub fn wide_ranges(&self) -> Vec<RangeInclusive<char>> {
        self.widen_ambiguous
            .iter()
            .filter_map(|n| ambiguous_block(n))
            .collect()
    }
}

/// `widen-ambiguous`: a list or a comma-separated string of block names;
/// unknown names come back as errors.
pub fn parse_blocks(value: &toml::Value) -> (Vec<String>, Vec<String>) {
    let names: Vec<String> = match value.as_array() {
        Some(list) => list
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_owned()))
            .collect(),
        None => value
            .as_str()
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_owned())
            .collect(),
    };
    let mut blocks = Vec::new();
    let mut unknown = Vec::new();
    for name in names.into_iter().filter(|n| !n.is_empty()) {
        if ambiguous_block(&name).is_none() {
            unknown.push(name);
        } else if !blocks.contains(&name) {
            blocks.push(name);
        }
    }
    (blocks, unknown)
}

#[cfg(test)]
mod tests {
    use toml::Value;

    use super::*;

    fn s(v: &str) -> Value {
        Value::String(v.into())
    }

    #[test]
    fn cell_height_syntax() {
        assert_eq!(
            CellHeight::parse(&Value::Integer(2)),
            Some(CellHeight::Points(2.))
        );
        assert_eq!(
            CellHeight::parse(&Value::Float(-1.5)),
            Some(CellHeight::Points(-1.5))
        );
        assert_eq!(CellHeight::parse(&s("3px")), Some(CellHeight::Points(3.)));
        assert_eq!(CellHeight::parse(&s("-2")), Some(CellHeight::Points(-2.)));
        assert_eq!(CellHeight::parse(&s("20%")), Some(CellHeight::Percent(20.)));
        assert_eq!(
            CellHeight::parse(&s("-10%")),
            Some(CellHeight::Percent(-10.))
        );
        assert_eq!(CellHeight::parse(&s("tall")), None);
        assert_eq!(CellHeight::parse(&s("%")), None);
        assert_eq!(CellHeight::Points(2.).apply(16.), 18.);
        assert_eq!(CellHeight::Percent(25.).apply(16.), 20.);
        assert_eq!(CellHeight::Percent(-10.).apply(20.), 18.);
        // Compacting stops at a third of the natural height.
        assert_eq!(CellHeight::Points(-100.).apply(15.), 5.);
    }

    #[test]
    fn widen_table() {
        let (blocks, unknown) = parse_blocks(&s("arrows, dingbats, nope, arrows"));
        assert_eq!(blocks, ["arrows", "dingbats"]);
        assert_eq!(unknown, ["nope"]);
        let list = Value::Array(vec![s("enclosed-alphanumerics")]);
        let prefs = TextPrefs {
            widen_ambiguous: parse_blocks(&list).0,
            ..Default::default()
        };
        assert_eq!(prefs.wide_ranges(), ['\u{2460}'..='\u{24FF}']);
        // Every block is a real Unicode block: 16-aligned, in order.
        for (name, range) in &AMBIGUOUS_BLOCKS {
            assert_eq!(*range.start() as u32 % 16, 0, "{name}");
            assert_eq!(*range.end() as u32 % 16, 15, "{name}");
            assert_eq!(ambiguous_block(name), Some(range.clone()));
        }
        assert!(ambiguous_block("misc-symbols").unwrap().contains(&'☆'));
        assert!(ambiguous_block("geometric-shapes").unwrap().contains(&'▶'));
    }

    #[test]
    fn modes() {
        assert_eq!(FaceMode::parse(&s("synthetic")), Some(FaceMode::Synthetic));
        assert_eq!(
            FaceMode::parse(&s("primary-only")),
            Some(FaceMode::PrimaryOnly)
        );
        assert_eq!(FaceMode::parse(&Value::Boolean(false)), Some(FaceMode::Off));
        assert_eq!(FaceMode::parse(&s("heavy")), None);
        assert_eq!(
            FontBlending::parse(&s("linear")),
            Some(FontBlending::Linear)
        );
        assert_eq!(FontBlending::parse(&s("gamma")), None);
    }
}
