# Motty

A fast, GPU-accelerated terminal for macOS (Linux and Windows planned), built in Rust on
[GPUI](https://gpui.rs) and [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal),
with first-class support for coding agents.

## Build and run

```sh
cargo run -p motty-app                    # login shell
cargo run -p motty-app -- -e htop         # run a command instead
cargo build --release -p motty-app        # optimized build: target/release/Motty
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
registered automatically.

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
| `scrollback-lines`, `shell` | Behaviour |

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
in `target/parity/`. Add `--tabs` to compare a three-tab sidebar, `--theme NAME` to switch both
apps to a theme, `--backdrop RRGGBB` to put a solid color behind the window (for materials), and
`--set key=value` (repeatable) to change a config key in both apps, and `--hover x,y` to rest the
pointer on a window point (hover-only chrome; apps may not see a warped pointer move). Otty's config is restored
byte for byte afterwards. Captures are local only.
