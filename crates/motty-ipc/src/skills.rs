//! The skills Settings › Agents › Skills hands a coding agent: what each
//! one teaches it to do with the `motty` command. `motty skill show <name>`
//! prints the instructions this build ships, so an installed skill never
//! teaches commands Motty no longer has.

pub struct Skill {
    pub name: &'static str,
    /// One line, for the agent deciding when to use it.
    pub description: &'static str,
    /// What to type once it's installed.
    pub usage: &'static str,
    /// It types into panes, which `ipc-allow-send-keys` must allow.
    pub types: bool,
    pub instructions: &'static str,
}

pub const SKILLS: [Skill; 4] = [
    Skill {
        name: "wait",
        description: "Block until a command, a pane, a tab or another coding agent in Motty is done, instead of sleeping and re-checking.",
        usage: "/wait for the build in the other tab",
        types: false,
        instructions: WAIT,
    },
    Skill {
        name: "tell",
        description: "Send a line to another terminal or coding agent in Motty: a pane, a tab or an agent session.",
        usage: "/tell the agent in tab 2 to start the smoke tests",
        types: true,
        instructions: TELL,
    },
    Skill {
        name: "open",
        description: "Open a file, a folder or a URL in Motty: in a split, a new tab or a new window.",
        usage: "/open src/main.rs in a split",
        types: false,
        instructions: OPEN,
    },
    Skill {
        name: "motty",
        description: "Spread work across Motty's window: open splits and tabs, run commands or a second coding agent in them, wait for them and read the results. Use when asked to run something in a split, in parallel, or to put another agent on a task.",
        usage: "/motty run the tests in a split while you keep going",
        types: true,
        instructions: MOTTY,
    },
];

pub fn find(name: &str) -> Option<&'static Skill> {
    SKILLS.iter().find(|s| s.name == name)
}

const PANES: &str = "\
Every pane has a number. `motty list` prints them, one per line: the id, its
window and tab (`w0 t1`), `*` on the focused one, the program, the folder and,
in brackets, the coding agent running there. `motty list --json` has more: the
agent's session id, whether a command is running (`busy`) and whether an agent
is in the middle of a turn (`in_turn`). Your own pane is `$MOTTY_PANE_ID`.";

const WAIT: &str = "\
# wait — block until something in Motty is done

The user gives you something to wait for: a pane id, an agent session id, or a
description (\"the build in the other tab\"). Find what they mean, then block
on it with one command — never sleep and poll.

## Find it
PANES

## Wait
- One pane: `motty wait --pane <id>`
- Every pane of that pane's tab: `motty wait --pane <id> --tab`
- An agent session: `motty wait --session <session-id>`
- Add `--timeout-secs <n>` to give up after n seconds (exit status 124).

A shell pane is done when its command has exited and the prompt is back; an
agent pane when the agent finished its turn. `motty wait` returns at once with
exit status 3 and says why when a pane can report neither (no shell
integration and no agent integration): tell the user exactly that rather than
falling back to sleeping.

When it returns, read what happened: `motty read --pane <id> --last-command`
for a shell command's output, or `motty read --pane <id> --lines 40` for the
end of the screen, and report it.";

const TELL: &str = "\
# tell — say something in another terminal

Send a line of text to another pane in Motty. Sent to a coding agent, it lands
in that agent's conversation (this is how one agent hands work to another);
sent to a shell, it's typed at the prompt as if the user typed it.

## Find the target
PANES

With no target named, the user means the focused pane (the one with `*`),
unless that's your own pane.

## Send
`motty send --pane <id> --submit \"<text>\"`

`--submit` presses Return when it's that pane's turn: the text waits in the
pane's prompt queue until the agent finishes its turn or the shell is back at
an empty prompt. `--submit=now` sends at once. Without `--submit` the text is
typed but not sent.

If Motty answers that typing into panes is off, tell the user to turn on
Settings › Advanced › IPC Allow Send Keys; don't retry. Panes in SSH, at a
password prompt or in Read Only mode refuse typed input.";

const OPEN: &str = "\
# open — put a file, a folder or a page on the user's screen

`motty open <path-or-url>` opens it in Motty:
- a file in an editor tab,
- a folder as a terminal there,
- a URL in the browser.

For a folder, add where: `--split right|left|down|up` beside your pane, `--tab`
(the default) or `--window`. Files always open in an editor tab. Relative paths
are from the current folder. Saying \"in a split\" or \"in a new window\" is
enough from the user; pick the flag.";

const MOTTY: &str = "\
# motty — spread work across Motty's window

You are running inside Motty, a terminal you can drive with the `motty`
command. Use it to run things beside you instead of blocking your own turn,
and to put a second coding agent on part of a task.

## See what's there
PANES

## Open panes (they don't take the user's focus with --no-focus)
- `motty split --pane $MOTTY_PANE_ID --direction right --no-focus` prints the
  new pane's id. `--cwd <dir>` sets its folder; arguments after it run a
  program instead of a shell.
- `motty new-tab --cwd <dir>` prints the new pane's id.

## Run and read
- `motty run --pane <id> <command line>` runs it in that pane's shell, waits,
  prints its output and exits with its status.
- `motty read --pane <id> --last-command` or `--lines <n>`.

## Hand work to another agent
1. `motty split --pane $MOTTY_PANE_ID --no-focus claude` (or `codex`, …).
2. `motty send --pane <id> --submit \"<the task, self-contained>\"`
3. `motty wait --pane <id>` blocks until it finishes its turn; then
   `motty read --pane <id> --lines 60`.

## Wait for several
`motty wait --pane <id> --tab` returns when every pane of that tab is idle.

## Tidy up
`motty close --pane <id>` — close only panes you opened, never your own
(`$MOTTY_PANE_ID`): that would end this conversation.

Typing into panes (`motty send`) needs Settings › Advanced › IPC Allow Send
Keys. If Motty refuses, tell the user; everything else works without it.";

/// The skill as the agent should store it, with its chosen `name`.
pub fn file(skill: &Skill, name: &str) -> String {
    let body = skill.instructions.replace("PANES", PANES);
    format!(
        "---\nname: {name}\ndescription: {}\n---\n\n\
         Before following this, run `motty skill show {}` and prefer what it \
         prints: it matches the Motty you're running.\n\n{body}\n",
        skill.description, skill.name
    )
}

/// What the user pastes into their agent: an installer, not the file.
pub fn installer(skill: &Skill, name: &str) -> String {
    format!(
        "Install a skill for yourself named \"{name}\", so that /{name} runs it. \
         Put it where you keep skills: for Claude Code that's \
         ~/.claude/skills/{name}/SKILL.md, for Codex ~/.codex/skills/{name}/SKILL.md; \
         for any other agent, wherever its skills or custom commands live. \
         Write the file below exactly, then tell me where you saved it and \
         whether I need to restart you to use /{name}.\n\n\
         ````markdown\n{}````\n",
        file(skill, name)
    )
}

/// The instructions `motty skill show` prints.
pub fn show(skill: &Skill) -> String {
    skill.instructions.replace("PANES", PANES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_carry_the_chosen_name() {
        let wait = find("wait").unwrap();
        let text = installer(wait, "otty-wait");
        assert!(text.contains("/otty-wait"));
        assert!(text.contains("name: otty-wait\n"));
        assert!(text.contains("motty skill show wait"));
        assert!(!show(wait).contains("PANES"));
    }
}
