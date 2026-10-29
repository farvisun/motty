//! Past coding-agent sessions on disk: finding them for the Agents list,
//! reading one back as a transcript, and the commands that resume or fork it.
//!
//! Every agent keeps its own store in its own format. All of it is read
//! only, and leniently: a line or file that doesn't parse is skipped.
//! Listing stats every file but reads only the head (and, for Claude Code,
//! the tail) of the newest ones.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

/// How much of a session file listing reads from the start.
const HEAD: u64 = 256 * 1024;
/// How much of a long Claude Code session it reads from the end, where the
/// latest title is.
const TAIL: u64 = 64 * 1024;
const TITLE_LEN: usize = 120;
const SUMMARY_LEN: usize = 100;
/// Lines of a tool's output a transcript shows.
const OUTPUT_LINES: usize = 3;

#[derive(Clone, Debug)]
pub struct Session {
    /// The catalog id: `claude`, `codex`, `opencode`, `pi`, `omp`, `cursor`,
    /// `kimi`.
    pub agent: &'static str,
    /// What resuming takes; for Pi and omp, the file's path.
    pub id: String,
    pub path: PathBuf,
    pub cwd: Option<PathBuf>,
    /// The agent's title for it, else its first prompt, on one line.
    pub title: String,
    pub modified: SystemTime,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    User,
    Assistant,
    Tool,
    /// The agent's thinking (Claude) or reasoning summary (Codex).
    Reasoning,
}

/// One turn of a transcript. A tool call reads `⏺ Name  summary`, and its
/// output follows as a `Tool` message starting with `⎿ `.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// The timestamp as the file has it.
    pub time: Option<String>,
}

/// Where to look: Claude config dirs besides the default (from
/// `$CLAUDE_CONFIG_DIR` and ones remembered from hooks).
pub struct Roots {
    pub home: PathBuf,
    pub claude: Vec<PathBuf>,
    /// `$CODEX_HOME`, when set.
    pub codex: Option<PathBuf>,
    /// `$KIMI_CODE_HOME`, when set.
    pub kimi: Option<PathBuf>,
}

impl Roots {
    pub fn current() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let var = |k| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let mut claude = vec![home.join(".claude")];
        claude.extend(var("CLAUDE_CONFIG_DIR"));
        claude.extend(remembered());
        let mut seen = HashSet::new();
        claude.retain(|p| seen.insert(fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
        Self {
            home,
            claude,
            codex: var("CODEX_HOME"),
            kimi: var("KIMI_CODE_HOME"),
        }
    }
}

fn roots_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("Motty/agent-roots.json"))
}

/// Notes the Claude config dir a transcript (`<root>/projects/<project>/<id>.jsonl`)
/// lives in, so sessions under a `$CLAUDE_CONFIG_DIR` Motty wasn't started
/// with are listed too.
pub fn remember_claude_root(transcript: &Path) {
    if let Some(file) = roots_file() {
        remember_in(&file, transcript);
    }
}

fn remembered() -> Vec<PathBuf> {
    roots_file().map(|f| read_roots(&f)).unwrap_or_default()
}

fn read_roots(file: &Path) -> Vec<PathBuf> {
    fs::read(file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn remember_in(file: &Path, transcript: &Path) {
    let Some(root) = claude_root(transcript) else {
        return;
    };
    let mut roots = read_roots(file);
    if roots.contains(&root) {
        return;
    }
    roots.push(root);
    let Ok(json) = serde_json::to_vec_pretty(&roots) else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let temp = file.with_extension("json.tmp");
    if fs::write(&temp, json).is_ok() {
        let _ = fs::rename(&temp, file);
    }
}

fn claude_root(transcript: &Path) -> Option<PathBuf> {
    let projects = transcript.parent()?.parent()?;
    if projects.file_name()? != "projects" {
        return None;
    }
    projects.parent().map(Path::to_path_buf)
}

/// The sessions on disk, newest first, at most `limit`.
pub fn list(roots: &Roots, limit: usize) -> Vec<Session> {
    let mut found = found(roots);
    found.sort_by_key(|f| std::cmp::Reverse(f.modified));
    let mut seen = HashSet::new();
    let mut lookups = Lookups::default();
    found
        .into_iter()
        .filter(|f| seen.insert(f.path.clone()))
        .filter_map(|f| read_session(f, roots, &mut lookups))
        .take(limit)
        .collect()
}

/// A session file, before it's read.
struct Found {
    agent: &'static str,
    path: PathBuf,
    modified: SystemTime,
}

/// What listing looks up once and reuses.
#[derive(Default)]
struct Lookups {
    codex_names: Option<HashMap<String, String>>,
    cursor_dirs: HashMap<OsString, Option<PathBuf>>,
    kimi_dirs: Option<HashMap<String, PathBuf>>,
}

fn found(roots: &Roots) -> Vec<Found> {
    let mut out = Vec::new();
    let jsonl = |n: &str| n.ends_with(".jsonl");
    for root in &roots.claude {
        for project in subdirs(&root.join("projects")) {
            files(&project, "claude", jsonl, &mut out);
        }
    }
    for year in subdirs(&codex_home(roots).join("sessions")) {
        for month in subdirs(&year) {
            for day in subdirs(&month) {
                let rollout = |n: &str| n.starts_with("rollout-") && n.ends_with(".jsonl");
                files(&day, "codex", rollout, &mut out);
            }
        }
    }
    // OpenCode's legacy file store; its newer SQLite database isn't read.
    let opencode = roots.home.join(".local/share/opencode/storage/session");
    for project in subdirs(&opencode) {
        let ses = |n: &str| n.starts_with("ses_") && n.ends_with(".json");
        files(&project, "opencode", ses, &mut out);
    }
    for (agent, dir) in [("pi", ".pi/agent/sessions"), ("omp", ".omp/agent/sessions")] {
        for project in subdirs(&roots.home.join(dir)) {
            files(&project, agent, jsonl, &mut out);
        }
    }
    for project in subdirs(&roots.home.join(".cursor/projects")) {
        for chat in subdirs(&project.join("agent-transcripts")) {
            let Some(name) = chat.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let path = chat.join(format!("{name}.jsonl"));
            if let Some(modified) = file_modified(&path) {
                out.push(Found {
                    agent: "cursor",
                    path,
                    modified,
                });
            }
        }
    }
    // Kimi Code: `sessions/<workspace>/session_<id>/agents/main/wire.jsonl`,
    // the main agent's log (subagents have their own beside it).
    for workspace in subdirs(&kimi_home(roots).join("sessions")) {
        for session in subdirs(&workspace) {
            if !session
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("session_"))
            {
                continue;
            }
            let path = session.join("agents/main/wire.jsonl");
            if let Some(modified) = file_modified(&path) {
                out.push(Found {
                    agent: "kimi",
                    path,
                    modified,
                });
            }
        }
    }
    out
}

/// The session stored at `path`, when it's under a known agent's store
/// (the History view opens these as transcripts).
pub fn at(path: &Path, roots: &Roots) -> Option<Session> {
    let agent = agent_of(path, roots)?;
    let modified = file_modified(path)?;
    let found = Found {
        agent,
        path: path.to_path_buf(),
        modified,
    };
    read_session(found, roots, &mut Lookups::default())
}

/// Which agent's store `path` is in, by the layouts [`found`] walks.
fn agent_of(path: &Path, roots: &Roots) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    let jsonl = name.ends_with(".jsonl");
    // `levels` folders up from the file is `dir`.
    let under = |dir: &Path, levels: usize| {
        path.ancestors()
            .nth(levels)
            .is_some_and(|a| same_dir(a, dir))
    };
    if jsonl && roots.claude.iter().any(|r| under(&r.join("projects"), 2)) {
        return Some("claude");
    }
    if name.starts_with("rollout-") && jsonl && under(&codex_home(roots).join("sessions"), 4) {
        return Some("codex");
    }
    let opencode = roots.home.join(".local/share/opencode/storage/session");
    if name.starts_with("ses_") && name.ends_with(".json") && under(&opencode, 2) {
        return Some("opencode");
    }
    for (agent, dir) in [("pi", ".pi/agent/sessions"), ("omp", ".omp/agent/sessions")] {
        if jsonl && under(&roots.home.join(dir), 2) {
            return Some(agent);
        }
    }
    let chat = path.parent()?;
    if jsonl
        && chat.file_name()?.to_str()? == name.trim_end_matches(".jsonl")
        && chat.parent()?.file_name()? == "agent-transcripts"
        && under(&roots.home.join(".cursor/projects"), 4)
    {
        return Some("cursor");
    }
    None
}

/// Whether two paths name the same folder, through symlinks (`/tmp`).
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || fs::canonicalize(a)
            .ok()
            .is_some_and(|a| fs::canonicalize(b).is_ok_and(|b| a == b))
}

fn codex_home(roots: &Roots) -> PathBuf {
    roots
        .codex
        .clone()
        .unwrap_or_else(|| roots.home.join(".codex"))
}

fn kimi_home(roots: &Roots) -> PathBuf {
    roots
        .kimi
        .clone()
        .unwrap_or_else(|| roots.home.join(".kimi-code"))
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

fn files(dir: &Path, agent: &'static str, wanted: impl Fn(&str) -> bool, out: &mut Vec<Found>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_str().is_some_and(&wanted) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if let (true, Ok(modified)) = (meta.is_file(), meta.modified()) {
            out.push(Found {
                agent,
                path: entry.path(),
                modified,
            });
        }
    }
}

fn file_modified(path: &Path) -> Option<SystemTime> {
    let meta = fs::metadata(path).ok()?;
    meta.is_file().then(|| meta.modified().ok()).flatten()
}

fn read_session(found: Found, roots: &Roots, lookups: &mut Lookups) -> Option<Session> {
    let Found {
        agent,
        path,
        modified,
    } = found;
    let head = match agent {
        "claude" => claude_head(&path),
        "codex" => codex_head(&path, roots, lookups),
        "opencode" => opencode_head(&path),
        "pi" | "omp" => pi_head(&path),
        "cursor" => cursor_head(&path, lookups),
        "kimi" => kimi_head(&path, roots, lookups),
        _ => None,
    }?;
    Some(Session {
        agent,
        id: head.id,
        path,
        cwd: head.cwd,
        title: title(&head.title),
        modified,
    })
}

/// What a session's head says about it.
struct Head {
    id: String,
    cwd: Option<PathBuf>,
    title: String,
}

fn claude_head(path: &Path) -> Option<Head> {
    let len = fs::metadata(path).ok()?.len();
    let mut tail = ClaudeTitles::default();
    if len > HEAD {
        for v in tail_lines(path, HEAD.max(len - TAIL)) {
            tail.note(&v);
        }
    }
    let mut head = ClaudeTitles::default();
    let (mut cwd, mut prompt) = (None, None);
    for v in head_lines(path) {
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(PathBuf::from);
        }
        if prompt.is_none() && v["type"] == "user" {
            prompt = claude_prompt(&v);
        }
        head.note(&v);
        if cwd.is_some() && prompt.is_some() && tail.titled() {
            break;
        }
    }
    let title = tail
        .custom
        .or(head.custom)
        .or(tail.ai)
        .or(head.ai)
        .or(head.summary)
        .or(tail.summary)
        .or(prompt)?;
    Some(Head {
        id: stem(path)?,
        cwd,
        title,
    })
}

/// The titles a Claude Code session file carries, latest kept.
#[derive(Default)]
struct ClaudeTitles {
    custom: Option<String>,
    ai: Option<String>,
    summary: Option<String>,
}

impl ClaudeTitles {
    fn note(&mut self, v: &Value) {
        let text = |key: &str| non_empty(v[key].as_str()).map(str::to_owned);
        match v["type"].as_str() {
            Some("custom-title") => self.custom = text("customTitle").or(self.custom.take()),
            Some("ai-title") => self.ai = text("aiTitle").or(self.ai.take()),
            Some("summary") if self.summary.is_none() => self.summary = text("summary"),
            _ => {}
        }
    }

    fn titled(&self) -> bool {
        self.custom.is_some() || self.ai.is_some()
    }
}

/// Whether a Claude Code line is part of the conversation the user saw.
fn claude_shown(v: &Value) -> bool {
    ["isMeta", "isSidechain", "isCompactSummary"]
        .iter()
        .all(|k| v[*k] != true)
}

fn claude_prompt(v: &Value) -> Option<String> {
    if !claude_shown(v) {
        return None;
    }
    texts(&v["message"]["content"])
        .into_iter()
        .find_map(|t| real(&t).map(str::to_owned))
}

fn codex_head(path: &Path, roots: &Roots, lookups: &mut Lookups) -> Option<Head> {
    let (mut id, mut cwd, mut prompt) = (None, None, None);
    for v in head_lines(path) {
        let p = &v["payload"];
        match v["type"].as_str() {
            Some("session_meta") => {
                if !codex_interactive(&p["source"]) {
                    return None;
                }
                id = p["id"].as_str().map(str::to_owned);
                cwd = p["cwd"].as_str().map(PathBuf::from);
            }
            Some("response_item")
                if prompt.is_none() && p["type"] == "message" && p["role"] == "user" =>
            {
                prompt = texts(&p["content"]).iter().find_map(|t| codex_prompt(t));
            }
            _ => {}
        }
        if id.is_some() && prompt.is_some() {
            break;
        }
    }
    let (id, prompt) = (id?, prompt?);
    let names = lookups
        .codex_names
        .get_or_insert_with(|| codex_names(&codex_home(roots)));
    Some(Head {
        title: names.get(&id).cloned().unwrap_or(prompt),
        id,
        cwd,
    })
}

/// Codex's resume list shows only sessions started from its CLI or editor,
/// not `codex exec` runs or subagents.
fn codex_interactive(source: &Value) -> bool {
    match source {
        Value::Null => true,
        Value::String(s) => matches!(s.as_str(), "cli" | "vscode"),
        _ => false,
    }
}

/// Session names from Codex's index, the latest for each id.
fn codex_names(home: &Path) -> HashMap<String, String> {
    let Ok(file) = File::open(home.join("session_index.jsonl")) else {
        return HashMap::new();
    };
    json_lines(BufReader::new(file))
        .filter_map(|v| {
            let id = v["id"].as_str()?.to_owned();
            Some((id, non_empty(v["thread_name"].as_str())?.to_owned()))
        })
        .collect()
}

/// A user message's own words, without the context Codex adds to it.
fn codex_prompt(text: &str) -> Option<String> {
    let text = match text.split_once("## My request for Codex:") {
        Some((_, request)) => request,
        None => text,
    };
    real(text)
        .filter(|t| !t.starts_with("# AGENTS.md"))
        .map(str::to_owned)
}

fn opencode_head(path: &Path) -> Option<Head> {
    let v: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    if !v["parentID"].is_null() {
        return None;
    }
    let id = v["id"].as_str()?.to_owned();
    let storage = path.parent()?.parent()?.parent()?;
    // An empty session has no messages.
    fs::read_dir(storage.join("message").join(&id))
        .ok()?
        .next()?
        .ok()?;
    let named = non_empty(v["title"].as_str()).filter(|t| !t.starts_with("New session - "));
    let title = match named {
        Some(t) => t.to_owned(),
        None => {
            opencode_messages(storage, &id)
                .into_iter()
                .find(|m| m.role == Role::User)?
                .text
        }
    };
    Some(Head {
        id,
        cwd: v["directory"].as_str().map(PathBuf::from),
        title,
    })
}

fn pi_head(path: &Path) -> Option<Head> {
    let (mut cwd, mut name, mut prompt) = (None, None, None);
    for v in head_lines(path) {
        match v["type"].as_str() {
            Some("session") => cwd = v["cwd"].as_str().map(PathBuf::from),
            Some("session_info") => {
                name = non_empty(v["name"].as_str()).map(str::to_owned).or(name);
            }
            Some("message") if prompt.is_none() && v["message"]["role"] == "user" => {
                prompt = texts(&v["message"]["content"])
                    .iter()
                    .find_map(|t| real(t).map(str::to_owned));
            }
            _ => {}
        }
    }
    Some(Head {
        id: path.to_string_lossy().into_owned(),
        cwd,
        title: name.or(prompt)?,
    })
}

fn cursor_head(path: &Path, lookups: &mut Lookups) -> Option<Head> {
    let prompt = head_lines(path)
        .filter(|v| v["role"] == "user")
        .find_map(|v| {
            texts(&v["message"]["content"])
                .iter()
                .find_map(|t| cursor_prompt(t).map(str::to_owned))
        })?;
    let chat = path.parent()?;
    let project = chat.parent()?.parent()?.file_name()?.to_owned();
    let cwd = lookups
        .cursor_dirs
        .entry(project)
        .or_insert_with_key(|p| p.to_str().and_then(dashed_dir))
        .clone();
    Some(Head {
        id: chat.file_name()?.to_string_lossy().into_owned(),
        cwd,
        title: prompt,
    })
}

fn kimi_head(path: &Path, roots: &Roots, lookups: &mut Lookups) -> Option<Head> {
    let session = path.parent()?.parent()?.parent()?;
    let id = session.file_name()?.to_str()?.to_owned();
    let state = kimi_state(session);
    // Sessions Kimi imported from the older kimi-cli; it hides them too.
    if state["custom"]["imported_from_kimi_cli"] == true {
        return None;
    }
    let state_cwd = ["cwd", "workDir"]
        .iter()
        .find_map(|k| non_empty(state[*k].as_str()))
        .map(PathBuf::from);
    let (mut cwd, mut prompt) = (state_cwd, None);
    for v in head_lines(path) {
        match v["type"].as_str() {
            Some("config.update") if cwd.is_none() => {
                cwd = non_empty(v["cwd"].as_str()).map(PathBuf::from);
            }
            Some("context.append_message") if prompt.is_none() => {
                prompt = kimi_prompt(&v["message"]).map(str::to_owned);
            }
            _ => {}
        }
        if cwd.is_some() && prompt.is_some() {
            break;
        }
    }
    if cwd.is_none() {
        let dirs = lookups
            .kimi_dirs
            .get_or_insert_with(|| kimi_dirs(&kimi_home(roots)));
        cwd = dirs.get(&id).cloned();
    }
    // A title Kimi wrote or the user gave; otherwise it's the first prompt.
    let named = state["isCustomTitle"] == true
        || matches!(state["titleKind"].as_str(), Some("custom" | "generated"));
    let title = non_empty(state["title"].as_str()).filter(|t| *t != "New Session");
    let title = match (named, title) {
        (true, Some(t)) => t.to_owned(),
        _ => prompt.or(title.map(str::to_owned))?,
    };
    Some(Head { id, cwd, title })
}

/// A Kimi Code session's `state.json` (older ones keep it in `session-meta/`).
fn kimi_state(session: &Path) -> Value {
    ["state.json", "session-meta/state.json"]
        .iter()
        .find_map(|f| serde_json::from_slice(&fs::read(session.join(f)).ok()?).ok())
        .unwrap_or_default()
}

/// Each session's folder from Kimi's `session_index.jsonl`.
fn kimi_dirs(home: &Path) -> HashMap<String, PathBuf> {
    let Ok(file) = File::open(home.join("session_index.jsonl")) else {
        return HashMap::new();
    };
    json_lines(BufReader::new(file))
        .filter_map(|v| {
            let id = v["sessionId"].as_str()?.to_owned();
            Some((id, PathBuf::from(non_empty(v["workDir"].as_str())?)))
        })
        .collect()
}

/// A user message's words, when the user typed it: Kimi also adds user
/// messages of its own (reminders, retries, compaction summaries), which
/// say so in `origin`.
fn kimi_prompt(message: &Value) -> Option<&str> {
    if message["role"] != "user" {
        return None;
    }
    let origin = &message["origin"];
    let typed = match origin["kind"].as_str() {
        None | Some("user") => true,
        Some("skill_activation" | "plugin_command") => origin["trigger"] == "user-slash",
        Some(_) => false,
    };
    if !typed {
        return None;
    }
    message["content"]
        .as_array()?
        .iter()
        .filter(|p| p["type"] == "text")
        .find_map(|p| real(p["text"].as_str()?))
}

fn cursor_prompt(text: &str) -> Option<&str> {
    let text = text.trim();
    match text.strip_prefix("<user_query>") {
        Some(query) => non_empty(Some(query.split("</user_query>").next()?.trim())),
        None => real(text),
    }
}

/// The folder a name like `Users-me-my-project` stands for: Cursor turns
/// every character but letters and digits into `-`, so this looks for
/// folders that read the same.
fn dashed_dir(name: &str) -> Option<PathBuf> {
    fn find(dir: &Path, rest: &str) -> Option<PathBuf> {
        for entry in fs::read_dir(dir).ok()?.flatten() {
            let Some(name) = entry.file_name().to_str().map(dashed) else {
                continue;
            };
            let deeper = match rest.get(..name.len()) {
                Some(head) if head.eq_ignore_ascii_case(&name) => &rest[name.len()..],
                _ => continue,
            };
            if !entry.path().is_dir() {
                continue;
            }
            if deeper.is_empty() {
                return Some(entry.path());
            }
            if let Some(found) = deeper
                .strip_prefix('-')
                .and_then(|d| find(&entry.path(), d))
            {
                return Some(found);
            }
        }
        None
    }
    fn dashed(name: &str) -> String {
        name.chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect()
    }
    find(Path::new("/"), name.trim_start_matches('-'))
}

/// A session's messages, oldest first.
pub fn transcript(session: &Session) -> Vec<Message> {
    let path = &session.path;
    match session.agent {
        "claude" => all_lines(path).flat_map(|v| claude_messages(&v)).collect(),
        "codex" => all_lines(path).flat_map(|v| codex_messages(&v)).collect(),
        "opencode" => path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(|storage| opencode_messages(storage, &session.id))
            .unwrap_or_default(),
        "pi" | "omp" => all_lines(path).flat_map(|v| pi_messages(&v)).collect(),
        "cursor" => all_lines(path).flat_map(|v| cursor_messages(&v)).collect(),
        "kimi" => all_lines(path).flat_map(|v| kimi_messages(&v)).collect(),
        _ => Vec::new(),
    }
}

fn message(role: Role, text: &str, time: Option<&str>) -> Option<Message> {
    let text = text.trim();
    (!text.is_empty()).then(|| Message {
        role,
        text: text.to_owned(),
        time: time.map(str::to_owned),
    })
}

fn tool_call(name: &str, input: &Value, time: Option<&str>) -> Option<Message> {
    let summary = tool_summary(input);
    let name = non_empty(Some(name)).unwrap_or("tool");
    message(Role::Tool, &format!("⏺ {name}  {summary}"), time)
}

fn tool_output(output: &str, time: Option<&str>) -> Option<Message> {
    Some(Message {
        role: Role::Tool,
        text: format!("⎿ {}", output.trim_end()),
        time: time.map(str::to_owned),
    })
}

fn claude_messages(v: &Value) -> Vec<Message> {
    if !claude_shown(v) {
        return Vec::new();
    }
    let time = v["timestamp"].as_str();
    let content = &v["message"]["content"];
    let mut out = Vec::new();
    match v["type"].as_str() {
        Some("user") => {
            if let Some(text) = content.as_str() {
                out.extend(real(text).and_then(|t| message(Role::User, t, time)));
            }
            for block in content.as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => out.extend(
                        block["text"]
                            .as_str()
                            .and_then(real)
                            .and_then(|t| message(Role::User, t, time)),
                    ),
                    Some("tool_result") => {
                        out.extend(tool_output(&output_text(&block["content"]), time));
                    }
                    _ => {}
                }
            }
        }
        Some("assistant") => {
            if let Some(text) = content.as_str() {
                out.extend(message(Role::Assistant, text, time));
            }
            for block in content.as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => out.extend(
                        block["text"]
                            .as_str()
                            .and_then(|t| message(Role::Assistant, t, time)),
                    ),
                    Some("tool_use") => out.extend(tool_call(
                        block["name"].as_str().unwrap_or_default(),
                        &block["input"],
                        time,
                    )),
                    Some("thinking") => out.extend(
                        block["thinking"]
                            .as_str()
                            .and_then(|t| message(Role::Reasoning, t, time)),
                    ),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    out
}

fn codex_messages(v: &Value) -> Vec<Message> {
    if v["type"] != "response_item" {
        return Vec::new();
    }
    let time = v["timestamp"].as_str();
    let p = &v["payload"];
    let name = p["name"].as_str().unwrap_or_default();
    let out = match p["type"].as_str() {
        Some("message") => {
            let texts = texts(&p["content"]);
            return match p["role"].as_str() {
                Some("user") => texts
                    .iter()
                    .filter_map(|t| codex_prompt(t))
                    .filter_map(|t| message(Role::User, &t, time))
                    .collect(),
                Some("assistant") => texts
                    .iter()
                    .filter_map(|t| message(Role::Assistant, t, time))
                    .collect(),
                _ => Vec::new(),
            };
        }
        Some("function_call") => tool_call(name, &p["arguments"], time),
        Some("custom_tool_call") => tool_call(name, &p["input"], time),
        Some("local_shell_call") => tool_call("shell", &p["action"], time),
        Some("function_call_output" | "custom_tool_call_output") => {
            tool_output(&output_text(&p["output"]), time)
        }
        Some("reasoning") => {
            let summary: Vec<&str> = p["summary"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s["text"].as_str())
                .collect();
            message(Role::Reasoning, &summary.join("\n\n"), time)
        }
        _ => None,
    };
    out.into_iter().collect()
}

fn opencode_messages(storage: &Path, session: &str) -> Vec<Message> {
    let mut infos: Vec<Value> = json_files(&storage.join("message").join(session))
        .into_iter()
        .filter(|m| m["role"].is_string())
        .collect();
    infos.sort_by_key(|m| m["time"]["created"].as_i64().unwrap_or_default());
    let mut out = Vec::new();
    for info in infos {
        let role = match info["role"].as_str() {
            Some("user") => Role::User,
            _ => Role::Assistant,
        };
        let time = info["time"]["created"].as_i64().map(iso_from_millis);
        let time = time.as_deref();
        let Some(id) = info["id"].as_str() else {
            continue;
        };
        for part in json_files(&storage.join("part").join(id)) {
            if part["synthetic"] == true || part["ignored"] == true {
                continue;
            }
            match part["type"].as_str() {
                Some("text") => {
                    out.extend(part["text"].as_str().and_then(|t| message(role, t, time)));
                }
                Some("tool") => {
                    let state = &part["state"];
                    let tool = part["tool"].as_str().unwrap_or_default();
                    out.extend(tool_call(tool, &state["input"], time));
                    if let Some(output) = state["output"].as_str().or(state["error"].as_str()) {
                        out.extend(tool_output(output, time));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// The JSON files in a folder, in file-name order (OpenCode's ids sort by time).
fn json_files(dir: &Path) -> Vec<Value> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|p| serde_json::from_slice(&fs::read(p).ok()?).ok())
        .collect()
}

fn pi_messages(v: &Value) -> Vec<Message> {
    if v["type"] != "message" {
        return Vec::new();
    }
    let time = v["timestamp"].as_str();
    let m = &v["message"];
    let content = &m["content"];
    match m["role"].as_str() {
        Some("user") => texts(content)
            .iter()
            .filter_map(|t| real(t).and_then(|t| message(Role::User, t, time)))
            .collect(),
        Some("assistant") => {
            let blocks = content.as_array().map(Vec::as_slice).unwrap_or_default();
            let mut out: Vec<Message> = content
                .as_str()
                .and_then(|t| message(Role::Assistant, t, time))
                .into_iter()
                .collect();
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => out.extend(
                        block["text"]
                            .as_str()
                            .and_then(|t| message(Role::Assistant, t, time)),
                    ),
                    Some("toolCall") => out.extend(tool_call(
                        block["name"].as_str().unwrap_or_default(),
                        &block["arguments"],
                        time,
                    )),
                    _ => {}
                }
            }
            out
        }
        Some("toolResult") => tool_output(&texts(content).join("\n"), time)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn cursor_messages(v: &Value) -> Vec<Message> {
    let content = &v["message"]["content"];
    match v["role"].as_str() {
        Some("user") => texts(content)
            .iter()
            .filter_map(|t| cursor_prompt(t).and_then(|t| message(Role::User, t, None)))
            .collect(),
        Some("assistant") => {
            let mut out: Vec<Message> = content
                .as_str()
                .and_then(|t| message(Role::Assistant, t, None))
                .into_iter()
                .collect();
            for block in content.as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => out.extend(
                        block["text"]
                            .as_str()
                            .and_then(|t| message(Role::Assistant, t, None)),
                    ),
                    Some("tool_use") => out.extend(tool_call(
                        block["name"].as_str().unwrap_or_default(),
                        &block["input"],
                        None,
                    )),
                    _ => {}
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

/// A Kimi Code wire line. Prompts are whole messages; a reply is recorded
/// as it streams, a text part, tool call or tool result at a time (older
/// logs hold whole assistant and tool messages instead).
fn kimi_messages(v: &Value) -> Vec<Message> {
    // Seconds in older logs, milliseconds now.
    let time = v["time"].as_f64().filter(|t| *t > 0.).map(|t| {
        let ms = if t > 1e12 { t } else { t * 1000. };
        iso_from_millis(ms as i64)
    });
    let time = time.as_deref();
    match v["type"].as_str() {
        Some("context.append_message") => {
            let m = &v["message"];
            match m["role"].as_str() {
                Some("user") => kimi_prompt(m)
                    .and_then(|t| message(Role::User, t, time))
                    .into_iter()
                    .collect(),
                Some("assistant") => {
                    let mut out: Vec<Message> = texts(&m["content"])
                        .iter()
                        .filter_map(|t| message(Role::Assistant, t, time))
                        .collect();
                    for call in m["toolCalls"].as_array().into_iter().flatten() {
                        // `{name, arguments}`, or nested under `function`.
                        let f = if call["function"].is_object() {
                            &call["function"]
                        } else {
                            call
                        };
                        out.extend(tool_call(
                            f["name"].as_str().unwrap_or_default(),
                            &f["arguments"],
                            time,
                        ));
                    }
                    out
                }
                Some("tool") => tool_output(&texts(&m["content"]).join("\n"), time)
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            }
        }
        Some("context.append_loop_event") => {
            let e = &v["event"];
            let out = match e["type"].as_str() {
                Some("content.part") if e["part"]["type"] == "text" => e["part"]["text"]
                    .as_str()
                    .and_then(|t| message(Role::Assistant, t, time)),
                Some("tool.call") => {
                    tool_call(e["name"].as_str().unwrap_or_default(), &e["args"], time)
                }
                Some("tool.result") => tool_output(&output_text(&e["result"]["output"]), time),
                _ => None,
            };
            out.into_iter().collect()
        }
        _ => Vec::new(),
    }
}

/// The text of a message's content: a string, or the text of its blocks.
fn texts(content: &Value) -> Vec<String> {
    match content {
        Value::String(s) => vec![s.clone()],
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| {
                matches!(
                    b["type"].as_str(),
                    None | Some("text" | "input_text" | "output_text")
                )
            })
            .filter_map(|b| b["text"].as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// What a tool printed: a string (Codex may wrap it in `{"output": …}`) or
/// text blocks.
fn output_text(output: &Value) -> String {
    match output {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(o)) if o.get("output").is_some_and(Value::is_string) => {
                o["output"].as_str().unwrap_or_default().to_owned()
            }
            _ => s.clone(),
        },
        Value::Object(o) => o.get("output").map(output_text).unwrap_or_default(),
        _ => texts(output).join("\n"),
    }
}

/// The part of a tool's input worth showing: its command, file or pattern.
fn tool_summary(input: &Value) -> String {
    const KEYS: [&str; 12] = [
        "command",
        "cmd",
        "file_path",
        "filePath",
        "path",
        "pattern",
        "glob_pattern",
        "query",
        "url",
        "description",
        "prompt",
        "paths",
    ];
    let text = match input {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v @ Value::Object(_)) => return tool_summary(&v),
            _ => s.lines().next().unwrap_or_default().to_owned(),
        },
        Value::Object(map) => KEYS
            .iter()
            .find_map(|k| map.get(*k).and_then(argument))
            .unwrap_or_default(),
        _ => String::new(),
    };
    clip(&one_line(&text), SUMMARY_LEN)
}

fn argument(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => non_empty(Some(s)).map(str::to_owned),
        Value::Array(items) => {
            let words: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
            match words.as_slice() {
                [shell, flag, script] if shell.ends_with("sh") && matches!(*flag, "-c" | "-lc") => {
                    Some((*script).to_owned())
                }
                [] => None,
                _ => Some(words.join(" ")),
            }
        }
        _ => None,
    }
}

/// The transcript as text to read, hard-wrapped at `width` columns (code and
/// indented lines are left as they are).
pub fn render(session: &Session, messages: &[Message], width: usize) -> String {
    let width = width.max(20);
    let label = super::label_for(session.agent);
    let mut out = format!("{label} · {}\n", session.title);
    match &session.cwd {
        Some(cwd) => out += &format!("{} · {}\n", cwd.display(), session.id),
        None => out += &format!("{}\n", session.id),
    }
    let mut in_reply = false;
    let mut fresh = false;
    for m in messages.iter().filter(|m| m.role != Role::Reasoning) {
        let heading = match m.role {
            Role::User => Some(match m.time.as_deref() {
                Some(t) => format!("You · {}", short_time(t)),
                None => "You".to_owned(),
            }),
            _ if !in_reply => Some(label.clone()),
            _ => None,
        };
        if let Some(heading) = heading {
            out += &format!("\n── {heading} ──\n");
            in_reply = m.role != Role::User;
            fresh = true;
        }
        if let Some(output) = m.text.strip_prefix("⎿ ").filter(|_| m.role == Role::Tool) {
            render_output(output, width, &mut out);
            continue;
        }
        if !fresh {
            out.push('\n');
        }
        fresh = false;
        if m.role == Role::Tool {
            out += &format!(
                "  {}\n",
                clip(m.text.lines().next().unwrap_or_default(), width - 2)
            );
        } else {
            wrap(&m.text, width, &mut out);
        }
    }
    out
}

fn render_output(output: &str, width: usize, out: &mut String) {
    let lines: Vec<&str> = output.trim_matches('\n').lines().collect();
    if lines.iter().all(|l| l.trim().is_empty()) {
        out.push_str("  ⎿ (no output)\n");
        return;
    }
    for (i, line) in lines.iter().take(OUTPUT_LINES).enumerate() {
        let lead = if i == 0 { "  ⎿ " } else { "    " };
        out.push_str(lead);
        out.push_str(&clip(line.trim_end(), width - 4));
        out.push('\n');
    }
    if lines.len() > OUTPUT_LINES {
        out.push_str(&format!(
            "    … {} more lines\n",
            lines.len() - OUTPUT_LINES
        ));
    }
}

fn wrap(text: &str, width: usize, out: &mut String) {
    let mut fenced = false;
    for line in text.lines() {
        let fence = line.trim_start().starts_with("```");
        if fence {
            fenced = !fenced;
        }
        if fence || fenced || line.starts_with([' ', '\t']) || line.chars().count() <= width {
            out.push_str(line.trim_end());
            out.push('\n');
            continue;
        }
        let mut current = String::new();
        let mut used = 0;
        for word in line.split_whitespace() {
            let mut chars: Vec<char> = word.chars().collect();
            if used > 0 && used + 1 + chars.len() > width {
                out.push_str(&current);
                out.push('\n');
                current.clear();
                used = 0;
            }
            while chars.len() > width {
                let rest = chars.split_off(width);
                out.extend(chars);
                out.push('\n');
                chars = rest;
            }
            if used > 0 {
                current.push(' ');
                used += 1;
            }
            used += chars.len();
            current.extend(chars);
        }
        if !current.is_empty() {
            out.push_str(&current);
            out.push('\n');
        }
    }
}

/// `2026-10-28T15:27:03.123Z` as `2026-10-28 15:27`; anything else as it is.
fn short_time(time: &str) -> String {
    match (time.get(..10), time.get(11..16)) {
        (Some(date), Some(clock)) if time.as_bytes()[4] == b'-' => format!("{date} {clock}"),
        _ => time.to_owned(),
    }
}

/// Unix milliseconds as an ISO timestamp, in UTC.
fn iso_from_millis(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, day) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        day / 3600,
        day % 3600 / 60,
        day % 60
    )
}

/// The command that reopens a session, in its folder.
pub fn resume_command(agent: &str, id: &str) -> Option<String> {
    let id = quote(id);
    Some(match agent {
        "claude" => format!("claude --resume {id}"),
        "codex" => format!("codex resume {id}"),
        "opencode" => format!("opencode --session {id}"),
        "cursor" => format!("agent --resume {id}"),
        "kimi" => format!("kimi --session {id}"),
        "pi" | "omp" => format!("{agent} --session {id}"),
        "grok" => format!("grok --resume {id}"),
        "reasonix" => format!("reasonix --resume {id}"),
        "codebuddy" => format!("codebuddy --resume {id}"),
        "antigravity" => format!("agy --conversation {id}"),
        "copilot" => format!("copilot --resume {id}"),
        _ => return None,
    })
}

/// The command that starts a new session from a copy of this one, where the
/// agent has one.
pub fn fork_command(agent: &str, id: &str) -> Option<String> {
    let id = quote(id);
    Some(match agent {
        "claude" => format!("claude --resume {id} --fork-session"),
        "codex" => format!("codex fork {id}"),
        "opencode" => format!("opencode --fork --session {id}"),
        "pi" | "omp" => format!("{agent} --fork {id}"),
        "grok" => format!("grok --resume {id} --fork-session"),
        "reasonix" => format!("reasonix --resume {id} --copy"),
        _ => return None,
    })
}

/// What the agent calls forking.
pub fn fork_label(agent: &str) -> &'static str {
    if agent == "claude" { "Branch" } else { "Fork" }
}

/// `s` as one shell word.
fn quote(s: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-');
    if !s.is_empty() && s.chars().all(plain) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

fn title(text: &str) -> String {
    clip(&one_line(text), TITLE_LEN)
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(max.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|s| !s.trim().is_empty())
}

/// Text a person typed, not a wrapper the agent added (`<command-name>`,
/// `<environment_context>`, …).
fn real(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('<')).then_some(text)
}

fn stem(path: &Path) -> Option<String> {
    Some(path.file_stem()?.to_str()?.to_owned())
}

fn json_lines(reader: impl BufRead) -> impl Iterator<Item = Value> {
    reader
        .split(b'\n')
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_slice(&line).ok())
}

fn head_lines(path: &Path) -> impl Iterator<Item = Value> {
    File::open(path)
        .ok()
        .map(|f| json_lines(BufReader::new(f.take(HEAD))))
        .into_iter()
        .flatten()
}

fn all_lines(path: &Path) -> impl Iterator<Item = Value> {
    File::open(path)
        .ok()
        .map(|f| json_lines(BufReader::new(f)))
        .into_iter()
        .flatten()
}

/// The lines from `from` to the end, skipping the one `from` lands in.
fn tail_lines(path: &Path, from: u64) -> impl Iterator<Item = Value> {
    File::open(path)
        .ok()
        .and_then(|mut f| f.seek(SeekFrom::Start(from)).ok().map(|_| f))
        .map(|f| {
            let mut reader = BufReader::new(f);
            let _ = reader.read_until(b'\n', &mut Vec::new());
            json_lines(reader)
        })
        .into_iter()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_home(name: &str) -> PathBuf {
        let home =
            std::env::temp_dir().join(format!("motty-sessions-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        home
    }

    fn roots(home: &Path) -> Roots {
        Roots {
            home: home.to_owned(),
            claude: vec![home.join(".claude")],
            codex: None,
            kimi: None,
        }
    }

    /// Writes `lines` to `path`, dated `age` seconds ago.
    fn write(path: &Path, lines: &[Value], age: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        fs::write(path, text).unwrap();
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(age))
            .unwrap();
    }

    fn claude_fixture(home: &Path) {
        let project = home.join(".claude/projects/-work-app");
        write(
            &project.join("aaa.jsonl"),
            &[
                serde_json::json!({"type":"queue-operation"}),
                serde_json::json!({"type":"user","isMeta":true,"cwd":"/work/app","message":{"role":"user","content":"meta"}}),
                serde_json::json!({"type":"user","cwd":"/work/app","timestamp":"2026-10-28T15:27:03.123Z","message":{"role":"user","content":"<command-name>/clear</command-name>"}}),
                serde_json::json!({"type":"user","cwd":"/work/app","timestamp":"2026-10-28T15:27:03.123Z","message":{"role":"user","content":"Fix   the\nbuild please"}}),
                serde_json::json!({"type":"assistant","timestamp":"2026-10-28T15:27:09Z","message":{"content":[
                    {"type":"thinking","thinking":"hmm"},
                    {"type":"text","text":"Looking."},
                    {"type":"tool_use","name":"Bash","input":{"command":"ls -la","description":"List"}}]}}),
                serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","content":[{"type":"text","text":"a\nb\nc\nd\ne"}]}]}}),
                serde_json::json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"side"}]}}),
                serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}),
                serde_json::json!({"type":"ai-title","aiTitle":"Fixing the build"}),
            ],
            30,
        );
        write(
            &project.join("bbb.jsonl"),
            &[
                serde_json::json!({"type":"user","cwd":"/work/app","message":{"content":[{"type":"text","text":"Second one"}]}}),
            ],
            10,
        );
        // Only wrappers: an empty session.
        write(
            &project.join("ccc.jsonl"),
            &[
                serde_json::json!({"type":"user","cwd":"/work/app","message":{"content":"<local-command-stdout></local-command-stdout>"}}),
            ],
            5,
        );
        write(
            &project.join("aaa/subagents/agent-1.jsonl"),
            &[serde_json::json!({"type":"user","message":{"content":"sub"}})],
            1,
        );
    }

    #[test]
    fn lists_claude_sessions() {
        let home = temp_home("claude");
        claude_fixture(&home);
        let sessions = list(&roots(&home), 10);
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["bbb", "aaa"]);
        assert_eq!(sessions[1].title, "Fixing the build");
        assert_eq!(sessions[0].title, "Second one");
        assert_eq!(sessions[1].cwd.as_deref(), Some(Path::new("/work/app")));
        assert_eq!(list(&roots(&home), 1).len(), 1);
    }

    #[test]
    fn knows_a_session_by_its_path() {
        let home = temp_home("at");
        claude_fixture(&home);
        let roots = roots(&home);
        let path = home.join(".claude/projects/-work-app/aaa.jsonl");
        let session = at(&path, &roots).unwrap();
        assert_eq!((session.agent, session.id.as_str()), ("claude", "aaa"));
        assert_eq!(session.title, "Fixing the build");
        // A subagent's log, and a .jsonl elsewhere, aren't sessions.
        assert!(
            at(
                &home.join(".claude/projects/-work-app/aaa/subagents/agent-1.jsonl"),
                &roots
            )
            .is_none()
        );
        let elsewhere = home.join("notes/aaa.jsonl");
        fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        fs::write(&elsewhere, "{}\n").unwrap();
        assert!(at(&elsewhere, &roots).is_none());
    }

    #[test]
    fn reads_and_renders_a_claude_transcript() {
        let home = temp_home("claude-render");
        claude_fixture(&home);
        let session = list(&roots(&home), 10).pop().unwrap();
        let messages = transcript(&session);
        let roles: Vec<Role> = messages.iter().map(|m| m.role).collect();
        use Role::*;
        assert_eq!(roles, [User, Reasoning, Assistant, Tool, Tool, Assistant]);
        assert_eq!(messages[0].text, "Fix   the\nbuild please");
        assert_eq!(messages[1].text, "hmm");
        assert_eq!(messages[3].text, "⏺ Bash  ls -la");
        let text = render(&session, &messages, 80);
        let expected = "\
Claude Code · Fixing the build
/work/app · aaa

── You · 2026-10-28 15:27 ──
Fix   the
build please

── Claude Code ──
Looking.

  ⏺ Bash  ls -la
  ⎿ a
    b
    c
    … 2 more lines

Done.
";
        assert_eq!(text, expected);
    }

    #[test]
    fn wraps_long_lines_but_not_code() {
        let mut out = String::new();
        wrap(
            "one two three four five six\n    indented line that is long\n```\ncode line that is long\n```",
            10,
            &mut out,
        );
        assert_eq!(
            out,
            "one two\nthree four\nfive six\n    indented line that is long\n```\ncode line that is long\n```\n"
        );
        let mut out = String::new();
        wrap("abcdefghijklmnopqrstuvwxyz end", 10, &mut out);
        assert_eq!(out, "abcdefghij\nklmnopqrst\nuvwxyz end\n");
    }

    #[test]
    fn lists_and_reads_codex_sessions() {
        let home = temp_home("codex");
        let day = home.join(".codex/sessions/2026/10/28");
        let meta = |id: &str, source: &str| serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":"/work/cx","source":source}});
        let user = |text: &str| serde_json::json!({"type":"response_item","timestamp":"2026-10-28T09:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}});
        write(
            &day.join("rollout-1.jsonl"),
            &[
                meta("019a", "cli"),
                serde_json::json!({"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}),
                user("# AGENTS.md instructions"),
                user("<environment_context>x</environment_context>"),
                user("Add a test"),
                serde_json::json!({"type":"response_item","payload":{"type":"reasoning","summary":[]}}),
                serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"cargo test\"]}"}}),
                serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","output":"{\"output\":\"ok\",\"metadata\":{}}"}}),
                serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch\n*** Update File: a.rs"}}),
                serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call_output","output":[{"type":"input_text","text":"Done!"}]}}),
                serde_json::json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Added."}]}}),
            ],
            20,
        );
        write(
            &day.join("rollout-2.jsonl"),
            &[meta("019b", "exec"), user("automated")],
            1,
        );
        write(
            &home.join(".codex/session_index.jsonl"),
            &[serde_json::json!({"id":"019a","thread_name":"Named thread"})],
            1,
        );
        let sessions = list(&roots(&home), 10);
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!((session.agent, session.id.as_str()), ("codex", "019a"));
        assert_eq!(session.title, "Named thread");
        assert_eq!(session.cwd.as_deref(), Some(Path::new("/work/cx")));
        let texts: Vec<String> = transcript(session).into_iter().map(|m| m.text).collect();
        assert_eq!(
            texts,
            [
                "Add a test",
                "⏺ shell  cargo test",
                "⎿ ok",
                "⏺ apply_patch  *** Begin Patch",
                "⎿ Done!",
                "Added."
            ]
        );
    }

    #[test]
    fn lists_and_reads_opencode_sessions() {
        let home = temp_home("opencode");
        let storage = home.join(".local/share/opencode/storage");
        let json = |path: PathBuf, v: Value| write(&path, &[v], 5);
        json(
            storage.join("session/p1/ses_1.json"),
            serde_json::json!({"id":"ses_1","title":"Theme picker","directory":"/work/oc","time":{"created":1,"updated":2}}),
        );
        json(
            storage.join("session/p1/ses_2.json"),
            serde_json::json!({"id":"ses_2","parentID":"ses_1","title":"child","directory":"/work/oc"}),
        );
        json(
            storage.join("message/ses_1/msg_b.json"),
            serde_json::json!({"id":"msg_b","role":"assistant","time":{"created":1_761_665_220_000_i64}}),
        );
        json(
            storage.join("message/ses_1/msg_a.json"),
            serde_json::json!({"id":"msg_a","role":"user","time":{"created":1_761_665_160_000_i64}}),
        );
        json(
            storage.join("part/msg_a/prt_1.json"),
            serde_json::json!({"type":"text","text":"Pick a theme"}),
        );
        json(
            storage.join("part/msg_b/prt_1.json"),
            serde_json::json!({"type":"reasoning","text":"…"}),
        );
        json(
            storage.join("part/msg_b/prt_2.json"),
            serde_json::json!({"type":"tool","tool":"read","state":{"input":{"filePath":"/work/oc/a.css"},"output":"body {}"}}),
        );
        json(
            storage.join("part/msg_b/prt_3.json"),
            serde_json::json!({"type":"text","text":"Done."}),
        );
        let sessions = list(&roots(&home), 10);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "ses_1");
        assert_eq!(sessions[0].title, "Theme picker");
        let messages = transcript(&sessions[0]);
        let roles: Vec<Role> = messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Tool, Role::Tool, Role::Assistant]);
        assert_eq!(messages[0].time.as_deref(), Some("2025-10-28T15:26:00Z"));
        assert_eq!(messages[1].text, "⏺ read  /work/oc/a.css");
    }

    #[test]
    fn lists_and_reads_pi_sessions() {
        let home = temp_home("pi");
        let path = home.join(".omp/agent/sessions/--work-pi--/2026 run.jsonl");
        write(
            &path,
            &[
                serde_json::json!({"type":"session","id":"x","cwd":"/work/pi"}),
                serde_json::json!({"type":"message","timestamp":"2026-10-28T08:00:00Z","message":{"role":"user","content":[{"type":"text","text":"Hello pi"}]}}),
                serde_json::json!({"type":"message","message":{"role":"assistant","content":[{"type":"thinking","thinking":"…"},{"type":"toolCall","name":"bash","arguments":{"command":"pwd"}},{"type":"text","text":"Hi"}]}}),
                serde_json::json!({"type":"message","message":{"role":"toolResult","content":[{"type":"text","text":"/work/pi"}]}}),
                serde_json::json!({"type":"weird"}),
            ],
            3,
        );
        let sessions = list(&roots(&home), 10);
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.agent, "omp");
        assert_eq!(session.id, path.to_string_lossy());
        assert_eq!(session.title, "Hello pi");
        assert_eq!(session.cwd.as_deref(), Some(Path::new("/work/pi")));
        let roles: Vec<Role> = transcript(session).iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Tool, Role::Assistant, Role::Tool]);
        assert_eq!(
            resume_command("omp", &session.id).unwrap(),
            format!("omp --session '{}'", path.display())
        );
    }

    const KIMI_ID: &str = "session_3f2a9c1e-7b4d-4e8a-9c2f-1d5e6b7a8c90";

    /// A Kimi Code home: one session with a reply and a tool call, a
    /// renamed one, an empty one and one imported from kimi-cli.
    fn kimi_fixture(home: &Path) {
        let sessions = home.join("sessions/wd_app_1a2b3c4d5e6f");
        let state = |dir: &Path, v: Value| write(&dir.join("state.json"), &[v], 0);
        let main = sessions.join(KIMI_ID);
        state(
            &main,
            serde_json::json!({"createdAt":"2026-10-28T09:00:00.000Z","updatedAt":"2026-10-28T09:01:00.000Z","title":"Fix the flaky test","isCustomTitle":false,"lastPrompt":"thanks","agents":{"main":{"type":"main","parentAgentId":null},"agent-0":{"type":"sub","parentAgentId":"main"}},"custom":{}}),
        );
        let event = |e: Value| serde_json::json!({"type":"context.append_loop_event","agentId":"main","event":e,"time":1_793_178_005_000_i64});
        write(
            &main.join("agents/main/wire.jsonl"),
            &[
                serde_json::json!({"type":"metadata","protocol_version":"1.4","created_at":1_793_178_000_000_i64}),
                serde_json::json!({"type":"config.update","cwd":"/work/kimi","profileName":"agent","time":1_793_178_000_100_i64}),
                serde_json::json!({"type":"turn.prompt","input":[{"type":"text","text":"Fix the flaky test"}],"origin":{"kind":"user"},"time":1_793_178_000_200_i64}),
                serde_json::json!({"type":"context.append_message","agentId":"main","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>Plan mode is off</system-reminder>"}],"toolCalls":[],"origin":{"kind":"injection","variant":"plan_mode"}},"time":1_793_178_000_250_i64}),
                serde_json::json!({"type":"context.append_message","agentId":"main","message":{"role":"user","content":[{"type":"text","text":"Fix the flaky test"}],"toolCalls":[],"origin":{"kind":"user"}},"time":1_793_178_000_300_i64}),
                event(serde_json::json!({"type":"step.begin","uuid":"s1","turnId":"t1","step":0})),
                event(
                    serde_json::json!({"type":"content.part","stepUuid":"s1","part":{"type":"think","think":"Look at the test first."}}),
                ),
                event(
                    serde_json::json!({"type":"content.part","stepUuid":"s1","part":{"type":"text","text":"Running it."}}),
                ),
                event(
                    serde_json::json!({"type":"tool.call","stepUuid":"s1","toolCallId":"call_1","name":"Shell","args":{"command":"cargo test flaky"}}),
                ),
                event(
                    serde_json::json!({"type":"step.end","uuid":"s1","finishReason":"tool_calls"}),
                ),
                event(
                    serde_json::json!({"type":"tool.result","toolCallId":"call_1","result":{"output":[{"type":"text","text":"test flaky ... ok"}],"isError":false}}),
                ),
                serde_json::json!({"type":"usage.record","model":"kimi-k2","usage":{"inputOther":10,"output":5},"time":1_793_178_006_000_i64}),
                event(serde_json::json!({"type":"step.begin","uuid":"s2","turnId":"t1","step":1})),
                event(
                    serde_json::json!({"type":"content.part","stepUuid":"s2","part":{"type":"text","text":"It passes now."}}),
                ),
                serde_json::json!("not a record"),
                serde_json::json!({"type":"context.append_message","agentId":"main","message":{"role":"user","content":[{"type":"text","text":"thanks"}],"toolCalls":[]},"time":1_793_178_060.5}),
            ],
            30,
        );
        write(
            &main.join("agents/agent-0/wire.jsonl"),
            &[
                serde_json::json!({"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"subagent task"}],"toolCalls":[]}}),
            ],
            1,
        );
        // Renamed, with its folder only in Kimi's index, and an older
        // log's whole messages.
        let renamed = sessions.join("session_renamed");
        state(
            &renamed,
            serde_json::json!({"title":"Release notes","titleKind":"custom"}),
        );
        write(
            &renamed.join("agents/main/wire.jsonl"),
            &[
                serde_json::json!({"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"Write the notes"}],"toolCalls":[]},"time":1_793_178_000_000_i64}),
                serde_json::json!({"type":"context.append_message","message":{"role":"assistant","content":[{"type":"text","text":"Reading the log."}],"toolCalls":[{"type":"function","id":"c1","function":{"name":"ReadFile","arguments":"{\"path\":\"CHANGELOG.md\"}"}}]}}),
                serde_json::json!({"type":"context.append_message","message":{"role":"tool","toolCallId":"c1","content":[{"type":"text","text":"## 1.0"}]}}),
            ],
            20,
        );
        write(
            &home.join("session_index.jsonl"),
            &[
                serde_json::json!({"sessionId":"session_renamed","sessionDir":renamed,"workDir":"/work/notes"}),
            ],
            0,
        );
        // Nothing asked yet.
        let empty = sessions.join("session_empty");
        state(&empty, serde_json::json!({"title":"New Session"}));
        write(
            &empty.join("agents/main/wire.jsonl"),
            &[
                serde_json::json!({"type":"metadata","protocol_version":"1.4","created_at":1}),
                serde_json::json!({"type":"config.update","cwd":"/work/kimi"}),
            ],
            5,
        );
        let imported = sessions.join("session_imported");
        state(
            &imported,
            serde_json::json!({"title":"old","custom":{"imported_from_kimi_cli":true}}),
        );
        write(
            &imported.join("agents/main/wire.jsonl"),
            &[
                serde_json::json!({"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"old"}]}}),
            ],
            2,
        );
    }

    #[test]
    fn lists_and_reads_kimi_sessions() {
        let home = temp_home("kimi");
        // `$KIMI_CODE_HOME` somewhere other than `~/.kimi-code`.
        let kimi = home.join("kimi-home");
        kimi_fixture(&kimi);
        assert!(list(&roots(&home), 10).is_empty());
        let roots = Roots {
            kimi: Some(kimi),
            ..roots(&home)
        };
        let sessions = list(&roots, 10);
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["session_renamed", KIMI_ID]);
        assert!(sessions.iter().all(|s| s.agent == "kimi"));
        let (renamed, session) = (&sessions[0], &sessions[1]);
        assert_eq!(renamed.title, "Release notes");
        assert_eq!(renamed.cwd.as_deref(), Some(Path::new("/work/notes")));
        assert_eq!(session.title, "Fix the flaky test");
        assert_eq!(session.cwd.as_deref(), Some(Path::new("/work/kimi")));
        assert_eq!(
            resume_command("kimi", &session.id).unwrap(),
            format!("kimi --session {KIMI_ID}")
        );
        assert_eq!(fork_command("kimi", &session.id), None);

        let messages = transcript(session);
        let texts: Vec<&str> = messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Fix the flaky test",
                "Running it.",
                "⏺ Shell  cargo test flaky",
                "⎿ test flaky ... ok",
                "It passes now.",
                "thanks"
            ]
        );
        let roles: Vec<Role> = messages.iter().map(|m| m.role).collect();
        use Role::*;
        assert_eq!(roles, [User, Assistant, Tool, Tool, Assistant, User]);
        assert_eq!(messages[0].time.as_deref(), Some("2026-10-28T09:00:00Z"));
        // Seconds, as older logs have them.
        assert_eq!(messages[5].time.as_deref(), Some("2026-10-28T09:01:00Z"));

        let texts: Vec<String> = transcript(renamed).into_iter().map(|m| m.text).collect();
        assert_eq!(
            texts,
            [
                "Write the notes",
                "Reading the log.",
                "⏺ ReadFile  CHANGELOG.md",
                "⎿ ## 1.0"
            ]
        );
    }

    #[test]
    fn quotes_ids_in_commands() {
        assert_eq!(
            resume_command("claude", "abc-123").as_deref(),
            Some("claude --resume abc-123")
        );
        assert_eq!(
            fork_command("claude", "abc").as_deref(),
            Some("claude --resume abc --fork-session")
        );
        assert_eq!(
            fork_command("pi", "/tmp/it's here.jsonl").as_deref(),
            Some(r"pi --fork '/tmp/it'\''s here.jsonl'")
        );
        assert_eq!(
            resume_command("antigravity", "c1").as_deref(),
            Some("agy --conversation c1")
        );
        assert_eq!(
            resume_command("cursor", "c1").as_deref(),
            Some("agent --resume c1")
        );
        assert_eq!(fork_command("cursor", "x"), None);
        assert_eq!(resume_command("nope", "x"), None);
        assert_eq!(fork_label("claude"), "Branch");
        assert_eq!(fork_label("codex"), "Fork");
    }

    #[test]
    fn titles_are_one_short_line() {
        let long = "word ".repeat(40);
        let t = title(&long);
        assert_eq!(t.chars().count(), TITLE_LEN);
        assert!(t.ends_with('…'));
        assert_eq!(title("a\n  b"), "a b");
    }

    #[test]
    fn remembers_claude_roots_once() {
        let home = temp_home("roots");
        let file = home.join("Motty/agent-roots.json");
        let transcript = home.join("cfg/projects/-p/abc.jsonl");
        remember_in(&file, &transcript);
        remember_in(&file, &transcript);
        remember_in(&file, &home.join("elsewhere/abc.jsonl"));
        assert_eq!(read_roots(&file), [home.join("cfg")]);
    }

    #[test]
    fn converts_millis_to_iso() {
        assert_eq!(iso_from_millis(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_millis(1_709_164_800_000), "2024-02-29T00:00:00Z");
        assert_eq!(short_time("2026-10-28T15:27:03Z"), "2026-10-28 15:27");
        assert_eq!(short_time("soon"), "soon");
    }

    /// Prints what's on this machine; run with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_stores() {
        let started = std::time::Instant::now();
        let sessions = list(&Roots::current(), 5000);
        println!("listed {} in {:?}", sessions.len(), started.elapsed());
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for s in &sessions {
            *counts.entry(s.agent).or_default() += 1;
        }
        println!("{counts:?}");
        let started = std::time::Instant::now();
        let newest = list(&Roots::current(), 100);
        println!("listed {} in {:?}", newest.len(), started.elapsed());
        let roles = |s: &Session| {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for m in transcript(s) {
                *counts.entry(format!("{:?}", m.role)).or_default() += 1;
            }
            counts
        };
        if let Some(s) = newest.first() {
            let started = std::time::Instant::now();
            let text = render(s, &transcript(s), 100);
            let widest = text.lines().map(|l| l.chars().count()).max();
            println!(
                "rendered {} lines, widest {widest:?}, in {:?}; roles {:?}",
                text.lines().count(),
                started.elapsed(),
                roles(s)
            );
        }
        for agent in ["claude", "codex", "opencode", "pi", "omp", "cursor", "kimi"] {
            for s in sessions.iter().filter(|s| s.agent == agent).take(3) {
                let messages = transcript(s);
                println!(
                    "{agent}: {} | cwd {} | {} messages",
                    clip(&s.title, 30),
                    s.cwd.is_some(),
                    messages.len()
                );
            }
        }
    }
}
