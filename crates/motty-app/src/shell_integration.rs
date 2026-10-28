//! Installs Motty's shell-integration scripts and the environment that makes
//! a shell load them: zsh through a `ZDOTDIR` bootstrap, fish through a
//! `vendor_conf.d` folder on `XDG_DATA_DIRS`. Both scripts undo the change
//! before the user's own startup files run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use motty_theme::Config;

const FILES: &[(&str, &str)] = &[
    (
        "zsh/.zshenv",
        include_str!("../assets/shell-integration/zsh/.zshenv"),
    ),
    (
        "zsh/motty.zsh",
        include_str!("../assets/shell-integration/zsh/motty.zsh"),
    ),
    (
        "fish/vendor_conf.d/motty.fish",
        include_str!("../assets/shell-integration/fish/vendor_conf.d/motty.fish"),
    ),
];

/// Writes the scripts once per run (only when they changed) and returns
/// their folder.
fn installed() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = dirs::cache_dir()?.join("motty/shell-integration");
        for (name, contents) in FILES {
            let path = dir.join(name);
            if std::fs::read_to_string(&path).ok().as_deref() == Some(contents) {
                continue;
            }
            let written = std::fs::create_dir_all(path.parent()?)
                .and_then(|_| std::fs::write(&path, contents));
            if let Err(e) = written {
                tracing::warn!("shell integration: can't write {}: {e}", path.display());
                return None;
            }
        }
        Some(dir)
    })
    .as_deref()
}

/// Environment for starting `shell` (a path or name) with integration, or
/// nothing when it's off, disabled for that shell, or not supported.
pub fn env(config: &Config, shell: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    let name = Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(shell)
        .trim_start_matches('-');
    if !config.shell_integration
        || config
            .shell_integration_disabled_shells
            .iter()
            .any(|s| s == name)
    {
        return env;
    }
    let Some(dir) = installed() else { return env };
    match name {
        "zsh" => {
            if let Ok(orig) = std::env::var("ZDOTDIR") {
                env.insert("MOTTY_ORIG_ZDOTDIR".into(), orig);
            }
            env.insert("ZDOTDIR".into(), dir.join("zsh").display().to_string());
        }
        "fish" => {
            let orig = std::env::var("XDG_DATA_DIRS").ok();
            let rest = orig
                .clone()
                .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
            env.insert("XDG_DATA_DIRS".into(), format!("{}:{rest}", dir.display()));
            if let Some(orig) = orig {
                env.insert("MOTTY_ORIG_XDG_DATA_DIRS".into(), orig);
            }
        }
        _ => return env,
    }
    env.insert("MOTTY_SHELL_INTEGRATION".into(), dir.display().to_string());
    env
}

/// The shell a session starts when no command is given: `shell` from the
/// config, else the login shell.
pub fn login_shell(config: &Config) -> String {
    config
        .shell
        .clone()
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/zsh".into())
}
