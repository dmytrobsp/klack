# Activity feed (web client, 2026-10-18)

From the bundles (`activityFeedFetcher` = `activity.feed`; generic fetchers convert camelCase args to
snake_case) and live probes with `slacktui ctl api`.

- `activity.feed mode=chrono_reads_and_unreads types=<comma list> limit=N` → `{ok, items:[…]}`.
  `types` is required; without it, or with an unknown type, the answer is `invalid_arguments`.
  `mode=priority_reads_and_unreads` was refused too. Other modes seen in the client: `chrono_unreads`
  (with `unread_only`, `priority_only`).
- Accepted types (testbed): `at_user`, `at_user_group`, `at_channel`, `at_everyone`, `keyword`,
  `message_reaction`, `thread_v2`, `internal_channel_invite`. `thread_reply` is refused (threads are
  `thread_v2`). The client enum also lists `dm`, `mpdm`, `bot_dm`, `list_*`, `saved_reminder`, … .
- Item: `{feed_ts, is_unread, is_archived, key, version, item:{type, …}}` with
  - mentions: `item.message = {channel, ts, author_user_id, is_broadcast}`
  - reactions: `item.message = {channel, ts}`, `item.reaction = {name, user}`
  - `thread_v2`: `item.bundle_info.payload.thread_entry = {channel_id, thread_ts, latest_ts, min_unread_ts, unread_msg_count}`
- No message text: fetch it with `conversations.replies channel ts=T latest=T oldest=T inclusive=true limit=1`,
  which returns exactly that message whether it is top level or a reply.
- Not used yet: `activity.markRead/markUnread` (custom fetcher, args from an `activityItem`),
  `activity.markAllRead`, `activity.views.*`, `activity.archive`.
