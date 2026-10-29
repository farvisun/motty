//! The flat verbs `motty` had before Otty's noun-verb tree (`list`, `read`,
//! `send`, ...): hidden from `--help`, and doing what they always did so
//! scripts and installed skills keep working.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use clap::Subcommand;
use motty_ipc::{
    ActionParams, Created, FindParams, NewTabParams, PaneInfo, PaneRef, ReadParams, RunParams,
    RunResult, SendTextParams, SetModeParams, SplitParams, TextRange, method,
};
use serde_json::Value;

use crate::output::{Ctx, own_pane};
use crate::tree::{Dir, Mode, SubmitWhen, exit_code};

#[derive(Subcommand)]
pub enum Legacy {
    #[command(hide = true)]
    List,
    #[command(hide = true)]
    Read {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(long, conflicts_with = "last_command")]
        lines: Option<usize>,
        #[arg(long)]
        last_command: bool,
    },
    #[command(hide = true)]
    Send {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(long, value_enum, num_args = 0..=1, require_equals = true, default_missing_value = "queue")]
        submit: Option<SubmitWhen>,
        text: String,
    },
    /// Exits 124 on timeout, 3 when the pane can't tell.
    #[command(hide = true)]
    Wait {
        #[arg(long, required_unless_present = "session")]
        pane: Option<u64>,
        #[arg(long, conflicts_with = "pane")]
        session: Option<String>,
        /// Every other pane of that pane's tab.
        #[arg(long)]
        tab: bool,
        #[arg(long)]
        timeout_secs: Option<f64>,
    },
    #[command(hide = true)]
    Run {
        #[arg(long)]
        pane: Option<u64>,
        /// Seconds (`--timeout` before `motty`'s own became milliseconds;
        /// see [`rewrite`]).
        #[arg(long)]
        run_timeout: Option<f64>,
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    #[command(hide = true)]
    Split {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(long, short, value_enum, default_value = "right")]
        direction: Dir,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        no_focus: bool,
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
    #[command(hide = true)]
    NewTab {
        #[arg(long)]
        window: Option<usize>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long, conflicts_with_all = ["cwd", "command"])]
        reopen: bool,
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
    #[command(hide = true)]
    Find {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(long, short = 'c')]
        case_sensitive: bool,
        #[arg(long, short = 'w')]
        word: bool,
        #[arg(long, short = 'e')]
        regex: bool,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        show: bool,
        query: String,
    },
    #[command(hide = true)]
    Mode {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(value_enum)]
        mode: Mode,
    },
    /// Quit Motty (saving the session, as ⌘Q does).
    #[command(hide = true)]
    Quit,
    #[command(hide = true)]
    Focus {
        #[arg(long)]
        pane: Option<u64>,
    },
    #[command(hide = true)]
    Action {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(long, conflicts_with = "name")]
        list: bool,
        #[arg(required_unless_present = "list")]
        name: Option<String>,
    },
    #[command(hide = true)]
    Close {
        #[arg(long)]
        pane: Option<u64>,
    },
}

/// `run --timeout <secs>` from before the global `--timeout <ms>`: renamed
/// to the old command's own flag, so old scripts keep their seconds.
pub fn rewrite(mut args: Vec<String>) -> Vec<String> {
    if args.get(1).map(String::as_str) != Some("run") {
        return args;
    }
    let mut takes_value = false;
    for arg in args.iter_mut().skip(2) {
        if arg == "--" {
            break;
        }
        if takes_value {
            takes_value = false;
            continue;
        }
        if !arg.starts_with('-') {
            break;
        }
        if arg == "--timeout" {
            *arg = "--run-timeout".into();
        } else if let Some(v) = arg.strip_prefix("--timeout=") {
            *arg = format!("--run-timeout={v}");
            continue;
        }
        takes_value = matches!(
            arg.as_str(),
            "--pane" | "--run-timeout" | "--socket" | "--config-file" | "--format"
        );
    }
    args
}

/// `--pane`, else the pane this command runs in.
fn pane(pane: Option<u64>) -> PaneRef {
    PaneRef::id(pane.or_else(own_pane))
}

fn argv(command: Vec<String>) -> Option<Vec<String>> {
    (!command.is_empty()).then_some(command)
}

pub fn run(ctx: &Ctx, cmd: Legacy) -> anyhow::Result<ExitCode> {
    let mut client = ctx.connect()?;
    match cmd {
        Legacy::List => {
            let panes: Vec<PaneInfo> = client.call(method::LIST_PANES, &Value::Null)?;
            ctx.print(&panes, || {
                for p in &panes {
                    // `[claude: awaiting] Approve Bash: rm -rf target/`
                    let state = match (&p.agent, &p.badge) {
                        (Some(a), Some(b)) => format!("  [{a}: {b}]"),
                        (Some(a), None) => format!("  [{a}]"),
                        (None, Some(b)) => format!("  [{b}]"),
                        (None, None) => String::new(),
                    };
                    let status = p
                        .status
                        .as_deref()
                        .map(|s| format!(" {s}"))
                        .unwrap_or_default();
                    println!(
                        "{:>4}  w{} t{}  {}{:<10}  {}{state}{status}",
                        p.id,
                        p.window,
                        p.tab,
                        if p.focused { "*" } else { " " },
                        p.process.as_deref().unwrap_or("-"),
                        p.cwd.as_deref().unwrap_or(&p.title),
                    );
                }
            });
        }
        Legacy::Read {
            pane: id,
            lines,
            last_command,
        } => {
            let range = match (lines, last_command) {
                (_, true) => TextRange::LastCommand,
                (Some(n), _) => TextRange::Lines(n),
                _ => TextRange::Screen,
            };
            let params = ReadParams {
                pane: pane(id),
                range,
            };
            let out: Value = client.call(method::READ, &params)?;
            ctx.print(&out, || {
                println!("{}", out["text"].as_str().unwrap_or_default())
            });
        }
        Legacy::Send {
            pane: id,
            submit,
            text,
        } => {
            let params = SendTextParams {
                pane: pane(id),
                text,
                submit: submit.map(SubmitWhen::ipc),
            };
            let _: Value = client.call(method::SEND_TEXT, &params)?;
        }
        Legacy::Wait {
            pane: id,
            session,
            tab,
            timeout_secs,
        } => {
            let deadline = timeout_secs.map(|s| Instant::now() + Duration::from_secs_f64(s));
            client.set_timeout(None)?;
            return wait(&mut client, id, session.as_deref(), tab, deadline);
        }
        Legacy::Run {
            pane: id,
            run_timeout,
            command,
        } => {
            let timeout_ms = run_timeout.map(|s| (s * 1000.) as u64);
            // The app answers with a timeout error; this is a backstop.
            client.set_timeout(timeout_ms.map(|ms| Duration::from_millis(ms + 2000)))?;
            let params = RunParams {
                pane: pane(id),
                command: command.join(" "),
                timeout_ms,
            };
            let result: RunResult = client.call(method::RUN, &params).context("run")?;
            ctx.print(&result, || {
                if !result.output.is_empty() {
                    println!("{}", result.output);
                }
            });
            return Ok(exit_code(result.exit));
        }
        Legacy::Split {
            pane: id,
            direction,
            cwd,
            no_focus,
            command,
        } => {
            let params = SplitParams {
                pane: pane(id),
                direction: direction.ipc(),
                command: argv(command),
                cwd,
                no_focus,
                size: None,
                line: None,
            };
            let created: Created = client.call(method::SPLIT, &params)?;
            println!("{}", created.pane);
        }
        Legacy::NewTab {
            window,
            cwd,
            reopen,
            command,
        } => {
            let params = NewTabParams {
                window,
                command: argv(command),
                cwd,
                reopen,
                ..Default::default()
            };
            let created: Created = client.call(method::NEW_TAB, &params)?;
            println!("{}", created.pane);
        }
        Legacy::Find {
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
            return crate::tree::find(ctx, &mut client, &params);
        }
        Legacy::Mode { pane: id, mode } => {
            let params = SetModeParams {
                pane: pane(id),
                mode: mode.ipc(),
            };
            let _: Value = client.call(method::SET_MODE, &params)?;
        }
        Legacy::Quit => {
            let _: Value = client.call(method::QUIT, &Value::Null)?;
        }
        Legacy::Focus { pane: id } => {
            let _: Value = client.call(method::FOCUS, &pane(id))?;
        }
        Legacy::Action { list: true, .. } => {
            let names: Vec<String> = client.call(method::LIST_ACTIONS, &Value::Null)?;
            ctx.print(&names, || names.iter().for_each(|n| println!("{n}")));
        }
        Legacy::Action { pane: id, name, .. } => {
            let params = ActionParams {
                pane: pane(id),
                name: name.unwrap_or_default(),
            };
            let _: Value = client.call(method::ACTION, &params)?;
        }
        Legacy::Close { pane: id } => {
            let _: Value = client.call(method::CLOSE, &pane(id))?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Polls the panes until every one waited on is done (or gone).
fn wait(
    client: &mut motty_ipc::Client,
    pane_id: Option<u64>,
    session: Option<&str>,
    tab: bool,
    deadline: Option<Instant>,
) -> anyhow::Result<ExitCode> {
    let me = own_pane();
    let mut first = true;
    loop {
        let panes: Vec<PaneInfo> = client.call(method::LIST_PANES, &Value::Null)?;
        let target = panes.iter().find(|p| match session {
            Some(s) => p.session.as_deref() == Some(s),
            None => Some(p.id) == pane_id,
        });
        let Some(target) = target else {
            anyhow::ensure!(
                !first,
                "no such {}",
                if session.is_some() { "session" } else { "pane" }
            );
            // It closed: nothing left to wait for.
            return Ok(ExitCode::SUCCESS);
        };
        let waited: Vec<&PaneInfo> = if tab {
            panes
                .iter()
                .filter(|p| p.window == target.window && p.tab == target.tab)
                .filter(|p| Some(p.id) != me)
                .collect()
        } else {
            vec![target]
        };
        let mut done = true;
        for p in waited {
            match p.idle() {
                Some(idle) => done &= idle,
                None => {
                    eprintln!(
                        "motty: pane {} can't report when it's done: it runs no coding agent \
                         Motty knows of and its shell has no shell integration",
                        p.id
                    );
                    return Ok(ExitCode::from(3));
                }
            }
        }
        if done {
            return Ok(ExitCode::SUCCESS);
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            eprintln!("motty: timed out");
            return Ok(ExitCode::from(124));
        }
        first = false;
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn old_run_timeouts_stay_seconds() {
        assert_eq!(
            rewrite(args("motty run --pane 3 --timeout 5 make --timeout 9")),
            args("motty run --pane 3 --run-timeout 5 make --timeout 9")
        );
        assert_eq!(
            rewrite(args("motty run --timeout=2 -- ls")),
            args("motty run --run-timeout=2 -- ls")
        );
        assert_eq!(
            rewrite(args("motty pane run --timeout 5 ls")),
            args("motty pane run --timeout 5 ls")
        );
    }
}
