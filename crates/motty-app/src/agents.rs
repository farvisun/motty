//! The coding agents Motty knows (Otty's list), and what a pane's tab
//! shows about the one running in it, or about its last command.

pub struct Agent {
    /// The id hooks report under (`motty hook claude`).
    pub id: &'static str,
    pub label: &'static str,
    /// The program's name, as the pane's foreground process.
    pub command: &'static str,
    /// Where its integration lives, as Settings shows it.
    pub config: &'static str,
}

pub const CATALOG: [Agent; 12] = [
    agent("claude", "Claude Code", "claude", "~/.claude/settings.json"),
    agent("codex", "Codex", "codex", "~/.codex/hooks.json"),
    agent(
        "opencode",
        "OpenCode",
        "opencode",
        "~/.config/opencode/plugins",
    ),
    agent("cursor", "Cursor CLI", "agent", "~/.cursor/hooks.json"),
    agent("kimi", "Kimi Code", "kimi", "~/.kimi-code/config.toml"),
    agent("pi", "Pi", "pi", "~/.pi/agent/extensions"),
    agent("omp", "omp", "omp", "~/.omp/agent/extensions"),
    agent("grok", "Grok Build", "grok", "~/.claude/settings.json"),
    agent(
        "reasonix",
        "Reasonix",
        "reasonix",
        "~/.reasonix/settings.json",
    ),
    agent(
        "codebuddy",
        "CodeBuddy CLI",
        "codebuddy",
        "~/.codebuddy/settings.json",
    ),
    agent(
        "antigravity",
        "Antigravity CLI",
        "agy",
        "~/.gemini/config/hooks.json",
    ),
    agent("copilot", "Copilot CLI", "copilot", "~/.copilot/hooks"),
];

const fn agent(
    id: &'static str,
    label: &'static str,
    command: &'static str,
    config: &'static str,
) -> Agent {
    Agent {
        id,
        label,
        command,
        config,
    }
}

pub fn by_id(id: &str) -> Option<&'static Agent> {
    CATALOG.iter().find(|a| a.id == id)
}

pub fn by_command(process: &str) -> Option<&'static Agent> {
    CATALOG.iter().find(|a| a.command == process)
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
