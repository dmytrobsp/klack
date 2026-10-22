# Contributing to ttyak

## Building and testing

Install the packages for your system from the README's Requirements, then:

```sh
cargo build            # debug build: target/debug/ttyak
cargo test             # unit tests of all crates
cargo test --release -p ttyak build_lines_cost -- --ignored --nocapture   # message-pane cost
```

`tools/container-build.sh debian|ubuntu|fedora|arch` builds and tests a clean checkout of the
current commit in a fresh container with only the documented packages (needs podman). Run it
before changing dependencies or build steps.

Windows binaries are cross-built from Linux: `rustup target add x86_64-pc-windows-gnu`, install
MinGW-w64 (`apt install mingw-w64`), then `cargo build --release --target x86_64-pc-windows-gnu`.

## Running against Slack

ttyak signs in with a browser session (`ttyak auth login`, see the README "Signing in"). Use a
workspace of your own for testing: `ttyak --profile <name>` keeps separate sessions, and
`ttyak --profile <name> ctl …` (post, edit, react, history, listen, api, huddle-media with a test
tone or test video …) lets a second account play the other person in tests.

Logs: `~/.cache/ttyak/<profile>/ttyak.log` (macOS `~/Library/Caches/ttyak/…`, Windows
`%LOCALAPPDATA%\ttyak\cache\…`); more detail with `TTYAK_LOG=ttyak=debug`. Debug builds only: `TTYAK_FAIL=chat.delete,chat.update` makes those
Web API calls fail, to try what ttyak does when Slack refuses (release builds ignore it), and
`TTYAK_DAYS_LATER=31` makes the sidebar think it is 31 days later (to see quiet conversations hide), and
`TTYAK_HUDDLE_DROP=40` drops a huddle's connection 40 s after it connected (once), to try the reconnect. Tokens and cookies are
never logged — keep it that way.

## Memory

Watch `VmRSS` / `RssAnon` in `/proc/<pid>/status` while using a release build on a busy
workspace (`RssAnon` is ttyak's own memory; the rest is mostly the program file). To see what
holds it, build with the heap profiler and quit normally (Ctrl+Q); `dhat-heap.json` lands in
the working folder:

    CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_STRIP=false \
    CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release --features dhat-heap

Load the file in [dhat's viewer](https://nnethercote.github.io/dh_view/dh_view.html), or sum
`gb` (bytes held at the peak) per allocation site from its `pps`. An optimised build with
`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` keeps `TTYAK_FAIL`, e.g. to measure on a real
account without marking anything read: `TTYAK_FAIL=conversations.mark,subscriptions.thread.mark`.

## Layout

- `crates/slack-core` — sessions, Web API, real-time events, message formatting.
- `crates/huddle` — huddles: Chime signaling, WebRTC audio/video, devices, camera, screen sharing.
- `crates/ttyak` — the terminal UI and the `ctl` command line.
- `docs/protocol/` — how the undocumented Slack endpoints ttyak uses were worked out.
- `tools/` — development and test helpers (see `tools/README.md`).

`DESIGN.md` has the architecture and portability rules; `PROGRESS.md` the development log.

## Style

Match the surrounding code. The code base is not formatted with `rustfmt`; do not reformat files
you are not otherwise changing.
