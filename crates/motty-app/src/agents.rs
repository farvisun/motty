//! The coding agents Motty knows (Otty's list), and what a pane's tab
//! shows about the one running in it, or about its last command.

pub mod hooks;

pub struct Agent {
    /// The id hooks report under (`motty hook claude`).
    pub id: &'static str,
    pub label: &'static str,
    /// The program's name, as the pane's foreground process.
    pub command: &'static str,
}

pub const CATALOG: [Agent; 12] = [
    agent("claude", "Claude Code", "claude"),
    agent("codex", "Codex", "codex"),
    agent("opencode", "OpenCode", "opencode"),
    agent("cursor", "Cursor CLI", "agent"),
    agent("kimi", "Kimi Code", "kimi"),
    agent("pi", "Pi", "pi"),
    agent("omp", "omp", "omp"),
    agent("grok", "Grok Build", "grok"),
    agent("reasonix", "Reasonix", "reasonix"),
    agent("codebuddy", "CodeBuddy CLI", "codebuddy"),
    agent("antigravity", "Antigravity CLI", "agy"),
    agent("copilot", "Copilot CLI", "copilot"),
];

const fn agent(id: &'static str, label: &'static str, command: &'static str) -> Agent {
    Agent { id, label, command }
}

pub fn by_id(id: &str) -> Option<&'static Agent> {
    CATALOG.iter().find(|a| a.id == id)
}

pub fn by_command(process: &str) -> Option<&'static Agent> {
    CATALOG.iter().find(|a| a.command == process)
}

/// Where the agent's program is, looking where installers put programs:
/// an app started from the Dock doesn't get the shell's PATH.
pub fn locate(command: &str) -> Option<std::path::PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for dir in [
        ".local/bin",
        ".bun/bin",
        ".npm-global/bin",
        ".cargo/bin",
        ".opencode/bin",
        ".volta/bin",
        "Library/pnpm",
        ".local/share/pnpm",
        ".grok/bin",
        ".kimi-code/bin",
    ] {
        dirs.push(home.join(dir));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(Into::into));
    // nvm's node versions, newest last.
    if let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) {
        let mut versions: Vec<_> = entries.flatten().map(|e| e.path().join("bin")).collect();
        versions.sort();
        dirs.extend(versions.into_iter().rev());
    }
    dirs.into_iter().map(|d| d.join(command)).find(|p| {
        std::fs::metadata(p).is_ok_and(|m| {
            m.is_file() && std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0
        })
    })
}

/// Keeps the Mac awake (`caffeinate -i`) while `on`; the hold ends with
/// Motty, however it quits.
pub fn keep_awake(on: bool) {
    use std::process::{Child, Command, Stdio};
    use std::sync::Mutex;
    static HOLD: Mutex<Option<Child>> = Mutex::new(None);
    let mut hold = HOLD.lock().unwrap_or_else(|e| e.into_inner());
    if on && hold.is_none() {
        *hold = Command::new("/usr/bin/caffeinate")
            .args(["-i", "-w", &std::process::id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
    } else if !on && let Some(mut child) = hold.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// A custom agent's name as Otty shows it: `myagent` reads "Myagent".
pub fn label_for(id: &str) -> String {
    match by_id(id) {
        Some(a) => a.label.to_owned(),
        None => {
            let mut chars = id.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

/// The small mark on a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge {
    /// An agent is working: a spinner.
    Running,
    /// An agent waits for approval or an answer: a hand.
    Awaiting,
    /// An agent finished its turn while you were elsewhere: a check.
    Done,
    /// A command exited 0 while you were elsewhere: an accent dot.
    Finished,
    /// An agent's turn or a command failed: a triangle.
    Error,
}

impl Badge {
    pub fn name(self) -> &'static str {
        match self {
            Badge::Running => "running",
            Badge::Awaiting => "awaiting",
            Badge::Done => "done",
            Badge::Finished => "finished",
            Badge::Error => "error",
        }
    }

    /// Needs the user: these come first in Inbox.
    pub fn needs_you(self) -> bool {
        matches!(self, Badge::Awaiting | Badge::Error | Badge::Done)
    }

    /// Ongoing state; the rest only mark what happened while away and
    /// clear when the tab is looked at.
    pub fn ongoing(self) -> bool {
        matches!(self, Badge::Running | Badge::Awaiting)
    }
}

/// What a pane is up to, for its tab's badge and its line in Inbox.
#[derive(Clone, Debug)]
pub struct Status {
    /// The agent's display name, when an agent reported this.
    pub agent: Option<String>,
    pub badge: Option<Badge>,
    /// The step it's on, what it waits for, or its last reply.
    pub message: Option<String>,
    pub session: Option<String>,
    /// A turn started and hasn't been reported finished.
    pub in_turn: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(label_for("claude"), "Claude Code");
        assert_eq!(label_for("myagent"), "Myagent");
        assert_eq!(by_command("agent").map(|a| a.id), Some("cursor"));
    }
}
