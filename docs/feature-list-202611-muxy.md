# Features to borrow from muxy (2026-11)

This compares [muxy](https://github.com/muxy-app/muxy) with Motty, as of 2026-11-11 (muxy 2.4.0 plus the 2.5 betas, Motty 0.3.3).

Muxy is a terminal multiplexer for macOS and Linux, built with Rust, GPUI and libghostty, and aimed at developers who run AI coding agents. It has about 2.4k stars. It is Motty's closest competitor: the same UI toolkit and the same audience. Its main difference is architecture: a background server owns the terminal sessions, and the desktop app, a terminal UI (TUI), the CLI and the phone apps all connect to it as clients.

Decisions behind this list:
- Motty keeps its single-process design for now. Features that need a background server are in the [Parked](#5-parked-needs-a-background-server) section.
- Matching Otty pixel for pixel no longer limits UI changes.

Each feature has six fields:
- **Status in Motty:** has it, partial, or missing. Each status was checked in the code.
- **Complexity:** L, M or H, and the parts of Motty it touches.
- **Priority:** from 1 to 5, where 5 means build it first.
- **Don't Go:** what to leave out of the feature.

---

## 1. Where Motty is already equal or ahead

Don't copy anything here; muxy is behind or level.

- **Built-in editor, diff view, and previews** for Markdown, Mermaid, SVG and HTML. See `editor.rs` and `preview.rs`. Muxy opens files in an external app.
- **Agent hooks for 12 agents.** These cover session history, resume and **fork**, and readouts for cost, tokens and plan quota (`agents/`, `usage.rs`).
- **An MCP server**, `motty mcp`, that lets agents drive panes directly (`crates/motty-cli/src/mcp.rs`).
- **Recipes, tab rules, an AppleScript dictionary and `motty://` links** (`recipes.rs`, `applescript/`, `url_scheme.rs`).
- **Config import and export** for Ghostty, Kitty and Alacritty (`crates/motty-cli/src/import.rs`).
- **Extra pane types:** a details panel with Info, Outline, Git and Files tabs, plus Quick Look, Picture in Picture, a folder pane and an image viewer.
- **Small features muxy has that Motty already has too:**
  - per-pane font zoom (`terminal_view.rs` `zoom()`)
  - a shortcut recorder in Settings (`settings.rs` `Record`)
  - Cmd-clicking `path:line:col` (`crates/motty-term/src/links.rs`)
  - OSC 9;4 progress
  - no notification for the tab you are looking at (`NotificationForeground::TabUnfocused` in `workspace/status.rs`)
  - tear-off windows
  - Kitty, Sixel and iTerm2 images
  - jumping between prompts

---

## 2. Agent workflow

### 2.1 Agent detection without hooks
- **What it does:** Motty detects an agent's state (working, waiting or idle) by reading the screen. It also accepts a status escape code that agents can print (OSC 7501, muxy's format), so detection works over SSH and without installing any hook.
- **Why users stay:** it works on first launch, with no hook setup. It also covers agents Motty has no hook for, and agents running on remote machines.
- **Muxy:** detects 13 agents from the screen or from OSC 7501.
- **Status in Motty:** partial. Detection uses hooks only (`agents/hooks.rs`).
- **Complexity:** M. Touches `motty-term` (an OSC 7501 parser and a screen scan) and `agents.rs`. The team already solved screen and process scanning in Agentilo, so reuse that approach.
- **Priority:** 5. A fresh install shows nothing until hooks are installed, which is the biggest onboarding gap.
- **Don't Go:** don't drop hooks, which stay the precise source. Don't read the whole screen on every frame; scan only the last rows, with a debounce.

### 2.2 Agents panel
- **What it does:** a side panel that can be docked or detached and lists every running agent. It can be grouped by project, shows how long each agent has been idle, and lets you choose a compact or roomy layout. When several agents are running, "waiting" outranks "working".
- **Why users stay:** you can run many agents at once without checking each tab.
- **Muxy:** Agents panel (2.4) and an "Agents Focused" sidebar mode.
- **Status in Motty:** partial. The Inbox list and tab badges exist (`workspace/status.rs`).
- **Complexity:** M. Touches `workspace/`, `agents/` and `recovery.rs` (to remember the panel state).
- **Priority:** 5. It turns the Inbox into a control centre for agents.
- **Don't Go:** don't build a second agent-state model; drive the panel from the Inbox's existing state.

### 2.3 Composer: send to several panes, and keep drafts across restarts
- **What it does:** sends one prompt to every visible split pane, or to a chosen set of panes. Keeps each project's draft after Motty quits. Allows non-image file attachments.
- **Why users stay:** you can compare agents side by side ("ask Claude and Codex the same thing"), and you don't lose a long prompt.
- **Muxy:** a Composer with attachments, sending to all visible panes, and per-project drafts that survive restarts.
- **Status in Motty:** partial.
  - Composer, Prompt Queue and pasted-image chips exist (`terminal_view/composer.rs`).
  - Drafts are kept on ⎋ but are not part of `recovery.rs`.
  - Sending to several panes is missing.
- **Complexity:** L to M. Touches `composer.rs` and `recovery.rs`.
- **Priority:** 4. Cheap, and something agent users notice.
- **Don't Go:** don't add rich-text editing. The prompt stays plain text plus chips.

### 2.4 Clicking a notification jumps to its pane
- **What it does:** clicking a macOS notification focuses the exact window, tab and pane that sent it.
- **Why users stay:** you get from the alert to the answer in one click.
- **Muxy:** notifications jump to their pane, and only one app sends each alert.
- **Status in Motty:** partial. Notifications are sent, but nothing handles a click (`notifications.rs` sets no userInfo or delegate). Clicking the Dock icon jumps to the next tab that needs attention (`dock_click.rs`).
- **Complexity:** L. Touches `notifications.rs` (a pane id in userInfo plus a delegate) and the focus code in `ipc.rs` and `workspace`.
- **Priority:** 5. Small, and used every day.
- **Don't Go:** don't add action buttons to notifications yet.

### 2.5 CLI scripting matching the MCP tools
- **What it does:**
  - `motty send` and `send-keys`, to type into a pane.
  - `motty read-screen`, to print a pane's screen.
  - `motty wait --text <re> | --exit`, to wait for output or for the command to end.
  - `motty activity --json`, to list the agent states.
- **Why users stay:** agents and shell scripts can drive Motty without MCP.
- **Muxy:** `create`, `send`, `send-keys`, `read-screen`, `history`, `search`, `wait`, `activity list --json`.
- **Status in Motty:** partial.
  - The MCP server has `read_pane`, `send_text`, `run_command` and `find_in_pane`.
  - The CLI has a pane tree, `open` and `watch`, but no `wait` and no `activity`.
- **Complexity:** L. Touches `crates/motty-cli/src/main.rs` and `motty-ipc`, reusing the IPC calls the MCP tools already use.
- **Priority:** 4. Reuses existing IPC, and `wait` is great for automation.
- **Don't Go:** don't add a second protocol. The CLI and MCP share one set of IPC calls.

### 2.6 Agent-specific paste for images
- **What it does:** pasting an image into a pane running an agent's TUI sends the agent's own image-paste key (for example `Ctrl+V` for Claude Code), so the agent reads the image from the clipboard.
- **Why users stay:** pasting screenshots into agents just works.
- **Muxy:** yes.
- **Status in Motty:** partial. Rich paste and image paste exist in `rich_paste.rs` and `pasteboard.rs`; the agent-specific key path needs checking.
- **Complexity:** L. Touches `terminal_view/paste.rs` and `agents.rs`.
- **Priority:** 3.
- **Don't Go:** don't upload images anywhere.

---

## 3. Git, worktrees and pull requests

### 3.1 Create and remove worktrees, with setup scripts
- **What it does:**
  - Creates a worktree from a new or existing branch, from the menu, the palette or the Git tab, and opens it in a new tab.
  - Runs an optional setup script on create and a teardown script on remove, for example `.motty/worktree.toml` with `setup = "pnpm i && cp ../.env ."`. Each script runs only after you approve it once.
  - Removes the worktree only when it is clean.
- **Why users stay:** this is the standard way to run several agents in parallel.
- **Muxy:** worktrees from a new or existing branch, with setup and teardown scripts that you approve.
- **Status in Motty:** partial. The branch menu lists existing worktrees and switches to them (`menus.rs` lines 80–160); creating them is missing.
- **Complexity:** M. Touches `menus.rs`, `workspace/details/git.rs`, the palette and `motty-theme` (for the config).
- **Priority:** 5. Agent users ask for this the most.
- **Don't Go:** never remove a dirty worktree, never run a script nobody approved, and don't manage branches beyond creating one.

### 3.2 AI commit message
- **What it does:** a "Generate" button by the commit field asks an AI tool to write the message. It uses the agent already installed (for example `claude -p`), can be limited to staged changes, and can push afterwards.
- **Why users stay:** you can commit without leaving Motty.
- **Muxy:** AI commit, with staged-only, auto-push and one-off instructions.
- **Status in Motty:** missing. The commit field is manual (`workspace/details/git.rs:512`).
- **Complexity:** L to M. Touches `details/git.rs` and `agents/` (to pick the agent CLI).
- **Priority:** 4.
- **Don't Go:** no built-in API keys or provider SDKs. Shell out to an agent CLI the user already has. Don't commit without a confirmation step.

### 3.3 Create PR and PR status
- **What it does:**
  - Creates a pull request: branch, commit and push, then `gh pr create` with an AI-written title and body that follows the repo's PR template.
  - After that, the status bar shows the PR's state and checks, and the PR can be merged from there.
- **Why users stay:** it closes the loop from agent to PR.
- **Muxy:** Create PR, PR state and checks in the status bar, and merge.
- **Status in Motty:** missing.
- **Complexity:** M. Touches `details/git.rs` and the status bar or titlebar, and depends on the `gh` and `glab` CLIs.
- **Priority:** 3. Valuable, but tied to one forge. Support GitLab (`glab`) too, since the team uses GitLab.
- **Don't Go:** no OAuth inside Motty, because the `gh` and `glab` login is enough. No review UI.

---

## 4. Terminal and workspace UX

### 4.1 Quick Terminal (global drop-down)
- **What it does:** a system-wide shortcut slides a terminal down from the top of the screen. It can be pinned above other apps.
- **Why users stay:** a terminal is one key away from any app.
- **Muxy:** Quick Terminal.
- **Status in Motty:** missing. `materials::float_everywhere` and the Composer's float panel (`terminal_view/composer_panel.rs`) can be reused.
- **Complexity:** M. Touches `main.rs` (a global hotkey through Carbon `RegisterEventHotKey`), `materials.rs` and window management.
- **Priority:** 4. Common in terminals today (Ghostty, iTerm2, Warp).
- **Don't Go:** don't build a second tab system inside the Quick Terminal. It is one window with ordinary tabs.

### 4.2 Floating panes
- **What it does:** pops a pane out to float over the workspace inside the same window. It can be hidden or shown with a shortcut, or docked back as a tab or split.
- **Why users stay:** you can keep a log or a server in view without changing the layout.
- **Muxy:** floating terminals, shown or hidden with `Cmd+Opt+F`.
- **Status in Motty:** missing. Picture in Picture (`pip.rs`) is the closest.
- **Complexity:** M. Touches `panes.rs` and `pane_drag.rs`.
- **Priority:** 3. Picture in Picture already covers part of it.
- **Don't Go:** no floating panes inside floating panes, and no free-form tiling.

### 4.3 Projects and Workspaces
- **What it does:**
  - Projects become first-class: a folder with an icon, its own tabs, and an agent summary.
  - Workspaces are named filters over projects, so one project can be in several.
  - The sidebar gets three modes: Projects, Tabs, or Agents.
- **Why users stay:** you can move between many repos without tab chaos.
- **Muxy:** Projects, Workspaces, and three sidebar layouts.
- **Status in Motty:** partial. Tabs can be grouped by project or date (`workspace/tabs.rs:777`), and tab rules set icons.
- **Complexity:** H. Touches `workspace/tabs.rs`, `recovery.rs` and the sidebar.
- **Priority:** 3. Large, but it shapes how the whole app is organized. Build it after the Agents panel (2.2).
- **Don't Go:** don't add accounts or sync, and don't add a project-settings UI; tab rules keep doing that.

### 4.4 Per-project layouts kept in the repo
- **What it does:** `.motty/layouts/*.toml` files in a repo describe its tabs, splits and commands, for example "dev": server, tests, agent. You apply one by hand from the palette.
- **Why users stay:** a team shares one setup, and starting work on a repo takes one step.
- **Muxy:** YAML or JSON layouts in `.muxy/layouts/`, applied by hand.
- **Status in Motty:** partial. Recipes do this globally (`recipes.rs`), not per repo.
- **Complexity:** L. Touches `recipes.rs` (adding a project source) and the palette.
- **Priority:** 4. Cheap, because it reuses the recipe engine.
- **Don't Go:** never apply a layout on its own when a folder opens, and never run commands from a repo you have not approved (same approval as in 3.1).

### 4.5 Pane headers
- **What it does:** an optional bar above each split pane showing an icon, the running program and the folder.
- **Why users stay:** you can tell panes apart in big split layouts.
- **Muxy:** configurable pane headers (2.4).
- **Status in Motty:** missing.
- **Complexity:** L. Touches `panes.rs` and `motty-render`.
- **Priority:** 3.
- **Don't Go:** don't put buttons in the header. It shows information only, and is off by default for single panes.

### 4.6 Settings backup and restore
- **What it does:** exports the config, themes, recipes and keybindings to one `.motty` archive, and imports it again.
- **Why users stay:** moving to a new Mac is easy.
- **Muxy:** backup and restore to a `.muxy` file.
- **Status in Motty:** partial. Export exists for other terminals' formats (`crates/motty-cli/src/export.rs`).
- **Complexity:** L. Touches `motty-cli` and Settings.
- **Priority:** 2. TOML files are easy to copy by hand already.
- **Don't Go:** never include agent tokens or session transcripts.

### 4.7 Bigger theme catalogue
- **What it does:** adds about 450 themes, converted from the iTerm2-Color-Schemes set, which is MIT licensed.
- **Why users stay:** first impressions and personal taste.
- **Muxy:** about 490 themes.
- **Status in Motty:** partial, with 18 built-in themes (`crates/motty-theme/themes`).
- **Complexity:** L. Touches `motty-theme` and the theme picker (it needs search).
- **Priority:** 2.
- **Don't Go:** don't bundle themes into the binary; load them from files.

---

## 5. Extensions and platform

### 5.1 Extension system
- **What it does:** extensions add panels, tabs, commands, status-bar items and event hooks. Each declares its permissions in a manifest, and the user approves them while it runs.
- **Why users stay:** the community can build features, which creates a long-term moat.
- **Muxy:** extensions, a verified marketplace, agents that write extensions, and language packs.
- **Status in Motty:** missing. Recipes, the CLI, IPC and MCP cover simple automation.
- **Complexity:** H. Needs a new crate, an API that has to stay stable, a sandbox (probably WASM) and UI hosting.
- **Priority:** 2. Expensive to build and to maintain. Start by publishing a stable event stream over IPC (`motty events --json`), which covers about 80% of uses.
- **Don't Go:** no marketplace and no remote code loading in v1.

### 5.2 Browser pane for agents
- **What it does:** adds browser profiles with separate cookies, the web inspector, and `motty browser open|snapshot` so agents can drive the page and capture it.
- **Why users stay:** agents can check their web UI from inside the terminal.
- **Muxy:** browser tabs with an inspector, profiles, imported history, and CLI control.
- **Status in Motty:** partial. A web pane exists (`web_pane.rs`, `web_view.rs`).
- **Complexity:** M. Touches `web_view.rs` (WKWebView settings) and the CLI and MCP.
- **Priority:** 3. The inspector and snapshot are the useful parts.
- **Don't Go:** don't import history or bookmarks from other browsers, and don't build a full browser UI.

### 5.3 Linux build
- **What it does:** ships Motty on Linux as a `.deb` and an AppImage.
- **Why users stay:** more potential users, and Linux developer machines.
- **Muxy:** experimental Linux on x86_64 and ARM64.
- **Status in Motty:** missing; the README lists it as planned.
- **Complexity:** H. Many macOS-only modules need replacements: `materials`, `dock`, `applescript`, `notifications`, `web_view`, `quick_look`, `secure_input` and `updater`.
- **Priority:** 2. Only after the agent features above; the macOS-native polish is a selling point.
- **Don't Go:** no Windows in the same effort.

---

## 6. Parked: needs a background server

The user chose not to build these now. They stay here so the idea is not lost.

| # | Feature | What muxy does | Status in Motty | Complexity | Priority |
|---|---|---|---|---|---|
| 6.1 | Detach and reattach | Closing a pane only detaches it, and quitting the app never ends a terminal; "Existing Terminals" brings them back | Missing. Restore after a crash brings back layout and scrollback, but the processes have ended (`recovery.rs`) | H | 2 |
| 6.2 | Remote servers over SSH | `~/.ssh/config` aliases, reconnects on its own, installs the server on the remote machine, groups servers | Missing | H | 1 |
| 6.3 | Phone companion app and push | Pairing by QR code, terminal, Git and files on the phone, push after 20 seconds unseen | Missing | H | 1 |
| 6.4 | TUI client | tmux-style `Ctrl-B` client of the same server | Missing | H | 1 |

What a phased server design would need, for later:
1. Move ownership of the PTY and `alacritty_terminal::Term` out of `motty-term` into a `motty-server` process that serves `motty-ipc` over the Unix socket.
2. Make the GPUI app a client that receives screen updates.
3. Make the server outlive the app, through launchd, with an idle timeout.
4. Only after that, add SSH transport and pairing.

Muxy issue #1395 (the server stops when the app quits, so terminals end) shows what goes wrong when detach is only half done: either ship lifetime management that works or don't ship it.

---

## 7. Top 10 shortlist

| Rank | Feature | Section | Complexity | Why now |
|---|---|---|---|---|
| 1 | Clicking a notification jumps to its pane | 2.4 | L | Used every day, small change |
| 2 | Agent detection without hooks (screen and OSC 7501) | 2.1 | M | Fixes "no agents after install"; Agentilo already has the approach |
| 3 | Create and remove worktrees, with approved scripts | 3.1 | M | The core workflow for running agents in parallel |
| 4 | Agents panel | 2.2 | M | A control centre for many agents |
| 5 | CLI `wait`, `send-keys`, `read-screen` and `activity --json` | 2.5 | L | Reuses the IPC calls behind MCP |
| 6 | Composer: send to several panes, drafts kept across restarts | 2.3 | L to M | Comparing agents side by side |
| 7 | Per-project layouts kept in the repo | 4.4 | L | Reuses recipes |
| 8 | AI commit message | 3.2 | L to M | Commit without leaving Motty |
| 9 | Quick Terminal | 4.1 | M | Expected in modern terminals |
| 10 | Create PR and PR status (`gh` and `glab`) | 3.3 | M | Finishes the path from agent to PR |

Next after these: pane headers (4.5), floating panes (4.2), Projects and Workspaces (4.3), and the browser pane for agents (5.2).

## 8. Don't copy

- **Anonymous daily usage pings** (muxy 2.5 betas). They cost privacy and give users nothing.
- **A half-done background server** (muxy #1395). See section 6.
- **About 490 bundled themes inside the app.** Load themes from files instead (4.7).
- **A verified extension marketplace** before a stable API exists (5.1).

## 9. Sources

- muxy repository and README: https://github.com/muxy-app/muxy
- muxy docs: https://github.com/muxy-app/muxy/tree/main/docs
- muxy releases, 2.0.0 (2026-10-08) to 2.4.0 (2026-11-07) and the 2.5 betas: https://github.com/muxy-app/muxy/releases
- muxy website: https://muxy.app
- muxy issues: [#1395](https://github.com/muxy-app/muxy/issues/1395), [#1349](https://github.com/muxy-app/muxy/issues/1349), [#1135](https://github.com/muxy-app/muxy/issues/1135), [#1380](https://github.com/muxy-app/muxy/issues/1380), [#1400](https://github.com/muxy-app/muxy/issues/1400)
- Motty statuses: checked in this repo at commit `c7488b9` (v0.3.3).
