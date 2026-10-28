#!/bin/sh
# Rendering fixture: exercises colors, styles, box drawing and wide text.
# Clears first so terminals that echo the launch command show the same grid.
printf '\033[H\033[2J\033[3J'
printf '\033[1mMotty render fixture\033[0m  \033[2mdim\033[0m \033[3mitalic\033[0m \033[4munderline\033[0m \033[9mstrike\033[0m \033[7minverse\033[0m\n'
for i in 0 1 2 3 4 5 6 7; do printf "\033[4${i}m  \033[0m"; done; printf '\n'
for i in 0 1 2 3 4 5 6 7; do printf "\033[10${i}m  \033[0m"; done; printf '\n'
for i in 0 1 2 3 4 5 6 7; do printf "\033[3${i}m%s\033[0m " "color$i"; done; printf '\n'
for i in 0 1 2 3 4 5 6 7; do printf "\033[9${i}m%s\033[0m " "brite$i"; done; printf '\n'
i=0; while [ $i -lt 72 ]; do r=$((i*255/71)); printf "\033[48;2;%d;%d;%dm \033[0m" $r $((255-r)) 128; i=$((i+1)); done; printf '\n'
printf '┌──────┬──────┐  ┏━━━━━━┓  ▀▄█▌▐░▒▓  ▖▗▘▙▚▛▜▝▞▟\n'
printf '│ box  │ grid │  ┃heavy ┃  ╭─╮ ═══ ║\n'
printf '├──────┼──────┤  ┗━━━━━━┛  ╰─╯\n'
printf '└──────┴──────┘\n'
printf 'wide: 漢字テスト 한국어  emoji: 🚀 👩‍💻 ✅  persian: سلام دنیا\n'
printf 'ligatures: -> => != === <= >= :: |> <|\n'
printf '\033]0;Motty fixture\007'
printf '$ '
