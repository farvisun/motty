//! Updates from GitHub releases: a daily check (`check-for-updates`) and
//! Motty › Check for Updates…. Installing downloads Motty.dmg, checks its
//! SHA-256 and its Ed25519 signature (made in the release workflow with the
//! key whose public half is [`PUBLIC_KEY`]), swaps the app bundle in place and
//! restarts; the running copy keeps working until then.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, VerifyingKey};
use gpui::{App, AsyncApp, PromptLevel, WindowHandle};
use sha2::{Digest, Sha256};

use crate::workspace::Workspace;

const LATEST: &str = "https://api.github.com/repos/farvisun/motty/releases/latest";
const ASSET: &str = "Motty.dmg";
const SUMS: &str = "SHA256SUMS.txt";
const SIGNATURE: &str = "Motty.dmg.sig";
/// Sparkle-format EdDSA key (`generate_keys --account motty`, login Keychain);
/// the private half is the MOTTY_ED_PRIVATE_KEY secret of the release workflow.
const PUBLIC_KEY: &str = "GNDDdFMCrQGu2suZq/EiE7BKhgbtl34DbuDejoF+22k=";
const FIRST_CHECK: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq)]
struct Release {
    version: String,
    notes: String,
    page: String,
    dmg: String,
    /// From the asset's digest, or SHA256SUMS.txt.
    sha256: Option<String>,
    /// Motty.dmg.sig: the base64 Ed25519 signature of Motty.dmg.
    signature: Option<String>,
}

/// The daily check. Only an installed bundle checks; `cargo run` doesn't.
pub fn start(cx: &mut App) {
    if app_bundle().is_none() {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(FIRST_CHECK).await;
        // Asked about once per run; a new version asks again.
        let mut offered = String::new();
        loop {
            if crate::load_config().check_for_updates
                && let Ok(Some(release)) = fetch(cx).await
                && release.version != offered
            {
                offered = release.version.clone();
                offer(release, cx).await;
            }
            cx.background_executor().timer(INTERVAL).await;
        }
    })
    .detach();
}

/// Motty › Check for Updates…: says so when there's nothing new.
pub fn check_now(cx: &mut App) {
    cx.spawn(async move |cx| match fetch(cx).await {
        Ok(Some(release)) => offer(release, cx).await,
        Ok(None) => {
            let detail = format!("Motty {} is the newest version.", env!("CARGO_PKG_VERSION"));
            let _ = ask(cx, PromptLevel::Info, "You're up to date", &detail, &["OK"]).await;
        }
        Err(e) => {
            let _ = ask(
                cx,
                PromptLevel::Warning,
                "Couldn't check for updates",
                &e,
                &["OK"],
            )
            .await;
        }
    })
    .detach();
}

async fn fetch(cx: &mut AsyncApp) -> Result<Option<Release>, String> {
    cx.background_executor()
        .spawn(async move {
            let json = curl(&[LATEST, "-H", "Accept: application/vnd.github+json"])?;
            let release = parse_release(&json)?;
            if !newer(&release.version, env!("CARGO_PKG_VERSION")) {
                return Ok(None);
            }
            Ok(Some(release))
        })
        .await
}

async fn offer(release: Release, cx: &mut AsyncApp) {
    let bundle = app_bundle();
    let detail = format!(
        "You have {}.{}\n\n{}",
        env!("CARGO_PKG_VERSION"),
        if bundle.is_some() {
            ""
        } else {
            " Motty isn't running from an app bundle, so it can't update itself."
        },
        excerpt(&release.notes, 900)
    );
    let title = format!("Motty {} is available", release.version);
    let buttons: &[&str] = if bundle.is_some() {
        &["Install and Restart", "Release Notes", "Later"]
    } else {
        &["Release Notes", "Later"]
    };
    let Some(answer) = ask(cx, PromptLevel::Info, &title, &detail, buttons).await else {
        return;
    };
    match (buttons[answer], bundle) {
        ("Install and Restart", Some(bundle)) => install(release, bundle, cx).await,
        ("Release Notes", _) => {
            cx.update(|cx| cx.open_url(&release.page));
        }
        _ => {}
    }
}

async fn install(release: Release, bundle: PathBuf, cx: &mut AsyncApp) {
    let target = bundle.clone();
    let done = cx
        .background_executor()
        .spawn(async move { replace_bundle(&release, &target) })
        .await;
    if let Err(e) = done {
        tracing::error!("update failed: {e}");
        let _ = ask(
            cx,
            PromptLevel::Critical,
            "Motty couldn't update",
            &e,
            &["OK"],
        )
        .await;
        return;
    }
    relaunch_after_exit(&bundle);
    cx.update(|cx| cx.quit());
}

/// Downloads, verifies and swaps the bundle. Blocking; runs off the main thread.
fn replace_bundle(release: &Release, bundle: &Path) -> Result<(), String> {
    let expected = release
        .sha256
        .clone()
        .ok_or("The release lists no SHA-256 for Motty.dmg, so the download can't be checked.")?;
    let tmp = std::env::temp_dir().join(format!("motty-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = (|| {
        let dmg = tmp.join(ASSET);
        curl(&[&release.dmg, "--max-time", "900", "-o", path_str(&dmg)?])?;
        let actual = sha256_file(&dmg)?;
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(format!(
                "The download doesn't match the release's SHA-256 ({actual}).",
            ));
        }
        let signature = curl(&[release
            .signature
            .as_deref()
            .ok_or("The release has no signature for Motty.dmg, so it can't be trusted.")?])?;
        let bytes = std::fs::read(&dmg).map_err(|e| e.to_string())?;
        verify(&bytes, &signature, PUBLIC_KEY)?;
        let mount = tmp.join("mnt");
        run(
            "hdiutil",
            &[
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-mountpoint",
                path_str(&mount)?,
                path_str(&dmg)?,
            ],
        )?;
        let staged = bundle.with_extension("app.new");
        let _ = std::fs::remove_dir_all(&staged);
        let copied = run(
            "ditto",
            &[path_str(&mount.join("Motty.app"))?, path_str(&staged)?],
        );
        let _ = run("hdiutil", &["detach", "-quiet", path_str(&mount)?]);
        copied?;
        let _ = run(
            "xattr",
            &["-dr", "com.apple.quarantine", path_str(&staged)?],
        );
        if let Err(e) = run(
            "codesign",
            &["--verify", "--deep", "--strict", path_str(&staged)?],
        ) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(format!("The new app's signature doesn't verify: {e}"));
        }
        swap(bundle, &staged)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Moves the new bundle into place, putting the old one back if that fails.
fn swap(bundle: &Path, staged: &Path) -> Result<(), String> {
    let old = bundle.with_extension("app.old");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(bundle, &old)
        .map_err(|e| format!("Couldn't move {}: {e}", bundle.display()))?;
    if let Err(e) = std::fs::rename(staged, bundle) {
        let _ = std::fs::rename(&old, bundle);
        return Err(format!("Couldn't put the new app in place: {e}"));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Opens the bundle again once this process is gone (gives up after ten
/// minutes, in case quitting was cancelled).
fn relaunch_after_exit(bundle: &Path) {
    let script = "i=0; while kill -0 \"$0\" 2>/dev/null && [ $i -lt 3000 ]; do sleep 0.2; i=$((i+1)); done; \
                  kill -0 \"$0\" 2>/dev/null || open \"$1\"";
    let _ = Command::new("/bin/sh")
        .args(["-c", script, &std::process::id().to_string()])
        .arg(bundle)
        .spawn();
}

/// A prompt on the frontmost Motty window; `None` without one or when dismissed.
async fn ask(
    cx: &mut AsyncApp,
    level: PromptLevel,
    title: &str,
    detail: &str,
    buttons: &[&str],
) -> Option<usize> {
    let answer = cx.update(|cx| {
        let window: WindowHandle<Workspace> = cx
            .window_stack()
            .into_iter()
            .flatten()
            .chain(cx.windows())
            .find_map(|w| w.downcast::<Workspace>())?;
        window
            .update(cx, |_, window, cx| {
                window.prompt(level, title, Some(detail), buttons, cx)
            })
            .ok()
    })?;
    answer.await.ok()
}

/// `/Applications/Motty.app` when running from an app bundle.
fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

fn parse_release(json: &str) -> Result<Release, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Unexpected reply from GitHub: {e}"))?;
    let tag = v["tag_name"]
        .as_str()
        .ok_or("GitHub's reply has no release tag.")?;
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let asset = |name: &str| assets.iter().find(|a| a["name"].as_str() == Some(name));
    let dmg = asset(ASSET).ok_or("The latest release has no Motty.dmg.")?;
    let mut sha256 = dmg["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(str::to_owned);
    if sha256.is_none()
        && let Some(url) = asset(SUMS).and_then(|a| a["browser_download_url"].as_str())
        && let Ok(sums) = curl(&[url])
    {
        sha256 = sum_for(&sums, ASSET);
    }
    Ok(Release {
        version: tag.trim_start_matches('v').to_owned(),
        notes: v["body"].as_str().unwrap_or_default().to_owned(),
        page: v["html_url"]
            .as_str()
            .unwrap_or("https://github.com/farvisun/motty/releases/latest")
            .to_owned(),
        dmg: dmg["browser_download_url"]
            .as_str()
            .ok_or("Motty.dmg has no download link.")?
            .to_owned(),
        sha256,
        signature: asset(SIGNATURE)
            .and_then(|a| a["browser_download_url"].as_str())
            .map(str::to_owned),
    })
}

/// Checks a Sparkle-style signature: base64 Ed25519 over the whole file.
fn verify(data: &[u8], signature: &str, public_key: &str) -> Result<(), String> {
    let bad = "The download's signature doesn't match Motty's release key.";
    let key: [u8; 32] = BASE64
        .decode(public_key)
        .ok()
        .and_then(|k| k.try_into().ok())
        .ok_or("Motty's release key is malformed.")?;
    let sig: [u8; 64] = BASE64
        .decode(signature.trim())
        .ok()
        .and_then(|s| s.try_into().ok())
        .ok_or(bad)?;
    VerifyingKey::from_bytes(&key)
        .map_err(|_| "Motty's release key is malformed.")?
        .verify_strict(data, &Signature::from_bytes(&sig))
        .map_err(|_| bad.to_owned())
}

/// The hash for `name` in `shasum -a 256` output.
fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file.rsplit('/').next() == Some(name)).then(|| hash.to_owned())
    })
}

/// `a` is a later version than `b` (dotted numbers, pre-release ignored).
fn newer(a: &str, b: &str) -> bool {
    let parts = |s: &str| -> Vec<u64> {
        s.split(['-', '+'])
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// The start of the release notes, cut at a line break.
fn excerpt(notes: &str, max: usize) -> String {
    let notes = notes.trim();
    if notes.len() <= max {
        return notes.to_owned();
    }
    let mut end = max;
    while !notes.is_char_boundary(end) {
        end -= 1;
    }
    let cut = notes[..end].rfind('\n').unwrap_or(end);
    format!("{}\n…", notes[..cut].trim_end())
}

fn curl(args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("/usr/bin/curl");
    cmd.args([
        "-fsSL",
        "--connect-timeout",
        "15",
        "--max-time",
        "60",
        "-A",
        concat!("Motty/", env!("CARGO_PKG_VERSION")),
    ]);
    // GUI apps don't see the shell's proxy variables; use the system proxy.
    if std::env::var_os("https_proxy").is_none()
        && std::env::var_os("HTTPS_PROXY").is_none()
        && let Some(proxy) = system_proxy()
    {
        cmd.args(["--proxy", &proxy]);
    }
    let out = cmd.args(args).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if err.is_empty() {
            "The download failed.".into()
        } else {
            err
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The HTTPS (or SOCKS) proxy from System Settings, as a curl `--proxy` value.
fn system_proxy() -> Option<String> {
    let out = Command::new("/usr/sbin/scutil")
        .arg("--proxy")
        .output()
        .ok()?;
    parse_scutil_proxy(&String::from_utf8_lossy(&out.stdout))
}

fn parse_scutil_proxy(text: &str) -> Option<String> {
    let get = |key: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once(" : ")?;
            (k.trim() == key).then(|| v.trim().to_owned())
        })
    };
    if get("HTTPSEnable").as_deref() == Some("1") {
        return Some(format!(
            "http://{}:{}",
            get("HTTPSProxy")?,
            get("HTTPSPort")?
        ));
    }
    if get("SOCKSEnable").as_deref() == Some("1") {
        return Some(format!(
            "socks5h://{}:{}",
            get("SOCKSProxy")?,
            get("SOCKSPort")?
        ));
    }
    None
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!(
        "{program} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn path_str(p: &Path) -> Result<&str, String> {
    p.to_str()
        .ok_or_else(|| format!("Path isn't UTF-8: {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(newer("0.3.0", "0.2.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("1.0", "0.9.9"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.2.0-beta", "0.2.0"));
        assert!(!newer("0.1.9", "0.2.0"));
    }

    #[test]
    fn reads_a_release() {
        let json = r#"{"tag_name":"v0.3.0","body":"notes","html_url":"https://x/r",
            "assets":[{"name":"Motty.dmg","browser_download_url":"https://x/Motty.dmg","digest":"sha256:abc"}]}"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(r.dmg, "https://x/Motty.dmg");
        assert_eq!(r.sha256.as_deref(), Some("abc"));
        assert_eq!(r.signature, None);
        assert!(parse_release(r#"{"tag_name":"v1","assets":[]}"#).is_err());
    }

    #[test]
    fn verifies_sparkle_signatures() {
        // `sign_update -p` over "test\n" with the release key.
        let sig = "+qb9KPxu2CQtLN1ASnnfdWi/BHPKtjyy5YKep+ZNdGoQ4f7zcuNtBN11mvs8rmNjmx9WkXaAKpE7db0cCJshCw==";
        assert!(verify(b"test\n", sig, PUBLIC_KEY).is_ok());
        assert!(verify(b"tampered\n", sig, PUBLIC_KEY).is_err());
        assert!(verify(b"test\n", "not base64", PUBLIC_KEY).is_err());
        assert!(verify(b"test\n", sig, "AAAA").is_err());
    }

    #[test]
    fn finds_a_sum() {
        let sums = "111  dist/Motty-0.3.0-universal.dmg\n222  dist/Motty.dmg\n";
        assert_eq!(sum_for(sums, "Motty.dmg").as_deref(), Some("222"));
        assert_eq!(
            sum_for("333 *Motty.dmg", "Motty.dmg").as_deref(),
            Some("333")
        );
        assert_eq!(sum_for(sums, "Other.dmg"), None);
    }

    #[test]
    fn reads_the_system_proxy() {
        let https =
            "<dictionary> {\n  HTTPSEnable : 1\n  HTTPSPort : 7779\n  HTTPSProxy : 127.0.0.1\n}";
        assert_eq!(
            parse_scutil_proxy(https).as_deref(),
            Some("http://127.0.0.1:7779")
        );
        let socks =
            "  HTTPSEnable : 0\n  SOCKSEnable : 1\n  SOCKSPort : 1080\n  SOCKSProxy : localhost\n";
        assert_eq!(
            parse_scutil_proxy(socks).as_deref(),
            Some("socks5h://localhost:1080")
        );
        assert_eq!(parse_scutil_proxy("  HTTPEnable : 0\n"), None);
    }

    #[test]
    fn cuts_long_notes_at_a_line() {
        let notes = "first line\nsecond line\nthird";
        assert_eq!(excerpt(notes, 15), "first line\n…");
        assert_eq!(excerpt("short", 15), "short");
    }
}
