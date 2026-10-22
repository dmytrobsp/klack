# Dev tools

Helpers used while developing and testing; none of them is needed to build or run ttyak. The
live-test helpers assume Linux with Hyprland, kitty and PipeWire.

- `launch.sh` — start ttyak on profile `default` (given by name, so it is account A even when another profile is the default) in the `ttyak-test` kitty window (abstract socket `unix:@ttyak-test`), DM with B open.
  Drive it with `kitty @ --to unix:@ttyak-test send-key …/send-text …`; quit with `send-key ctrl+q`.
- `streamrec.py OUT_DIR TOTAL_S [PER_STREAM_S]` — record every new ttyak playback stream (never a source) with
  pw-record while it runs; `tonecheck.py HZ[,HZ…] FILE.wav…` — peak level per frequency (to tell sounds apart).
- `shot.sh OUT_DIR NAME` — screenshot that window (then Read the PNG).
- `jsgrep.py PATTERN CONTEXT MAX FILE…` — regex matches with context in minified JS (ugrep chokes on wide contexts).
- `apiargs.py METHOD DIR…` — find a Slack Web API method's webpack module and print its call sites/args.

Web client bundles: `ttyak ctl get https://app.slack.com/client/<TEAM> client.html`, then chunk names/hashes from the
`__cf` map in it → `https://a.slack-edge.com/bv1-13/gantry-v2-async-<name>.<hash>.min.js`; entry: `client-boot.<hash>.min.js`
(see the end of client.html). The CDN serves gzip: decompress before searching. Details: `../docs/protocol/huddles.md`.

- `cdp.mjs PORT cookie|goto|eval|click|shot ARG` — drive a test Chrome over the DevTools protocol (Node 24, no
  packages). Real Slack web client as B with a fake camera/mic (never real devices):
  `flatpak run com.google.Chrome --user-data-dir=$HOME/.var/app/com.google.Chrome/ttyak-fakecam
  --remote-debugging-port=9333 --use-fake-device-for-media-stream --use-fake-ui-for-media-stream --no-first-run about:blank`,
  then `cookie ~/.config/ttyak/b/session.json` (sets `d`, never prints it), `goto https://app.slack.com/client/<TEAM>/<CHANNEL>`,
  `click '[data-qa=huddle_channel_header_button]'`, in the preview window `click '[data-qa=huddle_camera_huddle_toolbar]'` and
  `click '[data-qa=huddle_invite_window_with_preview_content_join_button]'`; leave with
  `click '[data-qa=huddle_toolbar__leave_button]'`. Close Chrome (CDP `Browser.close`) and delete the profile folder afterwards.
  NEVER click the screen-share button there: the fake UI flag auto-picks the real screen.
