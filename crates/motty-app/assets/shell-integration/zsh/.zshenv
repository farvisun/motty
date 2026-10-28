# Motty shell integration bootstrap. Motty starts zsh with ZDOTDIR pointing
# here, so this runs first: it puts the user's ZDOTDIR back, runs their
# .zshenv, and hooks Motty in for interactive shells. zsh then reads the
# user's other startup files from the restored ZDOTDIR.
if [[ -n "${MOTTY_ORIG_ZDOTDIR+set}" ]]; then
  ZDOTDIR="$MOTTY_ORIG_ZDOTDIR"
  builtin unset MOTTY_ORIG_ZDOTDIR
else
  builtin unset ZDOTDIR
fi

{
  if [[ -r "${ZDOTDIR:-$HOME}/.zshenv" ]]; then
    builtin source -- "${ZDOTDIR:-$HOME}/.zshenv"
  fi
} always {
  if [[ -o interactive && -r "${MOTTY_SHELL_INTEGRATION:-}/zsh/motty.zsh" ]]; then
    builtin source -- "$MOTTY_SHELL_INTEGRATION/zsh/motty.zsh"
  fi
}
