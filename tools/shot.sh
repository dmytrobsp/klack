#!/bin/sh
# usage: shot.sh OUT_DIR NAME — screenshot the ttyak-test kitty window to OUT_DIR/NAME.png
G=$(hyprctl clients -j | jq -r '.[] | select(.class=="ttyak-test") | "\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"' | head -1)
grim -g "$G" "$1/$2.png" && echo "$1/$2.png"
