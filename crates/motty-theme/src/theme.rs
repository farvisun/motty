//! `.ottytheme` model. Only `[meta]` and `[terminal]` are required; every
//! other value is derived from the terminal colors when omitted, following
//! the defaults documented in the reference Nord theme.

use crate::css::{self, Border, Edges, Rgba, Shadow};
use toml::{Table, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Material {
    #[default]
    None,
    VibrancyThin,
    VibrancyRegular,
    VibrancyThick,
    Glass,
}

impl Material {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => Self::None,
            "vibrancy-thin" => Self::VibrancyThin,
            "vibrancy-regular" | "vibrancy" => Self::VibrancyRegular,
            "vibrancy-thick" => Self::VibrancyThick,
            "glass" | "acrylic" => Self::Glass,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Meta {
    pub name: String,
    pub author: Option<String>,
    pub mode: Mode,
    pub description: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TerminalColors {
    pub foreground: Rgba,
    /// Transparent when the theme says `none`.
    pub background: Rgba,
    pub palette: [Rgba; 16],
    pub selection_background: Rgba,
    pub selection_foreground: Option<Rgba>,
    pub font_blending: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CursorColors {
    pub color: Rgba,
    /// Text drawn under a block cursor; `None` uses the cell background.
    pub text: Option<Rgba>,
    pub block: Rgba,
    pub block_text: Option<Rgba>,
    pub bar: Rgba,
    pub underline: Rgba,
    /// Whether the theme set the base color and text itself; the config's
    /// `cursor-color` and `cursor-text` only fill in when it didn't.
    pub color_themed: bool,
    pub text_themed: bool,
}

#[derive(Clone, Debug)]
pub struct Tokens {
    pub foreground: Rgba,
    pub secondary: Rgba,
    pub tertiary: Rgba,
    pub accent: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub danger: Rgba,
    pub hover_bg: Rgba,
    pub active_bg: Rgba,
    pub radius: f32,
    pub shadow: Vec<Shadow>,
    pub font_ui: Vec<String>,
    pub font_mono: Vec<String>,
    pub font_size: f32,
    /// Extra line height as a fraction of the font's natural cell height.
    pub adjust_cell_height: f32,
}

#[derive(Clone, Debug)]
pub struct Panel {
    pub background: Rgba,
    pub surface: Rgba,
    pub border: Rgba,
}

#[derive(Clone, Debug)]
pub struct WindowStyle {
    pub background: Rgba,
    pub material: Material,
    pub radius: f32,
    pub unfocused_opacity: f32,
}

#[derive(Clone, Debug)]
pub struct Titlebar {
    pub background: Option<Rgba>,
    pub foreground: Rgba,
    pub unfocused_foreground: Rgba,
    pub font_size: f32,
    pub font_weight: f32,
    /// Explicit band height; the sidebar layout reserves the native 28pt
    /// titlebar when unset.
    pub height: Option<f32>,
    pub material: Option<Material>,
}

#[derive(Clone, Debug)]
pub struct SidePanel {
    pub background: Rgba,
    pub border: Option<Border>,
    pub width: f32,
    pub material: Material,
    pub padding: Edges,
}

#[derive(Clone, Debug)]
pub struct TabStyle {
    pub background: Rgba,
    pub foreground: Rgba,
    pub radius: f32,
    pub font_size: f32,
    pub font_weight: f32,
    pub padding: Edges,
    pub height: Option<f32>,
    pub indicator: Rgba,
    /// `indicator = "bar"`: a 6×16 block in the indicator color at the
    /// start of the title.
    pub indicator_bar: bool,
    pub border: Option<Border>,
    pub shadow: Vec<Shadow>,
}

#[derive(Clone, Debug)]
pub struct TabStates {
    pub normal: TabStyle,
    pub hover: TabStyle,
    pub active: TabStyle,
    pub active_unfocused: TabStyle,
}

#[derive(Clone, Debug)]
pub struct TabBar {
    pub background: Option<Rgba>,
    pub height: f32,
    pub padding: Edges,
    pub border_bottom: Option<Border>,
    pub material: Option<Material>,
    pub separator: bool,
    pub tabs: TabStates,
}

#[derive(Clone, Debug)]
pub struct Container {
    pub background: Rgba,
    pub radius: f32,
    pub shadow: Vec<Shadow>,
    pub margin: Edges,
    pub margin_tabs_top: Edges,
    pub margin_tabs_bottom: Edges,
    pub padding: Edges,
    pub border: Option<Border>,
}

#[derive(Clone, Debug)]
pub struct Divider {
    pub color: Rgba,
    pub width: f32,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub meta: Meta,
    pub terminal: TerminalColors,
    pub cursor: CursorColors,
    pub token: Tokens,
    pub panel: Panel,
    pub window: WindowStyle,
    pub titlebar: Titlebar,
    pub sidebar: SidePanel,
    pub details_panel: SidePanel,
    pub tabs: TabStates,
    pub tab_bar: TabBar,
    pub container: Container,
    pub divider: Divider,
}

impl Theme {
    /// Resolves a border whose color is `auto` to the panel border color.
    pub fn border_color(&self, b: &Border) -> Rgba {
        b.color.unwrap_or(self.panel.border)
    }
}

/// Black (light mode) or white (dark mode) text at the given alpha, like
/// AppKit's label colors.
fn overlay_text(dark: bool, light_alpha: f32, dark_alpha: f32) -> Rgba {
    if dark {
        Rgba {
            r: 1.,
            g: 1.,
            b: 1.,
            a: dark_alpha,
        }
    } else {
        Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: light_alpha,
        }
    }
}

/// Mixes `a` toward `b` by `t` (0 = a, 1 = b).
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

pub fn luminance(c: Rgba) -> f32 {
    let lin = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// One theme section. Invalid values are reported through `warnings` and
/// treated as absent, so a typo never prevents a theme from loading.
struct Section<'a> {
    name: String,
    table: Option<&'a Table>,
    warnings: &'a std::cell::RefCell<Vec<String>>,
}

impl<'a> Section<'a> {
    fn get(&self, key: &str) -> Option<&'a Value> {
        self.table.and_then(|t| t.get(key))
    }

    fn warn(&self, key: &str, msg: impl std::fmt::Display) {
        self.warnings
            .borrow_mut()
            .push(format!("[{}] {key}: {msg}", self.name));
    }

    fn str(&self, key: &str) -> Option<&'a str> {
        match self.get(key)? {
            Value::String(s) => Some(s),
            other => {
                self.warn(key, format!("expected string, got {}", other.type_str()));
                None
            }
        }
    }

    fn num(&self, key: &str) -> Option<f32> {
        match self.get(key)? {
            Value::Integer(i) => Some(*i as f32),
            Value::Float(f) => Some(*f as f32),
            Value::String(s) => match s.trim_end_matches("px").parse() {
                Ok(v) => Some(v),
                Err(_) => {
                    self.warn(key, format!("expected number, got {s:?}"));
                    None
                }
            },
            other => {
                self.warn(key, format!("expected number, got {}", other.type_str()));
                None
            }
        }
    }

    fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::as_bool)
    }

    fn color(&self, key: &str) -> Option<Rgba> {
        let s = self.str(key)?;
        css::parse_color(s).map_err(|e| self.warn(key, e)).ok()
    }

    fn shadows(&self, key: &str) -> Option<Vec<Shadow>> {
        let s = self.str(key)?;
        css::parse_shadows(s).map_err(|e| self.warn(key, e)).ok()
    }

    fn border(&self, key: &str) -> Option<Option<Border>> {
        let s = self.str(key)?;
        css::parse_border(s).map_err(|e| self.warn(key, e)).ok()
    }

    fn edges(&self, key: &str) -> Option<Edges> {
        match self.get(key)? {
            Value::Array(items) => {
                let nums: Option<Vec<f32>> = items
                    .iter()
                    .map(|v| {
                        v.as_float()
                            .map(|f| f as f32)
                            .or_else(|| v.as_integer().map(|i| i as f32))
                    })
                    .collect();
                match nums.map(|n| Edges::from_slice(&n)) {
                    Some(Ok(e)) => Some(e),
                    _ => {
                        self.warn(key, "expected 1-4 numbers");
                        None
                    }
                }
            }
            _ => self.num(key).map(Edges::all),
        }
    }

    fn strings(&self, key: &str) -> Option<Vec<String>> {
        match self.get(key)? {
            Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
            ),
            Value::String(s) => Some(
                s.split(',')
                    .map(|p| p.trim().trim_matches('"').to_owned())
                    .collect(),
            ),
            _ => None,
        }
    }

    fn material(&self, key: &str) -> Option<Material> {
        let s = self.str(key)?;
        Material::parse(s).or_else(|| {
            self.warn(key, format!("unknown material {s:?}"));
            None
        })
    }

    /// `"20%"` or a plain fraction.
    fn percent(&self, key: &str) -> Option<f32> {
        match self.get(key)? {
            Value::String(s) => match s.strip_suffix('%').map(str::parse::<f32>) {
                Some(Ok(v)) => Some(v / 100.),
                _ => {
                    self.warn(key, format!("expected percentage, got {s:?}"));
                    None
                }
            },
            _ => self.num(key),
        }
    }
}

pub struct ParsedTheme {
    pub theme: Theme,
    pub warnings: Vec<String>,
}

pub fn parse(source: &str) -> anyhow::Result<ParsedTheme> {
    let root: Table = toml::from_str(source)?;
    let warnings = std::cell::RefCell::new(Vec::new());
    let theme = resolve(&root, &warnings)?;
    Ok(ParsedTheme {
        theme,
        warnings: warnings.into_inner(),
    })
}

/// Looks up a dotted section path such as `tab.active.unfocused`.
fn section<'a>(
    root: &'a Table,
    path: &str,
    warnings: &'a std::cell::RefCell<Vec<String>>,
) -> Section<'a> {
    let mut table = Some(root);
    for part in path.split('.') {
        table = table.and_then(|t| t.get(part)).and_then(Value::as_table);
    }
    Section {
        name: path.to_owned(),
        table,
        warnings,
    }
}

fn resolve(root: &Table, warnings: &std::cell::RefCell<Vec<String>>) -> anyhow::Result<Theme> {
    let s = |path: &str| section(root, path, warnings);

    let term = s("terminal");
    let foreground = term
        .color("foreground")
        .ok_or_else(|| anyhow::anyhow!("[terminal] foreground is required"))?;
    let background = term
        .color("background")
        .ok_or_else(|| anyhow::anyhow!("[terminal] background is required"))?;
    let palette = match term.get("palette") {
        Some(Value::Array(items)) if items.len() == 16 => {
            let mut out = [Rgba::TRANSPARENT; 16];
            for (i, item) in items.iter().enumerate() {
                out[i] = item
                    .as_str()
                    .and_then(|c| css::parse_color(c).ok())
                    .ok_or_else(|| anyhow::anyhow!("[terminal] palette[{i}] is not a color"))?;
            }
            out
        }
        _ => anyhow::bail!("[terminal] palette must have exactly 16 colors"),
    };

    let meta_s = s("meta");
    // With a transparent background, mode can only come from the foreground.
    let inferred_mode = if background.a > 0.5 {
        if luminance(background) < 0.4 {
            Mode::Dark
        } else {
            Mode::Light
        }
    } else if luminance(foreground) > 0.4 {
        Mode::Dark
    } else {
        Mode::Light
    };
    let mode = match meta_s.str("mode") {
        Some("dark") => Mode::Dark,
        Some("light") => Mode::Light,
        _ => inferred_mode,
    };
    let dark = mode == Mode::Dark;
    let meta = Meta {
        name: meta_s.str("name").unwrap_or("Untitled").to_owned(),
        author: meta_s.str("author").map(str::to_owned),
        mode,
        description: meta_s.str("description").map(str::to_owned),
    };

    // Opaque stand-in for derivations when the terminal is transparent.
    let base_bg = if background.a > 0.5 {
        background.with_alpha(1.)
    } else if dark {
        Rgba::rgb(0x1e1e1e)
    } else {
        Rgba::rgb(0xf5f5f5)
    };

    let tok = s("token");
    let tok_fg = tok.color("foreground").unwrap_or(foreground);
    let accent = tok.color("accent").unwrap_or(palette[4]);
    let overlay = |a: f32| {
        if dark {
            Rgba {
                r: 1.,
                g: 1.,
                b: 1.,
                a,
            }
        } else {
            Rgba {
                r: 0.,
                g: 0.,
                b: 0.,
                a,
            }
        }
    };
    let token = Tokens {
        foreground: tok_fg,
        secondary: tok
            .color("secondary")
            .unwrap_or_else(|| mix(tok_fg, base_bg, 0.2)),
        tertiary: tok
            .color("tertiary")
            .unwrap_or_else(|| mix(tok_fg, base_bg, 0.55)),
        accent,
        success: tok.color("success").unwrap_or(palette[2]),
        warning: tok.color("warning").unwrap_or(palette[3]),
        danger: tok.color("danger").unwrap_or(palette[1]),
        hover_bg: tok
            .color("hover-bg")
            .unwrap_or(overlay(if dark { 0.06 } else { 0.04 })),
        active_bg: tok.color("active-bg").unwrap_or(accent.with_alpha(0.18)),
        radius: tok.num("radius").unwrap_or(6.),
        shadow: tok.shadows("shadow").unwrap_or_else(|| {
            css::parse_shadows(if dark {
                "0 8px 24px rgba(0,0,0,0.35)"
            } else {
                "0 8px 24px rgba(0,0,0,0.12)"
            })
            .unwrap()
        }),
        font_ui: tok
            .strings("font-ui")
            .unwrap_or_else(|| vec!["-apple-system".into(), "SF Pro".into(), "sans-serif".into()]),
        font_mono: tok.strings("font-mono").unwrap_or_else(|| {
            vec![
                "JetBrains Mono".into(),
                "SF Mono".into(),
                "Menlo".into(),
                "monospace".into(),
            ]
        }),
        font_size: tok.num("font-size").unwrap_or(13.),
        adjust_cell_height: tok.percent("adjust-cell-height").unwrap_or(0.),
    };

    let terminal = TerminalColors {
        foreground,
        background,
        palette,
        selection_background: term
            .color("selection-background")
            .unwrap_or_else(|| accent.with_alpha(0.35)),
        selection_foreground: term.color("selection-foreground"),
        font_blending: term.str("font-blending").map(str::to_owned),
    };

    let cur = s("cursor");
    let cursor_color = cur
        .color("color")
        .or_else(|| term.color("cursor"))
        .unwrap_or(foreground);
    let cursor_text = cur.color("text").or_else(|| term.color("cursor-text"));
    let cursor = CursorColors {
        color: cursor_color,
        text: cursor_text,
        block: s("cursor.block").color("color").unwrap_or(cursor_color),
        block_text: s("cursor.block").color("text").or(cursor_text),
        bar: s("cursor.bar").color("color").unwrap_or(cursor_color),
        underline: s("cursor.underline").color("color").unwrap_or(cursor_color),
        color_themed: cur
            .color("color")
            .or_else(|| term.color("cursor"))
            .is_some(),
        text_themed: cursor_text.is_some(),
    };

    let pan = s("panel");
    let panel = Panel {
        // Measured from Otty's minimal themes (One Light, Dracula): chrome
        // sits a step off the terminal, darker in light mode, lighter in dark.
        background: pan.color("background").unwrap_or_else(|| {
            if dark {
                mix(base_bg, Rgba::rgb(0xffffff), 0.05)
            } else {
                mix(base_bg, Rgba::rgb(0x000000), 0.03)
            }
        }),
        surface: pan
            .color("surface")
            .unwrap_or_else(|| mix(base_bg, tok_fg, if dark { 0.06 } else { 0.03 })),
        border: pan
            .color("border")
            .unwrap_or(overlay(if dark { 0.06 } else { 0.08 })),
    };

    let win = s("window");
    // A section with a material and no background of its own is see-through:
    // the material is its fill.
    let window_material = win.material("material").unwrap_or_default();
    let window = WindowStyle {
        background: win
            .color("background")
            .unwrap_or(if window_material == Material::None {
                background
            } else {
                Rgba::TRANSPARENT
            }),
        material: window_material,
        radius: win.num("radius").unwrap_or(12.),
        unfocused_opacity: s("window.unfocused").num("opacity").unwrap_or(1.),
    };

    let tb = s("titlebar");
    let titlebar = Titlebar {
        background: tb.color("background"),
        // Unthemed titles use the native secondary/tertiary label colors.
        foreground: tb
            .color("foreground")
            .unwrap_or(overlay_text(dark, 0.5, 0.55)),
        unfocused_foreground: s("titlebar.unfocused")
            .color("foreground")
            .unwrap_or(overlay_text(dark, 0.26, 0.25)),
        font_size: tb.num("font-size").unwrap_or(13.),
        font_weight: tb.num("font-weight").unwrap_or(500.),
        height: tb.num("height"),
        material: tb.material("material"),
    };

    let sb = s("sidebar");
    let default_side_border = Some(Border {
        width: 1.,
        color: None,
    });
    let side = |sec: &Section, base: Option<&SidePanel>, default_width: f32| -> SidePanel {
        // Otty insets the sidebar's cards and header 8pt unless the theme
        // says otherwise (April and Ayu Light run cards edge to edge with 0).
        let mut padding = sec
            .edges("padding")
            .or(base.map(|b| b.padding))
            .unwrap_or(Edges::all(8.));
        if let Some(r) = sec.num("padding-right") {
            padding.right = r;
        }
        let material = sec
            .material("material")
            .or(base.map(|b| b.material))
            .unwrap_or_default();
        // In a window with a material, a plain side panel takes the terminal
        // background, not the panel color (Otty's Acrylic Dark: the sidebar
        // is #202020 under a #2C2C2C panel; the only theme that tells).
        let unset_background = match material {
            Material::None if window_material != Material::None => background,
            Material::None => panel.background,
            _ => Rgba::TRANSPARENT,
        };
        SidePanel {
            background: sec
                .color("background")
                .or(base.map(|b| b.background))
                .unwrap_or(unset_background),
            border: sec
                .border("border-right")
                .or_else(|| base.map(|b| b.border.clone()))
                .unwrap_or(default_side_border.clone()),
            width: sec
                .num("width")
                .or(base.map(|b| b.width))
                .unwrap_or(default_width),
            material,
            padding,
        }
    };
    let sidebar_base = side(&sb, None, 220.);
    let sidebar = side(&s("tabs-panel"), Some(&sidebar_base), 220.);
    let mut details_panel = side(&s("details-panel"), Some(&sidebar_base), 320.);
    if s("details-panel").num("width").is_none() {
        details_panel.width = 320.;
    }

    let tab_s = s("tab");
    let normal = TabStyle {
        background: tab_s.color("background").unwrap_or(Rgba::TRANSPARENT),
        foreground: tab_s.color("foreground").unwrap_or(token.secondary),
        radius: tab_s.num("radius").unwrap_or(token.radius),
        // Otty draws sidebar tab titles a point below the 13pt UI size.
        font_size: tab_s.num("font-size").unwrap_or(12.),
        font_weight: tab_s.num("font-weight").unwrap_or(400.),
        padding: tab_s.edges("padding").unwrap_or(Edges::all(10.)),
        height: tab_s.num("height"),
        indicator: tab_s.color("indicator-color").unwrap_or(token.tertiary),
        indicator_bar: tab_s.str("indicator") == Some("bar"),
        border: tab_s.border("border").flatten(),
        shadow: tab_s.shadows("shadow").unwrap_or_default(),
    };
    let tab_defaults = TabStates {
        hover: TabStyle {
            background: token.hover_bg,
            ..normal.clone()
        },
        active: TabStyle {
            // A translucent step over the sidebar (or its material), as Otty
            // draws it when a theme doesn't say.
            background: overlay(if dark { 0.08 } else { 0.06 }),
            foreground: token.foreground,
            font_weight: 500.,
            indicator: accent,
            // No shadow unless the theme asks (Otty's glass themes show none).
            ..normal.clone()
        },
        active_unfocused: normal.clone(),
        normal,
    };
    let tabs = tab_states(&s, "tab", &tab_defaults);

    let bar = s("tab-bar");
    let bar_tab_defaults = TabStates {
        normal: TabStyle {
            radius: 8.,
            height: Some(28.),
            ..tabs.normal.clone()
        },
        hover: TabStyle {
            radius: 8.,
            height: Some(28.),
            ..tabs.hover.clone()
        },
        active: TabStyle {
            radius: 8.,
            height: Some(28.),
            ..tabs.active.clone()
        },
        active_unfocused: TabStyle {
            radius: 8.,
            height: Some(28.),
            ..tabs.active_unfocused.clone()
        },
    };
    let tab_bar = TabBar {
        background: bar.color("background"),
        height: bar.num("height").unwrap_or(36.),
        padding: bar.edges("padding").unwrap_or(Edges::all(8.)),
        // Otty's Night and Paper bars show no line unless the theme draws one.
        border_bottom: bar.border("border-bottom").flatten(),
        material: bar.material("material"),
        separator: bar.bool("separator").unwrap_or(false),
        tabs: tab_states(&s, "tab-bar.tab", &bar_tab_defaults),
    };

    let con = s("container");
    let margin = con.edges("margin").unwrap_or_default();
    let container = Container {
        background: con.color("background").unwrap_or(background),
        radius: con.num("radius").unwrap_or(0.),
        shadow: con.shadows("shadow").unwrap_or_default(),
        margin,
        margin_tabs_top: s("container.tabs-top").edges("margin").unwrap_or(margin),
        margin_tabs_bottom: s("container.tabs-bottom").edges("margin").unwrap_or(margin),
        padding: con.edges("padding").unwrap_or(Edges::all(8.)),
        border: con.border("border").flatten(),
    };

    let div = s("divider");
    let divider = Divider {
        color: div.color("color").unwrap_or(overlay(0.1)),
        width: div.num("width").unwrap_or(1.),
    };

    Ok(Theme {
        meta,
        terminal,
        cursor,
        token,
        panel,
        window,
        titlebar,
        sidebar,
        details_panel,
        tabs,
        tab_bar,
        container,
        divider,
    })
}

impl TabStyle {
    /// Applies the keys present in `sec` on top of `self`.
    fn overlay(&self, sec: &Section) -> TabStyle {
        TabStyle {
            background: sec.color("background").unwrap_or(self.background),
            foreground: sec.color("foreground").unwrap_or(self.foreground),
            radius: sec.num("radius").unwrap_or(self.radius),
            font_size: sec.num("font-size").unwrap_or(self.font_size),
            font_weight: sec.num("font-weight").unwrap_or(self.font_weight),
            padding: sec.edges("padding").unwrap_or(self.padding),
            height: sec.num("height").or(self.height),
            indicator: sec.color("indicator-color").unwrap_or(self.indicator),
            indicator_bar: sec
                .str("indicator")
                .map_or(self.indicator_bar, |i| i == "bar"),
            border: sec.border("border").unwrap_or_else(|| self.border.clone()),
            shadow: sec.shadows("shadow").unwrap_or_else(|| self.shadow.clone()),
        }
    }
}

/// Resolves `<prefix>`, `<prefix>.hover`, `<prefix>.active`, `<prefix>.active.unfocused`.
/// Each state inherits from the resolved normal style, then from `defaults`.
fn tab_states<'a>(
    s: &dyn Fn(&str) -> Section<'a>,
    prefix: &str,
    defaults: &TabStates,
) -> TabStates {
    let normal = defaults.normal.overlay(&s(prefix));
    // Keys set on the base section override state defaults too, except the
    // ones each state is expected to change.
    let base = s(prefix);
    let inherit = |state: &TabStyle| -> TabStyle {
        TabStyle {
            radius: base.num("radius").unwrap_or(state.radius),
            font_size: base.num("font-size").unwrap_or(state.font_size),
            padding: base.edges("padding").unwrap_or(state.padding),
            height: base.num("height").or(state.height),
            foreground: if base.get("foreground").is_some()
                && state.foreground == defaults.normal.foreground
            {
                normal.foreground
            } else {
                state.foreground
            },
            ..state.clone()
        }
    };
    let hover = inherit(&TabStyle {
        background: defaults.hover.background,
        ..normal.clone()
    })
    .overlay(&s(&format!("{prefix}.hover")));
    let active = inherit(&defaults.active).overlay(&s(&format!("{prefix}.active")));
    let unfocused_default = TabStyle {
        foreground: normal.foreground,
        shadow: Vec::new(),
        ..active.clone()
    };
    let active_unfocused = unfocused_default.overlay(&s(&format!("{prefix}.active.unfocused")));
    TabStates {
        normal,
        hover,
        active,
        active_unfocused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r##"
[meta]
name = "Mini"
[terminal]
foreground = "#ffffff"
background = "#000000"
palette = ["#000000","#ff0000","#00ff00","#ffff00","#0000ff","#ff00ff","#00ffff","#ffffff",
           "#000000","#ff0000","#00ff00","#ffff00","#0000ff","#ff00ff","#00ffff","#ffffff"]
"##;

    #[test]
    fn minimal_theme_derives_everything() {
        let t = parse(MINIMAL).unwrap();
        assert!(t.warnings.is_empty(), "{:?}", t.warnings);
        let t = t.theme;
        assert_eq!(t.meta.mode, Mode::Dark);
        assert_eq!(t.token.accent, t.terminal.palette[4]);
        assert_eq!(t.container.padding, Edges::all(8.));
        assert_eq!(t.tab_bar.tabs.active.radius, 8.);
    }

    #[test]
    fn overrides_and_warnings() {
        let src = format!(
            "{MINIMAL}\n[tab]\nradius = 4\nforeground = \"#888888\"\n[tab.active]\nbackground = \"rgba(255,255,255,0.16)\"\n[container]\nmargin = [4, 16, 16, 16]\nradius = \"oops\"\n"
        );
        let t = parse(&src).unwrap();
        assert_eq!(t.warnings.len(), 1);
        let t = t.theme;
        assert_eq!(t.tabs.active.radius, 4.);
        assert_eq!(t.tabs.active.background.a, 0.16);
        assert_eq!(t.tabs.hover.foreground, Rgba::rgb(0x888888));
        assert_eq!(t.container.margin.top, 4.);
    }

    #[test]
    fn material_sections_without_background_are_transparent() {
        let t = parse(&format!(
            "{MINIMAL}\n[window]\nmaterial = \"glass\"\n[sidebar]\nmaterial = \"vibrancy-thin\"\n"
        ))
        .unwrap()
        .theme;
        assert_eq!(t.window.background.a, 0.);
        assert_eq!(t.sidebar.background.a, 0.);
        let plain = parse(MINIMAL).unwrap().theme;
        assert_eq!(plain.window.background.a, 1.);
        assert_eq!(plain.sidebar.background.a, 1.);
    }

    #[test]
    fn minimal_themes_derive_chrome_like_otty() {
        // Dracula's terminal colors; Otty draws its sidebar at #33353f and the
        // active card at #43454e.
        let t = parse(&MINIMAL.replace("#000000\"\n", "#282A36\"\n"))
            .unwrap()
            .theme;
        let hex = |c: Rgba| [c.r, c.g, c.b].map(|v| (v * 255.).round() as u8);
        assert_eq!(hex(t.panel.background), [0x33, 0x35, 0x40]);
        assert_eq!(hex(t.sidebar.background), [0x33, 0x35, 0x40]);
        let card = mix(
            t.sidebar.background,
            t.tabs.active.background.with_alpha(1.),
            t.tabs.active.background.a,
        );
        assert_eq!(hex(card), [0x43, 0x45, 0x4f]);
    }

    #[test]
    fn missing_palette_is_error() {
        assert!(
            parse("[meta]\nname='x'\n[terminal]\nforeground='#fff'\nbackground='#000'\n").is_err()
        );
    }
}
