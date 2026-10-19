# Activity feed (web client, 2026-10-18)

From the bundles (`activityFeedFetcher` = `activity.feed`; generic fetchers convert camelCase args to
snake_case) and live probes with `klack ctl api`.

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

## Threads view: `subscriptions.thread.getView`

What the Threads view (Ctrl+T, H4m) uses — Slack's own "Threads" list, found by probing (2026-10-19):

- Arguments: `limit` (page size); `current_ts` = only threads active before that time (the answer echoes it
  as `max_ts`). `max_ts`, `ts` and `cursor` as arguments change nothing.
- Answer: `threads[]`, unread threads first, then by latest reply; `has_more`, `max_ts`,
  `total_unread_replies`, `new_threads_count`.
- Each thread: `root_msg` (a message plus `channel`, `reply_count`, `latest_reply`, `last_read`,
  `subscribed`) and `unread_replies[]` (when some are unread) and/or `latest_replies[]` (the newest, when
  read) — full message objects.
- `subscriptions.thread.mark` with `channel`, `thread_ts`, `ts` (the newest reply read) and `read=1` marks a
  thread read (used since I1); the Threads view drops it from the unread ones at once.
