//! `motty keybind list`, `motty recipe list|apply` and `motty version`:
//! what the running app has.

use std::process::ExitCode;

use clap::Subcommand;
use motty_ipc::{AppInfo, KeybindInfo, RecipeApplyParams, RecipeInfo, method};
use serde_json::{Value, json};

use crate::output::{Ctx, utc_time};

#[derive(Subcommand)]
pub enum KeybindCmd {
    /// Every chord and what it runs: the config's keybind lines over the
    /// defaults.
    List {
        /// Only actions whose name holds this.
        #[arg(long)]
        action: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum RecipeCmd {
    /// Saved recipes, most recently updated first.
    List,
    /// Open a saved recipe, by id or name (case ignored).
    Apply {
        recipe: String,
        /// A layout recipe in a new window.
        #[arg(long, conflicts_with = "new_tab")]
        new_window: bool,
        /// A layout recipe as new tabs in the frontmost window.
        #[arg(long)]
        new_tab: bool,
        /// Where its Current Folder paths start (default: the front tab's
        /// folder).
        #[arg(long, value_name = "DIR")]
        cwd: Option<String>,
    },
}

pub fn keybind(ctx: &Ctx, cmd: KeybindCmd) -> anyhow::Result<ExitCode> {
    let KeybindCmd::List { action } = cmd;
    let mut binds: Vec<KeybindInfo> = ctx.connect()?.call(method::KEYBIND_LIST, &Value::Null)?;
    if let Some(filter) = action {
        let filter = filter.to_lowercase().replace('-', "_");
        binds.retain(|b| b.action.to_lowercase().contains(&filter));
    }
    ctx.print(&binds, || {
        let rows: Vec<Vec<String>> = binds
            .iter()
            .map(|b| vec![b.keys.clone(), b.action.clone(), b.source.clone()])
            .collect();
        ctx.table(&["KEYS", "ACTION", "SOURCE"], &rows);
    });
    Ok(ExitCode::SUCCESS)
}

pub fn recipe(ctx: &Ctx, cmd: RecipeCmd) -> anyhow::Result<ExitCode> {
    let mut client = ctx.connect()?;
    match cmd {
        RecipeCmd::List => {
            let recipes: Vec<RecipeInfo> = client.call(method::RECIPE_LIST, &Value::Null)?;
            // Otty puts the list under `data`.
            ctx.print(&json!({ "data": recipes }), || {
                let rows: Vec<Vec<String>> = recipes
                    .iter()
                    .map(|r| {
                        vec![
                            r.id.clone(),
                            r.name.clone(),
                            r.scope.clone(),
                            r.commands.to_string(),
                            utc_time(r.updated),
                        ]
                    })
                    .collect();
                ctx.table(&["ID", "NAME", "SCOPE", "COMMANDS", "UPDATED (UTC)"], &rows);
            });
        }
        RecipeCmd::Apply {
            recipe,
            new_window,
            new_tab,
            cwd,
        } => {
            let cwd = cwd
                .map(std::path::absolute)
                .transpose()?
                .map(|p| p.display().to_string());
            let params = RecipeApplyParams {
                recipe,
                new_window,
                new_tab,
                cwd,
            };
            let out: Value = client.call(method::RECIPE_APPLY, &params)?;
            ctx.print(&out, || {});
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The version and build, the protocol, and the running app's, if any.
pub fn version(ctx: &Ctx) -> anyhow::Result<ExitCode> {
    let app: Option<AppInfo> = ctx
        .connect()
        .ok()
        .and_then(|mut c| c.call(method::APP_INFO, &Value::Null).ok());
    let socket = ctx.socket.clone().unwrap_or_else(motty_ipc::socket_path);
    let info = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "build": option_env!("MOTTY_GIT_HASH").filter(|h| !h.is_empty()),
        "protocol": motty_ipc::PROTOCOL_VERSION,
        "socket": socket.display().to_string(),
        "app": app,
    });
    ctx.print(&info, || {
        println!("motty {}", crate::VERSION);
        println!(
            "protocol {} (JSON lines over {})",
            motty_ipc::PROTOCOL_VERSION,
            socket.display()
        );
        match &app {
            Some(app) => println!(
                "app {} (pid {}, protocol {})",
                app.version, app.pid, app.protocol
            ),
            None => println!("app not running"),
        }
    });
    Ok(ExitCode::SUCCESS)
}
