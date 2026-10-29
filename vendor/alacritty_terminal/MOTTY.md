# alacritty_terminal 0.26.0, patched for Motty

Copied from the crates.io release (Apache-2.0, see LICENSE-APACHE); the
`tests/` fixtures are left out. Changes, kept small so they can go upstream:

- `Grid::lines_pushed`: a running count of lines pushed up off the top of
  the screen (into history, or out of a full history). `lines_pushed + line`
  identifies a row for as long as it exists, which shell-integration marks
  rely on.
- Keyboard state for Motty's encoder: `TermMode::MODIFY_OTHER_KEYS_1/_2`
  record XTMODKEYS (`CSI > 4 ; n m`, reported back for `CSI ? 4 m`), and
  `TermMode::KEYBOARD_LOCKED` records KAM (`CSI 2 h`/`l`, also in DECRQM).
  RIS clears both with the rest of the mode.
- `Term::reset_key_encodings` drops the kitty keyboard flags of both screens
  and modifyOtherKeys, so a program that exits without cleaning up doesn't
  leave them to the shell; `Term::title` reads the title for XTWINOPS 21t.
- Fix: a full kitty keyboard mode stack evicted from the title stack.
- `Grid::remove_history_lines`: drops a run of history rows, moving the
  older ones down (and `lines_pushed` with them, so their names hold). The
  repaint de-duplication (`motty-term/src/repaint.rs`) uses it to drop the
  stale copies full-screen repaints leave in scrollback.
