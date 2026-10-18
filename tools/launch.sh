#!/bin/sh
# Launch klack (debug build) in a fresh kitty test window and open the DM with account B.
# Extra env (e.g. KLACK_LOG=klack=debug) is passed through. Run `cargo build` first.
hyprctl dispatch closewindow class:klack-test >/dev/null
timeout 10 sh -c 'while hyprctl clients -j | jq -e ".[] | select(.class==\"klack-test\")" >/dev/null; do sleep 0.3; done'
cd "$(dirname "$0")/.."
(kitty --class klack-test -o allow_remote_control=socket-only --listen-on unix:@klack-test sh -c './target/debug/klack; exec sleep 600' >/dev/null 2>&1 &)
timeout 10 sh -c 'until kitty @ --to unix:@klack-test ls >/dev/null 2>&1; do sleep 0.3; done'
timeout 5 sh -c 'sleep 3'
kitty @ --to unix:@klack-test send-key ctrl+k
kitty @ --to unix:@klack-test send-text dmytrobsp
kitty @ --to unix:@klack-test send-key enter
timeout 3 sh -c 'sleep 2'
