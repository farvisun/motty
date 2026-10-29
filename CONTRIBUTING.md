# Contributing to Motty

Thanks for helping out. Bug reports, ideas and pull requests are all welcome in
[issues](https://github.com/farvisun/motty/issues).

## Setup

You need a Mac with a recent stable Rust toolchain (edition 2024) and the Xcode command line
tools (`xcode-select --install`).

```sh
git clone https://github.com/farvisun/motty.git
cd motty
cargo run -p motty-app              # debug build, opens a window
cargo xtask bundle --release        # target/release/Motty.app
```

Development builds compile Metal shaders at launch, so the Metal toolchain isn't required. The
crate layout is described in the [README](README.md#layout).

## Before you open a pull request

Run the same checks CI runs:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For changes to rendering, `cargo xtask parity` compares Motty's output against Otty when it is
installed (see the README).

Keep pull requests focused on one change, and describe how you tested it. Screenshots help for
anything visible.

## Commit messages

Commits follow [Conventional Commits](https://www.conventionalcommits.org/): a type, an optional
scope naming the crate or area, and a short summary in the imperative.

```
feat(term): kitty graphics, iTerm2 and sixel inline images
fix(render): keep wide glyphs inside their cell
docs: explain session recovery
```

Common types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`, `style`.
The changelog is grouped by these types.

## Branches

`main` is the only long-lived branch and should always build. Work on a topic branch and open a
pull request against `main`.

## Releasing

Maintainers cut releases from `main`:

1. Bump `version` under `[workspace.package]` in `Cargo.toml`.
2. Move the entries under `## [Unreleased]` in `CHANGELOG.md` to a new version section with
   today's date, and update the compare links at the bottom.
3. Commit as `chore(release): vX.Y.Z`, then tag and push:

   ```sh
   git tag vX.Y.Z
   git push origin main vX.Y.Z
   ```

The `Release` workflow builds a universal `Motty.dmg` with `scripts/package-dmg.sh` and attaches
it to a GitHub release, using that version's changelog section as the notes. To build the DMG
locally, run `scripts/package-dmg.sh`; it writes to `dist/`.

## The website

The landing page lives in `site/` as plain HTML, CSS and JavaScript, with no build step. Preview
it with `python3 -m http.server -d site 4321`. Pushing changes under `site/` to `main` deploys
it to GitHub Pages.

## License

By contributing, you agree that your contributions are dual-licensed under the
[MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) licenses, as the rest of the project is.
