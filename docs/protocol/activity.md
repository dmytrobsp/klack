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
  - `dm` / `mpdm`: `item.bundle_info.payload.dm_entry.latest_message = {channel, ts}` (the newest message).
- Pages (2026-10-21): with more items than `limit` the answer has `response_metadata.next_cursor`; `cursor=<it>`
  gives the next, older page (no overlap). The Activity view (VW5) loads it when the end of its list is on screen.
- Tabs (VW5): All = `at_user,at_user_group,at_channel,at_everyone,keyword,message_reaction,thread_v2,dm,mpdm`;
  DMs = `dm,mpdm,bot_dm_bundle` (`bot_dm` fails the whole request); Mentions = the `at_*` types and `keyword`;
  Threads = `thread_v2`.
- No message text: fetch it with `conversations.replies channel ts=T latest=T oldest=T inclusive=true limit=1`,
  which returns exactly that message whether it is top level or a reply.
- Not used yet: `activity.markUnread` {entries:[{type,key,ts,feed_ts}]}, `activity.markAllRead`, `activity.views.*`,
  `activity.archive`.

## The Activity badge and live items (2026-10-22, VX3)

From the web client bundles and live checks on the test workspace (event types and key shapes only).

- The badge is Slack's own count: `client.counts` → **`activity_v2`** `{<item type>: n}` (every type listed), at start
  and again at each (re)connect, then the socket event **`badge_counts_updated`** `{activity_v2, event_ts}` (sometimes
  also `counts`) whenever it changes. The All badge adds `at_user`, `at_user_group`, `at_channel`, `at_everyone`,
  `keyword`, `thread_v2`, `dm`, `channel`, `bot_dm_bundle`, the invite/list-mention types, `unjoined_channel_mention`,
  `quietly_added_to_channel`, `prejoin_dm_welcome_party_alert`; **not `message_reaction`** (reactions never badge),
  `mpdm`, `thread_reply`, `saved_reminder`, `code_review_requested`, … .
- Socket event `activity` with `subtype`: `activity_updated` {`entry` (an `activity.feed` item, `is_bot` added),
  `key`, `event_ts`} — a new item or a change (read here or elsewhere: `is_unread` false; a thread's newer reply);
  `activity_deleted` {`key`}; `activity_invalidated` (drop everything, load again); also
  `activity_clear_all_completed`, `activity_views_updated`. A thread reply also brings a `thread_reply` entry, a type
  the feed does not list. The same on the gateway and on `rtm.connect`.
- Seen live (B acting on A): a DM, a mention, a thread reply → `activity_updated` (unread) + `badge_counts_updated`; a
  reaction → `activity_updated` (`message_reaction`, read) and no badge change; reading elsewhere
  (`conversations.mark`, `subscriptions.thread.mark`) → `*_marked` + `activity_updated` (read) + `badge_counts_updated`.
- Opening an unread item (not a reaction) the client calls **`activity.markRead`** `type`, `feed_ts`, `key` (a `dm`:
  also `ts` = its `feed_ts`) → `{ok}`; Slack marks the conversation/thread read too and pushes the new badge.
- klack (VX3): the sidebar's Activity row shows that badge, from `client.counts` at start and after a reconnect and
  `badge_counts_updated` in between (one less at once when an unread item is opened here); Enter on an unread item
  sends `activity.markRead`; pushed items go into the Activity view's loaded tabs, deleted ones leave, an invalidation
  loads the shown tab again. A workspace whose counts have no `activity_v2` keeps the old count (the All tab's unread
  items at start and at each load).

## The badge's other item types (2026-10-22, VX3b)

Probed on the test workspace (`activity.feed mode=chrono_reads_and_unreads`, each type alone first, then all
together; answers' shapes only) and read from the web client's item parser (`bulkAddActivityItems`).

- **The feed takes every type the badge counts**, alone and together: `channel`, `bot_dm_bundle`,
  `internal_channel_invite`, `quietly_added_to_channel`, `unjoined_channel_mention`, `prejoin_dm_welcome_party_alert`,
  `external_channel_invite`, `external_dm_invite`, `shared_workspace_invite`, `list_record_assigned`,
  `list_record_edited`, `list_user_mentioned` (with the ones listed before). `thread_reply` and `bot_dm` still fail
  the whole request. Only `internal_channel_invite` had an item on the test workspace; the rest are from the code.
- Item shapes (under `item`, besides `type`):
  - `internal_channel_invite` (seen): `message {channel, ts, author_user_id, is_broadcast}` — the `channel_join`
    message (with `inviter`) in the channel you were added to.
  - `channel`: `bundle_info.payload.channel_entry {latest_message {channel, ts}, unread_msg_count}`.
  - `bot_dm_bundle`: `bundle_info.payload.message {channel, ts, thread_ts}`, `bundle_info.unread_count` — an app's DM.
  - `unjoined_channel_mention`: `unjoined_channel_mention_payload {channel_id, message_ts}`.
  - `quietly_added_to_channel`: `quietly_added_to_channel_payload {channel_id}`, no message (the client uses
    `feed_ts`).
  - `prejoin_dm_welcome_party_alert`: `prejoin_dm_welcome_party_payload {channel_id, invitee_user_id}`.
  - `external_channel_invite` / `external_dm_invite`: `invite` (an invitation to accept; no conversation yet).
    `shared_workspace_invite` has no case in the client's parser.
  - lists: `linked_item_id` and `list_payload {list_id, row_id, assigned_by}` / `list_edit_payload {…, edited_by}` /
    `list_user_mention_payload {…, mentioned_by}`.
- The client's tabs: Mentions = the `at_*` types, `keyword`, `list_record_assigned`, `list_user_mentioned`;
  Invitations = the invite types and `quietly_added_to_channel`; Apps = `bot_dm_bundle`; Channels = `channel`.
- klack (VX3b): the All tab asks for every type the badge counts, so its unread items and the badge agree; Mentions
  also lists `unjoined_channel_mention`. Enter: a channel item → the channel at its newest message; an app's item →
  its DM (in the thread); `internal_channel_invite` → its join message; `quietly_added_to_channel` /
  `prejoin_dm_welcome_party_alert` → the conversation; Slack Connect invitations and list items have nowhere to go
  (marked read, the status line says to open them in Slack). `activity.markRead` gets `ts` = `feed_ts` for DMs,
  invitations and list items. A `client.counts` answer asked for before a newer `badge_counts_updated` (or an item
  read here) is dropped.

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
