# Motty shell integration for fish.
#   OSC 133 A/C/D  prompt start, output start, command end
#   OSC 7          working directory
# Motty put its folder first in XDG_DATA_DIRS to load this file; undo that.
if set -q MOTTY_ORIG_XDG_DATA_DIRS
    set -gx XDG_DATA_DIRS $MOTTY_ORIG_XDG_DATA_DIRS
    set -e MOTTY_ORIG_XDG_DATA_DIRS
else
    set -e XDG_DATA_DIRS
end

status is-interactive; or exit

function __motty_prompt --on-event fish_prompt
    if set -q __motty_running
        printf '\e]133;D;%d\a' $__motty_status
        set -e __motty_running
    end
    printf '\e]7;file://%s%s\a' (prompt_hostname) (string escape --style=url -- $PWD)
    printf '\e]133;A\a'
end

function __motty_preexec --on-event fish_preexec
    printf '\e]133;C\a'
    set -g __motty_running 1
end

function __motty_postexec --on-event fish_postexec
    set -g __motty_status $status
end
