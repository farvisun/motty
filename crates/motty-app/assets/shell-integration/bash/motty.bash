# Motty shell integration for bash (3.2 and later).
#   OSC 133 A/B/C/D  prompt start, input start, output start, command end
#   OSC 7            working directory
#
# Motty starts bash with `--rcfile` pointing here, since a login bash reads
# no file Motty could slip in first (and macOS's bash 3.2 ignores `ENV`).
# So this first runs the startup files a login shell would, then hooks in.

if [ -z "${_motty_sourced:-}" ]; then
  _motty_sourced=1
  if [ -r /etc/profile ]; then
    builtin source /etc/profile
  fi
  for _motty_file in ~/.bash_profile ~/.bash_login ~/.profile; do
    if [ -r "$_motty_file" ]; then
      builtin source "$_motty_file"
      break
    fi
  done
  builtin unset _motty_file
  # bash only allows `logout` in a login shell.
  if ! builtin declare -F logout >/dev/null; then
    logout() { builtin exit "$@"; }
  fi
fi

[ -n "${_motty_hooked:-}" ] && return
_motty_hooked=1
_motty_running=0
_motty_at_prompt=0

_motty_report_cwd() {
  # Byte by byte, so non-ASCII names are percent-encoded as UTF-8.
  builtin local LC_ALL=C url="file://${HOSTNAME}" ch enc i n
  for (( i = 0; i < ${#PWD}; i++ )); do
    ch="${PWD:i:1}"
    case "$ch" in
      [A-Za-z0-9/._~-]) url+="$ch" ;;
      *)
        # bash 3.2 reads bytes over 0x7f as negative.
        builtin printf -v n '%d' "'$ch"
        builtin printf -v enc '%%%02X' $(( n & 255 ))
        url+="$enc"
        ;;
    esac
  done
  builtin printf '\e]7;%s\a' "$url"
}

# First in PROMPT_COMMAND: close the last command while $? is still its status.
_motty_precmd_first() {
  builtin local status_=$?
  _motty_at_prompt=0
  if (( _motty_running )); then
    builtin printf '\e]133;D;%d\a' "$status_"
    _motty_running=0
  fi
  _motty_report_cwd
  return $status_
}

# Last in PROMPT_COMMAND, so other hooks' output comes before the prompt mark
# and prompt themes have rebuilt PS1 by now. Input starts where PS1 ends.
_motty_precmd_last() {
  builtin local status_=$?
  builtin printf '\e]133;A\a'
  builtin local mark='\[\e]133;B\a\]'
  case "$PS1" in
    *"$mark") ;;
    *) PS1="$PS1$mark" ;;
  esac
  _motty_at_prompt=1
  return $status_
}

_motty_preexec() {
  builtin printf '\e]133;C\a'
  _motty_running=1
}

if builtin declare -F __bp_install >/dev/null || [ -n "${bash_preexec_imported:-}" ]; then
  # bash-preexec owns the DEBUG trap and PROMPT_COMMAND; join its hooks.
  _motty_bp_preexec() { _motty_at_prompt=0; _motty_preexec; }
  precmd_functions=(_motty_precmd_first "${precmd_functions[@]}" _motty_precmd_last)
  preexec_functions+=(_motty_bp_preexec)
else
  # The first command run from the prompt is the user's; commands in
  # PROMPT_COMMAND and completion functions aren't.
  _motty_debug() {
    (( _motty_at_prompt )) || return
    [ -n "${COMP_LINE:-}" ] && return
    case "$BASH_COMMAND" in
      _motty_precmd_first*) return ;;
    esac
    _motty_at_prompt=0
    _motty_preexec
  }
  # Keep any DEBUG trap already set, running it after ours.
  _motty_prev_trap=$(builtin trap -p DEBUG)
  _motty_prev_trap=${_motty_prev_trap#trap -- \'}
  _motty_prev_trap=${_motty_prev_trap%\' DEBUG}
  if [ -n "$_motty_prev_trap" ]; then
    builtin trap "_motty_debug; $_motty_prev_trap" DEBUG
  else
    builtin trap '_motty_debug' DEBUG
  fi
  builtin unset _motty_prev_trap
  # bash 5.1 runs every element of an array PROMPT_COMMAND; older ones run
  # a string, joined with newlines in case the user's ends in `;`.
  if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )); then
    PROMPT_COMMAND=(_motty_precmd_first "${PROMPT_COMMAND[@]}" _motty_precmd_last)
  else
    PROMPT_COMMAND="_motty_precmd_first${PROMPT_COMMAND:+
$PROMPT_COMMAND}
_motty_precmd_last"
  fi
fi

# `motty jump` changes this shell's folder itself, as zoxide does: nothing
# is typed into the pane and history keeps the `motty jump` you ran.
motty() {
  if [ "${1:-}" = jump ]; then
    builtin shift
    builtin local out
    out="$(command motty jump --shell-cd "$@")" || return
    case "$out" in
      cd:*) builtin cd -- "${out#cd:}" ;;
      ?*) builtin printf '%s\n' "$out" ;;
    esac
  else
    command motty "$@"
  fi
}
