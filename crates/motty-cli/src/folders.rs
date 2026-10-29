//! `motty jump`, `motty learn` and `motty ignore`: the frecency-ranked
//! folders the app keeps (what Open Quickly's Folders filter shows).

use std::process::ExitCode;

use motty_ipc::{FolderParams, JumpParams, JumpResult, PaneRef, method};
use serde_json::Value;

use crate::output::{Ctx, own_pane};

/// With `shell_cd` (the shell integration's `motty` function), prints
/// `cd:<path>` for the shell to change to itself; with `no_cd`, just the
/// path; otherwise Motty types `cd <path>` into the pane.
pub fn jump(
    ctx: &Ctx,
    query: Option<String>,
    no_cd: bool,
    shell_cd: bool,
) -> anyhow::Result<ExitCode> {
    let params = JumpParams {
        pane: PaneRef::id(own_pane()),
        query,
        cwd: std::env::current_dir()
            .ok()
            .map(|d| d.display().to_string()),
        cd: !no_cd && !shell_cd,
    };
    let result: JumpResult = ctx.connect()?.call(method::JUMP, &params)?;
    if shell_cd && !no_cd {
        println!("cd:{}", result.path);
    } else if no_cd || ctx.json {
        ctx.print(&result, || println!("{}", result.path));
    }
    Ok(ExitCode::SUCCESS)
}

/// `learn` a folder (default: here), or with `ignore`, drop it for good.
pub fn learn(ctx: &Ctx, path: Option<String>, ignore: bool) -> anyhow::Result<ExitCode> {
    let path = match path {
        Some(p) => std::path::absolute(p)?,
        None => std::env::current_dir()?,
    };
    if !ignore {
        anyhow::ensure!(
            path.is_dir(),
            "{} isn't a folder (Motty learns folders only)",
            path.display()
        );
    }
    let params = FolderParams {
        pane: PaneRef::default(),
        path: Some(path.display().to_string()),
    };
    let method = if ignore {
        method::IGNORE
    } else {
        method::LEARN
    };
    let out: Value = ctx.connect()?.call(method, &params)?;
    ctx.print(&out, || {});
    Ok(ExitCode::SUCCESS)
}
