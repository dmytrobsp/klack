# klack's patch to crossterm 0.29.0

This folder is crossterm 0.29.0 exactly as published on crates.io, plus the changes below. Every changed
line is marked `// klack:`. `Cargo.toml` at the root of klack points at it with
`[patch.crates-io] crossterm = { path = "vendor/crossterm" }`. The crate is MIT (`LICENSE` here).

## Why

A terminal with SGR mouse reporting (`CSI < Cb ; Cx ; Cy M/m`) sends the mouse's side buttons as buttons 8
and 9 (`Cb` 128 and 129, plus 32 for a drag). crossterm 0.29's `parse_cb` answers those with a parse error,
so the event is lost and klack never sees "back" or "forward". browsers and X11/Wayland call button 8 back
and button 9 forward.

## What changed

- `MouseButton` (`src/event.rs`) gains `Back` and `Forward`.
- `parse_cb` (`src/event/sys/unix/parse.rs`) decodes buttons 8 and 9 as `Down`, and as `Drag` with the motion
  bit; the release (`m`) turns `Down` into `Up` as for the other buttons. Buttons 10 and 11 still fail.
- A unit test for the SGR sequences: `test_parse_csi_sgr_mouse_side_buttons`.
- Nothing else. The Windows backend never builds the new variants (its console events carry no side
  buttons), so it compiles unchanged. klack's own test is `the_mouses_side_buttons_go_back_and_forward`.

The crate is not a member of klack's workspace, so its own tests (which need dev-dependencies klack does
not have) are not run by `cargo test`; run them from a copy outside the workspace.

## When to drop it

When a crossterm release decodes SGR buttons 8 and 9 (with variants klack can match; adjust
`crates/klack/src/tui/event_loop.rs` if they are named differently). Then delete this folder and the
`[patch.crates-io]` entry. Also when klack moves to a `ratatui` / `ratatui-image` that needs a newer
crossterm: the patch then no longer applies (cargo warns that it is unused), so check whether that release
has the fix, and if not, vendor and patch the new version the same way.
