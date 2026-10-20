# Contributing to klack

## Building and testing

Install the packages for your system from the README's Requirements, then:

```sh
cargo build            # debug build: target/debug/klack
cargo test             # unit tests of all crates
cargo test --release -p klack build_lines_cost -- --ignored --nocapture   # message-pane cost
```

`tools/container-build.sh debian|ubuntu|fedora|arch` builds and tests a clean checkout of the
current commit in a fresh container with only the documented packages (needs podman). Run it
before changing dependencies or build steps.

Windows binaries are cross-built from Linux: `rustup target add x86_64-pc-windows-gnu`, install
MinGW-w64 (`apt install mingw-w64`), then `cargo build --release --target x86_64-pc-windows-gnu`.

## Running against Slack

klack signs in with a browser session (`klack auth login`, see the README "Signing in"). Use a
workspace of your own for testing: `klack --profile <name>` keeps separate sessions, and
`klack --profile <name> ctl …` (post, edit, react, history, listen, api, huddle-media with a test
tone or test video …) lets a second account play the other person in tests.

Logs: `~/.cache/klack/<profile>/klack.log` (macOS `~/Library/Caches/klack/…`, Windows
`%LOCALAPPDATA%\klack\cache\…`); more detail with `KLACK_LOG=klack=debug`. Debug builds only: `KLACK_FAIL=chat.delete,chat.update` makes those
Web API calls fail, to try what klack does when Slack refuses (release builds ignore it), and
`KLACK_DAYS_LATER=31` makes the sidebar think it is 31 days later (to see quiet conversations hide). Tokens and cookies are
never logged — keep it that way.

## Memory

Watch `VmRSS` / `RssAnon` in `/proc/<pid>/status` while using a release build on a busy
workspace (`RssAnon` is klack's own memory; the rest is mostly the program file). To see what
holds it, build with the heap profiler and quit normally (Ctrl+Q); `dhat-heap.json` lands in
the working folder:

    CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_STRIP=false \
    CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release --features dhat-heap

Load the file in [dhat's viewer](https://nnethercote.github.io/dh_view/dh_view.html), or sum
`gb` (bytes held at the peak) per allocation site from its `pps`. An optimised build with
`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` keeps `KLACK_FAIL`, e.g. to measure on a real
account without marking anything read: `KLACK_FAIL=conversations.mark,subscriptions.thread.mark`.

## Layout

- `crates/slack-core` — sessions, Web API, real-time events, message formatting.
- `crates/huddle` — huddles: Chime signaling, WebRTC audio/video, devices, camera, screen sharing.
- `crates/klack` — the terminal UI and the `ctl` command line.
- `docs/protocol/` — how the undocumented Slack endpoints klack uses were worked out.
- `tools/` — development and test helpers (see `tools/README.md`).

`DESIGN.md` has the architecture and portability rules; `PROGRESS.md` the development log.

## Style

Match the surrounding code. The code base is not formatted with `rustfmt`; do not reformat files
you are not otherwise changing.
