//! Installing and removing the hooks that make each agent report to Motty.
//!
//! Every entry Motty writes runs `motty hook <agent>`, and that command line
//! is how it's found again: removing takes out exactly those entries and
//! leaves the rest of the file as it was. Where an agent loads plugin files
//! from a folder, Motty writes its own file there and deletes it on removal.
//! A file Motty can't read is never written.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// Where an agent's settings live: the home folder, and the environment
/// variables that move them.
pub struct Env {
    pub home: PathBuf,
    pub var: fn(&str) -> Option<String>,
}

impl Env {
    pub fn current() -> Self {
        Self {
            home: dirs::home_dir().unwrap_or_default(),
            var: |k| std::env::var(k).ok().filter(|v| !v.is_empty()),
        }
    }

    /// `$var` if it's set, else `default` under the home folder.
    fn dir(&self, var: &str, default: &str) -> PathBuf {
        match (self.var)(var) {
            Some(dir) => PathBuf::from(dir),
            None => self.home.join(default),
        }
    }
}

/// The file an agent's switch writes, or the plugin file it adds.
pub fn target(id: &str, env: &Env) -> Option<PathBuf> {
    Some(match id {
        "claude" | "grok" => env
            .dir("CLAUDE_CONFIG_DIR", ".claude")
            .join("settings.json"),
        "codex" => env.dir("CODEX_HOME", ".codex").join("hooks.json"),
        "codebuddy" => env.home.join(".codebuddy/settings.json"),
        "cursor" => env.home.join(".cursor/hooks.json"),
        "reasonix" => env.dir("REASONIX_HOME", ".reasonix").join("settings.json"),
        "kimi" => env.dir("KIMI_CODE_HOME", ".kimi-code").join("config.toml"),
        "copilot" => env.dir("COPILOT_HOME", ".copilot").join("hooks/motty.json"),
        "antigravity" => env.home.join(".gemini/config/hooks.json"),
        "opencode" => env
            .dir("XDG_CONFIG_HOME", ".config")
            .join("opencode/plugins/motty.js"),
        "pi" => env
            .dir("PI_CODING_AGENT_DIR", ".pi/agent")
            .join("extensions/motty.ts"),
        "omp" => env.home.join(".omp/agent/extensions/motty.ts"),
        _ => return None,
    })
}

/// What to do after installing, when there's something.
pub fn note(id: &str) -> Option<&'static str> {
    Some(match id {
        "codex" => {
            "Run /hooks in Codex once and trust Motty's hooks: Codex runs no hook it hasn't been told to trust."
        }
        "grok" => {
            "Grok Build runs the hooks in Claude Code's settings, so this switch and Claude Code's are the same."
        }
        "opencode" => {
            "Send one message after restarting OpenCode so the plugin reports the session."
        }
        "cursor" => {
            "Cursor tells no one when it waits for approval, so its tab never shows the hand."
        }
        "antigravity" | "pi" | "omp" => {
            "This agent doesn't report waiting for approval, so its tab never shows the hand."
        }
        _ => return None,
    })
}

/// Whether Motty's entries are in the agent's settings.
pub fn installed(id: &str, env: &Env) -> bool {
    let Some(path) = target(id, env) else {
        return false;
    };
    match id {
        "opencode" | "pi" | "omp" | "copilot" => path.exists(),
        "kimi" => std::fs::read_to_string(&path).is_ok_and(|s| s.contains(&marker("kimi"))),
        "antigravity" => read_json(&path)
            .ok()
            .is_some_and(|v| v.get(GROUP).is_some()),
        _ => read_json(&path)
            .ok()
            .is_some_and(|v| json_has_ours(&v, hook_id(id))),
    }
}

/// Adds Motty's entries (replacing any it wrote before); returns the file.
pub fn install(id: &str, env: &Env, cli: &Cli) -> Result<PathBuf, String> {
    let path = target(id, env).ok_or_else(|| format!("Motty can't set up {id}"))?;
    let hook = hook_id(id);
    match id {
        "claude" | "grok" | "codex" | "codebuddy" => edit_json(&path, |root| {
            remove_nested(root, hook);
            for (event, matcher, timeout) in nested_events(hook) {
                let mut group = Map::new();
                if let Some(m) = matcher {
                    group.insert("matcher".into(), m.into());
                }
                group.insert(
                    "hooks".into(),
                    json!([{"type": "command", "command": cli.command(hook, None), "timeout": timeout}]),
                );
                push(root, event, Value::Object(group));
            }
        }),
        "cursor" => edit_json(&path, |root| {
            remove_flat(root, hook);
            root.entry("version").or_insert(json!(1));
            for event in [
                "beforeSubmitPrompt",
                "afterFileEdit",
                "stop",
                "sessionStart",
                "sessionEnd",
            ] {
                push(
                    root,
                    event,
                    json!({"command": cli.command(hook, None), "timeout": 10}),
                );
            }
        }),
        "reasonix" => edit_json(&path, |root| {
            remove_flat(root, hook);
            for event in [
                "UserPromptSubmit",
                "PermissionRequest",
                "Notification",
                "Stop",
                "StopFailure",
                "SessionStart",
                "SessionEnd",
            ] {
                // Reasonix counts in milliseconds.
                push(
                    root,
                    event,
                    json!({"command": cli.command(hook, None), "timeout": 5000}),
                );
            }
        }),
        "antigravity" => edit_json(&path, |root| {
            // Its events carry no name, so each hook says which it is.
            let hook = |event| json!([{"type": "command", "command": cli.command("antigravity", Some(event)), "timeout": 10}]);
            root.insert(
                GROUP.into(),
                json!({"PreInvocation": hook("PreInvocation"), "Stop": hook("Stop")}),
            );
        }),
        "copilot" => {
            // Copilot's camelCase events carry no name either.
            let mut hooks = Map::new();
            for event in [
                "sessionStart",
                "userPromptSubmitted",
                "notification",
                "agentStop",
                "errorOccurred",
                "sessionEnd",
            ] {
                hooks.insert(
                    event.into(),
                    json!([{"type": "command", "bash": cli.command("copilot", Some(event)), "timeoutSec": 5}]),
                );
            }
            let text = pretty(&json!({"version": 1, "hooks": hooks}));
            write(&path, &text).map(|_| path)
        }
        "kimi" => {
            let src = read_text(&path)?;
            let mut out = remove_kimi(&src);
            if !out.is_empty() && !out.ends_with("\n\n") {
                out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
            }
            out.push_str("# Motty: tells Motty's tab what Kimi Code is doing.\n");
            let command = toml_string(&cli.command("kimi", None));
            for event in [
                "SessionStart",
                "UserPromptSubmit",
                "PermissionRequest",
                "Stop",
                "StopFailure",
                "Interrupt",
                "SessionEnd",
            ] {
                out.push_str(&format!(
                    "[[hooks]]\nevent = \"{event}\"\ncommand = {command}\ntimeout = 10\n\n"
                ));
            }
            out.truncate(out.trim_end().len() + 1);
            write(&path, &out).map(|_| path)
        }
        "opencode" => plugin(&path, OPENCODE, cli),
        // omp has no `agent_settled`; its `agent_end` is the last word.
        "pi" | "omp" => {
            let settled = if id == "pi" {
                "agent_settled"
            } else {
                "agent_end"
            };
            let source = PI.replace("{{SETTLED}}", settled).replace("{{ID}}", id);
            plugin(&path, &source, cli)
        }
        _ => Err(format!("Motty can't set up {id}")),
    }
}

/// Takes Motty's entries out again; returns the file.
pub fn uninstall(id: &str, env: &Env) -> Result<PathBuf, String> {
    let path = target(id, env).ok_or_else(|| format!("Motty can't set up {id}"))?;
    let hook = hook_id(id);
    match id {
        "opencode" | "pi" | "omp" | "copilot" => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(path),
        },
        "kimi" => {
            let src = read_text(&path)?;
            let out = remove_kimi(&src);
            if out != src {
                write(&path, &out)?;
            }
            Ok(path)
        }
        _ if !path.exists() => Ok(path),
        "antigravity" => edit_json(&path, |root| {
            root.remove(GROUP);
        }),
        "cursor" | "reasonix" => edit_json(&path, |root| remove_flat(root, hook)),
        _ => edit_json(&path, |root| remove_nested(root, hook)),
    }
}

/// How hooks run the `motty` CLI.
pub struct Cli {
    /// As a shell runs it: `motty`, or a quoted path into the app.
    pub shell: String,
    /// The program's path, for plugins that start it themselves.
    pub path: String,
}

impl Cli {
    pub fn current() -> Self {
        Self {
            shell: crate::workspace::cli_command(),
            path: crate::workspace::cli_path()
                .map_or_else(|| "motty".into(), |p| p.to_string_lossy().into_owned()),
        }
    }

    fn command(&self, hook: &str, event: Option<&str>) -> String {
        match event {
            Some(event) => format!("{} hook {hook} {event}", self.shell),
            None => format!("{} hook {hook}", self.shell),
        }
    }
}

/// The antigravity hooks group Motty owns.
const GROUP: &str = "motty";

/// Grok runs Claude Code's hooks, and tells them apart itself.
fn hook_id(id: &str) -> &str {
    if id == "grok" { "claude" } else { id }
}

/// What every command Motty writes contains: `motty hook claude`, also
/// when the program is a quoted path ending in `motty'`.
fn marker(hook: &str) -> String {
    format!(" hook {hook}")
}

fn is_ours(command: &str, hook: &str) -> bool {
    let Some((program, rest)) = command.split_once(&marker(hook)) else {
        return false;
    };
    let program = program.trim_end_matches(['\'', '"']);
    program.ends_with("motty") && (rest.is_empty() || rest.starts_with(' '))
}

/// Claude Code's event list, with a tool matcher and a timeout in seconds.
fn nested_events(hook: &str) -> Vec<(&'static str, Option<&'static str>, u32)> {
    match hook {
        "codex" => vec![
            ("SessionStart", None, 10),
            ("UserPromptSubmit", None, 10),
            ("PreToolUse", None, 10),
            ("PermissionRequest", None, 10),
            ("PostToolUse", None, 10),
            ("Stop", None, 10),
            // Codex gives these one second at most.
            ("Interrupt", None, 1),
            ("SessionEnd", None, 1),
        ],
        "codebuddy" => vec![
            ("SessionStart", None, 10),
            ("UserPromptSubmit", None, 10),
            ("PreToolUse", None, 10),
            ("PermissionRequest", None, 10),
            ("PostToolUse", None, 10),
            ("Notification", None, 10),
            ("Stop", None, 10),
            ("SessionEnd", None, 10),
        ],
        _ => vec![
            ("SessionStart", None, 10),
            ("UserPromptSubmit", None, 10),
            ("PreToolUse", Some("*"), 10),
            ("PermissionRequest", Some("*"), 10),
            ("PostToolUse", Some("*"), 10),
            (
                "Notification",
                Some("permission_prompt|elicitation_dialog"),
                10,
            ),
            ("Stop", None, 10),
            ("StopFailure", None, 10),
            ("SessionEnd", None, 10),
        ],
    }
}

fn hooks_obj(root: &mut Map<String, Value>) -> Option<&mut Map<String, Value>> {
    root.get_mut("hooks")?.as_object_mut()
}

/// Appends to `hooks.<event>`, after whatever is there.
fn push(root: &mut Map<String, Value>, event: &str, entry: Value) {
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let list = hooks
        .as_object_mut()
        .expect("just made an object")
        .entry(event)
        .or_insert_with(|| json!([]));
    if !list.is_array() {
        *list = json!([]);
    }
    list.as_array_mut().expect("just made an array").push(entry);
}

/// Drops empty event lists and an empty `hooks`, so removing leaves the
/// file as it was before installing.
fn prune(root: &mut Map<String, Value>) {
    if let Some(hooks) = hooks_obj(root) {
        hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
        if hooks.is_empty() {
            root.remove("hooks");
        }
    }
}

/// Claude Code's shape: `hooks.<event>[].hooks[].command`.
fn remove_nested(root: &mut Map<String, Value>, hook: &str) {
    let Some(hooks) = hooks_obj(root) else {
        return;
    };
    for groups in hooks.values_mut().filter_map(Value::as_array_mut) {
        groups.retain_mut(|group| {
            let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = list.len();
            list.retain(|h| !command_of(h).is_some_and(|c| is_ours(c, hook)));
            !(list.is_empty() && before > 0)
        });
    }
    prune(root);
}

/// Cursor's and Reasonix's shape: `hooks.<event>[].command`.
fn remove_flat(root: &mut Map<String, Value>, hook: &str) {
    let Some(hooks) = hooks_obj(root) else {
        return;
    };
    for list in hooks.values_mut().filter_map(Value::as_array_mut) {
        list.retain(|h| !command_of(h).is_some_and(|c| is_ours(c, hook)));
    }
    prune(root);
}

fn command_of(entry: &Value) -> Option<&str> {
    entry.get("command").or_else(|| entry.get("bash"))?.as_str()
}

/// Whether any command in the file, at any depth, is one of Motty's.
fn json_has_ours(v: &Value, hook: &str) -> bool {
    match v {
        Value::String(s) => is_ours(s, hook),
        Value::Array(a) => a.iter().any(|v| json_has_ours(v, hook)),
        Value::Object(o) => o.values().any(|v| json_has_ours(v, hook)),
        _ => false,
    }
}

/// Kimi's config.toml without Motty's `[[hooks]]` tables and the comment
/// above them.
fn remove_kimi(src: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim() == "[[hooks]]" {
            let end = (i + 1..lines.len())
                .find(|&j| lines[j].trim_start().starts_with('['))
                .unwrap_or(lines.len());
            let ours = lines[i + 1..end].iter().any(|l| {
                l.trim_start().starts_with("command")
                    && l.split_once('=')
                        .is_some_and(|(_, v)| is_ours(&unquote(v.trim()), "kimi"))
            });
            if ours {
                while out.last().is_some_and(|l| l.trim().is_empty()) {
                    out.pop();
                }
                if out.last().is_some_and(|l| l.starts_with("# Motty:")) {
                    out.pop();
                }
                i = end;
                continue;
            }
        }
        out.push(line);
        i += 1;
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    if out.is_empty() {
        return String::new();
    }
    out.join("\n") + "\n"
}

/// A TOML basic string's text (Motty writes nothing fancier).
fn unquote(v: &str) -> String {
    let v = v.split(" #").next().unwrap_or(v).trim();
    let inner = v
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(v);
    inner.replace("\\\"", "\"").replace("\\\\", "\\")
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The real file behind a symlinked config, so dotfile links survive.
fn resolve(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

fn read_text(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("Couldn't read {}: {e}", path.display())),
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = read_text(path)?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|e| {
        format!(
            "{} isn't valid JSON ({e}), so Motty left it alone",
            path.display()
        )
    })
}

fn edit_json(path: &Path, f: impl FnOnce(&mut Map<String, Value>)) -> Result<PathBuf, String> {
    let mut value = read_json(path)?;
    let Some(root) = value.as_object_mut() else {
        return Err(format!(
            "{} isn't a JSON object, so Motty left it alone",
            path.display()
        ));
    };
    let before = root.clone();
    f(root);
    if *root != before {
        write(path, &pretty(&value))?;
    }
    Ok(path.to_owned())
}

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_default();
    s.push('\n');
    s
}

/// Writes through a temporary file, so a crash never leaves half a file.
fn write(path: &Path, text: &str) -> Result<(), String> {
    let path = resolve(path);
    let fail = |e: std::io::Error| format!("Couldn't write {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let tmp = path.with_extension("motty-tmp");
    std::fs::write(&tmp, text).map_err(fail)?;
    if let Ok(meta) = std::fs::metadata(&path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, &path).map_err(fail)
}

fn plugin(path: &Path, template: &str, cli: &Cli) -> Result<PathBuf, String> {
    let cli = serde_json::to_string(&cli.path).unwrap_or_else(|_| "\"motty\"".into());
    write(path, &template.replace("{{CLI}}", &cli))?;
    Ok(path.to_owned())
}

const OPENCODE: &str = include_str!("opencode.js");
const PI: &str = include_str!("pi.ts");

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_env(name: &str) -> Env {
        let home = std::env::temp_dir().join(format!("motty-hooks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        Env {
            home,
            var: |_| None,
        }
    }

    fn cli() -> Cli {
        Cli {
            shell: "'/Applications/Motty App.app/Contents/MacOS/motty'".into(),
            path: "/Applications/Motty App.app/Contents/MacOS/motty".into(),
        }
    }

    #[test]
    fn recognises_its_own_commands() {
        assert!(is_ours("motty hook claude", "claude"));
        assert!(is_ours("'/A B/motty' hook kimi", "kimi"));
        assert!(is_ours("/x/motty hook copilot agentStop", "copilot"));
        assert!(!is_ours("motty hook claudex", "claude"));
        assert!(!is_ours("orca hook claude", "claude"));
        assert!(!is_ours("motty hook codex", "claude"));
    }

    #[test]
    fn claude_round_trip_keeps_other_hooks() {
        let env = temp_env("claude");
        let path = target("claude", &env).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"{
  "model": "opus",
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "orca-hook stop"
          }
        ]
      }
    ]
  },
  "zeta": true
}
"#;
        std::fs::write(&path, original).unwrap();
        assert!(!installed("claude", &env));
        install("claude", &env, &cli()).unwrap();
        install("claude", &env, &cli()).unwrap();
        assert!(installed("claude", &env));
        assert!(installed("grok", &env));
        let v = read_json(&path).unwrap();
        // Twice installed is once installed, after the existing hook.
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(
            v["hooks"]["Stop"][0]["hooks"][0]["command"],
            "orca-hook stop"
        );
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "*");
        let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["model", "hooks", "zeta"]);
        uninstall("grok", &env).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn never_writes_a_file_it_cant_read() {
        let env = temp_env("bad");
        let path = target("codex", &env).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(install("codex", &env, &cli()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn flat_and_named_shapes() {
        let env = temp_env("flat");
        let cursor = target("cursor", &env).unwrap();
        std::fs::create_dir_all(cursor.parent().unwrap()).unwrap();
        std::fs::write(
            &cursor,
            r#"{"version":1,"hooks":{"stop":[{"command":"other"}]}}"#,
        )
        .unwrap();
        install("cursor", &env, &cli()).unwrap();
        let v = read_json(&cursor).unwrap();
        assert_eq!(v["hooks"]["stop"].as_array().unwrap().len(), 2);
        uninstall("cursor", &env).unwrap();
        let v = read_json(&cursor).unwrap();
        assert_eq!(v["hooks"]["stop"], json!([{"command": "other"}]));
        assert!(v["hooks"].get("sessionStart").is_none());

        install("antigravity", &env, &cli()).unwrap();
        assert!(installed("antigravity", &env));
        let v = read_json(&target("antigravity", &env).unwrap()).unwrap();
        assert!(
            v["motty"]["Stop"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with("hook antigravity Stop")
        );
        uninstall("antigravity", &env).unwrap();
        assert!(!installed("antigravity", &env));

        install("copilot", &env, &cli()).unwrap();
        assert!(installed("copilot", &env));
        uninstall("copilot", &env).unwrap();
        assert!(!installed("copilot", &env));
    }

    #[test]
    fn kimi_toml_round_trip() {
        let env = temp_env("kimi");
        let path = target("kimi", &env).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = "model = \"k2\"\n\n[[hooks]]\nevent = \"Stop\"\ncommand = \"say done\"\n";
        std::fs::write(&path, original).unwrap();
        install("kimi", &env, &cli()).unwrap();
        install("kimi", &env, &cli()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("[[hooks]]").count(), 8, "{text}");
        assert!(text.contains(
            r#"command = "'/Applications/Motty App.app/Contents/MacOS/motty' hook kimi""#
        ));
        assert!(installed("kimi", &env));
        uninstall("kimi", &env).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn plugins_name_the_cli() {
        let env = temp_env("plugins");
        for id in ["opencode", "pi", "omp"] {
            let path = install(id, &env, &cli()).unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains(r#""/Applications/Motty App.app/Contents/MacOS/motty""#));
            assert!(!text.contains("{{"));
            uninstall(id, &env).unwrap();
            assert!(!path.exists());
        }
    }
}
