//! Main loop: terminal input, background API results and periodic ticks.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use futures_util::StreamExt;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Paragraph;
use serde_json::Value;
use slack_core::Client;
use slack_core::types::{HistoryPage, User};
use tokio::sync::mpsc;

use super::composer::{Composer, KeyOutcome};
use super::keymap::{Action, Context, Key, Keymap, LEADER, Mode, Pending, Resolved};
use super::messages::MessagePane;
use super::picker::EmojiPicker;
use super::sidebar::Sidebar;
use super::state::Workspace;
use super::terminal::TerminalGuard;

/// Results of background API calls.
enum Net {
    Boot { boot: Value, users: Vec<User>, counts: Option<Value> },
    /// A desktop notification was clicked.
    NotificationClicked(super::notify::Target),
    /// A person looked up with `users.info`.
    UserInfo(User),
    /// Public channels, including those we are not in.
    Channels(Vec<slack_core::types::Conversation>),
    /// `users.channelSections.list`.
    Sections(Value),
    /// A channel we joined or created.
    Joined { channel: slack_core::types::Conversation, created: bool },
    /// We left a channel.
    Left(String),
    /// Starring or unstarring failed: put the old state back.
    StarFailed { channel: String, starred: bool, error: String },
    /// A page of history; `older` pages are prepended.
    History { channel: String, page: HistoryPage, older: bool },
    /// Our own message, as returned by chat.postMessage.
    Posted { channel: String, message: slack_core::types::Message },
    /// All replies of a thread (parent first).
    Replies { channel: String, ts: String, page: HistoryPage },
    /// Conversation details loaded for the F2 overlay.
    DetailsInfo(slack_core::types::Conversation),
    DetailsMembers { channel: String, members: Vec<String> },
    DetailsPins { channel: String, pins: Vec<slack_core::types::Message> },
    /// Search results for a query.
    SearchDone { query: String, page: u32, result: Result<serde_json::Value, String> },
    /// Huddle credentials from `rooms.join`.
    HuddleJoined { channel: String, thread: Option<String>, result: Result<huddle::signaling::Credentials, String> },
    /// Our screen share started (the user chose what to share), or why not.
    ScreenShared(Result<super::call::ScreenShare, String>),
    /// A DM opened from the quick switcher.
    DmOpened(slack_core::types::Conversation),
    /// A video downloaded to a temporary file, ready to play.
    VideoReady { title: String, result: Result<std::path::PathBuf, String> },
    /// Result of a file save or upload, shown in the status bar.
    FileDone(String),
    /// The workspace's custom emoji (`emoji.list`): name → picture URL or `alias:name`.
    Emoji(std::collections::HashMap<String, String>),
    /// The workspace's slash commands (`commands.list`).
    Commands(Vec<slack_core::types::SlashCommand>),
    /// Sending a composer's attachments failed: its draft (keyed like `App::drafts`) goes back.
    SendFailed { thread: bool, key: String, draft: Box<super::composer::Draft>, error: String },
    /// A pasted picture, encoded for a composer (the reply box's when `thread`).
    Pasted { thread: bool, result: Result<super::composer::Attachment, String> },
    /// `dnd.info` asked again after the known quiet-hours window ended.
    Dnd(Value),
    /// User prefs fetched again.
    Prefs(Value),
    /// The activity feed.
    ActivityFeed(Result<Value, String>),
    /// The newest page of a previewed channel, fetched again.
    PreviewPage { channel: String, page: slack_core::types::HistoryPage },
    /// Bytes of an upload sent so far.
    UploadProgress { name: String, sent: u64, total: u64 },
    /// A page of the Threads view (`more`: added to what is listed).
    ThreadView { result: Result<Value, String>, more: bool },
    /// A message an activity item is about.
    ActivityMessage { channel: String, ts: String, message: Option<slack_core::types::Message> },
    /// A snippet's full text for the snippet view.
    SnippetText { id: String, result: Result<String, String> },
    /// A full-size image for the viewer.
    FullImage { id: String, result: Result<image::DynamicImage, String> },
    /// A thumbnail finished loading.
    Thumb { id: String, result: Result<ratatui_image::sliced::SlicedProtocol, String> },
    /// A permalink to copy.
    Permalink(String),
    /// An optimistic change failed: put the original message back.
    Restore { channel: String, message: slack_core::types::Message, error: String },
    Error(String),
}

enum Confirm {
    Delete { channel: String, ts: String },
    Leave(String),
    DeleteSection { id: String, name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    /// The huddle view's tiles (while it is shown).
    Huddle,
    Messages,
    Thread,
    ThreadComposer,
    Composer,
}

struct App {
    /// Key bindings (defaults plus keys.toml).
    keys: Keymap,
    /// Keys of a sequence typed so far (e.g. the leader).
    pending: Pending,
    /// When the last key of the pending sequence was pressed (which-key opens a while after).
    pending_since: Option<std::time::Instant>,
    /// The which-key panel, once it is shown for the pending sequence.
    which_key: Option<super::which_key::Panel>,
    /// Problems found in keys.toml, shown once the workspace has loaded.
    key_warnings: Vec<String>,
    /// The profile's workspaces (labels) and which one this is.
    workspaces: Vec<String>,
    /// Unread state of the other workspaces (polled in the background).
    others: super::others::Badges,
    /// The workspace rail's tiles at the last draw (none without a rail).
    rail: Vec<ratatui::layout::Rect>,
    /// Workspace logos for the rail, and those it wanted at the last draw.
    logos: super::others::Logos,
    rail_wanted: Vec<(slack_core::types::File, (u16, u16))>,
    /// A notification clicked in another workspace, opened once this one has booted.
    open_after: super::others::OpenAfter,
    workspace: usize,
    /// Leave this workspace for another (index into `workspaces`).
    switch_to: Option<usize>,
    /// The huddle we are in.
    call: Option<super::call::Call>,
    /// Presence subscription sent on the current RTM connection.
    presence_subscribed: bool,
    /// Open conversation details (F2).
    details: Option<super::details::Details>,
    /// Open profile card.
    profile: Option<super::profile::ProfileCard>,
    /// A `dnd.info` refresh is on its way.
    dnd_refreshing: bool,
    /// Open context menu (right-click).
    menu: Option<super::menu::Menu>,
    /// Open preferences (F6).
    prefs: Option<super::prefs::Prefs>,
    /// The huddle view (F8), shown in place of the messages while Some.
    huddle_view: Option<super::huddle_view::HuddleView>,
    /// Where the call bar was drawn (a click toggles the huddle view).
    call_bar_area: ratatui::layout::Rect,
    /// A profile picture the call bar wants loaded.
    call_bar_wanted: Option<(slack_core::types::File, (u16, u16))>,
    /// Open activity view (F5).
    activity: Option<super::activity::Activity>,
    /// The save dialog (`s`).
    save: Option<super::save::SaveDialog>,
    /// Open Threads view (Ctrl+T).
    threads: Option<super::threads::Threads>,
    /// When the previewed channel was last fetched again.
    preview_polled: Option<std::time::Instant>,
    /// Open "move to section" picker.
    mover: Option<super::sections::MovePicker>,
    /// Join the huddle of the DM with this person once it is open.
    huddle_after_open: Option<String>,
    /// A huddle that ended before it connected was joined again (once).
    rejoined: bool,
    /// What the open composers held at the last look, since when, and whether that is saved.
    autosave: (Option<super::composer::Draft>, Option<super::composer::Draft>, std::time::Instant, bool),
    /// Open search overlay (Ctrl+F).
    search: Option<super::search::Search>,
    /// A search hit to select once its conversation is shown.
    pending_hit: Option<super::search::Hit>,
    /// A message (channel, top-level ts) older than the loaded history, being paged back to.
    seeking: Option<(String, String)>,
    /// A huddle invitation ringing now.
    ringing: Option<Ring>,
    /// What was typed in conversations (by id) and threads (by thread key) you left.
    drafts: super::drafts::Drafts,
    /// Open quick switcher (Ctrl+K).
    switcher: Option<super::switcher::Switcher>,
    /// Key help overlay is open.
    help: Option<super::help::Help>,
    /// Open video player.
    player: Option<super::video::Player>,
    /// Whole terminal area on the last draw (to size video frames).
    screen: ratatui::layout::Rect,
    /// Open image viewer.
    viewer: Option<super::viewer::Viewer>,
    /// Open snippet view.
    snippet: Option<super::snippet::SnippetView>,
    media: super::media::Media,
    /// The terminal window has focus (focus-change reporting).
    terminal_focused: bool,
    /// Open @/#/: suggestion list.
    completion: Option<super::autocomplete::Completion>,
    /// Suggestions closed with Esc for this (composer, token start) until the token changes.
    completion_dismissed: Option<(bool, usize)>,
    /// Threads and activity items opened in this run, for lists Slack has not updated yet.
    seen: super::threads::Seen,
    /// The last `/upload` path looked up and what its folder offered (kept while it is typed).
    path_suggestions: Option<(super::composer::Trigger, Vec<super::autocomplete::Suggestion>)>,
    /// Composer boxes on the last draw (to anchor the suggestion list).
    composer_area: ratatui::layout::Rect,
    thread_composer_area: ratatui::layout::Rect,
    /// Open thread pane: (channel, parent ts).
    thread: Option<(String, String)>,
    /// At the last draw there was no room for a thread beside the messages: an open thread
    /// takes their place.
    narrow: bool,
    thread_pane: MessagePane,
    thread_composer: Composer,
    /// Thread replies are also sent to the channel.
    broadcast: bool,
    clipboard: super::clipboard::Clipboard,
    /// An action awaiting y/n confirmation.
    confirm: Option<Confirm>,
    /// Where the y/n prompt was drawn.
    confirm_area: Rect,
    /// Open reaction picker, with the message it targets.
    picker: Option<(EmojiPicker, String, String)>,
    rtm_commands: mpsc::UnboundedSender<slack_core::rtm::RtmCommand>,
    connected: bool,
    last_typing_sent: Option<std::time::Instant>,
    /// Newest ts we already marked read, per channel.
    marked: std::collections::HashMap<String, String>,
    focus: Focus,
    composer: Composer,
    client: Client,
    net: mpsc::UnboundedSender<Net>,
    ws: Workspace,
    sidebar: Sidebar,
    messages: MessagePane,
    /// Conversation currently shown in the message pane.
    current: Option<String>,
    status: String,
    /// The status as last seen and since when, to clear it after [`STATUS_TTL`].
    status_shown: (String, std::time::Instant),
    loading: bool,
}

/// How long a status-bar message stays. One ending in "…" stays until it is replaced: it
/// describes something still going on (loading, uploading, joining, a dialog open).
const STATUS_TTL: Duration = Duration::from_secs(8);

/// What a prefilled command does with a draft in the message box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wrap {
    /// Refuse while the box holds text.
    No,
    /// The draft follows on the same line (`/upload <path> text`).
    Caption,
    /// The draft goes on the lines below (`/snippet` + its body).
    Body,
}

/// The message box's text and cursor with `command` put in front of `text` (see [`Wrap`]),
/// the cursor right after the command; an error note when it cannot be done.
fn prefilled(text: &str, editing: bool, command: &str, wrap: Wrap) -> Result<(String, usize), &'static str> {
    let cursor = command.chars().count();
    if editing {
        return Err("finish or cancel the edit first");
    }
    match wrap {
        _ if text.trim().is_empty() => Ok((command.to_string(), cursor)),
        Wrap::No => Err("the message box is in use"),
        Wrap::Caption => Ok((format!("{command} {text}"), cursor)),
        Wrap::Body => Ok((format!("{command}\n{text}"), cursor)),
    }
}

/// Whether a status shown for `age` should go.
fn status_expired(status: &str, age: Duration) -> bool {
    !status.is_empty() && !status.ends_with('…') && age >= STATUS_TTL
}

impl App {
    /// Fetch history for `channel`: the newest page, or the page before the oldest loaded.
    fn load_history(&mut self, channel: &str, older: bool) {
        let tl = self.ws.timelines.entry(channel.to_string()).or_default();
        if tl.loading || (older && (!tl.has_more || !tl.loaded)) || (!older && tl.loaded) {
            return;
        }
        tl.loading = true;
        let latest = if older { tl.messages.first().map(|m| m.ts.clone()) } else { None };
        let (client, tx, channel) = (self.client.clone(), self.net.clone(), channel.to_string());
        tokio::spawn(async move {
            let msg = match client.history(&channel, latest.as_deref(), 50).await {
                Ok(page) => Net::History { channel, page, older },
                Err(e) => Net::Error(format!("history failed: {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    /// Follow the sidebar selection into the message pane.
    fn sync_current(&mut self) {
        let selected = self.sidebar.selected_id().map(String::from);
        // On a folded section's header (or the Threads row) the conversation stays as it was.
        if selected.is_none() && (self.sidebar.selected_folded().is_some() || self.sidebar.on_threads()) {
            return;
        }
        if selected != self.current {
            if self.ws.preview.is_some() && self.ws.preview != selected {
                self.ws.preview = None;
                self.sidebar.rebuild(&self.ws);
            }
            if let Some(old) = &self.current {
                stash(&mut self.drafts, old, &mut self.composer);
            }
            if let Some(d) = selected.as_ref().and_then(|id| self.drafts.remove(id)) {
                self.composer.restore(d);
            }
            self.save_drafts();
            self.current = selected.clone();
            if let Some(id) = &selected {
                self.sidebar.remember(id);
            }
            // A quiet conversation left behind leaves the sidebar again.
            if self.ws.shown != selected {
                self.ws.shown = selected.clone();
                self.sidebar.rebuild(&self.ws);
            }
            // Where the unread part starts, kept while the conversation stays open (it is
            // marked read right away).
            self.messages.new_since = selected.as_ref().filter(|id| self.ws.unread.get(*id).is_some_and(|u| u.has_unreads)).and_then(|id| self.ws.conversations.get(id)).and_then(|c| c.last_read.clone());
            self.messages.jump_to_new = self.messages.new_since.is_some();
            self.messages.to_bottom();
            self.messages.selected = None;
            self.close_thread();
            if let Some(hit) = self.pending_hit.take().filter(|h| Some(&h.channel) == selected.as_ref()) {
                self.show_hit(hit);
            }
            if let Some(id) = selected {
                self.load_history(&id, false);
            }
        }
    }

    /// Mark the current conversation read up to its newest message.
    fn mark_current_read(&mut self) {
        let Some(channel) = self.current.clone().filter(|c| !self.ws.is_outside(c)) else { return };
        let Some(latest) = self.ws.timelines.get(&channel).and_then(|t| t.messages.last()).map(|m| m.ts.clone()) else { return };
        if self.marked.get(&channel) == Some(&latest) {
            return;
        }
        self.marked.insert(channel.clone(), latest.clone());
        if let Some(u) = self.ws.unread.get_mut(&channel) {
            u.has_unreads = false;
            u.mentions = 0;
        }
        let client = self.client.clone();
        tokio::spawn(async move {
            if let Err(e) = client.mark(&channel, &latest).await {
                tracing::warn!("mark read failed: {e}");
            }
        });
    }

    /// A channel being previewed gets no live events: fetch its newest page again every
    /// `preview_refresh_secs` while it is shown.
    fn refresh_preview(&mut self) {
        let secs = self.ws.settings.messages.preview_refresh_secs;
        let Some(channel) = self.ws.preview.clone().filter(|p| secs > 0 && self.current.as_ref() == Some(p)) else {
            self.preview_polled = None;
            return;
        };
        if !self.ws.timelines.get(&channel).is_some_and(|t| t.loaded && !t.loading) {
            return;
        }
        match self.preview_polled {
            Some(at) if at.elapsed() < Duration::from_secs(secs as u64) => return,
            None => {
                // Just loaded: the first refresh after a full interval.
                self.preview_polled = Some(std::time::Instant::now());
                return;
            }
            Some(_) => self.preview_polled = Some(std::time::Instant::now()),
        }
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            match client.history(&channel, None, 50).await {
                Ok(page) => {
                    let _ = tx.send(Net::PreviewPage { channel, page });
                }
                Err(e) => tracing::debug!("refreshing the previewed channel: {e}"),
            }
        });
    }

    /// Mark the open thread read up to its newest reply (once per new reply).
    fn mark_thread_read(&mut self) {
        let Some((channel, parent)) = self.thread.clone().filter(|(c, _)| !self.ws.is_outside(c)) else { return };
        let key = super::state::thread_key(&channel, &parent);
        let Some(latest) = self.ws.threads.get(&key).and_then(|t| thread_mark_target(t, &parent)) else { return };
        if self.marked.get(&key) == Some(&latest) {
            return;
        }
        self.marked.insert(key, latest.clone());
        let client = self.client.clone();
        tokio::spawn(async move {
            if let Err(e) = client.mark_thread(&channel, &parent, &latest).await {
                tracing::warn!("marking the thread read failed: {e}");
            }
        });
    }

    /// Toggle our reaction `name` on message `ts` in `channel` (optimistic, then API).
    fn toggle_reaction(&mut self, channel: String, ts: String, name: String) {
        let me = self.ws.self_id.clone();
        let Some(had) = self.ws.find_message(&channel, &ts).map(|m| m.reactions.iter().any(|r| r.name == name && r.users.contains(&me))) else { return };
        for msg in self.ws.copies_mut(&channel, &ts) {
            match msg.reactions.iter().position(|r| r.name == name) {
                Some(i) if had => {
                    let r = &mut msg.reactions[i];
                    r.users.retain(|u| *u != me);
                    r.count = r.count.saturating_sub(1);
                    if r.count == 0 {
                        msg.reactions.remove(i);
                    }
                }
                Some(i) => {
                    msg.reactions[i].users.push(me.clone());
                    msg.reactions[i].count += 1;
                }
                None => msg.reactions.push(slack_core::types::Reaction { name: name.clone(), users: vec![me.clone()], count: 1 }),
            }
        }
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            if let Err(e) = client.react(&channel, &ts, &name, !had).await {
                let _ = tx.send(Net::Error(format!("reaction failed: {e}")));
            }
        });
    }

    /// Tell the other side we are typing (at most every 3 s), in the thread from the reply box.
    fn notify_typing(&mut self, thread: bool) {
        let (channel, thread_ts) = match (thread, self.thread.clone()) {
            (true, Some((channel, ts))) => (channel, Some(ts)),
            _ => match self.current.clone() {
                Some(c) => (c, None),
                None => return,
            },
        };
        if self.last_typing_sent.is_some_and(|t| t.elapsed().as_secs() < 3) {
            return;
        }
        self.last_typing_sent = Some(std::time::Instant::now());
        let _ = self.rtm_commands.send(slack_core::rtm::RtmCommand::Typing { channel, thread_ts });
    }

    /// The channel composer, or the thread pane's (`thread`).
    fn composer_for(&mut self, thread: bool) -> &mut Composer {
        if thread { &mut self.thread_composer } else { &mut self.composer }
    }

    /// Send a composer's contents: the channel composer posts to the current conversation,
    /// the thread composer replies in the open thread (optionally also to the channel).
    fn send(&mut self, thread: bool) {
        let Some(channel) = self.current.clone() else { return };
        let thread_ts = if thread { self.thread.as_ref().map(|(_, ts)| ts.clone()) } else { None };
        if thread && thread_ts.is_none() {
            return;
        }
        let broadcast = thread && self.broadcast;
        let outside = self.ws.is_outside(&channel).then(|| self.ws.channel_label(&channel));
        let c = self.composer_for(thread);
        if !c.attachments.is_empty() {
            match outside {
                Some(name) => self.status = format!("you are not in #{name} — /join to post"),
                None => self.send_files(channel, thread, thread_ts),
            }
            return;
        }
        if c.is_empty() {
            return;
        }
        match super::commands::parse(&c.text(), &c.to_mrkdwn()) {
            Some(Ok(cmd)) => {
                c.push_history(c.text());
                c.clear();
                self.run_command(channel, cmd, thread_ts);
                return;
            }
            Some(Err(e)) => {
                self.status = e;
                return;
            }
            None if outside.is_some() => {
                self.status = format!("you are not in #{} — /join to post", outside.unwrap_or_default());
                return;
            }
            None => {}
        }
        let text = c.to_mrkdwn();
        c.push_history(c.text());
        c.clear();
        if thread {
            self.thread_pane.to_bottom();
        } else {
            self.messages.to_bottom();
        }
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let msg = match client.post_message(&channel, &text, thread_ts.as_deref(), broadcast).await {
                Ok(message) => Net::Posted { channel, message },
                Err(e) => Net::Error(format!("send failed: {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    /// Send a composer's attachments with its text (if any) as one message, as Slack's own
    /// client does with pasted files; on failure they go back in the composer.
    fn send_files(&mut self, channel: String, thread: bool, thread_ts: Option<String>) {
        use super::composer::AttachmentData;
        use slack_core::api::{Upload, UploadSource};
        let key = match &thread_ts {
            Some(ts) => super::state::thread_key(&channel, ts),
            None => channel.clone(),
        };
        let c = self.composer_for(thread);
        if c.attachments.len() > MAX_FILES {
            self.status = format!("Slack takes at most {MAX_FILES} files in one message (Backspace removes the last)");
            return;
        }
        let Some(draft) = c.draft() else { return };
        let text = c.to_mrkdwn();
        let items: Vec<Upload> = c
            .attachments
            .iter()
            .map(|a| Upload {
                name: a.name.clone(),
                source: match &a.data {
                    AttachmentData::File(path) => UploadSource::Path(path.clone()),
                    AttachmentData::Png(png) => UploadSource::Bytes(png.to_vec()),
                },
            })
            .collect();
        let label = match items.as_slice() {
            [one] => one.name.clone(),
            many => format!("{} files", many.len()),
        };
        if !c.is_empty() {
            c.push_history(c.text());
        }
        c.clear();
        if thread {
            self.thread_pane.to_bottom();
        } else {
            self.messages.to_bottom();
        }
        self.status = format!("uploading {label}…");
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let progress = upload_progress(tx.clone(), label.clone());
            let _ = tx.send(match client.upload_files(&channel, items, Some(&text), thread_ts.as_deref(), progress).await {
                Ok(_) => Net::FileDone(format!("sent {label}")),
                Err(e) => Net::SendFailed { thread, key, draft: Box::new(draft), error: format!("sending {label} failed: {e}") },
            });
        });
    }

    fn run_command(&mut self, channel: String, cmd: super::commands::Command, thread_ts: Option<String>) {
        use super::commands::Command;
        match cmd {
            Command::Slack { command, text } => {
                let (client, tx) = (self.client.clone(), self.net.clone());
                tokio::spawn(async move {
                    let msg = match client.slash_command(&channel, &command, &text, thread_ts.as_deref()).await {
                        Ok(()) => return,
                        Err(e) if e.to_string().contains("unknown_command") => format!("{command} is not a command Slack knows (start with a space to send it as text)"),
                        Err(e) => format!("{command} failed: {e}"),
                    };
                    let _ = tx.send(Net::Error(msg));
                });
            }
            Command::Upload(cmd) => self.upload(channel, cmd, thread_ts),
            Command::Join(None) => self.join_channel(channel),
            Command::Join(Some(name)) => match self.ws.conversations.values().find(|c| !c.is_dm() && c.name == name) {
                Some(c) if c.is_member => {
                    let id = c.id.clone();
                    self.jump_to(&id);
                }
                Some(c) => self.join_channel(c.id.clone()),
                None => self.status = format!("no channel #{name} (private channels need an invitation)"),
            },
            Command::Leave => match self.ws.conversations.get(&channel) {
                Some(c) if c.is_dm() => self.status = "/leave works in channels; direct messages cannot be left".into(),
                Some(c) if !c.is_member => self.status = format!("you are not in #{}", c.name),
                Some(_) => self.confirm = Some(Confirm::Leave(channel)),
                None => {}
            },
            Command::Dm(people) => self.open_group(people),
            Command::Star => self.toggle_star(channel),
            Command::Prefs => self.prefs = Some(super::prefs::Prefs::new()),
            Command::Workspace(None) => {
                let list: Vec<String> = self.workspaces.iter().enumerate().map(|(n, w)| format!("{}{} {w}", if n == self.workspace { "▸" } else { "" }, n + 1)).collect();
                self.status = format!("workspaces (Alt+number): {}", list.join(" · "));
            }
            Command::Workspace(Some(name)) => {
                let name = name.to_lowercase();
                match self.workspaces.iter().position(|w| w.to_lowercase().contains(&name)) {
                    Some(i) => self.switch_workspace(i),
                    None => self.status = format!("no workspace matching '{name}' in this profile"),
                }
            }
            Command::Status { emoji, text, expiry } => {
                use super::commands::Expiry;
                let now = super::clock::now();
                let expiration = match expiry {
                    Expiry::Never => 0,
                    Expiry::Minutes(m) => now.timestamp() + m as i64 * 60,
                    Expiry::Today => (now.date_naive() + chrono::Days::new(1)).and_hms_opt(0, 0, 0).and_then(|t| t.and_local_timezone(*now.offset()).earliest()).map_or(0, |t| t.timestamp()),
                };
                let done = if text.is_empty() && emoji.is_empty() { "status cleared".to_string() } else { "status set".to_string() };
                self.spawn_simple(done, async move |client: Client| client.set_status(&emoji, &text, expiration).await);
            }
            Command::Away(away) => {
                let done = if away { "you appear away" } else { "you appear active" };
                self.spawn_simple(done.into(), async move |client: Client| client.set_presence(away).await);
            }
            Command::Dnd(minutes) => {
                let done = if minutes.is_some() { "notifications paused" } else { "notifications resumed" };
                self.spawn_simple(done.into(), async move |client: Client| client.snooze(minutes).await);
            }
            Command::SectionRename(name) => match self.ws.section_of(&channel).map(|s| s.id.clone()) {
                Some(id) => self.rename_section(id, name),
                None => self.status = "this conversation is not in a section of yours (m in the sidebar)".into(),
            },
            Command::SectionDelete => match self.ws.section_of(&channel) {
                Some(s) => self.confirm = Some(Confirm::DeleteSection { id: s.id.clone(), name: s.name.clone() }),
                None => self.status = "this conversation is not in a section of yours".into(),
            },
            Command::Snippet { lang, title, body } => {
                let name = if title.is_empty() { "snippet".to_string() } else { title };
                self.status = format!("posting {name}…");
                let (client, tx) = (self.client.clone(), self.net.clone());
                tokio::spawn(async move {
                    let result = client.upload(&channel, &name, body.into_bytes(), None, thread_ts.as_deref(), Some(&lang)).await;
                    let _ = tx.send(Net::FileDone(match result {
                        Ok(_) => format!("posted snippet {name}"),
                        Err(e) => format!("snippet failed: {e}"),
                    }));
                });
            }
            Command::Create { name, private } => {
                self.status = format!("creating #{name}…");
                let (client, tx) = (self.client.clone(), self.net.clone());
                tokio::spawn(async move {
                    let _ = tx.send(match client.create_channel(&name, private).await {
                        Ok(channel) => Net::Joined { channel, created: true },
                        Err(e) => Net::Error(format!("creating #{name} failed: {e}")),
                    });
                });
            }
        }
    }

    /// `/dm`: the DM with one person, or a group DM with several.
    fn open_group(&mut self, people: Vec<super::commands::Recipient>) {
        use super::commands::Recipient;
        let mut ids = Vec::new();
        for p in people {
            let id = match p {
                Recipient::Id(id) => Some(id),
                Recipient::Name(name) => {
                    let name = name.to_lowercase();
                    let live = || self.ws.users.values().filter(|u| !u.deleted);
                    live().find(|u| u.name.to_lowercase() == name).or_else(|| live().find(|u| u.display().to_lowercase() == name)).map(|u| u.id.clone())
                }
            };
            match id {
                Some(id) if id == self.ws.self_id || ids.contains(&id) => {}
                Some(id) => ids.push(id),
                None => {
                    self.status = "no such person (use @ to pick from the list)".into();
                    return;
                }
            }
        }
        match ids.len() {
            0 => self.status = "name someone besides yourself".into(),
            1 => self.open_dm_with(ids.remove(0)),
            _ => {
                self.status = "opening the group DM…".into();
                let (client, tx) = (self.client.clone(), self.net.clone());
                tokio::spawn(async move {
                    let users: Vec<&str> = ids.iter().map(String::as_str).collect();
                    let _ = tx.send(match client.open_dm(&users).await {
                        Ok(mut c) => {
                            c.is_mpim = true;
                            Net::DmOpened(c)
                        }
                        Err(e) => Net::Error(format!("opening the group DM failed: {e}")),
                    });
                });
            }
        }
    }

    /// Star or unstar a conversation (optimistic, then `stars.add/remove`).
    fn toggle_star(&mut self, channel: String) {
        if self.ws.is_outside(&channel) {
            self.status = "join the channel to star it".into();
            return;
        }
        let star = !self.ws.starred.contains(&channel);
        if star {
            self.ws.starred.insert(channel.clone());
        } else {
            self.ws.starred.remove(&channel);
        }
        self.sidebar.rebuild(&self.ws);
        self.status = format!("{} {}", if star { "starred" } else { "unstarred" }, self.ws.channel_label(&channel));
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            if let Err(e) = client.star_channel(&channel, star).await {
                let _ = tx.send(Net::StarFailed { channel, starred: !star, error: e.to_string() });
            }
        });
    }

    /// Act on the move picker's choice (or create the named section and move there).
    fn finish_move(&mut self) {
        use super::sections::Choice;
        let Some(picker) = self.mover.take() else { return };
        let channel = picker.channel.clone();
        let from = self.ws.section_of(&channel).map(|s| s.id.clone());
        let (client, tx) = (self.client.clone(), self.net.clone());
        if let Some(name) = picker.naming.as_ref().map(|n| n.trim().to_string()) {
            if name.is_empty() {
                self.status = "a section needs a name".into();
                return;
            }
            self.status = format!("creating the section {name}…");
            tokio::spawn(async move {
                let result = async {
                    let id = client.create_section(&name).await?;
                    client.move_to_section(&channel, from.as_deref(), Some(&id)).await
                }
                .await;
                let _ = tx.send(match result {
                    Ok(()) => Net::FileDone(format!("moved to the new section {name}")),
                    Err(e) => Net::Error(format!("creating the section failed: {e}")),
                });
            });
            return;
        }
        let to = match picker.choice() {
            Choice::NoSection => None,
            Choice::Section { id, .. } => Some(id.clone()),
            Choice::New => return,
        };
        if to == from {
            return;
        }
        match &to {
            Some(id) => self.ws.place_in_section(id, std::slice::from_ref(&channel)),
            None => {
                if let Some(f) = &from {
                    self.ws.remove_from_section(f, std::slice::from_ref(&channel));
                }
            }
        }
        self.sidebar.rebuild(&self.ws);
        tokio::spawn(async move {
            if let Err(e) = client.move_to_section(&channel, from.as_deref(), to.as_deref()).await {
                let _ = tx.send(Net::Error(format!("moving failed: {e}")));
                if let Ok(v) = client.channel_sections().await {
                    let _ = tx.send(Net::Sections(v));
                }
            }
        });
    }

    fn rename_section(&mut self, id: String, name: String) {
        if let Some(s) = self.ws.sections.iter_mut().find(|s| s.id == id) {
            s.name = name.clone();
        }
        self.sidebar.rebuild(&self.ws);
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            if let Err(e) = client.rename_section(&id, &name).await {
                let _ = tx.send(Net::Error(format!("renaming failed: {e}")));
                if let Ok(v) = client.channel_sections().await {
                    let _ = tx.send(Net::Sections(v));
                }
            }
        });
    }

    fn delete_section(&mut self, id: String) {
        self.ws.remove_section(&id);
        self.sidebar.rebuild(&self.ws);
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            if let Err(e) = client.delete_section(&id).await {
                let _ = tx.send(Net::Error(format!("deleting the section failed: {e}")));
                if let Ok(v) = client.channel_sections().await {
                    let _ = tx.send(Net::Sections(v));
                }
            }
        });
    }

    /// Run an API call in the background; report `done` or the error in the status bar.
    fn spawn_simple<F>(&mut self, done: String, call: impl FnOnce(Client) -> F + Send + 'static)
    where
        F: std::future::Future<Output = anyhow::Result<()>> + Send,
    {
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let _ = tx.send(match call(client).await {
                Ok(()) => Net::FileDone(done),
                Err(e) => Net::Error(e.to_string()),
            });
        });
    }

    /// `␣n…`, `␣as`, `␣ap`: `command` in the conversation's message box (also from the thread
    /// pane), the cursor after it, typing there; a draft wraps as `wrap` says, or it refuses.
    fn prefill(&mut self, command: &str, wrap: Wrap) {
        if self.current.is_none() {
            self.status = "open a conversation first".into();
            return;
        }
        let c = &mut self.composer;
        // Attachments would be sent with the command as their text.
        let filled = if c.attachments.is_empty() { prefilled(&c.text(), c.editing.is_some(), command, wrap) } else { Err("the message box is in use") };
        match filled {
            Ok((text, cursor)) => {
                c.set_with_cursor(&text, cursor);
                self.focus = Focus::Composer;
            }
            Err(note) => self.status = note.into(),
        }
    }

    /// Esc in a composer: back to the message list (the reply box: the thread), the newest
    /// message selected.
    fn leave_composer(&mut self, thread: bool) {
        self.focus = if thread { Focus::Thread } else { Focus::Messages };
        let timeline = match (&self.thread, thread) {
            (Some((c, ts)), true) => self.ws.threads.get(&super::state::thread_key(c, ts)),
            _ => self.current.as_ref().and_then(|c| self.ws.timelines.get(c)),
        };
        let order: Vec<String> = timeline.map(|t| super::messages::shown_messages(t, thread).map(|m| m.ts.clone()).collect()).unwrap_or_default();
        let pane = if thread { &mut self.thread_pane } else { &mut self.messages };
        pane.selected = None;
        pane.move_selection(&order, 0);
        pane.to_bottom();
    }

    /// A messages-context action on the message pane or (`in_thread`) the thread pane;
    /// keys and the context menu both come here.
    fn message_action(&mut self, action: Action, in_thread: bool) {
                            let timeline = match (&self.thread, in_thread) {
            (Some((c, ts)), true) => self.ws.threads.get(&super::state::thread_key(c, ts)),
            _ => self.current.as_ref().and_then(|c| self.ws.timelines.get(c)),
        };
        let order: Vec<String> = timeline.map(|t| super::messages::shown_messages(t, in_thread).map(|m| m.ts.clone()).collect()).unwrap_or_default();
                            let pane = if in_thread { &mut self.thread_pane } else { &mut self.messages };
        match Some(action) {
            Some(Action::Up) => {
                pane.move_selection(&order, -1);
                // On the oldest loaded message: load older ones (the lines above it, like its
                // day separator, need not be on screen for that).
                if !in_thread && order.first() == self.messages.selected.as_ref() {
                    if let Some(id) = self.current.clone() {
                        self.load_history(&id, true);
                    }
                }
            }
            Some(Action::Down) => pane.move_selection(&order, 1),
            // The menu of the selected message (with none, the newest), where it is drawn.
            Some(Action::ContextMenu) => {
                if pane.selected.is_none() {
                    pane.move_selection(&order, 0);
                }
                if let Some(ts) = pane.selected.clone() {
                    let at = pane.spot(&ts);
                    self.message_menu(in_thread, ts, at);
                }
            }
            Some(Action::Top) => pane.move_selection(&order, isize::MIN / 2),
            Some(Action::Bottom) => pane.move_selection(&order, isize::MAX / 2),
            Some(Action::HalfPageUp | Action::HalfPageDown) => {
                let up = action == Action::HalfPageUp;
                let half = (pane.area.height.saturating_sub(2) / 2).max(1) as usize;
                // The selection goes with the page: half of the messages on screen.
                let step = (pane.shown_count() / 2).max(1) as isize;
                if up {
                    pane.scroll_up(half);
                    pane.move_selection(&order, -step);
                    if !in_thread {
                        self.maybe_load_older();
                    }
                } else {
                    pane.scroll_down(half);
                    pane.move_selection(&order, step);
                }
            }
            // Esc closes a thread; the messages pane has nothing to go back to.
            Some(Action::Back) if in_thread => self.close_thread(),
            Some(Action::Back) => {}
            Some(Action::Type) if in_thread => self.focus = Focus::ThreadComposer,
            Some(Action::Type) => self.focus = Focus::Composer,
            Some(Action::OpenThread) if !in_thread => {
                if let Some(ts) = self.messages.selected.clone() {
                    self.open_thread(&ts);
                }
            }
            Some(Action::React) => {
                if let (Some(channel), Some(ts)) = (self.current.clone(), self.selected_ts()) {
                    self.picker = Some((EmojiPicker::default(), channel, ts));
                }
            }
            Some(Action::Edit) => {
                if let Some(ts) = self.selected_ts() {
                    self.start_edit(&ts, in_thread);
                }
            }
            Some(Action::Copy) => {
                if let Some(ts) = self.selected_ts() {
                    self.copy_text(&ts);
                }
            }
            Some(Action::Pin) => {
                if let Some(ts) = self.selected_ts() {
                    self.toggle_pin(&ts);
                }
            }
            Some(Action::ThreadHuddle) => {
                let target = self.selected_ts().and_then(|ts| self.current.clone().map(|c| (c, ts)));
                match target {
                    _ if self.call.is_some() => self.status = "leave the current huddle first".into(),
                    Some((channel, ts)) => {
                        // A reply's huddle belongs to its thread.
                        let root = self.ws.find_message(&channel, &ts).and_then(|m| m.thread_ts.clone()).unwrap_or(ts);
                        self.join_huddle(channel, Some(root));
                    }
                    None => {}
                }
            }
            Some(Action::Profile) => {
                let m = self.selected_ts().and_then(|ts| self.current.as_deref().and_then(|c| self.ws.find_message(c, &ts)));
                match m.map(super::profile::people_in).and_then(super::profile::ProfileCard::new) {
                    Some(card) => self.profile = Some(card),
                    None => self.status = "no person on this message".into(),
                }
            }
            Some(Action::Save) => {
                if let Some(ts) = self.selected_ts() {
                    self.save_files(&ts);
                }
            }
            Some(Action::Open) => {
                if let Some(ts) = self.selected_ts() {
                    self.open_things(&ts, in_thread);
                }
            }
            Some(Action::CopyLink) => {
                if let Some(ts) = self.selected_ts() {
                    self.copy_permalink(&ts);
                }
            }
            Some(Action::Delete) => {
                if let Some(ts) = self.selected_ts() {
                    self.ask_delete(&ts);
                }
            }
            _ => {}
        }
    }

    /// Right-click: select what is under the pointer and offer its actions.
    fn open_menu(&mut self, column: u16, row: u16) {
        use super::menu::{Menu, Target};
        let at = (column, row);
        let panes = [(false, self.messages.message_at(column, row)), (true, if self.thread.is_some() { self.thread_pane.message_at(column, row) } else { None })];
        if let Some((thread, ts)) = panes.into_iter().find_map(|(t, ts)| ts.map(|ts| (t, ts))) {
            self.message_menu(thread, ts, at);
        } else if let Some(n) = self.sidebar.item_at(column, row) {
            self.sidebar.selected = n;
            let id = self.sidebar.selected_id().unwrap_or_default().to_string();
            let channel = self.ws.conversations.get(&id).is_some_and(|c| !c.is_dm() && c.is_member);
            let none = super::menu::Offers { own: false, open: false, save: false, text: false };
            self.menu = Some(Menu::new(Target::Conversation, at, &self.keys, none, channel, self.ws.starred.contains(&id)));
        }
    }

    /// The context menu of a message (selected now, its pane focused), at `at`.
    fn message_menu(&mut self, thread: bool, ts: String, at: (u16, u16)) {
        use super::menu::{Menu, Target};
        let channel = self.current.clone().unwrap_or_default();
        let Some(offers) = self.ws.find_message(&channel, &ts).map(|m| super::menu::Offers::of(m, &self.ws.self_id)) else { return };
        if thread {
            self.thread_pane.selected = Some(ts);
            self.focus = Focus::Thread;
        } else {
            self.messages.selected = Some(ts);
            self.focus = Focus::Messages;
        }
        self.menu = Some(Menu::new(Target::Message { thread }, at, &self.keys, offers, false, false));
    }

    /// Run the context menu's chosen item, like its key would.
    fn run_menu(&mut self) {
        use super::menu::{Item, Target};
        let Some(menu) = self.menu.take() else { return };
        let Some(item) = menu.chosen() else { return };
        if menu.target == Target::Conversation {
            // The clicked conversation becomes the current one first.
            self.sync_current();
        }
        match (menu.target, item) {
            (Target::Message { thread }, Item::Do(action)) => {
                self.focus = if thread { Focus::Thread } else { Focus::Messages };
                self.message_action(action, thread);
            }
            (Target::Conversation, Item::Do(action)) => self.sidebar_action(action),
            (Target::Opener { thread, ts }, Item::Thing(n)) => {
                self.focus = if thread { Focus::Thread } else { Focus::Messages };
                if let Some(thing) = menu.things.get(n).cloned() {
                    self.open_thing(&ts, thing);
                }
            }
            (_, Item::Thing(_)) | (Target::Opener { .. }, _) => {}
            (_, Item::Details) => self.open_details(),
            (_, Item::Leave) => {
                if let Some(id) = self.current.clone() {
                    self.confirm = Some(Confirm::Leave(id));
                }
            }
        }
    }

    /// A sidebar-context action on the selected conversation.
    fn sidebar_action(&mut self, action: Action) {
        match action {
            Action::Up => self.sidebar.move_by(-1),
            Action::Down => self.sidebar.move_by(1),
            Action::Top => self.sidebar.move_by(isize::MIN / 2),
            Action::Bottom => self.sidebar.move_by(isize::MAX / 2),
            Action::HalfPageUp => self.sidebar.move_by(-((self.sidebar.area.height.saturating_sub(2) / 2).max(1) as isize)),
            Action::HalfPageDown => self.sidebar.move_by((self.sidebar.area.height.saturating_sub(2) / 2).max(1) as isize),
            Action::Open if self.sidebar.selected_folded().is_some() => self.sidebar.toggle_selected_fold(&self.ws, self.current.as_deref()),
            Action::Open if self.sidebar.on_threads() => self.load_threads(false),
            Action::Open | Action::Type => self.focus = Focus::Composer,
            // The selected conversation is the current one.
            Action::Details | Action::NotificationLevel | Action::LeaveConversation if self.sidebar.selected_id().is_some() => {
                run_global(self, action);
            }
            Action::FoldSection => self.sidebar.toggle_selected_fold(&self.ws, self.current.as_deref()),
            Action::Star => {
                if let Some(id) = self.sidebar.selected_id().map(String::from) {
                    self.toggle_star(id);
                }
            }
            Action::MoveToSection => match self.sidebar.selected_id().map(String::from) {
                Some(id) if self.ws.is_outside(&id) => self.status = "join the channel to file it in a section".into(),
                Some(id) => self.mover = Some(super::sections::MovePicker::new(&self.ws, id)),
                None => {}
            },
            _ => {}
        }
    }

    fn join_channel(&mut self, channel: String) {
        if !self.ws.is_outside(&channel) {
            self.status = "you are already in this conversation".into();
            return;
        }
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let _ = tx.send(match client.join(&channel).await {
                Ok(channel) => Net::Joined { channel, created: false },
                Err(e) => Net::Error(format!("joining failed: {e}")),
            });
        });
    }

    fn leave_channel(&mut self, channel: String) {
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let _ = tx.send(match client.leave(&channel).await {
                Ok(()) => Net::Left(channel),
                Err(e) => Net::Error(format!("leaving failed: {e}")),
            });
        });
    }

    /// Load our own message `ts` in the current conversation into a composer for editing.
    fn start_edit(&mut self, ts: &str, thread: bool) {
        let Some(channel) = self.current.clone() else { return };
        let Some(m) = self.ws.find_message(&channel, ts) else { return };
        if m.user.as_deref() != Some(self.ws.self_id.as_str()) || super::messages::is_system(m) {
            self.status = "you can only edit your own messages".into();
            return;
        }
        let ws = &self.ws;
        let user_name = |id: &str| ws.user_name(id);
        let channel_name = |id: &str| ws.channel_label(id);
        let c = if thread { &mut self.thread_composer } else { &mut self.composer };
        c.load_for_edit(ts, &m.text, user_name, channel_name);
        if thread {
            self.thread_pane.selected = Some(ts.to_string());
            self.focus = Focus::ThreadComposer;
        } else {
            self.messages.selected = Some(ts.to_string());
            self.focus = Focus::Composer;
        }
        self.status.clear();
    }

    /// Our newest message in the current conversation (top level) or the open thread.
    fn last_own_message(&self, thread: bool) -> Option<String> {
        let t = match (thread, &self.thread) {
            (true, Some((c, ts))) => self.ws.threads.get(&super::state::thread_key(c, ts))?,
            (true, None) => return None,
            (false, _) => self.ws.timelines.get(self.current.as_deref()?)?,
        };
        let me = Some(self.ws.self_id.as_str());
        super::messages::shown_messages(t, thread).filter(|m| m.user.as_deref() == me && (m.subtype.is_none() || m.subtype.as_deref() == Some("thread_broadcast"))).last().map(|m| m.ts.clone())
    }

    /// Save the message being edited in a composer (optimistic, then chat.update).
    fn save_edit(&mut self, thread: bool) {
        let Some(channel) = self.current.clone() else { return };
        let c = self.composer_for(thread);
        let Some(ts) = c.editing.clone() else { return };
        if c.is_empty() {
            self.status = "empty edit: use d on the message to delete it".into();
            return;
        }
        let text = c.to_mrkdwn();
        c.clear();
        let Some(original) = self.ws.find_message(&channel, &ts).cloned() else { return };
        let me = self.ws.self_id.clone();
        let now = format!("{:.6}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64());
        for msg in self.ws.copies_mut(&channel, &ts) {
            msg.text = text.clone();
            msg.blocks.clear();
            msg.edited = Some(slack_core::types::Edited { user: me.clone(), ts: now.clone() });
        }
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            if let Err(e) = client.update_message(&channel, &ts, &text).await {
                let _ = tx.send(Net::Restore { channel, message: original, error: format!("edit failed: {e}") });
            }
        });
    }

    /// Ask to delete our own message `ts` in the current conversation.
    fn ask_delete(&mut self, ts: &str) {
        let Some(channel) = self.current.clone() else { return };
        let own = self.ws.find_message(&channel, ts).is_some_and(|m| m.user.as_deref() == Some(self.ws.self_id.as_str()) && !super::messages::is_system(m));
        if own {
            self.confirm = Some(Confirm::Delete { channel, ts: ts.to_string() });
        } else {
            self.status = "you can only delete your own messages".into();
        }
    }

    /// Delete a message (optimistic, then chat.delete).
    fn delete(&mut self, channel: String, ts: String) {
        let Some(original) = self.ws.find_message(&channel, &ts).cloned() else { return };
        self.ws.remove_message(&channel, &ts);
        if let Some(parent) = original.thread_ts.as_ref().filter(|_| original.is_reply()) {
            for p in self.ws.copies_mut(&channel, parent) {
                p.reply_count = p.reply_count.saturating_sub(1);
            }
        }
        for pane in [&mut self.messages, &mut self.thread_pane] {
            if pane.selected.as_deref() == Some(ts.as_str()) {
                pane.selected = None;
            }
        }
        for c in [&mut self.composer, &mut self.thread_composer] {
            if c.editing.as_deref() == Some(ts.as_str()) {
                c.clear();
            }
        }
        if self.thread.as_ref().is_some_and(|(c, t)| *c == channel && *t == ts) {
            self.close_thread();
        }
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            if let Err(e) = client.delete_message(&channel, &ts).await {
                let _ = tx.send(Net::Restore { channel, message: original, error: format!("delete failed: {e}") });
            }
        });
    }

    /// Copy message `ts` of the current conversation as plain text.
    fn copy_text(&mut self, ts: &str) {
        let Some(m) = self.current.as_deref().and_then(|c| self.ws.find_message(c, ts)) else { return };
        let ws = &self.ws;
        let text = slack_core::format::plain_text(&slack_core::format::message_blocks(&m.text, &m.blocks), &|i| super::text::inline_plain(ws, i));
        let how = self.clipboard.copy(&text);
        self.status = format!("copied message text to the {how}");
    }

    /// Fetch the permalink of message `ts` and copy it when it arrives.
    fn copy_permalink(&mut self, ts: &str) {
        let Some(channel) = self.current.clone() else { return };
        let (client, tx, ts) = (self.client.clone(), self.net.clone(), ts.to_string());
        tokio::spawn(async move {
            let msg = match client.permalink(&channel, &ts).await {
                Ok(url) => Net::Permalink(url),
                Err(e) => Net::Error(format!("permalink failed: {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    /// Open the thread of message `ts` (or the thread it was broadcast from) in the
    /// current conversation, loading its replies once.
    fn open_thread(&mut self, ts: &str) {
        let Some(channel) = self.current.clone() else { return };
        let Some(m) = self.ws.find_message(&channel, ts) else { return };
        let parent = if m.is_reply() { m.thread_ts.clone().unwrap_or_default() } else { m.ts.clone() };
        self.open_thread_at(channel, parent);
    }

    /// Open the thread whose parent is `parent` in `channel`.
    fn open_thread_at(&mut self, channel: String, parent: String) {
        let key = super::state::thread_key(&channel, &parent);
        self.seen.insert(key.clone(), opened_now());
        if self.thread.as_ref().is_none_or(|(c, t)| super::state::thread_key(c, t) != key) {
            if let Some((c, t)) = &self.thread {
                stash(&mut self.drafts, &super::state::thread_key(c, t), &mut self.thread_composer);
            }
            if let Some(d) = self.drafts.remove(&key) {
                self.thread_composer.restore(d);
            }
            self.save_drafts();
        }
        self.thread_pane.to_bottom();
        self.thread_pane.selected = None;
        self.focus = Focus::Thread;
        let tl = self.ws.threads.entry(super::state::thread_key(&channel, &parent)).or_default();
        self.thread = Some((channel.clone(), parent.clone()));
        if tl.loaded || tl.loading {
            return;
        }
        tl.loading = true;
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let msg = match client.replies(&channel, &parent).await {
                Ok(page) => Net::Replies { channel, ts: parent, page },
                Err(e) => Net::Error(format!("loading thread failed: {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn close_thread(&mut self) {
        if let Some((c, t)) = self.thread.take() {
            stash(&mut self.drafts, &super::state::thread_key(&c, &t), &mut self.thread_composer);
            self.save_drafts();
        }
        self.broadcast = false;
        if matches!(self.focus, Focus::Thread | Focus::ThreadComposer) {
            self.focus = Focus::Messages;
        }
    }

    /// Recompute suggestions for the token at the focused composer's cursor.
    fn refresh_completion(&mut self) {
        let thread = match self.focus {
            Focus::Composer => false,
            Focus::ThreadComposer => true,
            _ => {
                self.completion = None;
                return;
            }
        };
        let Some((start, trigger)) = self.composer_for(thread).trigger() else {
            self.path_suggestions = None;
            self.completion = None;
            self.completion_dismissed = None;
            return;
        };
        if self.completion_dismissed == Some((thread, start)) {
            self.completion = None;
            return;
        }
        let conversation = if thread { self.thread.as_ref().map(|(c, _)| c.clone()) } else { self.current.clone() };
        // A folder is read once per path typed, not at every event (pointer moves included).
        let items = match (&trigger, &self.path_suggestions) {
            (super::composer::Trigger::Path { .. }, Some((seen, items))) if *seen == trigger => items.clone(),
            (super::composer::Trigger::Argument { command, before, partial }, _) => super::autocomplete::arguments(command, before, partial, &self.workspaces),
            _ => super::autocomplete::suggestions(&self.ws, conversation.as_deref(), &trigger),
        };
        self.path_suggestions = matches!(trigger, super::composer::Trigger::Path { .. }).then(|| (trigger, items.clone()));
        if items.is_empty() {
            self.completion = None;
            return;
        }
        // Keep the highlighted row while the list itself is unchanged.
        let selected = match &self.completion {
            Some(c) if c.thread == thread && c.start == start && c.items == items => c.selected,
            _ => 0,
        };
        self.completion = Some(super::autocomplete::Completion { thread, start, items, selected });
    }

    /// Insert the highlighted suggestion.
    fn accept_completion(&mut self) {
        let Some(c) = self.completion.take() else { return };
        let Some(s) = c.items.get(c.selected) else { return };
        let space = if super::autocomplete::ends_open(&s.replacement) { "" } else { " " };
        self.composer_for(c.thread).complete(c.start, &format!("{}{space}", s.replacement), s.id.as_deref());
    }

    /// Run the search overlay's query, or open its highlighted hit if the query is unchanged.
    fn search_enter(&mut self) {
        let Some(search) = self.search.as_mut() else { return };
        let query = search.query.trim().to_string();
        if query.is_empty() {
            return;
        }
        if query == search.searched && !search.busy {
            if let Some(hit) = search.hits.get(search.selected).cloned() {
                self.search = None;
                self.open_hit(hit);
            }
            return;
        }
        search.busy = true;
        search.loading_more = false;
        self.fetch_search_page(query, 1);
    }

    /// Load the next page of search results once the selection nears the end of the list.
    fn search_more(&mut self) {
        let Some(search) = self.search.as_mut() else { return };
        if let Some(page) = search.next_page() {
            let query = search.searched.clone();
            self.fetch_search_page(query, page);
        }
    }

    fn fetch_search_page(&self, query: String, page: u32) {
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let result = client.search_messages(&query, super::search::PAGE_SIZE, page).await.map_err(|e| e.to_string());
            let _ = tx.send(Net::SearchDone { query, page, result });
        });
    }

    /// Go to a search hit: its conversation, the message selected, its thread open.
    fn open_hit(&mut self, hit: super::search::Hit) {
        if self.current.as_deref() == Some(hit.channel.as_str()) {
            self.show_hit(hit);
        } else {
            self.reveal(&hit.channel);
            self.sidebar.select_id(&self.ws, &hit.channel);
            self.pending_hit = Some(hit);
        }
    }

    fn show_hit(&mut self, hit: super::search::Hit) {
        let top = hit.thread_ts.clone().unwrap_or_else(|| hit.ts.clone());
        self.seeking = Some((hit.channel.clone(), top));
        self.continue_seek();
        match hit.thread_ts {
            Some(parent) => {
                self.messages.selected = Some(parent.clone());
                self.open_thread_at(hit.channel, parent);
                self.thread_pane.selected = Some(hit.ts);
            }
            None => {
                self.messages.selected = Some(hit.ts);
                self.focus = Focus::Messages;
            }
        }
    }

    /// Page the history back until the message being sought is loaded (the selection then
    /// shows it); stop at the beginning of the conversation or when it is no longer shown.
    fn continue_seek(&mut self) {
        let Some((channel, ts)) = self.seeking.clone() else { return };
        if self.current.as_deref() != Some(channel.as_str()) {
            // Not shown yet (the conversation is being switched to) or left: wait, or drop it.
            if self.pending_hit.is_none() {
                self.seeking = None;
            }
            return;
        }
        let Some(tl) = self.ws.timelines.get(&channel) else { return };
        if !tl.loaded || tl.loading {
            return;
        }
        let found = tl.index_of(&ts).is_some();
        let older = tl.messages.first().is_some_and(|m| seek_older(&ts, &m.ts));
        let (has_more, n) = (tl.has_more, tl.messages.len());
        if found || !older {
            self.seeking = None;
            if self.status.starts_with("looking for the message") {
                self.status.clear();
            }
        } else if has_more {
            self.status = format!("looking for the message… ({n} loaded)");
            self.load_history(&channel, true);
        } else {
            self.seeking = None;
            self.status = "that message is no longer in this conversation's history".into();
        }
    }

    /// Open what a clicked notification is about: the message (or thread reply), or the
    /// conversation.
    fn open_target(&mut self, target: super::notify::Target) {
        tracing::debug!(?target, "notification clicked");
        match target.ts {
            Some(ts) => self.open_hit(super::search::Hit { channel: target.channel, ts, thread_ts: target.thread_ts, user: None, text: String::new() }),
            None => self.jump_to(&target.channel),
        }
    }

    /// What clicking a desktop notification about `target` does: open it here (a click
    /// after switching workspace finds this session gone and does nothing).
    fn on_click(&self, target: super::notify::Target) -> Box<dyn FnOnce() + Send> {
        let net = self.net.clone();
        Box::new(move || {
            let _ = net.send(Net::NotificationClicked(target));
        })
    }

    /// Show conversation `id` and put the cursor in the composer. A channel we are not in
    /// is previewed.
    fn jump_to(&mut self, id: &str) {
        if self.ws.is_outside(id) {
            self.ws.preview = Some(id.to_string());
        }
        self.reveal(id);
        self.sidebar.select_id(&self.ws, id);
        self.focus = Focus::Composer;
    }

    /// Put conversation `id` in the sidebar however quiet it is (until another one is open).
    fn reveal(&mut self, id: &str) {
        if self.ws.shown.as_deref() != Some(id) {
            self.ws.shown = Some(id.to_string());
            self.sidebar.rebuild(&self.ws);
        }
    }

    /// Act on the quick switcher's highlighted entry.
    fn switch(&mut self) {
        let Some(sw) = self.switcher.take() else { return };
        let Some(entry) = sw.entries(&self.ws).into_iter().nth(sw.selected) else { return };
        match entry.target {
            super::switcher::Target::Conversation(id) => self.jump_to(&id),
            super::switcher::Target::User(user) => self.open_dm_with(user),
        }
    }

    /// Clear the status bar's message once it has been shown for [`STATUS_TTL`] (a new
    /// message starts the clock again).
    fn expire_status(&mut self, now: std::time::Instant) {
        if self.status != self.status_shown.0 {
            self.status_shown = (self.status.clone(), now);
        } else if status_expired(&self.status, now - self.status_shown.1) {
            self.status.clear();
            self.status_shown.0.clear();
        }
    }

    /// Subscribe to everyone's presence once connected and the user list is known
    /// (again after each reconnect); Slack answers with their current presence.
    fn subscribe_presence(&mut self) {
        if self.presence_subscribed || !self.connected || self.ws.users.is_empty() {
            return;
        }
        let users: Vec<String> = self.ws.users.values().filter(|u| !u.deleted && !u.is_bot && u.id != "USLACKBOT").map(|u| u.id.clone()).take(500).collect();
        if self.rtm_commands.send(slack_core::rtm::RtmCommand::PresenceSub { users }).is_ok() {
            self.presence_subscribed = true;
        }
    }

    /// F8: show the huddle view in place of the messages, or the messages again.
    fn toggle_huddle_view(&mut self) {
        if self.huddle_view.take().is_some() {
            self.focus = Focus::Composer;
        } else if self.call.is_some() {
            // Narrow: the view and a thread take turns in the messages' place.
            if self.narrow {
                self.close_thread();
            }
            self.huddle_view = Some(Default::default());
            self.focus = Focus::Huddle;
        } else {
            self.status = format!("not in a huddle ({} joins this conversation's)", self.keys.first_key(Context::Global, Action::Huddle).unwrap_or_default());
        }
    }

    /// F10: share the screen (the desktop asks what; nothing blocks meanwhile), or stop.
    fn toggle_share(&mut self) {
        let Some(call) = self.call.as_mut() else {
            self.status = "not in a huddle".into();
            return;
        };
        if call.share_pending {
            self.status = "choose what to share in the desktop's dialog…".into();
        } else if call.is_sharing() {
            call.stop_share();
            self.status = "stopped sharing".into();
        } else {
            call.share_pending = true;
            let asks = huddle::screen::Source::current().is_some_and(|s| s.asks_what_to_share());
            self.status = if asks { "choose what to share in the desktop's dialog…" } else { "starting to share your whole screen…" }.into();
            let (creds, videos, tx) = (call.creds.clone(), call.videos.clone(), self.net.clone());
            tokio::spawn(async move {
                let result = super::call::start_share(creds, videos).await.map_err(|e| e.to_string());
                let _ = tx.send(Net::ScreenShared(result));
            });
        }
    }

    /// F3: join (or start) the huddle of the current conversation, or leave the one we are in.
    fn toggle_huddle(&mut self) {
        if let Some(call) = self.call.as_mut() {
            call.leave();
            self.status = "leaving the huddle…".into();
            return;
        }
        // A ringing invitation is answered, wherever you are looking.
        if let Some(ring) = self.ringing.take() {
            self.jump_to(&ring.channel);
            self.join_huddle(ring.channel, None);
            return;
        }
        if let Some(channel) = self.current.clone() {
            self.join_huddle(channel, None);
        }
    }

    /// Join or start the huddle of `channel`, or of the thread under message `thread`
    /// (`rooms.join` with `thread_ts`, as the web client's "Start huddle in thread").
    /// Decline the ringing invitation on Slack (`rooms.inviteResponse`), or say there is none.
    fn decline_huddle(&mut self, auto: bool) {
        let Some(ring) = self.ringing.take() else {
            self.status = "no huddle invitation is ringing".into();
            return;
        };
        let who = self.ws.user_name(&ring.user);
        self.status = if auto { format!("declined {who}'s huddle (no answer)") } else { format!("declined {who}'s huddle") };
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let params = [("response", "decline".to_string()), ("channel_id", ring.channel), ("room_id", ring.room)];
            if let Err(e) = client.call_raw("rooms.inviteResponse", &params).await {
                let _ = tx.send(Net::Error(format!("declining the huddle failed: {e}")));
            }
        });
    }

    /// Play the ring every few seconds while an invitation rings; when it has rung long
    /// enough, stop (and decline it, if so set).
    fn ring_tick(&mut self) {
        let Some(ring) = self.ringing.as_mut() else { return };
        let h = &self.ws.settings.huddle;
        if ring.started.elapsed() >= std::time::Duration::from_secs(h.ring_timeout_secs as u64) {
            if h.auto_decline {
                self.decline_huddle(true);
            } else {
                let who = self.ws.user_name(&ring.user);
                self.ringing = None;
                self.status = format!("missed {who}'s huddle");
            }
            return;
        }
        if ring.last_ring.is_none_or(|t| t.elapsed() >= RING_EVERY) {
            ring.last_ring = Some(std::time::Instant::now());
            let s = &self.ws.settings;
            super::sound::play(&s.huddle.ring_sound, super::settings::Settings::device(huddle::devices::OUTPUT_ENV, &s.audio.output), s.audio.sound_volume);
        }
    }

    fn join_huddle(&mut self, channel: String, thread: Option<String>) {
        self.rejoined = false;
        self.join_huddle_after(channel, thread, std::time::Duration::ZERO);
    }

    /// [`App::join_huddle`] after `delay`.
    fn join_huddle_after(&mut self, channel: String, thread: Option<String>, delay: std::time::Duration) {
        if self.ringing.as_ref().is_some_and(|r| r.channel == channel) {
            self.ringing = None;
        }
        self.status = "joining the huddle…".into();
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let result = async {
                let mut params = vec![("channel_id", channel.clone())];
                if let Some(t) = &thread {
                    params.push(("thread_ts", t.clone()));
                }
                let v = client.call_raw("rooms.join", &params).await?;
                huddle::signaling::Credentials::from_rooms_join(&v)
            }
            .await
            .map_err(|e| e.to_string());
            let _ = tx.send(Net::HuddleJoined { channel, thread, result });
        });
    }

    /// Ask for the next quiet-hours window once the known one has ended.
    fn refresh_dnd(&mut self) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        if self.dnd_refreshing || !self.ws.dnd.stale(now) {
            return;
        }
        self.dnd_refreshing = true;
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            match client.call_raw("dnd.info", &[]).await {
                Ok(v) => {
                    let _ = tx.send(Net::Dnd(v));
                }
                Err(e) => tracing::warn!("dnd.info failed: {e}"),
            }
        });
    }

    /// A key pressed while the preferences wait for a new binding: the leader waits for one
    /// more key; then the sequence replaces the action's keys (or becomes the leader).
    fn capture_key(&mut self, key: Key) {
        let leaders = [self.keys.leader, self.keys.insert_leader];
        let Some(p) = self.prefs.as_mut() else { return };
        let field = p.selected_field();
        let Some(seq) = p.capturing.as_mut() else { return };
        if matches!(field, Some(super::prefs::Field::Binding(_))) && seq.is_empty() && leaders.contains(&Some(key)) {
            seq.push(LEADER);
            return;
        }
        seq.push(key);
        let seq = p.capturing.take().unwrap_or_default();
        let text = match field {
            Some(super::prefs::Field::Binding(i)) => {
                let info = &super::keymap::ACTIONS[i];
                self.keys.to_toml(Some((info.context, info.name, vec![seq])), None)
            }
            Some(super::prefs::Field::Leader) => self.keys.to_toml(None, Some(Some(key))),
            _ => return,
        };
        self.save_keys(&text);
    }

    /// Write keys.toml and use it; problems are shown under the preferences.
    fn save_keys(&mut self, text: &str) {
        let (map, warnings) = Keymap::load(text);
        let written = super::keymap::config_path().ok_or_else(|| "no config directory".to_string()).and_then(|path| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, text).map_err(|e| e.to_string()).map(|_| path.display().to_string())
        });
        if written.is_ok() {
            self.keys = map;
        }
        if let Some(p) = self.prefs.as_mut() {
            p.saved = Some(match (written, warnings.first()) {
                (Ok(_), Some(w)) => Err(w.trim_start_matches("keys.toml: ").to_string()),
                (result, _) => result,
            });
        }
    }

    /// +/- in the preferences: step a level up or down, save it and apply it.
    fn adjust_preference(&mut self, up: bool) {
        let Some(p) = self.prefs.as_mut() else { return };
        let Some(field) = p.selected_field() else { return };
        if field.adjust(&mut self.ws.settings, up, false) {
            p.saved = Some(super::settings::save(&self.ws.settings).map(|path| path.display().to_string()));
            self.audio_changed(Some(field));
        }
    }

    /// After an audio setting changed: the call follows at once; a new notification volume
    /// is played so it can be heard.
    fn audio_changed(&mut self, field: Option<super::prefs::Field>) {
        if let Some(call) = &self.call {
            call.set_audio(&self.ws.settings.audio);
        }
        if field == Some(super::prefs::Field::SoundVolume) {
            let s = &self.ws.settings;
            let name = if super::sound::samples(&s.notifications.sound).is_empty() { "chime" } else { s.notifications.sound.as_str() };
            super::sound::play(name, super::settings::Settings::device(huddle::devices::OUTPUT_ENV, &s.audio.output), s.audio.sound_volume);
        }
    }

    /// A key in the theme picker: typing filters, moving shows the theme at once, Enter keeps
    /// it (saved), Esc goes back to the theme before.
    fn theme_picker_key(&mut self, k: &crossterm::event::KeyEvent) {
        use super::theme_picker::COLOUR_THEME;
        let action = self.action_in(Context::Switcher, k);
        let Some(p) = self.prefs.as_mut() else { return };
        let Some(picker) = p.picker.as_mut() else { return };
        let appearance = &mut self.ws.settings.appearance;
        match action {
            Some(Action::Close) => p.picker = None,
            Some(Action::Up) => picker.move_by(-1),
            Some(Action::Down) => picker.move_by(1),
            Some(Action::Erase) => picker.erase(),
            Some(Action::Open) => {
                let Some(name) = picker.highlighted().filter(|_| picker.problem.is_none()).map(String::from) else { return };
                match (picker.workspace, name.as_str()) {
                    (true, COLOUR_THEME) => drop(appearance.workspace_themes.remove(&self.ws.team_id)),
                    (true, _) => drop(appearance.workspace_themes.insert(self.ws.team_id.clone(), name)),
                    (false, _) => appearance.theme = name,
                }
                p.picker = None;
                p.saved = Some(super::settings::save(&self.ws.settings).map(|path| path.display().to_string()));
            }
            _ => match (k.code, Key::from_event(k).is_text()) {
                (KeyCode::Char(c), true) => picker.type_char(c),
                _ => return,
            },
        }
        match self.prefs.as_mut().and_then(|p| p.picker.as_mut()) {
            Some(picker) => picker.preview(&self.ws.settings.appearance.theme),
            // Closed: the theme in force for this workspace.
            None => drop(super::theme::set(self.ws.settings.appearance.theme_for(&self.ws.team_id))),
        }
    }

    /// Enter in the preferences: change a local setting (and save the file) or send a
    /// Slack-side one (applied here at once; on failure the prefs are fetched again).
    fn change_preference(&mut self) {
        use super::prefs::SlackChange;
        let current = self.current.clone();
        let Some(p) = self.prefs.as_mut() else { return };
        let on_slack = p.on_slack();
        p.saved = None;
        let field = p.selected_field();
        let change = p.change(&mut self.ws, current.as_deref());
        if !on_slack {
            // A refused value (e.g. an unknown time zone) keeps its message and is not saved.
            if p.saved.is_none() && p.editing.is_none() && p.picker.is_none() {
                p.saved = Some(super::settings::save(&self.ws.settings).map(|path| path.display().to_string()));
            }
            if matches!(field, Some(super::prefs::Field::Compact | super::prefs::Field::HideInactive)) {
                // The sidebar's gaps between sections follow it.
                self.sidebar.rebuild(&self.ws);
            }
            if field == Some(super::prefs::Field::Graphics) {
                self.media.choose(&self.ws.settings.messages.graphics);
            }
            self.audio_changed(field);
            return;
        }
        let Some(change) = change else { return };
        match change {
            SlackChange::Command(text) => self.run_text(&text),
            SlackChange::ConversationLevel { channel, level } => self.set_conversation_level(channel, level),
            SlackChange::ChannelDefault(level) => {
                self.ws.notify.global = level.clone();
                self.save_notification_pref("global_desktop", level, None, false);
            }
            SlackChange::Keywords(words) => {
                self.ws.notify.keywords = words.split(',').filter(|w| !w.is_empty()).map(String::from).collect();
                self.save_notification_pref("global_keywords", words, None, false);
            }
        }
    }

    /// Run a slash command typed out (`/status clear`) in the current conversation.
    fn run_text(&mut self, text: &str) {
        match super::commands::parse(text, text) {
            Some(Ok(cmd)) => self.run_command(self.current.clone().unwrap_or_default(), cmd, None),
            Some(Err(e)) => self.status = e,
            None => {}
        }
    }

    /// ␣cb: the current conversation notifies about the next level (as F6 → Notifications
    /// does), and the status line says which.
    fn cycle_conversation_level(&mut self) {
        let Some(channel) = self.current.clone() else { return };
        let level = super::prefs::next_conversation_level(self.ws.notify.channels.get(&channel).map(String::as_str));
        self.set_conversation_level(channel.clone(), level);
        let mpim = self.ws.conversations.get(&channel).is_some_and(|c| c.is_mpim);
        let own = self.ws.notify.channels.contains_key(&channel);
        self.status = format!("{} notifies about {}{}", self.ws.channel_label(&channel), super::state::level_label(self.ws.notify.level(&channel, mpim)), if own { "" } else { " (the default)" });
    }

    /// A conversation's own level ("default": none), here at once and then on Slack.
    fn set_conversation_level(&mut self, channel: String, level: String) {
        if level == "default" {
            self.ws.notify.channels.remove(&channel);
            // A reset also drops the mute, which is put back after it.
            let remute = self.ws.muted.contains(&channel);
            self.save_notification_pref("reset", level, Some(channel), remute);
        } else {
            self.ws.notify.channels.insert(channel.clone(), level.clone());
            self.save_notification_pref("desktop", level, Some(channel), false);
        }
    }

    /// Send a notification setting; on failure say so and fetch the prefs again.
    fn save_notification_pref(&self, name: &'static str, value: String, channel: Option<String>, remute: bool) {
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let mut result = client.set_notification_pref(name, &value, channel.as_deref()).await;
            if remute && result.is_ok() {
                result = client.set_notification_pref("muted", "true", channel.as_deref()).await;
            }
            if let Err(e) = result {
                let _ = tx.send(Net::Error(format!("saving the notification setting failed: {e}")));
                if let Ok(prefs) = client.user_prefs().await {
                    let _ = tx.send(Net::Prefs(prefs));
                }
            }
        });
    }

    /// Add `user` to the VIPs or remove them (optimistic, then `users.prefs.set vip_users`).
    fn toggle_vip(&mut self, user: String) {
        if user == self.ws.self_id {
            self.status = "you cannot be your own VIP".into();
            return;
        }
        let added = self.ws.vip.insert(user.clone());
        if !added {
            self.ws.vip.remove(&user);
        }
        self.status = format!("{} {} your VIPs", self.ws.user_name(&user), if added { "added to" } else { "removed from" });
        self.sidebar.rebuild(&self.ws);
        let mut list: Vec<String> = self.ws.vip.iter().cloned().collect();
        list.sort();
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            if let Err(e) = client.set_pref("vip_users", &list.join(",")).await {
                let _ = tx.send(Net::Error(format!("saving VIPs failed: {e}")));
                if let Ok(prefs) = client.user_prefs().await {
                    let _ = tx.send(Net::Prefs(prefs));
                }
            }
        });
    }

    /// F5: open the activity view and load the feed.
    fn open_activity(&mut self) {
        self.activity = Some(super::activity::Activity::default());
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let _ = tx.send(Net::ActivityFeed(client.activity_feed(50).await.map_err(|e| e.to_string())));
        });
    }

    /// Ctrl+T: open the Threads view and load its first page (or, with `more`, the next).
    fn load_threads(&mut self, more: bool) {
        let before = match self.threads.as_mut() {
            Some(t) if more => match t.next_page() {
                Some(ts) => {
                    t.loading = true;
                    Some(ts)
                }
                None => return,
            },
            _ => {
                self.threads = Some(super::threads::Threads { loading: true, ..Default::default() });
                None
            }
        };
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let result = client.thread_view(before.as_deref(), 50).await.map_err(|e| e.to_string());
            let _ = tx.send(Net::ThreadView { result, more });
        });
    }

    /// The mouse wheel over an open list (Ctrl+K, the section mover, F5, Ctrl+T, search) moves
    /// its selection a step, like the keys; in the help and the preferences (one-row entries) it
    /// goes three rows, as in the message panes. False when none is open.
    fn wheel_in_popup(&mut self, down: bool) -> bool {
        let step = if down { 1 } else { -1 };
        if let Some((picker, _, _)) = self.picker.as_mut() {
            let len = picker.candidates(&self.ws.custom_emoji).len();
            picker.move_by(step, len);
        } else if let Some(d) = self.save.as_mut() {
            d.move_by(step);
        } else if let Some(help) = self.help.as_mut() {
            help.scroll_by(3 * step);
        } else if let Some(p) = self.prefs.as_mut() {
            if let Some(picker) = p.picker.as_mut() {
                picker.move_by(step);
                picker.preview(&self.ws.settings.appearance.theme);
            } else if p.editing.is_none() && p.capturing.is_none() {
                p.move_by(3 * step);
            }
        } else if let Some(sw) = self.switcher.as_mut() {
            let len = sw.entries(&self.ws).len();
            sw.move_by(step, len);
        } else if let Some(m) = self.mover.as_mut().filter(|m| m.naming.is_none()) {
            m.move_by(step);
        } else if let Some(a) = self.activity.as_mut() {
            a.move_by(step);
        } else if let Some(t) = self.threads.as_mut() {
            t.move_by(step);
            if t.at_end() {
                self.load_threads(true);
            }
        } else if let Some(search) = self.search.as_mut() {
            search.move_by(step);
            if down {
                self.search_more();
            }
        } else {
            return false;
        }
        true
    }

    /// Fetch the messages of the activity items around the selection.
    fn request_activity_messages(&mut self) {
        let Some(a) = self.activity.as_mut() else { return };
        for (channel, ts) in a.wanted() {
            a.requested.insert((channel.clone(), ts.clone()));
            let (client, tx) = (self.client.clone(), self.net.clone());
            tokio::spawn(async move {
                let message = match client.message_at(&channel, &ts).await {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!("activity message {channel}/{ts}: {e}");
                        None
                    }
                };
                let _ = tx.send(Net::ActivityMessage { channel, ts, message });
            });
        }
    }

    /// The preferences wait for a key to bind: every key goes to them.
    fn capturing_key(&self) -> bool {
        self.prefs.as_ref().is_some_and(|p| p.capturing.is_some())
    }

    /// Where input goes now (see [`mode_of`]).
    fn mode(&self) -> Mode {
        mode_of(self.overlay(), self.focus)
    }

    /// The topmost dialog's area at the last draw (None without a dialog).
    fn dialog_area(&self) -> Option<Rect> {
        self.overlay()?;
        Some(if let Some(m) = &self.menu {
            m.area
        } else if let Some(h) = &self.help {
            h.area
        } else if let Some(p) = &self.prefs {
            p.picker.as_ref().map_or(p.area, |t| t.area)
        } else if let Some(s) = &self.switcher {
            s.area
        } else if let Some(m) = &self.mover {
            m.area
        } else if let Some(d) = &self.save {
            d.area
        } else if let Some(a) = &self.activity {
            a.area
        } else if let Some(t) = &self.threads {
            t.area
        } else if let Some(card) = &self.profile {
            card.area
        } else if let Some(d) = &self.details {
            d.area
        } else if let Some(s) = &self.search {
            s.area
        } else if let Some(p) = &self.player {
            p.area
        } else if let Some(s) = &self.snippet {
            s.area
        } else if let Some(v) = &self.viewer {
            v.area
        } else if self.confirm.is_some() {
            self.confirm_area
        } else {
            self.picker.as_ref().map(|(p, _, _)| p.area).unwrap_or_default()
        })
    }

    /// Close the topmost dialog as its close key would (a click outside it).
    fn close_dialog(&mut self) {
        if self.menu.is_some() {
            self.menu = None;
        } else if self.help.is_some() {
            self.help = None;
        } else if let Some(p) = self.prefs.as_mut() {
            if p.capturing.is_some() {
                p.capturing = None;
            } else if p.picker.is_some() {
                p.picker = None;
                drop(super::theme::set(self.ws.settings.appearance.theme_for(&self.ws.team_id)));
            } else if p.editing.is_some() {
                p.editing = None;
            } else {
                self.prefs = None;
            }
        } else if self.switcher.is_some() {
            self.switcher = None;
        } else if let Some(m) = self.mover.as_mut() {
            if m.naming.take().is_none() {
                self.mover = None;
            }
        } else if self.save.is_some() {
            self.save = None;
        } else if self.activity.is_some() {
            self.activity = None;
        } else if self.threads.is_some() {
            self.threads = None;
        } else if self.profile.is_some() {
            self.profile = None;
        } else if self.details.is_some() {
            self.details = None;
        } else if self.search.is_some() {
            self.search = None;
        } else if self.player.is_some() {
            self.player = None;
        } else if self.snippet.is_some() {
            self.snippet = None;
        } else if self.viewer.is_some() {
            self.viewer = None;
        } else if self.confirm.is_some() {
            self.confirm = None;
        } else {
            self.picker = None;
        }
    }

    /// The context of the topmost open dialog, if any.
    fn overlay(&self) -> Option<Context> {
        if self.menu.is_some() {
            Some(Context::Menu)
        } else if self.help.is_some() {
            Some(Context::Help)
        } else if let Some(p) = &self.prefs {
            Some(if p.editing.is_some() {
                Context::TextField
            } else if p.picker.is_some() {
                // A filterable list like the switcher.
                Context::Switcher
            } else {
                Context::Preferences
            })
        } else if self.switcher.is_some() {
            Some(Context::Switcher)
        } else if let Some(m) = &self.mover {
            Some(if m.naming.is_some() { Context::TextField } else { Context::Mover })
        } else if let Some(d) = &self.save {
            Some(if d.editing { Context::TextField } else { Context::Save })
        } else if self.activity.is_some() {
            Some(Context::Activity)
        } else if self.threads.is_some() {
            Some(Context::Threads)
        } else if self.profile.is_some() {
            Some(Context::Profile)
        } else if self.details.is_some() {
            Some(Context::Details)
        } else if self.search.is_some() {
            Some(Context::Search)
        } else if self.player.is_some() {
            Some(Context::Player)
        } else if self.snippet.is_some() {
            Some(Context::Snippet)
        } else if self.viewer.is_some() {
            Some(Context::Viewer)
        } else if self.confirm.is_some() {
            Some(Context::Confirm)
        } else if self.picker.is_some() {
            Some(Context::Picker)
        } else {
            None
        }
    }

    /// The contexts a key press is looked up in now, most specific first: the open dialog's,
    /// or the suggestion list and the focused pane's; then global.
    fn active_contexts(&self) -> Vec<Context> {
        let mut out = Vec::new();
        if let Some(c) = self.overlay() {
            out.push(c);
        } else {
            if self.completion.is_some() {
                out.push(Context::Completion);
            }
            out.push(match self.focus {
                Focus::Sidebar => Context::Sidebar,
                Focus::Huddle => Context::Huddle,
                Focus::Messages | Focus::Thread => Context::Messages,
                Focus::Composer | Focus::ThreadComposer => Context::Composer,
            });
        }
        out.push(Context::Global);
        out
    }

    /// What a key press means now (after any pending keys).
    fn resolve(&self, k: &crossterm::event::KeyEvent) -> Resolved {
        self.keys.resolve(&self.active_contexts(), self.mode(), self.pending.keys(), Key::from_event(k))
    }

    /// Before a key is handled: hold it if it only starts a sequence (the leader), drop it
    /// if it ends a sequence that means nothing. True when the key was used up here.
    fn hold_sequence(&mut self, k: &crossterm::event::KeyEvent) -> bool {
        if self.capturing_key() {
            return false;
        }
        let mode = self.mode();
        self.pending.sync(mode);
        let typed = Key::from_event(k);
        if super::which_key::handle_key(&mut self.pending, self.which_key.as_mut(), typed) {
            self.pending_since = Some(std::time::Instant::now());
            return true;
        }
        let (resolved, key) = self.keys.press(&self.active_contexts(), mode, self.pending.keys(), typed);
        match resolved {
            Resolved::Action(..) => false,
            Resolved::Pending => {
                self.pending.push_typed(key, typed, mode);
                self.pending_since = Some(std::time::Instant::now());
                true
            }
            Resolved::None if !self.pending.keys().is_empty() => {
                let mut seq = self.pending.typed().to_vec();
                seq.push(typed);
                self.pending.clear();
                self.status = format!("{} is not bound here", self.keys.show(&seq));
                true
            }
            Resolved::None => false,
        }
    }

    /// When which-key is to open for the pending sequence (None: no sequence, shown already,
    /// or turned off).
    fn which_key_due(&self) -> Option<std::time::Instant> {
        let settings = &self.ws.settings.which_key;
        let since = self.pending_since.filter(|_| settings.show && self.which_key.is_none() && !self.pending.keys().is_empty())?;
        Some(since + Duration::try_from_secs_f32(settings.delay_secs.max(0.0)).unwrap_or_default())
    }

    /// Open which-key once the sequence has waited long enough; close it once it is over.
    fn sync_which_key(&mut self, now: std::time::Instant) {
        if self.pending.keys().is_empty() {
            (self.which_key, self.pending_since) = (None, None);
        } else if self.which_key_due().is_some_and(|due| due <= now) {
            self.which_key = Some(super::which_key::Panel::default());
        }
    }

    /// The action of a key press in `context` alone (after any pending keys), if any.
    fn action_in(&self, context: Context, k: &crossterm::event::KeyEvent) -> Option<Action> {
        match self.keys.resolve(&[context], self.mode(), self.pending.keys(), Key::from_event(k)) {
            Resolved::Action(_, a) => Some(a),
            Resolved::Pending | Resolved::None => None,
        }
    }

    /// Leave this workspace for workspace `index` of the profile.
    fn switch_workspace(&mut self, index: usize) {
        if index >= self.workspaces.len() {
            self.status = format!("there is no workspace {} (this profile has {})", index + 1, self.workspaces.len());
        } else if index == self.workspace {
            self.status = format!("already in {}", self.workspaces[index]);
        } else if self.call.is_some() {
            self.status = "leave the huddle first".into();
        } else {
            self.switch_to = Some(index);
        }
    }

    /// The thread of the huddle we are in, or of the current conversation's huddle.
    fn open_huddle_thread(&mut self) {
        let channel = self.call.as_ref().map(|c| c.channel.clone()).or_else(|| self.current.clone().filter(|c| self.ws.huddles.contains_key(c)));
        let Some(channel) = channel else {
            self.status = "no huddle here".into();
            return;
        };
        let thread = self.call.as_ref().and_then(|c| c.thread.clone()).or_else(|| self.ws.huddles.get(&channel).and_then(|h| h.thread.clone()));
        match thread {
            Some(ts) => {
                self.open_hit(super::search::Hit { channel, ts: ts.clone(), thread_ts: Some(ts), user: None, text: String::new() });
                self.focus = Focus::ThreadComposer;
            }
            None => self.status = "the huddle has no thread yet".into(),
        }
    }

    /// Profile card `h`: a huddle in the DM with `user`.
    fn huddle_with(&mut self, user: String) {
        if user == self.ws.self_id {
            self.status = "that is you".into();
        } else if self.call.is_some() {
            self.status = "leave the current huddle first".into();
        } else if let Some(id) = self.ws.conversations.values().find(|c| c.is_im && c.user.as_deref() == Some(user.as_str())).map(|c| c.id.clone()) {
            self.jump_to(&id);
            self.join_huddle(id, None);
        } else {
            self.huddle_after_open = Some(user.clone());
            self.open_dm_with(user);
        }
    }

    /// Show the DM with `user`, opening one if needed.
    fn open_dm_with(&mut self, user: String) {
        if let Some(id) = self.ws.conversations.values().find(|c| c.is_im && c.user.as_deref() == Some(user.as_str())).map(|c| c.id.clone()) {
            self.jump_to(&id);
            return;
        }
        self.status = format!("opening a DM with {}…", self.ws.user_name(&user));
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let _ = tx.send(match client.open_dm(&[&user]).await {
                Ok(mut c) => {
                    c.is_im = true;
                    c.user.get_or_insert(user);
                    Net::DmOpened(c)
                }
                Err(e) => Net::Error(format!("opening the DM failed: {e}")),
            });
        });
    }

    /// Pin or unpin message `ts` in the current conversation (optimistic, then the API).
    fn toggle_pin(&mut self, ts: &str) {
        let Some(channel) = self.current.clone() else { return };
        let Some(pinned) = self.ws.find_message(&channel, ts).map(|m| m.pinned_to.contains(&channel)) else { return };
        for m in self.ws.copies_mut(&channel, ts) {
            m.pinned_to.retain(|c| *c != channel);
            if !pinned {
                m.pinned_to.push(channel.clone());
            }
        }
        self.status = if pinned { "unpinned".into() } else { "pinned".into() };
        let (client, tx, ts) = (self.client.clone(), self.net.clone(), ts.to_string());
        tokio::spawn(async move {
            if let Err(e) = client.pin(&channel, &ts, !pinned).await {
                let _ = tx.send(Net::Error(format!("pin failed: {e}")));
            }
        });
    }

    /// Open the details overlay for the current conversation and load its tabs.
    fn open_details(&mut self) {
        let Some(channel) = self.current.clone() else { return };
        self.details = Some(super::details::Details::new(channel.clone()));
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let (info, members, pins) = tokio::join!(client.conversation_info(&channel), client.members(&channel), client.pins(&channel));
            match info {
                Ok(c) => {
                    let _ = tx.send(Net::DetailsInfo(c));
                }
                Err(e) => tracing::warn!("conversations.info failed: {e}"),
            }
            let _ = tx.send(Net::DetailsMembers { channel: channel.clone(), members: members.unwrap_or_default() });
            let pins = pins.map(|v| super::details::parse_pins(&v)).unwrap_or_default();
            let _ = tx.send(Net::DetailsPins { channel, pins });
        });
    }

    /// Enter in the details overlay: DM a member, or go to a pinned message.
    fn details_enter(&mut self) {
        let Some(d) = self.details.take() else { return };
        match d.tab {
            super::details::Tab::Members => {
                if let Some(user) = d.members.as_ref().and_then(|m| m.get(d.selected)).cloned() {
                    self.profile = super::profile::ProfileCard::new(vec![user]);
                }
                self.details = Some(d);
            }
            super::details::Tab::Pins => {
                if let Some(m) = d.pins.as_ref().and_then(|p| p.get(d.selected)) {
                    let thread_ts = m.thread_ts.clone().filter(|_| m.is_reply());
                    self.open_hit(super::search::Hit { channel: d.channel.clone(), ts: m.ts.clone(), thread_ts, user: m.user.clone(), text: m.text.clone() });
                }
            }
            super::details::Tab::About => self.details = Some(d),
        }
    }

    /// Upload a local file to `channel` (or a thread) with an optional message.
    fn upload(&mut self, channel: String, cmd: super::files::UploadCommand, thread_ts: Option<String>) {
        let name = cmd.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        self.status = format!("uploading {name}…");
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let message = super::composer::escape_text(&cmd.message);
            let progress = upload_progress(tx.clone(), name.clone());
            let result = client.upload_file(&channel, &cmd.path, &name, Some(&message), thread_ts.as_deref(), progress).await;
            let _ = tx.send(Net::FileDone(match result {
                Ok(_) => format!("uploaded {name}"),
                Err(e) => format!("upload failed: {e}"),
            }));
        });
    }

    /// Ctrl+V in a composer: text from the system clipboard is typed in, a picture or copied
    /// files are attached to the message.
    fn paste(&mut self, thread: bool) {
        use super::clipboard::Pasted;
        use super::composer::{Attachment, AttachmentData};
        let pasted = self.clipboard.paste();
        if !matches!(pasted, Pasted::Text(_) | Pasted::Nothing(_)) && self.composer_for(thread).editing.is_some() {
            self.status = "an edited message keeps its files; send pictures and files in a new one".into();
            return;
        }
        match pasted {
            Pasted::Text(text) => self.composer_for(thread).insert_str(&text.replace("\r\n", "\n")),
            Pasted::Files(files) => {
                let (mut attached, mut skipped) = (Vec::new(), Vec::new());
                for path in files {
                    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().to_string());
                    match std::fs::metadata(&path) {
                        Ok(m) if m.is_file() => {
                            attached.push(name.clone());
                            self.composer_for(thread).attachments.push(Attachment { name, size: m.len(), data: AttachmentData::File(path) });
                        }
                        Ok(_) => skipped.push(format!("{name} (a folder)")),
                        Err(e) => skipped.push(format!("{name} ({e})")),
                    }
                }
                self.status = attached_status(&attached, &skipped);
            }
            Pasted::Image { width, height, rgba } => {
                self.status = format!("attaching a {width}×{height} picture…");
                let tx = self.net.clone();
                tokio::task::spawn_blocking(move || {
                    let result = super::clipboard::png(width, height, rgba)
                        .map(|png| Attachment { name: "image.png".into(), size: png.len() as u64, data: AttachmentData::Png(std::sync::Arc::new(png)) })
                        .map_err(|e| format!("the pasted picture could not be read: {e:#}"));
                    let _ = tx.send(Net::Pasted { thread, result });
                });
            }
            Pasted::Nothing(why) => self.status = why.into(),
        }
    }

    /// `s` on message `ts`: the save dialog for its files.
    fn save_files(&mut self, ts: &str) {
        let Some(channel) = self.current.clone() else { return };
        let files = self.ws.find_message(&channel, ts).map(|m| m.files.clone()).unwrap_or_default();
        if files.is_empty() {
            self.status = "no files in this message".into();
            return;
        }
        self.save = Some(super::save::SaveDialog::new(files, &super::files::download_dir(&self.ws.settings.messages.save_dir)));
    }

    /// Enter in the save dialog: the ticked files into its folder (never over a file there).
    fn save_ticked(&mut self) {
        let Some(dialog) = self.save.as_ref() else { return };
        let files = dialog.chosen();
        if files.is_empty() {
            self.status = "tick a file to save (Space)".into();
            return;
        }
        let dir = dialog.folder_path();
        self.save = None;
        self.status = format!("saving {} file(s) to {}…", files.len(), dir.display());
        for file in files {
            let (client, tx, dir) = (self.client.clone(), self.net.clone(), dir.clone());
            tokio::spawn(async move {
                let name = file.name.clone();
                let _ = tx.send(Net::FileDone(match super::files::save(client, file, dir).await {
                    Ok(path) => format!("saved {}", path.display()),
                    Err(e) => format!("saving {name} failed: {e}"),
                }));
            });
        }
    }

    /// `o` on message `ts`: open what it holds — at once when it is one thing, else from a
    /// list of them beside the message.
    fn open_things(&mut self, ts: &str, thread: bool) {
        let Some(m) = self.current.as_deref().and_then(|c| self.ws.find_message(c, ts)) else { return };
        let things = super::open::things(m);
        match things.as_slice() {
            [] => self.status = "nothing to open in this message".into(),
            [one] => {
                let one = one.clone();
                self.open_thing(ts, one);
            }
            _ => {
                let labels = things.iter().map(|t| super::open::label(t, m)).collect();
                let at = if thread { self.thread_pane.spot(ts) } else { self.messages.spot(ts) };
                self.menu = Some(super::menu::Menu::opener(thread, ts.to_string(), things, labels, at));
            }
        }
    }

    /// Open one thing of message `ts`: a link in the browser, a picture in the viewer (the
    /// others a step away), a video in the player, a snippet in full.
    fn open_thing(&mut self, ts: &str, thing: super::open::Thing) {
        use super::open::Thing;
        match thing {
            Thing::Link { url, .. } => {
                self.status = match super::open::open_url(&url) {
                    Ok(()) => format!("opened {url}"),
                    Err(e) => format!("could not open {url}: {e:#}"),
                };
            }
            Thing::Picture(n) => {
                let pictures: Vec<slack_core::types::File> = self.current.as_deref().and_then(|c| self.ws.find_message(c, ts)).map(|m| m.files.iter().filter(|f| f.is_image()).cloned().collect()).unwrap_or_default();
                if n < pictures.len() {
                    self.viewer = Some(super::viewer::Viewer::new(pictures, n));
                    self.load_viewer_image();
                }
            }
            Thing::Video(file) => {
                self.status = format!("loading {}…", file.name);
                let (client, tx) = (self.client.clone(), self.net.clone());
                tokio::spawn(async move {
                    let title = file.name.clone();
                    let result = super::video::download(client, file).await.map_err(|e| e.to_string());
                    let _ = tx.send(Net::VideoReady { title, result });
                });
            }
            Thing::Snippet(file) => {
                let (client, tx, id) = (self.client.clone(), self.net.clone(), file.id.clone());
                let url = file.url_private_download.clone().or(file.url_private.clone());
                self.snippet = Some(super::snippet::SnippetView::new(file));
                tokio::spawn(async move {
                    let result = match url {
                        Some(url) => client.download(&url).await.map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| e.to_string()),
                        None => Err("no download URL".into()),
                    };
                    let _ = tx.send(Net::SnippetText { id, result });
                });
            }
        }
    }

    /// Fetch the viewer's current image.
    fn load_viewer_image(&mut self) {
        let Some(v) = &self.viewer else { return };
        let (client, tx, file) = (self.client.clone(), self.net.clone(), v.file().clone());
        tokio::spawn(async move {
            let id = file.id.clone();
            let result = super::viewer::load_full(client, file).await.map_err(|e| e.to_string());
            let _ = tx.send(Net::FullImage { id, result });
        });
    }

    /// Start loading thumbnails that were on screen but not cached yet.
    fn request_thumbs(&mut self) {
        let view = self.huddle_view.as_mut().map(|v| std::mem::take(&mut v.wanted)).unwrap_or_default();
        let wanted: Vec<_> = self.messages.wanted.drain(..).chain(self.thread_pane.wanted.drain(..)).chain(view).chain(self.call_bar_wanted.take()).chain(self.rail_wanted.drain(..)).collect();
        for (file, size) in wanted {
            if self.media.thumbs.contains_key(&file.id) {
                continue;
            }
            self.media.thumbs.insert(file.id.clone(), super::media::Thumb::Loading);
            let (client, tx, picker) = (self.client.clone(), self.net.clone(), self.media.picker.clone());
            tokio::spawn(async move {
                let id = file.id.clone();
                let result = super::media::load_thumb(client, picker, file, size).await.map_err(|e| format!("{e:#}"));
                let _ = tx.send(Net::Thumb { id, result });
            });
        }
    }

    /// Focus order for Tab: left to right, top to bottom.
    fn focus_order(&self) -> Vec<Focus> {
        // The huddle view stands in for the messages and the composer.
        let mut order = if self.huddle_view.is_some() { vec![Focus::Sidebar, Focus::Huddle] } else { vec![Focus::Sidebar, Focus::Messages, Focus::Composer] };
        // A narrow terminal's thread stands in for them.
        if self.narrow && self.thread.is_some() {
            order.truncate(1);
        }
        if self.thread.is_some() {
            order.extend([Focus::Thread, Focus::ThreadComposer]);
        }
        order
    }

    /// A dialog or popup is open over the panes (the composer's cursor must not show through).
    fn covered(&self) -> bool {
        self.menu.is_some()
            || self.help.is_some()
            || self.prefs.is_some()
            || self.switcher.is_some()
            || self.mover.is_some()
            || self.activity.is_some()
            || self.save.is_some()
            || self.threads.is_some()
            || self.profile.is_some()
            || self.details.is_some()
            || self.search.is_some()
            || self.player.is_some()
            || self.viewer.is_some()
            || self.snippet.is_some()
            || self.confirm.is_some()
            || self.picker.is_some()
    }

    /// The selected message of the focused pane.
    fn selected_ts(&self) -> Option<String> {
        match self.focus {
            Focus::Thread => self.thread_pane.selected.clone(),
            _ => self.messages.selected.clone(),
        }
    }

    /// Load older history once the top is visible.
    fn maybe_load_older(&mut self) {
        if let Some(id) = self.current.clone() {
            if self.messages.at_top() {
                self.load_history(&id, true);
            }
        }
    }
}

fn spawn_boot(client: Client, tx: mpsc::UnboundedSender<Net>) {
    tokio::spawn(async move {
        let result = async {
            let (boot, users, counts) = tokio::join!(client.user_boot(), client.users_list(), client.counts());
            anyhow::Ok(Net::Boot { boot: boot?, users: users?, counts: counts.ok() })
        }
        .await;
        let _ = tx.send(result.unwrap_or_else(|e| Net::Error(format!("loading workspace failed: {e}"))));
    });
}

/// The first of `candidates` (longest first) that fits in `room` columns, else the last.
fn fit_title(candidates: &[String], room: usize) -> String {
    candidates.iter().find(|c| c.chars().count() <= room).or(candidates.last()).cloned().unwrap_or_default()
}

/// The reply box title: the "also to channel" box and its key, shortened to fit `room`.
fn reply_title(broadcast: bool, key: &str, room: usize) -> String {
    let b = if broadcast { "x" } else { " " };
    fit_title(&[format!(" Reply · [{b}] also to channel ({key}) "), format!(" Reply · [{b}] also to channel "), format!(" Reply · [{b}] +channel "), format!(" [{b}] +channel ")], room)
}

/// A huddle invitation that rings until it is answered, cancelled or times out.
struct Ring {
    channel: String,
    room: String,
    user: String,
    started: std::time::Instant,
    last_ring: Option<std::time::Instant>,
}

/// How often the ring sound repeats.
const RING_EVERY: Duration = Duration::from_millis(2500);

/// "uploading a.mp4… 42% (3.1 of 7.4 MB)"; once all is sent, Slack still has to file it.
/// Now in epoch seconds, for `App::seen` (Slack's clock, not a debug offset).
fn opened_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// The most files Slack shares in one message.
const MAX_FILES: usize = 10;

/// Reports an upload's progress as `Net::UploadProgress`, at most every 250 ms (and at the end).
fn upload_progress(tx: mpsc::UnboundedSender<Net>, name: String) -> impl Fn(u64, u64) + Send + Sync + 'static {
    let last = std::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
    move |sent: u64, total: u64| {
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        if sent == total || last.elapsed() >= std::time::Duration::from_millis(250) {
            *last = std::time::Instant::now();
            let _ = tx.send(Net::UploadProgress { name: name.clone(), sent, total });
        }
    }
}

/// The status after pasting files: what was attached and what could not be.
fn attached_status(attached: &[String], skipped: &[String]) -> String {
    let mut parts = Vec::new();
    match attached {
        [] => {}
        [one] => parts.push(format!("attached {one}")),
        many => parts.push(format!("attached {} files", many.len())),
    }
    if !skipped.is_empty() {
        parts.push(format!("not attached: {}", skipped.join(", ")));
    }
    parts.join(" · ")
}

fn upload_status(name: &str, sent: u64, total: u64) -> String {
    if sent >= total {
        return format!("uploading {name}… sent, Slack is filing it");
    }
    let percent = sent * 100 / total.max(1);
    format!("uploading {name}… {percent}% ({} of {})", super::text::human_size(sent), super::text::human_size(total))
}

/// The reply a thread is read up to once it is shown: its newest, when its replies are loaded and
/// it has any.
fn thread_mark_target(t: &super::state::Timeline, parent: &str) -> Option<String> {
    t.messages.last().map(|m| m.ts.clone()).filter(|ts| t.loaded && ts != parent)
}

/// How far ← / → jump in a video, in seconds.
const SEEK_STEP: f64 = 10.0;

/// Whether a message at `ts` lies before the oldest loaded one (`first`): Slack timestamps
/// compare as numbers.
fn seek_older(ts: &str, first: &str) -> bool {
    match (ts.parse::<f64>(), first.parse::<f64>()) {
        (Ok(a), Ok(b)) => a < b,
        _ => false,
    }
}

impl App {
    /// Save the drafts once what is typed has not changed for [`DRAFT_SAVE_DELAY`], so a crash
    /// loses at most the last moments of typing.
    fn autosave_drafts(&mut self) {
        let now = (self.composer.draft(), self.thread_composer.draft());
        let (typed, thread_typed, since, saved) = &self.autosave;
        if (&now.0, &now.1) != (typed, thread_typed) {
            self.autosave = (now.0, now.1, std::time::Instant::now(), false);
        } else if draft_save_due(*saved, since.elapsed()) {
            self.save_drafts();
            self.autosave.3 = true;
        }
    }

    /// Save the drafts, with what is in the open composers, for the next run.
    fn save_drafts(&self) {
        let mut drafts = self.drafts.clone();
        if let Some((c, d)) = self.current.as_ref().zip(self.composer.draft()) {
            drafts.insert(c.clone(), d);
        }
        if let Some(((c, t), d)) = self.thread.as_ref().zip(self.thread_composer.draft()) {
            drafts.insert(super::state::thread_key(c, t), d);
        }
        super::drafts::save(&self.client.session.team_id, &self.client.session.user_id, &drafts);
    }
}

/// How long typing must pause before the drafts are saved.
const DRAFT_SAVE_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

/// Whether unsaved typing that has not changed for `quiet` is due to be saved.
fn draft_save_due(saved: bool, quiet: std::time::Duration) -> bool {
    !saved && quiet >= DRAFT_SAVE_DELAY
}

/// Keep what is typed in `composer` as the draft for `key` (or forget the old draft when
/// nothing is typed), leaving the composer empty.
fn stash(drafts: &mut super::drafts::Drafts, key: &str, composer: &mut super::composer::Composer) {
    match composer.take_draft() {
        Some(d) => drafts.insert(key.to_string(), d),
        None => drafts.remove(key),
    };
}

/// Run the app on the profile's workspace `team` (or its default); Alt+1…9 and `/workspace`
/// switch to another of the profile's workspaces without leaving the terminal.
pub async fn run(profile: slack_core::session::Profile, team: Option<&str>) -> Result<()> {
    let sessions = profile.workspaces.clone();
    let start = profile.workspace(team).ok_or_else(|| anyhow::anyhow!("no such workspace in this profile"))?;
    let mut index = sessions.iter().position(|s| s.team_id == start.team_id && s.user_id == start.user_id).unwrap_or(0);
    let labels = workspace_labels(&sessions);
    let keymap = super::keymap::load_user();
    let shown = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(index));
    let badges: super::others::Badges = Default::default();
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    super::others::spawn(sessions.clone(), shown.clone(), badges.clone(), wake.clone());
    let (clicks_tx, mut clicks) = mpsc::unbounded_channel();
    super::others::spawn_watchers(sessions.clone(), labels.clone(), shown.clone(), clicks_tx);
    let open_after: super::others::OpenAfter = Default::default();
    let logos: super::others::Logos = Default::default();
    super::others::spawn_logos(sessions.clone(), logos.clone());

    for w in &keymap.1 {
        tracing::warn!("{w}");
    }
    tokio::task::spawn_blocking(super::highlight::warm_up);
    let (_guard, mut terminal) = TerminalGuard::enter()?;
    // Query the terminal before the event stream starts reading stdin.
    let mut media = super::media::Media::detect(&super::settings::load().0.messages.graphics);
    let mut clipboard = super::clipboard::Clipboard::default();
    let mut events = EventStream::new();
    loop {
        let client = Client::new(sessions[index].clone())?;
        shown.store(index, std::sync::atomic::Ordering::Relaxed);
        badges.lock().unwrap_or_else(|e| e.into_inner()).remove(&index);
        // The workspace just left gets its badge now, not at the next poll.
        wake.notify_one();
        // Read again on every switch, so changes made in the preferences carry over.
        let (settings, settings_problem) = super::settings::load();
        let mut settings_problem = settings_problem;
        if let Err(e) = super::theme::set(settings.appearance.theme_for(&sessions[index].team_id)) {
            settings_problem.get_or_insert(e);
        }
        if let Err(e) = super::clock::set(&settings.region.timezone) {
            settings_problem.get_or_insert(e);
        }
        if let Err(e) = super::i18n::set(&settings.region.language) {
            settings_problem.get_or_insert(e);
        }
        // Keys may have been rebound in the preferences meanwhile.
        let mut keymap = super::keymap::load_user();
        keymap.1.extend(settings_problem);
        let (next, kept_media, kept_clipboard) = run_workspace(client, (labels.clone(), index, badges.clone(), open_after.clone(), logos.clone()), &mut clicks, &keymap, &settings, &mut terminal, &mut events, media, clipboard).await?;
        (media, clipboard) = (kept_media, kept_clipboard);
        match next {
            Some(i) => index = i,
            None => break,
        }
    }
    clipboard.hand_off();
    Ok(())
}

/// How each workspace is named in the app: the team, plus the account when one team is
/// signed in more than once.
fn workspace_labels(sessions: &[slack_core::session::WorkspaceSession]) -> Vec<String> {
    sessions
        .iter()
        .map(|s| if sessions.iter().filter(|o| o.team_id == s.team_id).count() > 1 { format!("{} · {}", s.team_name, s.user_name) } else { s.team_name.clone() })
        .collect()
}

/// One workspace until quit (None) or a switch (Some(index)); hands back what outlives it.
async fn run_workspace(
    client: Client,
    workspaces: (Vec<String>, usize, super::others::Badges, super::others::OpenAfter, super::others::Logos),
    clicks: &mut mpsc::UnboundedReceiver<super::others::Click>,
    keymap: &(Keymap, Vec<String>),
    settings: &super::settings::Settings,
    terminal: &mut super::terminal::Tui,
    events: &mut EventStream,
    mut media: super::media::Media,
    clipboard: super::clipboard::Clipboard,
) -> Result<(Option<usize>, super::media::Media, super::clipboard::Clipboard)> {
    let (net_tx, mut net_rx) = mpsc::unbounded_channel();
    spawn_boot(client.clone(), net_tx.clone());
    let mut rtm = slack_core::rtm::spawn(client.clone());
    media.thumbs.clear();
    let team_name = if workspaces.0.len() > 1 { format!("{}/{} {}", workspaces.1 + 1, workspaces.0.len(), workspaces.0[workspaces.1]) } else { client.session.team_name.clone() };
    let mut app = App {
        keys: keymap.0.clone(),
        key_warnings: keymap.1.clone(),
        pending: Pending::default(),
        pending_since: None,
        which_key: None,
        workspaces: workspaces.0,
        workspace: workspaces.1,
        others: workspaces.2,
        rail: Vec::new(),
        open_after: workspaces.3,
        logos: workspaces.4,
        rail_wanted: Vec::new(),
        switch_to: None,
        media,
        viewer: None,
        snippet: None,
        help: None,
        switcher: None,
        search: None,
        details: None,
        profile: None,
        activity: None,
        save: None,
        threads: None,
        preview_polled: None,
        prefs: None,
        huddle_view: None,
        call_bar_area: Default::default(),
        call_bar_wanted: None,
        menu: None,
        dnd_refreshing: false,
        mover: None,
        huddle_after_open: None,
        rejoined: false,
        autosave: (None, None, std::time::Instant::now(), true),
        presence_subscribed: false,
        call: None,
        pending_hit: None,
        ringing: None,
        seeking: None,
        drafts: super::drafts::load(&client.session.team_id, &client.session.user_id),
        player: None,
        screen: Default::default(),
        narrow: false,
        terminal_focused: true,
        completion: None,
        completion_dismissed: None,
        path_suggestions: None,
        seen: Default::default(),
        composer_area: Default::default(),
        thread_composer_area: Default::default(),
        thread: None,
        thread_pane: MessagePane::thread(),
        thread_composer: Composer::default(),
        broadcast: false,
        clipboard,
        confirm: None,
        confirm_area: Default::default(),
        picker: None,
        rtm_commands: rtm.commands.clone(),
        connected: false,
        last_typing_sent: None,
        marked: Default::default(),
        focus: Focus::Composer,
        composer: Composer::default(),
        client: client.clone(),
        net: net_tx.clone(),
        ws: Workspace {
            team_name,
            team_id: client.session.team_id.clone(),
            self_id: client.session.user_id.clone(),
            settings: settings.clone(),
            ..Default::default()
        },
        sidebar: Sidebar::for_account(&client.session.team_id, &client.session.user_id),
        messages: MessagePane::default(),
        current: None,
        status: "loading workspace…".into(),
        status_shown: (String::new(), std::time::Instant::now()),
        loading: true,
    };

    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut video_tick = tokio::time::interval(Duration::from_millis(40));
    // Redraws the call bar's level meters.
    let mut call_tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        app.expire_status(std::time::Instant::now());
        app.ring_tick();
        app.sync_which_key(std::time::Instant::now());
        // Alone in the huddle for a while: a quiet melody loops until someone joins.
        let s = &app.ws.settings;
        let wait = Duration::from_secs(s.huddle.alone_secs.max(1) as u64);
        let (name, device, volume) = (s.huddle.alone_sound.clone(), super::settings::Settings::device(huddle::devices::OUTPUT_ENV, &s.audio.output), s.audio.sound_volume * super::prefs::ALONE_VOLUME / 100);
        if let Some(call) = app.call.as_mut().filter(|c| c.connected) {
            if call.alone_tick(wait, || super::sound::play_looping(&name, device, volume)) {
                tracing::debug!("alone in the huddle: reminder loops");
            }
        }
        terminal.draw(|f| {
            draw(f, &mut app);
            super::media::reattach_pictures(f.buffer_mut());
        })?;
        app.request_thumbs();
        app.request_activity_messages();
        app.refresh_dnd();
        let which_key_due = app.which_key_due();
        tokio::select! {
            ev = events.next() => {
                let Some(ev) = ev else { break };
                if !handle_terminal(&mut app, ev?) {
                    break;
                }
                app.refresh_completion();
            }
            Some(msg) = net_rx.recv() => handle_net(&mut app, msg),
            Some((index, target)) = clicks.recv() => {
                if index == app.workspace {
                    app.open_target(target);
                } else {
                    *app.open_after.lock().unwrap_or_else(|e| e.into_inner()) = Some((index, target));
                    app.switch_workspace(index);
                }
            }
            Some(ev) = rtm.events.recv() => handle_rtm(&mut app, ev),
            Some(ev) = async {
                match app.call.as_mut() {
                    Some(call) => call.session.events.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                // The join sound, once the huddle is connected.
                if matches!(ev, huddle::session::SessionEvent::Connected) && !app.call.as_ref().is_some_and(|c| c.connected) {
                    let s = &app.ws.settings;
                    super::sound::play(&s.huddle.join_sound, super::settings::Settings::device(huddle::devices::OUTPUT_ENV, &s.audio.output), s.audio.sound_volume);
                }
                let ended = app.call.as_mut().is_some_and(|c| c.apply(ev));
                // Someone joined or left while we are in the huddle.
                if let Some((joined, left)) = app.call.as_mut().map(|c| c.take_roster_changes()) {
                    let s = &app.ws.settings;
                    let name = if joined > 0 { Some(&s.huddle.someone_joined) } else if left > 0 { Some(&s.huddle.someone_left) } else { None };
                    if let Some(name) = name.filter(|_| !ended) {
                        super::sound::play(name, super::settings::Settings::device(huddle::devices::OUTPUT_ENV, &s.audio.output), s.audio.sound_volume);
                    }
                }
                if ended {
                    let call = app.call.take();
                    app.status = "left the huddle".into();
                    // Right after a huddle emptied, Slack can still hand out its room though the
                    // meeting is over: the session ends before connecting. Ask again once.
                    if let Some(call) = call {
                        match rejoin_after_end(call.connected, call.leaving, app.rejoined) {
                            Some(true) => {
                                tracing::info!(channel = call.channel, "the huddle ended before connecting; joining again");
                                app.rejoined = true;
                                app.join_huddle_after(call.channel, call.thread, REJOIN_DELAY);
                            }
                            Some(false) => app.status = "cannot join the huddle: it ended as we joined".into(),
                            None => {}
                        }
                    }
                }
            }
            _ = tick.tick() => {}
            // which-key opens (at the top of the loop).
            _ = tokio::time::sleep_until(which_key_due.unwrap_or_else(std::time::Instant::now).into()), if which_key_due.is_some() => {}
            _ = call_tick.tick(), if app.call.is_some() => {}
            _ = video_tick.tick(), if app.player.is_some() => {
                if let Some(p) = app.player.as_mut() {
                    p.advance();
                }
            }
        }
        if app.switch_to.is_some() {
            break;
        }
        app.sync_current();
        app.autosave_drafts();
        if app.current.as_ref().and_then(|c| app.ws.timelines.get(c)).is_some_and(|t| t.loaded) {
            app.mark_current_read();
        }
        app.mark_thread_read();
        app.refresh_preview();
    }
    app.save_drafts();
    let App { switch_to, media, clipboard, .. } = app;
    Ok((switch_to, media, clipboard))
}

/// Returns false to quit.
fn handle_terminal(app: &mut App, ev: Event) -> bool {
    if let Event::Key(k) = &ev {
        if k.kind != KeyEventKind::Release && app.hold_sequence(k) {
            return true;
        }
    }
    let keep_going = handle_event(app, ev);
    app.pending.clear();
    keep_going
}

fn handle_event(app: &mut App, ev: Event) -> bool {
    if let Event::FocusGained | Event::FocusLost = ev {
        app.terminal_focused = matches!(ev, Event::FocusGained);
        if !app.terminal_focused {
            app.sidebar.hover = None;
            app.messages.hover = None;
            app.thread_pane.hover = None;
        }
        tracing::debug!(focused = app.terminal_focused, "terminal focus");
        return true;
    }
    if let Event::Key(k) = &ev {
        if k.kind != KeyEventKind::Release && !app.capturing_key() && app.resolve(k) == Resolved::Action(Context::Global, Action::Quit) {
            return false;
        }
    }
    match app.mode() {
        Mode::Dialog => handle_dialog(app, &ev),
        Mode::Normal | Mode::Insert => handle_main(app, ev),
    }
}

/// Input while a dialog is open: the topmost dialog takes every key and click; nothing under
/// it reacts.
fn handle_dialog(app: &mut App, ev: &Event) -> bool {
    // The global leader sequences (Ctrl+Space h m mutes with a dialog open).
    if let Event::Key(k) = ev {
        if k.kind != KeyEventKind::Release && !app.capturing_key() {
            match app.resolve(k) {
                Resolved::Action(Context::Global, action) if opens_something(action) => {
                    app.status = "close the dialog first (Esc)".into();
                    return true;
                }
                Resolved::Action(Context::Global, action) => return run_global(app, action),
                _ => {}
            }
        }
    }
    match ev {
        Event::Key(k) if k.kind != KeyEventKind::Release && app.menu.is_some() => {
            let action = app.action_in(Context::Menu, &k);
            let Some(m) = app.menu.as_mut() else { return true };
            match action {
                Some(Action::Up) => m.move_by(-1),
                Some(Action::Down) => m.move_by(1),
                Some(Action::Open) => app.run_menu(),
                Some(Action::Close) => app.menu = None,
                _ => {}
            }
        }
        // The reaction picker: a click on an emoji reacts with it, a click outside closes it.
        Event::Mouse(m) if app.picker.is_some() && matches!(m.kind, MouseEventKind::Down(_)) => {
            let Some((picker, _, _)) = app.picker.as_mut() else { return true };
            let len = picker.candidates(&app.ws.custom_emoji).len();
            match app.picker.as_ref().map(|(p, _, _)| (p.row_at(m.column, m.row).filter(|n| *n < len), p.area.contains((m.column, m.row).into()))) {
                Some((Some(n), _)) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
                    if let Some((picker, channel, ts)) = app.picker.take() {
                        if let Some(choice) = picker.candidates(&app.ws.custom_emoji).get(n) {
                            app.toggle_reaction(channel, ts, choice.name.clone());
                        }
                    }
                }
                Some((_, false)) => app.picker = None,
                _ => {}
            }
        }
        Event::Mouse(m) if app.menu.is_some() && matches!(m.kind, MouseEventKind::Moved | MouseEventKind::Down(_)) => {
            let hit = app.menu.as_ref().and_then(|menu| menu.item_at(m.column, m.row));
            match (m.kind, hit) {
                (MouseEventKind::Moved, Some(n)) => {
                    if let Some(menu) = app.menu.as_mut() {
                        menu.selected = n;
                    }
                }
                (MouseEventKind::Down(MouseButton::Left), Some(n)) => {
                    if let Some(menu) = app.menu.as_mut() {
                        menu.selected = n;
                    }
                    app.run_menu();
                }
                (MouseEventKind::Down(_), None) => app.menu = None,
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.help.is_some() => {
            let action = app.action_in(Context::Help, &k);
            let Some(h) = app.help.as_mut() else { return true };
            match action {
                Some(Action::Up) => h.scroll_by(-1),
                Some(Action::Down) => h.scroll_by(1),
                Some(Action::PageUp) => h.scroll_by(-h.page()),
                Some(Action::PageDown) => h.scroll_by(h.page()),
                Some(Action::Close) => app.help = None,
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.prefs.as_ref().is_some_and(|p| p.capturing.is_some()) => app.capture_key(Key::from_event(&k)),
        Event::Key(k) if k.kind != KeyEventKind::Release && app.prefs.as_ref().is_some_and(|p| p.picker.is_some()) => app.theme_picker_key(&k),
        Event::Key(k) if k.kind != KeyEventKind::Release && app.prefs.as_ref().is_some_and(|p| p.editing.is_some()) => {
            let action = app.action_in(Context::TextField, &k);
            let Some(p) = app.prefs.as_mut() else { return true };
            match (action, p.editing.as_mut()) {
                (Some(Action::Close), _) => p.editing = None,
                (Some(Action::Erase), Some(text)) => {
                    text.pop();
                }
                (Some(Action::Open), _) => app.change_preference(),
                (_, Some(text)) => {
                    if let (KeyCode::Char(c), true) = (k.code, Key::from_event(&k).is_text()) {
                        text.push(c);
                    }
                }
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.prefs.is_some() => {
            let action = app.action_in(Context::Preferences, &k);
            let Some(p) = app.prefs.as_mut() else { return true };
            match action {
                Some(Action::Close) => app.prefs = None,
                Some(Action::Up) => p.move_by(-1),
                Some(Action::Down) => p.move_by(1),
                Some(Action::PrevTab) => p.switch_category(-1),
                Some(Action::NextTab) => p.switch_category(1),
                Some(Action::Open) => app.change_preference(),
                Some(Action::Increase) => app.adjust_preference(true),
                Some(Action::Decrease) => app.adjust_preference(false),
                Some(Action::Unbind) => match p.selected_field() {
                    Some(super::prefs::Field::Binding(i)) => {
                        let info = &super::keymap::ACTIONS[i];
                        let text = app.keys.to_toml(Some((info.context, info.name, vec![])), None);
                        app.save_keys(&text);
                    }
                    Some(super::prefs::Field::Leader) => {
                        let text = app.keys.to_toml(None, Some(None));
                        app.save_keys(&text);
                    }
                    _ => {}
                },
                Some(Action::Reset) => {
                    if let Some(super::prefs::Field::Binding(i)) = p.selected_field() {
                        let info = &super::keymap::ACTIONS[i];
                        let defaults = Keymap::load("").0.sequences(info.context, info.name);
                        let text = app.keys.to_toml(Some((info.context, info.name, defaults)), None);
                        app.save_keys(&text);
                    }
                }
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.switcher.is_some() => {
            let len = app.switcher.as_ref().map(|s| s.entries(&app.ws).len()).unwrap_or(0);
            let action = app.action_in(Context::Switcher, &k);
            let Some(sw) = app.switcher.as_mut() else { return true };
            match action {
                Some(Action::Close) => app.switcher = None,
                Some(Action::Up) => sw.move_by(-1, len),
                Some(Action::Down) => sw.move_by(1, len),
                Some(Action::PageUp) => sw.move_by(-(sw.height.max(1) as isize), len),
                Some(Action::PageDown) => sw.move_by(sw.height.max(1) as isize, len),
                Some(Action::Open) => app.switch(),
                Some(Action::Erase) => {
                    sw.query.pop();
                    sw.selected = 0;
                }
                _ => {
                    if let (KeyCode::Char(c), true) = (k.code, Key::from_event(&k).is_text()) {
                        sw.query.push(c);
                        sw.selected = 0;
                    }
                }
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.mover.is_some() => {
            let naming = app.mover.as_ref().is_some_and(|m| m.naming.is_some());
            let action = app.action_in(if naming { Context::TextField } else { Context::Mover }, &k);
            let Some(m) = app.mover.as_mut() else { return true };
            let on_new = *m.choice() == super::sections::Choice::New;
            match (&mut m.naming, action) {
                (Some(_), Some(Action::Close)) => m.naming = None,
                (Some(name), Some(Action::Erase)) => {
                    name.pop();
                }
                (Some(_), Some(Action::Open)) => app.finish_move(),
                (Some(name), _) => {
                    if let (KeyCode::Char(c), true) = (k.code, Key::from_event(&k).is_text()) {
                        name.push(c);
                    }
                }
                (None, Some(Action::Close)) => app.mover = None,
                (None, Some(Action::Up)) => m.move_by(-1),
                (None, Some(Action::Down)) => m.move_by(1),
                (None, Some(Action::Top)) => m.move_by(isize::MIN / 2),
                (None, Some(Action::Bottom)) => m.move_by(isize::MAX / 2),
                (None, Some(Action::Open)) if on_new => m.naming = Some(String::new()),
                (None, Some(Action::Open)) => app.finish_move(),
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.save.is_some() => {
            let editing = app.save.as_ref().is_some_and(|d| d.editing);
            let (field, save) = (app.action_in(Context::TextField, &k), app.action_in(Context::Save, &k));
            let Some(d) = app.save.as_mut() else { return true };
            if editing {
                match field {
                    _ if save == Some(Action::Folder) => d.complete(),
                    Some(Action::Open) => app.save_ticked(),
                    Some(Action::Close) => d.editing = false,
                    Some(Action::Erase) => {
                        d.folder.pop();
                    }
                    _ => {
                        if let (KeyCode::Char(c), true) = (k.code, Key::from_event(&k).is_text()) {
                            d.folder.push(c);
                        }
                    }
                }
            } else {
                match save {
                    Some(Action::Up) => d.move_by(-1),
                    Some(Action::Down) => d.move_by(1),
                    Some(Action::Tick) => d.tick(),
                    Some(Action::TickAll) => d.tick_all(),
                    Some(Action::Folder) => d.editing = true,
                    Some(Action::Open) => app.save_ticked(),
                    Some(Action::Close) => app.save = None,
                    _ => {}
                }
            }
        }
        // The save dialog: a click on a file ticks it, one outside closes the dialog.
        Event::Mouse(m) if app.save.is_some() && matches!(m.kind, MouseEventKind::Down(_)) => {
            let Some(d) = app.save.as_mut() else { return true };
            match d.row_at(m.column, m.row) {
                Some(n) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
                    d.selected = n;
                    d.tick();
                }
                None if !d.area.contains((m.column, m.row).into()) => app.save = None,
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.activity.is_some() => {
            let action = app.action_in(Context::Activity, &k);
            let Some(a) = app.activity.as_mut() else { return true };
            match action {
                Some(Action::Close) => app.activity = None,
                Some(Action::Up) => a.move_by(-1),
                Some(Action::Down) => a.move_by(1),
                Some(Action::PageUp) => a.move_by(-(a.height.max(1) as isize)),
                Some(Action::PageDown) => a.move_by(a.height.max(1) as isize),
                Some(Action::Refresh) => app.open_activity(),
                Some(Action::Open) => {
                    if let Some(key) = a.selected_item().map(|it| it.key.clone()) {
                        app.seen.insert(key, opened_now());
                    }
                    if let Some((channel, ts, thread_ts)) = a.target() {
                        app.activity = None;
                        app.open_hit(super::search::Hit { channel, ts, thread_ts, user: None, text: String::new() });
                    }
                }
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.threads.is_some() => {
            let action = app.action_in(Context::Threads, &k);
            let Some(t) = app.threads.as_mut() else { return true };
            match action {
                Some(Action::Close) => app.threads = None,
                Some(Action::Up) => t.move_by(-1),
                Some(Action::Down) => t.move_by(1),
                Some(Action::PageUp) => t.move_by(-(t.height.max(1) as isize)),
                Some(Action::PageDown) => t.move_by(t.height.max(1) as isize),
                Some(Action::Refresh) => app.load_threads(false),
                Some(Action::Open) => {
                    if let Some((channel, ts, thread_ts)) = t.target() {
                        app.threads = None;
                        app.open_hit(super::search::Hit { channel, ts, thread_ts, user: None, text: String::new() });
                    }
                }
                _ => {}
            }
            // At the end of the list: the next page.
            if app.threads.as_ref().is_some_and(|t| t.at_end()) {
                app.load_threads(true);
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.profile.is_some() => {
            let action = app.action_in(Context::Profile, &k);
            let Some(card) = app.profile.as_mut() else { return true };
            match action {
                Some(Action::Close) => app.profile = None,
                Some(Action::PrevTab) => card.step(-1),
                Some(Action::NextTab) => card.step(1),
                Some(Action::Open) => {
                    let user = card.user().to_string();
                    app.profile = None;
                    app.details = None;
                    if user == app.ws.self_id {
                        app.status = "that is you".into();
                    } else {
                        app.open_dm_with(user);
                    }
                }
                Some(Action::Huddle) => {
                    let user = card.user().to_string();
                    app.profile = None;
                    app.details = None;
                    app.huddle_with(user);
                }
                Some(Action::Vip) => {
                    let user = card.user().to_string();
                    app.toggle_vip(user);
                }
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.details.is_some() => {
            let action = app.action_in(Context::Details, &k);
            let Some(d) = app.details.as_mut() else { return true };
            match action {
                Some(Action::Close) => app.details = None,
                Some(Action::PrevTab) => d.switch_tab(-1),
                Some(Action::NextTab) => d.switch_tab(1),
                Some(Action::Up) => d.move_by(-1),
                Some(Action::Down) => d.move_by(1),
                Some(Action::Open) => app.details_enter(),
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.search.is_some() => {
            let action = app.action_in(Context::Search, &k);
            let Some(search) = app.search.as_mut() else { return true };
            match action {
                Some(Action::Close) => app.search = None,
                Some(Action::Up) => search.move_by(-1),
                Some(Action::Down) => {
                    search.move_by(1);
                    app.search_more();
                }
                Some(Action::Open) => app.search_enter(),
                Some(Action::Erase) => {
                    search.query.pop();
                }
                _ => {
                    if let (KeyCode::Char(c), true) = (k.code, Key::from_event(&k).is_text()) {
                        search.query.push(c);
                    }
                }
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.player.is_some() => {
            let action = app.action_in(Context::Player, &k);
            let Some(p) = app.player.as_mut() else { return true };
            match action {
                Some(Action::Close) => app.player = None,
                Some(Action::Pause) => p.toggle_pause(),
                Some(Action::SeekBack) => p.seek(-SEEK_STEP),
                Some(Action::SeekForward) => p.seek(SEEK_STEP),
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.snippet.is_some() => {
            let action = app.action_in(Context::Snippet, &k);
            let Some(v) = app.snippet.as_mut() else { return true };
            match action {
                Some(Action::Close) => app.snippet = None,
                Some(Action::Up) => v.scroll_by(-1),
                Some(Action::Down) => v.scroll_by(1),
                Some(Action::PageUp) => v.scroll_by(-v.page()),
                Some(Action::PageDown) => v.scroll_by(v.page()),
                Some(Action::Top) => v.scroll_by(isize::MIN / 2),
                Some(Action::Bottom) => v.scroll_by(isize::MAX / 2),
                Some(Action::Copy) => {
                    if let Some(Ok(text)) = &v.text {
                        let how = app.clipboard.copy(text);
                        app.status = format!("copied the snippet to the {how}");
                    }
                }
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.viewer.is_some() => {
            let action = app.action_in(Context::Viewer, &k);
            let Some(v) = app.viewer.as_mut() else { return true };
            match action {
                Some(Action::Close) => app.viewer = None,
                Some(Action::PrevTab) if v.step(-1) => app.load_viewer_image(),
                Some(Action::NextTab) if v.step(1) => app.load_viewer_image(),
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.confirm.is_some() => {
            let yes = app.action_in(Context::Confirm, &k) == Some(Action::Yes);
            if let Some(confirm) = app.confirm.take().filter(|_| yes) {
                match confirm {
                    Confirm::Delete { channel, ts } => app.delete(channel, ts),
                    Confirm::Leave(channel) => app.leave_channel(channel),
                    Confirm::DeleteSection { id, .. } => app.delete_section(id),
                }
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.picker.is_some() => {
            let custom = app.ws.custom_emoji.clone();
            let action = app.action_in(Context::Picker, &k);
            let Some((picker, _, _)) = app.picker.as_mut() else { return true };
            let len = picker.candidates(&custom).len();
            match action {
                Some(Action::Close) => app.picker = None,
                Some(Action::Up) => picker.move_by(-1, len),
                Some(Action::Down) => picker.move_by(1, len),
                Some(Action::PageUp) => picker.move_by(-10, len),
                Some(Action::PageDown) => picker.move_by(10, len),
                Some(Action::Erase) => {
                    picker.query.pop();
                    picker.selected = 0;
                }
                Some(Action::Open) => {
                    if let Some((picker, channel, ts)) = app.picker.take() {
                        if let Some(choice) = picker.candidates(&custom).get(picker.selected) {
                            app.toggle_reaction(channel, ts, choice.name.clone());
                        }
                    }
                }
                _ => {
                    if let (KeyCode::Char(c), true) = (k.code, Key::from_event(&k).is_text()) {
                        picker.query.push(c);
                        picker.selected = 0;
                    }
                }
            }
        }
        Event::Mouse(m) if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) => {
            app.wheel_in_popup(m.kind == MouseEventKind::ScrollDown);
        }
        // A click outside the dialog closes it; one inside does nothing.
        Event::Mouse(m) if matches!(m.kind, MouseEventKind::Down(_)) => {
            if !app.dialog_area().is_some_and(|a| a.contains((m.column, m.row).into())) {
                app.close_dialog();
            }
        }
        _ => {}
    }
    true
}

/// Input on the main screen (no dialog open): the global keys, the suggestion list, the
/// focused pane, the mouse.
fn handle_main(app: &mut App, ev: Event) -> bool {
    match ev {
        Event::Key(k) if k.kind != KeyEventKind::Release => {
            // The most specific context decides: the suggestion list, the focused pane, then
            // global (so Tab accepts a suggestion, and Ctrl+A in a message box is line start).
            let resolved = app.resolve(&k);
            match resolved {
                Resolved::Action(Context::Global, action) => return run_global(app, action),
                Resolved::Action(Context::Sidebar | Context::Messages, Action::Help) => {
                    app.help = Some(super::help::Help::default());
                    return true;
                }
                Resolved::Action(Context::Completion, action) => {
                    // A command or choice typed out in full sends on the send key, as if the list were not
                    // there (mentions are accepted, to be sent as such).
                    let typed_command = app.completion.as_ref().and_then(|c| {
                        let s = c.items.get(c.selected)?;
                        let composer = if c.thread { &app.thread_composer } else { &app.composer };
                        (s.id.is_none() && s.replacement == composer.token(c.start)).then_some(c.thread)
                    });
                    let send_key = app.action_in(Context::Composer, &k) == Some(Action::Send);
                    let Some(c) = app.completion.as_mut() else { return true };
                    match action {
                        Action::Up => c.move_by(-1),
                        Action::Down => c.move_by(1),
                        Action::Accept if send_key && typed_command.is_some() => {
                            app.completion = None;
                            app.send(typed_command.unwrap_or_default());
                        }
                        Action::Accept => app.accept_completion(),
                        _ => {
                            app.completion_dismissed = Some((c.thread, c.start));
                            app.completion = None;
                        }
                    }
                    return true;
                }
                _ => {}
            }
            let action = match resolved {
                Resolved::Action(_, a) => Some(a),
                Resolved::Pending | Resolved::None => None,
            };
            match app.focus {
                Focus::Sidebar => {
                    if let Some(action) = action {
                        app.sidebar_action(action);
                    }
                }
                Focus::Huddle => {
                    use super::huddle_view::Dir;
                    if let Some(v) = app.huddle_view.as_mut() {
                        match action {
                            Some(Action::Up) => v.move_selection(Dir::Up),
                            Some(Action::Down) => v.move_selection(Dir::Down),
                            Some(Action::PrevTab) => v.move_selection(Dir::Left),
                            Some(Action::NextTab) => v.move_selection(Dir::Right),
                            Some(Action::Open) => v.toggle_maximize(),
                            Some(Action::Close) if !v.back() => app.focus = Focus::Sidebar,
                            _ => {}
                        }
                    }
                }
                Focus::Messages | Focus::Thread => {
                    if let Some(action) = action {
                        app.message_action(action, app.focus == Focus::Thread);
                    }
                }
                Focus::Composer | Focus::ThreadComposer => {
                    let thread = app.focus == Focus::ThreadComposer;
                    if thread && action == Some(Action::AlsoToChannel) {
                        app.broadcast = !app.broadcast;
                        return true;
                    }
                    let composer = if thread { &mut app.thread_composer } else { &mut app.composer };
                    match composer.handle_resolved(k, action) {
                        KeyOutcome::Done => {}
                        KeyOutcome::Typed => app.notify_typing(thread),
                        KeyOutcome::Submit if app.composer_for(thread).editing.is_some() => app.save_edit(thread),
                        KeyOutcome::Submit => app.send(thread),
                        KeyOutcome::Leave => app.leave_composer(thread),
                        KeyOutcome::UpWhenEmpty => {
                            if let Some(ts) = app.last_own_message(thread) {
                                app.start_edit(&ts, thread);
                            }
                        }
                        KeyOutcome::Paste => app.paste(thread),
                    }
                }
            }
        }
        Event::Paste(text) if app.focus == Focus::Composer => app.composer.insert_str(&text.replace("\r\n", "\n")),
        Event::Paste(text) if app.focus == Focus::ThreadComposer => app.thread_composer.insert_str(&text.replace("\r\n", "\n")),
        Event::Mouse(m) if m.kind == MouseEventKind::Moved => {
            let (c, r) = (m.column, m.row);
            app.sidebar.hover = app.sidebar.item_at(c, r);
            app.messages.hover = app.messages.message_at(c, r);
            app.thread_pane.hover = if app.thread.is_some() { app.thread_pane.message_at(c, r) } else { None };
        }
        Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Right) => app.open_menu(m.column, m.row),
        Event::Mouse(m) => match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let picked = app.completion.as_ref().and_then(|c| c.row_at(if c.thread { app.thread_composer_area } else { app.composer_area }, m.column, m.row));
                let close = [(false, app.composer_area), (true, app.thread_composer_area)]
                    .into_iter()
                    .filter(|&(thread, _)| !thread || app.thread.is_some())
                    .find_map(|(thread, area)| attachment_at(area, if thread { &app.thread_composer } else { &app.composer }.attachment_lines().1, m.column, m.row).map(|i| (thread, i)));
                if let Some(n) = picked {
                    if let Some(c) = app.completion.as_mut() {
                        c.selected = n;
                    }
                    app.accept_completion();
                } else if let Some((thread, i)) = close {
                    let removed = app.composer_for(thread).attachments.remove(i);
                    app.status = format!("removed {}", removed.name);
                } else if app.huddle_view.as_mut().is_some_and(|v| v.click(m.column, m.row)) {
                    app.focus = Focus::Huddle;
                } else if let Some(i) = app.rail.iter().position(|r| r.contains((m.column, m.row).into())) {
                    app.switch_workspace(i);
                } else if app.sidebar.threads_at(m.column, m.row) {
                    app.load_threads(false);
                } else if let Some(name) = app.sidebar.header_at(m.column, m.row) {
                    app.sidebar.toggle_fold_keeping(&app.ws, &name, app.current.as_deref());
                } else if app.sidebar.click(m.column, m.row) {
                    app.focus = Focus::Composer;
                } else if app.messages.click(m.column, m.row) {
                    app.focus = Focus::Messages;
                } else if app.thread.is_some() && app.thread_pane.click(m.column, m.row) {
                    app.focus = Focus::Thread;
                } else if app.call.is_some() && app.call_bar_area.contains((m.column, m.row).into()) {
                    app.toggle_huddle_view();
                }
            }
            // The huddle view's tiles, when they do not all fit (the layout clamps the scroll).
            MouseEventKind::ScrollUp if app.huddle_view.as_ref().is_some_and(|v| v.tiles_area.contains((m.column, m.row).into())) => {
                if let Some(v) = app.huddle_view.as_mut() {
                    v.scroll = v.scroll.saturating_sub(1);
                }
            }
            MouseEventKind::ScrollDown if app.huddle_view.as_ref().is_some_and(|v| v.tiles_area.contains((m.column, m.row).into())) => {
                if let Some(v) = app.huddle_view.as_mut() {
                    v.scroll += 1;
                }
            }
            MouseEventKind::ScrollUp if app.sidebar.area.contains((m.column, m.row).into()) => app.sidebar.move_by(-1),
            MouseEventKind::ScrollDown if app.sidebar.area.contains((m.column, m.row).into()) => app.sidebar.move_by(1),
            MouseEventKind::ScrollUp if app.messages.area.contains((m.column, m.row).into()) => {
                app.messages.scroll_up(3);
                app.maybe_load_older();
            }
            MouseEventKind::ScrollDown if app.messages.area.contains((m.column, m.row).into()) => app.messages.scroll_down(3),
            MouseEventKind::ScrollUp if app.thread.is_some() && app.thread_pane.area.contains((m.column, m.row).into()) => app.thread_pane.scroll_up(3),
            MouseEventKind::ScrollDown if app.thread.is_some() && app.thread_pane.area.contains((m.column, m.row).into()) => app.thread_pane.scroll_down(3),
            _ => {}
        },
        _ => {}
    }
    true
}

/// Global actions that open a view, a dialog or the message box: none of them opens over a
/// dialog (the insert leader reaches them there).
fn opens_something(action: Action) -> bool {
    use Action::*;
    matches!(action, Switcher | BrowseChannels | Search | Threads | Activity | Details | MoveToSection | Help | Preferences | LeaveConversation | DeleteSection | RenameSection | NewChannel | NewPrivateChannel | NewDm | NewSnippet | Upload | SetStatus | PauseNotifications)
}

/// What a global key does. False to quit.
fn run_global(app: &mut App, action: Action) -> bool {
    match action {
        Action::Quit => return false,
        Action::Help => app.help = Some(super::help::Help::default()),
        Action::Preferences => app.prefs = Some(super::prefs::Prefs::new()),
        Action::Switcher => app.switcher = Some(super::switcher::Switcher::default()),
        Action::Search => app.search = Some(super::search::Search::default()),
        Action::Activity => app.open_activity(),
        Action::Threads => app.load_threads(false),
        Action::Details => app.open_details(),
        Action::Star => {
            if let Some(id) = app.current.clone() {
                app.toggle_star(id);
            }
        }
        Action::MoveToSection => match app.current.clone() {
            Some(id) if app.ws.is_outside(&id) => app.status = "join the channel to file it in a section".into(),
            Some(id) => app.mover = Some(super::sections::MovePicker::new(&app.ws, id)),
            None => {}
        },
        Action::NotificationLevel => app.cycle_conversation_level(),
        Action::LeaveConversation | Action::JoinConversation | Action::DeleteSection => {
            use super::commands::Command;
            let cmd = match action {
                Action::LeaveConversation => Command::Leave,
                Action::JoinConversation => Command::Join(None),
                _ => Command::SectionDelete,
            };
            if let Some(id) = app.current.clone() {
                app.run_command(id, cmd, None);
            }
        }
        Action::RenameSection => app.prefill("/section rename ", Wrap::No),
        Action::NewChannel => app.prefill("/create ", Wrap::No),
        Action::NewPrivateChannel => app.prefill("/create-private ", Wrap::No),
        Action::NewDm => app.prefill("/dm @", Wrap::No),
        Action::NewSnippet => app.prefill("/snippet ", Wrap::Body),
        Action::Upload => app.prefill("/upload ", Wrap::Caption),
        Action::SetStatus => app.prefill("/status ", Wrap::No),
        Action::PauseNotifications => app.prefill("/dnd ", Wrap::No),
        Action::ClearStatus => app.run_text("/status clear"),
        Action::AwayActive => {
            let away = app.ws.presence.get(&app.ws.self_id) != Some(&false);
            app.run_command(app.current.clone().unwrap_or_default(), super::commands::Command::Away(away), None);
        }
        Action::BrowseChannels => app.switcher = Some(super::switcher::Switcher { query: "#".into(), ..Default::default() }),
        Action::Huddle => app.toggle_huddle(),
        Action::DeclineHuddle => app.decline_huddle(false),
        Action::HuddleThread => app.open_huddle_thread(),
        Action::HuddleView => app.toggle_huddle_view(),
        Action::ShareScreen => app.toggle_share(),
        Action::Camera => {
            let wanted = app.ws.settings.video.camera.clone();
            app.status = match app.call.as_mut() {
                Some(call) => call.toggle_camera(wanted.as_deref()).unwrap_or_else(|e| format!("camera: {e}")),
                None => "not in a huddle".into(),
            };
        }
        Action::Mute => {
            if let Some(call) = app.call.as_mut() {
                if let Err(e) = call.toggle_mute() {
                    app.status = format!("microphone: {e}");
                }
            }
        }
        Action::Workspace1 => app.switch_workspace(0),
        Action::Workspace2 => app.switch_workspace(1),
        Action::Workspace3 => app.switch_workspace(2),
        Action::Workspace4 => app.switch_workspace(3),
        Action::Workspace5 => app.switch_workspace(4),
        Action::Workspace6 => app.switch_workspace(5),
        Action::Workspace7 => app.switch_workspace(6),
        Action::Workspace8 => app.switch_workspace(7),
        Action::Workspace9 => app.switch_workspace(8),
        Action::PrevConversation => app.sidebar.move_by_conversation(-1),
        Action::NextConversation => app.sidebar.move_by_conversation(1),
        Action::ScrollUp => {
            app.messages.scroll_up(app.messages.area.height.saturating_sub(3) as usize);
            app.maybe_load_older();
        }
        Action::ScrollDown => app.messages.scroll_down(app.messages.area.height.saturating_sub(3) as usize),
        Action::FocusNext | Action::FocusPrev => {
            let order = app.focus_order();
            let pos = order.iter().position(|f| *f == app.focus).unwrap_or(0);
            let step = if action == Action::FocusNext { 1 } else { order.len() - 1 };
            app.focus = order[(pos + step) % order.len()];
        }
        _ => {}
    }
    true
}

fn handle_rtm(app: &mut App, ev: slack_core::rtm::RtmEvent) {
    use slack_core::rtm::RtmEvent;
    match ev {
        RtmEvent::Connected => {
            app.connected = true;
            app.presence_subscribed = false;
            app.subscribe_presence();
        }
        RtmEvent::Disconnected(reason) => {
            app.connected = false;
            tracing::warn!("rtm disconnected: {reason}");
        }
        RtmEvent::Event(v) => {
            let fx = super::live::apply(&mut app.ws, &v, app.current.as_deref());
            if fx.rebuild_sidebar {
                app.sidebar.rebuild(&app.ws);
            }
            if let Some((channel, user)) = fx.huddle_started {
                let in_it = app.call.as_ref().is_some_and(|c| c.channel == channel);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
                if let Some((title, body)) = super::notify::huddle_notification(&app.ws, &channel, &user, in_it, now, &app.keys.first_key(Context::Global, Action::Huddle).unwrap_or_default()) {
                    if app.ws.settings.notifications.desktop {
                        let target = super::notify::Target { channel: channel.clone(), ts: None, thread_ts: None };
                        super::notify::show(title, body, &app.ws.settings, Some(app.on_click(target)));
                    }
                }
            }
            if let Some((channel, room, user)) = fx.huddle_invite {
                // The same rules as the huddle notification: DMs, not muted, no DND, not ours.
                let in_it = app.call.as_ref().is_some_and(|c| c.channel == channel);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
                if super::notify::huddle_notification(&app.ws, &channel, &user, in_it, now, "").is_some() && app.call.is_none() {
                    tracing::debug!(channel, room, user, "huddle invitation rings");
                    app.ringing = Some(Ring { channel, room, user, started: std::time::Instant::now(), last_ring: None });
                }
            }
            if let Some(channel) = fx.invite_cancelled {
                if app.ringing.as_ref().is_some_and(|r| r.channel == channel) {
                    app.ringing = None;
                }
            }
            if let Some((channel, m)) = fx.incoming {
                let in_view = if m.is_reply() {
                    app.thread.as_ref().is_some_and(|(c, t)| *c == channel && m.thread_ts.as_ref() == Some(t))
                } else {
                    app.current.as_deref() == Some(channel.as_str())
                };
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
                let note = super::notify::notification(&app.ws, &channel, &m, app.terminal_focused && in_view, now);
                tracing::debug!(channel, ts = m.ts, in_view, focused = app.terminal_focused, dnd = ?app.ws.dnd, notify = note.is_some(), "incoming message");
                if let Some((title, body)) = note {
                    if app.ws.settings.notifications.desktop {
                        let target = super::notify::Target::message(&channel, &m);
                        super::notify::show(title, body, &app.ws.settings, Some(app.on_click(target)));
                    }
                }
            }
        }
    }
}

fn handle_net(app: &mut App, msg: Net) {
    match msg {
        Net::NotificationClicked(target) => app.open_target(target),
        Net::Boot { boot, users, counts } => {
            for u in users {
                app.ws.users.insert(u.id.clone(), u);
            }
            app.ws.apply_boot(&boot);
            if let Some(c) = counts {
                app.ws.apply_counts(&c);
            }
            app.sidebar.rebuild(&app.ws);
            app.loading = false;
            app.status = match app.key_warnings.as_slice() {
                [] => String::new(),
                [one] => one.clone(),
                [first, rest @ ..] => format!("{first} (and {} more key problems, see the log)", rest.len()),
            };
            app.subscribe_presence();
            let after = app.open_after.lock().unwrap_or_else(|e| e.into_inner()).take_if(|(i, _)| *i == app.workspace);
            if let Some((_, target)) = after {
                app.open_target(target);
            } else if let Some(id) = app.sidebar.remembered.clone().filter(|id| app.ws.conversations.get(id).is_some_and(|c| !c.is_archived)) {
                // Back where you were last time.
                app.jump_to(&id);
            }
            for user in app.ws.unknown_dm_users() {
                let (client, tx) = (app.client.clone(), app.net.clone());
                tokio::spawn(async move {
                    match client.user_info(&user).await {
                        Ok(u) => {
                            let _ = tx.send(Net::UserInfo(u));
                        }
                        Err(e) => tracing::warn!("users.info {user}: {e}"),
                    }
                });
            }
            let (client, tx) = (app.client.clone(), app.net.clone());
            tokio::spawn(async move {
                match client.channel_sections().await {
                    Ok(v) => {
                        let _ = tx.send(Net::Sections(v));
                    }
                    Err(e) => tracing::warn!("channel sections failed: {e}"),
                }
                match client.conversations_list("public_channel").await {
                    Ok(list) => {
                        let _ = tx.send(Net::Channels(list));
                    }
                    Err(e) => tracing::warn!("channel list failed: {e}"),
                }
                match client.emoji_list().await {
                    Ok(map) => {
                        let _ = tx.send(Net::Emoji(map));
                    }
                    Err(e) => tracing::warn!("custom emoji list failed: {e}"),
                }
                match client.commands().await {
                    Ok(list) => {
                        let _ = tx.send(Net::Commands(list));
                    }
                    Err(e) => tracing::warn!("slash command list failed: {e}"),
                }
            });
        }
        Net::Channels(list) => app.ws.add_channels(list),
        Net::Commands(list) => app.ws.slack_commands = list,
        Net::Emoji(map) => {
            app.ws.custom_emoji = map;
            app.ws.number_emoji();
        }
        Net::UserInfo(u) => {
            app.ws.users.insert(u.id.clone(), u);
            app.sidebar.rebuild(&app.ws);
        }
        Net::Sections(v) => {
            app.ws.apply_sections(&v);
            app.sidebar.rebuild(&app.ws);
        }
        Net::Joined { mut channel, created } => {
            channel.is_member = true;
            let id = channel.id.clone();
            app.status = format!("{} #{}", if created { "created" } else { "joined" }, channel.name);
            app.ws.conversations.insert(id.clone(), channel);
            if app.ws.preview.as_deref() == Some(id.as_str()) {
                app.ws.preview = None;
            }
            app.sidebar.rebuild(&app.ws);
            app.jump_to(&id);
        }
        Net::StarFailed { channel, starred, error } => {
            if starred {
                app.ws.starred.insert(channel);
            } else {
                app.ws.starred.remove(&channel);
            }
            app.sidebar.rebuild(&app.ws);
            app.status = format!("starring failed: {error}");
        }
        Net::Left(id) => {
            if let Some(c) = app.ws.conversations.get_mut(&id) {
                c.is_member = false;
                app.status = format!("left #{}", c.name);
            }
            app.sidebar.rebuild(&app.ws);
        }
        Net::History { channel, page, older } => {
            let tl = app.ws.timelines.entry(channel).or_default();
            if older {
                tl.prepend_page(page.messages, page.has_more);
            } else {
                // Newest page: merge with anything that arrived live meanwhile.
                let live = std::mem::take(&mut tl.messages);
                tl.prepend_page(page.messages, page.has_more);
                for m in live {
                    tl.upsert(m);
                }
            }
            app.continue_seek();
        }
        Net::Posted { channel, message } => {
            super::live::add_message(&mut app.ws, &channel, &message);
            if app.ws.note_activity(&channel, message.time()) && app.ws.conversations.get(&channel).is_some_and(|c| c.is_dm()) {
                app.sidebar.rebuild(&app.ws);
            }
        }
        Net::Replies { channel, ts, page } => {
            let tl = app.ws.threads.entry(super::state::thread_key(&channel, &ts)).or_default();
            for m in page.messages {
                tl.upsert(m);
            }
            tl.loaded = true;
            tl.loading = false;
        }
        Net::FileDone(message) => app.status = message,
        Net::SendFailed { thread, key, draft, error } => {
            app.status = error;
            let here = if thread { app.thread.as_ref().map(|(c, ts)| super::state::thread_key(c, ts)) } else { app.current.clone() };
            if here.as_deref() == Some(key.as_str()) {
                let composer = app.composer_for(thread);
                if composer.draft().is_none() {
                    composer.restore(*draft);
                } else {
                    // Something was typed meanwhile: keep it, and add the files back.
                    composer.attachments.extend(draft.attachments());
                }
            } else {
                app.drafts.entry(key).or_insert(*draft);
            }
        }
        Net::Pasted { thread, result } => match result {
            Ok(a) => {
                app.status = format!("attached {} ({})", a.name, super::text::human_size(a.size));
                app.composer_for(thread).attachments.push(a);
            }
            Err(e) => app.status = e,
        },
        Net::DetailsInfo(c) => {
            if let Some(d) = app.details.as_mut().filter(|d| d.channel == c.id) {
                d.info = Some(c);
            }
        }
        Net::DetailsMembers { channel, members } => {
            if let Some(d) = app.details.as_mut().filter(|d| d.channel == channel) {
                d.members = Some(members);
            }
        }
        Net::DetailsPins { channel, pins } => {
            if let Some(d) = app.details.as_mut().filter(|d| d.channel == channel) {
                d.pins = Some(pins);
            }
        }
        Net::SearchDone { query, page, result } => {
            if let Some(search) = app.search.as_mut() {
                if page <= 1 {
                    search.busy = false;
                }
                match result {
                    Ok(v) => search.add_page(query, page, &v),
                    Err(e) => {
                        search.loading_more = false;
                        app.status = format!("search failed: {e}");
                    }
                }
            }
        }
        // A share that comes back after the call ended is dropped, which ends it.
        Net::ScreenShared(result) => {
            if let Some(call) = app.call.as_mut() {
                call.share_pending = false;
                match result {
                    Ok(share) => {
                        app.status = format!("sharing your {}", share.what);
                        call.set_share(share);
                    }
                    Err(e) => app.status = format!("screen share: {e}"),
                }
            }
        }
        Net::HuddleJoined { channel, thread, result } => match result {
            Ok(creds) if app.call.is_none() => {
                let audio = &app.ws.settings.audio;
                let devices = (
                    super::settings::Settings::device(huddle::devices::INPUT_ENV, &audio.input),
                    super::settings::Settings::device(huddle::devices::OUTPUT_ENV, &audio.output),
                );
                let mut call = super::call::Call::start(channel, creds, devices, audio);
                call.thread = thread;
                app.call = Some(call);
                app.status.clear();
            }
            Ok(_) => {}
            Err(e) => app.status = format!("cannot join the huddle: {e}"),
        },
        Net::DmOpened(c) => {
            let id = c.id.clone();
            if app.huddle_after_open.take().is_some_and(|u| c.user.as_deref() == Some(u.as_str())) && app.call.is_none() {
                app.join_huddle(id.clone(), None);
            }
            app.ws.conversations.entry(id.clone()).or_insert(c);
            app.sidebar.rebuild(&app.ws);
            app.status.clear();
            app.jump_to(&id);
        }
        Net::VideoReady { title, result } => {
            let font = app.media.picker.font_size();
            let max_px = (app.screen.width.saturating_sub(2) as u32 * font.width as u32, app.screen.height.saturating_sub(3) as u32 * font.height as u32);
            match result.map_err(anyhow::Error::msg).and_then(|path| super::video::Player::start(title, path, max_px)) {
                Ok(player) => {
                    app.player = Some(player);
                    app.status.clear();
                }
                Err(e) => app.status = format!("cannot play video: {e}"),
            }
        }
        Net::Prefs(v) => app.ws.apply_prefs(&v),
        Net::Dnd(v) => {
            app.ws.dnd = super::state::Dnd::from_json(&v);
            // An answer that is already out of date is not asked again.
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
            app.dnd_refreshing = app.ws.dnd.stale(now);
        }
        Net::ActivityFeed(result) => match result {
            Ok(v) => {
                if let Some(a) = app.activity.as_mut() {
                    let mut items = super::activity::parse(&v);
                    super::activity::apply_seen(&mut items, &app.seen);
                    a.items = Some(items);
                }
            }
            Err(e) => {
                app.activity = None;
                app.status = format!("activity failed: {e}");
            }
        },
        Net::PreviewPage { channel, page } => {
            if let Some(tl) = app.ws.timelines.get_mut(&channel) {
                tl.refresh_newest(page.messages);
            }
        }
        Net::UploadProgress { name, sent, total } => {
            app.status = upload_status(&name, sent, total);
        }
        Net::ThreadView { result, more } => match result {
            Ok(v) => {
                if let Some(t) = app.threads.as_mut() {
                    let (mut page, has_more) = super::threads::parse(&v);
                    super::threads::apply_seen(&mut page, &app.seen);
                    if !more {
                        t.entries = None;
                    }
                    t.add_page(page, has_more);
                }
            }
            Err(e) => {
                app.threads = None;
                app.status = format!("threads failed: {e}");
            }
        },
        Net::ActivityMessage { channel, ts, message } => {
            if let Some(a) = app.activity.as_mut() {
                a.messages.insert((channel, ts), message);
            }
        }
        Net::SnippetText { id, result } => {
            if let Some(v) = app.snippet.as_mut().filter(|v| v.file.id == id) {
                v.set_text(result);
            }
        }
        Net::FullImage { id, result } => {
            if let Some(v) = app.viewer.as_mut() {
                v.set_image(&id, result);
            }
        }
        Net::Thumb { id, result } => {
            let thumb = match result {
                Ok(proto) => super::media::Thumb::Ready(proto),
                Err(e) => {
                    tracing::warn!("thumbnail {id} failed: {e}");
                    super::media::Thumb::Failed(super::media::short_reason(&e).to_string())
                }
            };
            app.media.thumbs.insert(id, thumb);
        }
        Net::Permalink(url) => {
            let how = app.clipboard.copy(&url);
            app.status = format!("copied link to the {how}");
        }
        Net::Restore { channel, message, error } => {
            let mut copies = app.ws.copies_mut(&channel, &message.ts);
            if copies.is_empty() {
                drop(copies);
                super::live::add_message(&mut app.ws, &channel, &message);
            } else {
                for c in copies.iter_mut() {
                    **c = message.clone();
                }
            }
            app.status = error;
        }
        Net::Error(e) => {
            app.loading = false;
            app.status = e;
        }
    }
}

/// How long to wait before joining again a huddle that ended before it connected.
const REJOIN_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

/// A call's session ended: None when that is an ordinary end (it had connected, or we left),
/// else whether to join again (only once; Some(false): give up).
fn rejoin_after_end(connected: bool, leaving: bool, rejoined: bool) -> Option<bool> {
    (!connected && !leaving).then_some(!rejoined)
}

/// The width the messages and a thread need side by side (about 50 columns each).
const THREAD_BESIDE_MIN: u16 = 100;

/// The messages' and an open thread's parts of the main area: side by side, or the thread
/// alone when there is no room for both.
fn split_main(main: Rect, thread: bool) -> (Option<Rect>, Option<Rect>) {
    if !thread {
        (Some(main), None)
    } else if main.width < THREAD_BESIDE_MIN {
        (None, Some(main))
    } else {
        let [left, right] = Layout::horizontal([Constraint::Min(30), Constraint::Percentage(40)]).areas(main);
        (Some(left), Some(right))
    }
}

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    app.screen = f.area();
    // A sequence started before a dialog opened (or closed) is over.
    let mode = app.mode();
    app.pending.sync(mode);
    app.sync_which_key(std::time::Instant::now());
    let messages = &app.ws.settings.messages;
    app.ws.emoji_pictures = app.media.graphics() && messages.custom_emoji_pictures && !messages.emoji_as_names;
    // The huddle view goes with the call, and gives way once the messages or the composer
    // get the focus (opening a conversation, replying, …).
    if app.call.is_none() || matches!(app.focus, Focus::Messages | Focus::Composer) {
        app.huddle_view = None;
    }
    if app.huddle_view.is_none() && app.focus == Focus::Huddle {
        app.focus = Focus::Composer;
    }
    let key = |a| app.keys.first_key(Context::Global, a).unwrap_or_default();
    let call_keys = (key(Action::Mute), key(Action::Huddle), key(Action::HuddleThread), key(Action::HuddleView), key(Action::Camera), key(Action::ShareScreen));
    let bar_keys = super::huddle_view::Keys { mute: &call_keys.0, leave: &call_keys.1, thread: &call_keys.2, view: &call_keys.3, camera: &call_keys.4, share: &call_keys.5 };
    let call_rows = app.call.as_ref().map_or(0, |call| call.bar_rows(&app.ws, &app.media, bar_keys, f.area().width));
    let [body, call_bar, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(call_rows), Constraint::Length(1)]).areas(f.area());
    let above_status = body;
    app.call_bar_area = call_bar;
    if let Some(call) = &app.call {
        if let Some(w) = call.draw(f, call_bar, &app.ws, &app.media, bar_keys) {
            app.call_bar_wanted = Some(w);
        }
    }
    // The workspace rail, with two or more workspaces and room for it.
    let body = if app.workspaces.len() > 1 && body.width >= 60 {
        let [rail, rest] = Layout::horizontal([Constraint::Length(super::rail::WIDTH), Constraint::Min(1)]).areas(body);
        let badges = app.others.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let logos = app.logos.lock().unwrap_or_else(|e| e.into_inner()).clone();
        (app.rail, app.rail_wanted) = super::rail::draw(f, rail, &app.workspaces, app.workspace, &badges, &logos, &app.media);
        rest
    } else {
        app.rail.clear();
        body
    };
    let side_w = (body.width / 4).clamp(18, 32);
    let [side, main] = Layout::horizontal([Constraint::Length(side_w), Constraint::Min(10)]).areas(body);
    // A thread's draft counts for its conversation.
    app.sidebar.drafts = app.drafts.keys().map(|k| k.split('/').next().unwrap_or(k).to_string()).collect();
    app.sidebar.draw(f, side, &app.ws, app.focus == Focus::Sidebar);
    // Too narrow for a thread beside the messages (as Slack does): the thread takes their place
    // until it is closed, or until the messages or their composer get the focus; the huddle
    // view and a thread take turns there.
    app.narrow = main.width < THREAD_BESIDE_MIN;
    if app.narrow && app.thread.is_some() {
        if matches!(app.focus, Focus::Messages | Focus::Composer) {
            app.close_thread();
        } else if app.huddle_view.take().is_some() && app.focus == Focus::Huddle {
            app.focus = Focus::Thread;
        }
    }
    let (messages_area, thread_area) = split_main(main, app.thread.is_some());
    let alone = messages_area.is_none();
    // (`main` stays the whole area when the thread takes it, for the dialogs drawn over it.)
    let main = messages_area.unwrap_or(main);
    if let (Some(area), Some((channel, ts))) = (thread_area, app.thread.clone()) {
        let h = app.thread_composer.height(area.width.saturating_sub(2) as usize);
        let [pane, comp] = Layout::vertical([Constraint::Min(3), Constraint::Length(h)]).areas(area);
        let title = format!("Thread · {} · {} close", app.ws.channel_label(&channel), app.keys.first_key(Context::Messages, Action::Back).unwrap_or_default());
        let timeline = app.ws.threads.get(&super::state::thread_key(&channel, &ts));
        app.thread_pane.editing = app.thread_composer.editing.clone();
        app.thread_pane.draw(f, pane, &app.ws, &title, timeline, app.focus == Focus::Thread, &app.media);
        app.thread_composer_area = comp;
        draw_composer(f, comp, app, true);
    }

    if alone {
        // Nothing of the message pane is on screen, so the mouse must not find it.
        app.messages.area = Default::default();
        app.messages.hover = None;
        app.composer_area = Default::default();
    } else if let (Some(view), Some(call)) = (app.huddle_view.as_mut(), &app.call) {
        // Nothing of the message pane is on screen, so the mouse must not find it.
        app.messages.area = Default::default();
        app.messages.hover = None;
        app.composer_area = Default::default();
        view.draw(f, main, call, &app.ws, &app.media, bar_keys);
    } else {
        let inner_w = main.width.saturating_sub(2) as usize;
        let composer_h = app.composer.height(inner_w);
        let [msgs, comp] = Layout::vertical([Constraint::Min(3), Constraint::Length(composer_h)]).areas(main);
        let current = app.current.clone();
        let timeline = current.as_deref().and_then(|c| app.ws.timelines.get(c));
        let title = current.as_deref().map(|c| app.ws.channel_label(c)).unwrap_or_default();
        app.messages.editing = app.composer.editing.clone();
        app.messages.draw(f, msgs, &app.ws, &title, timeline, app.focus == Focus::Messages, &app.media);
        app.composer_area = comp;
        draw_composer(f, comp, app, false);
    }
    if let Some(c) = &app.completion {
        super::autocomplete::draw(f, if c.thread { app.thread_composer_area } else { app.composer_area }, c);
    }
    if let Some((picker, _, _)) = app.picker.as_mut() {
        picker.draw(f, main, &app.ws.custom_emoji);
    }
    let question = match &app.confirm {
        Some(Confirm::Delete { .. }) => Some("Delete this message?".to_string()),
        Some(Confirm::Leave(channel)) => Some(format!("Leave #{}?", app.ws.channel_label(channel))),
        Some(Confirm::DeleteSection { name, .. }) => Some(format!("Delete the section {name}?")),
        None => None,
    };
    if let Some(q) = question {
        app.confirm_area = draw_confirm(f, main, &confirm_question(&q, &app.keys));
    }
    if let Some(d) = app.save.as_mut() {
        d.draw(f, main);
    }
    if let Some(v) = app.viewer.as_mut() {
        v.draw(f, body, &app.media.picker);
    }
    if let Some(p) = app.player.as_mut() {
        p.draw(f, body, &app.media.picker);
    }
    if let Some(s) = app.snippet.as_mut() {
        s.draw(f, body);
    }
    if let Some(sw) = app.switcher.as_mut() {
        sw.draw(f, body, &app.ws);
    }
    if let Some(search) = app.search.as_mut() {
        search.draw(f, body, &app.ws);
    }
    if let Some(d) = app.details.as_mut() {
        d.draw(f, body, &app.ws);
    }
    if let Some(t) = app.threads.as_mut() {
        t.draw(f, body, &app.ws);
    }
    if let Some(a) = app.activity.as_mut() {
        a.draw(f, body, &app.ws);
    }
    if let Some(card) = app.profile.as_mut() {
        card.draw(f, body, &app.ws);
    }
    if let Some(m) = app.mover.as_mut() {
        m.draw(f, body, &app.ws);
    }
    if let Some(p) = app.prefs.as_mut() {
        p.draw(f, body, &app.ws, app.current.as_deref(), &app.keys);
    }
    if let Some(m) = app.menu.as_mut() {
        m.draw(f, body);
    }
    if let Some(h) = app.help.as_mut() {
        h.draw(f, body, &app.keys);
    }
    // which-key: the keys that can come next.
    if app.which_key.is_some() {
        let contexts = app.active_contexts();
        let mut children = app.keys.children(&contexts, mode, app.pending.keys());
        // A leader as a next key shows as the leader's key.
        for c in children.iter_mut().filter(|c| c.key == LEADER) {
            c.key = app.keys.leader_key().unwrap_or(LEADER);
        }
        let settings = &app.ws.settings.which_key;
        let pages = super::which_key::layout(&children, above_status, settings.max_height_percent, settings.icons).pages;
        let title = super::which_key::title(&app.keys, &contexts, &app.pending);
        if let Some(panel) = app.which_key.as_mut() {
            panel.page = panel.page.min(pages.saturating_sub(1));
            super::which_key::draw(f, above_status, &title, &children, panel, settings);
        }
    }
    // The status line: the mode, what is going on, the keys under way and the hints.
    // The dialog's or the focused pane's context (not the suggestion list's).
    let top = app.active_contexts().into_iter().find(|c| *c != Context::Completion).unwrap_or(Context::Global);
    // The theme picker borrows the switcher's keys but is not the switcher.
    let picking = app.prefs.as_ref().is_some_and(|p| p.picker.is_some());
    let pill = if picking { super::status::Pill::Dialog("THEME") } else { mode_label(top, app.focus) };
    let hints = if picking {
        let key = |a| app.keys.first_key(Context::Switcher, a).unwrap_or_default();
        vec![(key(Action::Open), super::i18n::tr("keep").into_owned()), (key(Action::Close), super::i18n::tr("back").into_owned())]
    } else {
        app.keys.context_hints(top)
    };
    let (unread, mentions) = app.ws.total_unread();
    let note = if let Some(ring) = &app.ringing {
        let key = |a| app.keys.sequences(Context::Global, a).first().map(|s| app.keys.show(s)).unwrap_or_default();
        Some(format!("📞 {} invites you to a huddle · {} join · {} decline", app.ws.user_name(&ring.user), key("huddle"), key("decline_huddle")))
    } else if app.status.is_empty() {
        None
    } else {
        Some(app.status.clone())
    };
    let info = super::status::Info {
        unread,
        mentions,
        others: super::others::label(&app.others.lock().unwrap_or_else(|e| e.into_inner()), app.workspace),
        availability: super::profile::availability(&app.ws, chrono::Utc::now().timestamp()),
        live: app.connected,
        note,
    };
    let showcmd = app.keys.show(app.pending.typed());
    f.render_widget(Paragraph::new(super::status::line(pill, &info, &showcmd, &hints, status.width)), status);
}

/// The mode for the topmost dialog (`overlay`) and the focused pane: a dialog owns all input;
/// in a message box keys type.
fn mode_of(overlay: Option<Context>, focus: Focus) -> Mode {
    match (overlay, focus) {
        (Some(_), _) => Mode::Dialog,
        (None, Focus::Composer | Focus::ThreadComposer) => Mode::Insert,
        (None, _) => Mode::Normal,
    }
}

/// A yes / no question with the key that confirms ("Delete this message? y / n").
fn confirm_question(q: &str, keys: &Keymap) -> String {
    match keys.first_key(Context::Confirm, Action::Yes) {
        Some(yes) => format!("{q} {yes} / n"),
        None => q.to_string(),
    }
}

/// The status line's pill — where keys go: the open dialog, or the focused pane.
fn mode_label(top: Context, focus: Focus) -> super::status::Pill {
    use super::status::Pill::{Dialog, Insert, Normal};
    match top {
        Context::Menu => Dialog("MENU"),
        Context::Save => Dialog("SAVE"),
        Context::Help => Dialog("KEYS"),
        Context::Preferences => Dialog("PREFS"),
        Context::TextField => Dialog("EDIT"),
        Context::Switcher => Dialog("JUMP"),
        Context::Mover => Dialog("SECTION"),
        Context::Activity => Dialog("ACTIVITY"),
        Context::Threads => Dialog("THREADS"),
        Context::Profile => Dialog("PROFILE"),
        Context::Details => Dialog("DETAILS"),
        Context::Search => Dialog("SEARCH"),
        Context::Player => Dialog("VIDEO"),
        Context::Snippet => Dialog("SNIPPET"),
        Context::Viewer => Dialog("IMAGE"),
        Context::Confirm => Dialog("CONFIRM"),
        Context::Picker => Dialog("REACT"),
        _ => match focus {
            Focus::Sidebar | Focus::Huddle | Focus::Messages | Focus::Thread => Normal,
            Focus::ThreadComposer | Focus::Composer => Insert,
        },
    }
}

/// The yes / no prompt, centred in `area`; returns where it was drawn.
fn draw_confirm(f: &mut ratatui::Frame, area: ratatui::layout::Rect, question: &str) -> Rect {
    use ratatui::layout::Flex;
    use ratatui::style::Style;
    use ratatui::widgets::{Block, Borders};
    let [popup] = Layout::horizontal([Constraint::Length(question.chars().count() as u16 + 4)]).flex(Flex::Center).areas(area);
    let [popup] = Layout::vertical([Constraint::Length(3)]).flex(Flex::Center).areas(popup);
    super::media::clear_modal(f, popup);
    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(super::theme::p().danger));
    f.render_widget(Paragraph::new(format!(" {question}")).block(block), popup);
    popup
}

/// The channel composer, or the thread pane's (`thread`).
fn draw_composer(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App, thread: bool) {
    use ratatui::style::Style;
    use ratatui::widgets::{Block, Borders};
    let composer = if thread { &app.thread_composer } else { &app.composer };
    let focused = app.focus == (if thread { Focus::ThreadComposer } else { Focus::Composer }) && app.mode() == Mode::Insert;
    let border = if focused { Style::default().fg(super::theme::p().good) } else { Style::default().fg(super::theme::p().dim) };
    let target = app.current.as_deref().map(|c| app.ws.channel_label(c)).unwrap_or_default();
    let typing = if thread {
        app.thread.as_ref().map(|(c, ts)| app.ws.typing_in(&super::state::thread_key(c, ts))).unwrap_or_default()
    } else {
        app.current.as_deref().map(|c| app.ws.typing_in(c)).unwrap_or_default()
    };
    // Room for the title between the corners.
    let room = area.width.saturating_sub(2) as usize;
    let title = if composer.editing.is_some() {
        let key = |a| app.keys.first_key(Context::Composer, a).unwrap_or_default();
        let (save, cancel) = (key(Action::Send), key(Action::Leave));
        fit_title(&[format!(" Editing message · {save} save · {cancel} cancel "), format!(" Editing · {save} save · {cancel} cancel "), " Editing message ".into()], room)
    } else if thread {
        let key = app.keys.first_key(Context::Composer, Action::AlsoToChannel).unwrap_or_default();
        reply_title(app.broadcast, &key, room)
    } else if app.current.as_deref().and_then(|c| app.ws.conversations.get(c)).is_some_and(|c| c.is_archived) {
        format!(" #{target} is archived ")
    } else if app.current.as_deref().is_some_and(|c| app.ws.is_outside(c)) {
        format!(" Previewing #{target} · /join to join ")
    } else {
        format!(" Message {target} ")
    };
    let mut block = Block::default().borders(Borders::ALL).border_style(border).title(title);
    if !typing.is_empty() {
        let who = if typing.len() == 1 { format!("{} is typing…", typing[0]) } else { format!("{} people are typing…", typing.len()) };
        block = block.title(ratatui::text::Line::styled(format!(" {who} "), Style::default().fg(super::theme::p().dim)).right_aligned());
    }
    let inner = block.inner(area);
    let width = inner.width as usize;
    composer.wrap_width.set(width);
    // Attachments on top, each with a ✕ that removes it, then the text.
    let (attachments, closable) = composer.attachment_lines();
    let below = (attachments.len() as u16).min(inner.height);
    let accent = Style::default().fg(super::theme::p().accent);
    for (i, line) in attachments.iter().enumerate().take(below as usize) {
        let row = Rect { y: inner.y + i as u16, height: 1, width: inner.width.saturating_sub(3), ..inner };
        f.render_widget(Paragraph::new(line.as_str()).style(accent), row);
        if i < closable {
            let (x, y) = attachment_close(area, i);
            f.render_widget(Paragraph::new("✕").style(Style::default().fg(super::theme::p().dim)), Rect::new(x, y, 1, 1));
        }
    }
    let inner = Rect { y: inner.y + below, height: inner.height - below, ..inner };
    let (row, col) = composer.cursor_position(width);
    // Scroll so the cursor line stays visible.
    let skip = row.saturating_sub(inner.height.saturating_sub(1));
    let text = if composer.text().is_empty() && !focused {
        let hint = if thread {
            format!("{} to reply", app.keys.first_key(Context::Messages, Action::Type).unwrap_or_default())
        } else {
            format!("Press {} or click a conversation to type", app.keys.first_key(Context::Global, Action::FocusNext).unwrap_or_default())
        };
        Paragraph::new(ratatui::text::Text::styled(hint, Style::default().fg(super::theme::p().dim))).wrap(ratatui::widgets::Wrap { trim: false })
    } else {
        // Wrapped by the composer itself, so the cursor lands where the text is.
        Paragraph::new(composer.wrapped(width).into_iter().map(ratatui::text::Line::raw).collect::<Vec<_>>())
    };
    f.render_widget(block, area);
    f.render_widget(text.scroll((skip, 0)), inner);
    if focused {
        // A faint tint shows where typing goes (from slk).
        f.buffer_mut().set_style(block_inner(area), Style::default().bg(super::theme::p().hover));
        if !app.covered() {
            f.set_cursor_position((inner.x + col, inner.y + row - skip));
        }
    }
}

/// Inside a composer's border.
fn block_inner(area: Rect) -> Rect {
    ratatui::widgets::Block::default().borders(ratatui::widgets::Borders::ALL).inner(area)
}

/// Where the ✕ that removes attachment `i` sits in a composer drawn at `area`.
fn attachment_close(area: Rect, i: usize) -> (u16, u16) {
    let inner = block_inner(area);
    (inner.right().saturating_sub(2), inner.y + i as u16)
}

/// The attachment whose ✕ is at (`column`, `row`) — a cell either side counts — of the
/// `closable` ones in a composer drawn at `area`.
fn attachment_at(area: Rect, closable: usize, column: u16, row: u16) -> Option<usize> {
    (0..closable).find(|&i| {
        let (x, y) = attachment_close(area, i);
        row == y && column + 1 >= x && column <= x + 1 && y < block_inner(area).bottom()
    })
}

#[cfg(test)]
mod tests {
    use slack_core::session::WorkspaceSession;

    #[test]
    fn attachments_close_at_their_cross() {
        use ratatui::layout::Rect;
        let area = Rect::new(10, 20, 40, 6);
        assert_eq!(super::attachment_close(area, 0), (47, 21));
        assert_eq!(super::attachment_close(area, 1), (47, 22));
        assert_eq!(super::attachment_at(area, 2, 47, 22), Some(1));
        assert_eq!(super::attachment_at(area, 2, 46, 21), Some(0), "a cell to the left");
        assert_eq!(super::attachment_at(area, 2, 30, 21), None, "the name does nothing");
        assert_eq!(super::attachment_at(area, 1, 47, 22), None, "\"… and N more\" has no cross");
    }

    #[test]
    fn pasting_files_says_what_happened() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(super::attached_status(&s(&["a.pdf"]), &[]), "attached a.pdf");
        assert_eq!(super::attached_status(&s(&["a.pdf", "b.png"]), &s(&["pics (a folder)"])), "attached 2 files · not attached: pics (a folder)");
    }

    #[test]
    fn a_thread_takes_the_messages_place_when_narrow() {
        use ratatui::layout::Rect;
        let main = |w| Rect::new(20, 0, w, 30);
        assert_eq!(super::split_main(main(80), false), (Some(main(80)), None), "no thread");
        assert_eq!(super::split_main(main(99), true), (None, Some(main(99))), "too narrow: the thread alone");
        let (messages, thread) = super::split_main(main(100), true);
        let (messages, thread) = (messages.unwrap(), thread.unwrap());
        assert_eq!((messages.x, messages.width, thread.x, thread.width), (20, 60, 80, 40), "side by side");
    }

    #[test]
    fn upload_progress_reads_well() {
        assert_eq!(super::upload_status("a.mp4", 3_100_000, 7_400_000), format!("uploading a.mp4… 41% ({} of {})", super::super::text::human_size(3_100_000), super::super::text::human_size(7_400_000)));
        assert_eq!(super::upload_status("a.mp4", 7_400_000, 7_400_000), "uploading a.mp4… sent, Slack is filing it");
        assert_eq!(super::upload_status("empty", 0, 0), "uploading empty… sent, Slack is filing it");
    }

    #[test]
    fn threads_are_read_up_to_their_newest_reply() {
        use super::super::state::Timeline;
        use super::thread_mark_target;
        let msg = |ts: &str| slack_core::types::Message { ts: ts.into(), ..Default::default() };
        let mut t = Timeline::default();
        t.upsert(msg("1.0"));
        t.upsert(msg("2.0"));
        assert_eq!(thread_mark_target(&t, "1.0"), None, "replies not loaded yet");
        t.loaded = true;
        assert_eq!(thread_mark_target(&t, "1.0").as_deref(), Some("2.0"));
        let mut lone = Timeline { loaded: true, ..Default::default() };
        lone.upsert(msg("1.0"));
        assert_eq!(thread_mark_target(&lone, "1.0"), None, "no replies: nothing to mark");
    }

    #[test]
    fn the_pill_follows_the_mode() {
        use crate::tui::status::Pill;
        assert_eq!(super::mode_label(crate::tui::keymap::Context::Confirm, super::Focus::Messages), Pill::Dialog("CONFIRM"));
        assert_eq!(super::mode_label(crate::tui::keymap::Context::Composer, super::Focus::Composer), Pill::Insert);
        assert_eq!(super::mode_label(crate::tui::keymap::Context::Messages, super::Focus::Sidebar), Pill::Normal);
    }

    #[test]
    fn drafts_save_after_a_pause() {
        use super::{DRAFT_SAVE_DELAY, draft_save_due};
        let ms = std::time::Duration::from_millis;
        assert!(!draft_save_due(false, ms(500)), "still typing");
        assert!(draft_save_due(false, DRAFT_SAVE_DELAY), "a pause");
        assert!(!draft_save_due(true, ms(60_000)), "already saved");
    }

    #[test]
    fn rejoining_a_huddle_that_ended_at_once() {
        use super::rejoin_after_end;
        assert_eq!(rejoin_after_end(false, false, false), Some(true), "ended before connecting: join again");
        assert_eq!(rejoin_after_end(false, false, true), Some(false), "again: give up");
        assert_eq!(rejoin_after_end(true, false, false), None, "an ordinary end");
        assert_eq!(rejoin_after_end(false, true, false), None, "we left before it connected");
    }

    #[test]
    fn reply_titles_fit() {
        use super::reply_title;
        assert_eq!(reply_title(false, "Alt+A", 60), " Reply · [ ] also to channel (Alt+A) ");
        assert_eq!(reply_title(true, "Alt+A", 36), " Reply · [x] also to channel ");
        assert_eq!(reply_title(false, "Alt+A", 26), " Reply · [ ] +channel ");
        assert_eq!(reply_title(false, "Alt+A", 10), " [ ] +channel ", "the checkbox stays even when nothing fits");
        for room in 14..70 {
            assert!(reply_title(true, "Alt+A", room).chars().count() <= room);
        }
    }

    #[test]
    fn seeking_older_messages() {
        use super::seek_older;
        assert!(seek_older("1792344000.000100", "1792344759.290079"));
        assert!(!seek_older("1792344759.290079", "1792344759.290079"), "the oldest loaded one is there");
        assert!(!seek_older("1792349999.000000", "1792344759.290079"), "newer: not paged for");
        assert!(!seek_older("x", "1.0"));
    }

    #[test]
    fn status_messages_expire() {
        use super::{Duration, status_expired};
        let s = Duration::from_secs;
        assert!(!status_expired("left #general", s(7)));
        assert!(status_expired("left #general", s(8)));
        assert!(!status_expired("uploading notes.txt…", s(600)), "still going on");
        assert!(!status_expired("", s(600)));
    }

    #[test]
    fn confirm_prompt_names_the_bound_key() {
        use crate::tui::keymap::Keymap;
        let rebound = super::confirm_question("delete this message?", &Keymap::load("[confirm]\nyes = \"j\"\n").0);
        assert!(rebound.ends_with("j / n"), "{rebound:?}");
        let default = super::confirm_question("delete this message?", &Keymap::load("").0);
        assert!(default.ends_with("y / n"), "{default:?}");
    }

    #[test]
    fn dialogs_take_the_mode() {
        use super::{Focus, mode_of};
        use crate::tui::keymap::{Context, Mode};
        assert_eq!(mode_of(Some(Context::Search), Focus::Composer), Mode::Dialog, "a dialog over the composer");
        assert_eq!(mode_of(None, Focus::Composer), Mode::Insert);
        assert_eq!(mode_of(None, Focus::ThreadComposer), Mode::Insert);
        assert_eq!(mode_of(None, Focus::Messages), Mode::Normal);
        assert_eq!(mode_of(None, Focus::Huddle), Mode::Normal);
    }

    #[test]
    fn prefill_keeps_drafts() {
        use super::{Wrap, prefilled};
        use crate::tui::composer::Composer;
        let fill = |draft: &str, command: &str, wrap: Wrap| {
            let mut c = Composer::default();
            c.set(draft);
            let (text, cursor) = prefilled(&c.text(), false, command, wrap)?;
            c.set_with_cursor(&text, cursor);
            Ok::<_, &str>((c.text(), c.cursor()))
        };
        assert_eq!(fill("", "/create ", Wrap::No), Ok(("/create ".into(), 8)));
        assert_eq!(fill("hello", "/upload ", Wrap::Caption), Ok(("/upload  hello".into(), 8)), "the draft becomes the upload's message");
        assert_eq!(fill("line", "/snippet ", Wrap::Body), Ok(("/snippet \nline".into(), 9)), "the draft becomes the snippet's body");
        assert_eq!(fill("hello", "/create ", Wrap::No), Err("the message box is in use"));
        assert_eq!(fill(" ", "/dm @", Wrap::No), Ok(("/dm @".into(), 5)), "blank is empty");
        assert!(prefilled("", true, "/status ", Wrap::No).is_err() && prefilled("x", true, "/upload ", Wrap::Caption).is_err(), "never during an edit");
        let mut c = Composer::default();
        c.set_with_cursor("abc", 9);
        assert_eq!(c.cursor(), 3, "kept inside the text");
    }

    #[test]
    fn views_do_not_open_over_a_dialog() {
        use crate::tui::keymap::Action;
        assert!(super::opens_something(Action::Switcher) && super::opens_something(Action::Upload) && super::opens_something(Action::Details));
        assert!(!super::opens_something(Action::NotificationLevel) && !super::opens_something(Action::Mute) && !super::opens_something(Action::AwayActive));
    }

    fn session(team_id: &str, team: &str, user: &str) -> WorkspaceSession {
        WorkspaceSession { team_id: team_id.into(), team_name: team.into(), user_name: user.into(), ..Default::default() }
    }

    #[test]
    fn workspace_labels_name_the_account_when_a_team_repeats() {
        let labels = super::workspace_labels(&[session("T1", "acme", "ann"), session("T2", "beta", "ann"), session("T1", "acme", "bob")]);
        assert_eq!(labels, vec!["acme · ann", "beta", "acme · bob"]);
    }
}
