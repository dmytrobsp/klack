# Links into your Slack open in klack — design (queue Z)

Status: talked through with the user on 2026-10-20 and approved by them ("write it up"). "(user)" marks their
decisions, "(Claude's call)" marks choices proposed to them that they did not change, "(builder's call)" is left to
the implementer.

## Why

The user clicked a link to a specific reply in a thread of a channel in their work workspace. kitty opened it in the
browser. They expected klack to "open a specific channel, focus a specific message that started a thread i'm
interested in, open its thread view and open a specific message inside this thread the link was pointing to" (user).

## What counts as an in-app link

A link in a message whose target is in a workspace this profile has open (signed in, any of its accounts):

- a channel or DM: `https://<ws>.slack.com/archives/<C|D|G…>`;
- a message: `…/archives/<channel>/p<ts without the dot>` (`p1729440000123456` → `1729440000.123456`);
- a thread reply: the same with `?thread_ts=<parent ts>&cid=<channel>`;
- the desktop app's form: `https://app.slack.com/client/<team id>/<channel>[/thread/<channel>-<parent ts>]`;
- a person: `…/team/<user id>`, which opens the profile card klack already has;
- a file: `…/files/<user>/<file id>/…`, which opens klack's file viewer or the open menu's picture or snippet view
  where one exists (builder's call which view).

The workspace is matched by host (`<ws>.slack.com`) against the profile's workspaces' URLs, or by team id (`T…`) for
`app.slack.com/client/…`. Enterprise Grid (one org, many workspace hosts, `E…` ids, `*.enterprise.slack.com`) is not
handled: the user's two work workspaces are separate organisations with plain `<name>.slack.com` hosts (checked
2026-10-20 in their profile, addresses only), so exact matching covers them (Claude's call, after asking the user). Anything else (another workspace, Slack's help pages,
any other site) stays an external link and behaves exactly as today (Claude's call).

## How it looks (user: "we should clearly mark the link so the user knows its an in-app link")

- `↪` and a **readable label** instead of the URL, in klack's accent colour (user chose this over keeping the raw URL):
  - channel: `↪ #deploys`;
  - message: `↪ #deploys › message from Bob` (`DM with Bob › message`, `group DM › message`);
  - reply: `↪ #backend › thread reply` (or `› reply from Bob`);
  - person: `↪ Bob's profile`; file: `↪ file report.pdf`;
  - another workspace of the profile: `↪ acme · #deploys › message`.
- **Where the author comes from (Claude's call, after the builder check):** Slack's link preview attached to the
  message (the quoted message below a message link) when there is one; otherwise klack fetches the linked message once
  (`conversations.history` / `conversations.replies` with that ts, `inclusive`, `limit=1`) when the link first comes
  on screen, cached per link, a few at a time; the label updates when the answer arrives.
- **Another workspace of the profile (Claude's call):** no second connection just for a label — `↪ acme · #deploys ›
  message` with the channel's name only when klack already knows it (else `↪ acme · a channel › message`); the click
  switches and shows the rest.
- **Unknown details fall back, never guess:** if the message isn't loaded, the label is `#channel › message`. If the
  channel isn't known (not a member, not listed), it is `#channel` from what Slack's name lookup gives, or `↪ a
  channel` (builder's call).
- **A link the author labelled** (`<url|the fix>`): their text is kept (user asked; Claude's answer: "keep their
  words"), shown as `↪ the fix` with the same arrow, colour and click.
- External links look as they do now (link colour, underline).
- The real URL stays the cell's OSC 8 hyperlink under the label, so the terminal's own link handling still works.

## How it opens

- **Plain left click** on the link, with no modifiers (user), opens it in klack. A click elsewhere on the message does
  what it does today (Claude's call). kitty, foot and WezTerm pass an unmodified click to an app that captures the
  mouse; Shift-clicks stay with the terminal.
- **Ctrl+Shift+click** (kitty's link click) still opens the URL in the browser (user asked "is it possible to still
  preserve the ctrl+shift+click behaviour" → yes, through the kept OSC 8 hyperlink).
- **The `o` (open) action** (user: "it should also work with an 'open' action"): as today, a message with a single
  thing opens it at once — a single in-app link opens in klack (Claude's call); with several: the menu groups the links. In-app
  ones come first under "In klack", marked `↪`, then the rest under "In the browser" (user: "it should also be marked
  in 'open' menu differently"). A labelled in-app link shows both: `↪ the fix — #backend › thread reply`. Choosing an
  in-app one opens it in klack.
- **Hover:** with the mouse over any link, the status bar says where it goes: `↪ opens #backend › thread reply in
  klack`, or the URL for an external one (Claude's call).

Opening a target, reusing the search result path (`open_hit` / `show_hit` / `continue_seek` in event_loop.rs):

1. Another workspace of the profile → switch to it first (Alt+number's path), then continue there.
2. The conversation: open it as a sidebar pick does (lands in NORMAL, X1). For a channel the user is not in, open the
   preview that Jump to uses.
3. A message: select it, paging history back until it is loaded (`continue_seek`).
4. A thread reply: select the parent, open the thread, select the reply.
5. Not found (deleted, no access, beyond what can be paged): stay in the conversation and say so in the status bar
   ("that message isn't there any more, or you can't see it"). Never open the browser by surprise.
6. A person → the profile card. A file → its viewer where klack has one; otherwise the open menu's file entry.

## Constraints

- Nothing new on the system: no URL-scheme claims, no kitty config changes. Option 2 (kitty open-actions → `klack open`)
  was offered and not chosen (user chose option 1).
- No tokens or cookies anywhere. Labels use only what klack already has, or Slack's usual lookups (conversations.info,
  users.info) through the existing client.
- Works in any terminal that passes plain clicks (the in-app part); without OSC 8 support the label and the click
  still work, and the browser is reached by copying the link (no extra "open in browser" menu entry — Claude's call:
  Ctrl+Shift+click covers it where the terminal has links).
- Linux, macOS, Windows as today.

## Non-goals

- Enterprise Grid org-wide matching (see above).
- Opening links that point outside the profile's workspaces in klack (other workspaces, Slack help, canvases, lists,
  huddle links): they stay external. Canvases, lists and huddle links are possible later.
- Rewriting links in messages the user sends.

## Tests

- Unit: permalink parsing (all forms above, `p`-ts conversion, `thread_ts`, `cid`, app.slack.com, Enterprise hosts,
  non-matching hosts), workspace matching, labels with and without loaded data and with author labels, the open menu's
  grouping, click hit-testing on a link's cells (as `links_are_found_where_they_are_drawn` does), the hover note.
- Live (test workspace, A and B): B posts links to a channel, a message, a thread reply and A's profile in the A–B DM.
  A clicks each and lands right, including a reply far back in history. Ctrl+Shift+click opens the browser: check
  that kitty would open it (with a throwaway `open_url_with` pointing at a script that logs the URL, never a real
  browser). The `o` menu shows the groups. Delete the test posts afterwards.

## Left to the builder

- Where the parsed target lives (on the span, or found again from the URL at click time) and how labels refresh when
  data arrives later.
- The file view used for file links.
- Exact wording of the not-found and hover notes beyond the examples.
