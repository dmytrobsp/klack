# Keymap rework: modes, a leader, which-key — design (DRAFT, brainstorm in progress)

Status: being brainstormed with the user (2026-10-19). Parts marked **decided** were said or
confirmed by the user; **guess** marks Claude's proposals the user has seen but not ruled on;
**open** is still to be asked. Nothing is implemented yet. Next: finish the open questions, then
write the full design here, run the builder check, get the user's approval, then a plan.

## Why (the user's words)

- "the app occupies a lot of keys on the keyboard and accidental keypress can result in an
  unwanted action"
- "nothing advertises to the user what his keys do"
- "in preferences key actions are kind of just in a one big pile. and it makes it hard to find
  anything in there"
- Borrow from nvim + LazyVim: a leader key, and a `which-key`-like contextual help panel.

## What exists today (as of v0.5.0)

- `keymap.rs`: every action in an `actions!` table per context (global, sidebar, messages,
  composer, dialogs…), `keys.toml` overrides, key sequences and a `leader` setting already
  supported (`"<leader> x"`) but no default uses them.
- Risky single keys: in Messages `r` react, `e` edit, `d` delete (asks y/n), `P` pin, `H` huddle in
  thread, `y`/`Y` copy, `s` save, `o` open, `u` profile, `m` menu; in the sidebar `*` star, `m`
  move to section, Space/`z` fold; global F-keys that work even while typing: F3 join/leave
  huddle, F4 mute, F7 huddle thread, F8 huddle view, F9 camera, F10 screen share, Shift+F3
  decline, F2 details, F5 activity.
- The status bar already shows a mode pill (COMPOSE, REPLY, MESSAGES, SIDEBAR, dialogs) and a
  few hints from the bound keys; F1 lists every key by context; F6 → Keys lists every binding
  in one long list.

## Decided

1. **Two modes, as in nvim.** Typing in the message box or the thread reply box is **INSERT**,
   shown clearly (the status bar's mode label reads `INSERT`); keys type there (Enter sends, the
   composer's editing keys stay). **Esc leaves INSERT for NORMAL**; the user knows to press Esc
   first, as in nvim.
2. **In NORMAL only these are on single keys**: navigation, help, preferences, and y/n in modal
   prompts. Navigation is the full set, including PgUp/PgDn, Home/End, g/G, and nvim's
   Ctrl+U / Ctrl+D (half a page) — "we have more of them … we could also add some from
   usual/popular nvim keymaps".
3. **Everything else, potentially destructive, goes behind a `<leader>` keypress.**
4. **A which-key panel**: after the leader it shows the next keys for the focused view and the
   selected item (if one is selected).
5. **Preferences → Keys grouped**, not one pile (by the same tree which-key shows — guess on the
   exact grouping, see below).

## Guesses shown to the user (not objected to: "yes, looks right")

- Esc from INSERT lands on the messages pane with the newest message selected (today it goes to
  the sidebar); `i` goes back to INSERT. The composer border may change colour with the mode.
- Ctrl+K (jump), Ctrl+F (search), Ctrl+T (threads), Alt+↑/↓ (conversations), Tab / Shift+Tab
  (panes), Enter (open), `?` / F1 (help), F6 (preferences), Ctrl+Q (quit) stay direct.
- The leader is **Space** in NORMAL (it types nothing there).
- Moved behind the leader: react, edit, delete, pin, copy text / link, save, open, profile,
  huddle (join/leave, mute, camera, screen share, view, thread, decline), star, move to section,
  fold, details, activity.
- which-key appears shortly after the leader (LazyVim waits a moment), grouped; group keys
  (e.g. `m` for message) show that group's keys; the same tree can feed a quieter "what can I do
  here" hint so nothing is invisible.
- F6 → Keys follows that tree (groups with headings), with a filter to type into.
- Existing users' habits change on purpose (single letters and F-keys stop working directly).

## Open (ask next, one at a time)

- **The leader tree**: which actions should be one key after the leader, which can sit a level
  deeper in a group (Message, Conversation, Huddle, Go to, …)? (Asked; the user moved to the
  handoff before answering.)
- which-key timing (immediately, or after a short delay) and where it sits on screen.
- Is there also an always-visible hint (status bar) of the leader groups, or only after Space?
- Does the sidebar get NORMAL-mode keys of its own (fold, star, move) under the same leader?
- The leader inside dialogs and lists (Ctrl+T, F5, the image viewer…): their own keys stay as
  they are, or also go under a leader?
- What happens to a user's existing `keys.toml` (old action names, F-key overrides)?
- The mouse: right-click menus stay as they are (they show the actions and their keys)?
