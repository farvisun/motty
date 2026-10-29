//! A recipe's commands arriving in a pane, as `recipe-replay-*` says,
//! and the pane's recent commands for saving one.

use gpui::{Context, Window};
use motty_theme::Replay;

use super::TerminalView;
use crate::recipes::{MAX_COMMANDS, hands_off};

impl TerminalView {
    /// Runs or types `commands` here. Whatever follows a command that
    /// hands the keyboard to another shell (ssh, tmux attach) waits for a
    /// prompt.
    pub fn replay(
        &mut self,
        commands: Vec<String>,
        mode: Replay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if commands.is_empty() || mode == Replay::Skip {
            return;
        }
        self.queue.replayed.extend(commands.iter().cloned());
        match mode {
            Replay::Auto => self.enqueue(commands, window, cx),
            Replay::AskOnce => {
                let cut = commands
                    .iter()
                    .position(|c| hands_off(c))
                    .map_or(commands.len(), |i| i + 1);
                let (now, later) = commands.split_at(cut);
                self.write_input(&now.join("; "), cx);
                if !later.is_empty() {
                    self.queue.typed.extend(later.iter().cloned());
                }
            }
            Replay::Manually => {
                self.queue.typed.extend(commands);
                self.dispatch(window, cx);
            }
            Replay::Skip => {}
        }
    }

    /// The commands run here, oldest first, without those a recipe ran.
    pub fn recent_commands(&self) -> Vec<String> {
        let blocks = self.session.shell().blocks.clone();
        let mut replayed = self.queue.replayed.clone();
        let mut out: Vec<String> = blocks
            .iter()
            .filter(|b| b.end_row.is_some())
            .filter_map(|b| self.session.block_input(b))
            .map(|c| c.trim().to_owned())
            .filter(|c| !c.is_empty())
            .filter(|c| match replayed.iter().position(|r| r == c) {
                Some(i) => {
                    replayed.remove(i);
                    false
                }
                None => true,
            })
            .collect();
        let extra = out.len().saturating_sub(MAX_COMMANDS);
        out.drain(..extra);
        out
    }
}
