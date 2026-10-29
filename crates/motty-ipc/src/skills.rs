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
Every pane has a number. `motty pane list` prints a table of them: the id
(`*` on the focused one), its window and tab (`w_1`, `t_3`), its place in the
tab, the program, the coding agent running there or whether it's busy, and the
folder. `--tab <t_id>` or `--window <w_id>` narrows it. `motty --json pane
list` has more: the agent's session id, whether a command is running (`busy`)
and whether an agent is in the middle of a turn (`in_turn`). Your own pane is
`$MOTTY_PANE_ID`, and commands act on it when no `--pane` is given. A
`--pane`, `--tab` or `--window` that matches nothing fails with exit status 4;
it never falls back to another pane.";

const WAIT: &str = "\
# wait — block until something in Motty is done

The user gives you something to wait for: a pane id, an agent session id, or a
description (\"the build in the other tab\"). Find what they mean, then block
on it with one command — never sleep and poll.

## Find it
PANES

## Wait
- One pane: `motty pane wait --pane <id>`
- Every pane of that pane's tab: `motty pane wait --pane <id> --tab`
- Every pane of a window: `motty pane wait --window <w_id>`
- An agent session: `motty pane wait --session <session-id>`
- Add `--timeout-secs <n>` to give up after n seconds (exit status 9).

A shell pane is done when its command has exited and the prompt is back; an
agent pane when the agent finished its turn. `motty pane wait` returns at once
with exit status 6 and says why when a pane can report neither (no shell
integration and no agent integration): tell the user exactly that rather than
falling back to sleeping.

When it returns, read what happened: `motty pane capture --pane <id>
--last-command` for a shell command's output, or `motty pane capture --pane
<id> --lines 40` for the end of the screen, and report it.";

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
`motty pane send-text --pane <id> --submit \"<text>\"`

`--submit` presses Return when it's that pane's turn: the text waits in the
pane's prompt queue until the agent finishes its turn or the shell is back at
an empty prompt. `--submit=now` sends at once. Without `--submit` the text is
typed but not sent. C escapes in the text are typed as what they stand for
(`\\n`, `\\x03`), so write `\\\\` for a backslash; `--stdin` sends standard
input as it is. Single keys go with `motty pane send-keys --pane <id>
key:C-c` (or `key:Enter`, `key:Escape`, …).

If Motty answers that typing into panes is off, tell the user to turn on
Settings › Advanced › IPC Allow Send Keys; don't retry. Panes in SSH, at a
password prompt or in Read Only mode refuse typed input.";

const OPEN: &str = "\
# open — put a file, a folder or a page on the user's screen

- A file to read: `motty view <file>`; to change: `motty edit <file>`. Both
  open a new tab; `--right`, `--left`, `--top` or `--bottom` put it in a split
  beside your pane instead, `--new-window` in a new window. `motty view
  <http-url>` shows a page.
- A folder: `motty open <dir>` opens a new window there (`--command \"<line>\"`
  runs something in it); `motty tab new --cwd <dir>` a new tab; `motty pane
  split --cwd <dir> --direction right|left|up|down` a split beside your pane.
- A URL in the browser: `motty open <url>`.

Relative paths are from the current folder. Saying \"in a split\" or \"in a
new window\" is enough from the user; pick the command.";

const MOTTY: &str = "\
# motty — spread work across Motty's window

You are running inside Motty, a terminal you can drive with the `motty`
command. Use it to run things beside you instead of blocking your own turn,
and to put a second coding agent on part of a task.

## See what's there
PANES

## Open panes (they don't take the user's focus with --no-focus)
- `motty pane split --direction right --no-focus` splits your pane and prints
  the new pane's id. `--cwd <dir>` sets its folder, `--command \"<line>\"`
  runs a command line in its shell, `--size <10-90>` its share in percent.
- `motty tab new --cwd <dir>` prints the new tab's pane id.

## Run and read
- `motty pane exec --pane <id> <command line>` runs it in that pane's shell,
  waits, prints its output (stderr apart) and exits with its status. `motty
  pane run` does the same but only reports how it went.
- `motty pane capture --pane <id> --last-command`, `--lines <n>` or
  `--scrollback`.

## Hand work to another agent
1. `motty pane split --no-focus --command claude` (or `codex`, …).
2. `motty pane send-text --pane <id> --submit \"<the task, self-contained>\"`
3. `motty pane wait --pane <id>` blocks until it finishes its turn; then
   `motty pane capture --pane <id> --lines 60`.

## Wait for several
`motty pane wait --pane <id> --tab` returns when every pane of that tab is
idle. Exit status 6: a pane can't tell when it's done; 9: `--timeout-secs`
ran out.

## Tidy up
`motty pane close --pane <id>` — close only panes you opened, never your own
(`$MOTTY_PANE_ID`): that would end this conversation. It refuses when Motty
would ask first (a command still running); tell the user rather than adding
`--force`.

Typing into panes (`pane send-text`, `pane send-keys`) needs Settings ›
Advanced › IPC Allow Send Keys. If Motty refuses, tell the user; everything
else works without it.";

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
