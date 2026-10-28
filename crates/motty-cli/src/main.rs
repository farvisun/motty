//! `motty`: control a running Motty over its socket.
//!
//! Inside a Motty pane, `$MOTTY_SOCKET` and `$MOTTY_PANE_ID` are set, so
//! commands act on the pane they run in unless `--pane` says otherwise.

mod mcp;

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use motty_ipc::{
    Client, Created, Direction, FindParams, FindResult, NewTabParams, PaneInfo, PaneRef,
    ReadParams, RunParams, RunResult, SendTextParams, SplitParams, TextRange, method,
};
use serde_json::Value;

#[derive(Parser)]
#[command(name = "motty", version, about = "Control a running Motty terminal")]
struct Cli {
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List panes in every window.
    List,
    /// Print a pane's text.
    Read {
        #[arg(long)]
        pane: Option<u64>,
        /// The last N lines, scrollback included.
        #[arg(long, conflicts_with = "last_command")]
        lines: Option<usize>,
        /// The output of the last finished command (needs shell integration).
        #[arg(long)]
        last_command: bool,
    },
    /// Type text into a pane (add \n yourself to press Return).
    Send {
        #[arg(long)]
        pane: Option<u64>,
        text: String,
    },
    /// Run a command line in a pane's shell, wait for it, print its output
    /// and exit with its status (needs shell integration).
    Run {
        #[arg(long)]
        pane: Option<u64>,
        /// Give up after this many seconds.
        #[arg(long)]
        timeout: Option<f64>,
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// Split a pane; prints the new pane's id.
    Split {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(long, short, value_enum, default_value = "right")]
        direction: Dir,
        #[arg(long)]
        cwd: Option<String>,
        /// Keep focus on the current pane.
        #[arg(long)]
        no_focus: bool,
        /// Program and arguments to run instead of a shell.
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Open a tab; prints the new pane's id.
    NewTab {
        #[arg(long)]
        window: Option<usize>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Search a pane's screen and scrollback; prints `line:column: text`
    /// (negative lines are scrollback). Exits 1 when nothing matches.
    Find {
        #[arg(long)]
        pane: Option<u64>,
        /// Match case exactly.
        #[arg(long, short = 'c')]
        case_sensitive: bool,
        /// Match whole words only.
        #[arg(long, short = 'w')]
        word: bool,
        /// The query is a regular expression (Rust syntax).
        #[arg(long, short = 'e')]
        regex: bool,
        /// Matches to print.
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Also open the pane's find bar with the query.
        #[arg(long)]
        show: bool,
        query: String,
    },
    /// Focus a pane (and its tab and window).
    Focus {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// Close a pane.
    Close {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// Serve Motty's panes to a coding agent over MCP (stdio), e.g.
    /// `claude mcp add motty -- motty mcp`.
    Mcp,
}

#[derive(Clone, Copy, ValueEnum)]
enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// `--pane`, else the pane this command runs in.
fn pane(pane: Option<u64>) -> PaneRef {
    PaneRef {
        pane: pane.or_else(|| std::env::var(motty_ipc::PANE_ENV).ok()?.parse().ok()),
    }
}

fn argv(command: Vec<String>) -> Option<Vec<String>> {
    (!command.is_empty()).then_some(command)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("motty: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    if let Command::Mcp = cli.command {
        mcp::serve()?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut client = Client::connect()?;
    let json = cli.json;
    let print = |value: &Value| {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        )
    };
    match cli.command {
        Command::List => {
            let panes: Vec<PaneInfo> = client.call(method::LIST_PANES, &Value::Null)?;
            if json {
                print(&serde_json::to_value(&panes)?);
            } else {
                for p in panes {
                    println!(
                        "{:>4}  w{} t{}  {}{:<10}  {}",
                        p.id,
                        p.window,
                        p.tab,
                        if p.focused { "*" } else { " " },
                        p.process.as_deref().unwrap_or("-"),
                        p.cwd.as_deref().unwrap_or(&p.title),
                    );
                }
            }
        }
        Command::Read {
            pane: id,
            lines,
            last_command,
        } => {
            let range = match (lines, last_command) {
                (_, true) => TextRange::LastCommand,
                (Some(n), _) => TextRange::Lines(n),
                _ => TextRange::Screen,
            };
            let out: Value = client.call(
                method::READ,
                &ReadParams {
                    pane: pane(id),
                    range,
                },
            )?;
            if json {
                print(&out);
            } else {
                println!("{}", out["text"].as_str().unwrap_or_default());
            }
        }
        Command::Send { pane: id, text } => {
            let _: Value = client.call(
                method::SEND_TEXT,
                &SendTextParams {
                    pane: pane(id),
                    text,
                },
            )?;
        }
        Command::Run {
            pane: id,
            timeout,
            command,
        } => {
            let timeout_ms = timeout.map(|s| (s * 1000.) as u64);
            if let Some(ms) = timeout_ms {
                // The app answers with a timeout error; this is a backstop.
                client.set_timeout(Some(Duration::from_millis(ms + 2000)))?;
            }
            let params = RunParams {
                pane: pane(id),
                command: command.join(" "),
                timeout_ms,
            };
            let result: RunResult = client.call(method::RUN, &params).context("run")?;
            if json {
                print(&serde_json::to_value(&result)?);
            } else if !result.output.is_empty() {
                println!("{}", result.output);
            }
            return Ok(ExitCode::from(result.exit.unwrap_or(0).clamp(0, 255) as u8));
        }
        Command::Split {
            pane: id,
            direction,
            cwd,
            no_focus,
            command,
        } => {
            let direction = match direction {
                Dir::Left => Direction::Left,
                Dir::Right => Direction::Right,
                Dir::Up => Direction::Up,
                Dir::Down => Direction::Down,
            };
            let params = SplitParams {
                pane: pane(id),
                direction,
                command: argv(command),
                cwd,
                no_focus,
            };
            let created: Created = client.call(method::SPLIT, &params)?;
            println!("{}", created.pane);
        }
        Command::NewTab {
            window,
            cwd,
            command,
        } => {
            let created: Created = client.call(
                method::NEW_TAB,
                &NewTabParams {
                    window,
                    command: argv(command),
                    cwd,
                },
            )?;
            println!("{}", created.pane);
        }
        Command::Find {
            pane: id,
            case_sensitive,
            word,
            regex,
            limit,
            show,
            query,
        } => {
            let params = FindParams {
                pane: pane(id),
                query,
                case_sensitive,
                whole_word: word,
                regex,
                limit: Some(limit),
                show,
            };
            let result: FindResult = client.call(method::FIND, &params)?;
            if json {
                print(&serde_json::to_value(&result)?);
            } else {
                for m in &result.matches {
                    println!("{}:{}: {}", m.line, m.column + 1, m.line_text);
                }
                if result.total > result.matches.len() {
                    eprintln!("({} more)", result.total - result.matches.len());
                }
            }
            if result.total == 0 {
                return Ok(ExitCode::from(1));
            }
        }
        Command::Focus { pane: id } => {
            let _: Value = client.call(method::FOCUS, &pane(id))?;
        }
        Command::Close { pane: id } => {
            let _: Value = client.call(method::CLOSE, &pane(id))?;
        }
        Command::Mcp => unreachable!("handled above"),
    }
    Ok(ExitCode::SUCCESS)
}
