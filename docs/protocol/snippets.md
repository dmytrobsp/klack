# Snippets (`snippet_type`)

OBSERVED 2026-10-18 (rslack-testbed, uploads with `files.getUploadURLExternal` + `snippet_type`, read back
with `conversations.history`):

- A valid `snippet_type` is honoured: `real.rs` + `rust` → `filetype: rust` ("Rust"); `probe.py` + `rust` →
  renamed `probe.py.rs`, `rust` (the type wins over the name; the extension is appended); `probe.toml` +
  `toml` → `toml` ("TOML"); `build.sh` + `shell` → `shell`.
- An unknown type is refused: `nosuchlang` → `unknown_snippet_type` (not silently replaced).
- The tiny content `fn main() {}` came back as C++ (`probe.rs.cpp`, `probe.cpp`) whether sent as `rust`,
  `text`, or without an extension: Slack's own content detection seems to override for it. Longer Rust
  (`impl Foo for Bar { … }`) stayed Rust. (An earlier note blamed the `.rs` name — wrong.)

Accepted list: the web client's snippet editor languages, a `Set` in
`gantry-v2-async-client-boot-shared.*.min.js` (module `LqOh`, `let l=new Set(["apl","applescript",…])`,
142 names incl. `rust`, `toml`, `typescript`), plus `text`. `client-boot.*.min.js` module `ZOnG` `$D` is the
general filetype list (127, incl. documents/images; no `toml`) — not the snippet list.
