# How the numbers on the page were measured

Measured 2026-10-21 on one machine: AMD Ryzen 9 9950X (16 cores), 64 GB RAM, an AMD Radeon GPU, Pop!_OS 24.04
(Linux 7.2), Hyprland (Wayland). klack 0.8.0 — the release download (`klack-0.8.0-x86_64-linux.zip`, checked against
SHA256SUMS), not a development build. The official app: Slack 4.52.178 for Linux (flatpak `com.slack.Slack`).

App against app: the terminal klack runs in is not counted (every terminal costs something different; a stock
xterm costs far less than a GPU terminal), and the flatpak sandbox wrappers (`bwrap`, `flatpak`) are not counted
for the official app. Both apps were signed in to the same Slack account and did the same things, one app at a time.
Neither app made a sound: the official app's audio went to a silent PipeWire null sink; klack ran with sounds off.

The tool is `tools/site/measure.py` (RAM = the sum of PSS over the app's whole process tree from
`/proc/*/smaps_rollup`; CPU = utime+stime of the tree over a window, as a share of one core).

| Row | How |
|---|---|
| download | klack: the release zip's size on GitHub. The official app: the size of Slack's own Linux package (`slack-desktop-4.52.178-amd64.deb`, HTTP Content-Length). |
| on disk | klack: the unzipped binary. The official app: `du -sb` of the installed app files (the unpacked package; the flatpak runtime it also needs is not counted). |
| startup | Wall time from launch until the first conversation's messages are on screen; three warm starts each, the median. klack: launched in a terminal window, the moment its screen shows a loaded conversation (polled with `kitty @ get-text`; the window's own start is included). The official app: `flatpak run …`, the moment a message list exists in its page (polled over the DevTools protocol). |
| RAM at rest | A fresh start, two minutes untouched, then three samples five seconds apart. |
| RAM busy | Then the same busy channel was opened in both and scrolled back 60 steps (klack: PageUp ×60; the official app: 60 mouse-wheel steps over the message list — its PageUp went to the message box), 30 s settle, three samples. |
| CPU at rest | One minute untouched, after the two-minute settle. |
| CPU while scrolling | A 25-second window spanning the 60 scroll steps (0.3 s apart) in the same channel. The official app's GPU time in that window (from `drm-engine-*` in `/proc/<pid>/fdinfo`) was 0.18 % — it hands part of its drawing to the GPU; started with `--disable-gpu` it used 16.8 % CPU for the same steps. klack uses no GPU. |
| sharing a screen | A huddle between two test accounts in a test workspace: each app shared the same 1280×720 30 fps synthetic test window (picked in the desktop's own share picker), the other app watching (the watcher received 1280×720). One minute of CPU and a RAM sample for the sharing app. Microphones went to the silent null source; nothing was recorded. |
| video memory | `drm-memory-vram` of the official app's GPU process at rest. klack itself allocates none. |

klack is a little busier than the official app at rest (0.6 % vs 0.45 % of one core) — said so on the page.
