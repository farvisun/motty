//! Plan quota: how much of the plan's allowance is left, for the agents
//! that can say (Claude Code, Codex, omp). Each is asked through its own
//! CLI, the command you'd type yourself, with the login it already has:
//! `claude -p "/usage"`, `codex app-server`, `omp usage --json`.
//!
//! These are the only readings that cost anything, so they run in the
//! background, only while that agent is running in a pane, every few
//! minutes and again when a task finishes. A CLI that isn't there or
//! keeps failing is left alone after a few tries.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext, Global, Task};
use motty_theme::Config;
use serde_json::{Value, json};

/// The agents that report a quota.
pub const AGENTS: [&str; 3] = ["claude", "codex", "omp"];
/// How often a running agent's quota is read again.
const INTERVAL: Duration = Duration::from_secs(5 * 60);
/// After a task finishes, how long to wait for others finishing with it,
/// so they cost one reading.
const SETTLE: Duration = Duration::from_secs(3);
/// Failed readings in a row before giving up (until clicked).
const MAX_FAILURES: u32 = 3;
/// How long one reading may take.
const TIMEOUT: Duration = Duration::from_secs(45);

/// Whether `agent` can say how much of its plan is left.
pub fn reports(agent: &str) -> bool {
    AGENTS.contains(&agent)
}

/// What Show Plan Quota runs, for its description.
pub fn source(agent: &str) -> &'static str {
    match agent {
        "claude" => "Runs claude -p \"/usage\" every few minutes while Claude Code runs",
        "codex" => "Asks codex app-server every few minutes while Codex runs",
        "omp" => "Runs omp usage --json every few minutes while omp runs",
        _ => "",
    }
}

/// Turns Show Plan Quota on or off for `agent`, in config.toml.
pub fn show(agent: &str, on: bool, config: &Config) -> std::io::Result<PathBuf> {
    let mut off: Vec<String> = config
        .agent_quota_off
        .iter()
        .filter(|a| *a != agent)
        .cloned()
        .collect();
    if !on {
        off.push(agent.to_owned());
    }
    motty_theme::config::save_quota_off(&off)
}

/// One allowance: how much of it is used, and when it starts over.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub label: String,
    /// Percent used, 0–100.
    pub used: f32,
    pub resets: Option<Resets>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Resets {
    At(SystemTime),
    /// As the CLI wrote it (`3pm`, `Nov 4, 10am`).
    Text(String),
}

impl Resets {
    /// `Resets in 2h 5m`, `Resets Tue 14:00`, `Resets 3pm`.
    pub fn describe(&self, now: SystemTime) -> String {
        match self {
            Resets::Text(text) => format!("Resets {text}"),
            Resets::At(at) => {
                let left = at.duration_since(now).unwrap_or_default().as_secs();
                match left {
                    0..60 => "Resets in under a minute".to_owned(),
                    60..3_600 => format!("Resets in {}m", left / 60),
                    3_600..86_400 => format!("Resets in {}h {}m", left / 3_600, left % 3_600 / 60),
                    _ => format!("Resets {}", day_time(*at)),
                }
            }
        }
    }
}

/// `Tue 14:00`, local time.
fn day_time(at: SystemTime) -> String {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as libc::time_t);
    // SAFETY: localtime_r writes into the struct we pass.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    };
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    format!(
        "{} {:02}:{:02}",
        DAYS[tm.tm_wday.clamp(0, 6) as usize],
        tm.tm_hour,
        tm.tm_min
    )
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Quota {
    pub bars: Vec<Bar>,
    /// A line under the bars (`As observed by omp`).
    pub note: Option<String>,
}

// MARK: - Reading

/// Reads `agent`'s quota with its CLI, or says why it couldn't.
fn read(agent: &str) -> Result<Quota, String> {
    let command = super::by_id(agent).map_or(agent, |a| a.command);
    let program = super::locate(command).ok_or_else(|| format!("{command} isn't installed"))?;
    match agent {
        "claude" => claude(&program, TIMEOUT),
        "codex" => codex(&program, TIMEOUT),
        "omp" => omp(&program, TIMEOUT),
        _ => Err(format!("{agent} reports no quota")),
    }
}

fn claude(program: &Path, timeout: Duration) -> Result<Quota, String> {
    // The cheapest model at low effort, and not saved as a session, so
    // the probe stays out of history and costs next to nothing. Effort
    // goes in the environment: a build without `--effort` would refuse
    // the flag.
    let mut cmd = command(program);
    cmd.args([
        "-p",
        "/usage",
        "--model",
        "haiku",
        "--no-session-persistence",
    ])
    .env("CLAUDE_CODE_EFFORT_LEVEL", "low");
    let out = run(cmd, timeout)?;
    parse_claude(&out).ok_or_else(|| "claude /usage printed no quota".to_owned())
}

fn omp(program: &Path, timeout: Duration) -> Result<Quota, String> {
    let mut cmd = command(program);
    cmd.args(["usage", "--json"]);
    let out = run(cmd, timeout)?;
    let v: Value = serde_json::from_str(out.trim()).map_err(|e| format!("omp usage: {e}"))?;
    parse_omp(&v).ok_or_else(|| "omp usage reported no limits".to_owned())
}

/// Asks `codex app-server` over JSON-RPC on its stdin and stdout: the
/// handshake, then the account's rate limits.
fn codex(program: &Path, timeout: Duration) -> Result<Quota, String> {
    let mut child = command(program)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't run codex: {e}"))?;
    let lines = line_reader(&mut child);
    let deadline = Instant::now() + timeout;
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let mut send = |v: Value| {
            writeln!(stdin, "{v}")
                .and_then(|_| stdin.flush())
                .map_err(|e| format!("codex app-server: {e}"))
        };
        send(json!({
            "method": "initialize",
            "id": 0,
            "params": {"clientInfo": {
                "name": "motty",
                "title": "Motty",
                "version": env!("CARGO_PKG_VERSION"),
            }},
        }))?;
        answer(&lines, 0, deadline)?;
        send(json!({"method": "initialized"}))?;
        send(json!({"method": "account/rateLimits/read", "id": 1}))?;
        answer(&lines, 1, deadline)
    })();
    stop(child);
    parse_codex(&result?).ok_or_else(|| "codex reported no rate limits".to_owned())
}

/// The result of request `id`, skipping notifications and other replies.
fn answer(lines: &mpsc::Receiver<String>, id: u64, deadline: Instant) -> Result<Value, String> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = lines
            .recv_timeout(left)
            .map_err(|_| "codex app-server didn't answer".to_owned())?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v["id"].as_u64() != Some(id) {
            continue;
        }
        if let Some(error) = v.get("error") {
            let message = error["message"].as_str().unwrap_or("an error");
            return Err(format!("codex app-server: {message}"));
        }
        return Ok(v["result"].clone());
    }
}

/// The agent's program, started where an app from the Dock can run it:
/// with its own folder on `PATH` (a Node script needs `node` beside it)
/// and outside any pane.
fn command(program: &Path) -> Command {
    let mut path: Vec<PathBuf> = program.parent().into_iter().map(Path::to_owned).collect();
    path.extend(
        std::env::var_os("PATH").map_or_else(Vec::new, |p| std::env::split_paths(&p).collect()),
    );
    path.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    let mut cmd = Command::new(program);
    if let Ok(path) = std::env::join_paths(path) {
        cmd.env("PATH", path);
    }
    cmd.env_remove("MOTTY_PANE_ID");
    if let Some(home) = dirs::home_dir() {
        cmd.current_dir(home);
    }
    cmd
}

/// Runs `cmd` to the end and returns what it printed, giving up after
/// `timeout`.
fn run(mut cmd: Command, timeout: Duration) -> Result<String, String> {
    let name = Path::new(cmd.get_program())
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't run {name}: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                stop(child);
                return Err(format!("{name} took too long"));
            }
        }
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() && out.trim().is_empty() {
        return Err(format!("{name} failed ({status})"));
    }
    Ok(out)
}

/// The child's stdout, a line at a time, read on a thread of its own.
fn line_reader(child: &mut Child) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    rx
}

fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

// MARK: - Parsing

/// Claude Code's `/usage`. In print mode a line per limit:
///
/// ```text
/// Current session: 12% used · resets 3pm (Europe/London)
/// Current week (all models): 4% used · resets Oct 22 at 1pm (Europe/London)
/// ```
///
/// (older builds put `, resets`), and on screen the same over three
/// lines: the name, a bar ending `12% used`, then `Resets 3pm (…)`. The
/// session and the all-models week are what Otty shows.
pub fn parse_claude(text: &str) -> Option<Quota> {
    let mut bars: Vec<Bar> = Vec::new();
    // The limit the lines below belong to, until the next one.
    let mut current: Option<usize> = None;
    for line in text.lines().map(str::trim) {
        // ASCII only, so its offsets are the line's.
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("current ") {
            current = None;
            let label = if lower.starts_with("current session") {
                "Session quota"
            } else if lower.starts_with("current week (all models)")
                || lower.starts_with("current week:")
                || lower == "current week"
            {
                "Weekly quota"
            } else {
                continue;
            };
            if bars.iter().any(|b| b.label == label) {
                continue;
            }
            bars.push(Bar {
                label: label.to_owned(),
                used: f32::NAN,
                resets: None,
            });
            current = Some(bars.len() - 1);
        }
        let Some(bar) = current.and_then(|i| bars.get_mut(i)) else {
            continue;
        };
        if let Some(used) = percent(&lower) {
            bar.used = used;
        }
        if let Some(at) = lower.find("resets ") {
            let text = line[at + "resets ".len()..].trim_end_matches(')');
            bar.resets = Some(Resets::Text(without_zone(text).to_owned()));
        }
    }
    bars.retain(|b| !b.used.is_nan());
    (!bars.is_empty()).then_some(Quota { bars, note: None })
}

/// `12% used` (or `88% left`) in a line, as percent used.
fn percent(line: &str) -> Option<f32> {
    let at = line.find('%')?;
    let digits = line[..at]
        .rsplit(|c: char| !(c.is_ascii_digit() || c == '.'))
        .next()?;
    let n: f32 = digits.parse().ok()?;
    let rest = line[at + 1..].trim_start();
    if rest.starts_with("used") {
        Some(n)
    } else if rest.starts_with("left") || rest.starts_with("remaining") {
        Some(100. - n)
    } else {
        None
    }
}

/// `3pm (Europe/London)` without the zone, which is the Mac's own.
fn without_zone(text: &str) -> &str {
    match text.rfind(" (") {
        Some(i) if text[i..].contains('/') => text[..i].trim(),
        _ => text.trim(),
    }
}

/// A window's length as a bar's name: `5-hour`, `Weekly`.
fn window_label(minutes: u64) -> String {
    match minutes {
        10_080 => "Weekly".to_owned(),
        m if (28 * 1_440..=31 * 1_440).contains(&m) => "Monthly".to_owned(),
        m if m % 1_440 == 0 => format!("{}-day", m / 1_440),
        m if m % 60 == 0 => format!("{}-hour", m / 60),
        m => format!("{m}-minute"),
    }
}

/// The result of Codex's `account/rateLimits/read`: the plan's windows
/// (`codex`), then each model's own allowance (`limitName`), a bar per
/// window.
pub fn parse_codex(result: &Value) -> Option<Quota> {
    let mut limits: Vec<&Value> = match result["rateLimitsByLimitId"].as_object() {
        Some(by_id) if !by_id.is_empty() => by_id.values().collect(),
        _ => vec![&result["rateLimits"]],
    };
    // The plan's own first.
    limits.sort_by_key(|l| l["limitId"].as_str() != Some("codex"));
    let mut bars = Vec::new();
    for limit in limits {
        let model = limit["limitName"]
            .as_str()
            .filter(|_| limit["limitId"].as_str() != Some("codex"));
        for key in ["primary", "secondary"] {
            let w = &limit[key];
            let Some(used) = w["usedPercent"].as_f64() else {
                continue;
            };
            let window = w["windowDurationMins"].as_u64().map(window_label);
            let label = match (model, window) {
                (Some(m), Some(w)) => format!("{m} {}", w.to_lowercase()),
                (Some(m), None) => m.to_owned(),
                (None, Some(w)) => w,
                (None, None) => "Plan".to_owned(),
            };
            let resets = w["resetsAt"]
                .as_u64()
                .map(|s| Resets::At(UNIX_EPOCH + Duration::from_secs(s)));
            bars.push(Bar {
                label,
                used: used as f32,
                resets,
            });
        }
    }
    (!bars.is_empty()).then_some(Quota { bars, note: None })
}

/// `omp usage --json`: a report per signed-in account, each with its
/// limits. Like omp's own capacity line, the accounts are pooled: per
/// window, each account counts with its most used limit, and the bar is
/// how much of all of them is used.
pub fn parse_omp(v: &Value) -> Option<Quota> {
    struct Pool {
        provider: String,
        label: String,
        order: u64,
        fractions: Vec<f64>,
        resets: Option<u64>,
    }
    let mut pools: Vec<Pool> = Vec::new();
    for report in v["reports"].as_array()? {
        let provider = report["provider"].as_str().unwrap_or_default();
        // This account's most used limit per window.
        let mut account: Vec<(String, f64, Option<u64>, u64)> = Vec::new();
        for limit in report["limits"].as_array().into_iter().flatten() {
            let Some(fraction) = used_fraction(&limit["amount"]) else {
                continue;
            };
            let (label, order) = omp_window(limit);
            let tier = limit["scope"]["tier"].as_str().filter(|t| !t.is_empty());
            let label = match tier {
                Some(t) => {
                    let mut chars = t.chars();
                    let first: String = chars
                        .next()
                        .into_iter()
                        .flat_map(char::to_uppercase)
                        .collect();
                    format!("{label} ({first}{})", chars.as_str())
                }
                None => label,
            };
            let resets = limit["window"]["resetsAt"].as_u64();
            match account.iter_mut().find(|(l, ..)| *l == label) {
                Some(seen) if fraction > seen.1 => *seen = (label, fraction, resets, order),
                Some(_) => {}
                None => account.push((label, fraction, resets, order)),
            }
        }
        for (label, fraction, resets, order) in account {
            let pool = match pools
                .iter_mut()
                .position(|p| p.provider == provider && p.label == label)
            {
                Some(i) => &mut pools[i],
                None => {
                    pools.push(Pool {
                        provider: provider.to_owned(),
                        label,
                        order,
                        fractions: Vec::new(),
                        resets: None,
                    });
                    pools.last_mut()?
                }
            };
            pool.fractions.push(fraction);
            pool.resets = match (pool.resets, resets) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
    }
    let mut providers: Vec<String> = Vec::new();
    for p in &pools {
        if !providers.contains(&p.provider) {
            providers.push(p.provider.clone());
        }
    }
    let many = providers.len() > 1;
    let rank = |p: &Pool| providers.iter().position(|x| *x == p.provider);
    pools.sort_by_key(|p| (rank(p), p.order));
    let bars: Vec<Bar> = pools
        .iter()
        .map(|p| Bar {
            label: if many {
                let mut window = p.label.clone();
                if let Some(first) = window.get_mut(..1) {
                    first.make_ascii_lowercase();
                }
                format!("{} {window}", provider_name(&p.provider))
            } else {
                p.label.clone()
            },
            used: (p.fractions.iter().sum::<f64>() / p.fractions.len() as f64 * 100.) as f32,
            resets: p
                .resets
                .map(|ms| Resets::At(UNIX_EPOCH + Duration::from_millis(ms))),
        })
        .collect();
    (!bars.is_empty()).then(|| Quota {
        bars,
        note: Some("As observed by omp".to_owned()),
    })
}

/// A limit's share used, however omp put it.
fn used_fraction(amount: &Value) -> Option<f64> {
    if let Some(f) = amount["usedFraction"].as_f64() {
        return Some(f);
    }
    if let (Some(used), Some(limit)) = (amount["used"].as_f64(), amount["limit"].as_f64())
        && limit > 0.
    {
        return Some(used / limit);
    }
    if amount["unit"] == "percent"
        && let Some(used) = amount["used"].as_f64()
    {
        return Some(used / 100.);
    }
    amount["remainingFraction"]
        .as_f64()
        .map(|r| (1. - r).max(0.))
}

/// An omp limit's window as a bar's name, and where it sorts.
fn omp_window(limit: &Value) -> (String, u64) {
    if let Some(ms) = limit["window"]["durationMs"].as_u64() {
        return (window_label(ms / 60_000), ms);
    }
    // Claude's Extra Usage is a monthly spend cap.
    if limit["scope"]["windowId"] == "extra" {
        return ("Monthly".to_owned(), 31 * 86_400_000);
    }
    let label = limit["window"]["label"]
        .as_str()
        .or(limit["label"].as_str())
        .unwrap_or("Limit");
    (label.to_owned(), u64::MAX)
}

fn provider_name(provider: &str) -> &str {
    match provider {
        "anthropic" => "Claude",
        "openai-codex" => "Codex",
        other => other,
    }
}

// MARK: - Schedule

/// Every agent's last reading, shared by all windows.
#[derive(Default)]
struct Board {
    agents: HashMap<&'static str, Reading>,
}

impl Global for Board {}

#[derive(Default)]
struct Reading {
    quota: Option<Quota>,
    /// When the last attempt ended.
    at: Option<Instant>,
    /// Failed attempts in a row.
    failures: u32,
    /// A task finished: read again once this passes.
    due: Option<Instant>,
    running: Option<Task<()>>,
}

/// What the panel shows for an agent's quota.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub quota: Option<Quota>,
    pub reading: bool,
    /// It gave up after failing.
    pub failed: bool,
}

/// The quota to show for `agent`, unless it reports none or it's off.
pub fn shown(agent: &str, config: &Config, cx: &App) -> Option<Shown> {
    if !reports(agent) || !config.shows_quota(agent) {
        return None;
    }
    let r = cx.try_global::<Board>()?.agents.get(agent);
    Some(r.map_or_else(Shown::default, |r| Shown {
        quota: r.quota.clone(),
        reading: r.running.is_some(),
        failed: r.failures >= MAX_FAILURES,
    }))
}

/// Starts the readings that are due, for the agents in `running`.
pub fn poll(running: &[String], config: &Config, cx: &mut App) {
    let now = Instant::now();
    for agent in AGENTS {
        if !running.iter().any(|a| a == agent) || !config.shows_quota(agent) {
            continue;
        }
        let r = cx
            .default_global::<Board>()
            .agents
            .entry(agent)
            .or_default();
        let stale = r.at.is_none_or(|t| t.elapsed() >= INTERVAL);
        let asked = r.due.is_some_and(|d| now >= d);
        if r.running.is_none() && r.failures < MAX_FAILURES && (stale || asked) {
            start(agent, cx);
        }
    }
}

/// A task of `agent`'s finished: read again shortly, once for all the
/// tasks that finish together.
pub fn task_finished(agent: &str, cx: &mut App) {
    let Some(agent) = AGENTS.into_iter().find(|a| *a == agent) else {
        return;
    };
    let r = cx
        .default_global::<Board>()
        .agents
        .entry(agent)
        .or_default();
    r.due.get_or_insert(Instant::now() + SETTLE);
}

/// Reads `agent`'s quota now (a click on it), trying again after failures.
pub fn refresh(agent: &str, cx: &mut App) {
    let Some(agent) = AGENTS.into_iter().find(|a| *a == agent) else {
        return;
    };
    let r = cx
        .default_global::<Board>()
        .agents
        .entry(agent)
        .or_default();
    r.failures = 0;
    if r.running.is_none() {
        start(agent, cx);
    }
    cx.refresh_windows();
}

fn start(agent: &'static str, cx: &mut App) {
    let work = cx.background_spawn(async move { read(agent) });
    let task = cx.spawn(async move |cx| {
        let result = work.await;
        cx.update(|cx| {
            let r = cx
                .default_global::<Board>()
                .agents
                .entry(agent)
                .or_default();
            r.running = None;
            r.at = Some(Instant::now());
            match result {
                Ok(quota) => {
                    r.quota = Some(quota);
                    r.failures = 0;
                }
                Err(e) => {
                    r.failures += 1;
                    tracing::info!("plan quota for {agent}: {e}");
                }
            }
            cx.refresh_windows();
        });
    });
    let r = cx
        .default_global::<Board>()
        .agents
        .entry(agent)
        .or_default();
    r.due = None;
    r.running = Some(task);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars(q: &Quota) -> Vec<(&str, f32)> {
        q.bars.iter().map(|b| (b.label.as_str(), b.used)).collect()
    }

    fn at(secs: u64) -> Option<Resets> {
        Some(Resets::At(UNIX_EPOCH + Duration::from_secs(secs)))
    }

    /// `claude -p "/usage"` with a subscription (v2.1.300).
    const CLAUDE_PRINT: &str =
        "You are currently using your subscription to power your Claude Code usage

Current session: 37% used · resets Oct 15 at 11:10pm (Asia/Tokyo)
Current week (all models): 4% used · resets Oct 22 at 1pm (Asia/Tokyo)
Current week (Fable): 0% used · resets Oct 22 at 1pm (Asia/Tokyo)
";

    #[test]
    fn parses_claude_print_mode() {
        let q = parse_claude(CLAUDE_PRINT).unwrap();
        assert_eq!(bars(&q), [("Session quota", 37.), ("Weekly quota", 4.)]);
        assert_eq!(
            q.bars[0].resets,
            Some(Resets::Text("Oct 15 at 11:10pm".into()))
        );
        assert_eq!(q.note, None);
        // An August build: a comma, and the session's cost first.
        let older = "Total cost:            $0.0000\nTotal duration (API):  0s\n\nCurrent session: 2% used, resets 3pm (Europe/London)\nCurrent week (all models): 61% used, resets Nov 4, 10am (Europe/London)\n";
        let q = parse_claude(older).unwrap();
        assert_eq!(bars(&q), [("Session quota", 2.), ("Weekly quota", 61.)]);
        assert_eq!(q.bars[1].resets, Some(Resets::Text("Nov 4, 10am".into())));
    }

    #[test]
    fn parses_claude_screen_layout() {
        let screen = "  Current session\n  █████████████████▌                                 35% used\n  Resets 2am (Europe/London)\n\n  Current week (all models)\n  ██████                                             12% used\n  Resets Nov 4, 10am (Europe/London)\n\n  Current week (Opus)\n  ▌                                                  1% used\n";
        let q = parse_claude(screen).unwrap();
        assert_eq!(bars(&q), [("Session quota", 35.), ("Weekly quota", 12.)]);
        assert_eq!(q.bars[0].resets, Some(Resets::Text("2am".into())));
        let old = "Current session\n1% used  (Resets 5am (Europe/Vienna))\n";
        let q = parse_claude(old).unwrap();
        assert_eq!(q.bars[0].resets, Some(Resets::Text("5am".into())));
    }

    #[test]
    fn claude_without_a_plan_has_no_quota() {
        // An API key: only the session's cost.
        let api = "Total cost:            $0.1234\nTotal duration (API):  12.3s\nUsage by model:\n    claude-haiku:  1.2k input, 300 output\n";
        assert_eq!(parse_claude(api), None);
        assert_eq!(parse_claude(""), None);
    }

    /// `account/rateLimits/read` from openai/codex's app-server protocol.
    fn codex_result() -> Value {
        let window = |used, mins, resets| json!({"usedPercent": used, "windowDurationMins": mins, "resetsAt": resets});
        let limit = |id: &str, name: Value, primary: Value, secondary: Value| {
            json!({"limitId": id, "limitName": name, "normalModelSlug": null,
                "primary": primary, "secondary": secondary, "credits": null,
                "individualLimit": null, "spendControlReached": null,
                "planType": "plus", "rateLimitReachedType": null})
        };
        let plan = limit(
            "codex",
            Value::Null,
            window(42, 300, 1_761_760_800u64),
            window(18, 10_080, 1_762_128_000u64),
        );
        json!({
            "ordinaryUsageAllowed": true,
            "rateLimits": plan,
            "rateLimitsByLimitId": {
                "codex_bengalfox": limit("codex_bengalfox", json!("GPT-5.3-Codex-Spark"),
                    window(0, 300, 1_761_760_800u64), Value::Null),
                "codex": plan,
            },
            "rateLimitResetCredits": {"availableCount": 0, "credits": null},
            "accountId": "acc-1",
            "rateLimitUpsell": null,
        })
    }

    #[test]
    fn parses_codex_rate_limits() {
        let q = parse_codex(&codex_result()).unwrap();
        assert_eq!(
            bars(&q),
            [
                ("5-hour", 42.),
                ("Weekly", 18.),
                ("GPT-5.3-Codex-Spark 5-hour", 0.)
            ]
        );
        assert_eq!(q.bars[0].resets, at(1_761_760_800));
        // Only the plan's own, weekly only.
        let only = json!({"rateLimits": {"limitId": "codex", "limitName": null,
            "primary": {"usedPercent": 51, "windowDurationMins": 10080, "resetsAt": null},
            "secondary": null}, "rateLimitsByLimitId": null});
        let q = parse_codex(&only).unwrap();
        assert_eq!(bars(&q), [("Weekly", 51.)]);
        assert_eq!(q.bars[0].resets, None);
        assert_eq!(parse_codex(&json!({"rateLimits": null})), None);
    }

    /// `omp usage --json` with two Claude accounts and a Codex one.
    fn omp_usage() -> Value {
        let limit = |id: &str, window: Value, used: f64, tier: Option<&str>| {
            let mut scope = json!({"provider": id.split(':').next().unwrap(), "windowId": "x"});
            if let Some(t) = tier {
                scope["tier"] = json!(t);
            }
            json!({"id": id, "label": id, "scope": scope, "window": window,
                "amount": {"used": used * 100., "limit": 100, "usedFraction": used, "unit": "percent"},
                "status": "ok"})
        };
        let w = |ms: u64, resets: u64| json!({"id": "w", "label": "w", "durationMs": ms, "resetsAt": resets});
        let (h5, d7) = (5 * 3_600_000, 7 * 86_400_000);
        json!({
            "generatedAt": 1_761_750_000_000u64,
            "reports": [
                {"provider": "anthropic", "fetchedAt": 1, "limits": [
                    limit("anthropic:5h", w(h5, 1_761_760_800_000), 0.5, None),
                    limit("anthropic:7d", w(d7, 1_762_128_000_000), 0.2, None),
                    {"id": "anthropic:extra", "label": "Claude Extra Usage",
                     "scope": {"provider": "anthropic", "windowId": "extra"},
                     "amount": {"used": 12.5, "limit": 50, "unit": "usd"}},
                ]},
                {"provider": "anthropic", "fetchedAt": 1, "limits": [
                    limit("anthropic:5h", w(h5, 1_761_755_000_000), 0.3, None),
                    limit("anthropic:7d", w(d7, 1_762_200_000_000), 0.6, None),
                ]},
                {"provider": "openai-codex", "fetchedAt": 1, "limits": [
                    limit("openai-codex:primary", w(h5, 1_761_760_800_000), 0.1, None),
                    limit("openai-codex:spark:primary", w(h5, 1_761_760_800_000), 0.0, Some("spark")),
                ]},
            ],
            "accountsWithoutUsage": [],
            "disabledCredentials": [],
            "capacity": {},
        })
    }

    #[test]
    fn pools_omp_accounts() {
        let q = parse_omp(&omp_usage()).unwrap();
        let shown: Vec<(&str, i32)> = q
            .bars
            .iter()
            .map(|b| (b.label.as_str(), b.used.round() as i32))
            .collect();
        assert_eq!(
            shown,
            [
                ("Claude 5-hour", 40),
                ("Claude weekly", 40),
                ("Claude monthly", 25),
                ("Codex 5-hour", 10),
                ("Codex 5-hour (Spark)", 0),
            ]
        );
        // The soonest reset of the pooled accounts.
        assert_eq!(
            q.bars[0].resets,
            Some(Resets::At(
                UNIX_EPOCH + Duration::from_millis(1_761_755_000_000)
            ))
        );
        assert_eq!(q.note.as_deref(), Some("As observed by omp"));
        // One provider: no prefix.
        let mut one = omp_usage();
        one["reports"].as_array_mut().unwrap().truncate(1);
        let q = parse_omp(&one).unwrap();
        assert_eq!(q.bars[0].label, "5-hour");
        assert_eq!(parse_omp(&json!({"reports": []})), None);
    }

    #[test]
    fn describes_resets() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let later = |s| Resets::At(now + Duration::from_secs(s));
        assert_eq!(later(30).describe(now), "Resets in under a minute");
        assert_eq!(later(25 * 60).describe(now), "Resets in 25m");
        assert_eq!(later(2 * 3600 + 300).describe(now), "Resets in 2h 5m");
        assert!(later(3 * 86_400).describe(now).starts_with("Resets "));
        assert_eq!(Resets::Text("3pm".into()).describe(now), "Resets 3pm");
    }

    /// A fake CLI: `body` as a shell script, in a folder of its own.
    fn script(name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("motty-quota-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn runs_claude_cheaply() {
        let body = format!(
            "case \"$*\" in\n  *'-p /usage'*'--model haiku'*'--no-session-persistence'*) ;;\n  *) exit 3 ;;\nesac\n[ \"$CLAUDE_CODE_EFFORT_LEVEL\" = low ] || exit 4\ncat <<'EOF'\n{CLAUDE_PRINT}EOF\n"
        );
        let q = claude(&script("claude", &body), Duration::from_secs(10)).unwrap();
        assert_eq!(q.bars.len(), 2);
        let broken = script("claude-broken", "echo 'Error: not logged in' >&2\nexit 1\n");
        assert!(claude(&broken, Duration::from_secs(10)).is_err());
    }

    #[test]
    fn asks_codex_app_server() {
        let result = json!({"id": 1, "result": codex_result()});
        let body = format!(
            "[ \"$1\" = app-server ] || exit 2\nwhile IFS= read -r line; do\n  case \"$line\" in\n    *'\"method\":\"initialize\"'*)\n      echo '{{\"method\":\"account/updated\",\"params\":{{}}}}'\n      echo '{{\"id\":0,\"result\":{{\"userAgent\":\"codex/1\",\"platformOs\":\"macos\"}}}}' ;;\n    *'\"method\":\"initialized\"'*) ;;\n    *'\"method\":\"account/rateLimits/read\"'*'\"id\":1'*)\n      echo '{result}' ;;\n    *) echo '{{\"id\":9,\"error\":{{\"message\":\"Not initialized\"}}}}' ;;\n  esac\ndone\n"
        );
        let q = codex(&script("codex", &body), Duration::from_secs(10)).unwrap();
        assert_eq!(q.bars.len(), 3);
        let failing = "while IFS= read -r line; do\n  case \"$line\" in\n    *initialize*) echo '{\"id\":0,\"result\":{}}' ;;\n    *rateLimits*) echo '{\"id\":1,\"error\":{\"code\":-32600,\"message\":\"not signed in\"}}' ;;\n  esac\ndone\n";
        let e = codex(&script("codex-failing", failing), Duration::from_secs(10)).unwrap_err();
        assert!(e.contains("not signed in"), "{e}");
        let silent = script("codex-silent", "exec sleep 10\n");
        assert!(codex(&silent, Duration::from_millis(300)).is_err());
    }

    #[test]
    fn runs_omp_usage() {
        let body = format!(
            "[ \"$1 $2\" = 'usage --json' ] || exit 2\ncat <<'EOF'\n{}\nEOF\n",
            omp_usage()
        );
        let q = omp(&script("omp", &body), Duration::from_secs(10)).unwrap();
        assert_eq!(q.bars.len(), 5);
        let slow = script("omp-slow", "exec sleep 10\n");
        let started = Instant::now();
        let e = omp(&slow, Duration::from_millis(300)).unwrap_err();
        assert!(e.contains("took too long"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
