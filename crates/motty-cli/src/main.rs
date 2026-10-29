//! `motty`: control a running Motty over its socket.
//!
//! Inside a Motty pane, `$MOTTY_SOCKET` and `$MOTTY_PANE_ID` are set, so
//! commands act on the pane they run in unless `--pane` says otherwise.
//! The commands follow Otty's (`motty window|tab|pane <verb>`, `view`,
//! `watch`, `jump`, ...); the flat verbs from before stay as hidden aliases
//! (see `legacy`).

mod agent;
mod catalog;
mod config_cmd;
mod export;
mod features;
mod folders;
mod fonts;
mod import;
mod keys;
mod legacy;
mod mcp;
mod open;
mod output;
mod themes;
mod tree;
mod watch;

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use motty_ipc::{IpcError, skills};
use serde_json::{Value, json};

use output::Ctx;

/// `0.1.0 (1a2b3c4)`: the version and the commit it was built from.
pub const VERSION: &str = env!("MOTTY_VERSION");

/// The config file `--config-file` points the config commands at.
pub const CONFIG_FILE_ENV: &str = "MOTTY_CONFIG_FILE";

#[derive(Parser)]
#[command(name = "motty", version = VERSION, about = "Control a running Motty terminal")]
struct Cli {
    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Same as --format json.
    #[arg(long, global = true)]
    json: bool,
    /// Leave the header row out of tables.
    #[arg(long, global = true)]
    no_headers: bool,
    /// Print nothing on success but what was asked for.
    #[arg(short, long, global = true)]
    quiet: bool,
    /// Talk to the Motty at this socket instead of $MOTTY_SOCKET's.
    #[arg(long, global = true, value_name = "PATH")]
    socket: Option<PathBuf>,
    /// Use this config file instead of Motty's.
    #[arg(long, global = true, value_name = "PATH")]
    config_file: Option<PathBuf>,
    /// How long to wait for the app to answer, in milliseconds (0: forever).
    #[arg(long, global = true, value_name = "MS", default_value_t = 3000)]
    timeout: u64,
    /// Don't ask before destructive actions (and close what Motty would ask
    /// about).
    #[arg(short = 'y', long, global = true)]
    yes: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Motty's windows: list, show, new, close, focus, rename.
    Window {
        #[command(subcommand)]
        cmd: tree::WindowCmd,
    },
    /// Same as `window list`.
    Windows,
    /// Tabs: list, show, new, close, focus, rename, move, badge.
    Tab {
        #[command(subcommand)]
        cmd: tree::TabCmd,
    },
    /// Same as `tab list`.
    Tabs(tree::WindowSel),
    /// Panes: list, show, split, close, focus, zoom, resize, send-keys,
    /// send-text, run, exec, wait, capture, link, ...
    Pane {
        #[command(subcommand)]
        cmd: tree::PaneCmd,
    },
    /// Same as `pane list`.
    Panes(tree::TabSel),
    /// Open a new window, in a folder (default: here); a file or URL opens
    /// in an editor tab or the browser.
    Open(open::Open),
    /// Open a file or an http(s) URL as a pane, read-only.
    View(open::View),
    /// Open a file (or URL) as a pane, editable.
    Edit(open::View),
    /// Run a command with a spinner on its tab, then a badge and a
    /// notification (unless -q); exits with its status. `motty
    /// watch:<agent> <session-id>` instead waits for an agent session.
    Watch {
        #[arg(trailing_var_arg = true, required = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Go to the best-ranked folder matching the query (no query: home and
    /// back).
    Jump {
        query: Option<String>,
        /// Only print the folder.
        #[arg(long)]
        no_cd: bool,
        /// Print `cd:<path>` for the shell integration's `motty` function.
        #[arg(long, hide = true)]
        shell_cd: bool,
    },
    /// Record a visit to a folder (default: here) for `jump`.
    Learn { path: Option<String> },
    /// Forget a folder for `jump`, and stop recording it.
    Ignore { path: String },
    /// Key bindings.
    Keybind {
        #[command(subcommand)]
        cmd: catalog::KeybindCmd,
    },
    /// Saved recipes: list, apply.
    Recipe {
        #[command(subcommand)]
        cmd: catalog::RecipeCmd,
    },
    /// Terminal feature demos: `features list`, `features try <name>` or
    /// `features <name>`.
    Features { args: Vec<String> },
    /// Print a shell completion script.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Print the version, build and protocol.
    Version,
    /// The skills Settings › Agents › Skills installs into coding agents.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Serve Motty's panes to a coding agent over MCP (stdio), e.g.
    /// `claude mcp add motty -- motty mcp`.
    Mcp,
    /// Report a coding agent's hook event to Motty (what Motty's installed
    /// hooks run). Custom agents use `motty state:<name> state=…`.
    #[command(hide = true)]
    Hook {
        agent: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Read and change settings: config.toml and config.local.toml.
    Config {
        #[command(subcommand)]
        command: config_cmd::ConfigCommand,
    },
    /// List and import color themes.
    Theme {
        #[command(subcommand)]
        command: themes::ThemeCommand,
    },
    /// List, apply and import fonts.
    Font {
        #[command(subcommand)]
        command: fonts::FontCommand,
    },
    /// Import another terminal's config (ghostty, kitty, alacritty),
    /// zoxide's folders, a theme or a font.
    Import(import::ImportArgs),
    /// Write your config for another terminal (ghostty, kitty, alacritty).
    Export(export::ExportArgs),
    #[command(flatten)]
    Legacy(legacy::Legacy),
}

#[derive(Subcommand)]
enum SkillCommand {
    /// Name and one line for each.
    List,
    /// A skill's instructions, as this Motty understands them.
    Show { name: String },
}

/// The global flags for commands parsed outside clap (`watch:<agent>`).
fn default_ctx() -> Ctx {
    Ctx {
        json: false,
        no_headers: false,
        quiet: false,
        yes: false,
        socket: None,
        timeout: Some(Duration::from_millis(3000)),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // `motty state:myagent state=processing …` and `motty watch:claude
    // <id>`, as Otty spells them.
    if let Some(name) = args.get(1).and_then(|a| a.strip_prefix("state:")) {
        return agent::state(name, &args[2..]);
    }
    if let Some(name) = args.get(1).and_then(|a| a.strip_prefix("watch:")) {
        return watch::watch_agent(&default_ctx(), name, &args[2..]);
    }
    let cli = Cli::parse_from(legacy::rewrite(args));
    if let Some(path) = &cli.config_file {
        // SAFETY: nothing else runs yet; the config commands read it.
        unsafe { std::env::set_var(CONFIG_FILE_ENV, path) };
    }
    if let Command::Hook { agent, args } = &cli.command {
        return agent::hook(agent, args);
    }
    let ctx = Ctx {
        json: cli.json || cli.format == Format::Json,
        no_headers: cli.no_headers,
        quiet: cli.quiet,
        yes: cli.yes,
        socket: cli.socket,
        timeout: (cli.timeout > 0).then(|| Duration::from_millis(cli.timeout)),
    };
    match run(&ctx, cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("motty: {e:#}");
            ExitCode::from(exit_status(&e))
        }
    }
}

/// The status the app asked for (4 when a selector named nothing, as in
/// Otty), else 2.
fn exit_status(e: &anyhow::Error) -> u8 {
    e.downcast_ref::<IpcError>()
        .and_then(|e| e.code)
        .map_or(2, |code| code.clamp(1, 255) as u8)
}

fn run(ctx: &Ctx, command: Command) -> anyhow::Result<ExitCode> {
    match command {
        Command::Window { cmd } => tree::window(ctx, cmd),
        Command::Windows => tree::window(ctx, tree::WindowCmd::List),
        Command::Tab { cmd } => tree::tab(ctx, cmd),
        Command::Tabs(sel) => tree::tab(ctx, tree::TabCmd::List(sel)),
        Command::Pane { cmd } => tree::pane(ctx, cmd),
        Command::Panes(sel) => tree::pane(ctx, tree::PaneCmd::List { sel }),
        Command::Open(o) => open::open(ctx, o),
        Command::View(v) => open::view(ctx, v, false),
        Command::Edit(v) => open::view(ctx, v, true),
        Command::Watch { command } => watch::watch(ctx, &command),
        Command::Jump {
            query,
            no_cd,
            shell_cd,
        } => folders::jump(ctx, query, no_cd, shell_cd),
        Command::Learn { path } => folders::learn(ctx, path, false),
        Command::Ignore { path } => folders::learn(ctx, Some(path), true),
        Command::Keybind { cmd } => catalog::keybind(ctx, cmd),
        Command::Recipe { cmd } => catalog::recipe(ctx, cmd),
        Command::Features { args } => features(ctx, &args),
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "motty", &mut std::io::stdout());
            Ok(ExitCode::SUCCESS)
        }
        Command::Version => catalog::version(ctx),
        Command::Skill { command } => skill(&command),
        Command::Mcp => {
            mcp::serve()?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Config { command } => config_cmd::run(&command, ctx.json),
        Command::Theme { command } => themes::run(&command, ctx.json),
        Command::Font { command } => fonts::run(&command, ctx.json),
        Command::Import(args) => import::run(&args, ctx.json),
        Command::Export(args) => export::run(&args, ctx.json),
        Command::Hook { .. } => unreachable!("handled in main"),
        Command::Legacy(cmd) => legacy::run(ctx, cmd),
    }
}

/// `features`, `features list`, `features try <name>`, `features <name>`.
fn features(ctx: &Ctx, args: &[String]) -> anyhow::Result<ExitCode> {
    let name = match args {
        [] => None,
        [list] if list == "list" => None,
        [try_, name] if try_ == "try" => Some(name),
        [name] => Some(name),
        _ => anyhow::bail!("usage: motty features [list | try <name> | <name>]"),
    };
    let Some(name) = name else {
        let list: Vec<Value> = features::FEATURES
            .iter()
            .map(|f| json!({ "name": f.name, "description": f.description }))
            .collect();
        ctx.print(&list, || {
            let rows: Vec<Vec<String>> = features::FEATURES
                .iter()
                .map(|f| vec![f.name.to_owned(), f.description.to_owned()])
                .collect();
            ctx.table(&["NAME", "DESCRIPTION"], &rows);
        });
        return Ok(ExitCode::SUCCESS);
    };
    let feature = features::find(name)
        .with_context(|| format!("no feature {name:?} (see `motty features list`)"))?;
    let mut out = std::io::stdout().lock();
    features::play(feature, &mut out, false)?;
    out.flush()?;
    Ok(ExitCode::SUCCESS)
}

fn skill(command: &SkillCommand) -> anyhow::Result<ExitCode> {
    match command {
        SkillCommand::List => {
            for s in &skills::SKILLS {
                println!("{:<6}  {}", s.name, s.description);
            }
        }
        SkillCommand::Show { name } => {
            let skill = skills::find(name.trim_start_matches('/')).with_context(|| {
                let names: Vec<_> = skills::SKILLS.iter().map(|s| s.name).collect();
                format!("no skill {name} (there's {})", names.join(", "))
            })?;
            println!("{}", skills::show(skill));
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn flat_verbs_are_hidden_and_still_parse() {
        let help = Cli::command().render_long_help().to_string();
        for verb in ["window", "pane", "tab", "watch", "jump", "features"] {
            assert!(help.contains(verb), "{verb} missing from help");
        }
        for hidden in ["new-tab", "  send ", "  quit "] {
            assert!(!help.contains(hidden), "{hidden} shows in help");
        }
        for line in [
            "motty list",
            "motty read --pane 3 --lines 20",
            "motty send --pane 3 --submit hi",
            "motty run --pane 3 --timeout 5 make test",
            "motty split --direction down htop",
            "motty new-tab --cwd /tmp",
            "motty wait --pane 3 --tab --timeout-secs 1",
            "motty find -e foo.*",
            "motty mode vi",
            "motty quit",
            "motty focus --pane 2",
            "motty action --list",
            "motty close --pane 2",
            "motty open README.md --split right",
        ] {
            let args = legacy::rewrite(line.split(' ').map(str::to_owned).collect());
            assert!(Cli::try_parse_from(args).is_ok(), "{line}");
        }
    }

    #[test]
    fn the_tree_parses() {
        for line in [
            "motty --json panes",
            "motty pane split --direction right --command htop --size 30",
            "motty pane resize --right 10 --down 5",
            "motty pane send-keys --pane 3 -- echo-hi key:Enter",
            "motty pane capture --pane 2 --lines 50",
            "motty tab badge --kind awaiting-input",
            "motty tab badge --clear",
            "motty pane wait --tab",
            "motty pane wait --tab t_4 --timeout-secs 300",
            "motty pane link 12 --line 1204",
            "motty pane focus motty://pane/12?line=4",
            "motty window rename --window title:api API",
            "motty tab move --tab 2 --to-window w_1 --index 1",
            "motty view --right tail.log",
            "motty edit src/lib.rs --mode view",
            "motty watch -q cargo test",
            "motty watch -- make -j4",
            "motty jump api --no-cd",
            "motty recipe apply Dev --new-window --cwd /tmp",
            "motty keybind list --action split",
            "motty features try true-color",
            "motty completions zsh",
            "motty --timeout 500 --socket /tmp/s.sock version",
            "motty open . --command vim --title Editor",
            // The README's examples.
            "motty pane exec -- cargo test",
            "motty pane capture --last-command",
            "motty pane split -d down --command htop",
            "motty pane send-keys --pane 3 -- q key:Enter",
            "motty pane wait --pane 3 --tab",
            "motty pane mode read-only --pane 3",
            "motty pane find -e error",
            "motty pane link --last-command",
            "motty tab new --cwd /tmp",
            "motty tab close --tab t_4",
            "motty window list",
            "motty view --right build.log",
            "motty edit src/main.rs",
            "motty watch -- cargo build",
            "motty pane action split_right",
            "motty pane action --list",
        ] {
            assert!(
                Cli::try_parse_from(line.split(' ')).is_ok(),
                "{line}: {}",
                Cli::try_parse_from(line.split(' '))
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default()
            );
        }
        assert!(Cli::try_parse_from("motty pane split --size 95".split(' ')).is_err());
        assert!(Cli::try_parse_from("motty tab badge --kind shiny".split(' ')).is_err());
    }

    #[test]
    fn no_match_exits_4() {
        let e: anyhow::Error = IpcError::no_match("pane", "p_9").into();
        assert_eq!(exit_status(&e), 4);
        assert_eq!(e.to_string(), "No pane matched selector p_9");
        assert_eq!(exit_status(&anyhow::anyhow!("other")), 2);
    }
}
