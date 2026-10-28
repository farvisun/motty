# Motty

A fast, GPU-accelerated terminal for macOS (Linux and Windows planned), built in Rust on
[GPUI](https://gpui.rs) and [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal),
with first-class support for coding agents.

## Build and run

```sh
cargo run -p motty-app                    # login shell
cargo run -p motty-app -- -e htop         # run a command instead
cargo build --release -p motty-app        # optimized build: target/release/Motty
```

Development builds compile Metal shaders at launch (`runtime-shaders` feature), so Xcode's
Metal toolchain isn't required. For release packaging, install it with
`xcodebuild -downloadComponent MetalToolchain` and build with `--no-default-features`.

## Configuration

Motty reads `~/.config/motty/config.toml`, or an existing `~/.config/otty/config.toml`.
Themes are `.ottytheme` TOML files, loaded from `~/.config/motty/themes` and
`~/.config/otty/themes`. Font files dropped into `~/.config/{motty,otty}/fonts` are
registered automatically.

| Key | Meaning |
|---|---|
| `theme`, `theme-dark` | Theme names for light and dark system appearance |
| `font-family`, `font-size`, `font-ligatures` | Terminal font |
| `foreground`, `background`, `palette-0`…`palette-15` | Color overrides for `theme` |
| `option-as-meta` | Send Option+key as Esc-prefixed (default `true`) |
| `cursor-blink`, `scrollback`, `shell` | Behaviour |

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
