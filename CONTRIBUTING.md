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

klack signs in with a browser session (`klack login --workspace <url>`, see the README). Use a
workspace of your own for testing: `klack --profile <name>` keeps separate sessions, and
`klack --profile <name> ctl …` (post, edit, react, history, listen, api, huddle-media with a test
tone or test video …) lets a second account play the other person in tests.

Logs: `~/.cache/klack/<profile>/klack.log` (macOS `~/Library/Caches/klack/…`, Windows
`%LOCALAPPDATA%\klack\cache\…`); more detail with `KLACK_LOG=klack=debug`. Tokens and cookies are
never logged — keep it that way.

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
