# Motty

A fast, GPU-accelerated terminal for macOS (Linux and Windows planned), built in Rust on
[GPUI](https://gpui.rs) and [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal),
with first-class support for coding agents.

## Build and run

```sh
cargo run -p motty-app                    # login shell
cargo run -p motty-app -- -e htop         # run a command instead
cargo build --release -p motty-app        # optimized build: target/release/motty-app
cargo xtask bundle --release              # app bundle: target/release/Motty.app
```

Development builds compile Metal shaders at launch (`runtime-shaders` feature), so Xcode's
Metal toolchain isn't required. For release packaging, install it with
`xcodebuild -downloadComponent MetalToolchain` and build with `--no-default-features`.

## Configuration

Motty reads `~/.config/motty/config.toml`, or an existing `~/.config/otty/config.toml`, with
`config.local.toml` from the same folder applied on top.
Themes are `.ottytheme` TOML files, loaded from `~/.config/motty/themes` and
`~/.config/otty/themes`. Font files dropped into `~/.config/{motty,otty}/fonts` are
registered automatically. Saving a config or theme file applies it to open windows right away.

| Key | Meaning |
|---|---|
| `theme`, `theme-dark` | Theme names for light and dark system appearance |
| `font-family`, `font-size`, `font-ligatures` (`off`, `calt`, `dlig`) | Terminal font |
| `foreground`, `background`, `palette-0`…`palette-15` | Terminal colors when no `theme` is set (a theme's own colors win, as in Otty) |
| `macos-option-as-alt` | Send Option+key as Esc-prefixed (default `false`) |
| `cursor-style` | `block` (default), `bar`, `underline`, `block_hollow`; programs may switch it |
| `cursor-style-blink` | `default_off` (default) and `default_on` let programs turn blinking on or off; `always_off` and `always_on` don't |
| `cursor-color`, `cursor-text` | Cursor colors for themes that don't set their own |
| `window-hide-dirty-indicator` | Hide the close button's dot while a command runs |
| `sidebar-visible` | Open windows with the tabs sidebar (default `true`; toggle with ⌘⇧L) |
| `unfocused-split-opacity` | Opacity of split panes without focus (default `0.6`) |
| `scrollback-lines`, `shell` | Behaviour |

## Shell integration

zsh and fish started by Motty load a small script (Motty's own, injected through `ZDOTDIR` or
`XDG_DATA_DIRS` and undone before your startup files run) that reports prompts, commands with
their exit status (OSC 133) and the working directory (OSC 7). Programs can also send OSC 9 /
OSC 777 notifications and OSC 9;4 progress. Turn it off with `shell-integration = false`, or
per shell with `shell-integration-disabled-shells = ["fish"]`. bash support is still to come.

## Control from the shell (`motty`)

Motty listens on a Unix socket; shells it starts get `$MOTTY_SOCKET`, `$MOTTY_PANE_ID` and the
`motty` CLI on `PATH`, so scripts and coding agents can drive it:

```sh
motty list                         # panes in every window
motty run -- cargo test            # run in this pane's shell, wait, print output, exit with its status
motty read --last-command          # output of the last command (also --lines N, or the screen)
motty split -d down -- htop        # new pane; prints its id
motty send --pane 3 $'q'           # type into a pane
motty new-tab --cwd ~/src; motty focus --pane 3; motty close --pane 3
```

`motty mcp` serves the same operations to coding agents as MCP tools (`list_panes`, `read_pane`,
`run_command`, `send_text`, `split_pane`, `new_tab`, `focus_pane`, `close_pane`):

```sh
claude mcp add motty -- motty mcp      # Claude Code; Codex: [mcp_servers.motty] command = "motty", args = ["mcp"]
```

Agents can't type into their own pane through it; they split one off and work there.

`run` and `--last-command` need shell integration. `--json` prints machine-readable output; the
protocol (JSON lines) is in `crates/motty-ipc`.

## Splits

| Keys | Action |
|---|---|
| ⌘D, ⌘⌥D, ⇧⌘D, ⌥⇧⌘D | Split right, left, down, up (the new pane starts in the focused pane's folder) |
| ⌃⌘←→↑↓, ⌘], ⌘[ | Focus the pane in that direction, the next or the previous one |
| ⌃⌘=, ⇧⌘↩ | Equalize splits, zoom the focused pane |
| ⌘W | Close the focused pane (the tab with its last one) |

Drag a divider to resize. Themes set the divider with `[divider] color`, `width`.

## Layout

| Crate | Role |
|---|---|
| `motty-theme` | Theme and config parsing (CSS colors, shadows, borders, edges) |
| `motty-term` | PTY session, VT state, key/mouse/paste encoding |
| `motty-render` | The terminal grid as a GPUI element |
| `motty-app` | The application: windows, chrome, input |

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
Bundled JetBrains Mono is under the [SIL Open Font License](crates/motty-app/assets/fonts/jetbrains-mono/OFL.txt).

## Pixel parity

With Otty installed, `cargo xtask parity` renders `xtask/fixtures/render.sh` in both apps at
the same window bounds, captures both windows and prints per-region diff stats; images land
in `target/parity/`. Add `--tabs` to compare a three-tab sidebar, `--split right|down` to split the fixture pane, `--background CMD` (repeatable) to open tabs behind the fixture's, `--wait MS` to delay the captures, `--theme NAME` to switch both
apps to a theme, `--backdrop RRGGBB` to put a solid color behind the window (for materials), and
`--set key=value` (repeatable) to change a config key in both apps, and `--hover x,y` to rest the
pointer on a window point (hover-only chrome; apps may not see a warped pointer move). Otty's config is restored
byte for byte afterwards. Captures are local only.
