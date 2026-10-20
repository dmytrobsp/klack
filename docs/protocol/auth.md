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
cookie from a browser where you are signed in (`klack auth login`).

## Storage

Sessions (token and cookie for each workspace of a profile) are saved owner-only in the
profile's `session.json` and never logged (`WorkspaceSession`'s `Debug` redacts them).

## Email-code sign-in (recorded 2026-10-19, J3)

What Slack's sign-in page does to sign in by an emailed code (recorded from a throwaway browser over the
DevTools protocol; values never logged):

1. `signup.checkEmail email= get_info=1` → `{type ("gmail", …), auth_url, challenge_response: true}`.
2. `auth.captcha` → `{sitekey}`; the page then solves a **reCAPTCHA Enterprise** challenge.
3. `signup.confirmEmail email= domain= locale= entry_point= captcha_response=` → sends the code.
   Without `captcha_response` it fails with `captcha_empty` (checked; no email is sent).
4. `signin.confirmCode email= code= domain=` → then a magic-login link (`/z-app-…`, `/magic-login/…`)
   sets the session cookie in the browser.

Sending the code needs a browser (the captcha), but confirming it does not: see the next section.

## Email-code sign-in without a browser (verified 2026-10-20, Y)

The code can be sent from any device (a phone's browser, at `https://app.slack.com/get-started?email=<the email>#/createnew`,
because on a phone `slack.com/signin` redirects to the app download page) and confirmed by klack over plain HTTP.
All calls are `POST https://slack.com/api/<method>?_x_id=noversion-<unix time>` with a `multipart/form-data` body,
one cookie jar throughout and a browser `User-Agent`:

1. `GET https://slack.com/signin` sets the cookies `b`, `ui`, `x`.
2. `signin.confirmCode email code` (the code without its dash) replies `{ok, has_workspace}` and sets `ec`.
   Only sending the code needs the captcha, which happens on the phone; confirming is not tied to the device
   that asked.
3. `signin.findWorkspaces email reason=get_started_workspaces` replies with `current_teams`: a list with one
   entry per email address, each holding `email` and `teams`, where every team has `id`, `name`, `url`,
   `domain`, `associated_user`, `magic_login_url`, `sso_required`, `two_factor_required`, `is_logged_out` and
   more. `current_orgs` is assumed to have the same shape (not verified).
4. `GET` the team's `magic_login_url`, following redirects (app.slack.com, the workspace, slack.com), sets `d`.
5. The usual `/customize/emoji` page gives the `xoxc` token, and `auth.test` checks it.

Teams that need SSO or two-factor cannot be signed in this way; klack shows them dim. Errors seen in Slack's
page code for step 2: `no_email, no_code, failed, expired, ratelimited, magic_login_failed, captcha_failed`.
