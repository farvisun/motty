//! What a running agent's session has spent, read from the log it already
//! writes: how much of the context window it fills, its running token
//! total and, when the agent prices it itself, its cost.
//!
//! Nothing is guessed. A ceiling is known only when the log states one
//! (Codex's), so only that gets a bar; a cost is shown only when the agent
//! wrote one down (OpenCode, pi, omp), never priced from a rate card.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// How much of a log one read takes in, so a huge one catches up over a
/// few reads rather than stalling one.
const CHUNK: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    /// Tokens in the context window as of the latest reply.
    pub context: Option<u64>,
    /// The context window's size, when the log says.
    pub window: Option<u64>,
    /// Every token the session sent and received.
    pub tokens: Option<u64>,
    /// In US dollars, as the agent computed it.
    pub cost: Option<f64>,
}

impl Usage {
    pub fn is_empty(&self) -> bool {
        self.context.is_none() && self.tokens.is_none() && self.cost.is_none()
    }

    /// The share of the window in use, when both are known.
    pub fn fill(&self) -> Option<f32> {
        let (context, window) = (self.context?, self.window.filter(|w| *w > 0)?);
        Some((context as f64 / window as f64).min(1.) as f32)
    }
}

/// Whether an agent's log records usage at all (Otty's table).
pub fn reports(agent: &str) -> bool {
    matches!(
        agent,
        "claude" | "codex" | "omp" | "opencode" | "pi" | "kimi"
    )
}

/// A session's log, read as it grows: each update reads only what was
/// appended since the last.
pub struct Tally {
    agent: &'static str,
    path: PathBuf,
    offset: u64,
    /// Claude Code writes a line per content block, each carrying the
    /// whole message's usage: the message being written, counted once.
    message: Option<(String, u64)>,
    counted: u64,
    usage: Usage,
}

impl Tally {
    pub fn new(agent: &'static str, path: PathBuf) -> Self {
        Self {
            agent,
            path,
            offset: 0,
            message: None,
            counted: 0,
            usage: Usage::default(),
        }
    }

    /// Reads what was appended since the last update.
    pub fn update(&mut self) -> Usage {
        if self.agent == "opencode" {
            self.usage = opencode(&self.path);
            return self.usage.clone();
        }
        let Ok(mut file) = File::open(&self.path) else {
            return self.usage.clone();
        };
        let len = file.metadata().map_or(0, |m| m.len());
        // Rewritten from scratch: start over.
        if len < self.offset {
            *self = Self::new(self.agent, std::mem::take(&mut self.path));
        }
        if len == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return self.usage.clone();
        }
        let mut bytes = Vec::new();
        let _ = file.take(CHUNK).read_to_end(&mut bytes);
        // A line still being written waits for the next read.
        let Some(end) = bytes.iter().rposition(|b| *b == b'\n') else {
            return self.usage.clone();
        };
        for line in bytes[..end].split(|b| *b == b'\n') {
            if let Ok(v) = serde_json::from_slice::<Value>(line) {
                self.feed(&v);
            }
        }
        self.offset += end as u64 + 1;
        self.usage.clone()
    }

    fn feed(&mut self, v: &Value) {
        match self.agent {
            "claude" => self.claude(v),
            "codex" => self.codex(v),
            "pi" | "omp" => self.pi(v),
            "kimi" => self.kimi(v),
            _ => {}
        }
    }

    /// Kimi Code's `wire.jsonl`: a `usage.record` per request, and the
    /// context's size as it counts it.
    fn kimi(&mut self, v: &Value) {
        match v["type"].as_str().unwrap_or_default() {
            "usage.record" => {
                let u = &v["usage"];
                let total = num(&u["inputOther"])
                    + num(&u["output"])
                    + num(&u["inputCacheRead"])
                    + num(&u["inputCacheCreation"]);
                self.usage.tokens = Some(self.usage.tokens.unwrap_or(0) + total);
            }
            t if t.starts_with("token_counting.") => {
                if let Some(tokens) = v["tokens"].as_u64() {
                    self.usage.context = Some(tokens);
                }
            }
            // Older wires.
            "context.update_token_count" => {
                if let Some(tokens) = v["tokenCount"].as_u64() {
                    self.usage.context = Some(tokens);
                }
            }
            _ => {}
        }
    }

    fn claude(&mut self, v: &Value) {
        if v["type"] != "assistant" || v["isSidechain"] == true {
            return;
        }
        let u = &v["message"]["usage"];
        let input = num(&u["input_tokens"])
            + num(&u["cache_creation_input_tokens"])
            + num(&u["cache_read_input_tokens"]);
        let total = input + num(&u["output_tokens"]);
        // `<synthetic>` replies (an interrupted turn) carry no usage.
        if total == 0 {
            return;
        }
        let id = v["message"]["id"].as_str().unwrap_or_default().to_owned();
        match &mut self.message {
            // A line written before the reply was done can say less.
            Some((current, tokens)) if *current == id && !id.is_empty() => {
                if total < *tokens {
                    return;
                }
                *tokens = total;
            }
            _ => {
                self.counted += self.message.take().map_or(0, |(_, t)| t);
                self.message = Some((id, total));
            }
        }
        self.usage.context = Some(input);
        self.usage.tokens = Some(self.counted + total);
    }

    fn codex(&mut self, v: &Value) {
        let p = &v["payload"];
        if v["type"] != "event_msg" || p["type"] != "token_count" {
            return;
        }
        let info = &p["info"];
        if let Some(total) = info["total_token_usage"]["total_tokens"].as_u64() {
            self.usage.tokens = Some(total);
        }
        if let Some(last) = info["last_token_usage"]["total_tokens"].as_u64() {
            self.usage.context = Some(last);
        }
        if let Some(window) = info["model_context_window"].as_u64() {
            self.usage.window = Some(window);
        }
    }

    fn pi(&mut self, v: &Value) {
        let m = &v["message"];
        if v["type"] != "message" || m["role"] != "assistant" {
            return;
        }
        let u = &m["usage"];
        let total = match u["totalTokens"].as_u64().filter(|t| *t > 0) {
            Some(t) => t,
            None => {
                num(&u["input"]) + num(&u["output"]) + num(&u["cacheRead"]) + num(&u["cacheWrite"])
            }
        };
        self.usage.tokens = Some(self.usage.tokens.unwrap_or(0) + total);
        // An aborted or failed reply's usage is partial: not the context.
        // omp states the context itself.
        let settled = !matches!(m["stopReason"].as_str(), Some("aborted" | "error"));
        if settled && total > 0 {
            self.usage.context = Some(u["contextTokens"].as_u64().unwrap_or(total));
        }
        if let Some(cost) = u["cost"]["total"].as_f64() {
            self.usage.cost = Some(self.usage.cost.unwrap_or(0.) + cost);
        }
    }
}

/// OpenCode's legacy store: a JSON file per message, under a folder per
/// session, each assistant one with its tokens and cost.
fn opencode(dir: &Path) -> Usage {
    let mut messages: Vec<Value> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.ends_with(".json")))
        .filter_map(|e| serde_json::from_slice(&fs::read(e.path()).ok()?).ok())
        .filter(|v: &Value| v["role"] == "assistant")
        .collect();
    messages.sort_by_key(|v| v["time"]["created"].as_u64().unwrap_or(0));
    let mut usage = Usage::default();
    for m in &messages {
        let t = &m["tokens"];
        let total = num(&t["input"])
            + num(&t["output"])
            + num(&t["reasoning"])
            + num(&t["cache"]["read"])
            + num(&t["cache"]["write"]);
        if total > 0 {
            usage.context = Some(total);
            usage.tokens = Some(usage.tokens.unwrap_or(0) + total);
        }
        if let Some(cost) = m["cost"].as_f64() {
            usage.cost = Some(usage.cost.unwrap_or(0.) + cost);
        }
    }
    usage
}

fn num(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

/// A token count as the panel shows it: `950`, `45.2k`, `1.3M`.
pub fn count(n: u64) -> String {
    let short = |n: f64, unit: &str| {
        let text = if n < 100. {
            format!("{n:.1}")
        } else {
            format!("{n:.0}")
        };
        format!("{}{unit}", text.trim_end_matches(".0"))
    };
    match n {
        0..1_000 => n.to_string(),
        1_000..999_500 => short(n as f64 / 1e3, "k"),
        _ => short(n as f64 / 1e6, "M"),
    }
}

/// A cost in dollars, to the cent.
pub fn dollars(cost: f64) -> String {
    if cost > 0. && cost < 0.01 {
        "<$0.01".to_owned()
    } else {
        format!("${cost:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("motty-usage-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn append(path: &Path, lines: &[Value]) {
        let mut file = File::options()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    #[test]
    fn counts_each_claude_message_once() {
        let path = temp("claude").join("s.jsonl");
        let usage = |input, created, read, output| {
            json!({"input_tokens": input, "cache_creation_input_tokens": created,
                   "cache_read_input_tokens": read, "output_tokens": output})
        };
        append(
            &path,
            &[
                json!({"type":"user","message":{"role":"user","content":"hi"}}),
                // One reply written as two blocks, the usage on each.
                json!({"type":"assistant","message":{"id":"msg_1","usage":usage(10, 2000, 0, 5)}}),
                json!({"type":"assistant","message":{"id":"msg_1","usage":usage(10, 2000, 0, 40)}}),
                json!({"type":"assistant","isSidechain":true,"message":{"id":"msg_x","usage":usage(1, 1, 1, 1)}}),
                json!({"type":"assistant","message":{"id":"msg_s","model":"<synthetic>","usage":usage(0, 0, 0, 0)}}),
            ],
        );
        let mut tally = Tally::new("claude", path.clone());
        let u = tally.update();
        assert_eq!(u.context, Some(2010));
        assert_eq!(u.tokens, Some(2050));
        assert_eq!((u.window, u.cost, u.fill()), (None, None, None));
        // It reads on from where it stopped; a half-written line waits.
        append(
            &path,
            &[json!({"type":"assistant","message":{"id":"msg_2","usage":usage(3, 100, 2000, 7)}})],
        );
        let mut file = File::options().append(true).open(&path).unwrap();
        file.write_all(b"{\"type\":\"assis").unwrap();
        let u = tally.update();
        assert_eq!(u.context, Some(2103));
        assert_eq!(u.tokens, Some(2050 + 2110));
    }

    #[test]
    fn reads_codex_token_counts() {
        let path = temp("codex").join("rollout.jsonl");
        append(
            &path,
            &[
                json!({"type":"session_meta","payload":{"id":"abc"}}),
                json!({"type":"event_msg","payload":{"type":"token_count","info":null}}),
                json!({"type":"event_msg","payload":{"type":"token_count","info":{
                    "total_token_usage":{"input_tokens":52000,"cached_input_tokens":40000,"output_tokens":3000,"reasoning_output_tokens":1000,"total_tokens":55000},
                    "last_token_usage":{"input_tokens":20000,"cached_input_tokens":18000,"output_tokens":1000,"reasoning_output_tokens":200,"total_tokens":21000},
                    "model_context_window":258400}}}),
            ],
        );
        let u = Tally::new("codex", path).update();
        assert_eq!(u.tokens, Some(55000));
        assert_eq!(u.context, Some(21000));
        assert_eq!(u.window, Some(258400));
        assert!((u.fill().unwrap() - 21000. / 258400.).abs() < 1e-6);
    }

    #[test]
    fn reads_pi_usage_and_cost() {
        let path = temp("pi").join("2026-10-29_abc.jsonl");
        let reply = |total: u64, cost: f64| {
            json!({"type":"message","message":{"role":"assistant","usage":{
                "input":total - 100,"output":100,"cacheRead":0,"cacheWrite":0,"totalTokens":total,
                "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":cost}}}})
        };
        append(
            &path,
            &[
                json!({"type":"session","cwd":"/work"}),
                reply(1200, 0.01),
                json!({"type":"message","message":{"role":"assistant","stopReason":"aborted","usage":{"input":0,"output":0,"totalTokens":0}}}),
                reply(3400, 0.0325),
            ],
        );
        let u = Tally::new("omp", path).update();
        assert_eq!(u.context, Some(3400));
        assert_eq!(u.tokens, Some(4600));
        assert!((u.cost.unwrap() - 0.0425).abs() < 1e-9);
    }

    #[test]
    fn reads_opencode_messages() {
        let dir = temp("opencode");
        let message = |id: &str, created: u64, input: u64, cost: f64| {
            let v = json!({"id":id,"role":"assistant","time":{"created":created},"cost":cost,
                "tokens":{"input":input,"output":50,"reasoning":10,"cache":{"read":1000,"write":0}}});
            fs::write(dir.join(format!("{id}.json")), v.to_string()).unwrap();
        };
        message("msg_b", 2, 200, 0.02);
        message("msg_a", 1, 100, 0.01);
        fs::write(dir.join("msg_u.json"), json!({"role":"user"}).to_string()).unwrap();
        let u = Tally::new("opencode", dir).update();
        assert_eq!(u.context, Some(1260));
        assert_eq!(u.tokens, Some(1160 + 1260));
        assert!((u.cost.unwrap() - 0.03).abs() < 1e-9);
    }

    #[test]
    fn reads_kimi_wire_records() {
        let path = temp("kimi").join("wire.jsonl");
        let record = |input, output, read| {
            json!({"type":"usage.record","agentId":"main","model":"kimi-k2",
                "usage":{"inputOther":input,"output":output,"inputCacheRead":read,"inputCacheCreation":0},"time":1})
        };
        append(
            &path,
            &[
                json!({"type":"metadata","protocol_version":"1.5","created_at":1}),
                json!({"type":"context.update_token_count","tokenCount":900}),
                record(1000, 200, 0),
                json!({"type":"token_counting.measured","agentId":"main","length":4,"tokens":1200}),
                record(300, 100, 1200),
                json!({"type":"token_counting.turn_recorded","agentId":"main","length":6,"tokens":1600}),
            ],
        );
        let u = Tally::new("kimi", path).update();
        assert_eq!(u.context, Some(1600));
        assert_eq!(u.tokens, Some(2800));
        assert_eq!(u.cost, None);
    }

    #[test]
    fn short_numbers() {
        assert_eq!(count(950), "950");
        assert_eq!(count(45_230), "45.2k");
        assert_eq!(count(258_400), "258k");
        assert_eq!(count(999_700), "1M");
        assert_eq!(count(1_340_000), "1.3M");
        assert_eq!(dollars(0.4249), "$0.42");
        assert_eq!(dollars(0.001), "<$0.01");
    }
}
