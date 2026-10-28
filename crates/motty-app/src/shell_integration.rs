//! Installs Motty's shell-integration scripts and the environment that makes
//! a shell load them: zsh through a `ZDOTDIR` bootstrap, fish through a
//! `vendor_conf.d` folder on `XDG_DATA_DIRS`, both undone before the user's
//! own startup files run. bash is started with `--rcfile` instead, and the
//! script runs the login startup files itself.

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
        "bash/motty.bash",
        include_str!("../assets/shell-integration/bash/motty.bash"),
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

/// How to start the user's shell with integration: extra environment, and
/// for bash the command itself. Nothing when integration is off, disabled
/// for that shell, or not supported.
#[derive(Default)]
pub struct Launch {
    pub env: HashMap<String, String>,
    pub command: Option<(String, Vec<String>)>,
}

pub fn launch(config: &Config) -> Launch {
    let shell = login_shell(config);
    let mut launch = Launch::default();
    let env = &mut launch.env;
    let name = Path::new(&shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&shell)
        .trim_start_matches('-');
    if !config.shell_integration
        || config
            .shell_integration_disabled_shells
            .iter()
            .any(|s| s == name)
    {
        return launch;
    }
    let Some(dir) = installed() else {
        return launch;
    };
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
        "bash" => {
            let rcfile = dir.join("bash/motty.bash").display().to_string();
            launch.command = Some((shell.clone(), vec!["--rcfile".into(), rcfile]));
        }
        _ => return launch,
    }
    launch
        .env
        .insert("MOTTY_SHELL_INTEGRATION".into(), dir.display().to_string());
    launch
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
