# klack

**[dmytrobsp.github.io/klack](https://dmytrobsp.github.io/klack/)** — what it does, the numbers against the official
app, and the downloads.

A Slack client for the terminal, written in Rust (ratatui + crossterm). Messaging with
threads, reactions, autocomplete and formatting; inline images, an image viewer and video with
sound; desktop notifications; search; and **huddles with audio**.

klack was called slacktui until October 2026; on first start it moves that name's folders
(`~/.config/slacktui`, `~/.cache/slacktui`, …) to klack's.

It talks to Slack the way the web client does: a browser session (`xoxc-…` token plus the `d`
cookie). It is not an official Slack app.

## Install

Ready-made builds are on the [releases page](https://github.com/dmytrobsp/klack/releases): each is one zip with
`klack` (`klack.exe`), the README and the licences at its top. Unzip it and put `klack` somewhere on your PATH. None of
them is signed.

| Download | Runs on | Needs |
|---|---|---|
| `x86_64-linux.zip` | glibc 2.34 or newer (Ubuntu 22.04, Debian 12, Fedora 35, RHEL 9 and later) | `libasound.so.2` (ALSA), `libstdc++` — on every desktop Linux |
| `aarch64-macos.zip` | Apple Silicon Macs, macOS 11 or newer | nothing (system frameworks only); unsigned: `xattr -d com.apple.quarantine klack` once, or allow it in System Settings → Privacy & Security |
| `x86_64-macos.zip` | Intel Macs, macOS 11 or newer | as above |
| `x86_64-windows.zip` | 64-bit Windows 10 / 11 | nothing (Windows system DLLs only, no MinGW runtime); unsigned, so SmartScreen may ask once |

Optional everywhere: `ffmpeg` / `ffprobe` on PATH for video, cameras and screen sharing; a terminal
with graphics for real pictures (see below). To build it yourself instead, read on.

## Requirements

To build: a recent stable Rust toolchain (edition 2024; [rustup](https://rustup.rs)), a C/C++
compiler, `pkg-config`, **cmake** (the bundled Opus codec is built with it) and the **ALSA headers**
on Linux. On a fresh system that is (each line checked with a clean checkout in a container,
`tools/container-build.sh`):

| System | Packages |
|---|---|
| Debian 13 / Ubuntu 24.04 | `apt install build-essential pkg-config cmake libasound2-dev git curl` |
| Fedora | `dnf install gcc gcc-c++ make pkgconf-pkg-config cmake alsa-lib-devel git` |
| Arch | `pacman -S base-devel cmake alsa-lib git` |
| macOS | Xcode command-line tools, then `brew install cmake` |

To run: `libasound.so.2` (ALSA's library, on every desktop Linux) and the C++ runtime. Optional:
**ffmpeg / ffprobe on PATH** for video playback, cameras and (outside Wayland) screen sharing; a
D-Bus notification service for desktop notifications; a terminal with graphics for real images
(kitty, WezTerm, Ghostty, iTerm2, foot or any sixel terminal; others get half-block images). Without a
sound card huddles still work (silently, with a note in the call bar); without system CA
certificates klack uses the Mozilla roots it carries.

```sh
cargo build --release
./target/release/klack    # the first time: a short guide signs you in, then klack opens
```

Windows from Linux: `rustup target add x86_64-pc-windows-gnu`, `apt install mingw-w64`, then
`cargo build --release --target x86_64-pc-windows-gnu`. The result is one `klack.exe` that needs no
MinGW DLLs (`crates/klack/build.rs` links OpenH264's C++ runtime statically, `.cargo/config.toml` the C
runtime).

## The screen

The sidebar on the left (the Threads, Activity and Huddles rows, sections, starred conversations, channels, direct messages — the most
recently active first; channels and DMs without a message for 30 days are left out unless starred, unread
or open, F6 → Appearance; Ctrl+K still finds them and shows them while open; what you have not read is
bold with a bar in the first column, muted conversations are dimmed and not counted), the
messages in the middle with a reply pane for threads on the right (in a narrow terminal the thread
takes the messages' place until you close it), the composer below, and a status
line at the bottom: on the left a pill saying which mode you are in (NORMAL, INSERT, or the open
dialog: JUMP, SEARCH, PREFS, …; THREADS, ACTIVITY or HUDDLES in those views), then the unread and mention counts, other workspaces, your
status, and a dot for your presence as in Slack (green `● active`, dim `○ away`; "connecting…" after it while
the connection is down, a dim `…` when the line is at its narrowest) — a note or a ringing huddle takes their place; on the right
the keys of a sequence under way (`␣h`, `^␣h` for Ctrl+Space) and the hints of where you are, ending
in `? help` (`↑/↓ move · Enter thread · i type · ␣ actions · ? help`). A thumb on a pane's right border shows where you are when it scrolls. The selected message is highlighted
across the whole width of its pane, every row of it, like a selection in a list (so is the selected message of the Threads feed and the selected item of the Activity view). A conversation
opened with unread messages has a red "new" line above the first of them; while you read further
up, the bottom border counts the newer messages below ("↓ 3 newer"). With two or more
workspaces in the profile a rail on the far left has a tile for each — its logo where it has
one and the terminal draws pictures, else its initials and number in a colour of its own — with
the shown one marked by an accent bar and the others with unread messages by a thinner, dimmer
one, and below such a tile `@3` for three unread DMs or mentions there; a click on a tile switches, like Alt+1…9.

**Statuses.** A person's status emoji stands right after their name — in the sidebar's direct messages (a
long name is shortened to keep it; group DMs show none, neither on their rows nor in a hover note; a
custom emoji, or any with emoji shown as names, is a dim ◆), and on their messages (a custom emoji as its picture where the terminal draws pictures) — and a DM's title adds the
text ("Name — 🌴 Vacationing"); hovering their DM in the sidebar shows it in the status line. Someone in a
huddle without a status emoji of their own shows 🎧 ("In a huddle"; not on a sidebar row that shows the
huddle already). An expired status is gone at once, without waiting for Slack. klack connects to Slack's
own websocket gateway, as Slack's web client does, and asks it for the profile changes of the people it
shows: when someone's status or huddle state changes, Slack says so within a second or two and klack looks
that one person up. As a safety net — and on the older `rtm.connect` connection klack falls back to when
Slack offers no gateway or it fails twice in a row (refused, or closed before Slack's hello; a network drop
does not count; the gateway is tried again ten minutes later), which gets
no such pushes — the people shown are also looked up again every ten
minutes and once after the connection was lost (when it was down for 30 s or more, or the last such look-up
was five minutes ago or more, so a flapping link does not bring the bursts back), up to 50 at a time: first the authors in the open
conversation and thread, then the people of the DMs and group DMs on screen in the sidebar, then the
sidebar's other DMs and group DMs in turn, so that everyone's comes round: F6 → Messages & media → Refresh
people's statuses (`[messages] status_refresh_secs`, default 600, 0 = off).

## Signing in

`klack auth login` (and plain `klack` on a profile with no sign-ins yet) guides you, right in the
terminal: it looks for Slack in your browsers and lists every workspace each browser profile is signed
in to, grouped by where it comes from (`Chrome · Work  you@example.com`, `Firefox · default-release`),
with who you are there, and then the three ways to sign in afresh, for any workspace. ↑↓ or j k move,
Space marks several, Enter takes the marked ones (or the one under the cursor), `/` filters, a click marks
a row, Esc in the list leaves without saving anything. Workspaces a browser is signed out of are shown dim
with the reason; ones already in the profile say `already signed in` (picking one refreshes it). The marked
browser sign-ins are saved first, then each marked way runs; Esc in a way goes back to the list. Nothing is
signed out anywhere, and klack never opens a browser itself.

The three ways:

- **With a code from your email:** klack shows a QR code and a link (`app.slack.com/get-started?email=…`,
  your address filled in) for your phone or any browser. There Slack sends you a code (this step needs
  Slack's captcha, so only a real browser can do it). Type the code into the `___-___` prompt; klack confirms
  it with Slack itself and lists the workspaces of that address. Workspaces that need two-factor or SSO are
  shown dim ("needs your browser (two-factor)") — use the browser way for those. In a small terminal
  (80 × 24) the code is drawn smaller; in a smaller one only the link is shown.
  This way works over SSH and on machines with no browser.
  `klack auth login --email you@example.com` goes straight to it with the address filled in; without a
  terminal (a script) it prints the link, reads the code from standard input and signs in to every listed
  workspace that can be used (failing when none was).
- **In your browser:** klack prints a link; open it in whichever browser and profile you like and sign in.
  klack watches the browser profiles (read only) and, when one gets a new Slack session, offers it. A
  browser klack can't read is named while it waits: on Windows Chrome (app-bound encryption), elsewhere a
  Chrome-family browser whose key the keyring or the Keychain did not give. When none can be read, the row
  is dim and says why, pointing to Firefox or the code.
  It needs a browser on the machine klack runs on; over SSH, use the code way instead, or reach that
  machine's browser through a remote desktop (such as RDP) while klack runs in the SSH session.
- **Paste a cookie:** the `d` cookie by hand.

Each workspace ends in one line: `✓ Signed in to Acme as you — profile default saved.`

**One workspace:** `klack auth login --workspace acme.slack.com` in a terminal lists the browsers signed in
to it, then a code from your email, your browser and a pasted cookie (that workspace is marked in each
list). There is no browser window and no `--no-browser`.

**From the browser you already use:** `klack auth login --workspace acme.slack.com --from-browser firefox`
(or `chrome`) takes the session from a browser you are signed in to Slack with. Its cookie database is
copied and read, never changed; Chrome's (and Chromium's, Edge's, Brave's) cookies are decrypted with its
key from the system keyring (Linux) or the Keychain (macOS, which asks once per browser, one after another —
the guide too). On macOS answer the Keychain's question with “Allow”, not “Always Allow” (that would let any
program read the key through the `security` tool without asking). `--browser-profile DIR` picks one profile
(or a browser's folder of profiles); otherwise every profile is tried. When several sign in to the
workspace as different people, klack lists them (the browser's profile name and the account) and asks
which; without a terminal to ask in it takes the one used most recently. Chrome on Windows cannot be read
this way (app-bound encryption); Firefox can.

**For scripts** (nothing is asked without a terminal), by hand: `klack auth login --workspace
acme.slack.com` reads the value of the `d` cookie for `slack.com` from standard input (sign in in any
browser, then devtools → Application → Cookies); `KLACK_COOKIE_D` or `--cookie` give it directly. Either
way klack fetches the token itself and checks it with `auth.test`. `--from-browser` and `--email` work
without a terminal too, as said above.

Sessions are saved per profile in `~/.config/klack/<profile>/session.json` (owner-only
permissions), each with where it came from (which browser profile, an email code, or a cookie — names
only, nothing secret). One profile may hold two accounts of the same workspace.

- `klack auth status` — each workspace of the profile, who you are there, where the session came from
  and whether it still works (checked with Slack); ★ marks the one plain `klack` opens.
- `klack auth logout` — in a terminal lists the profile's sign-ins (Space marks several, Enter forgets
  the marked ones, Esc changes nothing); `--team NAME` (or `--all`) for scripts. It forgets the sign-in
  on this machine only; Slack is not signed out (a session taken from a browser stays that browser's). The profile stays, empty if
  it was the last.
- `klack profile list` — every profile, ★ on the default, with its workspaces (local files only); a
  folder with sign-ins whose name can't be a profile's is named (`skipped NAME: why`).
- `klack profile default NAME` — plain `klack` (and `auth`, `ctl`) use that profile from now on
  (kept in `~/.config/klack/default-profile`); `--profile NAME` always wins.
- `klack profile remove NAME` — lists and, after a y/N question (or `--yes`), deletes the profile's
  `~/.config/klack/NAME/` (sign-ins) and `~/.cache/klack/NAME/` (log, caches); drafts and the
  sidebar layout stay. Refused while a klack runs on that profile.
- `klack purge` — deletes everything klack wrote, as if it was never launched: `~/.config/klack/`
  (every profile's sign-ins, `settings.toml`, `keys.toml`, `locales/`, `themes/`), `~/.cache/klack/` (logs,
  caches, `login-browser/`), `~/.local/share/klack/` (drafts, sidebar layout), old `slacktui` folders and
  klack's own `klack-cookies-*` / `klack-<pid>-*` files in the temporary folder (yours only). Lists them first and asks
  y/N (or `--yes`); refused while a klack runs. A folder that is a symbolic link loses only the link (what it
  points to stays, and klack says so). Files you saved from messages stay; Slack is not signed out.

Use `--profile NAME` to keep several accounts apart (letters, digits, `-` and `_`, and `.` or `@` inside
the name: `john.doe`, `me@work`); signing in under a
new name makes that profile. Profiles separate accounts only:
the preferences (`settings.toml`, everything in F6), `keys.toml`, `themes/` and `locales/` are shared
by all profiles; each profile has its own sessions and its own log/cache folder. To tell workspaces
apart, give each a theme of its own (F6 → Appearance → Theme for this workspace). Signing in to more workspaces
with the same profile adds them to it: inside the app, `Alt+1 … Alt+9` switches between them
and `/workspace [name]` lists them or switches by name. The rail on the left shows
unread messages in the others (checked every 30 s), and their desktop notifications still arrive (titled with the workspace); clicking one switches there.

## Keys

Press **F1** (or `?` outside the composer) in the app for the full list, with the keys in effect.
klack has modes, like vim. In **NORMAL** (the sidebar, the messages, a thread) only moving around, help,
preferences and Y/N questions sit on a key of their own; everything else — anything that changes
something — is behind the **leader**, `Space`. **INSERT** is typing in the composer or the reply box:
`i` (or Enter in the sidebar) starts it, Esc leaves it, and the status bar pill says which mode you
are in. In **DIALOG** (jump to, search, preferences, menus, pickers, …) the dialog's keys are shown
at the bottom. While you type, or in a dialog, Space is a space, so the leader there is
`Ctrl+Space`. klack starts in NORMAL on the messages of the conversation you were last in, and
opens the thread again if you quit with one open.

Press the leader and wait (0.4 s) and the **which-key** panel opens in a corner with what each next
key does; a wrong key or Esc cancels. It follows where you are: on a message it lists the message's
actions, on a conversation the conversation's. Its corner, delay, height and icons are in F6 → Keys.

| Key | |
|---|---|
| ↑ ↓ / `k` `j`, Home End / `g` `G`, PgUp PgDn, Ctrl+U / Ctrl+D | move, first / last, a page, half a page — in the conversation, the sidebar, the Threads, Activity and Huddles views, and the Keys page (a snippet: lines, start / end, a page, half a page) |
| Tab / Shift+Tab | move between sidebar, messages, composer, thread, reply box |
| Alt+↑ / Alt+↓ | previous / next conversation |
| Alt+Shift+↑ / Alt+Shift+↓ | previous / next unread conversation (muted ones are skipped) |
| Ctrl+O · Alt+← / Alt+→ | back / forward, as in a browser: to the conversation (its thread open again if it was), the Threads, Activity (on its tab) or Huddles view you came from, at the message you were on (the reply selected in the open thread, the item selected in the Threads or Activity view, their next pages loaded to find it, up to five, until you do something there or leave; older history loaded until it is there, as for a link; a message deleted meanwhile: the one before it, said so; a selection you had left behind — sending a message takes you to the newest — or the newest message itself: at the newest, following new ones), however you went there (the sidebar, Ctrl+K, a link, Enter in a view, a notification; moving through the sidebar — ↑/↓, the wheel over it, Alt+↑/↓ — is one step however far, back to where you were before it) — the last 20 places, per workspace (kept while you are in another); going somewhere new after going back drops the places ahead; a channel left or archived meanwhile is skipped; only in NORMAL, never while you type. The mouse's side buttons do the same (button 8 back, 9 forward; in a terminal that reports them; klack reads them through a patched `crossterm`, `vendor/crossterm`) |
| Ctrl+K · `␣␣` | jump to any conversation or person — unread first, then the most recently active; with something typed, a name equal to it first (ignoring case, spaces and a leading `#` / `@`; a person's display name, real name or handle), then the names starting with it, then the rest (`#` lists every channel, including ones you are not in) |
| Ctrl+F · `␣fs` | search messages, the newest match first (more results load as you move down the list) |
| Ctrl+A · `␣fa` | the **Activity** view, in the main pane in place of the conversation: mentions, reactions to your messages, thread replies, new direct messages, and the rest of what Slack's Activity badge counts — new messages in a channel you get notified of, an app's messages, being added to a channel, a mention in a channel you are not in, someone new to welcome, Slack Connect invitations and list items — newest first under date headers ("Today", "Yesterday", then the day as F6 → Region writes it). Each item shows who (in a thread, "+N" others who replied in it), their status emoji and when; the message itself as a conversation draws it (links, code, custom emoji; up to three lines, then "…"; "message not available" when it is gone); and where it happened — "Mention in #general thread", "Thread in #general · 2 new replies", "Direct message", "Reacted (🎉 1) to:" over your message, "New messages in #general · 3 new", "App message in …", "You were added to #general", "Mention in #general, a channel you are not in" — with the unread bar on unread ones. Tabs **All · DMs · Mentions · Threads** in a bar on top: `]` / `[` next / previous, `1`–`4` straight to one, or click it (each tab keeps its list while the view is open, and loads again when shown); `u` keeps only the unread items (again: all; it stays on until you turn it off, across tabs and each time the view opens, until klack quits). Older items load as the end of the list comes on screen (with only the unread ones kept, until one shows or the list ends — at most 5 pages by themselves: then "There is more: move down to load it" at the end, and moving down, `u`, another tab, `r` or opening the view again goes on; a page that could not be loaded, or one with nothing new, is said there and asked for again the same way). With nothing shown, Enter does nothing. Enter goes there: a DM opens the conversation, a mention or a reaction the message (a reply in its thread), a thread item the thread at its first unread reply, a channel item the channel at its newest message, an app's item its DM, an "added to" item the channel (one you are not in is previewed); a Slack Connect invitation or a list item has nowhere to go in klack, so Enter marks it read and the status line says to open it in Slack; `r` or Ctrl+A again loads the tab again, Esc goes back to the conversation you were in. On an item that is a message (a mention, a reply, a DM, your message someone reacted to) the message keys below (`␣r` react, `␣e` `␣d` edit and delete your own, `␣y` `␣Y` `␣o` `␣s` `␣p` `␣u` `␣U` `␣hT`) and its right-click menu (`␣m`) work as in a conversation, and the view stays: an edit opens a box under the view, only while it lasts (Enter saves, Esc drops it, `i` goes back to it; the conversation's draft is kept aside meanwhile and comes back; leaving the view or quitting with it open keeps it as the draft of the message's thread — or conversation, for a top-level message; when that box is on screen and empty, the edit simply stays in it, "Editing message" — unless a draft is kept there already, and the status line says which; then quitting asks for Ctrl+Q a second time, and another workspace waits until the edit is saved or cancelled); a reaction, an edit or a deletion shows on the item at once (as do other people's, live); mark unread marks the message's own conversation (not a reply's, which is in a thread); on an item that is no message they say so (or that its message is still loading). The **Activity** row in the sidebar (under Threads) opens it too: it turns bold with a bar and a number while there is unread activity — Slack's own Activity badge (unread mentions, thread replies, direct messages and the other kinds above; reactions never count, as in Slack — every kind the badge counts is listed in the All tab, so its unread items and the number agree), kept current from Slack's live events: a new mention, DM or thread reply raises it, reading the item here or on another device lowers it, with no polling (asked for again only after a reconnect). New items show up in the open view as they come, and Enter on an unread item marks it read on Slack as Slack's own client does (a workspace whose counts have no such badge: the All tab's unread items, counted when the workspace starts and each time it loads) |
| `␣fh` | the **Huddles** view, in the main pane in place of the conversation: at a glance, every huddle going on now and who is in it, then the recent ones. **Live now** lists the huddles in conversations you are a member of (not muted ones, as Slack does), yours first, each marked with a green "● Live": where (`#channel`, the person or the people of a DM), how long it has been going, and who is in it with their status emoji. **Recent** lists the ended huddles you were in or missed (from Slack's huddle history, newest first, more loading as the end comes on screen): where, "3 h ago · 25 min", who took part, "N replies" when its thread has some, and a missed one as "Missed huddle" in the alert colour. ↑/↓ (and a click) select; Enter on a live one opens its conversation, on a recent one its thread; the huddle key (`␣hh`) on a live one joins it. `r` or `␣fh` again loads it again, Esc goes back to the conversation you were in. The **Huddles** row in the sidebar (under Activity) opens it too: a badge on it counts the live huddles you can join (not the one you are in), kept current from Slack's live events and asked for again at start and after each reconnect |
| Ctrl+T · `␣ft` | the **Threads** view, in the main pane in place of the conversation: a feed of every thread you are in, unread ones first, so you can read each one where it is; a dim "You're up to date" line under those that had new replies when the view loaded says that the rest had nothing new (no line when all or none had); it stays there until the view loads again — a thread that gets a reply meanwhile shows it in its own block. A thread's block has a heading in bold (where — `#channel`, the person or the people of a DM — and who is in it, "and 2 others", the day when it is not today; unread ones with a bar and "2 new"), its parent in full as in a conversation (a very long one cut after 12 lines with "… more"), "N more replies" when some are hidden, and its last 3 replies in full — all the unread ones when there are more, with the red "new" line above the first of them (Slack's list carries only some replies: for a block on screen that shows fewer than it has, klack fetches its newest ones once); a reply that comes meanwhile goes into its block (one of yours posted from another client too, as read), and an edit, a deletion or a reaction shows in its block as it happens (as in Slack, the threads keep their place: new threads and the order come when the view loads again); a parent klack cannot read says "message not available". A dim rule across the feed and a blank row set each block apart from the next. ↑/↓ (and a click) select message by message, highlighted across the row; Enter opens the whole thread at the selected message, as does a click on a block's header or its "more replies"; `i` replies in the selected message's thread in the box under the feed ("Reply in #channel thread"; its draft is the thread's own — the conversation's comes back when you leave the view), and the reply shows in its block; however the box gets the focus there, it replies in the selected message's thread (Esc with nothing typed lets go of it), and a thread open in the thread pane beside the feed is replied to in that pane's box. The message keys below (`␣r` react, `␣e` `␣d` edit and delete your own, `␣y` `␣Y` `␣o` `␣s` `␣p` `␣u` `␣U` `␣hT`) and the right-click menu (`␣m`) work on the selected message — a parent or a reply — as in a conversation, and the view stays: an edit goes into the box under the feed ("Editing message"; Enter saves, Esc drops it; `i` goes back to it — on another thread too, which the status line says: one edit stays open until you save or cancel it; leaving the view or quitting keeps it as its thread's draft (or in its box, when that is the thread's own and empty) unless that thread has a draft already, and the status line says which — then quitting asks for Ctrl+Q a second time), and once it is over the box has again what it held — a reply half typed stays, as does any thread's draft; nothing opens beside the feed and nothing is marked read for it; a reaction, an edit or a deletion shows in its block at once; mark unread is offered on a parent, not on a reply. A thread is marked read on Slack once its whole block — only for one taller than the view, its unread replies — has been on screen for 2 seconds, not while you scroll past (F6 → Messages & media → Mark feed threads read, `[threads] read_after_secs`, 0 = only when opened); its bar and count go, its "new" line stays until the view loads again; opening it or replying marks it read too. `r` or Ctrl+T again loads it again, more load as the end of the feed comes on screen, Esc goes back to the conversation you were in. The **Threads** row atop the sidebar opens it too: it turns bold with a bar and the number of unread replies when threads you follow get new ones. Selecting a conversation in the sidebar (or jumping to one) leaves a view; klack always starts on a conversation, never a view |
| F6 · `/prefs` | preferences: availability, appearance, messages, region, notifications, audio, keys |
| F1 · `?` | all keys |
| Alt+1 … Alt+9 · `␣w1` … | workspace |
| Ctrl+Q | quit |
| Enter (on a message) · `␣t` | open its thread |
| Enter · `i` (sidebar) | type in the conversation |
| Esc | leave the composer · close the thread or a dialog · back from the Threads / Activity / Huddles view |
| mouse | click to select, wheel to scroll (also the help and the preferences; in lists like Ctrl+K, search and the Activity and Huddles views it moves the selection; it scrolls the Threads feed; a selection any scroll — the wheel, PgUp / PgDn, new messages pushing it up — leaves off screen goes onto the nearest message still there, the top one after scrolling down, the bottom one after scrolling up, so ↑ / ↓ go on from what you see), hover shading (the whole message row, edge to edge), right-click for a context menu (messages, sidebar entries); a thumb on a pane's right border shows where you are when it scrolls |

After `Space` (written `␣`), on a **message**:

| Key | |
|---|---|
| `␣r` `␣e` `␣d` | react · edit · delete (the reaction picker also shows who reacted: `remove · you, alice`) |
| `␣y` `␣Y` | copy the text · copy a link to the message |
| `␣o` | open what the message holds — a link (one into your Slack opens in klack, others in your browser), a picture, a video (Space pauses, ← / → jump 10 s), a snippet; with several, a list to pick from |
| `␣s` | save files (a dialog: Space / `a` pick which, Tab changes the folder for this save, Enter saves) |
| `␣p` `␣u` `␣m` | pin · profile card of the author — their big profile picture on top (as Slack shows it; smaller on a short terminal, none when too short; their initials while it loads and where the terminal draws no pictures), then name, status, local time and the rest (←/→: people the message mentions; Enter messages them, `␣hp` starts a huddle) · the message's menu, as a right-click opens it |
| `␣U` | mark unread from this message (the conversation is unread again from here; klack leaves it so until you switch away and back) |
| `␣hT` | start (or join) a huddle in the message's thread |

The same keys work on the selected message of the Threads feed and on an Activity item that is a message, the view
staying (`␣t` says Enter opens it there); they are the `[messages]` keys, so rebinding one there rebinds it in those views too (their own keys come
first: a `[threads]` or `[activity]` binding on the same keys wins there).

On a **conversation** in the sidebar: `␣s` star · `␣m` move to a section (or a new one) · `␣d` details (about,
members, pins) · `␣b` what it notifies about · `␣M` mute / unmute · `␣y` copy the conversation's link · `␣l` leave. The
same, and more, from anywhere under `␣c` (`␣cs` `␣cm` `␣cd` `␣cb` `␣cM` `␣cy` `␣cl`, `␣cu` mark unread from the selected message
or the latest, `␣cj` join, `␣cr` rename and `␣cx` delete the section). The sidebar's right-click menu offers mute, mark unread and
copy link too. The other groups:

| Group | |
|---|---|
| `␣h` huddle | `h` join or leave · `m` mute · `t` its thread · `v` huddle view · `c` camera · `s` share screen · `d` decline an invitation |
| `␣n` new | `c` channel · `p` private channel · `d` DM · `s` snippet · `f` upload a file |
| `␣a` availability | `s` set status · `c` clear it · `a` away / active · `n` pause notifications for 30 minutes, or resume them when paused · `N` pause for a time you type (`/dnd `) |
| `␣f` find | `f` jump to · `s` search · `a` activity · `t` threads · `h` huddles · `c` every channel (channels) |
| `␣w` workspace | `1` … `9` |

In the sidebar `z` (or a click on a header) folds / unfolds the section: folded, only "▸ name" shows,
with the number of its conversations with unread messages and a badge for mentions; Enter on it unfolds
too, and going to a conversation inside (Ctrl+K, search, a notification) unfolds it. Kept per workspace
account in `sidebar/<team>-<user>.json` under the local data folder. `/section rename <name>` ·
`/section delete` (each section's name heads a rule, with a blank row between sections — none in
compact mode).

What you type is kept per conversation and per thread: leave a half-written message (or an
edit in progress) and it is back when you return, until you send it; the sidebar marks such
conversations with ✎. Drafts also survive quitting, switching workspace and crashes (they are saved two seconds after you stop typing): they are kept per
workspace account in `drafts/<team>-<user>.json` under the local data folder
(`~/.local/share/klack` on Linux), readable by you only, and the file goes once no draft is left.

Typing (INSERT):

| Key | |
|---|---|
| Enter · Alt+Enter | send · new line |
| Esc | cancel an edit, else leave the box (back to NORMAL) |
| Ctrl+V | paste from the clipboard: text is typed in; a picture (a screenshot) or files copied in a file manager are attached above the box (✕, or Backspace in an empty box, removes one) and Enter sends them with your text — or on their own — as one message. Your terminal's own paste (Ctrl+Shift+V) only carries text, and over SSH there is no clipboard to read |
| `Ctrl+Space` | the leader while you type: Ctrl+Space then `h` and so on, as `␣h` elsewhere |
| `Ctrl+Space x` · `Ctrl+Space Backspace` | clear the box for good: the text, attachments and mentions (an edit in progress is cancelled, as Esc does) |
| `/` (first in the box) · @ # : | command (klack's own and Slack's, with what they take; a command typed out in full runs on Enter) and its fixed choices (`/dnd 30m`…`off`, `/dnd schedule` kinds and hours, `/status clear` and `… for today`, `/section rename\|delete`, `/workspace` names, `/snippet` languages), mention, channel, emoji autocomplete |
| `/status [:emoji:] [text] [for 30m\|2h\|today]` · `/status clear` | set or clear your status |
| `/away` · `/active` · `/dnd` · `/dnd 30m` · `/dnd off` | appear away / active · plain `/dnd` pauses notifications for 30 minutes, or resumes them when paused · pause for a time · resume (ends a pause and the current quiet hours of your notification schedule; the schedule stays) |
| `/dnd schedule` · `/dnd schedule off` · `… every day 9:00-17:00` · `… weekdays 9:00-17:00` | say the notification schedule · turn it off · receive notifications every day / Monday to Friday in these hours (the account's time zone), paused outside them; per-day hours in F6 → Availability |
| any other `/command` | run by Slack, as its own clients do (`/remind`, `/giphy`, `/shrug`, apps' commands); an answer only you see is marked "only visible to you"; an unknown one says so; a leading space sends the line as text |
| `/upload <path> [text]` | upload a file (the path completes as you type: Tab goes into a folder; `~` is your home; names with spaces are quoted for you) |
| `/join [#channel]` · `/leave` | join (default: the channel you are previewing — Ctrl+K finds channels you are not in; a previewed one is fetched again every 15 s, F6 → Messages & media) · leave the current channel |
| `/create <name>` · `/create-private <name>` | create a channel |
| `/dm @a [@b …]` | open a DM, or a group DM with up to 8 people |
| `/snippet [lang] [title]` + lines | post the lines after the command (Alt+Enter) as a text snippet |

Ctrl+Space is also what macOS uses to switch input sources, and Windows input methods may take it. If
klack never sees it, rebind `insert_leader` in F6 → Keys.

If you miss a single key: single letters and F-keys moved behind `Space`, and every one of them can
be put back on a key of its own (see below). `klack ctl keys-template` prints every action with its keys.

### Links into your Slack

A link to a channel, a message, a thread reply, a person or a file in one of your profile's
workspaces is shown as `↪ <label>` (for a message, `› message from <author>` once klack has
looked it up) in a colour of its own (`app_link` in the theme). A plain click opens it in klack: a
link into another workspace of the profile switches to it first, and a link to something klack
cannot find says so in the status bar. Hovering a link names where it goes. `␣o` lists "In klack"
first, then "In the browser", then "Files"; Ctrl+Shift+click (kitty) still opens the link in the
browser. Links to other places open in the browser as before.

### Rebinding keys

F6 → Keys lists every action under its group, with the which-key settings above them. `/` filters
the list (Enter keeps the filter, Esc clears it), Enter on a row records the new keys — press
them, or a whole sequence such as `Space r` — Delete unbinds, `r` restores the default, and Enter on a
group renames it (`i` changes its icon). Only the one entry you change is written to
`~/.config/klack/keys.toml` (macOS: `~/Library/Application Support/klack/`, Windows:
`%APPDATA%\klack\config\`); your comments and the order of the file stay.

`klack ctl keys-template` prints a complete keys.toml with every default and a comment for each action
— a good start to edit by hand. Each table is a context (`[global]`, `[sidebar]`, `[messages]`,
`[composer]`, one per dialog…), each entry replaces an action's keys, and a line left out keeps its default:

```toml
leader = "space"              # the leader in NORMAL; "" for none
insert_leader = "ctrl+space"  # the leader while typing and in dialogs

[groups]                      # names and icons in the which-key panel
"<leader> h" = { name = "calls", icon = "🎧" }

[labels.messages]             # a label and icon for one action
react = { label = "quick", icon = "⚡" }

[messages]
react = ["<leader> r", "R"]   # several keys or sequences for one action
pin = []                      # unbound
copy = "y"                    # back on a key of its own
```

A sequence is keys separated by spaces, `"<leader> r"` or `"ctrl+x ctrl+s"`; a key is written like
`ctrl+k`, `alt+up`, `shift+tab`, `G`, `f5`, `space`, `pageup`. An action can have a key of its own and
sequences at once. Plain characters cannot be bound where you type (global, composer, search fields)
— there use `insert_leader` sequences. Problems are shown in the status bar at startup, and the
default stays in place for any entry that has one. Existing keys.toml files keep working: your binding
beats the default.

## Huddles

`␣hh` joins (or starts) the huddle of the conversation you are in, and `␣hT` on a message starts (or
joins) a huddle in that message's thread; you join **muted**, and the
microphone is only opened while you are unmuted (`␣hm`). While you are in a huddle, others see you
"In a huddle" (🎧 when you have no status emoji) as with Slack's own app: Slack sets it when you join and
clears it when you leave (leaving, or quitting klack in a huddle, also tells Slack with `rooms.leave`, as its own client does, so the huddle's list drops you at once), and klack renews it every ~4 minutes, as Slack's app does — only when your Slack
preference for it (`huddles_set_status`) is on, and never touching your status. The call bar at the bottom shows that you are
still in the huddle — where, how many are there, your microphone (🎙 live with your level, or 🔇
muted), your camera and a screen share — and the picture and name of whoever is talking, or talked
last, instead of everyone; with its keys underneath, and it takes more rows rather than cutting
anything off when the window is narrow. `␣hv` (or a click on the call bar) shows the huddle view in place of
the messages: a tile per participant with their picture or video — the border lights up while
they speak, 🔇 in the title when muted — that shrinks as people join and scrolls (mouse wheel,
"▲/▼ N more") when they no longer fit; a screen share takes the stage with everyone else in a
strip. Arrows select a tile, Enter (or a second click) maximizes it, Esc shows all tiles again
and then leaves the tiles; `␣hv` again, or opening a conversation, brings the messages back. Someone starting a huddle in a DM with you sends a
desktop notification. While you are in Do Not Disturb (snoozed or in your schedule), Slack sends no
invitation at all, so nothing rings — as in Slack's own app.

If a huddle's connection is lost (a network change, say), klack joins again by itself, as Slack's app
does: the status line says "huddle connection lost — joining again…" and the call bar "reconnecting…"
(with no microphone level meanwhile), then "back in the huddle", usually a few seconds later; mute and camera
stay as they were (also when changed meanwhile). Once you were
connected, any trouble with the connection joins again: the media failing or staying disconnected for
4 s, a signaling error from the server, a broken signaling socket. The call ends when you leave (also while
it is joining again), when the server closes the signaling connection (the huddle ended, or you were
removed), when the very first join fails, after three tries in a row that do not stay up for 30 s
(0, 2 and 4 s apart), or after more than ten reconnects within an hour. In those last two cases klack tells
Slack it left, as when you leave, and the status line says "huddle connection lost — left the huddle".

Clicking a desktop notification (Linux desktops with an XDG notification server) opens its
conversation in klack — a thread reply opens the thread with the reply selected — for 30
minutes after it was shown. The terminal window is not raised (a terminal program cannot do
that portably).

Under the hood a huddle is an Amazon Chime SDK meeting: `rooms.join` hands out the meeting
credentials, `crates/huddle` speaks Chime's signaling protocol and runs WebRTC audio (webrtc-rs,
Opus). See `docs/protocol/huddles.md` for how this was worked out.

Pick audio devices with `KLACK_AUDIO_INPUT` / `KLACK_AUDIO_OUTPUT` (part of a device id or
name from `klack ctl audio-devices`). A device that is named but missing is an error; there is
no silent fallback.

F6 → Huddles has the huddle sounds (at the notification sound volume; each can be turned off): a soft
rising arpeggio when you join, and while you are in a huddle a short "pop" when someone joins and a
falling pair when someone leaves. A huddle invitation (someone starts one in a DM with you) rings — a
soft trill every few seconds, with "📞 X invites you to a huddle" in the status bar — until you join (`␣hh`,
from any conversation), decline (`␣hd`), the huddle ends, or it has rung for the set time (30 s); then,
if "Then decline it on Slack" is on, it is declined for you. When you have been the only one in a
huddle for a while (300 s by default, adjustable), a quiet melody loops until someone joins or you leave.

F6 → Audio also has the huddle volume, the notification sound volume, a microphone gain, a
speaking threshold (quieter microphone input is sent as silence, held open 300 ms after you
stop so words are not cut), noise suppression (RNNoise via `nnnoiseless`) and echo cancellation
(on by default: WebRTC's AEC3 via the pure-Rust `sonora`, so others do not hear themselves from your
speakers; it drops the microphone above 16 kHz, which speech does not need). `+`/`-` (or Enter)
step the levels, and a huddle you are in follows at once.

Other people's cameras play in their tiles in the huddle view (`␣hv`); a screen share gets its own
"🖥 <name>'s screen" tile ("🖥 <name> is sharing" in the call bar).

Your camera: choose it in F6 → Audio & video → Camera (until you do, it can never switch on), then
`␣hc` in a huddle switches it on and off; you see yourself in your tile. Capture goes through the
`ffmpeg` tool (V4L2 on Linux; AVFoundation/DirectShow on macOS/Windows, where cameras are listed
with ffmpeg's `-list_devices`, are written but have never been run).

Video is H.264 (OpenH264, built from source — note that H.264 is patent-encumbered when you
distribute binaries). For tests, `klack ctl huddle-media <channel> --video-test` joins with a
synthetic camera (a moving test pattern), `--share-test` shares it as a screen, `--camera NAME`
sends a real camera (`ctl cameras` lists them), and `--record-video DIR` saves what arrives.

Your screen: `␣hs` in a huddle starts sharing, `␣hs` again stops. Others see it as your screen share;
you see it in a "🖥 Your screen" tile. In a Wayland session the desktop asks what to share (a
screen or a window, through the screen-cast portal, read with GStreamer's `pipewiresrc` —
`gst-launch-1.0` and the GStreamer PipeWire plugin must be installed). Elsewhere there is no picker
and `␣hs` shares the **whole** screen through `ffmpeg`: `x11grab` in an X11 session, the main screen
via AVFoundation on macOS (the terminal needs the Screen Recording permission), the desktop via
`gdigrab` on Windows. The Wayland portal and X11 (against a virtual Xvfb display) have been tried;
on Windows only from an SSH session, where `gdigrab` cannot capture (no desktop); macOS is untested.

## Other settings

| Variable | |
|---|---|
| (F6 or `/prefs`) | preferences: availability (status, away, Notifications — on or paused until when; Enter pauses for 30 minutes or resumes, + / - change a pause's length — and the notification schedule: off, every day, weekdays or custom, with the hours you receive notifications in, per day for custom: hours, receive all day or paused all day; kept on Slack), theme, compact list, thumbnails, profile pictures, emoji as names, custom emoji as pictures, how often people's statuses are looked up again, when feed threads are marked read, 12/24 h clock, date format, time zone, language, desktop notifications and sound, microphone and speaker, volumes, microphone gain, speaking threshold, noise suppression, echo cancellation — saved in `settings.toml` next to `keys.toml` |
| `KLACK_LOG` | log filter, e.g. `klack=debug` (log file: `~/.cache/klack/<profile>/klack.log`); the websocket library's handshake and frames (`tungstenite::handshake::client`, `tungstenite::client`, `tungstenite::protocol`), which would print the token, cookie and messages, stay off unless named |
| `KLACK_DOWNLOAD_DIR` | where `␣s` saves attachments first, over F6 → Messages & media → Save files to (default: your downloads folder) |
| `KLACK_OPEN_WITH` | the program `␣o` opens links with, given the address (default: the system's opener — `xdg-open`, `open`, Windows' URL handler); only web and mail links are opened |
| `KLACK_AVATARS=0` | initials instead of profile pictures next to names (as on terminals without real graphics; F6 → Messages & media → Show profile pictures turns both off, Two-row profile pictures makes them bigger) |
| `KLACK_GRAPHICS` | how pictures are drawn: `kitty`, `sixel`, `iterm2` or `halfblocks` (default: asked from the terminal). F6 → Messages & media → Draw pictures with (`[messages] graphics` in `settings.toml`) offers only the ways the terminal said at start it can draw — `auto` and `halfblocks` always — and its help marks the others "not available in this terminal"; a `settings.toml` naming one of those draws as `auto`, with a note in the status bar (the file is left as it is). `KLACK_GRAPHICS` still forces the way it names, for when the terminal is not asked right (over ssh or tmux) — with a warning in the log when the terminal did not report it |

### Themes

F6 → Appearance → Colour theme opens a picker of dark, light, terminal and your own themes,
each with a few of its colours: type to filter, ↑/↓ show each one on the whole screen at once,
Enter keeps it, Esc goes back (a theme file that does not load says why). A theme is
a TOML file in `themes/` next to `settings.toml`: `base = "dark"` plus any colours to change
(`"#ff8800"`, ANSI names such as `"light-red"`, or 0–255). Start from
`klack ctl theme-template dark > sunset.toml`, edit it, and install it (checked first) with
`klack ctl theme-import sunset.toml`; share themes by passing the file around. `link` is the colour of web links, `app_link` of links into your own Slack.

To tell workspaces apart at a glance, F6 → Appearance → Theme for this workspace gives the
workspace you are in a theme of its own (the same picker, with "(colour theme)" for none);
`settings.toml` keeps it under `[appearance.workspace_themes]` by workspace id.

### Translations

The interface can be translated: `klack ctl locale-template > ~/.config/klack/locales/de.toml`,
fill in the right-hand sides, then choose `de` in F6 → Language & region. Only the help, the
preferences, overlay titles and the status-bar hints go through translation so far.

## Scripting

`klack ctl …` exposes the API for automation and tests: `whoami`, `channels`,
`history`, `replies`, `post`, `react`, `pin`, `upload`, `search`, `listen`, `audio-devices`,
`huddle-media`, `huddles` (the huddles happening now, `--recent` the latest ones, with ids shown as `C1`, `U1`, …)
and a few research probes (`api`, `get`, `huddle-*`, `huddles --shape`) that redact secrets.

## Layout

- `crates/slack-core` — session/auth, Web API client, RTM websocket, message formatting.
- `crates/huddle` — Chime signaling (protobuf), WebRTC audio, Opus, audio devices.
- `crates/klack` — the TUI and the `ctl` CLI.
- `vendor/rtc-turn` — webrtc-rs' TURN client 0.21.1 with a fix for "438 Stale Nonce" answers, which
  dropped a huddle's audio about 10 minutes in (why, what changed, when to drop it: `vendor/rtc-turn/KLACK-PATCH.md`).

Status and verification notes: `PROGRESS.md`. Design: `DESIGN.md`.

## Licence

klack is licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT licence](LICENSE-MIT), at your option. Unless you say otherwise, any contribution you submit
for inclusion in klack is licensed the same way, without any additional terms or conditions.
`vendor/rtc-turn` is webrtc-rs' code (MIT or Apache-2.0, its licence files are in that folder).
