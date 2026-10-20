# F6 contextual help, interactive login and `auth` commands — design (DRAFT, brainstorm not started)

Status: asked by the user on 2026-10-20 (queue W). Nothing is decided beyond the user's own words below; the next
session brainstorms it (superpowers:brainstorming — one question at a time, play back, mark guesses), then writes the
full design here, runs the builder check, gets the user's approval, then a plan.

## 1. Contextual help in F6 (the user's words)

"contextual help in preferences window. right now every section in preferences just prints a bunch of text in the lower
right part. it's hard to read because a) its not formatted and b) it describes every single thing that's in the section.
instead every item inside the section should display its own text. and it should be neatly formatted so its easy to read"

What exists: prefs.rs draws per-category hint lines (`"Keys" => &[…]` etc. in `Prefs::draw`, ~line 632 in older
numbering) under the rows; each `Field` has a label and a value; the Keys category shows the tree (U10).

Open (ask): where the item's help sits (a box under / beside the list, a status-line-like line, which-key-style?); what
"neatly formatted" means to them (title + short paragraph, bullet list of values, the key to change it, the current
value's meaning?); whether the Keys rows get help too (they have descriptions); translation (the locale file).

## 2. Interactive login and `auth` commands (the user's words)

"interactive login. right now there's a special argument `login` that initiates the login flow and requires
`--workspace` argument and optionally accepts `--from-browser`. that's fine, but i want to streamline the process a bit.
i'm thinking that when the app is launched under a profile that's not logged-in into the workspace or simply with a
`login` argument it should guide the user through the login process interactively:
  - ask for a workspace name (and automatically fill-in .slack.com suffix if not filled in by user)
  - it should accept a url in case it was copied from a browser
  - offer different login methods and ask user to pick one
in fact i think the entire `login` argument should instead be replaced with `auth` family of commands. that should allow
for login/logout/status so user can control the workspaces he's signed into. these still should mesh with `--profile`,
so that user can still have separate profiles with different workspaces."

What exists: main.rs `login` (J1 browser-window login, J2 `--from-browser firefox|chrome` with profile choice — see
PROGRESS rows J1/J2/K), `--profile`, `--team`, sessions per profile under ~/.config/klack/<profile>/ (the user's own
`test` profile is real — never touch it), Alt+1…9 workspaces of a profile, `/workspace`.

Open (ask): is the guide a TUI screen inside klack (ratatui) or plain terminal prompts before the TUI starts; which login
methods are offered (browser window, cookies from Firefox/Chrome with profile choice, token paste?); `auth status`
output (workspaces per profile, signed-in user, token health); `auth logout` (one workspace / all; what happens to
drafts and caches); whether `login` stays as an alias for a while (habit change, scripts); how `--profile` interacts
(auth commands act on the chosen profile; a new profile is created by logging in?); what starts when a profile has
some workspaces signed in and some not.
