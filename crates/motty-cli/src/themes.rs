//! `motty theme list / import`: the installed themes, and theme files from
//! Otty, iTerm2, Kitty, Alacritty or Ghostty (a path or a URL).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, anyhow, bail};
use clap::{Subcommand, ValueEnum};
use motty_theme::config::{find_theme, list_themes, user_themes_dir};
use motty_theme::import::{self, Format};
use motty_theme::store::Files;
use motty_theme::{Mode, ParsedTheme, Rgba};
use serde_json::{Value as Json, json};

use crate::config_cmd::print_json;

#[derive(Subcommand)]
pub enum ThemeCommand {
    /// The installed themes: built-in and your own.
    List {
        #[arg(long, value_enum, default_value = "all")]
        color: ColorFilter,
    },
    /// Add a theme file (or URL) to your themes, converting it if needed.
    Import {
        source: String,
        /// Switch to it.
        #[arg(long)]
        activate: bool,
        /// Replace a theme of the same name instead of adding `-1`.
        #[arg(long)]
        overwrite: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorFilter {
    Dark,
    Light,
    All,
}

pub fn run(command: &ThemeCommand, json: bool) -> anyhow::Result<ExitCode> {
    match command {
        ThemeCommand::List { color } => list(*color, json),
        ThemeCommand::Import {
            source,
            activate,
            overwrite,
        } => {
            import(source, *activate, *overwrite, json)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `#rrggbb`.
fn hex(c: Rgba) -> String {
    import::hex(c).to_lowercase()
}

fn variant(mode: Mode) -> &'static str {
    match mode {
        Mode::Dark => "dark",
        Mode::Light => "light",
    }
}

/// The preview colors a launcher draws swatches with.
pub fn preview(parsed: &ParsedTheme) -> Json {
    let t = &parsed.theme;
    json!({
        "background": hex(t.terminal.background),
        "foreground": hex(t.terminal.foreground),
        "surface": hex(t.panel.surface),
        "accent": hex(t.token.accent),
        "secondary": hex(t.token.secondary),
        "tertiary": hex(t.token.tertiary),
    })
}

fn list(filter: ColorFilter, json: bool) {
    let themes: Vec<_> = list_themes()
        .into_iter()
        .filter(|(_, mode, _)| match filter {
            ColorFilter::All => true,
            ColorFilter::Dark => *mode == Mode::Dark,
            ColorFilter::Light => *mode == Mode::Light,
        })
        .collect();
    if !json {
        for (name, mode, path) in &themes {
            let source = if path.is_some() { "user" } else { "builtin" };
            println!("{name}\t{}\t{source}", variant(*mode));
        }
        return;
    }
    let data: Vec<Json> = themes
        .iter()
        .map(|(name, mode, path)| {
            let parsed = path
                .as_ref()
                .and_then(|p| motty_theme::theme::parse(&std::fs::read_to_string(p).ok()?).ok())
                .or_else(|| find_theme(name));
            json!({
                "name": name,
                "variant": variant(*mode),
                "source": if path.is_some() { "user" } else { "builtin" },
                "path": path,
                "colors": parsed.as_ref().map(preview),
            })
        })
        .collect();
    print_json(json!({ "data": data }));
}

/// A URL's file, downloaded with curl into a new temporary folder (the
/// name is kept: it tells the formats apart). The folder goes when the
/// returned guard drops.
pub struct Download {
    pub path: PathBuf,
    dir: PathBuf,
}

impl Drop for Download {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn is_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

pub fn download(url: &str, fallback_name: &str) -> anyhow::Result<Download> {
    let name = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit('/').next())
        .filter(|n| !n.is_empty() && !n.contains(':'))
        .unwrap_or(fallback_name)
        .to_owned();
    let dir = std::env::temp_dir().join(format!(
        "motty-download-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    std::fs::create_dir_all(&dir)?;
    let guard = Download {
        path: dir.join(&name),
        dir,
    };
    let status = std::process::Command::new("curl")
        .args(["-fsSL", "--proto", "=http,https", "-o"])
        .arg(&guard.path)
        .arg(url)
        .status()
        .context("couldn't run curl")?;
    if !status.success() {
        bail!("couldn't download {url} (curl exited with {status})");
    }
    Ok(guard)
}

/// Imports a theme file (or URL); returns its name.
pub fn import(source: &str, activate: bool, overwrite: bool, json: bool) -> anyhow::Result<String> {
    let download = is_url(source)
        .then(|| download(source, "theme"))
        .transpose()?;
    let path: &Path = match &download {
        Some(d) => &d.path,
        None => Path::new(source),
    };
    let bytes = std::fs::read(path).with_context(|| format!("can't read {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let format = Format::detect(path, &text);
    let imported = import::convert(format, path, &text).map_err(|e| anyhow!("{e}"))?;
    let dir = user_themes_dir();
    let saved = if overwrite {
        // In place: the file of a user theme with that name, else its slug.
        let existing = list_themes()
            .into_iter()
            .find(|(n, ..)| n.eq_ignore_ascii_case(&imported.name))
            .and_then(|(.., p)| p)
            .filter(|p| p.starts_with(&dir));
        let path = existing
            .unwrap_or_else(|| dir.join(format!("{}.ottytheme", import::slug(&imported.name))));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, &imported.source)?;
        path
    } else {
        import::save(&imported, &dir)?
    };
    let mut slot = None;
    if activate {
        // The dark slot when the theme is dark and a separate one is set,
        // as the theme picker does.
        let files = Files::current();
        let key = if imported.mode == Mode::Dark && !files.values("theme-dark").is_empty() {
            "theme-dark"
        } else {
            "theme"
        };
        files.set(key, &[toml::Value::String(imported.name.clone())], false)?;
        slot = Some(key);
    }
    if json {
        print_json(json!({ "data": {
            "name": imported.name,
            "variant": variant(imported.mode),
            "format": format.name(),
            "path": saved,
            "activated": slot,
        }}));
    } else {
        println!(
            "Imported {} ({} theme from {}) to {}",
            imported.name,
            variant(imported.mode),
            format.name(),
            saved.display()
        );
        if let Some(key) = slot {
            println!("Set {key} = {:?}", imported.name);
        }
    }
    Ok(imported.name)
}
