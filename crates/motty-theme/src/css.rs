//! Parsers for the CSS-flavoured values used in `.ottytheme` files:
//! colors, `box-shadow`, `border` and edge lists (margin / padding).

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 0.,
    };

    pub fn rgb(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xff) as f32 / 255.,
            g: ((hex >> 8) & 0xff) as f32 / 255.,
            b: (hex & 0xff) as f32 / 255.,
            a: 1.,
        }
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Source-over composite of `self` on top of an opaque `under`.
    pub fn over(self, under: Rgba) -> Rgba {
        let a = self.a;
        Rgba {
            r: self.r * a + under.r * (1. - a),
            g: self.g * a + under.g * (1. - a),
            b: self.b * a + under.b * (1. - a),
            a: 1.,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(msg: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError(msg.into()))
}

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(..)`, `rgba(..)`, `none`, `transparent`.
pub fn parse_color(s: &str) -> Result<Rgba, ParseError> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("transparent") {
        return Ok(Rgba::TRANSPARENT);
    }
    if let Some(hex) = s.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()
            .ok_or_else(|| ParseError(format!("bad hex color {s:?}")))?;
        let (r, g, b, a) = match digits.len() {
            3 | 4 => {
                let e = |d: u8| d * 17;
                (
                    e(digits[0]),
                    e(digits[1]),
                    e(digits[2]),
                    digits.get(3).map_or(255, |&d| e(d)),
                )
            }
            6 | 8 => {
                let p = |i: usize| digits[i] * 16 + digits[i + 1];
                (p(0), p(2), p(4), if digits.len() == 8 { p(6) } else { 255 })
            }
            _ => return err(format!("bad hex color {s:?}")),
        };
        return Ok(Rgba {
            r: r as f32 / 255.,
            g: g as f32 / 255.,
            b: b as f32 / 255.,
            a: a as f32 / 255.,
        });
    }
    let lower = s.to_ascii_lowercase();
    let inner = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'));
    if let Some(inner) = inner {
        let parts: Vec<&str> = inner
            .split([',', '/', ' '])
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() != 3 && parts.len() != 4 {
            return err(format!("bad rgb() color {s:?}"));
        }
        let chan = |p: &str| -> Result<f32, ParseError> {
            if let Some(pct) = p.strip_suffix('%') {
                pct.parse::<f32>()
                    .map(|v| v / 100.)
                    .map_err(|_| ParseError(format!("bad channel {p:?}")))
            } else {
                p.parse::<f32>()
                    .map(|v| v / 255.)
                    .map_err(|_| ParseError(format!("bad channel {p:?}")))
            }
        };
        let alpha = match parts.get(3) {
            None => 1.,
            Some(p) => match p.strip_suffix('%') {
                Some(pct) => pct.parse::<f32>().map(|v| v / 100.),
                None => p.parse::<f32>(),
            }
            .map_err(|_| ParseError(format!("bad alpha {p:?}")))?,
        };
        return Ok(Rgba {
            r: chan(parts[0])?,
            g: chan(parts[1])?,
            b: chan(parts[2])?,
            a: alpha.clamp(0., 1.),
        });
    }
    err(format!("unsupported color {s:?}"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub inset: bool,
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Rgba,
}

/// Splits on top-level commas (ignoring commas inside parentheses).
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Whitespace tokens, keeping `rgba( ... )` together.
fn tokens(s: &str) -> Vec<&str> {
    split_top_level(s.trim(), ' ')
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect()
}

pub fn parse_length(s: &str) -> Result<f32, ParseError> {
    let s = s.trim();
    let num = s
        .strip_suffix("px")
        .or_else(|| s.strip_suffix("pt"))
        .unwrap_or(s);
    num.parse::<f32>()
        .map_err(|_| ParseError(format!("bad length {s:?}")))
}

/// CSS `box-shadow`: comma-separated list, or `none`.
pub fn parse_shadows(s: &str) -> Result<Vec<Shadow>, ParseError> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    split_top_level(s, ',')
        .into_iter()
        .map(|part| {
            let mut inset = false;
            let mut lengths = Vec::new();
            let mut color = None;
            for tok in tokens(part) {
                if tok.eq_ignore_ascii_case("inset") {
                    inset = true;
                } else if let Ok(len) = parse_length(tok) {
                    lengths.push(len);
                } else {
                    color = Some(parse_color(tok)?);
                }
            }
            if !(2..=4).contains(&lengths.len()) {
                return err(format!("bad shadow {part:?}"));
            }
            Ok(Shadow {
                inset,
                offset_x: lengths[0],
                offset_y: lengths[1],
                blur: lengths.get(2).copied().unwrap_or(0.),
                spread: lengths.get(3).copied().unwrap_or(0.),
                color: color.unwrap_or(Rgba {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 1.,
                }),
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Border {
    pub width: f32,
    /// `None` means `auto`: the theme's panel border color.
    pub color: Option<Rgba>,
}

/// CSS `border` shorthand (`1px solid #25332D`, `1px solid auto`) or `none`. Style is ignored.
pub fn parse_border(s: &str) -> Result<Option<Border>, ParseError> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let mut width = 1.;
    let mut color = None;
    for tok in tokens(s) {
        match tok {
            "solid" | "dashed" | "dotted" | "double" | "auto" => {}
            t => match parse_length(t) {
                Ok(w) => width = w,
                Err(_) => color = Some(parse_color(t)?),
            },
        }
    }
    if width == 0. {
        return Ok(None);
    }
    Ok(Some(Border { width, color }))
}

/// Margin / padding in CSS order: top, right, bottom, left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub fn all(v: f32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    /// CSS 1/2/3/4-value expansion.
    pub fn from_slice(v: &[f32]) -> Result<Self, ParseError> {
        Ok(match *v {
            [a] => Self::all(a),
            [tb, lr] => Self {
                top: tb,
                right: lr,
                bottom: tb,
                left: lr,
            },
            [t, lr, b] => Self {
                top: t,
                right: lr,
                bottom: b,
                left: lr,
            },
            [t, r, b, l] => Self {
                top: t,
                right: r,
                bottom: b,
                left: l,
            },
            _ => return err(format!("edges need 1-4 values, got {}", v.len())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(
            parse_color("#fff").unwrap(),
            Rgba {
                r: 1.,
                g: 1.,
                b: 1.,
                a: 1.
            }
        );
        assert_eq!(parse_color("#2528325d").unwrap().a, 0x5d as f32 / 255.);
        let c = parse_color("rgba(255,255,255,0.16)").unwrap();
        assert_eq!((c.r, c.a), (1., 0.16));
        assert_eq!(parse_color("none").unwrap(), Rgba::TRANSPARENT);
        assert_eq!(parse_color("#ffffff00").unwrap().a, 0.);
        assert!(parse_color("#12345").is_err());
    }

    #[test]
    fn shadows() {
        let s = parse_shadows("0 20px 40px rgba(0,0,0,0.4)").unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].offset_y, s[0].blur, s[0].color.a), (20., 40., 0.4));
        let s = parse_shadows("0 1.5px 6px rgba(0,0,0,0.18), inset 0 1px 0 #fff").unwrap();
        assert_eq!(s.len(), 2);
        assert!(s[1].inset);
        assert!(parse_shadows("none").unwrap().is_empty());
    }

    #[test]
    fn borders() {
        let b = parse_border("1px solid rgba(255,255,255,0.18)")
            .unwrap()
            .unwrap();
        assert_eq!(b.width, 1.);
        assert_eq!(b.color.unwrap().a, 0.18);
        assert_eq!(parse_border("1px solid auto").unwrap().unwrap().color, None);
        assert_eq!(parse_border("none").unwrap(), None);
    }

    #[test]
    fn edges() {
        assert_eq!(Edges::from_slice(&[4., 16., 16., 16.]).unwrap().top, 4.);
        assert_eq!(Edges::from_slice(&[8., 16.]).unwrap().left, 16.);
    }
}
