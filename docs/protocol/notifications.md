# Notification preferences (web client, 2026-10-18)

- Boot (`client.userBoot` → `prefs`) and `users.prefs.get` carry `all_notifications_prefs`, a JSON
  **string**: `{"channels": {"C…": {"desktop": "everything"|"mentions_dms"|"nothing", "muted": bool,
  "temp_mute_expiration_ts": 0}}, "global": {"global_desktop", "global_mpdm_desktop", "global_keywords"
  (comma list), "global_mobile", "threads_everything", sounds, …}}`.
- `prefs.muted_channels` is gone on current workspaces (null); mutes are `channels[*].muted`.
- Writes: `users.prefs.setNotifications name=… value=… global=true|false [channel_id=…]`. Without
  `global` the answer is `invalid_arguments`. Per conversation: `desktop` (`everything`, `mentions_dms`
  = "Just mentions", `nothing`; `default` and an empty value give `internal_error`, so an override
  cannot be removed this way), `muted` (`true`/`false`). Global: `global_desktop`, `global_keywords`.
- Removing a conversation's overrides (OBSERVED 2026-10-18, from the web client's
  `resetChannelNotificationOverride`): `name=reset value=default global=false channel_id=C…` drops the
  whole entry from `all_notifications_prefs.channels` — the mute too. `name=reset value=default_all
  global=true` resets every conversation. The answer of `setNotifications` carries the updated
  `all_notifications_prefs`, and RTM sends `pref_change` with the whole value.
- `users.prefs.set name=vip_users value=U1,U2` (comma list; empty clears).
- RTM: `{"type":"pref_change","name":"all_notifications_prefs","value":"<the whole JSON string>"}`
  and `{"type":"pref_change","name":"vip_users","value":"U1,U2"}`; the client also handles
  `vip_users_updated`.
