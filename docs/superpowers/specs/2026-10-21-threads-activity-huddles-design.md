# Threads, Activity and Huddles views; people's statuses — design

Asked by the user 2026-10-21, after comparing klack with seven screenshots of the official app (their work workspace;
nothing from those screenshots goes into the repo). Research: `~/projects/slack-rea/notes/huddles-list.md` (huddle
lists, live huddles, `huddle_state`, DM activity) and `notes/activity.md`.

## Why

Today's Threads (Ctrl+T) and Activity (Ctrl+A) are popups that say *that* something happened — a one-line quote, "X
mentioned you in #y" — but not enough to understand it without remembering the conversation. The user: "both fail to
provide me with enough context to understand what i'm looking at. i cant remember every single conversation there is.
slack somehow pulls it off, klack doesnt. their design is objectively better." Today Threads is only used to jump into
a conversation, Activity not at all.

**Success:** you open Threads, Activity or Huddles, read down, and understand each item where it is; you jump into a
conversation only because you want to. **Failure, even if it works:** anything that still needs you to remember the
conversation to make sense of an item.

## Goals

1. Threads, Activity and Huddles are **views in the main pane** (where a conversation is shown), not popups (user).
2. The sidebar has three rows at the top, together: **Threads** (exists), **Activity** (new), **Huddles** (new) (user).
3. Threads: a feed with enough context per thread to read it there; reply from it with the **shared composer at the
   bottom** (user).
4. Activity: read each item there, then jump to it (open the DM / the mentioning message / the thread at that reply).
   No in-place actions (user).
5. Huddles: at a glance, **which huddles are happening right now and who is in them**, clearly marked, plus recent
   huddles. The sidebar Huddles row shows a **badge counter while a huddle you can join is happening** (user).
6. **People's statuses** are visible at a glance — most of all in the sidebar, also in messages, threads, etc. — so you
   can see someone is in a huddle / in a meeting / on vacation (user).
7. While you are in a huddle through klack, **your "in a huddle" state is kept alive** the way the official app does it,
   so colleagues see it (user: yes).
8. Fix folded in: keyboard/click selection highlights the whole message row (AA2–AA4); the **mouse hover shade must
   cover the whole row too** (user).

## Non-goals

- Reply boxes under every thread (one shared composer instead — user's choice).
- Actions inside Activity items (reply, react, mark done) — Activity is read-then-jump (user).
- AI notes / recordings / transcripts of huddles (a paid Slack add-on). At most a note that a huddle has notes, if the
  data says so — builder's call.
- Activity's custom tabs ("+"), saved filters, and the official app's ✓ "done" marker — not asked for.
- Huddles filters "With / In" and "New Huddle" button — not asked for; starting a huddle stays where it is today.
- Changing how a huddle itself works (joining, media).

## Constraints

- Everything klack already does stays: keyboard-first with mouse support, NORMAL/INSERT modes, which-key, keys.toml
  overrides, F6 help, themes, i18n via `tr`, narrow terminals, pictures (kitty/sixel/iterm2/halfblocks), OSC 8 links,
  `↪` in-app links, ctl CLI unaffected.
- Ctrl+T and Ctrl+A keep opening Threads and Activity (now the views). Huddles gets a key too (builder: pick one that is
  free, add it to the keymap/which-key/README like the others).
- Messages in the new views look and behave exactly as in a conversation — the existing message rendering is reused
  (code blocks, reactions, pictures, links, `↪`, full-row selection), not re-implemented.
- Privacy as always: no real names, ids, emails or work-workspace content in the repo, tests or PROGRESS. Live checks
  only on the test workspace (rslack-testbed, profiles `default` = A and `b` = B), never the `test` profile.
- No new crate dependencies.

## Decided by the user (as decided)

- Main-pane views: yes. Shared composer at the bottom for replies from the Threads feed.
- An Activity row in the sidebar near Threads; a new Huddles view and row.
- Huddles shows live huddles clearly marked, with who is in them, plus recent ones; joining is possible but secondary
  ("i rarely use it as a tool to join huddles … but it is still useful to be able to see all huddles that are happening
  at a glance and who's in them").
- The Huddles row badge counts live huddles you can join (the official app's badge means something else — a ringing
  invite / unread invites — the user's version wins).
- Statuses: sidebar first, also messages, threads, etc.
- Keep your own `huddle_state` alive while in a huddle: yes.
- Hover shade full-row: yes (the sidebar's selected-row style and other lists stay as they are).

## The views

### Threads (main pane)

A scrolling feed, newest activity first (unread threads first, as today's `subscriptions.thread.getView` order), loaded
in pages as you scroll. One block per thread:
- a header: where (`#channel`, a DM's person, a group DM's people) and who is in the thread (a few names, "and N others");
- the thread's first message in full (author, time, status emoji, text, files, reactions);
- "N more replies" when replies are hidden;
- the last few replies in full; the "new" line where unread starts;
- blocks separated clearly (a blank row / rule).

Selection moves message by message through the feed (↑/↓, mouse), with the full-row highlight. Enter on any message
opens the whole thread in the thread pane at that message (as today). The Type key (`i`) on a selected message opens the
shared composer at the bottom, labelled with where it goes ("Reply in #channel thread" or the like); sending posts a reply
to that thread and the block updates. **Read-marking (user: "let's try your suggestion"):** a thread in the feed is marked read on Slack (the same call
opening it makes today, `subscriptions.thread.mark`) once its **whole block has been on screen for a moment** (**2 seconds**, the user's number — a setting like klack's other times; not while scrolling past quickly); its "new" line then fades on the next refresh as in a
conversation. A block too tall to fit counts once its unread replies have all been on screen. Opening the thread (Enter)
or replying marks it read as today.

### Activity (main pane)

Tabs: **All · DMs · Mentions · Threads** — switched with Tab/Shift+Tab or 1–4 (builder: fit klack's keymap; show the
keys in which-key and the status line). A key filters to unread only (builder's call, like the official "Unreads").
Items newest first, grouped under date headers ("Today", "Yesterday", weekday + date), loaded in pages. One item:
- author (and "+N" when several people are behind it, e.g. a thread with many new replies), time, status emoji;
- the message itself, up to about three lines, then cut off with an ellipsis; mentions/links/code styled as in messages;
- a "where" line: "Thread in #channel", "Mention in #channel thread", "Direct message", "Reacted :emoji: to: …" with the
  reaction pill, etc.;
- unread items marked as today.

Enter jumps: a DM item opens the DM; a mention opens its message (in the thread pane if it is a reply); a thread item opens
the thread at its newest/first unread reply; a reaction opens the reacted message. Data: `activity.feed`
(`mode=chrono_reads_and_unreads`). Tab types (research §5): All = today's types + `dm` (+ `mpdm`); DMs = `dm` (+ `mpdm`,
`bot_dm_bundle`; never `bot_dm`, which fails the whole request); Mentions = `at_user`, `at_user_group`, `at_channel`,
`at_everyone`, `keyword`; Threads = `thread_v2`. Message texts are fetched per shown item as today
(`conversations.replies … limit=1`).

### Huddles (main pane)

Two sections:
1. **Live now** — every huddle happening right now in conversations you are a member of (and not muted — builder: the
   official app hides muted ones; follow it), each clearly marked as live (colour + a live marker), showing where, how
   long it has been going, and **who is in it** (names/initials or profile pictures where the terminal can draw them,
   with status emoji). Your own current huddle first. Enter opens the conversation; joining from here is possible (the
   existing join action/keys).
2. **Recent** — `huddles.history` (pages of up to 30), newest first: where (`#channel`, a person for a DM), "N ago ·
   duration", a missed one as "Missed huddle" in the theme's alert colour, the people who took part, and "N replies" when
   its thread has replies. Enter opens the huddle's thread (`thread_root_ts`) in its conversation.

Data (research §1–2): recent = Web API `huddles.history`; live = the Flannel edge API — `api.getFlannelHttpUrl
include_external_workspaces=true` → URL, then POST JSON `{token, …}` with the `d` cookie to `<url>/huddles/list` and
`<url>/huddles/info {channel_ids}` — kept current by RTM `sh_room_join` / `sh_room_leave` / `sh_room_update` /
`huddle_start` / `ended_huddle_update`. klack has no Flannel client yet: it is new code in slack-core (small: one POST
helper). The live object's fields (`channel_id, call_id, room_id, active_members, dropped_members`) are only known from
the client's code — **verify them against a real test huddle first** (B starts one in rslack-testbed with
`klack ctl … huddle-media` as in earlier huddle tests; A reads), before building the view on them.

### Where you are, and going back (controller's call)

Threads, Activity and Huddles are places in the main pane like a conversation: opening one replaces what the main pane
shows. Esc (or the existing "back" behaviour) returns to the conversation you were in before; Ctrl+T / Ctrl+A (or the
Huddles key) while already in that view refreshes it. A jump from a view (Enter on an item) opens the conversation/thread
as a normal open, so back from there is the conversation-level behaviour klack already has (no new multi-level history).
At start klack restores the last **conversation** as today, never a view.

### Sidebar rows

At the top, in this order: Threads, Activity, Huddles (each with its icon, the selected-row style like a conversation's).
- Threads: as today (unread mark).
- Activity: an unread mark/count when the feed has unread items (builder: match how Threads shows unread).
- Huddles: a **badge with the number of live huddles you can join** (the "Live now" list's count, excluding the one you
  are in). Updated live from the RTM events.
Narrow sidebars and the collapsed sidebar handle the new rows as they handle Threads today.

## Statuses

Show each person's custom status **emoji right after their name**:
- the sidebar's DMs and group DMs (first priority);
- message authors (conversations, threads, the Threads feed, Activity items);
- huddle people (live and recent);
- the profile popup already shows emoji + text (keep).

The status **text** ("Vacationing until Friday") where there is room: on hover over a name/emoji (builder: the existing
hover-note mechanism), in a DM's conversation header, in the profile popup. Expired statuses
(`status_expiration` in the past) are not shown. Custom emoji statuses use the existing custom-emoji drawing.

**In a huddle** (research §4): a person whose profile has `huddle_state = in_a_huddle` with `huddle_state_expiration_ts`
in the future and **no** status emoji of their own shows a huddle marker (🎧 or the theme's equivalent) the same way; with
an emoji, their emoji wins (the official app's rule). Profiles update from `user_change` / `user_profile_changed` events
as today; a huddle-state expiry is re-checked over time without an event.

**Your own huddle state** (goal 7): while you are in a huddle through klack, do **exactly what the official client
does** (controller's call — the user's words were "the way the official app does it"): the same gate (the client only does
it when the user's `huddles_set_status` pref is on — so it applies in every workspace, the work one included, exactly when
the official app would), the same fields (research §4: the client only bumps `huddle_state_expiration_ts` to about now +
300 s every ~240 s and never writes `huddle_state` itself — if a live check on the test workspace confirms the server
sets `in_a_huddle` on join, send only what the client sends), and on leaving whatever the client does (stop / reset).
Confirm by reading the client code in `~/projects/slack-rea` and one live check on rslack-testbed before writing it. It
must never touch your status emoji/text.

## Hover

The mouse hover shade on messages covers the whole message row from the pane's left inner edge to the right one, like the
selection (AA2's approach in `MessagePane::draw`), wherever messages are drawn (conversation, thread pane, the new feeds).
The sidebar's hover stays as it is.

## Left to the builder

- How many replies a Threads block shows (2–3), and how a very long first message is shortened (with a way to see it all).
- Exact layout, colours (theme tokens; add new ones like the others and list them in README's Themes), icons, the live
  marker, date-header wording (i18n).
- Keys for the tabs, the unread filter and the Huddles view; mouse on tabs and items.
- Paging and refresh policy; what happens while loading or on an API error (say it in the view, never a blank pane).
- Whether the old popups' code is replaced or reused; tests (TestBackend for each view, parsers from recorded *shapes*).
- README sections for the three views and statuses; PROGRESS rows per step.

## Verification the design expects

- Each view checked live in kitty on the test workspace (A reading, B posting/reacting/mentioning, B starting a test
  huddle with synthetic media — never a real mic/camera/screen).
- The live-huddle object, the badge and the huddle-state heartbeat checked with a running test huddle (A's view of B, and
  B's profile while B is in a klack huddle).
- Narrow terminal (~80 columns) and the light theme looked at.
