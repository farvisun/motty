//! Development tasks. Run with `cargo xtask <command>`.
//!
//! - `parity [--release] [--fixture PATH]`: renders the same fixture in Otty and
//!   Motty at identical window bounds, captures both windows and diffs them.
//! - `diff A.png B.png [OUT.png]`: pixel diff with per-region stats.
//! - `crop IMG.png X Y W H OUT.png`: crops a region given in points.
//! - `scan IMG.png x|y POS`: color transitions along a column (x=POS) or row (y=POS), in points.
//! - `probe IMG.png X Y`: prints the color at a point (in points, @2x aware).
//! - `metrics IMG.png [LEFT_PT]`: text grid metrics measured from ink rows.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use image::{Rgba, RgbaImage};

const OTTY_CLI: &str = "/Applications/Otty.app/Contents/MacOS/otty-cli";
const BOUNDS: (u32, u32, u32, u32) = (100, 100, 1200, 800);
const FIXTURE_TITLE: &str = "Motty fixture";
/// Otty's sidebar width from `sidebar-width` (points); regions are split here.
const SIDEBAR_PT: u32 = 225;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("parity") => parity(&args[1..]),
        Some("diff") if args.len() >= 3 => {
            let out = args.get(3).map(PathBuf::from);
            diff(Path::new(&args[1]), Path::new(&args[2]), out.as_deref())
        }
        Some("crop") if args.len() == 7 => {
            let n: Vec<f32> = args[2..6].iter().map(|v| v.parse()).collect::<Result<_, _>>()?;
            crop(Path::new(&args[1]), (n[0], n[1], n[2], n[3]), Path::new(&args[6]))
        }
        Some("scan") if args.len() == 4 => scan(Path::new(&args[1]), &args[2], args[3].parse()?),
        Some("probe") if args.len() == 4 => probe(Path::new(&args[1]), args[2].parse()?, args[3].parse()?),
        Some("metrics") if args.len() >= 2 => {
            let left = args.get(2).map(|v| v.parse()).transpose()?.unwrap_or(SIDEBAR_PT);
            metrics(Path::new(&args[1]), left)
        }
        _ => {
            eprintln!("usage: cargo xtask parity [--release] [--fixture PATH] | diff A B [OUT] | probe IMG X Y | metrics IMG [LEFT_PT]");
            std::process::exit(2);
        }
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn pause(ms: u64) {
    sleep(Duration::from_millis(ms));
}

fn run(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().with_context(|| format!("running {cmd:?}"))?;
    if !out.status.success() {
        bail!("{cmd:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Builds the CoreGraphics window-lookup helper on first use.
fn winid_tool() -> Result<PathBuf> {
    let bin = root().join("target/xtask-winid");
    let src = root().join("xtask/src/winid.swift");
    let stale = match (bin.metadata(), src.metadata()) {
        (Ok(b), Ok(s)) => b.modified()? < s.modified()?,
        _ => true,
    };
    if stale {
        run(Command::new("swiftc").args(["-O", "-o"]).arg(&bin).arg(&src))?;
    }
    Ok(bin)
}

fn find_window(owner: &str, title: &str) -> Result<String> {
    let tool = winid_tool()?;
    for _ in 0..40 {
        if let Ok(id) = run(Command::new(&tool).args([owner, title])) {
            return Ok(id);
        }
        pause(250);
    }
    bail!("no {owner} window titled {title:?}")
}

fn capture(window_id: &str, out: &Path) -> Result<()> {
    run(Command::new("screencapture").args(["-x", "-o", "-l", window_id]).arg(out))?;
    Ok(())
}

fn otty_window_ids() -> Result<Vec<(String, String)>> {
    let json = run(Command::new(OTTY_CLI).args(["--json", "window", "list"]))?;
    let v: serde_json::Value = serde_json::from_str(&json)?;
    Ok(v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| Some((w["id"].as_str()?.to_owned(), w["title"].as_str().unwrap_or("").to_owned())))
        .collect())
}

fn capture_otty(fixture: &Path, out: &Path) -> Result<()> {
    let socket = dirs_home().join("Library/Application Support/io.appmakes.otty/otty.sock");
    if !socket.exists() {
        run(Command::new("open").args(["-a", "Otty"]))?;
        for _ in 0..40 {
            if socket.exists() {
                break;
            }
            pause(250);
        }
    }
    let command = format!("/bin/sh -c '{}; sleep 600'", fixture.display());
    run(Command::new(OTTY_CLI).args(["open", "--command", &command]))?;
    pause(1500);
    let (x, y, w, h) = BOUNDS;
    run(Command::new("osascript").args([
        "-e",
        &format!("tell application \"Otty\" to set bounds of front window to {{{x}, {y}, {}, {}}}", x + w, y + h),
    ]))?;
    pause(1500);
    let id = find_window("Otty", FIXTURE_TITLE)?;
    capture(&id, out)?;
    for (wid, title) in otty_window_ids()? {
        if title == FIXTURE_TITLE {
            let _ = run(Command::new(OTTY_CLI).args(["-y", "window", "close", "--force", &wid]));
        }
    }
    Ok(())
}

fn capture_motty(release: bool, fixture: &Path, out: &Path) -> Result<()> {
    let mut build = Command::new("cargo");
    build.args(["build", "--quiet", "-p", "motty-app"]).current_dir(root());
    if release {
        build.arg("--release");
    }
    run(&mut build)?;
    let bin = root().join(if release { "target/release/Motty" } else { "target/debug/Motty" });
    let (x, y, w, h) = BOUNDS;
    let script = format!("{}; sleep 600", fixture.display());
    let mut child: Child = Command::new(bin)
        .args(["--bounds", &format!("{x},{y},{w},{h}"), "-e", "/bin/sh", "-c", &script])
        .spawn()?;
    let result = (|| {
        let id = find_window("Motty", FIXTURE_TITLE)?;
        pause(1200);
        capture(&id, out)
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn dirs_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn parity(args: &[String]) -> Result<()> {
    let release = args.iter().any(|a| a == "--release");
    let fixture = args
        .iter()
        .position(|a| a == "--fixture")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("xtask/fixtures/render.sh"));
    let fixture = fixture.canonicalize().context("fixture path")?;
    let dir = root().join("target/parity");
    std::fs::create_dir_all(&dir)?;
    let (otty, motty) = (dir.join("otty.png"), dir.join("motty.png"));
    capture_otty(&fixture, &otty).context("capturing Otty")?;
    capture_motty(release, &fixture, &motty).context("capturing Motty")?;
    println!("captured {} and {}", otty.display(), motty.display());
    diff(&otty, &motty, Some(&dir.join("diff.png")))?;
    for (name, img) in [("otty", &otty), ("motty", &motty)] {
        println!("--- {name} grid metrics");
        metrics(img, SIDEBAR_PT)?;
    }
    Ok(())
}

fn load(path: &Path) -> Result<RgbaImage> {
    Ok(image::open(path).with_context(|| format!("opening {}", path.display()))?.to_rgba8())
}

/// Backing scale, inferred from the capture width vs. the requested bounds.
fn scale_of(img: &RgbaImage) -> u32 {
    (img.width() / BOUNDS.2).max(1)
}

fn distance(a: &Rgba<u8>, b: &Rgba<u8>) -> f32 {
    let d = |i: usize| (a[i] as f32 - b[i] as f32).powi(2);
    (d(0) + d(1) + d(2)).sqrt()
}

/// Differences above this RGB distance count as visible.
const VISIBLE: f32 = 12.0;

fn diff(a_path: &Path, b_path: &Path, out: Option<&Path>) -> Result<()> {
    let (a, b) = (load(a_path)?, load(b_path)?);
    if a.dimensions() != b.dimensions() {
        println!("size mismatch: {:?} vs {:?}", a.dimensions(), b.dimensions());
    }
    let (w, h) = (a.width().min(b.width()), a.height().min(b.height()));
    let split = SIDEBAR_PT * scale_of(&a);
    let mut out_img = RgbaImage::new(w, h);
    let mut regions = [("sidebar", 0u64, 0u64, 0f64), ("terminal", 0, 0, 0.)];
    for y in 0..h {
        for x in 0..w {
            let (pa, pb) = (a.get_pixel(x, y), b.get_pixel(x, y));
            let d = distance(pa, pb);
            let r = &mut regions[(x >= split) as usize];
            r.1 += 1;
            r.3 += d as f64;
            let gray = ((pa[0] as u32 + pa[1] as u32 + pa[2] as u32) / 3) as u8 / 3 + 170;
            let px = if d > VISIBLE {
                r.2 += 1;
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([gray, gray, gray, 255])
            };
            out_img.put_pixel(x, y, px);
        }
    }
    for (name, total, differing, sum) in regions {
        println!(
            "{name:>9}: {:6.2}% pixels differ, mean distance {:.2}",
            100. * differing as f64 / total.max(1) as f64,
            sum / total.max(1) as f64
        );
    }
    if let Some(out) = out {
        out_img.save(out)?;
        println!("diff image: {}", out.display());
    }
    Ok(())
}

fn crop(path: &Path, (x, y, w, h): (f32, f32, f32, f32), out: &Path) -> Result<()> {
    let img = load(path)?;
    let s = scale_of(&img) as f32;
    let view = image::imageops::crop_imm(&img, (x * s) as u32, (y * s) as u32, (w * s) as u32, (h * s) as u32);
    view.to_image().save(out)?;
    Ok(())
}

fn scan(path: &Path, axis: &str, pos: f32) -> Result<()> {
    let img = load(path)?;
    let s = scale_of(&img);
    let fixed = (pos * s as f32) as u32;
    let len = if axis == "x" { img.height() } else { img.width() };
    let at = |i: u32| if axis == "x" { *img.get_pixel(fixed, i) } else { *img.get_pixel(i, fixed) };
    let mut prev = at(0);
    let mut out = Vec::new();
    for i in 1..len {
        let p = at(i);
        if distance(&p, &prev) > 30. {
            out.push(format!("{:.1}:#{:02x}{:02x}{:02x}", i as f32 / s as f32, p[0], p[1], p[2]));
        }
        prev = p;
    }
    println!("{}", out.join(" "));
    Ok(())
}

fn probe(path: &Path, x: f32, y: f32) -> Result<()> {
    let img = load(path)?;
    let s = scale_of(&img) as f32;
    let p = img.get_pixel((x * s) as u32, (y * s) as u32);
    println!("#{:02x}{:02x}{:02x} (alpha {})", p[0], p[1], p[2], p[3]);
    Ok(())
}

/// Finds rows of text "ink" right of `left_pt` and reports the grid origin and pitch.
fn metrics(path: &Path, left_pt: u32) -> Result<()> {
    let img = load(path)?;
    let s = scale_of(&img);
    let x0 = left_pt * s + 4 * s;
    // Background: bottom-right corner of the terminal area is blank in fixtures.
    let bg = *img.get_pixel(img.width() - 20 * s, img.height() - 20 * s);
    let top = 0;
    let ink_row = |y: u32| (x0..img.width() - 4 * s).any(|x| distance(img.get_pixel(x, y), &bg) > 60.);
    let mut runs: Vec<(u32, u32)> = Vec::new();
    let mut y = top;
    while y < img.height() {
        if ink_row(y) {
            let start = y;
            while y < img.height() && ink_row(y) {
                y += 1;
            }
            runs.push((start, y));
        }
        y += 1;
    }
    let first_ink_x = (x0..img.width())
        .find(|&x| (0..img.height()).any(|y| distance(img.get_pixel(x, y), &bg) > 60.))
        .unwrap_or(0);
    println!("background #{:02x}{:02x}{:02x}, first ink x = {:.1}pt", bg[0], bg[1], bg[2], first_ink_x as f32 / s as f32);
    let shown: Vec<String> = runs
        .iter()
        .take(16)
        .map(|(a, b)| format!("{:.1}-{:.1}", *a as f32 / s as f32, *b as f32 / s as f32))
        .collect();
    println!("ink rows (pt): {}", shown.join(" "));
    Ok(())
}
