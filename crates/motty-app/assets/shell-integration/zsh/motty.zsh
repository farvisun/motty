# Motty shell integration for zsh.
#   OSC 133 A/B/C/D  prompt start, input start, output start, command end
#   OSC 7            working directory
[[ -n "${_motty_hooked:-}" ]] && return
typeset -g _motty_hooked=1 _motty_running=0

builtin autoload -Uz add-zsh-hook

_motty_report_cwd() {
  builtin emulate -L zsh
  builtin setopt localoptions no_multibyte
  builtin local url="file://${HOST}" ch enc i
  for (( i = 1; i <= ${#PWD}; i++ )); do
    ch="${PWD[i]}"
    if [[ "$ch" == [A-Za-z0-9/._~-] ]]; then
      url+="$ch"
    else
      builtin printf -v enc '%%%02X' "'$ch"
      url+="$enc"
    fi
  done
  builtin printf '\e]7;%s\a' "$url"
}

# First precmd hook: close the last command while $? is still its status.
_motty_precmd_first() {
  builtin local status_=$?
  if (( _motty_running )); then
    builtin printf '\e]133;D;%d\a' "$status_"
    _motty_running=0
  fi
  _motty_report_cwd
}

# Last precmd hook, so other hooks' output comes before the prompt mark and
# prompt themes have rebuilt PS1 by now. Input starts where PS1 ends.
_motty_precmd_last() {
  builtin printf '\e]133;A\a'
  builtin local mark=$'%{\e]133;B\a%}'
  [[ "$PS1" == *"$mark" ]] || PS1="$PS1$mark"
  precmd_functions=(${precmd_functions:#_motty_precmd_last} _motty_precmd_last)
}

_motty_preexec() {
  builtin printf '\e]133;C\a'
  _motty_running=1
}

precmd_functions=(_motty_precmd_first ${precmd_functions:#_motty_precmd_first})
add-zsh-hook precmd _motty_precmd_last
add-zsh-hook preexec _motty_preexec
