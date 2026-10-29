//! `motty config`: read, change, check and reload config.toml and
//! config.local.toml, the way Otty's `otty config` does.

use std::process::ExitCode;

use anyhow::{Context, anyhow, bail};
use clap::Subcommand;
use motty_ipc::{Client, ConfigSetParams, method};
use motty_theme::schema::{self, KeySpec, Kind, List};
use motty_theme::store::{self, Files, Origin};
use serde_json::{Value as Json, json};
use toml::Value;

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Print a key's value; a list prints one entry per line.
    Get {
        key: String,
        /// Also print the file and line it comes from.
        #[arg(long)]
        show_origin: bool,
        /// Only what config.local.toml sets.
        #[arg(long)]
        local: bool,
    },
    /// Set a key. A list takes its whole new value, or signed entries:
    /// `+NAME=value` adds or replaces, `-NAME` removes.
    Set {
        key: String,
        #[arg(required = true, num_args = 1.., allow_hyphen_values = true)]
        values: Vec<String>,
        /// Write it to config.local.toml (this machine only).
        #[arg(long)]
        local: bool,
        /// Also reload the running app.
        #[arg(long)]
        reload: bool,
        /// Change the running app only; nothing is written.
        #[arg(long)]
        transient: bool,
    },
    /// Back to the default: commented out in config.toml and removed from
    /// config.local.toml.
    Unset {
        key: String,
        /// Only remove it from config.local.toml (config.toml's value
        /// applies again).
        #[arg(long)]
        local: bool,
    },
    /// Open the config in $EDITOR (else the default text editor).
    Edit {
        /// config.local.toml, created if needed.
        #[arg(long)]
        local: bool,
    },
    /// Print the effective config: every key, defaults filled in.
    Show,
    /// Print the config file's path.
    Path {
        #[arg(long)]
        local: bool,
    },
    /// Every key Motty accepts: type, default, allowed values, where
    /// Settings shows it.
    Schema,
    /// Check the config files; exits 1 when something is wrong.
    Validate,
    /// Make the running app read the config files again.
    Reload,
}

/// The flags `set` takes, pulled out of its values: a value list that
/// allows `-NAME` would otherwise swallow them.
fn split_flags(values: &[String]) -> (Vec<String>, [bool; 3]) {
    let mut flags = [false; 3];
    let mut rest = Vec::new();
    for v in values {
        match v.as_str() {
            "--local" => flags[0] = true,
            "--reload" => flags[1] = true,
            "--transient" => flags[2] = true,
            _ => rest.push(v.clone()),
        }
    }
    (rest, flags)
}

pub fn run(command: &ConfigCommand, json: bool) -> anyhow::Result<ExitCode> {
    let files = Files::current();
    match command {
        ConfigCommand::Get {
            key,
            show_origin,
            local,
        } => return get(&files, spec(key)?, *show_origin, *local, json),
        ConfigCommand::Set {
            key,
            values,
            local,
            reload,
            transient,
        } => {
            let (values, [l, r, t]) = split_flags(values);
            if values.is_empty() {
                bail!("no value given");
            }
            let (local, reload, transient) = (*local || l, *reload || r, *transient || t);
            if transient && local {
                bail!("--transient writes nothing, so --local doesn't apply");
            }
            set(&files, spec(key)?, &values, local, reload, transient, json)?;
        }
        ConfigCommand::Unset { key, local } => {
            let spec = spec(key)?;
            files.unset(spec.key, *local)?;
        }
        ConfigCommand::Edit { local } => edit(&files, *local)?,
        ConfigCommand::Show => show(&files, json),
        ConfigCommand::Path { local } => {
            let path = if *local { &files.local } else { &files.main };
            if json {
                print_json(json!({ "data": { "path": path } }));
            } else {
                println!("{}", path.display());
            }
        }
        ConfigCommand::Schema => print_schema(json),
        ConfigCommand::Validate => return Ok(validate(&files, json)),
        ConfigCommand::Reload => {
            Client::connect()?.call::<_, Json>(method::CONFIG_RELOAD, &Json::Null)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn spec(key: &str) -> anyhow::Result<&'static KeySpec> {
    schema::lookup(key)
        .ok_or_else(|| anyhow!("unknown key {key:?} (`motty config schema` lists them)"))
}

pub fn print_json(value: Json) {
    println!(
        "{}",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    );
}

/// A TOML value as JSON.
pub fn to_json(value: &Value) -> Json {
    serde_json::to_value(value).unwrap_or(Json::Null)
}

/// Integers where a float goes read as floats, so `show` prints one type.
fn normalize(spec: &KeySpec, value: Value) -> Value {
    match (spec.kind, value) {
        (Kind::Float { .. }, Value::Integer(i)) => Value::Float(i as f64),
        (_, v) => v,
    }
}

/// The value in effect: what the files say, else the default.
fn effective(files: &Files, spec: &KeySpec) -> Vec<Value> {
    let values = files.values(spec.key);
    let values = if values.is_empty() {
        defaults(spec)
    } else {
        values
    };
    values.into_iter().map(|v| normalize(spec, v)).collect()
}

fn defaults(spec: &KeySpec) -> Vec<Value> {
    match spec.default_value() {
        Some(Value::Array(items)) => items,
        Some(v) => vec![v],
        None => Vec::new(),
    }
}

fn get(
    files: &Files,
    spec: &KeySpec,
    show_origin: bool,
    local: bool,
    json: bool,
) -> anyhow::Result<ExitCode> {
    let origin = files.origin(spec.key);
    let values = if local {
        let src = std::fs::read_to_string(&files.local).unwrap_or_default();
        let (entries, _) = motty_theme::config::parse_lines(&src);
        let values = entries
            .into_iter()
            .filter(|(k, _)| k == spec.key)
            .map(|(_, v)| v)
            .collect();
        let values = store::list_entries(spec.key, values);
        if values.is_empty() {
            if !matches!(origin, Origin::Local(..)) {
                eprintln!("motty: config.local.toml doesn't set {}", spec.key);
                return Ok(ExitCode::from(1));
            }
            // Reset to default there.
            defaults(spec)
                .into_iter()
                .map(|v| normalize(spec, v))
                .collect()
        } else {
            values
        }
    } else {
        effective(files, spec)
    };
    if json {
        let value = match spec.list {
            Some(_) => Json::Array(values.iter().map(to_json).collect()),
            None => values.last().map(to_json).unwrap_or(Json::Null),
        };
        let mut data = json!({ "key": spec.key, "value": value });
        if show_origin {
            data["origin"] = json!(origin.describe());
        }
        print_json(json!({ "data": data }));
        return Ok(ExitCode::SUCCESS);
    }
    let prefix = if show_origin {
        format!("{}\t", origin.describe())
    } else {
        String::new()
    };
    for value in &values {
        println!("{prefix}{}", schema::display(value));
    }
    if values.is_empty() && show_origin {
        println!("{prefix}");
    }
    Ok(ExitCode::SUCCESS)
}

fn set(
    files: &Files,
    spec: &KeySpec,
    values: &[String],
    local: bool,
    reload: bool,
    transient: bool,
    json: bool,
) -> anyhow::Result<()> {
    let new: Vec<Value> = match spec.list {
        // `[]` empties a list.
        Some(_) if values == ["[]"] => Vec::new(),
        Some(_) => store::edit_list(spec, &files.values(spec.key), values)
            .map_err(|e| anyhow!("{}: {e}", spec.key))?,
        None => vec![
            schema::parse_text(spec, &values.join(" "))
                .map_err(|e| anyhow!("{}: {e}", spec.key))?,
        ],
    };
    if transient {
        let lines: String = match spec.list {
            Some(List::Inline) | None => {
                let value = match spec.list {
                    Some(_) => Value::Array(new.clone()),
                    None => new[0].clone(),
                };
                format!("{}\n", schema::line(spec.key, &value))
            }
            Some(List::Repeated) => new
                .iter()
                .map(|v| format!("{}\n", schema::line(spec.key, v)))
                .collect(),
        };
        Client::connect()?
            .call::<_, Json>(method::CONFIG_SET, &ConfigSetParams { lines })
            .context("the running app didn't take it")?;
    } else {
        files.set(spec.key, &new, local)?;
        if reload {
            match Client::connect() {
                Ok(mut client) => {
                    client.call::<_, Json>(method::CONFIG_RELOAD, &Json::Null)?;
                }
                Err(e) => eprintln!("motty: saved, but couldn't reload: {e:#}"),
            }
        }
    }
    // A list prints what the edit produced.
    if spec.list.is_some() {
        if json {
            print_json(
                json!({ "data": { "key": spec.key, "value": new.iter().map(to_json).collect::<Vec<_>>() } }),
            );
        } else {
            for v in &new {
                println!("{}", schema::display(v));
            }
        }
    }
    Ok(())
}

fn edit(files: &Files, local: bool) -> anyhow::Result<()> {
    let path = if local {
        if !files.local.exists() {
            files.edit_local(|_| {
                "# Settings for this machine only: never synced, and its keys win over config.toml.\n"
                    .into()
            })?;
        }
        files.local.clone()
    } else {
        if !files.main.exists() {
            files.edit_main(str::to_owned)?;
        }
        files.main.clone()
    };
    let editor = std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok())
        .filter(|e| !e.trim().is_empty());
    let status = match editor {
        // `code -w` and the like: the program, then its own arguments.
        Some(editor) => {
            let mut words = editor.split_whitespace();
            let program = words.next().unwrap_or("vi");
            std::process::Command::new(program)
                .args(words)
                .arg(&path)
                .status()
                .with_context(|| format!("couldn't run {editor}"))?
        }
        None => std::process::Command::new("/usr/bin/open")
            .arg("-t")
            .arg(&path)
            .status()
            .context("couldn't run open")?,
    };
    if !status.success() {
        bail!("the editor exited with {status}");
    }
    Ok(())
}

fn show(files: &Files, json: bool) {
    if json {
        let mut data = serde_json::Map::new();
        for spec in schema::KEYS {
            let values = effective(files, spec);
            let value = match spec.list {
                Some(_) => Json::Array(values.iter().map(to_json).collect()),
                None => values.last().map(to_json).unwrap_or(Json::Null),
            };
            data.insert(spec.field(), value);
        }
        print_json(json!({ "data": data }));
        return;
    }
    for spec in schema::KEYS {
        let values = effective(files, spec);
        match spec.list {
            Some(List::Inline) => println!("{}", schema::line(spec.key, &Value::Array(values))),
            _ => {
                for value in values {
                    println!("{}", schema::line(spec.key, &value));
                }
            }
        }
    }
}

fn schema_json(spec: &KeySpec) -> Json {
    let (min, max) = spec.range().unzip();
    json!({
        "key": spec.key,
        "field": spec.field(),
        "type": spec.type_name(),
        "list": spec.list.map(|l| match l {
            List::Repeated => "repeated",
            List::Inline => "inline",
        }),
        "default": spec.default_value().as_ref().map(to_json),
        "default_text": (!spec.default.is_empty()).then_some(spec.default),
        "values": spec.allowed(),
        "min": min,
        "max": max,
        "aliases": spec.aliases,
        "settings": (!spec.settings.is_empty()).then_some(spec.settings),
        "description": spec.description,
    })
}

fn print_schema(json: bool) {
    if json {
        let keys: Vec<Json> = schema::KEYS.iter().map(schema_json).collect();
        print_json(json!({ "data": { "keys": keys } }));
        return;
    }
    for spec in schema::KEYS {
        let mut ty = spec.type_name();
        if let Some((min, max)) = spec.range() {
            ty = format!("{ty} {min}–{max}");
        }
        let default = match spec.default {
            "" => "(none)",
            d => d,
        };
        println!("{}  {ty}  default: {default}", spec.key);
        let allowed = spec.allowed();
        if matches!(spec.kind, Kind::Enum { .. }) {
            println!("    values: {}", allowed.join(", "));
        }
        if !spec.aliases.is_empty() {
            println!("    aliases: {}", spec.aliases.join(", "));
        }
        if !spec.settings.is_empty() {
            println!("    settings: {}", spec.settings);
        }
        println!("    {}", spec.description);
    }
}

fn validate(files: &Files, json: bool) -> ExitCode {
    let problems = files.validate();
    if json {
        let list: Vec<Json> = problems
            .iter()
            .map(|p| json!({ "path": p.path, "line": p.line, "message": p.message }))
            .collect();
        print_json(json!({ "data": { "valid": problems.is_empty(), "problems": list } }));
    } else if problems.is_empty() {
        println!("No problems found.");
    } else {
        for p in &problems {
            println!("{}:{}: {}", p.path.display(), p.line, p.message);
        }
    }
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
