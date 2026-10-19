# Keymap rework: modes, a leader, which-key — design

Queue **U**. Brainstormed with the user on 2026-10-19. Marks: **(user)** said or chosen by the
user; **(shown)** Claude's proposal the user saw and accepted; **(Claude's call)** decided while
writing this, not yet seen by the user — open to change. Unmarked text describes what exists.

## Why

The user's words:

- "the app occupies a lot of keys on the keyboard and accidental keypress can result in an
  unwanted action"
- "nothing advertises to the user what his keys do"
- "in preferences key actions are kind of just in a one big pile. and it makes it hard to find
  anything in there"
- Borrow from nvim + LazyVim: a leader key and a `which-key`-like panel.

Success: in NORMAL, no single key press can post, delete, change or broadcast anything; every
action is findable from the keyboard (which-key, F1, F6 → Keys) without reading docs; the user
can reshape the whole keymap in one file.

## Goals

1. Two modes, as in nvim: **INSERT** (typing in the message box or the thread reply box) and
   **NORMAL** (everything else on the main screen) **(user)**.
2. In NORMAL only navigation, help, preferences and look-only views are single keys; everything
   that acts goes behind `<leader>` **(user)**.
3. A which-key panel showing the next keys for the focused view and the selected item **(user)**,
   configurable **(user)**.
4. The status line laid out like nvim's, showing the mode and the key sequence under way, and
   shrinking gracefully **(user)**.
5. The keymap is a file the user can rewrite freely: any leader, several sequences per action,
   ungroup actions back onto single keys, own groups **(user)**.
6. F1 help and F6 → Keys follow the same tree instead of one pile **(user, shown)**.
7. Fix the dispatch problems found during the inventory (below) **(user: "we should definitely fix
   the problems you've found")**.

## Non-goals

- The Activity view's own rework — "out of scope for current work" **(user)**.
- New features that have no UI today — mute a conversation, mark unread, next unread
  conversation, copy a conversation's link. They come as a **separate queue after this one**
  **(user)**; this design only keeps sensible free slots for them (`c M`, `c u`, …).
- Vim counts (`5j`), visual mode, marks, a `:` command line (Claude's call).
- Dragging scrollbars and other new mouse features (Claude's call).

## Anti-goals

- A stray key in NORMAL that posts, deletes, reacts, joins a huddle, unmutes or shares a screen.
- Anything bound but undiscoverable (not in which-key, F1 or F6).
- An existing `keys.toml` silently stops doing what its owner wrote.

## What exists today (v0.5.0)

- `crates/klack/src/tui/keymap.rs`: an `actions!` table of every action per context (Global,
  Sidebar, Messages, Composer, Completion, Switcher, Search, Picker, Details, Profile, Activity,
  Threads, Mover, TextField, Snippet, Viewer, Player, Confirm, Huddle, Preferences, Menu, Save,
  Help), each with a name, a description and default keys. `keys.toml` (`config_path()`) overrides
  per `[context] action = "key" | ["k1", "k2"]`; sequences are space-separated keys
  (`"<leader> s"`); a `leader` key is supported but unset by default and no default uses it.
  `lookup(context, pending, key)` returns Action / Pending / None; the event loop keeps
  `app.pending`.
- Global bindings are looked up before the focused pane's (a "shadowed by a [global] binding"
  warning exists for that).
- The status bar (`event_loop.rs` `status_line`, ~3969): a mode pill (COMPOSE, REPLY, EDITING,
  MESSAGES, THREAD, SIDEBAR, HUDDLE, and per dialog SEARCH, JUMP, …), the conversation crumb, the
  info (unread · mentions · other workspaces, or a note, or a ringing huddle), and on the right
  `F1 keys · <pending> … · <availability> · ● live`.
- F1 help lists every key by context; F6 → Keys lists every action in one list, Enter captures a
  new key (leader first for a chord), Delete unbinds, `r` resets, writes `keys.toml`.
- Three right-click menus (message, sidebar conversation, opener list) show each item's key.
- Several actions have **no key**, only a `/` command or a menu item: leave (`/leave`, sidebar
  menu), join (`/join`), create channel (`/create`, `/create-private`), group DM (`/dm`), rename /
  delete a section (`/section`), status (`/status`), away/active (`/away`, `/active`), pause
  notifications (`/dnd`), this conversation's notification level (F6 → Notifications only).

### Problems to fix (found in the inventory)

- **Keys leak under dialogs.** The event loop's arms run in order (event_loop.rs ~2715–3175): F6
  opens over the switcher, mover, save, activity…; the huddle F-keys (F3/F4/F7–F10/Shift+F3) act
  with search, player, snippet, viewer, confirm or picker open, but not with profile or details.
- **Clicks fall through dialogs.** Only the picker, menu and save dialog consume the mouse; with
  any other dialog open a left click selects in the sidebar/messages underneath and a right click
  opens a menu underneath (~3272–3305).
- **The y/n prompts** print a literal "y / n", so they don't follow a rebound `Confirm.yes`.

## The design

### Modes

- **INSERT** = focus in the message box or the thread reply box (today's Composer context). Keys
  type; the composer's editing keys work as today. The status pill reads `INSERT` (green, as the
  typing pill is today) **(user)**; the composer border may take the same colour (shown).
- **NORMAL** = the sidebar, the messages pane, the thread pane, the huddle view. Pill `NORMAL`
  **(user, shown)**.
- Dialogs keep their own pills (SEARCH, JUMP, PREFS, …) and their own keys (see Dialogs).
- `Esc` in INSERT → NORMAL **(user)**: from the message box to the messages pane with the newest
  message selected; from the reply box to the thread pane with the newest reply selected
  **(shown)**. While editing a message, the first `Esc` still cancels the edit (as today) and
  stays in INSERT (Claude's call).
- `i` → INSERT (the box of the pane you're in: messages/sidebar → message box, thread → reply
  box) **(shown)**. `Enter` on a sidebar conversation also goes to INSERT, as today.
- `Esc` in NORMAL: in the thread pane closes the thread (as today); elsewhere it does nothing —
  it no longer jumps into the composer (Claude's call: Esc never enters INSERT).
- An empty composer's `↑` still edits your last message; that is INSERT behaviour, unchanged.

### Direct keys (no leader)

NORMAL, any pane **(user, shown)**:

| Area | Keys |
|---|---|
| Move | `j`/`k`, `↑`/`↓` · `g`/`G`, `Home`/`End` · `PgUp`/`PgDn` · **new** `Ctrl+U`/`Ctrl+D` half a page (sidebar, messages, thread; the huddle view: none) · `Tab`/`Shift+Tab` panes · `Alt+↑`/`↓` previous / next conversation · `Alt+1…9` workspaces |
| Open, type, back | `Enter` open (thread, conversation, unfold a header) · `i` INSERT · `Esc` back (above) |
| Views (look only) | `Ctrl+K` jump · `Ctrl+F` search · `Ctrl+T` threads · **`Ctrl+A` activity** (moved from F5 — **user**: "non-destructive", "immediately accessible just like threads") |
| App | `?` and `F1` help · `F6` preferences · `Ctrl+Q` quit |
| Sidebar | `z` fold / unfold the section (vim's fold key; view only) **(shown)** |
| Huddle view | arrows / `hjkl` tiles, `Enter`/`f` maximize (Space leaves this list: it is the leader), `Esc` all tiles / leave |

INSERT keeps every global key that the composer doesn't use: `Ctrl+K`, `Ctrl+F`, `Ctrl+T`,
`Ctrl+Q`, `F1`, `F6`, `Alt+↑/↓`, `Alt+1…9`, `PgUp/PgDn` (scroll the messages), `Tab`/`Shift+Tab`.
`Ctrl+A` in INSERT stays "line start" **(shown)**: the composer's binding wins (see Precedence).

All other F-keys lose their defaults (F2, F3, Shift+F3, F4, F5, F7, F8, F9, F10, Shift+F10, the
Menu key) **(shown)**; users may bind them again.

### The leader

- `leader` — default **`space`** **(user: "natural default for a <leader> is `space`")**, any key
  allowed **(user)**. It acts in NORMAL panes.
- `insert_leader` — default **`ctrl+space`**, empty for none **(shown)**. It acts where the leader
  can't: in INSERT (where Space types) and in dialogs (where Space often has a meaning). With
  `insert_leader` every `<leader>` sequence available there works with it instead, so
  `Ctrl+Space h m` mutes mid-sentence. If `leader` is a key that never types text (e.g.
  `ctrl+b`), it works in INSERT and dialogs too, and `insert_leader` may be left empty.
- `insert_leader` defaults to `ctrl+space` on **every platform** **(user)**, although macOS (input
  source switching), Windows (IME toggle) and some Linux input methods may take it first; F1 help
  and the README say so and show how to rebind it (F6 → Keys or `keys.toml`). Crossterm reports
  Ctrl+Space as `Char(' ')` + CONTROL (NUL in legacy mode, and with the kitty protocol); check
  it on the Linux/macOS/Windows test machines.

### The leader tree (defaults)

Rule **(shown)**: after the leader, **the selected item's own actions are one key**; everything
else sits in a **group** with a mnemonic letter, the same from every pane.

`␣` = the leader. Group keys: `c` `h` `n` `a` `f` `w`.

**Selected message** (messages or thread pane):

| Key | Action | Key | Action |
|---|---|---|---|
| `r` | react | `o` | open link / picture / video / snippet (a list if several) |
| `e` | edit (own) | `s` | save attachments |
| `d` | delete (own, y/n) | `p` | pin / unpin |
| `y` | copy text | `u` | profile card |
| `Y` | copy link | `t` | open thread (not in the thread pane) |
| `m` | the message's menu | `␣` | jump to (= `Ctrl+K`, LazyVim's `Space Space`) |

**Selected conversation** (sidebar): `s` star · `m` move to section · `d` details · `b`
notification level (bell) · `l` leave (y/n). (`z` fold is direct.)

**Groups:**

| Group | Keys |
|---|---|
| `c` Conversation (the current one) | `d` details · `s` star · `m` move to section · `b` notification level · `l` leave (y/n) · `j` join (while previewing) · `r` rename its section · `x` delete its section (y/n) |
| `h` Huddle | `h` join / leave (answers a ringing invitation) · `d` decline · `m` mute · `c` camera · `s` share screen · `v` view · `t` the huddle's thread · `T` huddle in the selected message's thread · `p` huddle with this person (on a profile card) |
| `n` New | `c` channel · `p` private channel · `d` DM / group DM · `s` snippet · `f` upload a file **(user: `n f`)** |
| `a` Availability | `s` set status · `c` clear status · `a` away / active · `p` pause notifications |
| `f` Find | `f` jump · `#` browse channels · `s` search · `t` threads · `a` activity |
| `w` Workspace | `1`…`9` |

- `n …`, `a s` and `a p` put the matching command into the INSERT composer with the cursor after
  it (`/create `, `/create-private `, `/dm @`, `/snippet `, `/upload `, `/status `, `/dnd `) and
  enter INSERT **(shown)**; Enter then runs it as today. They always use the conversation's
  message box, also from the thread pane. A draft is never lost **(user: "some of them could
  append to draft")**: `n f` (`/upload <path> [text]`) and `n s` (`/snippet` + the lines below)
  put the command in front of the draft, so the draft becomes the upload's message or the
  snippet's body, the cursor after the command; the others refuse with a status note while the
  box holds text. While a message edit is in progress all of them refuse (Claude's call on
  which ones wrap). `a c` and `a a` act at once (`/status
  clear`, away ↔ active). `c b` cycles the level like F6 → Notifications does today, with a status
  note naming the new level (Claude's call). `c r` prefills `/section rename `.
- Actions that don't apply are left out of which-key (edit / delete on others' messages, open
  without a link, save without files, join when already a member, leave in a DM, `t` in the
  thread pane, `h p` off a profile card…) — the same `Offers` filter the message menu uses
  (Claude's call). Pressing such a key anyway does nothing but a status note.
- Free slots kept for the later queue: `c M` mute, `c u` mark unread (the user's follow-up).

### Precedence and dispatch (fixes the leaks)

- Lookup order: **the open dialog → the focused pane → global**, for single keys and for
  sequences alike (Claude's call; today global wins). A sequence is Pending if it is a prefix in
  any active context; which-key shows the union, the more specific context winning a clash.
- **While a dialog is open** only its own keys act, plus `Ctrl+Q`, plus `insert_leader` sequences
  of the global groups (so `Ctrl+Space h m` mutes with search open) (Claude's call). F6, F1, the
  views (`Ctrl+K/F/T/A`) do not open over a dialog any more.
- **Mouse in dialogs:** a dialog consumes every click and the wheel; a click outside it closes it,
  as the menu, picker and save dialog already do (Claude's call). Nothing under a dialog reacts.
- The y/n prompts are written from `Confirm.yes`'s keys (any other key = no, as today).
- The key-shadowing warning turns around: report a `[global]` sequence that a pane or dialog
  binding hides (informational; defaults may do it on purpose, e.g. `Ctrl+A` in INSERT, so those
  are not reported) (Claude's call).

### Dialogs

- Dialogs keep their short single keys (preferences, save, mover, menu, switcher, search, picker,
  viewer, player, snippet, details, activity, threads, help, confirm) **(shown)**.
- Exceptions that reach other people move behind the leader (which in a dialog means
  `insert_leader`): on the profile card, "start a huddle" → `h p` in the Huddle group, VIP → `v`
  at the top level **(shown, adjusted by Claude to avoid the `h` group clash)**. "Message them"
  stays on `Enter`.
- Right-click menus stay as they are; their key column shows the new sequences (`␣r`) (Claude's
  call).

### which-key panel

- Opens when a sequence is incomplete **0.4 s** after the last key **(user)**; once open, each
  further key updates it at once (shown).
- Sits in the **bottom-right** corner by default **(user)**, above the status line; corner
  configurable **(user)**.
- Can be turned off **(user)**; the status line still shows the sequence.
- **Max height**, then **pages** **(user)**: default half the screen height (shown); it first adds
  columns, then pages; `Ctrl+D` / `Ctrl+U` turn pages while a sequence is pending (shown); the
  footer reads `1/3 · Ctrl+D more`.
- Layout (shown, v3 mockup): a title `␣h  huddle`; entries in aligned columns of `key → icon
  label`; groups in a different colour with a `+` prefix (`+huddle`); footer `Esc close ·
  Backspace back`.
- `Esc` cancels the sequence, `Backspace` goes back one key; there is **no timeout** — a pending
  sequence waits (shown). A key that continues nothing cancels and the status line says "␣x isn't
  bound here" (as today's "X is not bound here").
- **Icons** **(user: liked LazyVim's)**: each action and group has a label and an icon, both
  configurable, defaulting to emoji. Icons draw in a fixed 2-cell column; defaults must be emoji
  that are wide by default (Emoji_Presentation, no VS16 — e.g. `📝` rather than `✏️`) so terminals
  agree on the width (Claude's call). An on/off setting for icons (shown).
- Labels and group names are translatable through the locale file, as hints are today
  (Claude's call).

Settings (in `settings.toml`, shown in F6 → Keys above the bindings) **(user: off, corner,
delay; shown: height, icons)**: show which-key (on/off) · corner (4) · delay (seconds, 0 = at
once) · max height (% of the screen) · icons (on/off).

### Status line

The mockups `which-key-v3` and `status-widths` were agreed **(user)**:

- One row at the bottom, the full width of the screen (as today).
- **Left:** the MODE pill at the left edge; then the info: unread · mentions · other workspaces ·
  your availability · `● live` (the latter two move here from the right) **(shown)**. A note or a
  ringing huddle (`📞 X invites you · ␣hh join · ␣hd decline`) takes the info's place **(shown)**.
- No conversation crumb **(user: "we dont need that")**.
- **Right:** the pending key sequence (as nvim's showcmd, e.g. `␣h`, `^␣h` for Ctrl+Space), then
  the hints flush with the right edge **(user)**; hints for NORMAL e.g. `j/k move · Enter thread
  · i type · ␣ actions · ? help`.
- **Shrinking order** **(user: "that order works")**: (1) info goes short: `3 unread · 1 mention
  · ● live` → `3 · @1 ●`; (2) hints drop one at a time, least useful first, `? help` last;
  (3) MODE becomes one letter (`N`, `I`, dialogs their first letter); (4) hints end at `?`, info
  at `●`. The pending sequence is never cut. A note or a ringing huddle wins over the hints; still
  too long → cut with `…`.
- `F1 keys ·` leaves the right side (the hints end in `? help`).

### The keymap file (`keys.toml`)

**(shown)** — extends today's format, which keeps working:

```toml
leader = "space"
insert_leader = "ctrl+space"

[groups]                                   # names (and icons) which-key, F1 and F6 show
"<leader> h" = { name = "huddle", icon = "🎧" }
"<leader> c" = "conversation"              # a plain string = name only

[global]
activity = "ctrl+a"
mute = ["<leader> h m", "f4"]              # several sequences for one action

[messages]
react = "r"                                # ungrouped: back on a single key in NORMAL

[labels.messages]                          # which-key label / icon per action
react = { label = "react", icon = "😀" }
```

- **Bindings decide the tree**, not the action: a group is any prefix shared by bound sequences;
  moving an action between groups or out of one is rebinding it. `[groups]` only names prefixes;
  an unnamed prefix shows as `+…` with its key (Claude's call).
- `= []` unbinds. Several sequences = a list. Notation stays today's: space-separated keys,
  `<leader>` for the leader (Claude's call: no vim `<C-a>` notation; compatible with existing
  files).
- **Any action can go back on a single key in NORMAL** **(user: "ungroup them and put back on
  single keys in normal mode … we should allow him to do it")**, `[global]` ones included: a
  printable single key bound in `[global]` acts only in NORMAL panes (never in INSERT or a
  dialog, where it types or belongs to the dialog). In `[composer]` a printable single key is
  still refused (Claude's call on the mechanism).
- **The user's binding wins over a default** (anti-goal above): when a user sequence collides with
  a default one — the same sequence, or one that is a prefix of the other (e.g. a user's
  `huddle = "<leader> h"` against the default Huddle group under `<leader> h`) — the default ones
  it blocks are dropped with a warning naming them; the user's binding acts. Two user sequences
  colliding in one context: the first in table order keeps it, with a warning, as today.
- A sequence that is both an action and a prefix of another in the same context could never
  complete; this is resolved by the rule above and never left in the map.
- **`klack ctl keys-template`** prints the whole default keymap — grouped, commented, every action
  with its description — the way `theme-template` works **(shown)**. A `keys.toml` may be that
  whole file or only the lines that differ.
- **Existing files:** action names stay; the actions in the table keep their contexts where
  possible, so `[messages] react = "r"` or `[global] huddle = "f3"` still mean what they meant —
  they now override a `<leader>` default **(shown)**. The habit change is intended: single letters
  and F-keys stop working unless rebound **(shown)**; the release notes say so and point to
  `keys-template`.

New actions the table needs (names are the builder's): half page up / down (sidebar, messages,
thread); in Global the Conversation, New and Availability actions that have only commands today
(star / move / details / notification level / leave / join / rename section / delete section;
new channel / private channel / DM / snippet / upload; set status / clear status / away-active /
pause notifications); huddle with a person in Profile; activity on `ctrl+a`. Every action also
gets a short which-key label and a default icon in the table.

### F1 help and F6 → Keys

- Both follow the tree **(shown)**: headings "Direct keys", then the leader's top level for each
  pane, then each group (`␣c Conversation`, `␣h Huddle`, …), then each dialog's keys.
- **F6 keeps a hand-written `keys.toml` intact** **(user)**: a change in F6 edits only the
  entries it touches, keeping comments, order and layout (a format-preserving TOML editor such as
  `toml_edit`, already in `Cargo.lock` through `toml`; today `Keymap::to_toml` regenerates the
  file and must be replaced). A file that doesn't exist yet is created with only the changed
  entries.
- F6 → Keys has a filter to type into (shown), shows the which-key settings above the bindings,
  lets the user rename a group (name and icon) and record a whole sequence (shown). How a
  recording ends is the builder's call (e.g. a pause of about a second, shown live), as long as
  any key, Enter and Esc included, can be part of a sequence.

## Constraints

- Rust, ratatui 0.30, crossterm 0.29; Linux, macOS and Windows builds stay green.
- The user's standing rules: small verified steps (build without warnings, tests green, a live
  check in the kitty test window, a PROGRESS row, a commit, push, CI); no `cargo fmt`; the user's
  own `~/.config/klack` (incl. `keys.toml`, `settings.toml`) is backed up and restored around
  tests, never deleted.
- Everything configurable stays in seconds where it is time (the user's rule from earlier queues).

## Left to the builder

- Internal structure: how contexts merge for lookup, where which-key lives (a module like the
  others), how the event loop's dialog arms are reorganised.
- Names of new actions and settings; exact default icons (within the width rule); column order in
  which-key; how F6 records a sequence.
- The step order of the plan, as long as each step leaves the app usable (e.g. which-key and the
  status line before moving keys behind the leader is fine).
