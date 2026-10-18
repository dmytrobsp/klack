#!/bin/sh
# Launch slacktui (debug build) in a fresh kitty test window and open the DM with account B.
# Extra env (e.g. SLACKTUI_LOG=slacktui=debug) is passed through. Run `cargo build` first.
hyprctl dispatch closewindow class:slacktui-test >/dev/null
timeout 10 sh -c 'while hyprctl clients -j | jq -e ".[] | select(.class==\"slacktui-test\")" >/dev/null; do sleep 0.3; done'
cd "$(dirname "$0")/.."
(kitty --class slacktui-test -o allow_remote_control=socket-only --listen-on unix:@slacktui-test sh -c './target/debug/slacktui; exec sleep 600' >/dev/null 2>&1 &)
timeout 10 sh -c 'until kitty @ --to unix:@slacktui-test ls >/dev/null 2>&1; do sleep 0.3; done'
timeout 5 sh -c 'sleep 3'
kitty @ --to unix:@slacktui-test send-key ctrl+k
kitty @ --to unix:@slacktui-test send-text dmytrobsp
kitty @ --to unix:@slacktui-test send-key enter
timeout 3 sh -c 'sleep 2'
