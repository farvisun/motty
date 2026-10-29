//! `motty font list / apply / import`: the fonts Motty can use, the macOS
//! ones and those in `~/.config/motty/fonts/`, which the app registers at
//! launch and on config reload.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, anyhow, bail};
use clap::Subcommand;
use motty_theme::config::{motty_dir, otty_dir};
use motty_theme::store::Files;
use serde_json::{Value as Json, json};

use crate::config_cmd::print_json;
use crate::themes::{download, is_url};

#[derive(Subcommand)]
pub enum FontCommand {
    /// Font families, one per line.
    List {
        /// Families whose name contains this.
        #[arg(long)]
        family: Option<String>,
        /// Monospace families only.
        #[arg(long)]
        monospace: bool,
        /// Only fonts installed system-wide.
        #[arg(long, conflicts_with = "user")]
        system: bool,
        /// Only fonts you added (your Fonts folder, Motty's fonts folder).
        #[arg(long)]
        user: bool,
    },
    /// Use a family as the terminal font (writes font-family).
    Apply { family: String },
    /// Copy a font file (or URL) into ~/.config/motty/fonts/.
    Import {
        file: String,
        /// Also use it as the terminal font.
        #[arg(long)]
        apply: bool,
    },
}

/// The file kinds Motty registers.
pub const EXTENSIONS: [&str; 4] = ["ttf", "otf", "ttc", "otc"];

pub fn is_font(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The bundled family, always there in the app.
const BUNDLED: &str = "JetBrains Mono";

#[derive(Default)]
struct Family {
    monospace: bool,
    user: bool,
    system: bool,
}

fn source_path(source: &fontdb::Source) -> Option<&Path> {
    match source {
        fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => Some(p),
        fontdb::Source::Binary(_) => None,
    }
}

/// Every family: the system's, the user's and Motty's fonts folders.
fn families() -> BTreeMap<String, Family> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    for dir in [motty_dir(), otty_dir()] {
        db.load_fonts_dir(dir.join("fonts"));
    }
    let home = dirs::home_dir();
    let mut out: BTreeMap<String, Family> = BTreeMap::new();
    for face in db.faces() {
        let Some((name, _)) = face.families.first() else {
            continue;
        };
        // `.SF NS` and friends are private to macOS.
        if name.starts_with('.') {
            continue;
        }
        let user = source_path(&face.source)
            .zip(home.as_deref())
            .is_some_and(|(p, home)| p.starts_with(home));
        let f = out.entry(name.clone()).or_default();
        f.monospace |= face.monospaced;
        f.user |= user;
        f.system |= !user;
    }
    out.entry(BUNDLED.into()).or_insert(Family {
        monospace: true,
        user: false,
        system: true,
    });
    out
}

pub fn run(command: &FontCommand, json: bool) -> anyhow::Result<ExitCode> {
    match command {
        FontCommand::List {
            family,
            monospace,
            system,
            user,
        } => {
            let needle = family.as_deref().map(str::to_lowercase);
            let list: Vec<(String, Family)> = families()
                .into_iter()
                .filter(|(name, f)| {
                    needle
                        .as_ref()
                        .is_none_or(|n| name.to_lowercase().contains(n))
                        && (!monospace || f.monospace)
                        && (!system || f.system)
                        && (!user || f.user)
                })
                .collect();
            if json {
                let data: Vec<Json> = list
                    .iter()
                    .map(|(name, f)| {
                        json!({
                            "family": name,
                            "monospace": f.monospace,
                            "source": if f.user { "user" } else { "system" },
                        })
                    })
                    .collect();
                print_json(json!({ "data": data }));
            } else {
                for (name, _) in list {
                    println!("{name}");
                }
            }
        }
        FontCommand::Apply { family } => {
            let name = apply(family)?;
            if !json {
                println!("font-family = {name:?}");
            }
        }
        FontCommand::Import { file, apply } => {
            import(file, *apply, json)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Writes `family` (as the font spells it) to font-family.
fn apply(family: &str) -> anyhow::Result<String> {
    let name = families()
        .into_keys()
        .find(|n| n.eq_ignore_ascii_case(family))
        .ok_or_else(|| anyhow!("no font family named {family:?} (`motty font list` shows them)"))?;
    Files::current().set("font-family", &[toml::Value::String(name.clone())], false)?;
    Ok(name)
}

/// The family names in a font file.
fn families_in(path: &Path) -> anyhow::Result<Vec<String>> {
    let mut db = fontdb::Database::new();
    db.load_font_file(path)
        .with_context(|| format!("can't read {}", path.display()))?;
    let mut names: Vec<String> = db
        .faces()
        .filter_map(|f| f.families.first().map(|(n, _)| n.clone()))
        .collect();
    names.dedup();
    if names.is_empty() {
        bail!("{} isn't a font Motty can read", path.display());
    }
    Ok(names)
}

/// Copies a font file into Motty's fonts folder; returns where it went.
pub fn import(source: &str, apply_it: bool, json: bool) -> anyhow::Result<PathBuf> {
    let download = is_url(source)
        .then(|| download(source, "font.ttf"))
        .transpose()?;
    let path: &Path = match &download {
        Some(d) => &d.path,
        None => Path::new(source),
    };
    if !is_font(path) {
        bail!("{} isn't a .ttf, .otf, .ttc or .otc file", path.display());
    }
    let names = families_in(path)?;
    let dir = motty_dir().join("fonts");
    std::fs::create_dir_all(&dir)?;
    let file_name = path.file_name().context("no file name")?;
    let to = dir.join(file_name);
    std::fs::copy(path, &to).with_context(|| format!("can't copy to {}", to.display()))?;
    if apply_it {
        Files::current().set(
            "font-family",
            &[toml::Value::String(names[0].clone())],
            false,
        )?;
    }
    if json {
        print_json(json!({ "data": {
            "path": to,
            "families": names,
            "applied": apply_it,
        }}));
    } else {
        println!("Imported {} to {}", names.join(", "), to.display());
        if apply_it {
            println!("font-family = {:?}", names[0]);
        }
    }
    Ok(to)
}
