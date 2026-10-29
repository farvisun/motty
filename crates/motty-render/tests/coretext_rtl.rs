//! Right-to-left runs through CoreText, the way the terminal element shapes
//! them: logical text inside RLO…PDF, then placed with `bidi::place_rtl`.

#![cfg(target_os = "macos")]

use gpui::{
    Font, FontFeatures, FontRun, FontStyle, FontWeight, LineLayout, PlatformTextSystem, px,
};
use gpui_apple::AppleTextSystem;
use motty_render::bidi::{self, PDF, RLO};

fn shape(ts: &AppleTextSystem, text: &str) -> LineLayout {
    let font = Font {
        family: "Menlo".into(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    };
    let font_id = ts.font_id(&font).unwrap();
    ts.layout_line(
        text,
        px(13.),
        &[FontRun {
            len: text.len(),
            font_id,
        }],
    )
}

/// `word` (logical order) as the element hands it over: the text, each
/// cell's first byte and where the PDF starts.
fn wrapped(word: &str) -> (String, Vec<usize>, usize) {
    let mut text = String::from(RLO);
    let mut starts = Vec::new();
    for c in word.chars() {
        starts.push(text.len());
        text.push(c);
    }
    let end = text.len();
    text.push(PDF);
    (text, starts, end)
}

fn glyphs(layout: &LineLayout) -> Vec<(usize, f32, u32)> {
    layout
        .runs
        .iter()
        .flat_map(|r| r.glyphs.iter())
        .map(|g| (g.index, f32::from(g.position.x), g.id.0))
        .collect()
}

#[test]
fn hebrew_comes_out_right_to_left_one_glyph_per_cell() {
    let ts = AppleTextSystem::new();
    let (text, starts, end) = wrapped("שלום");
    let layout = shape(&ts, &text);
    let shaped = glyphs(&layout);
    let x_of = |byte: usize| shaped.iter().find(|g| g.0 == byte).map(|g| g.1);
    // The first logical letter is the rightmost glyph.
    assert!(
        x_of(starts[0]).unwrap() > x_of(starts[3]).unwrap(),
        "{shaped:?}"
    );
    let pairs: Vec<(usize, f32)> = shaped.iter().map(|g| (g.0, g.1)).collect();
    let xs = bidi::place_rtl(&pairs, &starts, end, 8., f32::from(layout.width), false);
    let mut placed: Vec<(usize, f32)> = pairs
        .iter()
        .zip(&xs)
        .filter_map(|(g, x)| x.map(|x| (g.0, x)))
        .collect();
    placed.sort_by(|a, b| a.1.total_cmp(&b.1));
    // ם ו ל ש from left to right, one per 8-point cell.
    let order: Vec<usize> = placed.iter().map(|p| p.0).collect();
    assert_eq!(order, [starts[3], starts[2], starts[1], starts[0]]);
    for (i, (_, x)) in placed.iter().enumerate() {
        assert!((x - i as f32 * 8.).abs() < 0.01, "{placed:?}");
    }
}

#[test]
fn persian_letters_join() {
    let ts = AppleTextSystem::new();
    let id = |text: &str| glyphs(&shape(&ts, text));
    let isolated = |c: &str| {
        let shaped = id(&wrapped(c).0);
        shaped.iter().find(|g| g.0 == RLO.len_utf8()).unwrap().2
    };
    let alone = isolated("ب");
    let (text, starts, _) = wrapped("ببب");
    let word = id(&text);
    let form = |byte: usize| word.iter().find(|g| g.0 == byte).unwrap().2;
    // Initial, medial and final forms: none is the isolated glyph.
    let forms = [form(starts[0]), form(starts[1]), form(starts[2])];
    assert!(forms.iter().all(|&f| f != alone), "{forms:?} vs {alone}");
    assert_ne!(forms[0], forms[2]);
    // Persian-only letters (پ گ) and the Persian yeh join too.
    let (text, starts, _) = wrapped("گپی");
    let word = id(&text);
    let joined = |byte: usize| word.iter().find(|g| g.0 == byte).unwrap().2;
    assert_ne!(joined(starts[0]), isolated("گ"));
    assert_ne!(joined(starts[2]), isolated("ی"));
}

#[test]
fn brackets_mirror_and_controls_are_dropped() {
    let ts = AppleTextSystem::new();
    let (text, starts, end) = wrapped("(");
    let inside = glyphs(&shape(&ts, &text));
    let open_id = inside.iter().find(|g| g.0 == starts[0]).unwrap().2;
    let close = glyphs(&shape(&ts, ")"));
    assert_eq!(open_id, close[0].2, "( drawn right to left is )");
    // Whatever CoreText makes of RLO and PDF never reaches the screen.
    let pairs: Vec<(usize, f32)> = inside.iter().map(|g| (g.0, g.1)).collect();
    let xs = bidi::place_rtl(&pairs, &starts, end, 8., 8., false);
    for (g, x) in pairs.iter().zip(&xs) {
        assert_eq!(x.is_some(), g.0 == starts[0], "{pairs:?}");
    }
}
