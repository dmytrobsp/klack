# Sidebar sections and starred conversations (web client, 2026-10-18)

Found in the web client bundles (`client-boot.js`, fetchers `fetchChannelSections*`) and
confirmed live against the testbed with `slacktui ctl api` / `ctl listen`.

## Starred ("favourites")

- Boot: `client.userBoot` → `starred: ["C…", …]` (conversation ids).
- `stars.add channel=C…` / `stars.remove channel=C…` (the client's `setStar` passes the item args
  through; messages/files use other args). `stars.list` → `items: [{type:"channel", channel}]`.
- The `stars` section (see below) only fixes where Starred sits; it lists no ids, and
  `users.channelSections.channels.bulkUpdate` into it fails with `cannot_modify_special_sections`.
- RTM: `{"type":"star_added"|"star_removed","user":…,"item":{"type":"channel","channel":"C…","date_create":…}}`.

## Sections

- `users.channelSections.list` → `channel_sections: [{channel_section_id:"L…", type, name, emoji,
  next_channel_section_id, channel_ids_page:{channel_ids:[…], count, cursor?}, last_updated}]`, plus
  `count`, `cursor`, `entities`. The order is a linked list via `next_channel_section_id` (null = last).
- Types seen: `stars`, `slack_connect`, `temporary_channels`, `salesforce_records`, `channels`,
  `direct_messages`, `recent_apps`, `agents` (built in; `channels`/`direct_messages` hold everything
  not placed elsewhere, so their id lists are empty) and `standard` (user-made).
- `users.channelSections.create name= emoji= [next_channel_section_id=] [channels=]` → `{channel_section_id}`;
  without `next_…` the new section is appended at the end.
- `users.channelSections.set channel_section_id= [name= emoji=] [next_channel_section_id=]` (rename / move).
- `users.channelSections.delete channel_section_id=`.
- `users.channelSections.channels.bulkUpdate insert=JSON remove=JSON`, each a list of
  `{channel_section_id, channel_ids:[…]}` (the client moves a channel with a remove from the old
  section and an insert into the new one in one call). DMs can be placed in sections too.
- `users.channelSections.channels.remove channel_section_id= channel_id=`.
- RTM:
  - `channel_section_upserted` `{channel_section_id, channel_section_type, name, emoji, next_channel_section_id, …}`
    (create and rename; creating did not re-send the previous tail)
  - `channel_section_deleted` `{channel_section_id}`
  - `channel_sections_channels_upserted` / `channel_sections_channels_removed` `{channel_section_id, channel_ids}`
- Not used yet: `createLink`/`linkInfo`/`validateShare` (shared sections), `entities.update`, the
  `saved.*` family (Later / saved items).
- Starring a conversation removes it from its user section on the server (`channel_sections_channels_removed` follows); unstarring does not put it back.

OBSERVED 2026-10-18 (D9): `channel_ids_page.cursor` is present on a section with ids and equals its last
channel id (`count` 1, one id, cursor = that id); the top-level `cursor` is the last section's id. The web
client (`resetChannelSectionsFromListApiResponseHandler`) reads only `channel_ids_page.channel_ids` and never
follows a cursor, and calls `users.channelSections.list` with no arguments — so the page is the whole list.
slacktui logs a warning if `count` ever exceeds the ids sent.
