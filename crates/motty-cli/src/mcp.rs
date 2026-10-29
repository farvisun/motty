//! `motty mcp`: a Model Context Protocol server on stdio, so coding agents
//! (Claude Code, Codex, ...) can see and drive Motty's panes.
//!
//! JSON-RPC 2.0, one message per line. Each tool call opens its own socket
//! connection, so the server survives Motty restarting.

use std::io::{BufRead, Write};

use anyhow::{Context, bail};
use motty_ipc::{
    Client, Created, Direction, FindParams, FindResult, NewTabParams, PaneInfo, PaneRef,
    ReadParams, RunParams, RunResult, SendTextParams, SplitParams, Submit, TextRange, method,
};
use serde_json::{Value, json};

const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn serve() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                write(
                    &mut stdout,
                    &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}}),
                )?;
                continue;
            }
        };
        // Notifications (no id) need no answer.
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let reply = match message["method"].as_str().unwrap_or_default() {
            "initialize" => Ok(json!({
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "motty", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => Ok(call_tool(&params)),
            other => Err(json!({"code": -32601, "message": format!("unknown method {other}")})),
        };
        let response = match reply {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        };
        write(&mut stdout, &response)?;
    }
    Ok(())
}

fn write(out: &mut impl Write, message: &Value) -> anyhow::Result<()> {
    writeln!(out, "{message}")?;
    out.flush()?;
    Ok(())
}

const INSTRUCTIONS: &str = "Motty is the terminal this session may be running in. \
Use list_panes to see its panes (the one marked `caller` is yours, if any). \
Prefer run_command in a separate pane (split_pane first) for long or interactive work, \
and never send text to your own pane.";

fn tools() -> Value {
    let pane = json!({"type": "integer", "description": "Pane id from list_panes"});
    json!([
        {
            "name": "list_panes",
            "description": "List Motty's panes: id, window, tab, title, cwd, foreground process, busy, focus, size, and whether shell integration is active.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "read_pane",
            "description": "Read text from a pane: the visible screen (default), the last N lines including scrollback, or the output of the last finished command.",
            "inputSchema": {"type": "object", "required": ["pane"], "properties": {
                "pane": pane,
                "lines": {"type": "integer", "minimum": 1, "description": "Last N lines, scrollback included"},
                "last_command": {"type": "boolean", "description": "Output of the last finished command (needs shell integration)"}
            }},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "find_in_pane",
            "description": "Search a pane's screen and scrollback. Returns each match's line (0 is the top of the screen, negative lines are scrollback), column and the line's text.",
            "inputSchema": {"type": "object", "required": ["pane", "query"], "properties": {
                "pane": pane,
                "query": {"type": "string"},
                "regex": {"type": "boolean", "description": "Treat the query as a Rust regular expression"},
                "case_sensitive": {"type": "boolean"},
                "whole_word": {"type": "boolean"},
                "limit": {"type": "integer", "minimum": 1, "description": "Matches to return (default 50)"}
            }},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "run_command",
            "description": "Type a command line into a pane's shell, wait until it finishes, and return its output and exit status. The pane must be at a prompt with shell integration.",
            "inputSchema": {"type": "object", "required": ["pane", "command"], "properties": {
                "pane": pane,
                "command": {"type": "string"},
                "timeout_seconds": {"type": "number", "description": "Give up waiting after this long (default 600)"}
            }}
        },
        {
            "name": "send_text",
            "description": "Type text into a pane, e.g. answers for an interactive program, or a prompt for the coding agent there. Include \\n (or \\r) to press Return, or set submit.",
            "inputSchema": {"type": "object", "required": ["pane", "text"], "properties": {
                "pane": pane,
                "text": {"type": "string"},
                "submit": {"type": "string", "enum": ["queue", "now"], "description": "Press Return too: `queue` waits until the agent finishes its turn or the shell is back at an empty prompt; `now` sends at once"}
            }}
        },
        {
            "name": "split_pane",
            "description": "Split a pane (default: the caller's own pane, else the focused one) and start a shell, or a program, in the new half. Returns the new pane's id.",
            "inputSchema": {"type": "object", "properties": {
                "pane": pane,
                "direction": {"type": "string", "enum": ["left", "right", "up", "down"], "default": "right"},
                "command": {"type": "array", "items": {"type": "string"}, "description": "Program and arguments instead of a shell"},
                "cwd": {"type": "string"},
                "focus": {"type": "boolean", "default": false, "description": "Move focus to the new pane"}
            }}
        },
        {
            "name": "new_tab",
            "description": "Open a tab with a shell, or a program, in the frontmost window. Returns the new pane's id.",
            "inputSchema": {"type": "object", "properties": {
                "command": {"type": "array", "items": {"type": "string"}},
                "cwd": {"type": "string"}
            }}
        },
        {
            "name": "focus_pane",
            "description": "Bring a pane (its tab and window) to the front.",
            "inputSchema": {"type": "object", "required": ["pane"], "properties": {"pane": pane}}
        },
        {
            "name": "close_pane",
            "description": "Close a pane, ending whatever runs in it.",
            "inputSchema": {"type": "object", "required": ["pane"], "properties": {"pane": pane}},
            "annotations": {"destructiveHint": true}
        }
    ])
}

/// A tool result: text content, flagged as an error when the call failed.
fn call_tool(params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or_default();
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match tool(name, &args) {
        Ok(text) => json!({"content": [{"type": "text", "text": text}]}),
        Err(e) => json!({"content": [{"type": "text", "text": format!("{e:#}")}], "isError": true}),
    }
}

fn caller_pane() -> Option<u64> {
    std::env::var(motty_ipc::PANE_ENV).ok()?.parse().ok()
}

fn tool(name: &str, args: &Value) -> anyhow::Result<String> {
    let mut client = Client::connect()?;
    let pane = || -> anyhow::Result<PaneRef> {
        Ok(PaneRef::id(Some(
            args["pane"]
                .as_u64()
                .context("`pane` (an id from list_panes) is required")?,
        )))
    };
    let strings = |v: &Value| -> Option<Vec<String>> {
        v.as_array().map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
    };
    match name {
        "list_panes" => {
            let panes: Vec<PaneInfo> = client.call(method::LIST_PANES, &Value::Null)?;
            let caller = caller_pane();
            let panes: Vec<Value> = panes
                .into_iter()
                .map(|p| {
                    let mut v = serde_json::to_value(&p).unwrap_or_default();
                    v["caller"] = json!(Some(p.id) == caller);
                    v
                })
                .collect();
            Ok(serde_json::to_string_pretty(&panes)?)
        }
        "read_pane" => {
            let range = if args["last_command"].as_bool() == Some(true) {
                TextRange::LastCommand
            } else if let Some(n) = args["lines"].as_u64() {
                TextRange::Lines(n as usize)
            } else {
                TextRange::Screen
            };
            let out: Value = client.call(
                method::READ,
                &ReadParams {
                    pane: pane()?,
                    range,
                },
            )?;
            Ok(out["text"].as_str().unwrap_or_default().to_owned())
        }
        "find_in_pane" => {
            let params = FindParams {
                pane: pane()?,
                query: args["query"]
                    .as_str()
                    .context("`query` is required")?
                    .to_owned(),
                case_sensitive: args["case_sensitive"].as_bool() == Some(true),
                whole_word: args["whole_word"].as_bool() == Some(true),
                regex: args["regex"].as_bool() == Some(true),
                limit: Some(args["limit"].as_u64().unwrap_or(50) as usize),
                show: false,
            };
            let result: FindResult = client.call(method::FIND, &params)?;
            let mut out = format!("{} matches\n", result.total);
            for m in &result.matches {
                out.push_str(&format!("{}:{}: {}\n", m.line, m.column + 1, m.line_text));
            }
            Ok(out)
        }
        "run_command" => {
            let pane = pane()?;
            if pane.pane == caller_pane() {
                bail!("that is your own pane; split_pane first and run the command there");
            }
            let command = args["command"]
                .as_str()
                .context("`command` is required")?
                .to_owned();
            let secs = args["timeout_seconds"].as_f64().unwrap_or(600.);
            client.set_timeout(Some(std::time::Duration::from_secs_f64(secs + 2.)))?;
            let params = RunParams {
                pane,
                command,
                timeout_ms: Some((secs * 1000.) as u64),
            };
            let result: RunResult = client.call(method::RUN, &params)?;
            let status = result.exit.map_or("unknown".into(), |c| c.to_string());
            Ok(format!("exit status: {status}\n{}", result.output))
        }
        "send_text" => {
            let pane = pane()?;
            if pane.pane == caller_pane() {
                bail!("refusing to type into your own pane");
            }
            let text = args["text"]
                .as_str()
                .context("`text` is required")?
                .to_owned();
            let submit = match args["submit"].as_str() {
                Some("queue") => Some(Submit::Queue),
                Some("now") => Some(Submit::Now),
                _ => None,
            };
            let _: Value =
                client.call(method::SEND_TEXT, &SendTextParams { pane, text, submit })?;
            Ok("sent".into())
        }
        "split_pane" => {
            let direction = match args["direction"].as_str().unwrap_or("right") {
                "left" => Direction::Left,
                "up" => Direction::Up,
                "down" => Direction::Down,
                _ => Direction::Right,
            };
            let params = SplitParams {
                pane: PaneRef::id(args["pane"].as_u64().or_else(caller_pane)),
                direction,
                command: strings(&args["command"]).filter(|c| !c.is_empty()),
                cwd: args["cwd"].as_str().map(str::to_owned),
                no_focus: args["focus"].as_bool() != Some(true),
                size: None,
                line: None,
            };
            let created: Created = client.call(method::SPLIT, &params)?;
            Ok(format!("new pane {}", created.pane))
        }
        "new_tab" => {
            let params = NewTabParams {
                command: strings(&args["command"]).filter(|c| !c.is_empty()),
                cwd: args["cwd"].as_str().map(str::to_owned),
                ..Default::default()
            };
            let created: Created = client.call(method::NEW_TAB, &params)?;
            Ok(format!("new pane {}", created.pane))
        }
        "focus_pane" => {
            let _: Value = client.call(method::FOCUS, &pane()?)?;
            Ok("focused".into())
        }
        "close_pane" => {
            let _: Value = client.call(method::CLOSE, &pane()?)?;
            Ok("closed".into())
        }
        other => bail!("unknown tool {other}"),
    }
}
