//! `minimum-contrast`: text too close to its background in WCAG terms is
//! lightened or darkened, keeping its hue, until it reaches the ratio.

use std::collections::HashMap;

use motty_theme::Rgba;

/// WCAG relative luminance of an sRGB color (alpha ignored).
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

/// WCAG contrast ratio between two luminances, 1 to 21.
pub fn ratio(a: f32, b: f32) -> f32 {
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

fn mix(c: Rgba, toward: f32, t: f32) -> Rgba {
    Rgba {
        r: c.r + (toward - c.r) * t,
        g: c.g + (toward - c.g) * t,
        b: c.b + (toward - c.b) * t,
        a: c.a,
    }
}

/// `fg` moved toward white or black, whichever can contrast more with
/// `bg`, just far enough to reach `min`; as far as it goes when it can't.
pub fn adjust(fg: Rgba, bg: Rgba, min: f32) -> Rgba {
    let bg_l = luminance(bg);
    if ratio(luminance(fg), bg_l) >= min {
        return fg;
    }
    let toward = if ratio(1., bg_l) >= ratio(0., bg_l) {
        1.
    } else {
        0.
    };
    if ratio(luminance(mix(fg, toward, 1.)), bg_l) < min {
        return mix(fg, toward, 1.);
    }
    // Luminance grows steadily with the mix, so bisect for the least one.
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..12 {
        let mid = (lo + hi) / 2.;
        if ratio(luminance(mix(fg, toward, mid)), bg_l) >= min {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    mix(fg, toward, hi)
}

/// [`adjust`] remembered per color pair for one frame: a screen holds few
/// distinct pairs, so most cells cost one hash lookup.
pub struct Contrast {
    min: f32,
    seen: HashMap<(u32, u32), Rgba>,
    /// Neighbouring cells mostly share colors: the last pair skips hashing.
    last: Option<((u32, u32), Rgba)>,
}

impl Contrast {
    /// `None` when `min` leaves colors alone.
    pub fn new(min: f32) -> Option<Self> {
        (min > 1.).then(|| Self {
            min,
            seen: HashMap::new(),
            last: None,
        })
    }

    pub fn apply(&mut self, fg: Rgba, bg: Rgba) -> Rgba {
        let key = (pack(fg), pack(bg));
        if let Some((last, out)) = self.last
            && last == key
        {
            return out;
        }
        let min = self.min;
        let out = *self.seen.entry(key).or_insert_with(|| adjust(fg, bg, min));
        self.last = Some((key, out));
        out
    }
}

fn pack(c: Rgba) -> u32 {
    let b = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    (b(c.r) << 24) | (b(c.g) << 16) | (b(c.b) << 8) | b(c.a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wcag_reference_values() {
        let (white, black) = (Rgba::rgb(0xffffff), Rgba::rgb(0x000000));
        assert!((ratio(luminance(white), luminance(black)) - 21.).abs() < 0.01);
        assert!((ratio(luminance(white), luminance(white)) - 1.).abs() < 1e-6);
        // #777 on white is the textbook 4.48:1.
        let grey = Rgba::rgb(0x777777);
        assert!((ratio(luminance(grey), luminance(white)) - 4.48).abs() < 0.01);
    }

    #[test]
    fn low_contrast_text_reaches_the_ratio() {
        let bg = Rgba::rgb(0x1e1e2e);
        let fg = Rgba::rgb(0x45475a);
        let out = adjust(fg, bg, 4.5);
        let r = ratio(luminance(out), luminance(bg));
        assert!((4.5..4.6).contains(&r), "{r}");
        // Lightened, not darkened, on a dark background.
        assert!(out.r > fg.r && out.g > fg.g && out.b > fg.b);
        // On a light background it darkens instead.
        let out = adjust(Rgba::rgb(0xdddddd), Rgba::rgb(0xffffff), 3.);
        assert!(out.r < 0.85);
        assert!(ratio(luminance(out), 1.) >= 3.);
    }

    #[test]
    fn enough_contrast_is_kept_and_the_impossible_goes_all_the_way() {
        let fg = Rgba::rgb(0xcdd6f4);
        assert_eq!(adjust(fg, Rgba::rgb(0x1e1e2e), 4.5), fg);
        // Mid grey can't reach 21:1 against anything but black or white.
        let out = adjust(Rgba::rgb(0x808080), Rgba::rgb(0x777777), 21.);
        assert_eq!((out.r, out.g, out.b), (0., 0., 0.));
    }

    #[test]
    fn off_at_one_and_cached() {
        assert!(Contrast::new(1.).is_none());
        let mut c = Contrast::new(7.).unwrap();
        let (fg, bg) = (Rgba::rgb(0x444444), Rgba::rgb(0x222222));
        let a = c.apply(fg, bg);
        assert_eq!(c.apply(fg, bg), a);
        assert_eq!(c.seen.len(), 1);
    }
}
