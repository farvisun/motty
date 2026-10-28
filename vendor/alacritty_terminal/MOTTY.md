# alacritty_terminal 0.26.0, patched for Motty

Copied from the crates.io release (Apache-2.0, see LICENSE-APACHE); the
`tests/` fixtures are left out. Changes, kept small so they can go upstream:

- `Grid::lines_pushed`: a running count of lines pushed up off the top of
  the screen (into history, or out of a full history). `lines_pushed + line`
  identifies a row for as long as it exists, which shell-integration marks
  rely on.
