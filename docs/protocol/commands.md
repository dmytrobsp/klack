# Slash commands

How klack runs slash commands it does not implement itself (I6), found by probing (2026-10-19):

- `chat.command` with `channel`, `command` (with its `/`), `text` (mentions as `<@U…>`), and `thread_ts` for
  a thread → `{"ok": true, "keep_input": false}`. Slack's built-in commands work server-side this way,
  including `/shrug` (it posts "text ¯\_(ツ)_/¯") and `/remind`.
- An unknown command → error `unknown_command`.
- Answers come over RTM as a `message` event with `is_ephemeral: true` (e.g. `subtype: bot_message`,
  `user: USLACKBOT`, `text` plus `attachments` with `text`, `fallback` and `mrkdwn_in: ["text"]`); they are
  not in `conversations.history` and vanish on reload.
