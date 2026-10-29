//! `motty watch <cmd>`: a command with a spinner on its tab and a badge and
//! a notification when it ends; and `motty watch:<agent> <session>`, which
//! blocks until a coding agent's session is idle.

use std::io::Write as _;
use std::os::unix::process::ExitStatusExt as _;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use clap::Parser;
use motty_ipc::{AgentIntegration, PaneInfo, PaneRef, TabBadgeParams, method};
use serde_json::{Value, json};

use crate::output::{Ctx, own_pane};

/// Spinner while it runs (OSC 9;4;3), cleared when it ends.
const SPINNER: &str = "\x1b]9;4;3\x07";
const CLEAR: &str = "\x1b]9;4;0\x07";

/// The terminal, even when stdout is piped; nothing outside one.
fn tty(text: &str) {
    if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        let _ = tty.write_all(text.as_bytes());
        let _ = tty.flush();
    }
}

/// Runs `command` (one argument with spaces goes to `$SHELL -c`) and exits
/// with its status. Ctrl-C reaches only the command and just clears the
/// spinner: stopping it on purpose isn't a failure.
pub fn watch(ctx: &Ctx, command: &[String]) -> anyhow::Result<ExitCode> {
    let mut child = match command {
        [line] if line.contains(char::is_whitespace) => {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
            let mut c = Command::new(shell);
            c.arg("-c").arg(line);
            c
        }
        [program, args @ ..] => {
            let mut c = Command::new(program);
            c.args(args);
            c
        }
        [] => anyhow::bail!("nothing to watch"),
    };
    let started = Instant::now();
    tty(SPINNER);
    let spawned = child.spawn();
    let status = match spawned {
        Ok(mut child) => {
            // SAFETY: plain signal dispositions, set and restored around
            // the wait; the child was started with the default one.
            let previous = unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN) };
            let status = child.wait();
            unsafe { libc::signal(libc::SIGINT, previous) };
            status
        }
        Err(e) => Err(e),
    };
    tty(CLEAR);
    let status = match status {
        Ok(status) => status,
        Err(e) => {
            badge(ctx, Some("error"));
            anyhow::bail!("can't run {}: {e}", command[0]);
        }
    };
    let took = format_duration(started.elapsed());
    let name = command.join(" ");
    if status.signal() == Some(libc::SIGINT) {
        badge(ctx, None);
        return Ok(ExitCode::from(130));
    }
    let code = status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
    let (kind, body) = if code == 0 {
        ("completed", format!("Finished in {took}"))
    } else {
        (
            "error",
            format!("Failed with exit status {code} after {took}"),
        )
    };
    badge(ctx, Some(kind));
    if !ctx.quiet {
        tty(&format!("\x1b]777;notify;{};{body}\x07", sanitize(&name)));
    }
    Ok(ExitCode::from(code.clamp(0, 255) as u8))
}

/// OSC text can't hold control characters or the `;` that ends its title.
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() || c == ';' { ' ' } else { c })
        .collect()
}

/// Sets this pane's tab badge, when it's in Motty; best effort.
fn badge(ctx: &Ctx, kind: Option<&str>) {
    let Some(pane) = own_pane() else { return };
    let Ok(mut client) = ctx.connect() else {
        return;
    };
    let params = TabBadgeParams {
        target: PaneRef::id(Some(pane)),
        kind: kind.map(str::to_owned),
    };
    let _ = client.call::<_, Value>(method::TAB_BADGE, &params);
}

pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60. {
        format!("{secs:.1}s")
    } else {
        let s = secs as u64;
        format!("{}m {:02}s", s / 60, s % 60)
    }
}

/// `motty watch:<agent> <session-id>`.
#[derive(Parser)]
#[command(name = "motty watch:<agent>")]
pub struct WatchAgent {
    /// The session id (as in Agent History, or `motty pane list --json`).
    session: String,
    /// How often to look, in milliseconds.
    #[arg(long, default_value_t = 5000)]
    interval_ms: u64,
    /// Give up after this long; 0 waits forever.
    #[arg(long, default_value_t = 0)]
    timeout_secs: u64,
    /// Give up after this long with no state reported at all; 0 waits
    /// forever.
    #[arg(long, default_value_t = 60)]
    unknown_timeout_secs: u64,
    /// Print state changes to stderr.
    #[arg(short, long)]
    verbose: bool,
}

/// Otty's statuses for `watch:<agent>`.
pub const IDLE: u8 = 0;
pub const NEVER_REPORTED: u8 = 4;
pub const NO_INTEGRATION: u8 = 6;
pub const TIMED_OUT: u8 = 9;

/// What a pane says of its agent session.
fn state(p: &PaneInfo) -> &'static str {
    if p.badge.as_deref() == Some("awaiting") {
        "awaiting-input"
    } else if p.in_turn {
        "processing"
    } else {
        "idle"
    }
}

/// Blocks until the session is idle (0) or gone (0); 4 when it never
/// reported, 6 when the agent's hooks aren't installed, 9 on timeout.
pub fn watch_agent(ctx: &Ctx, agent: &str, args: &[String]) -> ExitCode {
    let argv = std::iter::once(format!("motty watch:{agent}")).chain(args.iter().cloned());
    let w = match WatchAgent::try_parse_from(argv) {
        Ok(w) => w,
        Err(e) => e.exit(),
    };
    match watch_agent_inner(ctx, agent, &w) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("motty: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn watch_agent_inner(ctx: &Ctx, agent: &str, w: &WatchAgent) -> anyhow::Result<u8> {
    let mut client = ctx.connect()?;
    let hooks: AgentIntegration =
        client.call(method::AGENT_INTEGRATION, &json!({ "agent": agent }))?;
    // Custom agents report through `motty state:`; nothing to check.
    if hooks.known && !hooks.installed {
        eprintln!(
            "motty: {agent} has no Motty integration installed, so its session can never \
             report; install it in Settings › Agents (then restart the session)"
        );
        return Ok(NO_INTEGRATION);
    }
    let started = Instant::now();
    let interval = Duration::from_millis(w.interval_ms.max(50));
    let mut seen: Option<&'static str> = None;
    loop {
        let panes: Vec<PaneInfo> = client.call(method::LIST_PANES, &Value::Null)?;
        match panes
            .iter()
            .find(|p| p.session.as_deref() == Some(w.session.as_str()))
        {
            Some(p) => {
                let now = state(p);
                if w.verbose && seen != Some(now) {
                    eprintln!("{} {now}", w.session);
                }
                seen = Some(now);
                if now == "idle" {
                    return Ok(IDLE);
                }
            }
            None if seen.is_some() => {
                if w.verbose {
                    eprintln!("{} closed", w.session);
                }
                return Ok(IDLE);
            }
            None => {
                let limit = w.unknown_timeout_secs;
                if limit > 0 && started.elapsed() >= Duration::from_secs(limit) {
                    eprintln!(
                        "motty: session {} reported no state in {limit}s (hooks are read when \
                         a session starts: restart it after installing them)",
                        w.session
                    );
                    return Ok(NEVER_REPORTED);
                }
            }
        }
        if w.timeout_secs > 0 && started.elapsed() >= Duration::from_secs(w.timeout_secs) {
            eprintln!(
                "motty: timed out; session {} is still {}",
                w.session,
                seen.unwrap_or("unknown")
            );
            return Ok(TIMED_OUT);
        }
        std::thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_watch_flags_and_codes() {
        let w = WatchAgent::try_parse_from(["x", "9f3c", "--interval-ms", "2000", "-v"]).unwrap();
        assert_eq!(
            (w.interval_ms, w.timeout_secs, w.unknown_timeout_secs),
            (2000, 0, 60)
        );
        assert!(w.verbose);
        assert!(WatchAgent::try_parse_from(["x"]).is_err());
        assert_eq!(
            [IDLE, NEVER_REPORTED, NO_INTEGRATION, TIMED_OUT],
            [0, 4, 6, 9]
        );
    }

    #[test]
    fn states_from_panes() {
        let mut p: PaneInfo = serde_json::from_value(json!({
            "id": 1, "window": 0, "tab": 0, "title": "", "cwd": null, "process": null,
            "busy": true, "focused": false, "cols": 80, "rows": 24,
            "shell_integration": false, "agent": "Claude Code", "in_turn": true,
        }))
        .unwrap();
        assert_eq!(state(&p), "processing");
        p.badge = Some("awaiting".into());
        assert_eq!(state(&p), "awaiting-input");
        p.badge = None;
        p.in_turn = false;
        assert_eq!(state(&p), "idle");
    }

    #[test]
    fn notification_text() {
        assert_eq!(sanitize("make; deploy\n"), "make  deploy ");
        assert_eq!(format_duration(Duration::from_millis(3300)), "3.3s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
    }
}
