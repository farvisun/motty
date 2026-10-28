//! Coding agents reporting their state to Motty.
//!
//! `motty hook <agent>` is what the hooks Motty installs run: the agent
//! hands it the event (JSON on stdin, or as the last argument for Codex's
//! `notify`), and it tells Motty what that means for the tab. A hook must
//! never slow down or break the agent, so it prints nothing, gives up
//! quickly and always succeeds.
//!
//! `motty state:<name> state=processing …` is the same report from any
//! other agent's own hook script, in Otty's syntax.

use std::io::Read;
use std::process::ExitCode;
use std::time::Duration;

use motty_ipc::{AgentState, AgentStateParams, Client, PaneRef, method};
use serde_json::Value;

/// How long a report may take before the hook gives up.
const TIMEOUT: Duration = Duration::from_millis(800);
/// Messages are cut to this many characters.
const MESSAGE_LEN: usize = 160;

/// `motty hook <agent> [args…]`.
pub fn hook(agent: &str, args: &[String]) -> ExitCode {
    let payload = read_payload(args);
    if let Some(report) = report_for(agent, &payload, args) {
        let _ = send(agent, report, None, None);
    }
    ExitCode::SUCCESS
}

/// `motty state:<name> state=<state> [agent-pid=…] [session-id=…]
/// [label=…] [message=…]`.
pub fn state(name: &str, args: &[String]) -> ExitCode {
    let mut state = None;
    let mut report = Report::default();
    let (mut pid, mut label) = (None, None);
    for arg in args {
        let Some((key, value)) = arg.split_once('=') else {
            eprintln!("motty: expected key=value, got `{arg}`");
            return ExitCode::from(2);
        };
        match key {
            "state" => state = AgentState::parse(value),
            "agent-pid" => pid = value.parse().ok(),
            "session-id" => report.session = Some(value.to_owned()),
            "label" => label = Some(value.to_owned()),
            "message" | "msg" => report.message = Some(clip(value)),
            _ => {}
        }
    }
    let Some(state) = state else {
        eprintln!("motty: state= must be processing, awaiting, idle or error");
        return ExitCode::from(2);
    };
    report.state = Some(state);
    match send(name, report, pid, label) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("motty: {e:#}");
            ExitCode::from(1)
        }
    }
}

#[derive(Debug, Default, PartialEq)]
struct Report {
    state: Option<AgentState>,
    message: Option<String>,
    session: Option<String>,
}

fn send(
    agent: &str,
    report: Report,
    agent_pid: Option<u32>,
    label: Option<String>,
) -> anyhow::Result<()> {
    let pane = std::env::var(motty_ipc::PANE_ENV)
        .ok()
        .and_then(|p| p.parse().ok());
    if pane.is_none() && agent_pid.is_none() {
        anyhow::bail!(
            "not in a Motty pane (no ${} or agent-pid=)",
            motty_ipc::PANE_ENV
        );
    }
    let Some(state) = report.state else {
        return Ok(());
    };
    let mut client = Client::connect()?;
    client.set_timeout(Some(TIMEOUT))?;
    let params = AgentStateParams {
        pane: PaneRef { pane },
        agent: agent.to_owned(),
        state,
        // The hook's parent is the agent.
        agent_pid: agent_pid.or_else(|| Some(std::os::unix::process::parent_id())),
        session_id: report.session,
        label,
        message: report.message,
    };
    let _: Value = client.call(method::AGENT_STATE, &params)?;
    Ok(())
}

/// The event: a JSON argument (Codex's `notify`) or JSON on stdin.
fn read_payload(args: &[String]) -> Value {
    if let Some(json) = args.iter().rev().find(|a| a.trim_start().starts_with('{')) {
        return serde_json::from_str(json).unwrap_or(Value::Null);
    }
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Value::Null;
    }
    let mut text = String::new();
    let _ = std::io::stdin().take(4 << 20).read_to_string(&mut text);
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

fn str_at<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| v.get(k)?.as_str())
        .filter(|s| !s.is_empty())
}

/// What an agent's event means: Claude Code's hook events (which Grok,
/// CodeBuddy and Codex's hooks share), Cursor's, Codex's `notify`, or a
/// state named on the command line.
fn report_for(agent: &str, payload: &Value, args: &[String]) -> Option<Report> {
    let session = str_at(
        payload,
        &["session_id", "sessionId", "conversation_id", "thread-id"],
    )
    .map(str::to_owned);
    let event = str_at(
        payload,
        &["hook_event_name", "hookEventName", "event", "type"],
    );
    let (state, message) = match event {
        Some("UserPromptSubmit" | "beforeSubmitPrompt" | "user-prompt-submit") => (
            AgentState::Processing,
            str_at(payload, &["prompt"]).map(clip),
        ),
        Some(
            "PreToolUse"
            | "PostToolUse"
            | "SubagentStart"
            | "beforeShellExecution"
            | "beforeMCPExecution"
            | "afterFileEdit"
            | "beforeReadFile",
        ) => (AgentState::Processing, tool_step(payload)),
        Some("PermissionRequest") => (
            AgentState::Awaiting,
            Some(match tool_step(payload) {
                Some(step) => format!("Approve {step}"),
                None => "Needs permission".to_owned(),
            }),
        ),
        Some("Notification") => {
            let kind = str_at(payload, &["notification_type"]);
            let message = str_at(payload, &["message"]);
            let waiting = match kind {
                Some(k) => matches!(k, "permission_prompt" | "elicitation_dialog"),
                None => message.is_some_and(|m| m.contains("permission")),
            };
            if !waiting {
                return None;
            }
            (AgentState::Awaiting, message.map(clip))
        }
        Some("Stop" | "stop" | "agent-turn-complete") => {
            let failed = str_at(payload, &["status"]).is_some_and(|s| s == "error");
            let state = if failed {
                AgentState::Error
            } else {
                AgentState::Idle
            };
            let message = str_at(
                payload,
                &[
                    "last_assistant_message",
                    "last-assistant-message",
                    "message",
                ],
            )
            .map(clip)
            .or_else(|| last_reply(str_at(payload, &["transcript_path"])?));
            (state, message)
        }
        Some("StopFailure") => (
            AgentState::Error,
            str_at(payload, &["error", "message"]).map(clip),
        ),
        Some("SessionStart" | "sessionStart") => (AgentState::Idle, None),
        Some("SessionEnd" | "sessionEnd") => (AgentState::Ended, None),
        // `motty hook myagent processing` from a hand-written hook.
        _ => (args.iter().find_map(|a| AgentState::parse(a))?, None),
    };
    let _ = agent;
    Some(Report {
        state: Some(state),
        message,
        session,
    })
}

/// A tool call in a few words: `Bash: cargo test`, `Edit: src/main.rs`.
fn tool_step(payload: &Value) -> Option<String> {
    if let Some(command) = str_at(payload, &["command"]) {
        return Some(clip(command));
    }
    let tool = str_at(payload, &["tool_name", "toolName"])?;
    let input = payload.get("tool_input").unwrap_or(&Value::Null);
    let detail = str_at(
        input,
        &[
            "command",
            "file_path",
            "path",
            "pattern",
            "url",
            "query",
            "description",
        ],
    );
    Some(clip(&match detail {
        Some(d) => format!("{tool}: {d}"),
        None => tool.to_owned(),
    }))
}

/// Claude Code's last reply, from the end of the session transcript.
fn last_reply(path: &str) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(256 << 10)))
        .ok()?;
    let mut tail = String::new();
    file.read_to_string(&mut tail).ok()?;
    tail.lines().rev().find_map(|line| {
        let entry: Value = serde_json::from_str(line).ok()?;
        if entry.get("type")?.as_str()? != "assistant" {
            return None;
        }
        entry["message"]["content"]
            .as_array()?
            .iter()
            .rev()
            .find_map(|c| (c["type"] == "text").then(|| c["text"].as_str()).flatten())
            .filter(|t| !t.trim().is_empty())
            .map(clip)
    })
}

/// The first non-empty line, cut to [`MESSAGE_LEN`] characters.
fn clip(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= MESSAGE_LEN {
        return line.to_owned();
    }
    let mut out: String = line.chars().take(MESSAGE_LEN - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(payload: Value) -> Option<(AgentState, Option<String>)> {
        report_for("claude", &payload, &[]).map(|r| (r.state.unwrap(), r.message))
    }

    #[test]
    fn claude_events() {
        assert_eq!(
            state(
                json!({"hook_event_name": "UserPromptSubmit", "prompt": "fix the build\nplease"})
            ),
            Some((AgentState::Processing, Some("fix the build".into())))
        );
        assert_eq!(
            state(json!({"hook_event_name": "PreToolUse", "tool_name": "Bash",
                "tool_input": {"command": "cargo test"}})),
            Some((AgentState::Processing, Some("Bash: cargo test".into())))
        );
        assert_eq!(
            state(
                json!({"hook_event_name": "PermissionRequest", "tool_name": "Edit",
                "tool_input": {"file_path": "src/a.rs"}})
            ),
            Some((AgentState::Awaiting, Some("Approve Edit: src/a.rs".into())))
        );
        assert_eq!(
            state(
                json!({"hook_event_name": "Notification", "notification_type": "idle_prompt",
                "message": "Claude is waiting for your input"})
            ),
            None
        );
        assert_eq!(
            state(json!({"hook_event_name": "Notification",
                "message": "Claude needs your permission to use Bash"})),
            Some((
                AgentState::Awaiting,
                Some("Claude needs your permission to use Bash".into())
            ))
        );
        assert_eq!(
            state(json!({"hook_event_name": "SessionEnd"})),
            Some((AgentState::Ended, None))
        );
    }

    #[test]
    fn codex_notify_and_cursor_stop() {
        let args = vec![
            r#"{"type":"agent-turn-complete","thread-id":"t1","last-assistant-message":"All tests pass."}"#
                .to_owned(),
        ];
        let report = report_for("codex", &read_payload(&args), &args).unwrap();
        assert_eq!(report.state, Some(AgentState::Idle));
        assert_eq!(report.message.as_deref(), Some("All tests pass."));
        assert_eq!(report.session.as_deref(), Some("t1"));
        assert_eq!(
            state(json!({"hook_event_name": "stop", "status": "error"})).map(|s| s.0),
            Some(AgentState::Error)
        );
    }

    #[test]
    fn named_state_and_clipping() {
        let args = vec!["awaiting".to_owned()];
        assert_eq!(
            report_for("mine", &Value::Null, &args).and_then(|r| r.state),
            Some(AgentState::Awaiting)
        );
        assert_eq!(clip(&"x".repeat(500)).chars().count(), MESSAGE_LEN);
    }
}
