//! Puts the git commit the CLI is built from in `MOTTY_GIT_HASH` (empty
//! outside a checkout) and the version with it in `MOTTY_VERSION`.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    let hash = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=MOTTY_GIT_HASH={hash}");
    // `0.1.0 (1a2b3c4)`, for `--version`.
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let long = if hash.is_empty() {
        version
    } else {
        format!("{version} ({hash})")
    };
    println!("cargo:rustc-env=MOTTY_VERSION={long}");
    // A new commit (or a branch switch) builds again.
    for path in ["HEAD", "logs/HEAD"] {
        if let Some(file) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={file}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
