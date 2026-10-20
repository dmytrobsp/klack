# Sign in anywhere: a code from your phone, or your own browser — design (queue Y)

Status: brainstormed with the user on 2026-10-20; builder check done (its questions were all about claiming
`slack://`, which the user then dropped). "(user)" marks their decisions, "(Claude's call)" marks choices made for
them that they did not object to when played back, "(builder's call)" is left to the implementer. Anything unmarked
follows from the user's request, from the code as it is, or from what was verified live.

Builds on queue W (docs/superpowers/specs/2026-10-20-prefs-help-and-auth-design.md): the guide of `klack auth login`,
its picker (`pick.rs`), sign-ins lifted from browser profiles (`login_import.rs`), expired sessions at start
(`auth::at_start`, `sign_in_again`) and `SessionSource`.

## Why

- "Current way of login is still inconvenient, especially over ssh on a remote machine where graphic environment is
  unavailable and there might be no browser at all installed" (user). There the guide finds no browsers, the
  throwaway window can't open, and the only way left is copying the `d` cookie out of another machine's devtools.
- "Phone is always nearby. If we could somehow integrate it into the login process it would be ideal. QR or email
  code, anything would do. I wouldn't go as far as to build a companion app" (user).
- The throwaway window (a fresh browser profile) cost the user Google sign-ins, captchas "sometimes for minutes" and
  Chrome's "set up a profile" prompts (W). Signing in with the browser they already use avoids all three, and Google
  sign-in, passkeys (including a phone's passkey over the browser's own QR), SSO and 2FA then work as on any site.
  The user has several Chrome profiles, so klack must not pick the browser or profile: "give user a link to open, so
  he can either copy it in whatever browser he chooses or just click" (user).

## What was verified (2026-10-20)

**The emailed code can be confirmed by klack, whoever asked for it.** Live on test account B, with three codes the
user requested on their phone at slack.com/signin: only *sending* the code needs Slack's reCAPTCHA Enterprise
(J3, docs/protocol/auth.md); confirming it does not, and is not tied to the device that asked. From a plain HTTP
client, keeping one cookie jar throughout, a browser `User-Agent`, `POST https://slack.com/api/<method>?_x_id=noversion-<unix time>`
with a `multipart/form-data` body:

1. `GET https://slack.com/signin` — sets cookies `b`, `ui`, `x`.
2. `signin.confirmCode` `email`, `code` — the code without its dash (`REP-OEU` → `REPOEU`). Reply `{ok, has_workspace}`
   only; sets cookie `ec`. Errors seen in Slack's page code: `no_email, no_code, failed, expired, ratelimited,
   magic_login_failed, captcha_failed`.
3. `signin.findWorkspaces` `email`, `reason=get_started_workspaces` — reply `current_teams` (and `current_orgs`,
   `invited_teams`, `domain_enabled_teams`); each team has `id, name, domain, url, magic_login_url, magic_login_code,
   sso_required, two_factor_required, is_logged_out, …`.
4. `GET` a team's `magic_login_url`, following redirects (app.slack.com → the workspace → slack.com) — sets `d`
   (and `d-s`, `lc`).
5. The usual `session_from_cookie` (`/customize/emoji` → `xoxc`, then `auth.test`) — passed as B on rslack-testbed.

Each try left one more B session on Slack; nothing was signed out.

**Slack's sign-in page fills in the email from the address:** `https://slack.com/signin?email=you%40example.com`
(a query string; after the `#` it is ignored) — checked in a headless browser, field value read live.

**Not used, and why** (so it isn't rediscovered): catching the desktop app's `slack://…/magic-login/<token>` hand-off
would mean claiming the system's `slack://` handler; the user first allowed it with a guaranteed restore, then dropped
it (the browser's portal and Chrome remember "open with" choices outside what klack could restore; it may not even
reach `slack://` outside the desktop app). Slack's `lsp=<port>` localhost hand-off works on its dev/QA hosts only.

## The guide's list (user: "equal choices")

`klack auth login` (and plain `klack` on a profile with no sign-ins, and "sign in again" for an expired session) keeps
W's sign-ins found in the browsers first, then **three ways to sign in afresh**, replacing W's single "another
workspace, or sign in another way…" row and its sub-list:

```
Sign in to Slack (profile default)

  Chrome · Personal                     you@gmail.com
    rslack-testbed                      ✓ works
  Firefox · default
    acme                                ✓ works
  ──────────
  with a code from your email (phone or any device)
  in your browser
  paste a cookie
```

- The ways are equals (user); the code is first, as it works everywhere (Claude's call on the order).
- With no graphical session (Linux: no `DISPLAY`/`WAYLAND_DISPLAY`; any system: `SSH_CONNECTION` or `SSH_TTY` set)
  "in your browser" is **left out**: a browser elsewhere can't be read from here (Claude's call, played back: "code
  first"). A way that can't work here for another reason is shown dim with the reason, as W's rows are.
- **No way asks for a workspace name first** (Claude's call): each ends with the workspaces Slack lists for that
  sign-in, in the same picker (Space marks several, Enter signs in to all marked, `/` filters, a click moves and
  toggles), with W's notes ("already signed in", a reason when one can't be used).
- Marking browser sign-ins *and* a way: the marked ones are saved first, then the way runs (as W does today).
- **The throwaway browser window is removed** (user: "regular browser login is pretty much the same thing without the
  throwaway part"): `login_browser.rs`, its DevTools capture and Chrome-prefs seeding, the "in a new browser window"
  row and `--no-browser` go. Old sessions saved with `SessionSource::Window` still load (shown "a browser window";
  signing them in again shows the full list). `purge` still removes the old window-profile folder if one is left.

## Way 1 — a code from your email

```
Sign in with a code
  Email: you@gmail.com

  ▄▄▄▄▄▄▄ ▄ ▄▄ ▄▄▄▄▄▄▄     On your phone, scan this — or open
  █ ▄▄▄ █ ▀█▄█ █ ▄▄▄ █     slack.com/signin?email=you%40gmail.com
  █ ███ █ ▄▀ ▀ █ ███ █     Slack emails you a code. Type it here;
  …                        don't enter it on the phone.

  Code: ___-___
```

1. **Email** — one line (W5's `ask`); when signing in again, the email the session came from is filled in (Enter
   keeps it).
2. **QR code** (user: "definitely") opening `https://slack.com/signin?email=<the email, percent-encoded>` (user: "not
   have to input email twice"), drawn with Unicode half blocks, dark modules on an explicitly light background with
   the quiet zone, so it scans on dark terminal themes too. The same link is printed beside it (or under it when the
   terminal is narrow), as an OSC 8 hyperlink with the address as its visible text; a terminal too small for the code
   shows only the link. On the phone the user passes Slack's captcha if asked; Slack emails the code.
3. **Code** — six letters or digits, with or without the dash, any case; shown as typed (it is single-use and
   short-lived — Claude's call). `failed` → "that code didn't work — type it again, or ask for a new one on the
   phone" and the prompt again; `expired` → ask for a new one; `ratelimited` → say so and stop. Esc at any prompt
   goes back to the list.
4. **Workspaces** — the picker over `current_teams` and the teams of `current_orgs` (Enterprise Grid), marked by
   default when there is exactly one (Claude's call). A team with `sso_required` is dim: "needs your browser (SSO)".
   `two_factor_required`: after its magic link klack asks for the 2FA code and finishes the sign-in (Slack's page
   uses `auth.loginMagic` `magic_token, two_factor_pin, ssb`); if that turns out not to work without a browser, the
   row is dim "needs your browser (two-factor)" instead (builder's call after a live check — needs an account with
   2FA, ask the user).
5. Each marked team's `magic_login_url` is redeemed in the one cookie jar, then `session_from_cookie`; saved with a
   new `SessionSource::Code { email }`.
- `klack auth login --email ADDRESS` starts this way directly (Claude's call); without a terminal the link is printed,
  the code is read from standard input, and every listed team that can be used is signed in.

## Way 2 — in your browser (user: a link, any browser, any profile)

```
Sign in in your browser
  Open this link in the browser and profile you want to use (copy it, or Ctrl+Shift+click in kitty):

  https://slack.com/signin

  Sign in there any way Slack offers. klack is watching your browsers and picks the new sign-in up
  (it can take up to half a minute for the browser to save it). Esc cancels.
```

1. klack prints the link (an OSC 8 hyperlink with the address as visible text, so it can be clicked or copied; never
   opened by klack — user). When signing in again, the link is the workspace's own sign-in page (Claude's call).
2. Before printing it, klack notes the Slack sign-ins every browser profile has now (W6's discovery; on macOS the
   Keychain is asked once, as in W, and the key kept for the wait). Then it **watches**: when a profile's Slack
   cookie appears or changes (cookie files checked every 2 s by modification time, then read — Claude's call), it
   checks that profile's sign-ins.
3. What is new (a team + user the profile was not signed in to before, or a new session for one that was) is shown
   in the picker, marked, labelled with the profile ("Chrome · Work — rslack-testbed as you"); Enter saves it, like a
   lifted sign-in (`SessionSource::Browser`, so signing in again later offers that profile first, as W does). The
   watch keeps going until the user picks or presses Esc, or for 10 minutes (Claude's call).
- Browsers klack can't read are named on the waiting screen: "Chrome on Windows can't be read — use Firefox, or the
  code" (W's rule); on Windows with only Chrome, the row is dim with that reason.
- No change to the system, the browser or its profiles: klack only reads, as W's discovery does.

## Way 3 — paste a cookie

As W (hidden input), then the cookie's sign-ins (`GET https://slack.com/signin` with it, W6's `signed_in_teams`) in
the picker instead of asking for a workspace first. `--cookie`/`$KLACK_COOKIE_D` with `--workspace` keep working for
scripts.

## Signing in again (expired sessions, W8)

"Sign in again" offers first the way the session came from, as W does for browser profiles: a code session asks the
code way with its email filled in and that workspace marked; a browser session offers that profile (W), then this
queue's browser way with the workspace's own sign-in link. Esc there, or any other source, shows the full list.

## Remembering the source

`SessionSource` gains `Code { email }`; `auth status` shows it ("code to you@gmail.com"). The email is kept in the
owner-only `session.json`, never logged.

## Constraints

- Never print or log tokens, cookies, codes or magic links; the email only on screen where the user typed or confirms
  it. Fixtures anonymised.
- Nothing is signed out anywhere; every way only adds sessions.
- No server of klack's own, no relay, no companion app (user); no change to the user's system or browsers (klack
  only reads browser profiles).
- The new HTTP calls go through `slack-core` with the existing browser `User-Agent` and client settings; no new HTTP
  stack. A QR crate is fine (the `qrcode` crate or similar, pure Rust).
- Builds on Linux, macOS and Windows as today (CI).

## Anti-goals

- klack opening a browser or choosing the profile (user).
- klack sending the code itself (that needs the captcha) or solving captchas.
- Claiming `slack://`, passkeys handled by klack, OAuth/`xoxp` tokens, a hosted page or relay, a phone app.
- A code way that only works on some machines: it must work over SSH with nothing installed.

## Tests

- Unit: parsing of `signin.confirmCode` / `signin.findWorkspaces` replies (anonymised fixtures from the live run),
  the code normalisation, the QR link and its percent-encoding, the list's rows with and without a graphical session,
  the "what is new" comparison of two discovery snapshots.
- Live (needs the user, who requests the codes on their phone and reads B's inbox): the code way end to end on B in
  an environment without `DISPLAY` (as over SSH). The browser way only against a throwaway browser profile seeded or
  signed in as B, never the user's real profiles (W's rule). Windows on the VM (restore it to its prior power state
  afterwards); the Mac for the code way and the browser way's Keychain prompt (once).

## Left to the builder

- How the watch reads cookies cheaply (mtime first; Chrome's and Firefox's write delays), and how a changed `d` is
  told apart without keeping cookie values around (a hash is fine).
- Whether `signin.findWorkspaces` needs the email (it worked with it) and what `current_orgs` looks like on a Grid.
- Removing `login_browser.rs` cleanly (what else uses it, e.g. W7's seeding), and the docs (README, CONTRIBUTING,
  docs/protocol/auth.md gets the verified code sequence).
- Wording of prompts and notes beyond what is quoted here.
