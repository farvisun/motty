//! Development tasks. Run with `cargo xtask <command>`.
//!
//! - `parity [--release] [--fixture PATH] [--tabs]`: renders the same fixture in
//!   Otty and Motty at identical window bounds, captures both windows and diffs
//!   them. `--tabs` adds two shell tabs (the last one active) to compare the
//!   sidebar; the terminal region then shows the user's prompt. `--theme NAME`
//!   renders both with that theme (Otty's config is restored afterwards).
//!   `--backdrop RRGGBB` puts a solid window behind both and captures their
//!   screen rect, so materials blur a known color.
//! - `bundle [--release]`: builds `target/<profile>/Motty.app`.
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
        Some("bundle") => {
            bundle(args.iter().any(|a| a == "--release")).map(|app| println!("{}", app.display()))
        }
        Some("diff") if args.len() >= 3 => {
            let out = args.get(3).map(PathBuf::from);
            diff(Path::new(&args[1]), Path::new(&args[2]), out.as_deref())
        }
        Some("crop") if args.len() == 7 => {
            let n: Vec<f32> = args[2..6]
                .iter()
                .map(|v| v.parse())
                .collect::<Result<_, _>>()?;
            crop(
                Path::new(&args[1]),
                (n[0], n[1], n[2], n[3]),
                Path::new(&args[6]),
            )
        }
        Some("scan") if args.len() >= 4 => scan(
            Path::new(&args[1]),
            &args[2],
            args[3].parse()?,
            args.get(4).map(|t| t.parse()).transpose()?.unwrap_or(30.),
        ),
        Some("probe") if args.len() == 4 => {
            probe(Path::new(&args[1]), args[2].parse()?, args[3].parse()?)
        }
        Some("metrics") if args.len() >= 2 => {
            let left = args
                .get(2)
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(SIDEBAR_PT);
            metrics(Path::new(&args[1]), left)
        }
        _ => {
            eprintln!(
                "usage: cargo xtask parity [--release] [--fixture PATH] [--tabs] [--split right|down] [--background CMD]... [--wait MS] [--theme NAME] [--backdrop RRGGBB] [--set KEY=VALUE]... [--hover X,Y] | bundle [--release] | diff A B [OUT] | probe IMG X Y | metrics IMG [LEFT_PT]"
            );
            std::process::exit(2);
        }
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
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
    swift_tool("winid")
}

/// Compiles `xtask/src/<name>.swift` to `target/xtask-<name>` when stale.
fn swift_tool(name: &str) -> Result<PathBuf> {
    let bin = root().join(format!("target/xtask-{name}"));
    let src = root().join(format!("xtask/src/{name}.swift"));
    let stale = match (bin.metadata(), src.metadata()) {
        (Ok(b), Ok(s)) => b.modified()? < s.modified()?,
        _ => true,
    };
    if stale {
        run(Command::new("swiftc")
            .args(["-O", "-o"])
            .arg(&bin)
            .arg(&src))?;
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

/// The backdrop's window id while one is up (0 otherwise): captures then take
/// the window's screen rect, so behind-window materials show what they blur.
static BACKDROP_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn capture(window_id: &str, out: &Path) -> Result<()> {
    let backdrop = BACKDROP_ID.load(std::sync::atomic::Ordering::Relaxed);
    if backdrop != 0 {
        // The screen rect, composited from the window and the backdrop only.
        let (x, y, w, h) = BOUNDS;
        run(Command::new(swift_tool("rectshot")?)
            .args([x, y, w, h].map(|v| v.to_string()))
            .arg(out)
            .args([window_id.to_owned(), backdrop.to_string()]))?;
    } else {
        run(Command::new("screencapture")
            .args(["-x", "-o", "-l", window_id])
            .arg(out))?;
    }
    Ok(())
}

/// A solid-color window behind the window under test, covering everything
/// else on screen around it. Killed on drop.
struct Backdrop(Child);

/// The live backdrop's stdin and stdout, for ordering it below a window.
static BACKDROP_IO: std::sync::Mutex<
    Option<(
        std::process::ChildStdin,
        std::io::BufReader<std::process::ChildStdout>,
    )>,
> = std::sync::Mutex::new(None);

/// Puts the backdrop (if any) directly below `window_id` and waits for it.
fn order_backdrop_below(window_id: &str) -> Result<()> {
    use std::io::{BufRead, Write};
    let mut io = BACKDROP_IO.lock().unwrap();
    if let Some((stdin, stdout)) = io.as_mut() {
        writeln!(stdin, "{window_id}")?;
        let mut line = String::new();
        stdout.read_line(&mut line)?;
        pause(200);
    }
    Ok(())
}

impl Backdrop {
    fn show(color: &str) -> Result<Self> {
        let tool = swift_tool("backdrop")?;
        let (x, y, w, h) = BOUNDS;
        let m = 40;
        let mut child = Command::new(tool)
            .args([x - m, y - m, w + 2 * m, h + 2 * m].map(|v| v.to_string()))
            .arg(color.trim_start_matches('#'))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        // Wait for "ready" so the window is up before anything is raised over it.
        let mut stdout = std::io::BufReader::new(child.stdout.take().context("backdrop stdout")?);
        let mut line = String::new();
        std::io::BufRead::read_line(&mut stdout, &mut line)?;
        let id = line
            .trim()
            .strip_prefix("ready ")
            .and_then(|id| id.parse().ok())
            .context("backdrop window id")?;
        *BACKDROP_IO.lock().unwrap() =
            Some((child.stdin.take().context("backdrop stdin")?, stdout));
        BACKDROP_ID.store(id, std::sync::atomic::Ordering::Relaxed);
        Ok(Self(child))
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        BACKDROP_ID.store(0, std::sync::atomic::Ordering::Relaxed);
        *BACKDROP_IO.lock().unwrap() = None;
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn otty_window_ids() -> Result<Vec<(String, String)>> {
    let json = run(Command::new(OTTY_CLI).args(["--json", "window", "list"]))?;
    let v: serde_json::Value = serde_json::from_str(&json)?;
    Ok(v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            Some((
                w["id"].as_str()?.to_owned(),
                w["title"].as_str().unwrap_or("").to_owned(),
            ))
        })
        .collect())
}

/// Window title once the extra tabs are open: the active tab's cwd, the repo.
fn tabs_title() -> String {
    let root = root();
    match root.strip_prefix(dirs_home()) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => root.display().to_string(),
    }
}

/// Temporarily changes Otty's config (a theme, `--set` keys) for a parity
/// run; Motty reads the same file. Dropping it writes the user's
/// config.toml back byte for byte (the CLI may reformat it) and reloads, so
/// the user's setup is restored even when the capture fails.
struct OttyConfig {
    path: PathBuf,
    original: Vec<u8>,
}

impl OttyConfig {
    fn apply(theme: Option<&str>, sets: &[(String, String)]) -> Result<Self> {
        let path = PathBuf::from(run(Command::new(OTTY_CLI).args(["config", "path"]))?.trim());
        let original =
            std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let guard = Self { path, original };
        let themes = theme
            .into_iter()
            .flat_map(|t| [("theme", t), ("theme-dark", t)]);
        for (key, value) in themes.chain(sets.iter().map(|(k, v)| (k.as_str(), v.as_str()))) {
            run(Command::new(OTTY_CLI).args(["config", "set", key, value]))?;
        }
        run(Command::new(OTTY_CLI).args(["config", "reload"]))?;
        pause(1000);
        Ok(guard)
    }
}

impl Drop for OttyConfig {
    fn drop(&mut self) {
        if let Err(e) = std::fs::write(&self.path, &self.original) {
            eprintln!("could not restore {}: {e}", self.path.display());
        }
        let _ = run(Command::new(OTTY_CLI).args(["config", "reload"]));
        // Otty applies a reload asynchronously; a switch right after (the next
        // parity run) could otherwise be overtaken by this one.
        pause(1000);
    }
}

fn capture_otty(fixture: &Path, tabs: bool, split: Option<&str>, out: &Path) -> Result<()> {
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
    // Fixture windows a failed run left behind would share the title.
    for (id, title) in otty_window_ids()? {
        if title == FIXTURE_TITLE {
            let _ = run(Command::new(OTTY_CLI).args(["-y", "window", "close", "--force", &id]));
        }
    }
    let before: Vec<String> = otty_window_ids()?.into_iter().map(|(id, _)| id).collect();
    let command = format!("/bin/sh -c '{}; sleep 600'", fixture.display());
    run(Command::new(OTTY_CLI).args(["open", "--command", &command]))?;
    pause(1500);
    let window = otty_window_ids()?
        .into_iter()
        .find(|(id, t)| t == FIXTURE_TITLE && !before.contains(id))
        .map(|(id, _)| id);
    let window = window.context("fixture window not found")?;
    // Place the fixture window and make it Otty's key window (another Otty
    // window may otherwise stay key when the app is brought forward).
    let (x, y, w, h) = BOUNDS;
    // The CGWindowID (also the AppleScript id) stays put when new tabs
    // retitle the window, unlike the title, which other windows may share.
    let id = find_window("Otty", FIXTURE_TITLE)?;
    let target = format!("(window id {id})");
    run(Command::new("osascript").args([
        "-e",
        &format!(
            "tell application \"Otty\" to set bounds of {target} to {{{x}, {y}, {}, {}}}",
            x + w,
            y + h
        ),
        "-e",
        &format!("tell application \"Otty\" to set index of {target} to 1"),
    ]))?;
    if tabs {
        // A busy tab and an idle one; Otty activates each new tab.
        run(Command::new(OTTY_CLI).args([
            "tab",
            "new",
            "--window",
            &window,
            "--command",
            "sleep 600",
        ]))?;
        run(Command::new(OTTY_CLI).args(["tab", "new", "--window", &window]))?;
        // New tabs can bring back the previous theme; reload the current one.
        run(Command::new(OTTY_CLI).args(["config", "reload"]))?;
    }
    if !background_tabs().is_empty() {
        let json =
            run(Command::new(OTTY_CLI).args(["--json", "tab", "list", "--window", &window]))?;
        let tabs: serde_json::Value = serde_json::from_str(&json)?;
        let first = tabs["data"][0]["id"]
            .as_str()
            .context("fixture tab not found")?
            .to_owned();
        for command in background_tabs() {
            run(Command::new(OTTY_CLI).args([
                "tab",
                "new",
                "--window",
                &window,
                "--no-focus",
                "--command",
                command,
            ]))?;
        }
        // `--no-focus` still activates the new tab.
        run(Command::new(OTTY_CLI).args(["tab", "focus", &first]))?;
    }
    if let Some(direction) = split {
        // The new pane runs the fixture too and takes focus.
        let json =
            run(Command::new(OTTY_CLI).args(["--json", "pane", "list", "--window", &window]))?;
        let panes: serde_json::Value = serde_json::from_str(&json)?;
        let pane = panes["data"][0]["id"]
            .as_str()
            .context("fixture pane not found")?
            .to_owned();
        run(Command::new(OTTY_CLI).args([
            "pane",
            "split",
            "--pane",
            &pane,
            "--direction",
            direction,
            "--command",
            &command,
        ]))?;
    }
    pause(1000);
    if tabs {
        // Make it key again: new tabs can hand that to another window.
        run(Command::new("osascript").args([
            "-e",
            &format!("tell application \"Otty\" to set index of {target} to 1"),
        ]))?;
    }
    let result = (|| {
        let pid = run(Command::new("pgrep").args(["-x", "Otty"]))?;
        capture_focused(&id, pid.lines().next().unwrap_or_default().parse()?, out)
    })();
    let _ = run(Command::new(OTTY_CLI).args(["-y", "window", "close", "--force", &window]));
    result
}

const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleExecutable</key><string>Motty</string>
    <key>CFBundleIdentifier</key><string>dev.motty.Motty</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>Motty</string>
    <key>CFBundleDisplayName</key><string>Motty</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleIconFile</key><string>Motty</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
    <key>CFBundleVersion</key><string>{version}</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
    <key>CFBundleURLTypes</key>
    <array>
        <dict>
            <key>CFBundleURLName</key><string>Motty Link</string>
            <key>CFBundleURLSchemes</key><array><string>motty</string></array>
        </dict>
    </array>
    <key>NSAppleScriptEnabled</key><true/>
    <key>OSAScriptingDefinition</key><string>Motty.sdef</string>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key><string>Motty Recipe</string>
            <key>CFBundleTypeRole</key><string>Viewer</string>
            <key>LSHandlerRank</key><string>Owner</string>
            <key>CFBundleTypeExtensions</key><array><string>mottyrecipe</string></array>
        </dict>
        <dict>
            <key>CFBundleTypeName</key><string>Otty Recipe</string>
            <key>CFBundleTypeRole</key><string>Viewer</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>CFBundleTypeExtensions</key><array><string>ottyrecipe</string></array>
        </dict>
        <dict>
            <key>CFBundleTypeName</key><string>Otty Theme</string>
            <key>CFBundleTypeRole</key><string>Viewer</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>CFBundleTypeExtensions</key><array><string>ottytheme</string></array>
        </dict>
    </array>
</dict>
</plist>
"#;

/// Builds the app and wraps it in a minimal `.app` bundle, which macOS needs
/// to treat Motty as a regular app (activation, Dock, bundle id).
fn bundle(release: bool) -> Result<PathBuf> {
    let mut build = Command::new("cargo");
    build
        .args(["build", "--quiet", "-p", "motty-app", "-p", "motty-cli"])
        .current_dir(root());
    if release {
        build.arg("--release");
    }
    run(&mut build)?;
    let profile = root().join(if release {
        "target/release"
    } else {
        "target/debug"
    });
    let app = profile.join("Motty.app");
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos)?;
    std::fs::create_dir_all(app.join("Contents/Resources"))?;
    // Replace rather than overwrite: the kernel caches code signatures per
    // inode, and rewriting a signed binary in place gets it killed on launch.
    let exe = macos.join("Motty");
    let _ = std::fs::remove_file(&exe);
    std::fs::copy(profile.join("motty-app"), &exe)?;
    // The CLI can't sit beside it (`motty` and `Motty` are one name on a
    // case-insensitive disk); shells find it on PATH.
    let bin = app.join("Contents/Resources/bin");
    std::fs::create_dir_all(&bin)?;
    let cli = bin.join("motty");
    let _ = std::fs::remove_file(&cli);
    std::fs::copy(profile.join("motty"), &cli)?;
    icns(&profile, &app.join("Contents/Resources/Motty.icns"))?;
    // The AppleScript dictionary (OSAScriptingDefinition).
    std::fs::copy(
        root().join("crates/motty-app/resources/Motty.sdef"),
        app.join("Contents/Resources/Motty.sdef"),
    )?;
    let plist = INFO_PLIST.replace("{version}", env!("CARGO_PKG_VERSION"));
    std::fs::write(app.join("Contents/Info.plist"), plist)?;
    run(Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app))?;
    Ok(app)
}

/// Builds `out` from the 1024px icon (every size macOS asks for, via an
/// iconset), unless it's newer than the icon already.
fn icns(profile: &Path, out: &Path) -> Result<()> {
    let src = root().join("crates/motty-app/assets/icon/motty.png");
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    if modified(out).is_some_and(|built| modified(&src).is_some_and(|src| built >= src)) {
        return Ok(());
    }
    let set = profile.join("Motty.iconset");
    let _ = std::fs::remove_dir_all(&set);
    std::fs::create_dir_all(&set)?;
    for size in [16, 32, 128, 256, 512] {
        for (scale, suffix) in [(1, ""), (2, "@2x")] {
            let px = (size * scale).to_string();
            run(Command::new("sips")
                .args(["-z", &px, &px])
                .arg(&src)
                .arg("--out")
                .arg(set.join(format!("icon_{size}x{size}{suffix}.png")))
                .stdout(std::process::Stdio::null()))?;
        }
    }
    run(Command::new("iconutil")
        .args(["-c", "icns"])
        .arg(&set)
        .arg("-o")
        .arg(out))?;
    std::fs::remove_dir_all(&set)?;
    Ok(())
}

fn capture_motty(
    release: bool,
    fixture: &Path,
    tabs: bool,
    split: Option<&str>,
    theme: Option<&str>,
    out: &Path,
) -> Result<()> {
    let bin = bundle(release)?.join("Contents/MacOS/Motty");
    let (x, y, w, h) = BOUNDS;
    // Otty types the fixture into the user's zsh, so run it under zsh too
    // (the trailing `:` keeps zsh from exec-ing sleep in its place).
    let script = format!("{}; sleep 600; :", fixture.display());
    let mut motty = Command::new(bin);
    motty
        .args(["--bounds", &format!("{x},{y},{w},{h}")])
        .current_dir(root());
    if tabs {
        motty.args(["--extra-tabs", "2"]);
    }
    if let Some(direction) = split {
        motty.args(["--split", direction]);
    }
    for command in background_tabs() {
        motty.args(["--background", command]);
    }
    if let Some(theme) = theme {
        motty.args(["--theme", theme]);
    }
    let mut child: Child = motty.args(["-e", "/bin/zsh", "-c", &script]).spawn()?;
    let title = if tabs {
        tabs_title()
    } else {
        FIXTURE_TITLE.to_owned()
    };
    let result = (|| {
        let id = find_window("Motty", &title)?;
        pause(700);
        capture_focused(&id, child.id(), out)
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Makes the process frontmost so both apps are captured focused (cursor,
/// traffic lights). `activate` alone is unreliable for an unbundled binary
/// started from a background process.
/// Captures the window while its app is active. Frontmost can still leave the
/// window unkeyed now and then, so the minimize light (centered at 39,16) is
/// checked for color.
fn capture_focused(window_id: &str, pid: u32, out: &Path) -> Result<()> {
    pause(WAIT.get().copied().unwrap_or(0));
    for _ in 0..4 {
        bring_to_front(pid)?;
        order_backdrop_below(window_id)?;
        park_pointer()?;
        capture(window_id, out)?;
        let img = load(out)?;
        let s = scale_of(&img) as f32;
        // The close light is red only when the window is key (and present: a
        // saturated backdrop alone must not pass). Sampled above its center,
        // where the edited dot sits while a command runs.
        let p = img.get_pixel((16. * s) as u32, (12.5 * s) as u32);
        if p[0] > 200 && p[1] < 140 && p[2] < 140 {
            return Ok(());
        }
        pause(500);
    }
    bail!("window {window_id} never captured in its focused state")
}

/// `--hover x,y`: window-relative point the pointer rests on during
/// captures, to compare hover-only chrome. Unset, it parks outside.
static HOVER: std::sync::OnceLock<Option<(f32, f32)>> = std::sync::OnceLock::new();
/// `--wait MS`: extra time before each capture, for output that comes later.
static WAIT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
/// `--background CMD` (repeatable): tabs opened behind the fixture's.
static BACKGROUND: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

fn background_tabs() -> &'static [String] {
    BACKGROUND.get().map_or(&[], Vec::as_slice)
}

/// Moves the pointer just right of the window (inside the backdrop margin),
/// so hover-only chrome such as titlebar buttons stays hidden, or onto the
/// `--hover` point.
fn park_pointer() -> Result<()> {
    let (x, y, w, h) = BOUNDS;
    let (px, py) = HOVER
        .get()
        .copied()
        .flatten()
        .unwrap_or(((w + 20) as f32, (h / 2) as f32));
    run(Command::new(swift_tool("warp")?)
        .args([(x as f32 + px).to_string(), (y as f32 + py).to_string()]))?;
    pause(300);
    Ok(())
}

fn bring_to_front(pid: u32) -> Result<()> {
    let process = format!("(first process whose unix id is {pid})");
    for _ in 0..10 {
        let set =
            format!("tell application \"System Events\" to set frontmost of {process} to true");
        run(Command::new("osascript").args(["-e", &set]))?;
        pause(300);
        let get = format!("tell application \"System Events\" to get frontmost of {process}");
        if run(Command::new("osascript").args(["-e", &get]))?.trim() == "true" {
            // Let the window redraw in its key state.
            pause(400);
            return Ok(());
        }
    }
    bail!("could not bring pid {pid} to the front")
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
    let tabs = args.iter().any(|a| a == "--tabs");
    let split = args
        .iter()
        .position(|a| a == "--split")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);
    let theme = args
        .iter()
        .position(|a| a == "--theme")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let backdrop = args
        .iter()
        .position(|a| a == "--backdrop")
        .and_then(|i| args.get(i + 1));
    let hover = args
        .iter()
        .position(|a| a == "--hover")
        .and_then(|i| args.get(i + 1));
    let hover = hover.map(|v| {
        v.split_once(',')
            .and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)))
            .context("--hover wants x,y")
    });
    let _ = HOVER.set(hover.transpose()?);
    if let Some(ms) = args
        .iter()
        .position(|a| a == "--wait")
        .and_then(|i| args.get(i + 1))
    {
        let _ = WAIT.set(ms.parse().context("--wait wants milliseconds")?);
    }
    let _ = BACKGROUND.set(
        args.windows(2)
            .filter(|w| w[0] == "--background")
            .map(|w| w[1].clone())
            .collect(),
    );
    // `--set key=value` (repeatable) changes a config key in both apps.
    let sets: Vec<(String, String)> = args
        .windows(2)
        .filter(|w| w[0] == "--set")
        .map(|w| {
            w[1].split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .context("--set wants key=value")
        })
        .collect::<Result<_>>()?;
    let _backdrop = backdrop.map(|c| Backdrop::show(c)).transpose()?;
    let mut restore = (theme.is_some() || !sets.is_empty())
        .then(|| OttyConfig::apply(theme.as_deref(), &sets))
        .transpose()?;
    capture_otty(&fixture, tabs, split, &otty).context("capturing Otty")?;
    // Without `--set`, Motty gets the theme as a flag, so the user's config
    // can be back before Motty starts.
    if sets.is_empty() {
        restore = None;
    }
    capture_motty(release, &fixture, tabs, split, theme.as_deref(), &motty)
        .context("capturing Motty")?;
    drop(restore);
    println!("captured {} and {}", otty.display(), motty.display());
    diff(&otty, &motty, Some(&dir.join("diff.png")))?;
    for (name, img) in [("otty", &otty), ("motty", &motty)] {
        println!("--- {name} grid metrics");
        metrics(img, SIDEBAR_PT)?;
    }
    Ok(())
}

fn load(path: &Path) -> Result<RgbaImage> {
    Ok(image::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .to_rgba8())
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
        println!(
            "size mismatch: {:?} vs {:?}",
            a.dimensions(),
            b.dimensions()
        );
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
    let view = image::imageops::crop_imm(
        &img,
        (x * s) as u32,
        (y * s) as u32,
        (w * s) as u32,
        (h * s) as u32,
    );
    view.to_image().save(out)?;
    Ok(())
}

fn scan(path: &Path, axis: &str, pos: f32, threshold: f32) -> Result<()> {
    let img = load(path)?;
    let s = scale_of(&img);
    let fixed = (pos * s as f32) as u32;
    let len = if axis == "x" {
        img.height()
    } else {
        img.width()
    };
    let at = |i: u32| {
        if axis == "x" {
            *img.get_pixel(fixed, i)
        } else {
            *img.get_pixel(i, fixed)
        }
    };
    let mut prev = at(0);
    let mut out = Vec::new();
    for i in 1..len {
        let p = at(i);
        if distance(&p, &prev) > threshold {
            out.push(format!(
                "{:.1}:#{:02x}{:02x}{:02x}",
                i as f32 / s as f32,
                p[0],
                p[1],
                p[2]
            ));
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
    let ink_row =
        |y: u32| (x0..img.width() - 4 * s).any(|x| distance(img.get_pixel(x, y), &bg) > 60.);
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
    println!(
        "background #{:02x}{:02x}{:02x}, first ink x = {:.1}pt",
        bg[0],
        bg[1],
        bg[2],
        first_ink_x as f32 / s as f32
    );
    let shown: Vec<String> = runs
        .iter()
        .take(16)
        .map(|(a, b)| format!("{:.1}-{:.1}", *a as f32 / s as f32, *b as f32 / s as f32))
        .collect();
    println!("ink rows (pt): {}", shown.join(" "));
    Ok(())
}
