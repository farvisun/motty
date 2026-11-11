//! An agent's state read from its screen, for agents without a hook or a
//! status report (OSC 7501): a working agent shows how to interrupt it, and
//! one waiting for approval shows a numbered choice or a yes/no prompt.
//! Only the bottom of the screen is read, where agents draw these.

use motty_ipc::AgentState;

/// Rows read from the bottom of the screen.
const ROWS: usize = 14;

/// Shown while waiting for the user's approval or answer.
const AWAITING: [&str; 9] = [
    "1. yes",
    "1. allow",
    "(y/n)",
    "[y/n]",
    "allow command",
    "allow execution",
    "permission required",
    "do you want to proceed",
    "waiting for approval",
];

/// Shown while working.
const WORKING: [&str; 5] = [
    "esc to interrupt",
    "esc to cancel",
    "esc to stop",
    "ctrl+c to interrupt",
    "ctrl-c to interrupt",
];

/// The state the bottom of `screen` shows: waiting beats working, and
/// anything else is idle.
pub fn classify(screen: &str) -> AgentState {
    let bottom: Vec<String> = screen
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(ROWS)
        .map(str::to_lowercase)
        .collect();
    let any = |markers: &[&str]| {
        bottom
            .iter()
            .any(|line| markers.iter().any(|m| line.contains(m)))
    };
    if any(&AWAITING) {
        AgentState::Awaiting
    } else if any(&WORKING) {
        AgentState::Processing
    } else {
        AgentState::Idle
    }
}

/// Follows one pane's screen. A change counts once it's seen on two reads
/// in a row, so a redraw doesn't end a turn.
#[derive(Debug, Default)]
pub struct Tracker {
    /// The agent being followed (its catalog id) and the state reported.
    current: Option<(&'static str, AgentState)>,
    /// A different state seen once.
    pending: Option<AgentState>,
}

impl Tracker {
    /// Reads the screen of a pane running `agent` (or none); returns the
    /// agent and state to report when they changed. `Ended` when the agent
    /// left.
    pub fn read(
        &mut self,
        agent: Option<&'static str>,
        screen: impl FnOnce() -> String,
    ) -> Option<(&'static str, AgentState)> {
        let Some(agent) = agent else {
            self.pending = None;
            let (gone, _) = self.current.take()?;
            return Some((gone, AgentState::Ended));
        };
        let state = classify(&screen());
        match self.current {
            // A new agent is reported right away.
            Some((id, _)) if id != agent => {}
            None => {}
            Some((_, now)) if now == state => {
                self.pending = None;
                return None;
            }
            Some(_) if self.pending != Some(state) => {
                self.pending = Some(state);
                return None;
            }
            Some(_) => {}
        }
        self.pending = None;
        self.current = Some((agent, state));
        Some((agent, state))
    }

    /// Stops following: a hook or a status report speaks for the pane.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_bottom_of_the_screen() {
        let working = "> fix the bug\n\n✻ Thinking… (12s · esc to interrupt)\n";
        assert_eq!(classify(working), AgentState::Processing);
        let asking = "Bash command\n  rm -rf target\nDo you want to proceed?\n❯ 1. Yes\n  2. No\n";
        assert_eq!(classify(asking), AgentState::Awaiting);
        let codex = "Would you like to run the following command?\n› 1. Yes, proceed (y)\n";
        assert_eq!(classify(codex), AgentState::Awaiting);
        assert_eq!(classify("> \n  ? for shortcuts\n"), AgentState::Idle);
        // Old output scrolled above the bottom rows doesn't count.
        let old = format!("esc to interrupt\n{}", "line\n".repeat(ROWS));
        assert_eq!(classify(&old), AgentState::Idle);
    }

    #[test]
    fn a_change_needs_two_reads() {
        let mut t = Tracker::default();
        let working = || "esc to interrupt".to_owned();
        let idle = || "> ".to_owned();
        assert_eq!(
            t.read(Some("claude"), idle),
            Some(("claude", AgentState::Idle))
        );
        assert_eq!(t.read(Some("claude"), working), None);
        assert_eq!(
            t.read(Some("claude"), working),
            Some(("claude", AgentState::Processing))
        );
        // A one-read flicker is ignored.
        assert_eq!(t.read(Some("claude"), idle), None);
        assert_eq!(t.read(Some("claude"), working), None);
        assert_eq!(t.read(Some("claude"), working), None);
        assert_eq!(t.read(None, idle), Some(("claude", AgentState::Ended)));
        assert_eq!(t.read(None, idle), None);
    }
}
