# Signing in

What klack needs to talk to Slack like the web client, and how it gets it. Observed in the
Slack desktop app (4.52) and web client in October 2026.

## Credentials

- **The `d` cookie** (domain `.slack.com`): the browser session. It is the same for every workspace
  you are signed in to in that browser. Slack also sets `lc`; klack does not need it.
- **A per-workspace `xoxc-…` token**: sent with every Web API call (form field `token`), together
  with the `d` cookie (`Cookie: d=…`). The web client reads it from the boot data of a
  server-rendered workspace page; klack fetches one such page with the cookie and takes the
  `api_token` from it (`session::session_from_cookie`), then checks it with `auth.test`.
- A desktop-browser `User-Agent` (`session::USER_AGENT`): the web endpoints expect a browser.

## How the official clients sign in

The desktop app opens the system browser; sign-in ends at a `slack://…/magic-login/<token>` link,
and the app then loads `https://app.slack.com/api/auth.loginMagicBulk?magic_tokens=…&ssb=1` in a
hidden window, which sets the cookies (`d`, `lc`). klack does not do this: you copy the `d`
cookie from a browser where you are signed in (`klack login --workspace <url>`).

## Storage

Sessions (token and cookie for each workspace of a profile) are saved owner-only in the
profile's `session.json` and never logged (`WorkspaceSession`'s `Debug` redacts them).
