# klack's patch to rtc-turn 0.21.1

This folder is rtc-turn 0.21.1 exactly as published on crates.io (git `2a732df5`, see `.cargo_vcs_info.json`),
plus the changes below. Every changed line is marked `// klack:`. `Cargo.toml` at the root of klack points
at it with `[patch.crates-io] rtc-turn = { path = "vendor/rtc-turn" }`. The crate on crates.io ships no
license files; `LICENSE-MIT` and `LICENSE-APACHE` here are the ones of the same repository
(webrtc-rs/rtc, copied from the `rtc` 0.21.1 crate). The crate is MIT or Apache-2.0, like klack.

## Why

About ten minutes into a huddle, Chime's TURN server can answer a refresh with **438 Stale Nonce**.
rtc-turn 0.21.1 handled that badly in all three places where it refreshes something:

- **ChannelBind** (`Relay::handle_channel_bind_response`): it took the new nonce, then deleted the
  binding. The server still relayed the meeting's audio on that channel, and the client dropped every
  packet (`no binding found for channel`). Its next ChannelBind, a new channel number for the same peer,
  was refused with 400. ICE consent failed a few seconds later, and the media connection went down.
  klack's reconnect (VX5) brought the call back, but with a gap of about 12 s.
- **CreatePermission** (`handle_create_permission_response`): a first permission answered 438 was
  reported as failed and removed. A periodic refresh answered 438 was not sent again.
- **Refresh of the allocation** (`handle_refresh_allocation_response`): it kept the new nonce but sent
  nothing. The next refresh came half a lifetime later, when the allocation had already expired.

## What changed

- On a 438 the request is sent again at once with the server's new nonce, as a new transaction, and only
  once. A second 438 for that retry is handled as before (fails, or deletes the binding).
- ChannelBind: the retry uses the same channel number, and the binding stays (with its state) meanwhile.
  So data on the channel keeps flowing both ways.
- CreatePermission: the retry asks for the same peers, and the permission stays. Its answer is reported
  under the transaction id the caller was given for the first request (`Event::CreatePermissionResponse`,
  `CreatePermissionError`, and `TransactionTimeout` too). webrtc's TURN relayer matches its pending
  permissions by that id.
- CreatePermission whose retry cannot be built (say the server's new nonce is longer than STUN allows):
  the error is logged and the permission fails as before (`CreatePermissionError` under the caller's id,
  the permission removed). Before, the error returned early and the permission waited forever.
- Refresh: the retry asks for the same lifetime. That is why the refresh transaction now carries the
  lifetime it asked for (`TransactionType::RefreshRequest(addr, lifetime)`): a `close()` (lifetime 0)
  answered 438 stays a release and does not extend the allocation. The refresh timer is not moved: the
  retry goes out at once instead.
- To do that, `Client` keeps a map from each retry's transaction id to the id it replaces
  (`stale_nonce_retries`). The private `channel_bind` and `refresh_allocation` return the transaction id.
  `handle_create_permission_response` and `handle_refresh_allocation_response` take the extra values they
  need. The crate's own tests were updated for the new arguments (`src/client/client_test.rs`).

The tests for this are klack's, against the crate's public API: `crates/huddle/src/turn_stale_nonce.rs`.
(The crate is not a member of klack's workspace, so its own tests, which need dev-dependencies klack
does not have, are not run by `cargo test`.)

## When to drop it

When an rtc-turn release (or the `webrtc` / `rtc` release klack moves to) retries ChannelBind,
CreatePermission and Refresh on 438 with the new nonce, without deleting the binding. Then delete this
folder and the `[patch.crates-io]` entry, and check that `crates/huddle/src/turn_stale_nonce.rs` still
passes (or delete it with the folder). Also when klack moves to a `webrtc` / `rtc` that needs a newer
rtc-turn: the patch then no longer applies (cargo warns that it is unused), so check whether that release
has the fix, and if not, vendor and patch the new version the same way.
