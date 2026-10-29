//! What bidi and minimum-contrast add to a frame, on a full 200×60 screen.
//! Run with `cargo test --release -p motty-render --test text_cost -- --ignored --nocapture`.

use std::hint::black_box;
use std::time::Instant;

use motty_render::bidi::{self, BidiCell};
use motty_render::contrast::Contrast;
use motty_theme::Rgba;

const COLS: usize = 200;
const ROWS: usize = 60;

fn screen(line: &str) -> Vec<Vec<char>> {
    let row: Vec<char> = line.chars().cycle().take(COLS).collect();
    vec![row; ROWS]
}

/// Microseconds per frame, best of a few rounds.
fn per_frame(mut frame: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..20 {
            frame();
        }
        best = best.min(start.elapsed().as_secs_f64() * 1e6 / 20.);
    }
    best
}

fn bidi_frame(rows: &[Vec<char>]) {
    for row in rows {
        let cells: Vec<BidiCell> = row
            .iter()
            .map(|&c| BidiCell {
                c,
                extra: &[],
                spacer: false,
            })
            .collect();
        black_box(bidi::reorder(&cells));
    }
}

#[test]
#[ignore]
fn frame_cost() {
    let ascii = screen("cargo build --release && ls -la ~/src | grep motty ");
    let mixed = screen("سلام دنیا ۱۲۳ hello שלום (עולם) cp ~/فایل.txt /tmp ");
    // What the element does per row before any bidi work.
    let scan = per_frame(|| {
        for row in &ascii {
            black_box(row.iter().any(|&c| c >= '\u{0590}'));
        }
    });
    let ascii_bidi = per_frame(|| bidi_frame(&ascii));
    let mixed_bidi = per_frame(|| bidi_frame(&mixed));

    let fg = [0x45475a, 0xcdd6f4, 0xf38ba8, 0xa6e3a1, 0x585b70, 0x9399b2].map(Rgba::rgb);
    let bg = [0x1e1e2e, 0x1e1e2e, 0x313244].map(Rgba::rgb);
    let contrast = per_frame(|| {
        let mut c = Contrast::new(4.5).unwrap();
        // Runs of a few cells per color pair, as on a real screen.
        for i in 0..COLS * ROWS {
            black_box(c.apply(fg[i / 7 % fg.len()], bg[i / 31 % bg.len()]));
        }
    });
    let contrast_uncached = per_frame(|| {
        for i in 0..COLS * ROWS {
            black_box(motty_render::contrast::adjust(
                fg[i % fg.len()],
                bg[i % bg.len()],
                4.5,
            ));
        }
    });
    println!("200x60 cells, µs per frame:");
    println!("  bidi pre-scan, ASCII screen     {scan:8.1}");
    println!("  bidi::reorder, ASCII screen     {ascii_bidi:8.1}");
    println!("  bidi::reorder, mixed RTL screen {mixed_bidi:8.1}");
    println!("  minimum-contrast 4.5 (cached)   {contrast:8.1}");
    println!("  minimum-contrast 4.5 (uncached) {contrast_uncached:8.1}");
    #[cfg(target_os = "macos")]
    {
        let shaping = per_frame(shape_rtl_runs);
        println!(
            "  CoreText, 12 RTL runs per row   {shaping:8.1} (first frame; GPUI caches after)"
        );
    }
}

/// Shaping the right-to-left runs of the mixed screen, uncached.
#[cfg(target_os = "macos")]
fn shape_rtl_runs() {
    use gpui::{Font, FontFeatures, FontRun, FontStyle, FontWeight, PlatformTextSystem, px};
    thread_local! {
        static TS: gpui_apple::AppleTextSystem = gpui_apple::AppleTextSystem::new();
    }
    TS.with(|ts| {
        let font = Font {
            family: "Menlo".into(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };
        let font_id = ts.font_id(&font).unwrap();
        let runs = [
            "\u{202E}سلام دنیا \u{202C}",
            "\u{202E}שלום (עולם)\u{202C}",
            "\u{202E}فایل\u{202C}",
        ];
        for _ in 0..ROWS {
            for i in 0..12 {
                let text = runs[i % runs.len()];
                let run = FontRun {
                    len: text.len(),
                    font_id,
                };
                black_box(ts.layout_line(text, px(13.), &[run]));
            }
        }
    });
}
