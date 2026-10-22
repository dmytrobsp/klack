#!/bin/sh
# Launch ttyak (debug build) on profile `default` (given by name, so it is test account A even when another profile is
# the default) in a fresh kitty test window and open the DM with account B.
# Extra env (e.g. TTYAK_LOG=ttyak=debug) is passed through. Run `cargo build` first.
hyprctl dispatch closewindow class:ttyak-test >/dev/null
timeout 10 sh -c 'while hyprctl clients -j | jq -e ".[] | select(.class==\"ttyak-test\")" >/dev/null; do sleep 0.3; done'
cd "$(dirname "$0")/.."
(kitty --class ttyak-test -o allow_remote_control=socket-only --listen-on unix:@ttyak-test sh -c './target/debug/ttyak --profile default; exec sleep 600' >/dev/null 2>&1 &)
timeout 10 sh -c 'until kitty @ --to unix:@ttyak-test ls >/dev/null 2>&1; do sleep 0.3; done'
timeout 5 sh -c 'sleep 3'
kitty @ --to unix:@ttyak-test send-key ctrl+k
kitty @ --to unix:@ttyak-test send-text dmytrobsp
kitty @ --to unix:@ttyak-test send-key enter
timeout 3 sh -c 'sleep 2'
