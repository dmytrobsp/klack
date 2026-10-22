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
use super::history::Place;
use super::keymap::{Action, Context, Key, Keymap, LEADER, Mode, Pending, Resolved};
use super::keys_file::Change;
use super::messages::MessagePane;
use super::picker::EmojiPicker;
use super::sidebar::Sidebar;
use super::state::Workspace;
use super::terminal::TerminalGuard;

/// Results of background API calls.
enum Net {
    /// Muting (`true`) or unmuting a conversation failed.
    MuteFailed { channel: String, muted: bool, error: String },
    /// Marking a conversation unread failed: Slack still has it read.
    MarkUnreadFailed { channel: String, error: String },
    Boot { boot: Value, users: Vec<User>, counts: Option<Value> },
    /// A desktop notification was clicked.
    NotificationClicked(super::notify::Target),
    /// A person looked up with `users.info`.
    UserInfo(User),
    /// The people shown, looked up again for their statuses (`status_refresh_secs`).
    Profiles(Vec<User>),
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
    /// The thread left open last time, fetched at start (none: it could not be loaded).
    ThreadBack { channel: String, ts: String, page: Option<HistoryPage> },
    /// Conversation details loaded for the details overlay.
    DetailsInfo(slack_core::types::Conversation),
    /// What Slack says about a conversation a link (or notification) leads to that this workspace did not know.
    LinkConversation { result: Result<slack_core::types::Conversation, String>, target: super::applink::Target },
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
    /// `/dnd` paused or resumed notifications (or failed); `dnd.info` asked right after.
    DndSet { resumed: bool, error: Option<String>, info: Option<Value> },
    /// klack's schedule write ended (`error` if not); `prefs` read back after it (or fetched again
    /// after a failure) and `dnd.info` asked right after.
    DndSchedule { error: Option<String>, prefs: Option<Value>, info: Option<Value> },
    /// User prefs fetched again.
    Prefs(Value),
    /// A tab of the Activity view.
    ActivityFeed { tab: super::activity::Tab, more: bool, result: Result<Value, String> },
    /// The activity feed fetched at start, for the sidebar row's unread count (when Slack's counts have no badge).
    ActivityUnread(Result<Value, String>),
    /// `client.counts` asked again after a reconnect, for Slack's Activity badge (`activity_v2`).
    /// `since`: the workspace's `activity_seq` when it was asked for.
    ActivityBadge { since: u64, result: Result<Value, String> },
    /// The huddles going on now (at start, after a reconnect, when one starts, when the Huddles view opens).
    /// `since`: the workspace's `huddle_seq` when it was asked for.
    LiveHuddles { since: u64, result: Result<Vec<slack_core::types::LiveHuddle>, String> },
    /// A page of the Huddles view's recent huddles (`more`: added to what is listed) for its load `generation`.
    HuddleHistory { generation: u64, more: bool, result: Result<(Vec<slack_core::types::PastHuddle>, Option<String>), String> },
    /// How many replies a recent huddle's thread has (None: its message could not be had).
    HuddleReplies { channel: String, ts: String, count: Option<u64> },
    /// The newest page of a previewed channel, fetched again.
    PreviewPage { channel: String, page: slack_core::types::HistoryPage },
    /// Bytes of an upload sent so far.
    UploadProgress { name: String, sent: u64, total: u64 },
    /// A page of the Threads view (`more`: added to what is listed).
    ThreadView { result: Result<Value, String>, more: bool },
    FeedReplies { channel: String, ts: String, result: Result<Vec<slack_core::types::Message>, String> },
    /// A message an activity item is about.
    ActivityMessage { channel: String, ts: String, message: Option<slack_core::types::Message> },
    /// A snippet's full text for the snippet view.
    SnippetText { id: String, result: Result<String, String> },
    /// A full-size image for the viewer.
    FullImage { id: String, result: Result<image::DynamicImage, String> },
    /// A linked message's author: Slack's answer (None: no such message), or why there is none (the request failed).
    LinkAuthor { channel: String, ts: String, result: Result<Option<slack_core::types::Message>, String> },
    /// A thumbnail finished loading.
    Thumb { id: String, result: Result<ratatui_image::sliced::SlicedProtocol, String> },
    /// A permalink to copy.
    Permalink(String),
    /// An optimistic change failed: put the original message back.
    Restore { channel: String, message: slack_core::types::Message, error: String },
    /// A history request failed: the conversation can be asked again.
    HistoryFailed { channel: String, error: String },
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

/// What the main pane shows: the conversation (with its message box), or one of the views
/// opened from the sidebar's rows atop it. A view gives way to the conversation it was opened
/// over on Esc, and when a conversation is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum View {
    #[default]
    Conversation,
    Threads,
    Activity,
    Huddles,
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
    /// Where the which-key panel was at the last draw (empty: not drawn), for clicks.
    which_key_area: Rect,
    /// When and where the last click that closed a dialog was (see [`swallowed`]).
    closing_click: Option<(std::time::Instant, u16, u16)>,
    /// Problems found in keys.toml, shown once the workspace has loaded.
    key_warnings: Vec<String>,
    /// The profile's workspaces (labels) and which one this is.
    workspaces: Vec<String>,
    /// The profile's workspaces' (url, team id), which links into them are matched against.
    addresses: Vec<(String, String)>,
    /// Who wrote the messages links point to, as far as known.
    authors: super::applink::Authors,
    /// Unread state of the other workspaces (polled in the background).
    others: super::others::Badges,
    /// The workspace rail's tiles at the last draw (none without a rail).
    rail: Vec<ratatui::layout::Rect>,
    /// Workspace logos for the rail, and those it wanted at the last draw.
    logos: super::others::Logos,
    rail_wanted: Vec<(slack_core::types::File, (u16, u16))>,
    /// A notification clicked in another workspace, opened once this one has booted.
    open_after: super::others::OpenAfter,
    /// At start: the conversation to settle into once its history is in, and the thread to reopen there.
    start: Option<(String, Option<String>)>,
    workspace: usize,
    /// Leave this workspace for another (index into `workspaces`).
    switch_to: Option<usize>,
    /// The workspaces whose sessions have ended (skipped: not switched to, not polled).
    signed_out: SignedOut,
    /// Once this workspace's session has ended while running: what the status line keeps saying.
    signed_out_note: Option<String>,
    /// The huddle we are in.
    call: Option<super::call::Call>,
    /// Presence subscription sent on the current RTM connection.
    presence_subscribed: bool,
    /// People whose `user_change` events were asked for on the current RTM connection, when the
    /// list was last checked for new ones, and what it went by then (`Workspace::people_key`).
    followed: (std::collections::HashSet<String>, Option<std::time::Instant>, Option<u64>),
    /// How many times the people to follow were gone through.
    follow_passes: u64,
    /// Open conversation details (␣cd).
    details: Option<super::details::Details>,
    /// Open profile card.
    profile: Option<super::profile::ProfileCard>,
    /// A `dnd.info` refresh is on its way.
    dnd_refreshing: bool,
    /// klack's own schedule writes, one at a time.
    schedule_writes: ScheduleQueue,
    /// Open context menu (right-click).
    menu: Option<super::menu::Menu>,
    /// Open preferences (F6).
    prefs: Option<super::prefs::Prefs>,
    /// The huddle view (␣hv), shown in place of the messages while Some.
    huddle_view: Option<super::huddle_view::HuddleView>,
    /// The huddle view while a thread stands in its place (narrow): it comes back when the thread closes.
    parked_huddle: Option<super::huddle_view::HuddleView>,
    /// Where the call bar was drawn (a click toggles the huddle view).
    call_bar_area: ratatui::layout::Rect,
    /// A profile picture the call bar wants loaded.
    call_bar_wanted: Option<(slack_core::types::File, (u16, u16))>,
    /// What the main pane shows.
    view: View,
    /// Where the main pane was, for back and forward (this workspace's, kept across switches).
    history: super::history::History,
    /// The Activity view (Ctrl+A), shown while `view` is Activity.
    activity: super::activity::Activity,
    /// The save dialog (`␣s`).
    save: Option<super::save::SaveDialog>,
    /// The Threads view (Ctrl+T), shown while `view` is Threads.
    threads: super::threads::Threads,
    /// The Huddles view (␣fh), shown while `view` is Huddles.
    huddles: super::huddles::Huddles,
    /// The thread (channel, its parent's ts) the composer under the Threads feed replies in, once the Type key chose
    /// it there; its draft is in the composer meanwhile.
    feed_reply: Option<(String, String)>,
    /// The composer had the focus in the Threads view at the last look (it aims at the selected
    /// message's thread whenever it gets it).
    feed_typing: bool,
    /// When the previewed channel was last fetched again.
    preview_polled: Option<std::time::Instant>,
    /// When the people shown were last looked up for their statuses (the slow safety net).
    statuses_polled: Option<std::time::Instant>,
    /// People to look up again (`users.info`) for their statuses.
    lookups: Lookups,
    /// The kind of the current (or last) websocket; None before the first connection.
    socket: Option<slack_core::api::Socket>,
    /// Since when the websocket is down (the first drop of a run of failed reconnects).
    rtm_down_since: Option<std::time::Instant>,
    /// When a reconnect last queued the people shown for a look-up.
    people_requeued: Option<std::time::Instant>,
    /// Where the next status refresh goes on among the sidebar's DMs and group DMs off screen.
    status_turn: usize,
    /// Open "move to section" picker.
    mover: Option<super::sections::MovePicker>,
    /// Join the huddle of the DM with this person once it is open.
    huddle_after_open: Option<String>,
    /// A huddle that ended before it connected was joined again (once).
    rejoined: bool,
    /// When your "in a huddle" state is renewed next (None: not in a connected huddle).
    huddle_beat: Option<std::time::Instant>,
    /// What the open composers held at the last look, since when, and whether that is saved.
    autosave: (Option<super::composer::Draft>, Option<super::composer::Draft>, std::time::Instant, bool),
    /// Open search overlay (Ctrl+F).
    search: Option<super::search::Search>,
    /// A search hit to select once its conversation is shown.
    pending_hit: Option<super::search::Hit>,
    /// A message (channel, top-level ts) older than the loaded history, being paged back to.
    seeking: Option<(String, String)>,
    /// A link (or a notification) started the seek: a message that isn't there says so.
    seek_from_link: bool,
    /// The link's hit, selected once the seek finds its message (never when it is missing).
    seek_hit: Option<super::search::Hit>,
    /// The seek fetched the conversation's newest page again (once, before saying a newer message is missing).
    seek_refreshed: bool,
    /// A thread reply a link leads to (channel, parent, reply): said to be missing once the thread is in without it.
    link_reply: Option<(String, String, String)>,
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
    /// What the link under the mouse pointer does (the status line shows it while nothing else is said).
    link_note: Option<String>,
    /// Where the mouse pointer was last seen (None when it left the window): the hover note is worked out from it at
    /// every draw, so scrolling, a keyboard move or a new conversation under a still pointer is followed too.
    pointer: Option<(u16, u16)>,
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

/// Whether a key press clears the status note: a command or the start/end of a sequence does,
/// plain typing (a key bound to nothing outside a sequence) and editing in the message box
/// (cursor moves, deleting, a new line, pasting, formatting) do not. It clears before the
/// action runs, so a note the action sets survives.
fn clears_note(resolved: &Resolved, in_sequence: bool) -> bool {
    use super::keymap::Action::*;
    match resolved {
        Resolved::Action(Context::Composer, Newline | Bold | Italic | Strike | Code | DeleteWord | DeleteLine | LineStart | LineEnd | WordLeft | WordRight | Left | Right | Up | Down | Backspace | DeleteForward | Paste) => false,
        Resolved::Action(..) | Resolved::Pending => true,
        Resolved::None => in_sequence,
    }
}

/// Whether a command key drops `status`: not while its work goes on ("uploading x…").
fn note_clears(status: &str) -> bool {
    !status.ends_with('…')
}

/// Whether a status shown for `age` should go.
fn status_expired(status: &str, age: Duration) -> bool {
    !status.is_empty() && note_clears(status) && age >= STATUS_TTL
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
                Err(e) => Net::HistoryFailed { channel, error: e.to_string() },
            };
            let _ = tx.send(msg);
        });
    }

    /// Follow the sidebar selection into the message pane.
    fn sync_current(&mut self) {
        let selected = self.sidebar.selected_id().map(String::from);
        // On a folded section's header (or a view's row) the conversation stays as it was.
        if selected.is_none() && (self.sidebar.selected_folded().is_some() || self.sidebar.on_view().is_some()) {
            return;
        }
        // A view keeps its row selected while it is shown: a conversation selected in the
        // sidebar (the keys, a click, a jump) is shown instead.
        if selected.is_some() && self.view != View::Conversation {
            self.set_view(View::Conversation);
        }
        if selected != self.current {
            if self.ws.preview.is_some() && self.ws.preview != selected {
                self.ws.preview = None;
                self.sidebar.rebuild(&self.ws);
            }
            if let Some(old) = &self.current {
                stash(&mut self.drafts, old, &mut self.composer);
                // Marked unread and left: opening it again marks it read as usual.
                self.ws.release_unread(old);
            }
            if let Some(d) = selected.as_ref().and_then(|id| self.drafts.remove(id)) {
                self.composer.restore(d);
            }
            self.save_drafts();
            self.current = selected.clone();
            // A saved thread is only reopened in the conversation it was saved for.
            self.start = self.start.take().filter(|(c, _)| selected.as_ref() == Some(c));
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
        let Some(channel) = self.current.clone() else { return };
        let Some(latest) = read_mark_target(&self.ws, &channel, &self.marked) else { return };
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

    /// The slow safety net under the pushed changes: every `status_refresh_secs` (ten minutes by default) — the
    /// first time a full interval after start — the people shown are looked up again, at most
    /// [`STATUS_REFRESH_MAX`]: the authors of the open conversation and thread, the sidebar's DMs and group DMs on
    /// screen, then its others in turn (see [`Workspace::people_shown`]).
    fn refresh_statuses(&mut self) {
        let secs = self.ws.settings.messages.status_refresh_secs;
        if secs == 0 || !self.connected || self.ws.users.is_empty() || self.lookups.busy {
            return;
        }
        match self.statuses_polled {
            Some(at) if at.elapsed() < Duration::from_secs(secs as u64) => return,
            None => {
                self.statuses_polled = Some(std::time::Instant::now());
                return;
            }
            Some(_) => self.statuses_polled = Some(std::time::Instant::now()),
        }
        self.queue_people_shown();
    }

    /// Put the people shown (at most [`STATUS_REFRESH_MAX`]) up for looking up again.
    fn queue_people_shown(&mut self) {
        let rows = self.sidebar.on_screen();
        let (mut on_screen, mut others): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
        for (n, item) in self.sidebar.items.iter().enumerate() {
            if let super::sidebar::Item::Conversation(id) = item {
                if rows.contains(&n) { on_screen.push(id) } else { others.push(id) }
            }
        }
        let thread = self.thread.as_ref().map(|(c, ts)| super::state::thread_key(c, ts));
        let timelines: Vec<&str> = self.current.as_deref().into_iter().chain(thread.as_deref()).collect();
        let people = self.ws.people_shown(&timelines, &on_screen, &others, &mut self.status_turn, STATUS_REFRESH_MAX);
        for id in people {
            self.lookups.stale.remove(&id);
            self.lookups.due.insert(id);
        }
    }

    /// Slack's gateway pushed that `id`'s profile (status, huddle state) changed: look them up again if they are
    /// shown, else once they are.
    fn profile_invalidated(&mut self, id: String) {
        if self.followed.0.contains(&id) {
            self.lookups.due.insert(id);
        } else {
            self.lookups.stale.insert(id);
        }
    }

    /// Look the people due up (`users.info`, one after another, at most [`STATUS_REFRESH_MAX`] a run), one run
    /// at a time.
    fn look_up_people(&mut self) {
        if self.lookups.busy || self.lookups.due.is_empty() || !self.connected {
            return;
        }
        let people: Vec<String> = self.lookups.due.iter().take(STATUS_REFRESH_MAX).cloned().collect();
        for id in &people {
            self.lookups.due.remove(id);
        }
        self.lookups.busy = true;
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            // One after another: a gentle pace for Slack.
            let mut users = Vec::new();
            for id in people {
                match client.user_info(&id).await {
                    Ok(u) => users.push(u),
                    Err(e) => tracing::debug!("refreshing a status: {e}"),
                }
            }
            let _ = tx.send(Net::Profiles(users));
        });
    }

    /// Mark the open thread read up to its newest reply (once per new reply).
    fn mark_thread_read(&mut self) {
        let Some((channel, parent)) = self.thread.clone().filter(|(c, _)| !self.ws.is_outside(c)) else { return };
        let key = super::state::thread_key(&channel, &parent);
        let Some(latest) = self.ws.threads.get(&key).and_then(|t| thread_mark_target(t, &parent)) else { return };
        self.mark_thread_on_slack(channel, parent, latest);
    }

    /// Mark thread `parent` of `channel` read on Slack up to reply `latest` (once per reply).
    fn mark_thread_on_slack(&mut self, channel: String, parent: String, latest: String) {
        let key = super::state::thread_key(&channel, &parent);
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
        let feed = self.feed_reply.clone().filter(|_| self.view == View::Threads);
        let (channel, thread_ts) = match (thread, self.thread.clone(), feed) {
            (true, Some((channel, ts)), _) => (channel, Some(ts)),
            (false, _, Some((channel, ts))) => (channel, Some(ts)),
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
        // Under the Threads feed, the composer replies in the thread chosen there.
        let feed = self.feed_reply.clone().filter(|_| !thread && self.view == View::Threads);
        let Some(channel) = feed.as_ref().map(|(c, _)| c.clone()).or_else(|| self.current.clone()) else { return };
        // Signed out: what is typed stays in the composer; only klack's own commands run.
        if self.signed_out_note.is_some() {
            let c = self.composer_for(thread);
            let local = c.attachments.is_empty() && matches!(super::commands::parse(&c.text(), &c.to_mrkdwn()), Some(Ok(cmd)) if runs_while_signed_out(&cmd));
            if !local && self.refused_signed_out() {
                return;
            }
        }
        let thread_ts = if thread { self.thread.as_ref().map(|(_, ts)| ts.clone()) } else { feed.map(|(_, ts)| ts) };
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
            Command::Dnd(action) => {
                use super::commands::DndAction;
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
                let action = action.decided(self.ws.dnd_active(now));
                let resume = self.ws.dnd_resume(now);
                let (client, tx) = (self.client.clone(), self.net.clone());
                tokio::spawn(async move {
                    let done = async {
                        match action {
                            DndAction::Pause(m) => client.snooze(Some(m)).await?,
                            _ => {
                                if resume.snooze {
                                    client.snooze(None).await?;
                                }
                                if resume.period {
                                    client.end_dnd().await?;
                                }
                            }
                        }
                        anyhow::Ok(())
                    }
                    .await;
                    // What is in effect now, for the status line and the note — also after a failure,
                    // which may have got half-way.
                    let info = client.call_raw("dnd.info", &[]).await.inspect_err(|e| tracing::warn!("dnd.info failed: {e}")).ok();
                    let error = done.err().map(|e| e.to_string());
                    let _ = tx.send(Net::DndSet { resumed: action == DndAction::Resume, error, info });
                });
            }
            Command::DndSchedule(Some(edits)) => self.edit_schedule(edits),
            Command::DndSchedule(None) => {
                let zone = super::prefs::schedule_zone(&self.ws);
                self.status = format!("notification schedule: {}", super::prefs::describe_schedule(self.ws.dnd.schedule.as_ref(), self.ws.settings.time_format(), &zone));
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

    /// `␣n…`, `␣as`, `␣aN`: `command` in the conversation's message box (also from the thread
    /// pane), the cursor after it, typing there; a draft wraps as `wrap` says, or it refuses.
    fn prefill(&mut self, command: &str, wrap: Wrap) {
        if self.current.is_none() {
            self.status = "open a conversation first".into();
            return;
        }
        // In the Threads view: the box of the selected message's thread.
        let thread = self.view == View::Threads && self.aim_feed_composer() == Some(Focus::ThreadComposer);
        let (target, other) = if thread { (&self.thread_composer, &self.composer) } else { (&self.composer, &self.thread_composer) };
        match prefill_box(target, other.editing.is_some(), command, wrap) {
            Ok((text, cursor)) => {
                self.composer_for(thread).set_with_cursor(&text, cursor);
                self.focus = if thread { Focus::ThreadComposer } else { Focus::Composer };
                self.feed_typing = self.view == View::Threads && !thread;
            }
            Err(note) => self.status = note.into(),
        }
    }

    /// Esc in a composer: back to the message list (the reply box: the thread), the newest
    /// message selected.
    fn leave_composer(&mut self, thread: bool) {
        self.focus = if thread { Focus::Thread } else { Focus::Messages };
        // Under the Threads feed: back to the feed, its selection as it was; with nothing typed, in
        // no thread any more (the next box goes to the selection then).
        if !thread && self.view == View::Threads {
            if self.composer.is_empty() && self.composer.attachments.is_empty() {
                self.set_feed_reply(None);
            }
            return;
        }
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
            Some(Action::Back) => self.stop_seek(),
            Some(Action::Type) if in_thread => self.focus = Focus::ThreadComposer,
            Some(Action::Type) => self.focus = Focus::Composer,
            Some(Action::OpenThread) if !in_thread => {
                if let Some(ts) = self.messages.selected.clone() {
                    self.open_thread(&ts);
                }
            }
            Some(Action::OpenThread) => self.status = "already in the thread".into(),
            Some(Action::React) => {
                if let (Some(channel), Some(ts)) = (self.current.clone(), self.selected_ts()) {
                    let picker = self.ws.find_message(&channel, &ts).map(|m| EmojiPicker::for_message(&m.reactions, &self.ws.self_id)).unwrap_or_default();
                    self.picker = Some((picker, channel, ts));
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
            Some(Action::MarkUnread) => self.mark_unread(self.messages.selected.clone(), in_thread),
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
            self.menu = Some(Menu::new(Target::Conversation, at, &self.keys, none, channel, self.ws.starred.contains(&id), self.ws.muted.contains(&id)));
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
        self.menu = Some(Menu::new(Target::Message { thread }, at, &self.keys, offers, false, false, false));
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
            (_, Item::Thing(_) | Item::Heading) | (Target::Opener { .. }, _) => {}
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
            Action::Open if self.sidebar.on_view().is_some() => {
                if let Some(v) = self.sidebar.on_view().and_then(|i| view_of_row(&i)) {
                    self.open_view(v);
                }
            }
            Action::Open | Action::Type => self.focus = Focus::Composer,
            // The selected conversation is the current one.
            Action::Details | Action::NotificationLevel | Action::MuteConversation | Action::CopyConversationLink | Action::MarkUnread | Action::LeaveConversation if self.sidebar.selected_id().is_some() => {
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

    /// True (and the status line says why) when this workspace is signed out, so nothing is sent.
    fn refused_signed_out(&mut self) -> bool {
        let Some(note) = self.signed_out_note.clone() else { return false };
        self.status = note;
        true
    }

    /// Save the message being edited in a composer (optimistic, then chat.update). Signed out, the
    /// edit stays in the composer.
    fn save_edit(&mut self, thread: bool) {
        if self.refused_signed_out() {
            return;
        }
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
        if self.refused_signed_out() {
            return;
        }
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
        if self.refused_signed_out() {
            return;
        }
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
        if text.trim().is_empty() {
            self.status = "no text in this message".into();
            return;
        }
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
        self.sidebar.remember_thread(Some(&parent));
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
        self.sidebar.remember_thread(None);
        if let Some((c, t)) = self.thread.take() {
            stash(&mut self.drafts, &super::state::thread_key(&c, &t), &mut self.thread_composer);
            self.save_drafts();
        }
        self.broadcast = false;
        if self.call.is_some() && self.huddle_view.is_none() {
            self.huddle_view = self.parked_huddle.take();
        }
        if matches!(self.focus, Focus::Thread | Focus::ThreadComposer) {
            self.focus = focus_after_thread(self.huddle_view.is_some());
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
        let feed = self.feed_reply.as_ref().filter(|_| self.view == View::Threads).map(|(c, _)| c.clone());
        let conversation = if thread { self.thread.as_ref().map(|(c, _)| c.clone()) } else { feed.or_else(|| self.current.clone()) };
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

    /// Close the search and open its selected hit (Enter, or a click on it).
    fn open_search_hit(&mut self) {
        let Some(hit) = self.search.as_ref().and_then(|s| s.hits.get(s.selected).cloned()) else { return };
        self.search = None;
        self.open_hit(hit);
    }

    /// Leave the Threads view for its selected thread (Enter, or a click on it).
    fn open_thread_item(&mut self) {
        let Some((channel, ts, thread_ts)) = self.threads.target() else { return };
        self.close_view();
        self.open_hit(super::search::Hit { channel, ts, thread_ts, user: None, text: String::new(), ..Default::default() });
    }

    /// Mark the Activity view's selected item seen (and read on Slack, as its own client does: the badge drops
    /// at once) and leave the view for it (Enter, or a click on it); with nowhere in klack to show it, say so.
    fn open_activity_item(&mut self) {
        if let Some(it) = self.activity.selected_item().cloned() {
            self.seen.insert(it.key.clone(), opened_now());
            if let Some(params) = super::activity::mark_read_params(&it) {
                let client = self.client.clone();
                tokio::spawn(async move {
                    if let Err(e) = client.activity_mark_read(&params).await {
                        tracing::warn!("marking the activity item read failed: {e}");
                    }
                });
                // (Not without the call: Slack still counts it.)
                self.ws.activity_item_read(&it.ty);
            }
            self.activity.apply_seen(&self.seen);
            if let Some(items) = self.activity.items.as_ref().filter(|_| self.ws.activity_counts.is_none()) {
                self.ws.activity_unread = super::activity::unread_count(items);
            }
        }
        // Nowhere to show it in klack (a list item, a Slack Connect invitation): said so, the view stays.
        let Some(target) = self.activity.target() else {
            if self.activity.selected_item().is_some() {
                self.status = super::i18n::tr(super::activity::NOT_HERE).into_owned();
            }
            return;
        };
        self.close_view();
        self.open_in_app(target, false);
    }

    /// Show view `v` in the main pane (its sidebar row selected, the focus on it) and load it;
    /// already shown, it is loaded again.
    fn open_view(&mut self, v: View) {
        self.show_view(v, None);
    }

    /// [`App::open_view`], the Activity view on tab `tab` (None: the one it is on when shown already, else the first).
    fn show_view(&mut self, v: View, tab: Option<super::activity::Tab>) {
        if let Some(row) = row_of_view(v) {
            self.sidebar.select_view(&row);
        }
        let again = self.view == v;
        self.set_view(v);
        self.focus = Focus::Messages;
        match v {
            View::Threads => self.load_threads(false),
            View::Activity => self.open_activity(tab.or(again.then_some(self.activity.tab))),
            View::Huddles => self.load_huddles(),
            View::Conversation => {}
        }
    }

    /// Load the Huddles view afresh: the huddles going on now, and the first page of the recent ones.
    fn load_huddles(&mut self) {
        self.fetch_live_huddles();
        let generation = self.huddles.reload();
        self.fetch_huddle_history(generation, None);
    }

    /// Ask Slack for the huddles going on now (they replace those kept).
    fn fetch_live_huddles(&self) {
        let (client, tx, since) = (self.client.clone(), self.net.clone(), self.ws.huddle_seq);
        tokio::spawn(async move {
            let result = client.huddles_live().await.map_err(|e| e.to_string());
            let _ = tx.send(Net::LiveHuddles { since, result });
        });
    }

    /// Ask for a page of the recent huddles (the first, or the one at `cursor`) for load `generation`.
    fn fetch_huddle_history(&self, generation: u64, cursor: Option<String>) {
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let result = client.huddles_history(cursor.as_deref(), super::huddles::PAGE).await.map_err(|e| e.to_string());
            let _ = tx.send(Net::HuddleHistory { generation, more: cursor.is_some(), result });
        });
    }

    /// The Huddles view's next page, once the end of its list is on screen; and the reply counts of the threads
    /// of the recent huddles on screen.
    fn request_huddles(&mut self) {
        if self.view != View::Huddles {
            return;
        }
        if let Some(cursor) = self.huddles.next_page() {
            self.fetch_huddle_history(self.huddles.generation, Some(cursor));
        }
        for (channel, ts) in self.huddles.wanted() {
            self.huddles.requested.insert((channel.clone(), ts.clone()));
            let (client, tx) = (self.client.clone(), self.net.clone());
            tokio::spawn(async move {
                let count = match client.message_at(&channel, &ts).await {
                    Ok(m) => m.map(|m| m.reply_count),
                    Err(e) => {
                        tracing::warn!("a huddle's thread: {e}");
                        None
                    }
                };
                let _ = tx.send(Net::HuddleReplies { channel, ts, count });
            });
        }
    }

    /// Leave the Huddles view for its selected row (Enter, or a click on it): a live huddle's conversation, a recent
    /// one's thread (its conversation when it has none).
    fn open_huddle_item(&mut self) {
        match self.huddles.target() {
            Some(super::huddles::Row::Live(channel)) => {
                self.close_view();
                self.jump_to(&channel);
            }
            Some(super::huddles::Row::Past(n)) => {
                let Some(h) = self.huddles.past_at(n).cloned() else { return };
                self.close_view();
                match h.thread_ts {
                    Some(ts) => self.open_hit(super::search::Hit { channel: h.channel, ts: ts.clone(), thread_ts: Some(ts), user: None, text: String::new(), ..Default::default() }),
                    None => self.jump_to(&h.channel),
                }
            }
            None => {}
        }
    }

    /// Where the main pane is, as the history keeps it (None: no conversation yet).
    fn place(&self) -> Option<Place> {
        match self.view {
            View::Threads => Some(Place::Threads),
            View::Activity => Some(Place::Activity(self.activity.tab)),
            View::Huddles => Some(Place::Huddles),
            View::Conversation => {
                let channel = self.current.clone()?;
                let thread = self.thread.as_ref().filter(|(c, _)| *c == channel).map(|(_, ts)| ts.clone());
                let member = !self.ws.is_outside(&channel);
                Some(Place::Conversation { channel, thread, member })
            }
        }
    }

    /// After an event: the place shown now goes into the history (`went`: by your doing, see [`goes_somewhere`]).
    fn track_place(&mut self, went: bool) {
        let place = self.place();
        self.history.arrive(place, went);
    }

    /// Back to the place left last (`forward`: forward to the one gone back from), as opening it does; with
    /// nowhere to go, say so.
    fn step_history(&mut self, forward: bool) {
        let ws = &self.ws;
        let gone = |p: &Place| place_gone(ws, p);
        let to = if forward { self.history.forward(gone) } else { self.history.back(gone) };
        let Some(to) = to else {
            self.status = super::i18n::tr(if forward { super::history::NOTHING_FORWARD } else { super::history::NOTHING_BACK }).into_owned();
            return;
        };
        match to {
            Place::Threads => self.open_view(View::Threads),
            Place::Huddles => self.open_view(View::Huddles),
            Place::Activity(tab) => self.show_view(View::Activity, Some(tab)),
            Place::Conversation { channel, thread: Some(ts), .. } => self.open_hit(super::search::Hit { channel, ts: ts.clone(), thread_ts: Some(ts), ..Default::default() }),
            Place::Conversation { channel, thread: None, .. } if self.current.as_deref() == Some(channel.as_str()) => {
                self.close_view();
                self.close_thread();
            }
            Place::Conversation { channel, thread: None, .. } => self.jump_to(&channel),
        }
    }

    /// Back from a view to the conversation it was opened over, on its messages.
    fn close_view(&mut self) {
        self.set_view(View::Conversation);
        self.focus = Focus::Messages;
        if let Some(id) = self.current.as_deref() {
            self.sidebar.select_row_of(id);
        }
    }

    /// The composer's draft key: under the Threads feed the thread it replies in (none chosen: none, it
    /// is empty there), else the conversation's.
    fn composer_key(&self) -> Option<String> {
        match (&self.feed_reply, self.view) {
            (Some((c, ts)), _) => Some(super::state::thread_key(c, ts)),
            (None, View::Threads) => None,
            (None, _) => self.current.clone(),
        }
    }

    /// The composer changed hands (from the draft key `old` to the current one): keep the draft it
    /// had and take up the one it goes to.
    fn rekey_composer(&mut self, old: Option<String>) {
        let new = self.composer_key();
        if old == new {
            return;
        }
        if let Some(old) = &old {
            stash(&mut self.drafts, old, &mut self.composer);
        }
        // (With nothing to keep it under, what is typed stays, the draft taken up below it.)
        if let Some(d) = new.and_then(|k| self.drafts.remove(&k)) {
            self.composer.merge(d);
        }
        self.save_drafts();
    }

    /// Show `v` in the main pane; the composer goes with it (under the Threads feed it replies in a thread).
    fn set_view(&mut self, v: View) {
        let old = self.composer_key();
        if v != View::Threads {
            self.feed_reply = None;
        }
        self.view = v;
        self.rekey_composer(old);
    }

    /// Make the composer reply in `target` under the Threads feed (None: in no thread), keeping the
    /// draft of the one it leaves and taking up the one it goes to.
    fn set_feed_reply(&mut self, target: Option<(String, String)>) {
        let old = self.composer_key();
        self.feed_reply = target;
        self.rekey_composer(old);
    }

    /// A box gets the focus in the Threads view: it replies in the selected message's thread — the
    /// thread pane's own box when that thread is open there (one box per thread draft), else the one
    /// under the feed. None: no message is selected.
    fn aim_feed_composer(&mut self) -> Option<Focus> {
        let thread = self.threads.selected_thread()?;
        if self.thread.as_ref() == Some(&thread) {
            self.set_feed_reply(None);
            return Some(Focus::ThreadComposer);
        }
        self.set_feed_reply(Some(thread));
        Some(Focus::Composer)
    }

    /// After an event (and before a draw): the composer that just got the focus in the Threads view,
    /// by whatever way, replies in the selected message's thread.
    fn sync_feed_composer(&mut self) {
        let typing = self.view == View::Threads && self.focus == Focus::Composer;
        if typing && !self.feed_typing {
            if let Some(focus) = self.aim_feed_composer() {
                self.focus = focus;
            }
        }
        self.feed_typing = self.view == View::Threads && self.focus == Focus::Composer;
    }

    /// Thread `parent` of `channel` in the Threads feed was read there (on screen long enough, or replied to): marked
    /// read on Slack up to its newest reply, and here.
    fn mark_feed_read(&mut self, channel: String, parent: String) {
        self.seen.insert(super::state::thread_key(&channel, &parent), opened_now());
        self.threads.mark_read(&channel, &parent);
        if self.ws.is_outside(&channel) {
            return;
        }
        if let Some(latest) = self.threads.newest_reply(&channel, &parent) {
            self.mark_thread_on_slack(channel, parent, latest);
        }
    }

    /// The Threads feed after a draw: threads on screen long enough are marked read, and the next page is loaded
    /// once the end of the feed is on screen.
    fn feed_tick(&mut self, now: std::time::Instant) {
        let shown = self.view == View::Threads && self.threads.area != Rect::default() && self.overlay().is_none();
        let after = Duration::from_secs(self.ws.settings.threads.read_after_secs as u64);
        for (channel, parent) in self.threads.tick(shown, now, after) {
            self.mark_feed_read(channel, parent);
        }
        if shown && self.threads.at_end() {
            self.load_threads(true);
        }
    }

    /// Fetch the newest replies of the threads on screen in the Threads feed that show fewer than they have.
    fn request_feed_replies(&mut self) {
        if self.view != View::Threads {
            return;
        }
        for (channel, ts) in self.threads.wanted_replies() {
            let (client, tx) = (self.client.clone(), self.net.clone());
            tokio::spawn(async move {
                let result = client.latest_replies(&channel, &ts, FEED_REPLIES).await.map_err(|e| e.to_string());
                let _ = tx.send(Net::FeedReplies { channel, ts, result });
            });
        }
    }

    /// The keys' context of the view shown in the main pane (none for the conversation).
    fn view_context(&self) -> Option<Context> {
        match self.view {
            View::Threads => Some(Context::Threads),
            View::Activity => Some(Context::Activity),
            View::Huddles => Some(Context::Huddles),
            View::Conversation => None,
        }
    }

    /// An action of the view shown (its keys while the main pane has the focus).
    fn view_action(&mut self, action: Action) {
        match self.view {
            View::Threads => {
                let t = &mut self.threads;
                match action {
                    Action::Close => self.close_view(),
                    Action::Up => t.move_by(-1),
                    Action::Down => t.move_by(1),
                    Action::PageUp => t.move_by(-t.page()),
                    Action::PageDown => t.move_by(t.page()),
                    Action::Refresh => self.open_view(View::Threads),
                    Action::Open => self.open_thread_item(),
                    // The composer under the feed, replying in the selected message's thread.
                    Action::Type => {
                        if let Some(focus) = self.aim_feed_composer() {
                            self.focus = focus;
                            self.feed_typing = focus == Focus::Composer;
                        }
                    }
                    _ => {}
                }
            }
            View::Activity => {
                let a = &mut self.activity;
                match action {
                    Action::Close => self.close_view(),
                    Action::Up => a.move_by(-1),
                    Action::Down => a.move_by(1),
                    Action::PageUp => a.move_by(-a.page()),
                    Action::PageDown => a.move_by(a.page()),
                    Action::Refresh => self.open_view(View::Activity),
                    Action::Open => self.open_activity_item(),
                    Action::NextTab => {
                        let next = a.tab.next();
                        self.show_activity_tab(next);
                    }
                    Action::PrevTab => {
                        let prev = a.tab.prev();
                        self.show_activity_tab(prev);
                    }
                    Action::Tab1 => self.show_activity_tab(super::activity::Tab::All),
                    Action::Tab2 => self.show_activity_tab(super::activity::Tab::Dms),
                    Action::Tab3 => self.show_activity_tab(super::activity::Tab::Mentions),
                    Action::Tab4 => self.show_activity_tab(super::activity::Tab::Threads),
                    Action::Unreads => a.toggle_unread(),
                    _ => {}
                }
            }
            View::Huddles => {
                let h = &mut self.huddles;
                match action {
                    Action::Close => self.close_view(),
                    Action::Up => h.move_by(-1),
                    Action::Down => h.move_by(1),
                    Action::PageUp => h.move_by(-h.page()),
                    Action::PageDown => h.move_by(h.page()),
                    Action::Refresh => self.open_view(View::Huddles),
                    Action::Open => self.open_huddle_item(),
                    _ => {}
                }
            }
            View::Conversation => {}
        }
    }

    /// The area of the view shown at the last draw (empty for the conversation).
    fn view_area(&self) -> Rect {
        match self.view {
            View::Threads => self.threads.area,
            View::Activity => self.activity.area,
            View::Huddles => self.huddles.area,
            View::Conversation => Rect::default(),
        }
    }

    /// Run the search overlay's query, or open its highlighted hit if the query is unchanged.
    fn search_enter(&mut self) {
        let Some(search) = self.search.as_mut() else { return };
        let query = search.query.trim().to_string();
        if query.is_empty() {
            return;
        }
        if query == search.searched && !search.busy {
            self.open_search_hit();
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
        self.open_hit_from(hit, false);
    }

    /// `open_hit`; `from_link`: a link (or a notification) leads there, so a missing message says so.
    fn open_hit_from(&mut self, hit: super::search::Hit, from_link: bool) {
        // Its conversation takes the main pane from a view.
        if self.view != View::Conversation {
            self.close_view();
        }
        self.seek_from_link = from_link;
        (self.seek_hit, self.seek_refreshed, self.link_reply) = (None, false, None);
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
        let seek = self.continue_seek();
        if select_now(self.seek_from_link, seek) {
            self.select_hit(hit);
        } else if seek.is_none_or(|s| matches!(s, Seek::More | Seek::Refresh)) {
            // A link's message: selected once the seek finds it.
            self.seek_hit = Some(hit);
        }
    }

    /// Select a hit's message (the reply in its open thread).
    fn select_hit(&mut self, hit: super::search::Hit) {
        match hit.thread_ts {
            Some(parent) => {
                self.messages.selected = Some(parent.clone());
                if self.seek_from_link && hit.ts != parent {
                    self.link_reply = Some((hit.channel.clone(), parent.clone(), hit.ts.clone()));
                }
                self.open_thread_at(hit.channel, parent);
                self.thread_pane.selected = Some(hit.ts);
                self.check_link_reply();
            }
            None => {
                self.messages.selected = Some(hit.ts);
                self.focus = Focus::Messages;
            }
        }
    }

    /// Page the history back until the message being sought is loaded (the selection then
    /// shows it); stop at the beginning of the conversation or when it is no longer shown.
    /// What it did: None while it waits for the history (or has nothing to seek).
    fn continue_seek(&mut self) -> Option<Seek> {
        let (channel, ts) = self.seeking.clone()?;
        if self.current.as_deref() != Some(channel.as_str()) {
            // Not shown yet (the conversation is being switched to) or left: wait, or drop it.
            if self.pending_hit.is_none() {
                self.seeking = None;
                self.seek_hit = None;
            }
            return None;
        }
        let tl = self.ws.timelines.get(&channel)?;
        if !tl.loaded || tl.loading {
            return None;
        }
        let found = tl.index_of(&ts).is_some();
        let older = tl.messages.first().is_some_and(|m| seek_older(&ts, &m.ts));
        let newer = tl.messages.last().is_none_or(|m| seek_older(&m.ts, &ts));
        let (has_more, n) = (tl.has_more, tl.messages.len());
        let step = seek_step(Sought { found, older, newer, has_more }, self.seek_from_link, self.seek_refreshed);
        match step {
            Seek::Done => {
                self.seeking = None;
                let seeking = super::i18n::tr(SEEKING);
                if seeking.split("{}").next().is_some_and(|p| !p.is_empty() && self.status.starts_with(p)) {
                    self.status.clear();
                }
                if let Some(hit) = self.seek_hit.take() {
                    self.select_hit(hit);
                }
            }
            Seek::More => {
                self.status = super::i18n::tr(SEEKING).replace("{}", &n.to_string());
                self.load_history(&channel, true);
            }
            Seek::Refresh => {
                self.seek_refreshed = true;
                self.status = super::i18n::tr(SEEKING).replace("{}", &n.to_string());
                self.refresh_history(&channel);
            }
            Seek::Missing(note) => {
                // A link's message was never selected: the conversation stays as it is.
                (self.seeking, self.seek_hit, self.link_reply) = (None, None, None);
                self.status = super::i18n::tr(note).into_owned();
            }
        }
        Some(step)
    }

    /// Fetch the newest page of `channel` again, though its history is loaded (merged with what is there).
    fn refresh_history(&mut self, channel: &str) {
        let tl = self.ws.timelines.entry(channel.to_string()).or_default();
        if tl.loading {
            return;
        }
        tl.loading = true;
        let (client, tx, channel) = (self.client.clone(), self.net.clone(), channel.to_string());
        tokio::spawn(async move {
            let msg = match client.history(&channel, None, 50).await {
                Ok(page) => Net::History { channel, page, older: false },
                Err(e) => Net::HistoryFailed { channel, error: e.to_string() },
            };
            let _ = tx.send(msg);
        });
    }

    /// Esc in the messages pane: stop paging back for a message.
    fn stop_seek(&mut self) {
        if self.seeking.take().is_some() {
            (self.pending_hit, self.seek_hit, self.link_reply) = (None, None, None);
            self.status = super::i18n::tr(SEEK_STOPPED).into_owned();
        }
    }

    /// The thread reply a link leads to, once its thread is in: not there → say so.
    fn check_link_reply(&mut self) {
        let Some((channel, parent, reply)) = self.link_reply.clone() else { return };
        let Some(tl) = self.ws.threads.get(&super::state::thread_key(&channel, &parent)).filter(|t| t.loaded) else { return };
        self.link_reply = None;
        if tl.index_of(&reply).is_none() && self.thread.as_ref().is_some_and(|(c, t)| *c == channel && *t == parent) {
            self.thread_pane.selected = None;
            self.status = super::i18n::tr(LINK_GONE).into_owned();
        }
    }

    /// A plain click on a link into the profile's workspaces: open where it goes, in klack (another workspace is
    /// switched to first and opens it once booted).
    fn open_app_link(&mut self, link: &super::applink::AppLink) {
        let Some(at) = super::applink::workspace_of(link, &self.addresses, self.workspace) else { return };
        if at == self.workspace {
            self.open_in_app(link.target.clone(), true);
            return;
        }
        *self.open_after.lock().unwrap_or_else(|e| e.into_inner()) = Some((at, link.target.clone()));
        self.switch_workspace(at);
        if self.switch_to.is_none() {
            // Not switched (the reason is in the status line): nothing waits for that workspace.
            self.open_after.lock().unwrap_or_else(|e| e.into_inner()).take();
        }
    }

    /// Open a target in this workspace: a conversation as Jump to does (previewed when you are not in it), a message
    /// selected (paging back to it, a reply in its open thread), a person's profile card, a file in its viewer.
    /// `from_link`: a message that isn't there says so. A conversation this workspace doesn't know is asked about
    /// first.
    fn open_in_app(&mut self, target: super::applink::Target, from_link: bool) {
        use super::applink::Target;
        if let Some(channel) = target.channel().filter(|c| !self.ws.conversations.contains_key(*c)).map(String::from) {
            let (client, tx) = (self.client.clone(), self.net.clone());
            tokio::spawn(async move {
                let result = client.conversation_info(&channel).await.map_err(|e| e.to_string());
                let _ = tx.send(Net::LinkConversation { result, target });
            });
            return;
        }
        match target {
            Target::Conversation { channel } => self.jump_to(&channel),
            Target::Message { .. } => {
                let Some(hit) = hit_of(target) else { return };
                if self.ws.is_outside(&hit.channel) {
                    self.ws.preview = Some(hit.channel.clone());
                }
                self.open_hit_from(hit, from_link);
            }
            Target::Person { user } => {
                // A change pushed while they were not shown.
                if self.lookups.stale.remove(&user) {
                    self.lookups.due.insert(user.clone());
                }
                if !self.ws.users.contains_key(&user) {
                    let (client, tx, id) = (self.client.clone(), self.net.clone(), user.clone());
                    tokio::spawn(async move {
                        match client.user_info(&id).await {
                            Ok(u) => {
                                let _ = tx.send(Net::UserInfo(u));
                            }
                            Err(e) => tracing::warn!("users.info {id}: {e}"),
                        }
                    });
                }
                self.profile = super::profile::ProfileCard::new(vec![user]);
            }
            Target::File { file } => self.open_file_link(&file),
        }
    }

    /// A link to a file: its viewer when a loaded message carries it and klack has one (pictures, videos, snippets).
    fn open_file_link(&mut self, id: &str) {
        use super::open::Thing;
        let carrier = self.ws.timelines.values().chain(self.ws.threads.values()).flat_map(|t| t.messages.iter()).find(|m| m.files.iter().any(|f| f.id == id));
        let Some(files) = carrier.map(|m| m.files.clone()) else {
            self.status = super::i18n::tr(FILE_NOT_SHOWN).into_owned();
            return;
        };
        let Some(file) = files.iter().find(|f| f.id == id).cloned() else { return };
        if file.is_image() {
            let pictures: Vec<slack_core::types::File> = files.into_iter().filter(|f| f.is_image()).collect();
            let n = pictures.iter().position(|f| f.id == id).unwrap_or(0);
            self.viewer = Some(super::viewer::Viewer::new(pictures, n));
            self.load_viewer_image();
        } else if file.is_video() {
            self.open_thing("", Thing::Video(file));
        } else if file.is_snippet() {
            self.open_thing("", Thing::Snippet(file));
        } else {
            self.status = super::i18n::tr(FILE_NOT_SHOWN).into_owned();
        }
    }

    /// The hover note for the link drawn at a terminal position (the thread pane, then the
    /// messages), or for a person's status over their DM in the sidebar, if any.
    fn link_note_at(&self, column: u16, row: u16) -> Option<String> {
        if let Some(n) = self.sidebar.item_at(column, row) {
            return self.sidebar.status_note(&self.ws, n, chrono::Utc::now().timestamp());
        }
        let (url, app) = self.thread.as_ref().and_then(|_| self.thread_pane.link_under(column, row)).or_else(|| self.messages.link_under(column, row)).or_else(|| self.threads.link_under(column, row)).or_else(|| self.activity.link_under(column, row))?;
        let here = super::applink::Here { ws: &self.ws, workspaces: &self.addresses, names: &self.workspaces, shown: self.workspace, authors: &self.authors, message: None };
        Some(super::applink::hover_note(&url, app.as_ref(), &here))
    }

    /// The link into the profile's workspaces drawn at a terminal position (the thread pane, then the messages).
    fn link_at(&self, column: u16, row: u16) -> Option<super::applink::AppLink> {
        self.thread.as_ref().and_then(|_| self.thread_pane.link_at(column, row)).or_else(|| self.messages.link_at(column, row)).or_else(|| self.threads.link_under(column, row).and_then(|(_, app)| app)).or_else(|| self.activity.link_under(column, row).and_then(|(_, app)| app))
    }

    /// Open what a clicked notification is about: the message (or thread reply), or the
    /// conversation.
    fn open_target(&mut self, target: super::notify::Target) {
        tracing::debug!(?target, "notification clicked");
        self.open_in_app(target.into(), true);
    }

    /// What clicking a desktop notification about `target` does: open it here (a click
    /// after switching workspace finds this session gone and does nothing).
    fn on_click(&self, target: super::notify::Target) -> Box<dyn FnOnce() + Send> {
        let net = self.net.clone();
        Box::new(move || {
            let _ = net.send(Net::NotificationClicked(target));
        })
    }

    /// Show conversation `id`, on its messages. A channel we are not in is previewed.
    fn jump_to(&mut self, id: &str) {
        if self.ws.is_outside(id) {
            self.ws.preview = Some(id.to_string());
        }
        self.reveal(id);
        self.sidebar.select_id(&self.ws, id);
        // In NORMAL, as at start (`i` types).
        self.focus = Focus::Messages;
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

    /// This workspace's session has ended: it is marked in the rail (and not switched back to), the
    /// status line keeps saying so, what is loaded stays readable and nothing more is sent.
    fn mark_signed_out(&mut self) {
        if self.signed_out_note.is_some() {
            return;
        }
        tracing::warn!("workspace {} ({}): the session has ended", self.workspace + 1, self.client.session.team_name);
        self.signed_out.lock().unwrap_or_else(|e| e.into_inner()).insert(self.workspace);
        let note = signed_out_note(&self.workspaces[self.workspace]);
        self.status = note.clone();
        self.signed_out_note = Some(note);
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

    /// Ask Slack for the profile changes (statuses, huddle state) of the people shown — DMs, group
    /// DMs, message authors — as the web client does. On Slack's gateway they come as
    /// `user_invalidated` ([`Self::profile_invalidated`]); on the `rtm.connect` fallback not at all, hence
    /// also [`Self::refresh_statuses`]. New people are added every couple of seconds, in batches of
    /// Slack's limit; one whose change came while not shown is looked up now. Only when who is shown can have changed
    /// (people, conversations, loaded messages: [`Workspace::people_key`]), at most every [`FOLLOW_EVERY`].
    fn follow_profiles(&mut self, now: std::time::Instant) {
        if !self.connected || self.ws.users.is_empty() || self.followed.1.is_some_and(|t| now.duration_since(t) < FOLLOW_EVERY) {
            return;
        }
        self.followed.1 = Some(now);
        let key = self.ws.people_key();
        if self.followed.2 == Some(key) {
            return;
        }
        self.follow_passes += 1;
        let new: Vec<String> = self.ws.people_to_follow().into_iter().filter(|id| !self.followed.0.contains(id)).collect();
        for id in &new {
            if self.lookups.stale.remove(id) {
                self.lookups.due.insert(id.clone());
            }
        }
        for chunk in new.chunks(slack_core::rtm::USER_CHANGE_SUB_MAX) {
            if self.rtm_commands.send(slack_core::rtm::RtmCommand::UserChangeSub { users: chunk.to_vec() }).is_err() {
                return;
            }
            self.followed.0.extend(chunk.iter().cloned());
        }
        // (All asked for: not again until something changes.)
        self.followed.2 = Some(key);
    }

    /// Show the huddle view in place of the messages, or the messages again.
    fn toggle_huddle_view(&mut self) {
        self.parked_huddle = None;
        if self.huddle_view.take().is_some() {
            self.focus = Focus::Messages;
        } else if self.call.is_some() {
            // Narrow: the view and a thread take turns in the messages' place.
            if self.narrow {
                self.close_thread();
            }
            // It takes the main pane from a view as from the messages.
            if self.view != View::Conversation {
                self.close_view();
            }
            self.huddle_view = Some(Default::default());
            self.focus = Focus::Huddle;
        } else {
            self.status = format!("not in a huddle ({} joins this conversation's)", self.keys.first_shown(Context::Global, Action::Huddle).unwrap_or_default());
        }
    }

    /// Share the screen (the desktop asks what; nothing blocks meanwhile), or stop.
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

    /// The huddle's session has ended: we left, it ended before connecting, or its connection was lost for good.
    fn huddle_ended(&mut self) {
        let Some(call) = self.call.take() else { return };
        self.status = "left the huddle".into();
        // Lost for good (the reconnect tries used up): Slack is told we left, as when leaving by hand (once: the
        // call is gone now).
        if call.connected && !call.leaving {
            self.send_leave(&call);
            self.status = super::i18n::tr(CONNECTION_LOST_LEFT).into_owned();
        }
        // Right after a huddle emptied, Slack can still hand out its room though the
        // meeting is over: the session ends before connecting. Ask again once.
        match rejoin_after_end(call.connected, call.leaving, self.rejoined) {
            Some(true) => {
                tracing::info!(channel = call.channel, "the huddle ended before connecting; joining again");
                self.rejoined = true;
                self.join_huddle_after(call.channel, call.thread, REJOIN_DELAY);
            }
            Some(false) => self.status = "cannot join the huddle: it ended as we joined".into(),
            None => {}
        }
    }

    /// Tell Slack we left `call` (`rooms.leave`, in the background; a failure is benign).
    fn send_leave(&self, call: &super::call::Call) {
        spawn_leave(self.client.clone(), call.leave_request());
    }

    /// Join (or start) the huddle of the current conversation, or leave the one we are in.
    fn toggle_huddle(&mut self) {
        if let Some(call) = self.call.as_mut() {
            // Slack is told too (the session ending alone does not), once; a failure is benign.
            if !call.leaving {
                let (client, params) = (self.client.clone(), call.leave_request());
                spawn_leave(client, params);
            }
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
        // In the Huddles view: the live huddle selected (never the conversation hidden under the view).
        if self.view == View::Huddles {
            match self.huddles.target() {
                Some(super::huddles::Row::Live(channel)) => {
                    self.close_view();
                    self.jump_to(&channel);
                    self.join_huddle(channel, None);
                }
                _ => self.status = super::i18n::tr(CHOOSE_LIVE).into_owned(),
            }
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

    /// While you are in a connected huddle, renew your "in a huddle" state when it is due, as
    /// Slack's client does (with the `huddles_set_status` pref on): only its expiry, 5 minutes on,
    /// every ~4 minutes. Slack sets the state when you join and clears it when you leave.
    fn beat_huddle(&mut self, in_call: bool, now: std::time::Instant) {
        if !super::call::huddle_beat_due(&mut self.huddle_beat, in_call, self.ws.huddles_set_status, now, super::call::beat_jitter) {
            return;
        }
        let client = self.client.clone();
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        tokio::spawn(async move {
            match client.set_huddle_state(&super::call::huddle_state_fields(unix)).await {
                Ok(()) => tracing::info!("renewed your huddle state"),
                Err(e) => tracing::warn!("could not renew your huddle state: {e:#}"),
            }
        });
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
        self.fetch_dnd();
    }

    /// Ask Slack for your DND (`dnd.info`) now: it changed elsewhere (the gateway's `dnd_invalidated`).
    fn fetch_dnd(&mut self) {
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

    /// A key pressed while F6 → Keys records: every key is part of the sequence until a pause;
    /// then Enter writes it (the action's keys, or a leader of one key) and Esc drops it.
    fn record_key(&mut self, key: Key) {
        use super::prefs::Field;
        let Some(p) = self.prefs.as_mut() else { return };
        let Some(recording) = p.recording.as_mut() else { return };
        match recording.press(key, std::time::Instant::now()) {
            Some(true) => {}
            Some(false) => p.recording = None,
            None => return,
        }
        let Some(seq) = p.recording.take().map(|r| r.keys) else { return };
        let change = match p.selected_field(&self.keys) {
            Some(Field::Binding(i)) => {
                let info = &super::keymap::ACTIONS[i];
                Change::Binding { context: info.context, name: info.name, seqs: Some(vec![super::prefs::recorded(&self.keys, &seq)]) }
            }
            Some(Field::Leader | Field::InsertLeader) if seq.len() != 1 => {
                p.saved = Some(Err("a leader is one key".into()));
                return;
            }
            Some(Field::Leader) => Change::Leader(Some(seq[0].spec())),
            Some(Field::InsertLeader) => Change::InsertLeader(Some(seq[0].spec())),
            _ => return,
        };
        self.save_keys(&change);
    }

    /// End a recording once its pause is over (at the top of the loop).
    fn sync_recording(&mut self, now: std::time::Instant) {
        if let Some(r) = self.prefs.as_mut().and_then(|p| p.recording.as_mut()) {
            r.tick(now);
        }
    }

    /// When the recording's pause ends (None: no recording, or it is over).
    fn recording_due(&self) -> Option<std::time::Instant> {
        self.prefs.as_ref()?.recording.as_ref()?.due()
    }

    /// Change one entry of keys.toml (the rest of the file stays as the user wrote it) and use
    /// it; problems are shown under the preferences.
    fn save_keys(&mut self, change: &Change) {
        // The selection stays on the entry changed, wherever the new tree puts it.
        let anchor = self.prefs.as_ref().and_then(|p| p.anchor(&self.keys));
        let current = match super::keymap::config_path().map(std::fs::read_to_string) {
            Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            Some(Ok(text)) => Ok(text),
            _ => Ok(String::new()),
        };
        let edited = current.and_then(|text| super::keys_file::apply(&text, change).map_err(|e| format!("keys.toml has an error; fix it by hand: {e}")));
        let (map, warnings, written) = match edited {
            Ok(text) => {
                let (map, warnings) = Keymap::load(&text);
                let written = super::keymap::config_path().ok_or_else(|| "no config directory".to_string()).and_then(|path| super::keys_file::write(&path, &text).map(|_| path.display().to_string()));
                (map, warnings, written)
            }
            Err(e) => (Keymap::load("").0, Vec::new(), Err(e)),
        };
        if written.is_ok() {
            super::messages::set_open_key(map.first_shown(Context::Messages, Action::Open).unwrap_or_default());
            self.keys = map;
        }
        if let Some(p) = self.prefs.as_mut() {
            // Written: saved, with a note when the keymap warns (never "not saved").
            p.saved = Some(written.map(|path| super::keys_file::saved_note(&path, &warnings, change)));
            p.reselect(anchor, &self.keys);
        }
    }

    /// +/- in the preferences: step a level up or down, save it and apply it.
    fn adjust_preference(&mut self, up: bool) {
        let Some(p) = self.prefs.as_mut() else { return };
        let Some(field) = p.selected_field(&self.keys) else { return };
        if field == super::prefs::Field::Notifications {
            match super::prefs::Prefs::pause_step(&self.ws, up) {
                Some(Ok(command)) => self.run_text(&command),
                Some(Err(note)) => self.status = note,
                None => {}
            }
            return;
        }
        if matches!(field, super::prefs::Field::Schedule | super::prefs::Field::ScheduleHours | super::prefs::Field::ScheduleDay(_)) {
            p.saved = None;
            match p.schedule_change(&self.ws, field, Some(up)) {
                Some(Ok(edits)) => self.edit_schedule(edits),
                Some(Err(e)) => p.saved = Some(Err(e)),
                None => {}
            }
            return;
        }
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
        let action = self.action_in(Context::Switcher, k);
        let Some(p) = self.prefs.as_mut() else { return };
        let Some(picker) = p.picker.as_mut() else { return };
        match action {
            Some(Action::Close) => p.picker = None,
            Some(Action::Up) => picker.move_by(-1),
            Some(Action::Down) => picker.move_by(1),
            Some(Action::Erase) => picker.erase(),
            Some(Action::Open) => {
                self.keep_theme();
                return;
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

    /// Enter (or a second click) in the theme picker: keep the highlighted theme and save it.
    fn keep_theme(&mut self) {
        use super::theme_picker::COLOUR_THEME;
        let Some(p) = self.prefs.as_mut() else { return };
        let Some(picker) = p.picker.as_mut() else { return };
        let Some(name) = picker.highlighted().filter(|_| picker.problem.is_none()).map(String::from) else { return };
        let appearance = &mut self.ws.settings.appearance;
        match (picker.workspace, name.as_str()) {
            (true, COLOUR_THEME) => drop(appearance.workspace_themes.remove(&self.ws.team_id)),
            (true, _) => drop(appearance.workspace_themes.insert(self.ws.team_id.clone(), name)),
            (false, _) => appearance.theme = name,
        }
        p.picker = None;
        p.saved = Some(super::settings::save(&self.ws.settings).map(|path| path.display().to_string()));
        // Closed: the theme in force for this workspace.
        drop(super::theme::set(self.ws.settings.appearance.theme_for(&self.ws.team_id)));
    }

    /// A left click in the open preferences (false: not on anything here). A category switches to
    /// it; a row is selected, and a second click on it acts as Enter ([`Prefs::click`]); the
    /// theme picker's rows are selected (shown at once) and the highlighted one is kept. A click
    /// while recording does nothing; one off the field being typed in ends the typing, like Esc.
    fn click_prefs(&mut self, column: u16, row: u16) -> bool {
        let Some(p) = self.prefs.as_mut() else { return false };
        if let Some(picker) = p.picker.as_mut() {
            let Some(n) = picker.row_at(column, row) else { return false };
            if n == picker.selected {
                self.keep_theme();
                self.closing_click = Some((std::time::Instant::now(), column, row));
            } else {
                picker.selected = n;
                picker.preview(&self.ws.settings.appearance.theme);
            }
            return true;
        }
        if !p.area.contains((column, row).into()) {
            return false;
        }
        if p.recording.is_some() {
            return true;
        }
        let row_hit = p.row_at(column, row);
        if p.editing.is_some() {
            if row_hit != Some(p.selected) {
                (p.editing, p.editing_icon) = (None, false);
            }
            return true;
        }
        if let Some(n) = p.category_at(column, row) {
            if n != p.category {
                p.switch_category(n as isize - p.category as isize);
            }
        } else if let Some(n) = row_hit {
            if p.click(n) {
                self.change_preference();
            }
        }
        true
    }

    /// Enter in the preferences: change a local setting (and save the file) or send a
    /// Slack-side one (applied here at once; on failure the prefs are fetched again).
    fn change_preference(&mut self) {
        use super::prefs::SlackChange;
        let current = self.current.clone();
        let Some(p) = self.prefs.as_mut() else { return };
        (p.saved, p.clicked) = (None, None);
        let field = p.selected_field(&self.keys);
        // A group's name: written to keys.toml.
        if let Some(super::prefs::Field::GroupHeader(_)) = field {
            if let Some(change) = p.group_edit(&self.keys) {
                self.save_keys(&change);
            }
            return;
        }
        let on_slack = p.on_slack(&self.keys);
        let change = p.change(&mut self.ws, current.as_deref(), &self.keys);
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
            SlackChange::Schedule(edits) => self.edit_schedule(edits),
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

    /// Change the notification schedule: shown at once, then written after any earlier change
    /// still on its way (`ScheduleQueue`).
    fn edit_schedule(&mut self, edits: Vec<slack_core::dnd::ScheduleEdit>) {
        let before = self.ws.dnd.schedule.clone();
        if let Some(s) = self.ws.dnd.schedule.as_ref().and_then(|s| s.edited(&edits).ok()) {
            self.ws.dnd.schedule = Some(s);
        }
        self.status = "changing the notification schedule…".into();
        if let Some(edits) = self.schedule_writes.push(edits, before.as_ref()) {
            self.write_schedule(edits);
        }
    }

    /// Edit Slack's copy of the schedule (read just before, so another client's change stays),
    /// read it back and ask `dnd.info` again — any schedule write brings back a quiet period that
    /// was ended early. After a failure the prefs are fetched again.
    fn write_schedule(&mut self, edits: Vec<slack_core::dnd::ScheduleEdit>) {
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let (error, prefs) = match client.edit_dnd_schedule(&edits).await {
                Ok(prefs) => (None, Some(prefs)),
                Err(e) => (Some(e.to_string()), client.user_prefs().await.inspect_err(|e| tracing::warn!("users.prefs.get failed: {e}")).ok()),
            };
            let info = client.call_raw("dnd.info", &[]).await.inspect_err(|e| tracing::warn!("dnd.info failed: {e}")).ok();
            let _ = tx.send(Net::DndSchedule { error, prefs, info });
        });
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

    /// ␣cM: mute the current conversation, or unmute it. Here at once (the sidebar dims it and the
    /// unread count leaves it out), then on Slack; a failure puts it back (`Net::MuteFailed`).
    fn toggle_conversation_mute(&mut self) {
        let Some(channel) = self.current.clone() else { return };
        if self.ws.is_outside(&channel) {
            self.status = "join the channel to mute it".into();
            return;
        }
        let muted = self.ws.toggle_muted(&channel);
        self.sidebar.rebuild(&self.ws);
        self.status = format!("{} {}", if muted { "muted" } else { "unmuted" }, self.ws.channel_label(&channel));
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            if let Err(e) = client.set_notification_pref("muted", &muted.to_string(), Some(&channel)).await {
                let _ = tx.send(Net::MuteFailed { channel, muted, error: e.to_string() });
            }
        });
    }

    /// ␣cy: copy the current conversation's link, as Slack shows it.
    fn copy_conversation_link(&mut self) {
        let Some(channel) = self.current.clone() else { return };
        let link = conversation_link(&self.client.session.url, &channel);
        let how = self.clipboard.copy(&link);
        self.status = if how == "clipboard" { "link copied".into() } else { format!("link copied to the {how}") };
    }

    /// ␣cu: mark the current conversation unread from `selected` (with none: from the newest), as
    /// Slack's "Mark unread" does. Unread here at once, the "new" line above that message, and
    /// klack does not mark it read again until the user switches away and back
    /// (`Workspace::held_unread`); then on Slack. `in_thread`: a thread's replies are not marked.
    fn mark_unread(&mut self, selected: Option<String>, in_thread: bool) {
        if in_thread {
            self.status = "mark unread works on the conversation's messages".into();
            return;
        }
        let Some(channel) = self.current.clone() else { return };
        if self.ws.is_outside(&channel) {
            self.status = "join the channel to mark it unread".into();
            return;
        }
        let order: Vec<String> = self.ws.timelines.get(&channel).map(|t| super::messages::shown_messages(t, false).map(|m| m.ts.clone()).collect()).unwrap_or_default();
        let Some(ts) = mark_unread_ts(&order, selected.as_deref()) else {
            self.status = "no messages to mark unread".into();
            return;
        };
        hold_unread(&mut self.ws, &mut self.marked, &channel, &ts);
        self.messages.new_since = Some(ts.clone());
        self.messages.jump_to_new = false;
        self.sidebar.rebuild(&self.ws);
        self.status = format!("marked {} unread", self.ws.channel_label(&channel));
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            if let Err(e) = client.mark(&channel, &ts).await {
                let _ = tx.send(Net::MarkUnreadFailed { channel, error: e.to_string() });
            }
        });
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

    /// Load the Activity view afresh on `tab` (None: its All tab, whose unread count is the sidebar row's). Filtered
    /// as it was (the filter is kept for the run).
    fn open_activity(&mut self, tab: Option<super::activity::Tab>) {
        let (tab, unread_only) = (tab.unwrap_or_default(), self.activity.unread_only);
        self.activity = super::activity::Activity::default();
        (self.activity.tab, self.activity.unread_only) = (tab, unread_only);
        self.fetch_activity(tab, None);
    }

    /// Show tab `tab` of the Activity view (its list as it was, while the view is open) and load it again.
    fn show_activity_tab(&mut self, tab: super::activity::Tab) {
        self.activity.set_tab(tab);
        self.fetch_activity(tab, None);
    }

    /// Ask for tab `tab` of the activity feed: its first page, or the one at `cursor`.
    fn fetch_activity(&self, tab: super::activity::Tab, cursor: Option<String>) {
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let result = client.activity_feed_after(tab.types(), 50, cursor.as_deref()).await.map_err(|e| e.to_string());
            let _ = tx.send(Net::ActivityFeed { tab, more: cursor.is_some(), result });
        });
    }

    /// The feed's first page for the sidebar's Activity row (Slack's counts had no badge).
    fn fetch_activity_unread(&self) {
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let result = client.activity_feed(super::activity::ALL_TYPES, 50).await.map_err(|e| e.to_string());
            let _ = tx.send(Net::ActivityUnread(result));
        });
    }

    /// Slack's Activity badge asked for again (`client.counts`, as its client does at each connect).
    fn fetch_activity_badge(&self) {
        let (client, tx, since) = (self.client.clone(), self.net.clone(), self.ws.activity_seq);
        tokio::spawn(async move {
            let result = client.counts().await.map_err(|e| e.to_string());
            let _ = tx.send(Net::ActivityBadge { since, result });
        });
    }

    /// The Activity view's next page, once the end of its list is on screen.
    fn request_activity_page(&mut self) {
        if self.view != View::Activity {
            return;
        }
        if let Some(cursor) = self.activity.next_page() {
            self.fetch_activity(self.activity.tab, Some(cursor));
        }
    }

    /// Load the Threads view's first page afresh (or, with `more`, the next).
    fn load_threads(&mut self, more: bool) {
        let before = if more {
            let Some(ts) = self.threads.next_page() else { return };
            self.threads.loading = true;
            Some(ts)
        } else {
            self.threads = Default::default();
            self.threads.loading = true;
            None
        };
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let result = client.thread_view(before.as_deref(), 50).await.map_err(|e| e.to_string());
            let _ = tx.send(Net::ThreadView { result, more });
        });
    }

    /// The mouse wheel over an open list (Ctrl+K, the section mover, F5, search) moves
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
            } else if p.editing.is_none() && p.recording.is_none() {
                p.move_by(3 * step, &self.keys);
            }
        } else if let Some(sw) = self.switcher.as_mut() {
            let len = sw.entries(&self.ws).len();
            sw.move_by(step, len);
        } else if let Some(m) = self.mover.as_mut().filter(|m| m.naming.is_none()) {
            m.move_by(step);
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

    /// Fetch the messages of the activity items around the selection (while the view is shown).
    fn request_activity_messages(&mut self) {
        if self.view != View::Activity {
            return;
        }
        let a = &mut self.activity;
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

    /// The preferences record keys to bind: every key goes to them.
    fn capturing_key(&self) -> bool {
        self.prefs.as_ref().is_some_and(|p| p.recording.is_some())
    }

    /// Where input goes now (see [`mode_of`]).
    fn mode(&self) -> Mode {
        mode_of(self.overlay(), self.focus)
    }

    /// Forget the hovered row of the sidebar, the message panes and the views.
    fn clear_main_hover(&mut self) {
        self.sidebar.hover = None;
        self.messages.hover = None;
        self.thread_pane.hover = None;
        self.threads.hover = None;
        self.activity.hover = None;
    }

    /// Forget the row the pointer was over in every dialog's list.
    fn clear_dialog_hover(&mut self) {
        if let Some((p, _, _)) = self.picker.as_mut() {
            p.hover = None;
        }
        if let Some(p) = self.prefs.as_mut() {
            (p.hover, p.hover_category) = (None, None);
            if let Some(t) = p.picker.as_mut() {
                t.hover = None;
            }
        }
        self.switcher.iter_mut().for_each(|d| d.hover = None);
        self.mover.iter_mut().for_each(|d| d.hover = None);
        self.save.iter_mut().for_each(|d| d.hover = None);
        self.search.iter_mut().for_each(|d| d.hover = None);
        if let Some(d) = self.details.as_mut() {
            (d.hover, d.hover_tab) = (None, None);
        }
    }

    /// The pointer moved with a dialog open: shade the row of the topmost list under it (the
    /// selection stays), none when it is off the list.
    fn hover_dialog(&mut self, column: u16, row: u16) {
        self.clear_main_hover();
        self.clear_dialog_hover();
        if let Some(p) = self.prefs.as_mut() {
            match p.picker.as_mut() {
                Some(t) => t.hover_at(column, row),
                None => p.hover_at(column, row),
            }
        } else if let Some(d) = self.switcher.as_mut() {
            d.hover_at(column, row);
        } else if let Some(d) = self.mover.as_mut() {
            d.hover_at(column, row);
        } else if let Some(d) = self.save.as_mut() {
            d.hover_at(column, row);
        } else if let Some(d) = self.search.as_mut() {
            d.hover_at(column, row);
        } else if let Some(d) = self.details.as_mut().filter(|_| self.profile.is_none()) {
            d.hover_at(column, row);
        } else if let Some((p, _, _)) = self.picker.as_mut() {
            let len = p.candidates(&self.ws.custom_emoji).len();
            p.hover_at(column, row);
            p.hover = p.hover.filter(|n| *n < len);
        }
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
            if p.recording.is_some() {
                p.recording = None;
            } else if p.filter_focused {
                p.filter_focused = false;
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
                // The main pane: the view shown there, else the conversation's messages.
                Focus::Messages => self.view_context().unwrap_or(Context::Messages),
                Focus::Thread => Context::Messages,
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

    /// Drop the status bar's message (a command key was pressed), unless work it names goes on.
    /// The ringing invitation is not a note.
    fn clear_note(&mut self) {
        if !note_clears(&self.status) {
            return;
        }
        self.status.clear();
        self.status_shown.0.clear();
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
        // Backspace goes back a key, unless the sequence binds it (`^␣ Backspace` clears the box).
        let bound = k.code == crossterm::event::KeyCode::Backspace && k.modifiers.is_empty() && !super::which_key::backspace_goes_back(&self.keys, &self.active_contexts(), mode, self.pending.keys());
        if !bound && super::which_key::handle_key(&mut self.pending, self.which_key.as_mut(), typed) {
            self.clear_note();
            self.pending_since = Some(std::time::Instant::now());
            return true;
        }
        let (resolved, key) = self.keys.press(&self.active_contexts(), mode, self.pending.keys(), typed);
        if clears_note(&resolved, !self.pending.keys().is_empty()) {
            self.clear_note();
        }
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
        let signed_out = self.signed_out.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match switch_check(index, self.workspace, &self.workspaces, self.call.is_some(), &signed_out) {
            Ok(i) => self.switch_to = Some(i),
            Err(why) => self.status = why,
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
                self.open_hit(super::search::Hit { channel, ts: ts.clone(), thread_ts: Some(ts), user: None, text: String::new(), ..Default::default() });
                self.focus = Focus::ThreadComposer;
            }
            None => self.status = "the huddle has no thread yet".into(),
        }
    }

    /// The profile card's huddle: one in the DM with `user`.
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

    /// A left click in the open details (false: not on a tab or row). A tab switches to it; a
    /// member or pin row is selected and acted on as Enter does. A row's click is remembered: the
    /// second click of a double-click would otherwise close the profile card it opened (the card
    /// doesn't cover the member names) or act under the pin's jump.
    fn click_details(&mut self, column: u16, row: u16) -> bool {
        let Some(d) = self.details.as_mut() else { return false };
        if let Some(n) = d.tab_at(column, row) {
            d.switch_tab(n as isize - d.tab_index() as isize);
            return true;
        }
        let Some(n) = d.row_at(column, row) else { return false };
        d.selected = n;
        self.details_enter();
        self.closing_click = Some((std::time::Instant::now(), column, row));
        true
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
                    self.open_hit(super::search::Hit { channel: d.channel.clone(), ts: m.ts.clone(), thread_ts, user: m.user.clone(), text: m.text.clone(), ..Default::default() });
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
        let here = super::applink::Here { ws: &self.ws, workspaces: &self.addresses, names: &self.workspaces, shown: self.workspace, authors: &self.authors, message: None };
        let Some(m) = self.current.as_deref().and_then(|c| self.ws.find_message(c, ts)) else { return };
        let things = super::open::things(m, Some(&here));
        let here = here.of(m);
        let opened = match things.as_slice() {
            [] => None,
            [one] => Some(Err(one.clone())),
            _ => {
                let labels = things.iter().map(|t| super::open::label(t, m, &here)).collect();
                let at = if thread { self.thread_pane.spot(ts) } else { self.messages.spot(ts) };
                Some(Ok(super::menu::Menu::opener(thread, ts.to_string(), things, labels, at)))
            }
        };
        match opened {
            None => self.status = "nothing to open in this message".into(),
            Some(Err(one)) => self.open_thing(ts, one),
            Some(Ok(menu)) => self.menu = Some(menu),
        }
    }

    /// Open one thing of message `ts`: a link in the browser, a picture in the viewer (the
    /// others a step away), a video in the player, a snippet in full.
    fn open_thing(&mut self, ts: &str, thing: super::open::Thing) {
        use super::open::Thing;
        match thing {
            Thing::Link { app: Some(link), .. } => self.open_app_link(&link),
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

    /// Ask for the authors of linked messages that are drawn now without one (this workspace's links only), a few at a
    /// time.
    fn request_authors(&mut self) {
        let shown = self.workspace;
        let wanted: Vec<_> = self
            .messages
            .authors_wanted()
            .chain(self.thread_pane.authors_wanted())
            .chain(self.threads.authors_wanted())
            .chain(self.activity.authors_wanted())
            .filter(|l| super::applink::workspace_of(l, &self.addresses, shown) == Some(shown))
            .filter_map(|l| match &l.target {
                super::applink::Target::Message { channel, ts, thread_ts } => Some((channel.clone(), ts.clone(), thread_ts.clone())),
                _ => None,
            })
            .collect();
        for (channel, ts, thread_ts) in &wanted {
            self.authors.want(channel, ts, thread_ts.as_deref());
        }
        for (channel, ts, _) in self.authors.next_requests(&wanted, std::time::Instant::now()) {
            tracing::debug!("link author: conversations.replies for {channel} {ts}");
            let (client, tx) = (self.client.clone(), self.net.clone());
            tokio::spawn(async move {
                let result = match client.message_at(&channel, &ts).await {
                    Err(e) if slack_core::api::is_unanswered(&e) => Err(format!("{e:#}")),
                    other => Ok(other.ok().flatten()),
                };
                let _ = tx.send(Net::LinkAuthor { channel, ts, result });
            });
        }
    }

    /// Start loading thumbnails that were on screen but not cached yet.
    fn request_thumbs(&mut self) {
        let view = self.huddle_view.as_mut().map(|v| std::mem::take(&mut v.wanted)).unwrap_or_default();
        let wanted: Vec<_> = self.messages.wanted.drain(..).chain(self.thread_pane.wanted.drain(..)).chain(self.threads.wanted.drain(..)).chain(self.activity.pictures.drain(..)).chain(view).chain(self.call_bar_wanted.take()).chain(self.rail_wanted.drain(..)).collect();
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
        pane_order(self.huddle_view.is_some(), self.narrow, self.thread.is_some())
    }

    /// A dialog or popup is open over the panes (the composer's cursor must not show through).
    fn covered(&self) -> bool {
        self.menu.is_some()
            || self.help.is_some()
            || self.prefs.is_some()
            || self.switcher.is_some()
            || self.mover.is_some()
            || self.save.is_some()
            || self.profile.is_some()
            || self.details.is_some()
            || self.search.is_some()
            || self.player.is_some()
            || self.viewer.is_some()
            || self.snippet.is_some()
            || self.confirm.is_some()
            || self.picker.is_some()
    }

    /// Whether `action` of `context` makes sense now (see [`applies`]).
    fn applies(&self, context: Context, action: Action) -> bool {
        let leavable = |id: &str| self.ws.conversations.get(id).is_some_and(|c| !c.is_dm() && c.is_member);
        let selected = self.sidebar.selected_id();
        let here = Here {
            offers: self.selected_ts().and_then(|ts| self.current.as_deref().and_then(|c| self.ws.find_message(c, &ts))).map(|m| super::menu::Offers::of(m, &self.ws.self_id)),
            in_thread: self.focus == Focus::Thread,
            outside: self.current.as_deref().is_some_and(|c| self.ws.is_outside(c)),
            leavable: self.current.as_deref().is_some_and(leavable),
            selected_leavable: selected.is_some_and(leavable),
            selected: selected.is_some(),
            card: self.profile.is_some(),
            dialog: self.mode() == Mode::Dialog,
            workspaces: self.workspaces.len(),
        };
        applies(&here, context, action)
    }

    /// The next keys after `prefix` that which-key offers: what doesn't apply here is left
    /// out (edit on others' messages, join when a member…), and so are groups left empty.
    fn offered(&self, contexts: &[Context], mode: Mode, prefix: &[Key]) -> Vec<super::keymap::Child> {
        let mut children = self.keys.children(contexts, mode, prefix);
        children.retain(|c| match c.action {
            Some((context, action)) => self.applies(context, action),
            None => !self.offered(contexts, mode, &[prefix, &[c.key]].concat()).is_empty(),
        });
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        self.keys.follow_state(&mut children, self.ws.dnd_active(now));
        self.keys.name_workspaces(&mut children, &self.workspaces);
        children
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
/// People to look up again for their statuses.
#[derive(Default)]
struct Lookups {
    /// To look up now.
    due: std::collections::BTreeSet<String>,
    /// Their profile changed (Slack pushed it) while not shown: looked up once shown.
    stale: std::collections::HashSet<String>,
    /// A run is under way.
    busy: bool,
}

/// The most people one status refresh looks up.
const STATUS_REFRESH_MAX: usize = 50;
/// How often the people shown are checked for ones whose profile changes are not asked for yet.
const FOLLOW_EVERY: Duration = Duration::from_secs(2);

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

/// What the open conversation `channel` is marked read up to now: its newest message, unless
/// that is marked already (`marked`), the user marked it unread while here
/// (`Workspace::held_unread`), or it is a channel we are not in.
fn read_mark_target(ws: &super::state::Workspace, channel: &str, marked: &std::collections::HashMap<String, String>) -> Option<String> {
    if ws.is_outside(channel) || ws.held_unread.contains(channel) {
        return None;
    }
    let latest = ws.timelines.get(channel).and_then(|t| t.messages.last()).map(|m| m.ts.clone())?;
    (marked.get(channel) != Some(&latest)).then_some(latest)
}

/// ␣cu here: the conversation is unread after `ts` and held so (`Workspace::mark_unread`); what
/// was marked read before is forgotten, so opening it again marks it read even with nothing new.
fn hold_unread(ws: &mut super::state::Workspace, marked: &mut std::collections::HashMap<String, String>, channel: &str, ts: &str) {
    ws.mark_unread(channel, ts);
    marked.remove(channel);
}

/// A conversation's link as Slack shows it: `https://acme.slack.com/archives/C…`.
fn conversation_link(workspace_url: &str, channel: &str) -> String {
    format!("{}/archives/{channel}", workspace_url.trim_end_matches('/'))
}

/// Where Slack's read mark goes so that `selected` (with none: the newest) is the first unread
/// message of `order`: the message before it, or a microsecond before it when it is the first
/// one loaded.
fn mark_unread_ts(order: &[String], selected: Option<&str>) -> Option<String> {
    let at = match selected {
        Some(ts) => order.iter().position(|o| o == ts)?,
        None => order.len().checked_sub(1)?,
    };
    match at {
        0 => ts_just_before(&order[0]),
        i => Some(order[i - 1].clone()),
    }
}

/// The Slack timestamp one microsecond before `ts` (exact: no floats).
fn ts_just_before(ts: &str) -> Option<String> {
    let (secs, frac) = ts.split_once('.').unwrap_or((ts, ""));
    let micros: u64 = format!("{frac:0<6}").get(..6)?.parse().ok()?;
    let total = secs.parse::<u64>().ok()?.checked_mul(1_000_000)?.checked_add(micros)?.checked_sub(1)?;
    Some(format!("{}.{:06}", total / 1_000_000, total % 1_000_000))
}

/// The reply a thread is read up to once it is shown: its newest, when its replies are loaded and
/// it has any.
fn thread_mark_target(t: &super::state::Timeline, parent: &str) -> Option<String> {
    t.messages.last().map(|m| m.ts.clone()).filter(|ts| t.loaded && ts != parent)
}

/// How far ← / → jump in a video, in seconds.
const SEEK_STEP: f64 = 10.0;

/// The note when the message a link points to can't be found.
const LINK_GONE: &str = "that message isn't there any more, or you can't see it";

/// The note when a search's message is older than the conversation's history.
const HISTORY_GONE: &str = "that message is no longer in this conversation's history";

/// The note when the conversation a link points to can't be found.
const CONVERSATION_GONE: &str = "that conversation isn't there any more, or you can't see it";

/// The note while a seek pages back (`{}`: how many messages are loaded).
const SEEKING: &str = "looking for the message… ({} loaded)";

/// The note when Esc stops a seek.
const SEEK_STOPPED: &str = "stopped looking for the message";

/// The note when a file link's file is not one klack shows.
const FILE_NOT_SHOWN: &str = "klack can't show that file here";

const CHOOSE_LIVE: &str = "choose a live huddle to join";
const RECENT_FAILED: &str = "recent huddles failed";

/// A huddle whose connection could not be brought back (the reconnect tries used up).
const CONNECTION_LOST_LEFT: &str = "huddle connection lost — left the huddle";

/// The Huddles view's and a huddle's status notes, for `ctl locale-template`.
pub const HUDDLE_NOTES: &[&str] = &[CHOOSE_LIVE, RECENT_FAILED, CONNECTION_LOST_LEFT];

/// `rooms.leave` with `params`, in the background (a failure is benign).
fn spawn_leave(client: slack_core::api::Client, params: Vec<(&'static str, String)>) {
    tokio::spawn(async move {
        if let Err(e) = client.call_raw("rooms.leave", &params).await {
            tracing::info!("rooms.leave: {e}");
        }
    });
}

/// The status notes of links into klack, for `ctl locale-template`.
pub const LINK_NOTES: &[&str] = &[LINK_GONE, HISTORY_GONE, CONVERSATION_GONE, SEEKING, SEEK_STOPPED, FILE_NOT_SHOWN, super::applink::OPENS_IN_KLACK];

/// A history request for a conversation (`tl`) failed: it may be asked again (no longer loading). The note to show:
/// a link's seek in it (`seeking`) ends with its not-found note, anything else says what failed.
fn history_failed(tl: Option<&mut super::state::Timeline>, seeking: bool, from_link: bool, error: &str) -> String {
    if let Some(tl) = tl {
        tl.loading = false;
    }
    if seeking && from_link { super::i18n::tr(LINK_GONE).into_owned() } else { format!("history failed: {error}") }
}

/// What a seek does once the history has come: stop there (the message is loaded, or a search's is newer than the
/// oldest loaded), page further back, or stop and say the message is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seek {
    Done,
    More,
    /// Fetch the newest page again first (what is loaded may be from a while ago).
    Refresh,
    Missing(&'static str),
}

/// What is loaded of a conversation, for the message sought: it is `found`, `older` than all of it (`has_more` to
/// page) or `newer` than all of it (or nothing is loaded).
#[derive(Debug, Clone, Copy)]
struct Sought {
    found: bool,
    older: bool,
    newer: bool,
    has_more: bool,
}

/// Whether a hit's message is selected (and its thread opened) now, `seek` what its seek did first (None: waiting
/// for the history): a search's at once, as always; a link's only once it is found.
fn select_now(from_link: bool, seek: Option<Seek>) -> bool {
    !from_link || seek == Some(Seek::Done)
}

/// The stop rule of a seek, from what is loaded; `from_link`: a link started it (a search's hit newer than the
/// loaded history is left alone); `refreshed`: the newest page was fetched again since it started.
fn seek_step(sought: Sought, from_link: bool, refreshed: bool) -> Seek {
    let Sought { found, older, newer, has_more } = sought;
    match (found, older, has_more) {
        (true, _, _) => Seek::Done,
        (false, false, _) if from_link && newer && !refreshed => Seek::Refresh,
        (false, false, _) if from_link => Seek::Missing(LINK_GONE),
        (false, false, _) => Seek::Done,
        (false, true, true) => Seek::More,
        (false, true, false) if from_link => Seek::Missing(LINK_GONE),
        (false, true, false) => Seek::Missing(HISTORY_GONE),
    }
}

/// The search hit that opens a message target (None: not a message).
fn hit_of(target: super::applink::Target) -> Option<super::search::Hit> {
    match target {
        super::applink::Target::Message { channel, ts, thread_ts } => Some(super::search::Hit { channel, ts, thread_ts, user: None, text: String::new(), ..Default::default() }),
        _ => None,
    }
}

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
        if let Some((c, d)) = self.composer_key().zip(self.composer.draft()) {
            drafts.insert(c, d);
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

/// The workspaces (indices into the profile's) whose sessions have ended, shared with the pollers.
pub type SignedOut = std::sync::Arc<std::sync::Mutex<std::collections::HashSet<usize>>>;

/// The workspace to start on: `wanted` if it is signed in, else the first that is; None when
/// every one is signed out.
fn start_index(sessions: &[slack_core::session::WorkspaceSession], wanted: usize, signed_out: &std::collections::HashSet<usize>) -> Option<usize> {
    std::iter::once(wanted).chain(0..sessions.len()).find(|i| *i < sessions.len() && !signed_out.contains(i))
}

/// What the status line says about workspace `label` once its session has ended.
fn signed_out_note(label: &str) -> String {
    format!("{label}: signed out — klack auth login, then restart klack")
}

/// Whether `cmd` can run in a workspace whose session has ended: only klack's own that need no
/// Slack (switching workspaces, the preferences).
fn runs_while_signed_out(cmd: &super::commands::Command) -> bool {
    use super::commands::Command;
    matches!(cmd, Command::Workspace(_) | Command::Prefs)
}

/// The status line's note: the ringing invitation, else the latest note — followed by the
/// signed-out note (`sticky`), which routine notes do not replace.
fn shown_note(ringing: Option<String>, sticky: Option<&str>, status: &str) -> Option<String> {
    if ringing.is_some() {
        return ringing;
    }
    match (status, sticky) {
        ("", None) => None,
        ("", Some(sticky)) => Some(sticky.to_string()),
        (status, Some(sticky)) if status != sticky => Some(format!("{status} · {sticky}")),
        (status, _) => Some(status.to_string()),
    }
}

/// Whether workspace `index` can be switched to from `current` (`labels` name them; `in_call`: in
/// a huddle): the index, or what the status line says instead.
fn switch_check(index: usize, current: usize, labels: &[String], in_call: bool, signed_out: &std::collections::HashSet<usize>) -> Result<usize, String> {
    if index >= labels.len() {
        Err(format!("there is no workspace {} (this profile has {})", index + 1, labels.len()))
    } else if index == current {
        Err(format!("already in {}", labels[index]))
    } else if signed_out.contains(&index) {
        Err(signed_out_note(&labels[index]))
    } else if in_call {
        Err("leave the huddle first".into())
    } else {
        Ok(index)
    }
}

/// The index of `account` (team id, user id) in `profile`; without one, or when it is no longer
/// there (removed at the start question), the default workspace's, else the first.
fn wanted_index(profile: &slack_core::session::Profile, account: Option<(&str, &str)>) -> usize {
    let at = |team: &str, user: &str| profile.workspaces.iter().position(|s| s.team_id == team && s.user_id == user);
    account
        .and_then(|(team, user)| at(team, user))
        .or_else(|| profile.workspace(None).and_then(|w| at(&w.team_id, &w.user_id)))
        .unwrap_or(0)
}

/// Run the app on the profile's `account` (team id, user id; or its default workspace; the first
/// signed-in one when that one is in `signed_out`); Alt+1…9 and `/workspace` switch to another of
/// the profile's workspaces without leaving the terminal. The signed-out ones are skipped.
pub async fn run(profile: slack_core::session::Profile, account: Option<(&str, &str)>, signed_out: std::collections::HashSet<usize>) -> Result<()> {
    let sessions = profile.workspaces.clone();
    let wanted = wanted_index(&profile, account);
    let mut index = start_index(&sessions, wanted, &signed_out).ok_or_else(|| anyhow::anyhow!("every workspace of this profile is signed out — klack auth login"))?;
    let signed_out: SignedOut = std::sync::Arc::new(std::sync::Mutex::new(signed_out));
    let labels = workspace_labels(&sessions);
    let addresses: Vec<(String, String)> = sessions.iter().map(|s| (s.url.clone(), s.team_id.clone())).collect();
    let keymap = super::keymap::load_user();
    let shown = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(index));
    let badges: super::others::Badges = Default::default();
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    super::others::spawn(sessions.clone(), shown.clone(), badges.clone(), wake.clone(), signed_out.clone());
    let (clicks_tx, mut clicks) = mpsc::unbounded_channel();
    super::others::spawn_watchers(sessions.clone(), labels.clone(), shown.clone(), clicks_tx, signed_out.clone());
    let open_after: super::others::OpenAfter = Default::default();
    let logos: super::others::Logos = Default::default();
    super::others::spawn_logos(sessions.clone(), logos.clone(), signed_out.clone());

    for w in &keymap.1 {
        tracing::warn!("{w}");
    }
    tokio::task::spawn_blocking(super::highlight::warm_up);
    let (_guard, mut terminal) = TerminalGuard::enter()?;
    // Query the terminal before the event stream starts reading stdin.
    let mut media = super::media::Media::detect(&super::settings::load().0.messages.graphics);
    let mut clipboard = super::clipboard::Clipboard::default();
    let mut events = EventStream::new();
    // Each workspace's back and forward, kept while another is shown.
    let mut histories: std::collections::HashMap<usize, super::history::History> = std::collections::HashMap::new();
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
        // A graphics mode this terminal cannot draw: said once, at the first workspace.
        keymap.1.extend(media.note.take());
        super::messages::set_open_key(keymap.0.first_shown(Context::Messages, Action::Open).unwrap_or_default());
        let (next, kept_media, kept_clipboard) = run_workspace(client, (labels.clone(), index, badges.clone(), open_after.clone(), logos.clone(), signed_out.clone(), addresses.clone()), &mut clicks, &keymap, &settings, &mut terminal, &mut events, media, clipboard, histories.entry(index).or_default()).await?;
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

impl App {
    /// An app on `client`'s workspace with nothing loaded or open yet, no keys bound and nothing
    /// kept from earlier runs (the caller fills those in); requests answer on `net`.
    fn new(client: Client, net: mpsc::UnboundedSender<Net>, rtm_commands: mpsc::UnboundedSender<slack_core::rtm::RtmCommand>, media: super::media::Media, clipboard: super::clipboard::Clipboard) -> App {
        App {
            keys: Keymap::default(),
            key_warnings: Vec::new(),
            pending: Pending::default(),
            pending_since: None,
            which_key: None,
            which_key_area: Rect::default(),
            closing_click: None,
            workspaces: Vec::new(),
            addresses: Vec::new(),
            authors: Default::default(),
            workspace: 0,
            others: Default::default(),
            rail: Vec::new(),
            open_after: Default::default(),
            start: None,
            logos: Default::default(),
            rail_wanted: Vec::new(),
            switch_to: None,
            signed_out: Default::default(),
            signed_out_note: None,
            media,
            viewer: None,
            snippet: None,
            help: None,
            switcher: None,
            search: None,
            details: None,
            profile: None,
            view: View::Conversation,
            activity: Default::default(),
            save: None,
            threads: Default::default(),
            huddles: Default::default(),
            feed_reply: None,
            feed_typing: false,
            preview_polled: None,
            statuses_polled: None,
            lookups: Lookups::default(),
            socket: None,
            rtm_down_since: None,
            people_requeued: None,
            status_turn: 0,
            prefs: None,
            huddle_view: None,
            parked_huddle: None,
            call_bar_area: Default::default(),
            call_bar_wanted: None,
            menu: None,
            dnd_refreshing: false,
            schedule_writes: ScheduleQueue::default(),
            mover: None,
            huddle_after_open: None,
            rejoined: false,
            huddle_beat: None,
            autosave: (None, None, std::time::Instant::now(), true),
            presence_subscribed: false,
            followed: Default::default(),
            follow_passes: 0,
            call: None,
            pending_hit: None,
            ringing: None,
            seeking: None,
            seek_from_link: false,
            seek_hit: None,
            seek_refreshed: false,
            link_reply: None,
            drafts: Default::default(),
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
            rtm_commands,
            connected: false,
            last_typing_sent: None,
            marked: Default::default(),
            focus: Focus::Messages,
            composer: Composer::default(),
            client,
            net,
            ws: Workspace::default(),
            sidebar: Sidebar::default(),
            messages: MessagePane::default(),
            current: None,
            status: "loading workspace…".into(),
            link_note: None,
            pointer: None,
            status_shown: (String::new(), std::time::Instant::now()),
            loading: true,
            history: Default::default(),
        }
    }
}

/// One workspace until quit (None) or a switch (Some(index)); hands back what outlives it.
async fn run_workspace(
    client: Client,
    workspaces: (Vec<String>, usize, super::others::Badges, super::others::OpenAfter, super::others::Logos, SignedOut, Vec<(String, String)>),
    clicks: &mut mpsc::UnboundedReceiver<super::others::Click>,
    keymap: &(Keymap, Vec<String>),
    settings: &super::settings::Settings,
    terminal: &mut super::terminal::Tui,
    events: &mut EventStream,
    mut media: super::media::Media,
    clipboard: super::clipboard::Clipboard,
    history: &mut super::history::History,
) -> Result<(Option<usize>, super::media::Media, super::clipboard::Clipboard)> {
    let (net_tx, mut net_rx) = mpsc::unbounded_channel();
    spawn_boot(client.clone(), net_tx.clone());
    let mut rtm = slack_core::rtm::spawn(client.clone());
    media.thumbs.clear();
    let team_name = if workspaces.0.len() > 1 { format!("{}/{} {}", workspaces.1 + 1, workspaces.0.len(), workspaces.0[workspaces.1]) } else { client.session.team_name.clone() };
    let mut app = App::new(client.clone(), net_tx.clone(), rtm.commands.clone(), media, clipboard);
    (app.keys, app.key_warnings) = keymap.clone();
    (app.workspaces, app.workspace, app.others, app.open_after, app.logos, app.signed_out, app.addresses) = workspaces;
    app.drafts = super::drafts::load(&client.session.team_id, &client.session.user_id);
    app.ws = Workspace { team_name, team_id: client.session.team_id.clone(), self_id: client.session.user_id.clone(), settings: settings.clone(), ..Default::default() };
    app.sidebar = Sidebar::for_account(&client.session.team_id, &client.session.user_id);
    app.history = std::mem::take(history);

    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut video_tick = tokio::time::interval(Duration::from_millis(40));
    // Redraws the call bar's level meters.
    let mut call_tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        app.expire_status(std::time::Instant::now());
        app.follow_profiles(std::time::Instant::now());
        app.ring_tick();
        let in_call = app.call.as_ref().is_some_and(|c| c.connected && !c.leaving);
        app.beat_huddle(in_call, std::time::Instant::now());
        app.sync_which_key(std::time::Instant::now());
        app.sync_recording(std::time::Instant::now());
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
        app.request_feed_replies();
        app.request_authors();
        app.request_activity_messages();
        app.request_activity_page();
        app.request_huddles();
        app.refresh_dnd();
        let which_key_due = app.which_key_due();
        let recording_due = app.recording_due();
        // You went somewhere (a key, a click, a notification): a step in the history.
        let mut went = false;
        tokio::select! {
            ev = events.next() => {
                let Some(ev) = ev else { break };
                went = true;
                if !handle_terminal(&mut app, ev?) {
                    break;
                }
                app.refresh_completion();
            }
            Some(msg) = net_rx.recv() => {
                went = goes_somewhere(&msg);
                handle_net(&mut app, msg);
            }
            Some((index, target)) = clicks.recv() => {
                went = true;
                if index == app.workspace {
                    app.open_target(target);
                } else {
                    *app.open_after.lock().unwrap_or_else(|e| e.into_inner()) = Some((index, target.into()));
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
                match ev {
                    huddle::session::SessionEvent::Reconnecting => app.status = "huddle connection lost — joining again…".into(),
                    huddle::session::SessionEvent::Connected if app.call.as_ref().is_some_and(|c| c.connected) => app.status = "back in the huddle".into(),
                    _ => {}
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
                    app.huddle_ended();
                }
            }
            _ = tick.tick() => {}
            // which-key opens (at the top of the loop).
            _ = tokio::time::sleep_until(which_key_due.unwrap_or_else(std::time::Instant::now).into()), if which_key_due.is_some() => {}
            // A recording in F6 → Keys ends (at the top of the loop).
            _ = tokio::time::sleep_until(recording_due.unwrap_or_else(std::time::Instant::now).into()), if recording_due.is_some() => {}
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
        // A call answered that the session has ended (one of the app's, or a poller's of this workspace).
        if app.signed_out_note.is_none() && (app.client.signed_out() || app.signed_out.lock().unwrap_or_else(|e| e.into_inner()).contains(&app.workspace)) {
            app.mark_signed_out();
        }
        app.sync_current();
        app.track_place(went);
        app.autosave_drafts();
        // Not while a view hides it: what comes meanwhile stays unread.
        if app.view == View::Conversation && app.current.as_ref().and_then(|c| app.ws.timelines.get(c)).is_some_and(|t| t.loaded) {
            app.mark_current_read();
        }
        app.mark_thread_read();
        app.feed_tick(std::time::Instant::now());
        app.refresh_preview();
        app.refresh_statuses();
        app.look_up_people();
    }
    // Quitting in a huddle: tell Slack we left (best effort, not holding the quit for long).
    if let Some(call) = app.call.as_mut().filter(|c| !c.leaving) {
        call.leave();
        let params = call.leave_request();
        let _ = tokio::time::timeout(Duration::from_secs(1), app.client.call_raw("rooms.leave", &params)).await;
    }
    app.save_drafts();
    let App { switch_to, media, clipboard, history: kept, .. } = app;
    *history = kept;
    Ok((switch_to, media, clipboard))
}

/// Whether an event, once handled, ends the key sequence under way: a key press that did not
/// continue it, a click, the wheel or a paste. A key release (Windows reports them), a mouse
/// move, focus and a resize leave it.
fn ends_sequence(ev: &Event) -> bool {
    use crossterm::event::MouseEventKind;
    match ev {
        Event::Key(k) => k.kind != KeyEventKind::Release,
        Event::Mouse(m) => !matches!(m.kind, MouseEventKind::Moved | MouseEventKind::Drag(_)),
        Event::Paste(_) => true,
        Event::FocusGained | Event::FocusLost | Event::Resize(..) => false,
    }
}

/// A second left click at the same cell within this is the second half of a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Whether a mouse event is taken before anything acts on it: a click on the which-key panel
/// (`which_key`, while shown), or the second half of a double-click whose first click closed a
/// dialog (`closing_click`: when and where), so it does not act on what was under the dialog.
fn swallowed(ev: &Event, which_key: Option<Rect>, closing_click: Option<(std::time::Instant, u16, u16)>, now: std::time::Instant) -> bool {
    let Event::Mouse(m) = ev else { return false };
    let (column, row) = (m.column, m.row);
    match m.kind {
        MouseEventKind::Down(_) | MouseEventKind::Up(_) if which_key.is_some_and(|a| a.contains((column, row).into())) => true,
        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => closing_click.is_some_and(|(at, c, r)| (c, r) == (column, row) && now.duration_since(at) < DOUBLE_CLICK),
        _ => false,
    }
}

/// Whether a reply goes somewhere you asked to go (a notification clicked, a link's conversation looked up, a DM
/// opened, a channel joined or created): a step in the history, as a key or a click is.
fn goes_somewhere(msg: &Net) -> bool {
    matches!(msg, Net::NotificationClicked(_) | Net::LinkConversation { .. } | Net::DmOpened(_) | Net::Joined { .. })
}

/// Whether a place in the history is no longer there: a conversation unknown now, archived, or left (a DM is
/// never left; a channel previewed stays one to preview).
fn place_gone(ws: &Workspace, place: &Place) -> bool {
    let Place::Conversation { channel, member, .. } = place else { return false };
    ws.conversations.get(channel).is_none_or(|c| c.is_archived || *member && ws.is_outside(channel))
}

/// Returns false to quit.
fn handle_terminal(app: &mut App, ev: Event) -> bool {
    if let Event::Key(k) = &ev {
        if k.kind != KeyEventKind::Release && app.hold_sequence(k) {
            return true;
        }
    }
    let ends = ends_sequence(&ev);
    let which_key = app.which_key.is_some().then_some(app.which_key_area);
    let keep_going = swallowed(&ev, which_key, app.closing_click, std::time::Instant::now()) || handle_event(app, ev);
    if ends {
        app.pending.clear();
    }
    keep_going
}

fn handle_event(app: &mut App, ev: Event) -> bool {
    match &ev {
        Event::Mouse(m) => app.pointer = Some((m.column, m.row)),
        Event::FocusLost => app.pointer = None,
        _ => {}
    }
    if let Event::FocusGained | Event::FocusLost = ev {
        app.terminal_focused = matches!(ev, Event::FocusGained);
        if !app.terminal_focused {
            app.clear_main_hover();
            app.clear_dialog_hover();
        }
        tracing::debug!(focused = app.terminal_focused, "terminal focus");
        return true;
    }
    if let Event::Key(k) = &ev {
        if k.kind != KeyEventKind::Release && !app.capturing_key() && app.resolve(k) == Resolved::Action(Context::Global, Action::Quit) {
            return false;
        }
    }
    let keep_going = match app.mode() {
        Mode::Dialog => handle_dialog(app, &ev),
        Mode::Normal | Mode::Insert => handle_main(app, ev),
    };
    app.sync_feed_composer();
    keep_going
}

/// Input while a dialog is open: the topmost dialog takes every key and click; nothing under
/// it reacts.
fn handle_dialog(app: &mut App, ev: &Event) -> bool {
    // A key can change what is under the pointer (a move, a filter): its hover waits for the next mouse move.
    if let Event::Key(k) = ev {
        if k.kind != KeyEventKind::Release {
            app.clear_dialog_hover();
        }
    }
    // The global leader sequences (Ctrl+Space h m mutes with a dialog open).
    if let Event::Key(k) = ev {
        if k.kind != KeyEventKind::Release && !app.capturing_key() {
            match app.resolve(k) {
                Resolved::Action(Context::Global, action) if opens_something(action) => {
                    app.status = close_first(&app.keys, app.overlay());
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
                // An item's own key runs it (`r` for the `␣r` shown beside it).
                _ => {
                    if let Some(n) = m.item_for_key(Key::from_event(k)).and_then(|item| m.items.iter().position(|i| i.0 == item)) {
                        m.selected = n;
                        app.run_menu();
                    }
                }
            }
        }
        // The reaction picker: a click on an emoji reacts with it, a click outside closes it.
        Event::Mouse(m) if app.picker.is_some() && matches!(m.kind, MouseEventKind::Down(_)) => {
            let Some((picker, _, _)) = app.picker.as_mut() else { return true };
            let len = picker.candidates(&app.ws.custom_emoji).len();
            match app.picker.as_ref().map(|(p, _, _)| (p.row_at(m.column, m.row).filter(|n| *n < len), p.area.contains((m.column, m.row).into()))) {
                Some((Some(n), _)) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
                    app.closing_click = Some((std::time::Instant::now(), m.column, m.row));
                    if let Some((picker, channel, ts)) = app.picker.take() {
                        if let Some(choice) = picker.candidates(&app.ws.custom_emoji).get(n) {
                            app.toggle_reaction(channel, ts, choice.name.clone());
                        }
                    }
                }
                Some((_, false)) => {
                    app.picker = None;
                    app.closing_click = Some((std::time::Instant::now(), m.column, m.row));
                }
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
                    app.closing_click = Some((std::time::Instant::now(), m.column, m.row));
                }
                (MouseEventKind::Down(_), None) => {
                    app.menu = None;
                    app.closing_click = Some((std::time::Instant::now(), m.column, m.row));
                }
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
        Event::Key(k) if k.kind != KeyEventKind::Release && app.prefs.as_ref().is_some_and(|p| p.recording.is_some()) => app.record_key(Key::from_event(&k)),
        Event::Key(k) if k.kind != KeyEventKind::Release && app.prefs.as_ref().is_some_and(|p| p.picker.is_some()) => app.theme_picker_key(&k),
        Event::Key(k) if k.kind != KeyEventKind::Release && app.prefs.as_ref().is_some_and(|p| p.editing.is_some()) => {
            let action = app.action_in(Context::TextField, &k);
            let Some(p) = app.prefs.as_mut() else { return true };
            match (action, p.editing.as_mut()) {
                (Some(Action::Close), _) => (p.editing, p.editing_icon) = (None, false),
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
        // The key list's filter after `/`: every printable key types into it.
        Event::Key(k) if k.kind != KeyEventKind::Release && app.prefs.as_ref().is_some_and(|p| p.filter_focused) => {
            let action = app.action_in(Context::TextField, &k);
            if let Some(p) = app.prefs.as_mut() {
                p.filter_key(Key::from_event(&k), action);
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.prefs.is_some() => {
            use super::prefs::Field;
            let action = app.action_in(Context::Preferences, &k);
            let Some(p) = app.prefs.as_mut() else { return true };
            match action {
                // A filter kept after Enter: Esc clears it before it closes.
                Some(Action::Close) if p.keys_shown() && !p.filter.is_empty() => p.set_filter(String::new()),
                Some(Action::Filter) if p.keys_shown() => p.filter_focused = true,
                Some(Action::Close) => app.prefs = None,
                Some(Action::Up) => p.move_by(-1, &app.keys),
                Some(Action::Down) => p.move_by(1, &app.keys),
                Some(Action::PrevTab) => p.switch_category(-1),
                Some(Action::NextTab) => p.switch_category(1),
                Some(Action::Open) => app.change_preference(),
                Some(Action::Increase) => app.adjust_preference(true),
                Some(Action::Decrease) => app.adjust_preference(false),
                Some(Action::EditIcon) => p.edit_icon(&app.keys),
                Some(Action::Unbind) => match p.selected_field(&app.keys) {
                    Some(Field::Binding(i)) => {
                        let info = &super::keymap::ACTIONS[i];
                        app.save_keys(&Change::Binding { context: info.context, name: info.name, seqs: Some(vec![]) });
                    }
                    Some(Field::Leader) => app.save_keys(&Change::Leader(Some(String::new()))),
                    Some(Field::InsertLeader) => app.save_keys(&Change::InsertLeader(Some(String::new()))),
                    _ => {}
                },
                Some(Action::Reset) => match p.selected_field(&app.keys) {
                    Some(Field::Binding(i)) => {
                        let info = &super::keymap::ACTIONS[i];
                        app.save_keys(&Change::Binding { context: info.context, name: info.name, seqs: None });
                    }
                    Some(Field::Leader) => app.save_keys(&Change::Leader(None)),
                    Some(Field::InsertLeader) => app.save_keys(&Change::InsertLeader(None)),
                    _ => {}
                },
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
        Event::Mouse(m) if m.kind == MouseEventKind::Moved => app.hover_dialog(m.column, m.row),
        // The wheel moves the rows under the pointer: its hover waits for the next move.
        Event::Mouse(m) if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) => {
            app.clear_dialog_hover();
            app.wheel_in_popup(m.kind == MouseEventKind::ScrollDown);
        }
        // A click in the preferences, or on a row of jump to or search, as Enter does.
        Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) && app.prefs.is_some() && app.click_prefs(m.column, m.row) => {}
        Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) && app.prefs.is_none() && click_row(app, m.column, m.row) => {}
        Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) && app.profile.is_none() && app.click_details(m.column, m.row) => {}
        // A click outside the dialog closes it; one inside does nothing.
        Event::Mouse(m) if matches!(m.kind, MouseEventKind::Down(_)) => {
            if !app.dialog_area().is_some_and(|a| a.contains((m.column, m.row).into())) {
                app.close_dialog();
                app.closing_click = Some((std::time::Instant::now(), m.column, m.row));
            }
        }
        _ => {}
    }
    true
}

/// A left click at a position on a row of the topmost jump to / search list: selects it and
/// does what Enter does (which closes the list). False when no such list has a row there.
fn click_row(app: &mut App, column: u16, row: u16) -> bool {
    if let Some(sw) = app.switcher.as_mut() {
        let Some(n) = sw.row_at(column, row) else { return false };
        sw.selected = n;
        app.switch();
    } else if let Some(search) = app.search.as_mut() {
        let Some(n) = search.row_at(column, row) else { return false };
        search.selected = n;
        app.open_search_hit();
    } else {
        return false;
    }
    app.closing_click = Some((std::time::Instant::now(), column, row));
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
                Resolved::Action(Context::Sidebar | Context::Messages | Context::Huddle | Context::Activity | Context::Threads | Context::Huddles, Action::Help) => {
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
                // The main pane shows a view: its keys.
                Focus::Messages if app.view != View::Conversation => {
                    if let Some(action) = action {
                        app.view_action(action);
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
                        // (Not under the Threads feed: the conversation's messages are not on screen.)
                        KeyOutcome::UpWhenEmpty if !thread && app.view == View::Threads => {}
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
            app.sidebar.hover = app.sidebar.hover_at(c, r);
            app.messages.hover = app.messages.message_at(c, r);
            app.thread_pane.hover = if app.thread.is_some() { app.thread_pane.message_at(c, r) } else { None };
            // (A view not shown has no rows to hover.)
            app.threads.hover_at(c, r);
            app.activity.hover_at(c, r);
            app.huddles.hover_at(c, r);
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
                } else if let Some(v) = app.sidebar.view_at(m.column, m.row).and_then(|i| view_of_row(&i)) {
                    app.open_view(v);
                } else if let Some(name) = app.sidebar.header_at(m.column, m.row) {
                    app.sidebar.toggle_fold_keeping(&app.ws, &name, app.current.as_deref());
                } else if app.sidebar.click(m.column, m.row) {
                    // On its messages in NORMAL, as Jump to (`i` types).
                    app.focus = Focus::Messages;
                } else if let Some(link) = m.modifiers.is_empty().then(|| app.threads.link_under(m.column, m.row).and_then(|(_, app)| app)).flatten() {
                    // A plain click on a link into your Slack in the Threads feed opens it here.
                    app.open_app_link(&link);
                } else if let Some(at) = app.threads.message_at(m.column, m.row) {
                    // A message in the Threads feed: selected, as in a conversation.
                    app.threads.selected = Some(at);
                    app.focus = Focus::Messages;
                } else if let Some((channel, parent)) = app.threads.thread_at(m.column, m.row) {
                    // A thread's header (or its "more replies"): the whole thread, at its parent.
                    app.threads.selected = Some((channel, parent));
                    app.open_thread_item();
                    app.closing_click = Some((std::time::Instant::now(), m.column, m.row));
                } else if let Some(tab) = app.activity.tab_at(m.column, m.row) {
                    app.show_activity_tab(tab);
                    app.focus = Focus::Messages;
                } else if let Some(link) = m.modifiers.is_empty().then(|| app.activity.link_under(m.column, m.row).and_then(|(_, app)| app)).flatten() {
                    // A plain click on a link into your Slack in an Activity item opens it here.
                    app.open_app_link(&link);
                } else if let Some(n) = app.activity.row_at(m.column, m.row) {
                    app.activity.selected = n;
                    app.open_activity_item();
                    app.closing_click = Some((std::time::Instant::now(), m.column, m.row));
                } else if let Some(row) = app.huddles.row_at(m.column, m.row) {
                    app.huddles.selected = Some(row);
                    app.open_huddle_item();
                    app.closing_click = Some((std::time::Instant::now(), m.column, m.row));
                } else if app.view_area().contains((m.column, m.row).into()) {
                    app.focus = Focus::Messages;
                } else if let Some(link) = m.modifiers.is_empty().then(|| app.link_at(m.column, m.row)).flatten() {
                    // A plain click on a link into your Slack opens it here (Ctrl+Shift+click stays the terminal's).
                    app.open_app_link(&link);
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
            // The wheel over the Threads feed scrolls it, as in a conversation.
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if app.view == View::Threads && app.threads.area.contains((m.column, m.row).into()) => {
                app.threads.scroll_by(if m.kind == MouseEventKind::ScrollDown { 3 } else { -3 });
            }
            // The wheel over a view moves its selection a step, like the keys.
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if app.view_area().contains((m.column, m.row).into()) => {
                app.threads.hover = None;
                app.activity.hover = None;
                app.huddles.hover = None;
                app.view_action(if m.kind == MouseEventKind::ScrollDown { Action::Down } else { Action::Up });
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

/// klack's own schedule writes, one at a time in the order made: each reads Slack's copy just
/// before it writes, so a later one waits for the earlier one.
#[derive(Debug, Default)]
struct ScheduleQueue {
    busy: bool,
    /// The edits being written now.
    flying: Option<Vec<slack_core::dnd::ScheduleEdit>>,
    waiting: std::collections::VecDeque<Vec<slack_core::dnd::ScheduleEdit>>,
    /// Slack's copy as last known (when the queue went busy, then as each write read it back):
    /// shown again when a write fails and Slack can't be asked.
    known: Option<slack_core::dnd::Schedule>,
}

impl ScheduleQueue {
    /// Edits made, `shown` the schedule before them: returned to be written now, or kept (None)
    /// while a write is in flight.
    fn push(&mut self, edits: Vec<slack_core::dnd::ScheduleEdit>, shown: Option<&slack_core::dnd::Schedule>) -> Option<Vec<slack_core::dnd::ScheduleEdit>> {
        if self.busy {
            self.waiting.push_back(edits);
            return None;
        }
        // Nothing in flight: what is shown is Slack's copy.
        self.known = shown.cloned();
        self.busy = true;
        self.flying = Some(edits.clone());
        Some(edits)
    }

    /// The write in flight ended: the next edits to write, if any. After a failure what waits is
    /// dropped (it was made on top of the failed change).
    fn done(&mut self, ok: bool) -> Option<Vec<slack_core::dnd::ScheduleEdit>> {
        if !ok {
            self.waiting.clear();
        }
        let next = self.waiting.pop_front();
        self.busy = next.is_some();
        self.flying = next.clone();
        next
    }

    /// Slack's copy was read back after a write.
    fn read(&mut self, slack: Option<&slack_core::dnd::Schedule>) {
        self.known = slack.cloned();
    }

    /// Slack's copy as last known.
    fn last_read(&self) -> Option<&slack_core::dnd::Schedule> {
        self.known.as_ref()
    }

    /// The edits waiting their turn, in order.
    fn waiting(&self) -> impl Iterator<Item = &Vec<slack_core::dnd::ScheduleEdit>> {
        self.waiting.iter()
    }

    /// The edits not yet on Slack: the one being written, then those waiting.
    fn pending(&self) -> impl Iterator<Item = &Vec<slack_core::dnd::ScheduleEdit>> {
        self.flying.iter().chain(&self.waiting)
    }
}

/// The schedule shown: Slack's copy (`ws.dnd.schedule`) with klack's changes not yet on Slack on
/// top. Slack echoes a write as one `pref_change` per pref, the kind last, so its copy passes
/// through the previous schedule while a write is in flight.
fn show_pending_schedule(ws: &mut Workspace, q: &ScheduleQueue) {
    for edits in q.pending() {
        if let Some(s) = ws.dnd.schedule.as_ref().and_then(|s| s.edited(edits).ok()) {
            ws.dnd.schedule = Some(s);
        }
    }
}

/// The note after the schedule changed (or `/dnd schedule`): the schedule `described`, and
/// `until` when notifications are paused now.
fn schedule_note(described: String, until: Option<String>) -> String {
    match until {
        Some(t) => format!("notification schedule: {described} · paused now, until {t}"),
        None => format!("notification schedule: {described}"),
    }
}

/// The note after `/dnd`: what is in effect now, from `dnd.info` (`until`: when paused, till when).
fn dnd_note(resumed: bool, until: Option<String>) -> String {
    match (resumed, until) {
        (true, None) => "notifications resumed".into(),
        // Never a silent "resumed" while Slack still holds them.
        (true, Some(t)) => format!("notifications are still paused until {t}"),
        (false, Some(t)) => format!("notifications paused until {t}"),
        (false, None) => "notifications paused".into(),
    }
}

/// Global actions that open a view, a dialog or the message box: none of them opens over a
/// dialog (the insert leader reaches them there).
fn opens_something(action: Action) -> bool {
    use Action::*;
    matches!(action, Switcher | BrowseChannels | Search | Threads | Activity | Details | MoveToSection | Help | Preferences | LeaveConversation | DeleteSection | RenameSection | NewChannel | NewPrivateChannel | NewDm | NewSnippet | Upload | SetStatus | PauseFor | HuddleThread | HuddleView)
}

/// The note for an action that would open something under the dialog `overlay`, naming
/// the dialog's close key (a y/n question has none: any other key is no).
fn close_first(keys: &Keymap, overlay: Option<Context>) -> String {
    if overlay == Some(Context::Confirm) {
        return "answer the question first".into();
    }
    match overlay.and_then(|c| keys.first_key(c, Action::Close)) {
        Some(key) => format!("close the dialog first ({key})"),
        None => "close the dialog first".into(),
    }
}

/// [`prefilled`] for the message box `c`; `other_editing`: the reply box is editing a message.
fn prefill_box(c: &Composer, other_editing: bool, command: &str, wrap: Wrap) -> Result<(String, usize), &'static str> {
    // Attachments would be sent with the command as their text.
    if c.attachments.is_empty() { prefilled(&c.text(), c.editing.is_some() || other_editing, command, wrap) } else { Err("the message box is in use") }
}

/// Where you are, as far as which actions apply (see [`applies`]).
#[derive(Debug, Clone, Copy, Default)]
struct Here {
    /// What the selected message offers (None: no message selected).
    offers: Option<super::menu::Offers>,
    /// The thread pane has the focus.
    in_thread: bool,
    /// The current conversation is a channel you are not in.
    outside: bool,
    /// The current conversation is a channel you can leave.
    leavable: bool,
    /// So is the conversation selected in the sidebar.
    selected_leavable: bool,
    /// A conversation (not a header or Threads) is selected in the sidebar.
    selected: bool,
    /// A profile card is open.
    card: bool,
    /// A dialog is open: nothing opens under it.
    dialog: bool,
    /// How many workspaces the profile has.
    workspaces: usize,
}

/// Whether `action` of `context` makes sense here; which-key leaves the others out.
fn applies(here: &Here, context: Context, action: Action) -> bool {
    use Action::*;
    match (context, action) {
        (Context::Global, _) if here.dialog && opens_something(action) => false,
        (Context::Messages, OpenThread) => !here.in_thread,
        (Context::Messages, MarkUnread) => !here.in_thread && here.offers.is_some(),
        (Context::Global, MarkUnread) => !here.in_thread && !here.outside,
        (Context::Messages, Edit | Delete | Open | Save | Copy) => here.offers.is_some_and(|o| o.allows(action)),
        (Context::Global, JoinConversation) => here.outside,
        (Context::Global, LeaveConversation) => here.leavable,
        (Context::Sidebar, LeaveConversation) => here.selected_leavable,
        (Context::Sidebar, Star | MoveToSection | Details | NotificationLevel | MuteConversation | CopyConversationLink) => here.selected,
        (Context::Profile, Huddle) => here.card,
        // Only the profile's workspaces (the one you are in too).
        (Context::Global, _) if super::keymap::workspace_of(action).is_some() => super::keymap::workspace_of(action).is_some_and(|i| i < here.workspaces),
        _ => true,
    }
}

/// What a global key does. False to quit.
fn run_global(app: &mut App, action: Action) -> bool {
    match action {
        Action::Quit => return false,
        Action::Help => app.help = Some(super::help::Help::default()),
        Action::Preferences => app.prefs = Some(super::prefs::Prefs::new()),
        Action::Switcher => app.switcher = Some(super::switcher::Switcher::default()),
        Action::Search => app.search = Some(super::search::Search::default()),
        Action::Activity => app.open_view(View::Activity),
        Action::Threads => app.open_view(View::Threads),
        Action::Huddles => app.open_view(View::Huddles),
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
        Action::MuteConversation => app.toggle_conversation_mute(),
        Action::CopyConversationLink => app.copy_conversation_link(),
        Action::MarkUnread => {
            // From the messages pane its selection; from the sidebar or the composer the newest.
            let selected = app.messages.selected.clone().filter(|_| app.focus == Focus::Messages);
            let in_thread = matches!(app.focus, Focus::Thread | Focus::ThreadComposer);
            app.mark_unread(selected, in_thread);
        }
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
        Action::PauseNotifications => app.run_command(app.current.clone().unwrap_or_default(), super::commands::Command::Dnd(super::commands::DndAction::Toggle), None),
        Action::PauseFor => app.prefill("/dnd ", Wrap::No),
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
        Action::Mute => match app.call.as_mut() {
            Some(call) => {
                if let Err(e) = call.toggle_mute() {
                    app.status = format!("microphone: {e}");
                }
            }
            None => app.status = "not in a huddle".into(),
        },
        Action::Workspace1 => app.switch_workspace(0),
        Action::Workspace2 => app.switch_workspace(1),
        Action::Workspace3 => app.switch_workspace(2),
        Action::Workspace4 => app.switch_workspace(3),
        Action::Workspace5 => app.switch_workspace(4),
        Action::Workspace6 => app.switch_workspace(5),
        Action::Workspace7 => app.switch_workspace(6),
        Action::Workspace8 => app.switch_workspace(7),
        Action::Workspace9 => app.switch_workspace(8),
        Action::GoBack => app.step_history(false),
        Action::GoForward => app.step_history(true),
        Action::PrevConversation => app.sidebar.move_by_conversation(-1),
        Action::NextConversation => app.sidebar.move_by_conversation(1),
        Action::PrevUnread | Action::NextUnread => {
            // From the open conversation, else from where the sidebar's selection is.
            let from = app.current.clone().or_else(|| app.sidebar.selected_id().map(String::from));
            match unread_jump(app.current.as_deref(), app.ws.next_unread(from.as_deref(), action == Action::NextUnread)) {
                Ok(id) => app.jump_to(&id),
                Err(note) => app.status = note.into(),
            }
        }
        Action::ScrollUp | Action::ScrollDown => {
            let older = scroll_page(app.focus, app.thread.is_some(), &mut app.messages, &mut app.thread_pane, action == Action::ScrollUp);
            if older {
                app.maybe_load_older();
            }
        }
        Action::FocusNext | Action::FocusPrev => {
            app.focus = step_focus(&app.focus_order(), app.focus, action == Action::FocusNext);
        }
        _ => {}
    }
    true
}

/// Where next / previous unread goes: the conversation found, or why nowhere. Finding the open
/// conversation itself means it is the only one unread: nothing else.
fn unread_jump(open: Option<&str>, found: Option<String>) -> Result<String, &'static str> {
    match found {
        None => Err("nothing unread"),
        Some(id) if open == Some(id.as_str()) => Err("nothing else unread"),
        Some(id) => Ok(id),
    }
}

/// PgUp / PgDn: a page of the pane that has focus (the thread and its reply box: the thread;
/// anything else: the messages). When the thread replaces the messages (a narrow terminal: the
/// messages pane has no room) the thread pages whatever has focus. True when the messages
/// scrolled up (older ones may be wanted).
fn scroll_page(focus: Focus, thread_open: bool, messages: &mut MessagePane, thread: &mut MessagePane, up: bool) -> bool {
    let in_thread = thread_open && (matches!(focus, Focus::Thread | Focus::ThreadComposer) || messages.area.height == 0);
    let pane = if in_thread { thread } else { messages };
    let page = pane.area.height.saturating_sub(3) as usize;
    if up {
        pane.scroll_up(page);
    } else {
        pane.scroll_down(page);
    }
    up && !in_thread
}

fn handle_rtm(app: &mut App, ev: slack_core::rtm::RtmEvent) {
    use slack_core::rtm::RtmEvent;
    match ev {
        RtmEvent::Connected(socket) => {
            tracing::info!("rtm connected ({socket:?})");
            // Profile changes pushed while the connection was away are lost: the people shown are looked up.
            let down_since = app.rtm_down_since.take();
            if app.socket.replace(socket).is_some() {
                // (Not at each reconnect of a flapping link: up to STATUS_REFRESH_MAX look-ups each time.)
                let now = std::time::Instant::now();
                if requeue_people(down_since.map(|t| now.duration_since(t)), app.people_requeued.map(|t| now.duration_since(t))) {
                    app.people_requeued = Some(now);
                    app.queue_people_shown();
                }
                // (Slack's client asks for its counts at each connect: badge changes while away are not pushed.)
                app.fetch_activity_badge();
            }
            app.connected = true;
            app.presence_subscribed = false;
            app.subscribe_presence();
            // A new connection knows of no subscription.
            app.followed = Default::default();
            app.follow_profiles(std::time::Instant::now());
            // Huddles that started or ended while it was away (at start: those going on).
            app.fetch_live_huddles();
        }
        RtmEvent::Disconnected(reason) => {
            app.connected = false;
            // (Each failed reconnect says so again: the first drop counts.)
            app.rtm_down_since.get_or_insert_with(std::time::Instant::now);
            tracing::warn!("rtm disconnected: {reason}");
        }
        RtmEvent::SignedOut => {
            app.connected = false;
            app.mark_signed_out();
        }
        RtmEvent::Event(v) => {
            let fx = super::live::apply(&mut app.ws, &v, app.current.as_deref());
            // An edit, a deletion or a reaction in a thread of the Threads feed: in its block.
            app.threads.apply_live(&v);
            if v["type"] == "pref_change" {
                // (The pref's name only, never its value.)
                tracing::debug!(name = v["name"].as_str().unwrap_or_default(), "pref_change applied");
            }
            if v["type"] == "pref_change" && v["name"].as_str().is_some_and(|n| n.starts_with("dnd_")) {
                show_pending_schedule(&mut app.ws, &app.schedule_writes);
            }
            if fx.rebuild_sidebar {
                app.sidebar.rebuild(&app.ws);
            }
            if fx.refetch_live_huddles {
                app.fetch_live_huddles();
            }
            if let Some(id) = fx.invalidated {
                app.profile_invalidated(id);
            }
            if fx.dnd_invalidated {
                app.fetch_dnd();
            }
            match fx.activity {
                Some(super::live::ActivityPush::Updated(mut it)) => {
                    super::activity::apply_seen(std::slice::from_mut(&mut it), &app.seen);
                    app.activity.upsert(it);
                }
                Some(super::live::ActivityPush::Deleted(key)) => app.activity.remove_key(&key),
                // (As Slack's client does: its lists are dropped and the one shown is loaded again.)
                Some(super::live::ActivityPush::Invalidated) if app.view == View::Activity => app.open_activity(Some(app.activity.tab)),
                _ => {}
            }
            // (As the official client does: an ended huddle the view shows may now be among the recent ones.)
            if app.view == View::Huddles && app.huddles.concerns(&fx.huddles_ended) {
                let generation = app.huddles.refresh();
                app.fetch_huddle_history(generation, None);
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
                // A reply in a thread of the Threads feed: into its block, unread when someone else's.
                let mine = m.user.as_deref() == Some(app.ws.self_id.as_str());
                app.threads.add_reply(&channel, &m, !mine);
                let in_view = if m.is_reply() {
                    app.thread.as_ref().is_some_and(|(c, t)| *c == channel && m.thread_ts.as_ref() == Some(t))
                } else {
                    // (A view in the main pane hides the conversation.)
                    app.view == View::Conversation && app.current.as_deref() == Some(channel.as_str())
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
            app.ws.users_changed();
            app.ws.apply_boot(&boot);
            if let Some(c) = counts {
                app.ws.apply_counts(&c);
            }
            // (No Activity badge in Slack's counts: the row counts the feed's unread items.)
            if app.ws.activity_counts.is_none() {
                app.fetch_activity_unread();
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
                // A link's or a notification's, from the workspace switched from.
                app.open_in_app(target, true);
            } else if let Some(id) = app.sidebar.remembered.clone().filter(|id| app.ws.conversations.get(id).is_some_and(|c| !c.is_archived)) {
                // Back where you were last time, in NORMAL on the messages (or the thread).
                app.start = Some((id.clone(), app.sidebar.remembered_thread.clone()));
                app.jump_to(&id);
                app.focus = Focus::Messages;
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
            app.ws.put_user(u);
            app.sidebar.rebuild(&app.ws);
        }
        Net::Profiles(users) => {
            app.lookups.busy = false;
            for u in users {
                app.ws.put_user(u);
            }
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
            app.ws.put_conversation(channel);
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
            let tl = app.ws.timelines.entry(channel.clone()).or_default();
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
            if !older && app.start.as_ref().is_some_and(|(c, _)| *c == channel) {
                let thread = app.start.take().and_then(|(_, t)| t);
                if app.focus == Focus::Messages {
                    app.leave_composer(false);
                }
                if let Some(ts) = thread {
                    let (client, tx) = (app.client.clone(), app.net.clone());
                    tokio::spawn(async move {
                        let page = client.replies(&channel, &ts).await.ok();
                        let _ = tx.send(Net::ThreadBack { channel, ts, page });
                    });
                }
            }
        }
        Net::ThreadBack { channel, ts, page } => {
            // Dropped quietly when it cannot be loaded, or you moved on meanwhile.
            let Some(page) = page.filter(|p| !p.messages.is_empty()) else { return };
            if app.current.as_ref() != Some(&channel) || app.thread.is_some() {
                return;
            }
            let tl = app.ws.threads.entry(super::state::thread_key(&channel, &ts)).or_default();
            for m in page.messages {
                tl.upsert(m);
            }
            tl.loaded = true;
            let before = app.focus;
            app.open_thread_at(channel, ts);
            app.leave_composer(true);
            app.focus = focus_after_thread_back(before, app.focus);
        }
        Net::Posted { channel, message } => {
            super::live::add_message(&mut app.ws, &channel, &message);
            // A reply from the feed: into its block; replying marks the thread read.
            if app.threads.add_reply(&channel, &message, false) {
                if let Some(parent) = message.thread_ts.clone() {
                    app.mark_feed_read(channel.clone(), parent);
                }
            }
            if app.ws.note_activity(&channel, message.time()) && app.ws.conversations.get(&channel).is_some_and(|c| c.is_dm()) {
                app.sidebar.rebuild(&app.ws);
            }
        }
        Net::LinkAuthor { channel, ts, result } => match result {
            Ok(message) => {
                let name = message.and_then(|m| super::applink::author_name(&app.ws, &m));
                app.authors.finish(&channel, &ts, name);
            }
            Err(e) => {
                tracing::debug!("link author for {channel} {ts}: {e}");
                app.authors.fail(&channel, &ts, std::time::Instant::now());
            }
        },
        Net::Replies { channel, ts, page } => {
            let tl = app.ws.threads.entry(super::state::thread_key(&channel, &ts)).or_default();
            for m in page.messages {
                tl.upsert(m);
            }
            tl.loaded = true;
            tl.loading = false;
            app.check_link_reply();
        }
        Net::LinkConversation { result, target } => match result {
            // Known now: opened as any other (the same one only, so this is asked once).
            Ok(c) if target.channel() == Some(c.id.as_str()) => {
                app.ws.put_conversation(c);
                app.sidebar.rebuild(&app.ws);
                app.open_in_app(target, true);
            }
            other => {
                if let Err(e) = other {
                    tracing::debug!("conversations.info for a link: {e}");
                }
                let note = match target {
                    super::applink::Target::Conversation { .. } => CONVERSATION_GONE,
                    _ => LINK_GONE,
                };
                app.status = super::i18n::tr(note).into_owned();
            }
        },
        Net::FileDone(message) => app.status = message,
        Net::SendFailed { thread, key, draft, error } => {
            app.status = error;
            let here = if thread { app.thread.as_ref().map(|(c, ts)| super::state::thread_key(c, ts)) } else { app.composer_key() };
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
        Net::MarkUnreadFailed { channel, error } => {
            // Slack still has it read: so here, and read-marking takes it up again.
            app.ws.release_unread(&channel);
            if let Some(u) = app.ws.unread.get_mut(&channel) {
                u.has_unreads = false;
            }
            if app.current.as_deref() == Some(channel.as_str()) {
                app.messages.new_since = None;
            }
            app.sidebar.rebuild(&app.ws);
            app.status = format!("couldn't mark {} unread: {error}", app.ws.channel_label(&channel));
        }
        Net::MuteFailed { channel, muted, error } => {
            if app.ws.revert_mute(&channel, muted) {
                app.sidebar.rebuild(&app.ws);
            }
            app.status = format!("couldn't {} {}: {error}", if muted { "mute" } else { "unmute" }, app.ws.channel_label(&channel));
        }
        Net::Dnd(v) => {
            app.ws.dnd.set_info(&v);
            // An answer that is already out of date is not asked again.
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
            tracing::debug!(paused = app.ws.dnd_active(now), "dnd.info");
            app.dnd_refreshing = app.ws.dnd.stale(now);
        }
        Net::DndSchedule { error, prefs, info } => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
            if let Some(v) = &info {
                app.ws.dnd.set_info(v);
                app.dnd_refreshing = app.ws.dnd.stale(now);
            }
            // Slack's copy (when it couldn't be read after a failure, the last one known), then the
            // changes still waiting shown on top of it.
            if let Some(prefs) = &prefs {
                let dnd: serde_json::Map<String, Value> = prefs.as_object().into_iter().flatten().filter(|(k, _)| k.starts_with("dnd_")).map(|(k, v)| (k.clone(), v.clone())).collect();
                app.ws.apply_prefs(&Value::Object(dnd));
                app.schedule_writes.read(app.ws.dnd.schedule.as_ref());
            } else if error.is_some() {
                app.ws.dnd.schedule = app.schedule_writes.last_read().cloned();
            }
            let dropped = if error.is_some() { app.schedule_writes.waiting().count() } else { 0 };
            let next = app.schedule_writes.done(error.is_none());
            show_pending_schedule(&mut app.ws, &app.schedule_writes);
            match error {
                Some(e) => {
                    let also = if dropped > 0 { format!(" ({dropped} later change{} not sent)", if dropped == 1 { "" } else { "s" }) } else { String::new() };
                    app.status = format!("changing the notification schedule failed: {e}{also}");
                    if let Some(p) = app.prefs.as_mut() {
                        p.saved = Some(Err(e));
                    }
                }
                None => {
                    if let Some(p) = app.prefs.as_mut() {
                        p.saved = None;
                    }
                    // The note once the last change is written.
                    if next.is_none() {
                        let format = app.ws.settings.time_format();
                        let until = info.and_then(|_| app.ws.dnd_until(now)).and_then(|t| super::clock::until_label(t as i64, now as i64, format));
                        app.status = schedule_note(super::prefs::describe_schedule(app.ws.dnd.schedule.as_ref(), format, &super::prefs::schedule_zone(&app.ws)), until);
                    }
                }
            }
            if let Some(edits) = next {
                app.write_schedule(edits);
            }
        }
        Net::DndSet { resumed, error, info } => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
            if let Some(v) = &info {
                app.ws.dnd.set_info(v);
                app.dnd_refreshing = app.ws.dnd.stale(now);
            }
            app.status = match error {
                Some(e) => format!("{} failed: {e}", if resumed { "resuming notifications" } else { "pausing notifications" }),
                None => {
                    let until = info.and_then(|_| app.ws.dnd_until(now)).and_then(|t| super::clock::until_label(t as i64, now as i64, app.ws.settings.time_format()));
                    dnd_note(resumed, until)
                }
            };
        }
        Net::ActivityFeed { tab, more, result } => match result {
            Ok(v) => {
                let mut items = super::activity::parse(&v);
                super::activity::apply_seen(&mut items, &app.seen);
                let cursor = v["response_metadata"]["next_cursor"].as_str().filter(|c| !c.is_empty()).map(String::from);
                if more {
                    app.activity.add_page(tab, items, cursor);
                } else {
                    // (Without Slack's badge the sidebar row counts the All tab's first page, as at start.)
                    if tab == super::activity::Tab::All && app.ws.activity_counts.is_none() {
                        app.ws.activity_unread = super::activity::unread_count(&items);
                    }
                    app.activity.set_items(tab, items, cursor);
                }
            }
            Err(e) => {
                app.status = format!("activity failed: {e}");
                app.activity.fail(tab, more, e);
            }
        },
        Net::LiveHuddles { since, result } => match result {
            Ok(live) => {
                app.ws.set_live_huddles(live, since);
                app.sidebar.rebuild(&app.ws);
            }
            // The huddles kept from the live events stay.
            Err(e) => tracing::warn!("the huddles going on: {e}"),
        },
        Net::HuddleHistory { generation, more, result } => match result {
            Ok((page, cursor)) => app.huddles.add_page(generation, more, page, cursor),
            Err(e) => {
                app.status = format!("{}: {e}", super::i18n::tr(RECENT_FAILED));
                app.huddles.fail(generation, e);
            }
        },
        Net::HuddleReplies { channel, ts, count } => {
            if let Some(n) = count {
                app.huddles.replies.insert((channel, ts), n);
            }
        }
        // The sidebar's Activity row, at start (Slack's counts had no badge).
        Net::ActivityUnread(result) => match result {
            Ok(v) if app.ws.activity_counts.is_none() => {
                let mut items = super::activity::parse(&v);
                super::activity::apply_seen(&mut items, &app.seen);
                app.ws.activity_unread = super::activity::unread_count(&items);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("activity feed for the sidebar: {e}"),
        },
        // (A push or an item read here since it was asked is newer: it stays.)
        Net::ActivityBadge { since, result } => match result {
            Ok(v) => {
                app.ws.activity_counts_fetched(since, &v);
            }
            Err(e) => tracing::warn!("counts after a reconnect: {e}"),
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
                let t = &mut app.threads;
                let (mut page, has_more) = super::threads::parse(&v);
                super::threads::apply_seen(&mut page, &app.seen);
                if !more {
                    t.entries = None;
                }
                t.add_page(page, has_more);
            }
            Err(e) => {
                app.status = format!("threads failed: {e}");
                app.threads.loading = false;
                // A next page that failed leaves what is listed.
                if !more {
                    app.threads.failed = Some(e);
                }
            }
        },
        Net::FeedReplies { channel, ts, result } => match result {
            Ok(page) => app.threads.merge_replies(&channel, &ts, page),
            // The block keeps what it has.
            Err(e) => tracing::warn!("the newest replies of a thread in the feed: {e}"),
        },
        Net::ActivityMessage { channel, ts, message } => {
            app.activity.add_message((channel, ts), message);
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
        Net::HistoryFailed { channel, error } => {
            app.loading = false;
            let seeking = app.seeking.as_ref().is_some_and(|(c, _)| *c == channel);
            app.status = history_failed(app.ws.timelines.get_mut(&channel), seeking, app.seek_from_link, &error);
            if seeking {
                // The seek ends here (a link's message never selected), as when the message is missing.
                (app.seeking, app.seek_hit, app.link_reply) = (None, None, None);
            }
        }
        Net::Error(e) => {
            app.loading = false;
            app.status = e;
        }
    }
}

/// Replies asked for a Threads feed block that shows fewer than it has (one more than it shows).
const FEED_REPLIES: usize = 4;

/// How long to wait before joining again a huddle that ended before it connected.
const REJOIN_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

/// A reconnect looks the people shown up again when the websocket was down this long ...
const REQUEUE_DOWN: Duration = Duration::from_secs(30);
/// ... or the last reconnect's look-up was this long ago (a short drop misses few pushes).
const REQUEUE_EVERY: Duration = Duration::from_secs(300);

/// Whether a reconnect after `down` (None: not known) looks the people shown up again, the last time having
/// been `since_last` ago (None: never).
fn requeue_people(down: Option<Duration>, since_last: Option<Duration>) -> bool {
    down.is_some_and(|d| d >= REQUEUE_DOWN) || since_last.is_none_or(|d| d >= REQUEUE_EVERY)
}

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

/// The screen split into the body, the call bar and the status line; an empty row separates
/// the call bar from the status line, only when there is a call bar.
fn bottom_rows(area: Rect, call_rows: u16) -> [Rect; 3] {
    let gap = u16::from(call_rows > 0);
    let [body, call_bar, _, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(call_rows), Constraint::Length(gap), Constraint::Length(1)]).areas(area);
    [body, call_bar, status]
}

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    app.screen = f.area();
    // A sequence started before a dialog opened (or closed) is over.
    let mode = app.mode();
    app.pending.sync(mode);
    // Nothing behind a dialog is hovered (it opened with the pointer over a row).
    if mode == Mode::Dialog {
        app.clear_main_hover();
    }
    app.sync_which_key(std::time::Instant::now());
    let messages = &app.ws.settings.messages;
    app.ws.emoji_pictures = app.media.graphics() && messages.custom_emoji_pictures && !messages.emoji_as_names;
    // The huddle view goes with the call, and gives way once the messages or the composer
    // get the focus (opening a conversation, replying, …).
    if app.call.is_none() || matches!(app.focus, Focus::Messages | Focus::Composer) {
        app.huddle_view = None;
    }
    if app.call.is_none() || matches!(app.focus, Focus::Messages | Focus::Composer) {
        app.parked_huddle = None;
    }
    // Back to the messages, not into the message box (only you start typing).
    if app.huddle_view.is_none() && app.focus == Focus::Huddle {
        app.focus = Focus::Messages;
    }
    // A view gives way to the conversation once its message box gets the focus (a command put
    // into it, …): only the Threads feed has one, once a thread to reply in was chosen there.
    app.sync_feed_composer();
    if app.view != View::Conversation && app.focus == Focus::Composer && app.feed_reply.is_none() {
        app.close_view();
        app.focus = Focus::Composer;
    }
    let mode = app.mode();
    let call_rows = app.call.as_ref().map_or(0, |call| call.bar_rows(&app.ws, &app.media, f.area().width));
    let [body, call_bar, status] = bottom_rows(f.area(), call_rows);
    let above_status = body;
    app.call_bar_area = call_bar;
    if let Some(call) = &app.call {
        if let Some(w) = call.draw(f, call_bar, &app.ws, &app.media) {
            app.call_bar_wanted = Some(w);
        }
    }
    // The workspace rail, with two or more workspaces and room for it.
    let body = if app.workspaces.len() > 1 && body.width >= 60 {
        let [rail, rest] = Layout::horizontal([Constraint::Length(super::rail::WIDTH), Constraint::Min(1)]).areas(body);
        let badges = app.others.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let logos = app.logos.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let signed_out = app.signed_out.lock().unwrap_or_else(|e| e.into_inner()).clone();
        (app.rail, app.rail_wanted) = super::rail::draw(f, rail, &app.workspaces, app.workspace, &badges, &logos, &app.media, &signed_out);
        rest
    } else {
        app.rail.clear();
        body
    };
    let side_w = (body.width / 4).clamp(18, 32);
    let [side, main] = Layout::horizontal([Constraint::Length(side_w), Constraint::Min(10)]).areas(body);
    // A thread's draft counts for its conversation.
    app.sidebar.drafts = app.drafts.keys().map(|k| k.split('/').next().unwrap_or(k).to_string()).collect();
    app.sidebar.in_call = app.call.as_ref().map(|c| c.channel.clone());
    app.sidebar.draw(f, side, &app.ws, app.focus == Focus::Sidebar);
    // Too narrow for a thread beside the messages (as Slack does): the thread takes their place
    // until it is closed, or until the messages or their composer get the focus; the huddle
    // view and a thread take turns there.
    app.narrow = main.width < THREAD_BESIDE_MIN;
    if app.narrow && app.thread.is_some() {
        if matches!(app.focus, Focus::Messages | Focus::Composer) {
            app.close_thread();
        } else if let Some(view) = app.huddle_view.take() {
            app.parked_huddle = Some(view);
            if app.focus == Focus::Huddle {
                app.focus = Focus::Thread;
            }
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
        let here = super::applink::Here { ws: &app.ws, workspaces: &app.addresses, names: &app.workspaces, shown: app.workspace, authors: &app.authors, message: None };
        app.thread_pane.draw(f, pane, &here, &title, timeline, app.focus == Focus::Thread, &app.media);
        app.thread_composer_area = comp;
        draw_composer(f, comp, app, true);
    } else {
        // No thread drawn (closed, or none open): its links from an earlier draw are gone.
        app.thread_pane.hide();
    }

    // Only the view shown can be pointed at.
    if app.view != View::Threads || alone {
        app.threads.hide();
    }
    if app.view != View::Activity || alone {
        app.activity.hide();
    }
    if app.view != View::Huddles || alone {
        app.huddles.hide();
    }
    if alone {
        // Nothing of the message pane is on screen, so the mouse must not find it.
        app.messages.hide();
        app.composer_area = Default::default();
    } else if app.view != View::Conversation {
        // A view in the main pane, in place of the messages and their box (the Threads feed keeps the box, to reply
        // in its threads).
        app.messages.hide();
        app.composer_area = Default::default();
        let focused = app.focus == Focus::Messages;
        match app.view {
            View::Threads => {
                let composer_h = app.composer.height(main.width.saturating_sub(2) as usize);
                let [feed, comp] = Layout::vertical([Constraint::Min(3), Constraint::Length(composer_h)]).areas(main);
                let here = super::applink::Here { ws: &app.ws, workspaces: &app.addresses, names: &app.workspaces, shown: app.workspace, authors: &app.authors, message: None };
                app.threads.draw(f, feed, &here, focused, &app.media);
                app.composer_area = comp;
                draw_composer(f, comp, app, false);
            }
            View::Activity => {
                let here = super::applink::Here { ws: &app.ws, workspaces: &app.addresses, names: &app.workspaces, shown: app.workspace, authors: &app.authors, message: None };
                app.activity.draw(f, main, &here, focused, &app.media);
            }
            View::Huddles => {
                let me_in = app.call.as_ref().map(|c| c.channel.clone());
                app.huddles.draw(f, main, &app.ws, focused, &app.media, me_in.as_deref(), chrono::Utc::now().timestamp());
            }
            View::Conversation => {}
        }
    } else if let (Some(view), Some(call)) = (app.huddle_view.as_mut(), &app.call) {
        // Nothing of the message pane is on screen, so the mouse must not find it.
        app.messages.hide();
        app.composer_area = Default::default();
        view.draw(f, main, call, &app.ws, &app.media);
    } else {
        let inner_w = main.width.saturating_sub(2) as usize;
        let composer_h = app.composer.height(inner_w);
        let [msgs, comp] = Layout::vertical([Constraint::Min(3), Constraint::Length(composer_h)]).areas(main);
        let current = app.current.clone();
        let timeline = current.as_deref().and_then(|c| app.ws.timelines.get(c));
        let title = current.as_deref().map(|c| super::text::conversation_title(&app.ws, c, chrono::Utc::now().timestamp())).unwrap_or_default();
        app.messages.editing = app.composer.editing.clone();
        let here = super::applink::Here { ws: &app.ws, workspaces: &app.addresses, names: &app.workspaces, shown: app.workspace, authors: &app.authors, message: None };
        app.messages.draw(f, msgs, &here, &title, timeline, app.focus == Focus::Messages, &app.media);
        app.composer_area = comp;
        draw_composer(f, comp, app, false);
    }
    if let Some(c) = &app.completion {
        super::autocomplete::draw(f, if c.thread { app.thread_composer_area } else { app.composer_area }, c);
    }
    if let Some((picker, _, _)) = app.picker.as_mut() {
        picker.draw(f, main, &app.ws.custom_emoji, &app.ws.self_id, &|u| app.ws.user_name(u));
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
        h.draw(f, body, &app.keys, app.ws.settings.which_key.icons);
    }
    // which-key: the keys that can come next.
    app.which_key_area = Rect::default();
    if app.which_key.is_some() {
        let contexts = app.active_contexts();
        let mut children = app.offered(&contexts, mode, app.pending.keys());
        // A leader as a next key shows as the leader's key.
        for c in children.iter_mut().filter(|c| c.key == LEADER) {
            c.key = app.keys.leader_for(mode).unwrap_or(LEADER);
        }
        let settings = &app.ws.settings.which_key;
        let pages = super::which_key::layout(&children, above_status, settings.max_height_percent, settings.icons).pages;
        let back = super::which_key::backspace_goes_back(&app.keys, &contexts, mode, app.pending.keys());
        let title = super::which_key::title(&app.keys, &contexts, &app.pending);
        if let Some(panel) = app.which_key.as_mut() {
            panel.page = panel.page.min(pages.saturating_sub(1));
            let icons;
            (app.which_key_area, icons) = super::which_key::draw(f, above_status, &title, &children, panel, settings, back);
            // A workspace's logo over its icon, where the terminal draws pictures (the 🏢 stands in while it loads).
            let logos = app.logos.lock().unwrap_or_else(|e| e.into_inner()).clone();
            for (i, cell) in icons {
                let Some(logo) = children[i].action.and_then(|(_, a)| super::keymap::workspace_of(a)).and_then(|w| logos.get(&w)).filter(|_| app.media.graphics()) else { continue };
                let small = slack_core::types::File { id: format!("{}@small", logo.id), ..logo.clone() };
                match app.media.thumbs.get(&small.id) {
                    Some(super::media::Thumb::Ready(proto)) => {
                        f.render_widget(ratatui::widgets::Clear, cell);
                        f.render_widget(ratatui_image::sliced::SlicedImage::new(proto, ratatui_image::sliced::SignedPosition { x: 0, y: 0 }), cell);
                    }
                    Some(_) => {}
                    // Loaded with the rail's pictures.
                    None => app.rail_wanted.push((small, super::media::AVATAR_SIZE)),
                }
            }
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
    let ringing = app.ringing.as_ref().map(|ring| {
        let key = |a| app.keys.first_shown_in(app.mode(), Context::Global, a).unwrap_or_default();
        format!("📞 {} invites you to a huddle · {} join · {} decline", app.ws.user_name(&ring.user), key(Action::Huddle), key(Action::DeclineHuddle))
    });
    // The note for the link under the pointer, from where the panes are drawn now (not under a dialog or menu).
    app.link_note = app.pointer.filter(|_| app.overlay().is_none()).and_then(|(c, r)| app.link_note_at(c, r));
    // The hover note only fills a silent status line: a real message, whenever it came, stays.
    let note = shown_note(ringing, app.signed_out_note.as_deref(), &app.status).or_else(|| app.link_note.clone());
    let info = super::status::Info {
        unread,
        mentions,
        availability: super::profile::availability(&app.ws, chrono::Utc::now().timestamp()),
        paused: app.ws.dnd_active(chrono::Utc::now().timestamp() as f64),
        away: app.ws.presence.get(&app.ws.self_id) == Some(&false),
        live: app.connected,
        note,
    };
    let showcmd = app.keys.show(app.pending.typed());
    f.render_widget(Paragraph::new(super::status::line(pill, &info, &showcmd, &hints, status.width)), status);
}

/// The view a sidebar row opens.
fn view_of_row(item: &super::sidebar::Item) -> Option<View> {
    match item {
        super::sidebar::Item::Threads => Some(View::Threads),
        super::sidebar::Item::Activity => Some(View::Activity),
        super::sidebar::Item::Huddles => Some(View::Huddles),
        _ => None,
    }
}

/// The sidebar row of a view (none for the conversation).
fn row_of_view(v: View) -> Option<super::sidebar::Item> {
    match v {
        View::Threads => Some(super::sidebar::Item::Threads),
        View::Activity => Some(super::sidebar::Item::Activity),
        View::Huddles => Some(super::sidebar::Item::Huddles),
        View::Conversation => None,
    }
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
        Context::Huddles => Dialog("HUDDLES"),
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
    // Under the Threads feed: the thread it replies in (chosen with the Type key), else the selected message's.
    let feed = (!thread && app.view == View::Threads).then(|| app.feed_reply.clone().filter(|_| focused || !composer.is_empty()).or_else(|| app.threads.selected_thread())).flatten();
    let typing = if thread {
        app.thread.as_ref().map(|(c, ts)| app.ws.typing_in(&super::state::thread_key(c, ts))).unwrap_or_default()
    } else if let Some((c, ts)) = &feed {
        app.ws.typing_in(&super::state::thread_key(c, ts))
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
    } else if let Some((c, _)) = &feed {
        format!(" {} ", super::i18n::tr(super::feed::REPLY_IN).replace("{where}", &super::feed::place(&app.ws, c)))
    } else if app.view == View::Threads {
        String::new()
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
        } else if app.view == View::Threads {
            super::i18n::tr(super::feed::PRESS_TO_REPLY).replace("{key}", &app.keys.first_key(Context::Threads, Action::Type).unwrap_or_default())
        } else {
            format!("Press {} to type", app.keys.first_key(Context::Messages, Action::Type).unwrap_or_default())
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

/// Where the focus goes when the thread closes: back to the huddle view when it is shown
/// (it opened from there), else the messages.
fn focus_after_thread(huddle_shown: bool) -> Focus {
    if huddle_shown { Focus::Huddle } else { Focus::Messages }
}

/// The restored thread opens in the background: the focus moves to it only if you were
/// still on the messages (not in the message box, the sidebar, …).
fn focus_after_thread_back(before: Focus, opened: Focus) -> Focus {
    if before == Focus::Messages { opened } else { before }
}

/// The panes Tab cycles, left to right, top to bottom. Composers are left out: Tab never
/// changes the mode, typing is the user's own action.
fn pane_order(huddle: bool, narrow: bool, thread: bool) -> Vec<Focus> {
    // The huddle view stands in for the messages.
    let mut order = if huddle { vec![Focus::Sidebar, Focus::Huddle] } else { vec![Focus::Sidebar, Focus::Messages] };
    // A narrow terminal's thread stands in for them.
    if narrow && thread {
        order.truncate(1);
    }
    if thread {
        order.push(Focus::Thread);
    }
    order
}

/// The pane Tab (or Shift+Tab) moves to from `from`. A composer counts as its own pane
/// (the messages, the thread), so Tab from it goes to the pane after that one.
fn step_focus(order: &[Focus], from: Focus, forward: bool) -> Focus {
    let pane = match from {
        Focus::Composer => Focus::Messages,
        Focus::ThreadComposer => Focus::Thread,
        f => f,
    };
    let pos = order.iter().position(|f| *f == pane).unwrap_or(0);
    let step = if forward { 1 } else { order.len() - 1 };
    order[(pos + step) % order.len()]
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
    #[test]
    fn a_gap_separates_the_call_bar_from_the_status_line() {
        use ratatui::layout::Rect; use super::bottom_rows;
        let area = Rect { x: 0, y: 0, width: 80, height: 30 };
        let [body, bar, status] = bottom_rows(area, 2);
        assert_eq!((body.height, bar.height, status.height), (26, 2, 1));
        assert_eq!(status.y - (bar.y + bar.height), 1, "one empty row");
        let [body, bar, status] = bottom_rows(area, 0);
        assert_eq!((body.height, bar.height, status.y), (29, 0, 29), "no call, no gap");
    }

    use slack_core::session::WorkspaceSession;

    #[test]
    fn closing_a_thread_returns_to_the_huddle_view() {
        use super::{focus_after_thread, focus_after_thread_back, Focus};
        assert_eq!(focus_after_thread(true), Focus::Huddle);
        assert_eq!(focus_after_thread(false), Focus::Messages);
        assert_eq!(focus_after_thread_back(Focus::Messages, Focus::Thread), Focus::Thread);
        assert_eq!(focus_after_thread_back(Focus::Composer, Focus::Thread), Focus::Composer);
    }

    #[test]
    fn tab_never_enters_a_composer() {
        use super::{pane_order, step_focus, Focus};
        for (huddle, narrow, thread) in [(false, false, false), (false, false, true), (false, true, true), (true, false, false), (true, false, true)] {
            let order = pane_order(huddle, narrow, thread);
            assert!(!order.contains(&Focus::Composer) && !order.contains(&Focus::ThreadComposer), "{huddle} {narrow} {thread}");
            for &f in &order {
                for fwd in [true, false] {
                    assert!(order.contains(&step_focus(&order, f, fwd)));
                }
            }
        }
        assert_eq!(pane_order(false, false, true), [Focus::Sidebar, Focus::Messages, Focus::Thread]);
        assert_eq!(pane_order(false, true, true), [Focus::Sidebar, Focus::Thread]);
        let order = pane_order(false, false, true);
        assert_eq!(step_focus(&order, Focus::Composer, true), Focus::Thread, "Tab from the message box");
        assert_eq!(step_focus(&order, Focus::ThreadComposer, true), Focus::Sidebar, "Tab from the reply box");
        assert_eq!(step_focus(&order, Focus::Composer, false), Focus::Sidebar);
        assert_eq!(step_focus(&order, Focus::Messages, true), Focus::Thread);
    }

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
    fn a_failed_history_request_can_be_made_again() {
        use super::{LINK_GONE, history_failed};
        let mut tl = crate::tui::state::Timeline { loading: true, loaded: true, ..Default::default() };
        assert_eq!(history_failed(Some(&mut tl), true, true, "conversations.history: request failed"), LINK_GONE, "a link's seek ends with its note");
        assert!(!tl.loading, "not loading any more: the conversation pages back again");
        tl.loading = true;
        assert_eq!(history_failed(Some(&mut tl), true, false, "conversations.history: ratelimited"), "history failed: conversations.history: ratelimited", "a search's says what failed");
        assert!(!tl.loading);
        assert_eq!(history_failed(None, false, true, "x"), "history failed: x", "not the conversation sought");
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
    fn a_seek_that_reaches_the_start_says_so() {
        use super::{LINK_GONE, Seek, Sought, seek_step};
        let s = |found, older, newer, has_more| Sought { found, older, newer, has_more };
        // (what is loaded; started by a link; the newest page fetched again since)
        assert_eq!(seek_step(s(true, false, false, true), true, false), Seek::Done);
        assert_eq!(seek_step(s(false, true, false, true), true, false), Seek::More, "far back: paged for");
        assert_eq!(seek_step(s(false, true, false, false), true, false), Seek::Missing(LINK_GONE), "the start of the history");
        assert_eq!(seek_step(s(false, false, false, true), true, false), Seek::Missing(LINK_GONE), "deleted: inside what is loaded, not there");
        assert_eq!(LINK_GONE, "that message isn't there any more, or you can't see it");
        // Newer than anything loaded (a timeline loaded a while ago): the newest page is fetched once before judging.
        assert_eq!(seek_step(s(false, false, true, true), true, false), Seek::Refresh);
        assert_eq!(seek_step(s(false, false, true, true), true, true), Seek::Missing(LINK_GONE), "still not there");
        assert_eq!(seek_step(s(true, false, false, true), true, true), Seek::Done);
        // Search behaves as before.
        assert_eq!(seek_step(s(true, false, false, true), false, false), Seek::Done);
        assert_eq!(seek_step(s(false, false, false, true), false, false), Seek::Done);
        assert_eq!(seek_step(s(false, false, true, true), false, false), Seek::Done);
        assert_eq!(seek_step(s(false, true, false, true), false, false), Seek::More);
        assert_eq!(seek_step(s(false, true, false, false), false, false), Seek::Missing("that message is no longer in this conversation's history"));
    }

    #[test]
    fn a_link_selects_its_message_only_once_found() {
        use super::{LINK_GONE, Seek, select_now};
        for seek in [None, Some(Seek::More), Some(Seek::Done), Some(Seek::Missing("that message is no longer in this conversation's history"))] {
            assert!(select_now(false, seek), "a search hit: at once, as before ({seek:?})");
        }
        assert!(select_now(true, Some(Seek::Done)), "found");
        for seek in [None, Some(Seek::More), Some(Seek::Refresh), Some(Seek::Missing(LINK_GONE))] {
            assert!(!select_now(true, seek), "a link's: not before it is found, never when missing ({seek:?})");
        }
    }

    #[test]
    fn notify_targets_still_open() {
        use super::super::applink::Target;
        use super::super::notify::Target as Notified;
        use super::hit_of;
        let reply = Target::from(Notified { channel: "C1".into(), ts: Some("2.0".into()), thread_ts: Some("1.0".into()) });
        let hit = hit_of(reply.clone()).expect("a message");
        assert_eq!((hit.channel.as_str(), hit.ts.as_str(), hit.thread_ts.as_deref()), ("C1", "2.0", Some("1.0")), "the reply in its thread");
        let top = hit_of(Target::from(Notified { channel: "C1".into(), ts: Some("2.0".into()), thread_ts: None })).unwrap();
        assert_eq!((top.ts.as_str(), top.thread_ts), ("2.0", None));
        assert!(hit_of(Target::from(Notified { channel: "C1".into(), ts: None, thread_ts: None })).is_none(), "a conversation: opened as Jump to does");
        // What waits for another workspace to boot is the same kind of target as a link's.
        let after: super::super::others::OpenAfter = Default::default();
        *after.lock().unwrap() = Some((1, reply.clone()));
        assert_eq!(after.lock().unwrap().take(), Some((1, reply)));
        // The desktop app's thread link: the thread, its parent selected.
        let thread = hit_of(Target::Message { channel: "C1".into(), ts: "1.0".into(), thread_ts: Some("1.0".into()) }).unwrap();
        assert_eq!((thread.ts.as_str(), thread.thread_ts.as_deref()), ("1.0", Some("1.0")));
        assert!(hit_of(Target::Person { user: "U1".into() }).is_none() && hit_of(Target::File { file: "F1".into() }).is_none());
    }

    #[test]
    fn command_keys_clear_the_note() {
        use super::clears_note;
        use super::super::keymap::{Action, Context, Resolved};
        assert!(clears_note(&Resolved::Action(Context::Global, Action::Quit), false), "a command");
        assert!(clears_note(&Resolved::Pending, false), "the leader");
        assert!(clears_note(&Resolved::None, true), "ends a sequence with its own note");
        assert!(!clears_note(&Resolved::None, false), "typing");
        // Editing in the message box is typing too; sending, leaving and clearing the box are commands.
        for a in [Action::Backspace, Action::Left, Action::Right, Action::Up, Action::Down, Action::WordLeft, Action::LineEnd, Action::DeleteWord, Action::DeleteForward, Action::Newline, Action::Paste, Action::Bold, Action::Code] {
            assert!(!clears_note(&Resolved::Action(Context::Composer, a), false), "{a:?}");
        }
        for a in [Action::Send, Action::Leave, Action::ClearBox] {
            assert!(clears_note(&Resolved::Action(Context::Composer, a), false), "{a:?}");
        }
        assert!(clears_note(&Resolved::Action(Context::Messages, Action::Up), false), "moving in the messages is a command");
        use super::note_clears;
        assert!(note_clears("left #general") && note_clears(""));
        assert!(!note_clears("uploading notes.txt…") && !note_clears("posting notes…"), "work still going on keeps its note");
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
    fn dnd_notes() {
        // The note after /dnd says what dnd.info says now.
        assert_eq!(super::dnd_note(true, None), "notifications resumed");
        assert_eq!(super::dnd_note(true, Some("09:00".into())), "notifications are still paused until 09:00", "never a silent resume");
        assert_eq!(super::dnd_note(false, Some("14:30".into())), "notifications paused until 14:30");
        assert_eq!(super::dnd_note(false, None), "notifications paused");
    }

    #[test]
    fn schedule_writes_go_in_order() {
        use slack_core::dnd::{ScheduleDays::*, ScheduleEdit::*};
        let mut q = super::ScheduleQueue::default();
        assert_eq!(q.push(vec![Days(EveryDay)], None), Some(vec![Days(EveryDay)]), "nothing in flight: written at once");
        assert_eq!(q.push(vec![Days(Weekdays)], None), None, "one in flight: it waits");
        assert_eq!(q.push(vec![Days(Custom)], None), None);
        assert_eq!(q.waiting().cloned().collect::<Vec<_>>(), [vec![Days(Weekdays)], vec![Days(Custom)]]);
        assert_eq!(q.done(true), Some(vec![Days(Weekdays)]), "then each in the order made");
        assert_eq!(q.done(true), Some(vec![Days(Custom)]));
        assert_eq!(q.done(true), None);
        assert_eq!(q.push(vec![Days(Off)], None), Some(vec![Days(Off)]), "idle again");
        assert_eq!(q.push(vec![Days(EveryDay)], None), None);
        assert_eq!(q.done(false), None, "after a failure what waits (made on top of it) is dropped");
        assert_eq!(q.waiting().count(), 0);
        assert_eq!(q.push(vec![Days(Off)], None), Some(vec![Days(Off)]));
    }

    #[test]
    fn a_failed_write_shows_slacks_copy_again() {
        use slack_core::dnd::{Schedule, ScheduleDays::*, ScheduleEdit::*};
        let weekdays = Schedule::from_prefs(&serde_json::json!({"dnd_enabled": true, "dnd_days": "weekdays", "dnd_end_hour": "10:00", "dnd_start_hour": "18:00"}));
        let every_day = Schedule { days: EveryDay, ..weekdays.clone() };
        let mut q = super::ScheduleQueue::default();
        assert_eq!(q.push(vec![Days(EveryDay)], Some(&weekdays)), Some(vec![Days(EveryDay)]));
        assert_eq!(q.last_read(), Some(&weekdays), "Slack's copy when the queue went busy");
        assert_eq!(q.push(vec![Days(Custom)], Some(&every_day)), None);
        assert_eq!(q.last_read(), Some(&weekdays), "edits shown on top of it don't count");
        // A write answered: Slack's copy read back is the one to show if a later write fails unread.
        q.read(Some(&every_day));
        assert_eq!(q.done(true), Some(vec![Days(Custom)]));
        assert_eq!(q.last_read(), Some(&every_day));
        assert_eq!(q.done(false), None);
        assert_eq!(q.push(vec![Days(Off)], None), Some(vec![Days(Off)]));
        assert_eq!(q.last_read(), None, "not known: nothing was shown");
    }

    #[test]
    fn slacks_echo_of_a_write_never_shows_the_previous_kind() {
        use slack_core::dnd::{ScheduleDays::*, ScheduleEdit::*};
        let mut ws = super::super::state::Workspace::default();
        ws.apply_prefs(&serde_json::json!({"dnd_enabled": true, "dnd_days": "weekdays", "dnd_end_hour": "9:00", "dnd_start_hour": "17:00"}));
        let mut q = super::ScheduleQueue::default();
        let before = ws.dnd.schedule.clone();
        let edits = q.push(vec![Days(EveryDay)], before.as_ref()).expect("written at once");
        ws.dnd.schedule = ws.dnd.schedule.as_ref().and_then(|s| s.edited(&edits).ok());
        q.push(vec![Days(Custom)], ws.dnd.schedule.as_ref());
        ws.dnd.schedule = ws.dnd.schedule.as_ref().and_then(|s| s.edited(&[Days(Custom)]).ok());
        // Slack echoes the first write a pref at a time, the kind last (as seen live 2026-10-20).
        for (name, value) in [("dnd_start_hour", "17:00"), ("dnd_end_hour", "9:00"), ("dnd_enabled_monday", "partial"), ("dnd_days", "every_day")] {
            super::super::live::apply(&mut ws, &serde_json::json!({"type": "pref_change", "name": name, "value": value}), None);
            super::show_pending_schedule(&mut ws, &q);
            assert_eq!(ws.dnd.schedule.as_ref().map(|s| s.days), Some(Custom), "after {name}: the latest change stays shown");
        }
        // Nothing of klack's in flight: another client's change shows as it is.
        assert_eq!(q.done(true), Some(vec![Days(Custom)]));
        assert_eq!(q.done(true), None);
        super::super::live::apply(&mut ws, &serde_json::json!({"type": "pref_change", "name": "dnd_days", "value": "weekdays"}), None);
        super::show_pending_schedule(&mut ws, &q);
        assert_eq!(ws.dnd.schedule.as_ref().map(|s| s.days), Some(Weekdays));
    }

    #[test]
    fn schedule_notes() {
        use slack_core::dnd::{ScheduleDays, Schedule};
        let weekdays = Schedule::from_prefs(&serde_json::json!({"dnd_enabled": true, "dnd_days": "weekdays", "dnd_end_hour": "10:00", "dnd_start_hour": "18:00"}));
        let say = |s: Option<&Schedule>, zone: &str| crate::tui::prefs::describe_schedule(s, "%H:%M", zone);
        assert_eq!(say(Some(&weekdays), ""), "weekdays, receive 10:00–18:00");
        let every_day = Schedule { days: ScheduleDays::EveryDay, ..weekdays.clone() };
        assert_eq!(say(Some(&every_day), " (Asia/Jerusalem)"), "every day, receive 10:00–18:00 (Asia/Jerusalem)");
        assert_eq!(say(Some(&Schedule { days: ScheduleDays::Off, ..weekdays.clone() }), ""), "off (notifications at any time)");
        assert!(say(Some(&Schedule { days: ScheduleDays::Custom, ..weekdays.clone() }), "").starts_with("custom"));
        assert!(say(Some(&Schedule { days: ScheduleDays::Unknown, ..weekdays.clone() }), "").contains("change it in Slack"));
        assert_eq!(say(None, ""), "not known yet");
        // After a change: the schedule, and that notifications are paused now if they are (any
        // schedule write brings back a quiet period that was ended early).
        assert_eq!(super::schedule_note(say(Some(&weekdays), ""), None), "notification schedule: weekdays, receive 10:00–18:00");
        assert_eq!(super::schedule_note(say(Some(&weekdays), ""), Some("Wed 10:00".into())), "notification schedule: weekdays, receive 10:00–18:00 · paused now, until Wed 10:00");
    }

    #[test]
    fn views_do_not_open_over_a_dialog() {
        use crate::tui::keymap::Action;
        assert!(super::opens_something(Action::Switcher) && super::opens_something(Action::Upload) && super::opens_something(Action::Details));
        assert!(super::opens_something(Action::HuddleThread) && super::opens_something(Action::HuddleView), "^␣ht / ^␣hv would open them under the dialog");
        assert!(!super::opens_something(Action::NotificationLevel) && !super::opens_something(Action::Mute) && !super::opens_something(Action::AwayActive));
        assert!(super::opens_something(Action::PauseFor) && !super::opens_something(Action::PauseNotifications), "␣aN opens the box, ␣an acts at once");
        // The note names the dialog's own close key.
        use crate::tui::keymap::{Context, Keymap};
        let keys = Keymap::load("").0;
        assert_eq!(super::close_first(&keys, Some(Context::Search)), "close the dialog first (Esc)");
        let rebound = Keymap::load("[search]\nclose = \"ctrl+g\"\n").0;
        assert_eq!(super::close_first(&rebound, Some(Context::Search)), "close the dialog first (Ctrl+G)");
        assert_eq!(super::close_first(&keys, Some(Context::Confirm)), "answer the question first");
    }

    #[test]
    fn no_prefill_during_a_reply_edit() {
        use super::{Wrap, prefill_box};
        use crate::tui::composer::Composer;
        let c = Composer::default();
        assert_eq!(prefill_box(&c, false, "/status ", Wrap::No), Ok(("/status ".into(), 8)));
        assert_eq!(prefill_box(&c, true, "/status ", Wrap::No), Err("finish or cancel the edit first"), "the reply box is editing a message");
        assert_eq!(prefill_box(&c, true, "/upload ", Wrap::Caption), Err("finish or cancel the edit first"));
    }

    #[test]
    fn which_key_leaves_out_what_does_not_apply() {
        use super::{Here, applies};
        use crate::tui::keymap::{Action::*, Context as C};
        use crate::tui::menu::Offers;
        let theirs = Offers { own: false, open: false, save: false, text: true };
        let here = Here { offers: Some(theirs), ..Default::default() };
        assert!(!applies(&here, C::Messages, Edit) && !applies(&here, C::Messages, Delete), "others' messages");
        assert!(!applies(&here, C::Messages, Open) && !applies(&here, C::Messages, Save), "nothing to open or save");
        assert!(applies(&here, C::Messages, Copy) && applies(&here, C::Messages, React) && applies(&here, C::Messages, OpenThread));
        assert!(!applies(&Here { offers: Some(Offers { text: false, ..theirs }), ..here }, C::Messages, Copy), "no text");
        assert!(!applies(&Here { offers: None, ..here }, C::Messages, Edit), "no message selected");
        assert!(!applies(&Here { in_thread: true, ..here }, C::Messages, OpenThread), "already in the thread");
        assert!(applies(&here, C::Messages, MarkUnread) && applies(&here, C::Global, MarkUnread));
        assert!(!applies(&Here { in_thread: true, ..here }, C::Messages, MarkUnread) && !applies(&Here { in_thread: true, ..here }, C::Global, MarkUnread), "not a thread's replies");
        assert!(!applies(&Here { offers: None, ..here }, C::Messages, MarkUnread) && !applies(&Here { outside: true, ..here }, C::Global, MarkUnread));
        assert!(!applies(&here, C::Global, JoinConversation) && applies(&Here { outside: true, ..here }, C::Global, JoinConversation), "join only from outside");
        assert!(!applies(&here, C::Global, LeaveConversation) && applies(&Here { leavable: true, ..here }, C::Global, LeaveConversation), "no leaving a DM");
        assert!(!applies(&here, C::Sidebar, LeaveConversation) && applies(&Here { selected_leavable: true, ..here }, C::Sidebar, LeaveConversation));
        assert!(!applies(&here, C::Sidebar, Star) && applies(&Here { selected: true, ..here }, C::Sidebar, Star), "a header or Threads is selected");
        assert!(!applies(&here, C::Profile, Huddle) && applies(&Here { card: true, ..here }, C::Profile, Huddle));
        let two = Here { workspaces: 2, ..here };
        assert!(applies(&two, C::Global, Workspace1) && applies(&two, C::Global, Workspace2), "the one you are in too");
        assert!(!applies(&two, C::Global, Workspace3) && !applies(&two, C::Global, Workspace9), "the profile has two");
        assert!(applies(&Here { workspaces: 1, ..two }, C::Global, Workspace1) && !applies(&Here { workspaces: 1, ..two }, C::Global, Workspace2), "one workspace: itself"); 
        assert!(applies(&here, C::Global, Mute) && applies(&here, C::Global, Huddle));
        let dialog = Here { dialog: true, ..here };
        assert!(!applies(&dialog, C::Global, Search) && !applies(&dialog, C::Global, HuddleView) && applies(&dialog, C::Global, Mute), "nothing opens under a dialog");
        assert!(!applies(&Here { leavable: true, ..dialog }, C::Global, LeaveConversation), "its y/n would open under the dialog");
    }

    #[test]
    fn page_keys_scroll_what_has_focus() {
        use super::{Focus, MessagePane};
        use ratatui::layout::Rect;
        let panes = || {
            let (mut m, mut t) = (MessagePane::default(), MessagePane::thread());
            (m.area, t.area) = (Rect::new(0, 0, 40, 23), Rect::new(40, 0, 40, 13));
            (m, t)
        };
        for focus in [Focus::Thread, Focus::ThreadComposer] {
            let (mut m, mut t) = panes();
            assert!(!super::scroll_page(focus, true, &mut m, &mut t, true));
            assert_eq!((m.offset(), t.offset()), (0, 10), "{focus:?}: PgUp moves the thread only");
            super::scroll_page(focus, true, &mut m, &mut t, false);
            assert_eq!((m.offset(), t.offset()), (0, 0), "{focus:?}: PgDn");
        }
        for focus in [Focus::Messages, Focus::Composer, Focus::Sidebar] {
            let (mut m, mut t) = panes();
            assert!(super::scroll_page(focus, true, &mut m, &mut t, true));
            assert_eq!((m.offset(), t.offset()), (20, 0), "{focus:?}: PgUp moves the messages only");
        }
        // A narrow terminal: the thread replaces the messages, so it pages whatever has focus.
        for focus in [Focus::Messages, Focus::Composer, Focus::Sidebar] {
            let (mut m, mut t) = panes();
            m.area = Rect::new(0, 0, 0, 0);
            assert!(!super::scroll_page(focus, true, &mut m, &mut t, true), "{focus:?}: no older-history load for a hidden pane");
            assert_eq!((m.offset(), t.offset()), (0, 10), "{focus:?}: PgUp pages the thread");
        }
        let (mut m, mut t) = panes();
        super::scroll_page(Focus::Thread, false, &mut m, &mut t, true);
        assert_eq!((m.offset(), t.offset()), (20, 0), "no thread open: the messages");
    }

    #[test]
    fn next_unread_never_lands_on_the_open_conversation() {
        let found = |s: &str| Some(s.to_string());
        assert_eq!(super::unread_jump(Some("C1"), found("C2")), Ok("C2".to_string()));
        assert_eq!(super::unread_jump(Some("C1"), found("C1")), Err("nothing else unread"));
        assert_eq!(super::unread_jump(None, found("C1")), Ok("C1".to_string()));
        assert_eq!(super::unread_jump(Some("C1"), None), Err("nothing unread"));
    }

    #[test]
    fn what_ends_a_key_sequence() {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let key = |kind| Event::Key(KeyEvent { kind, ..KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE) });
        let mouse = |kind| Event::Mouse(MouseEvent { kind, column: 3, row: 4, modifiers: KeyModifiers::NONE });
        // Windows reports releases: Space up must not cancel ␣ before the next key.
        assert!(!super::ends_sequence(&key(KeyEventKind::Release)));
        assert!(super::ends_sequence(&key(KeyEventKind::Press)) && super::ends_sequence(&key(KeyEventKind::Repeat)));
        // Hovering, focus and a resize leave it; a click, the wheel and a paste end it.
        for ev in [mouse(MouseEventKind::Moved), mouse(MouseEventKind::Drag(MouseButton::Left)), Event::FocusGained, Event::FocusLost, Event::Resize(80, 24)] {
            assert!(!super::ends_sequence(&ev), "{ev:?}");
        }
        for ev in [mouse(MouseEventKind::Down(MouseButton::Left)), mouse(MouseEventKind::Up(MouseButton::Left)), mouse(MouseEventKind::ScrollDown), mouse(MouseEventKind::ScrollUp), Event::Paste("x".into())] {
            assert!(super::ends_sequence(&ev), "{ev:?}");
        }
    }

    #[test]
    fn clicks_that_are_taken_first() {
        use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use ratatui::layout::Rect;
        use std::time::{Duration, Instant};
        let at = |kind, column, row| Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE });
        let (down, up, right) = (MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left), MouseEventKind::Down(MouseButton::Right));
        let t = Instant::now();
        let (closed, soon) = (Some((t, 10, 5)), t + Duration::from_millis(150));
        // The second half of a double-click whose first click closed a dialog: press and release.
        assert!(super::swallowed(&at(down, 10, 5), None, closed, soon) && super::swallowed(&at(up, 10, 5), None, closed, soon));
        assert!(!super::swallowed(&at(down, 10, 5), None, closed, t + super::DOUBLE_CLICK), "too late");
        assert!(!super::swallowed(&at(down, 11, 5), None, closed, soon) && !super::swallowed(&at(down, 10, 6), None, closed, soon), "another cell");
        assert!(!super::swallowed(&at(right, 10, 5), None, closed, soon), "another button");
        assert!(!super::swallowed(&at(down, 10, 5), None, None, soon), "no dialog closed by a click");
        // The which-key panel takes any click on it, not those beside it, nor hovering or the wheel.
        let panel = Some(Rect::new(50, 10, 20, 6));
        for (kind, c, r) in [(down, 50, 10), (up, 69, 15), (right, 60, 12)] {
            assert!(super::swallowed(&at(kind, c, r), panel, None, soon), "{kind:?} {c} {r}");
        }
        for (kind, c, r) in [(down, 49, 10), (down, 70, 15), (down, 60, 16), (MouseEventKind::Moved, 60, 12), (MouseEventKind::ScrollDown, 60, 12)] {
            assert!(!super::swallowed(&at(kind, c, r), panel, None, soon), "{kind:?} {c} {r}");
        }
        assert!(!super::swallowed(&Event::Paste("x".into()), panel, closed, soon));
    }

    fn session(team_id: &str, team: &str, user: &str) -> WorkspaceSession {
        WorkspaceSession { team_id: team_id.into(), team_name: team.into(), user_name: user.into(), ..Default::default() }
    }

    #[test]
    fn workspace_labels_name_the_account_when_a_team_repeats() {
        let labels = super::workspace_labels(&[session("T1", "acme", "ann"), session("T2", "beta", "ann"), session("T1", "acme", "bob")]);
        assert_eq!(labels, vec!["acme · ann", "beta", "acme · bob"]);
    }

    #[test]
    fn start_skips_signed_out() {
        use std::collections::HashSet;
        let sessions = [session("T1", "acme", "ann"), session("T2", "beta", "ann"), session("T3", "gamma", "ann")];
        assert_eq!(super::start_index(&sessions, 0, &HashSet::new()), Some(0));
        assert_eq!(super::start_index(&sessions, 2, &HashSet::from([0])), Some(2), "the wanted one works");
        assert_eq!(super::start_index(&sessions, 0, &HashSet::from([0])), Some(1), "else the first that does");
        assert_eq!(super::start_index(&sessions, 1, &HashSet::from([0, 1])), Some(2));
        assert_eq!(super::start_index(&sessions, 0, &HashSet::from([0, 1, 2])), None);
    }

    #[test]
    fn switching_to_a_signed_out_workspace_stays() {
        use std::collections::HashSet;
        let labels: Vec<String> = ["acme", "beta", "gamma"].map(String::from).into();
        let out = HashSet::from([1]);
        let note = Err("beta: signed out — klack auth login, then restart klack".to_string());
        assert_eq!(super::switch_check(1, 0, &labels, false, &out), note);
        assert_eq!(super::switch_check(1, 0, &labels, true, &out), note, "in a huddle too");
        assert_eq!(super::switch_check(2, 0, &labels, false, &out), Ok(2));
        assert_eq!(super::switch_check(2, 0, &labels, true, &out), Err("leave the huddle first".to_string()));
        assert_eq!(super::switch_check(0, 0, &labels, false, &out), Err("already in acme".to_string()));
        assert_eq!(super::switch_check(5, 0, &labels, false, &out), Err("there is no workspace 6 (this profile has 3)".to_string()));
    }

    #[test]
    fn the_account_asked_for_or_the_default() {
        let mut p = slack_core::session::Profile::default();
        for s in [session("T1", "acme", "ann"), session("T2", "beta", "ann"), WorkspaceSession { user_id: "U2".into(), ..session("T2", "beta", "bob") }] {
            p.upsert(s);
        }
        p.default_team = Some("T2".into());
        assert_eq!(super::wanted_index(&p, Some(("T2", "U2"))), 2, "the second account of beta");
        assert_eq!(super::wanted_index(&p, None), 1, "the default team's first account");
        // --team named an account removed at the start question: the default, as plain klack.
        assert_eq!(super::wanted_index(&p, Some(("T9", "U9"))), 1);
        p.default_team = None;
        assert_eq!(super::wanted_index(&p, Some(("T9", "U9"))), 0);
    }

    #[test]
    fn signed_out_only_klack_commands_run() {
        use super::super::commands::{Command, parse};
        let cmd = |text: &str| parse(text, text).unwrap().unwrap();
        assert!(super::runs_while_signed_out(&cmd("/workspace 2")));
        assert!(super::runs_while_signed_out(&Command::Workspace(None)));
        assert!(super::runs_while_signed_out(&Command::Prefs));
        assert!(!super::runs_while_signed_out(&cmd("/remind me later")), "Slack's commands need the session");
        assert!(!super::runs_while_signed_out(&cmd("/away")));
        assert!(!super::runs_while_signed_out(&cmd("/leave")));
    }

    #[test]
    fn the_signed_out_note_stays_over_routine_notes() {
        let sticky = super::signed_out_note("acme");
        assert_eq!(sticky, "acme: signed out — klack auth login, then restart klack");
        assert_eq!(super::shown_note(None, Some(&sticky), "already in acme"), Some(format!("already in acme · {sticky}")), "a routine note goes before it");
        assert_eq!(super::shown_note(None, Some(&sticky), ""), Some(sticky.clone()), "and when it has gone the note stays");
        assert_eq!(super::shown_note(None, Some(&sticky), &sticky), Some(sticky.clone()), "a refused send says it once");
        assert_eq!(super::shown_note(None, None, "copied the link"), Some("copied the link".to_string()));
        assert_eq!(super::shown_note(None, None, ""), None);
        assert_eq!(super::shown_note(Some("📞 ann invites you".into()), Some(&sticky), ""), Some("📞 ann invites you".to_string()));
    }

    #[test]
    fn conversation_link_form() {
        use super::conversation_link;
        assert_eq!(conversation_link("https://acme.slack.com/", "C0123"), "https://acme.slack.com/archives/C0123");
        assert_eq!(conversation_link("https://acme.slack.com", "D0123"), "https://acme.slack.com/archives/D0123");
    }

    #[test]
    fn mark_unread_ts_is_just_before() {
        use super::mark_unread_ts;
        let order: Vec<String> = ["1792357735.815229", "1792357790.000100", "1792357801.123456"].map(String::from).to_vec();
        assert_eq!(mark_unread_ts(&order, Some("1792357801.123456")).as_deref(), Some("1792357790.000100"), "the previous message");
        assert_eq!(mark_unread_ts(&order, Some("1792357790.000100")).as_deref(), Some("1792357735.815229"));
        assert_eq!(mark_unread_ts(&order, Some("1792357735.815229")).as_deref(), Some("1792357735.815228"), "the first loaded: a microsecond before it");
        assert_eq!(mark_unread_ts(&order, None).as_deref(), Some("1792357790.000100"), "no selection: from the latest");
        assert_eq!(mark_unread_ts(&order[..1], None).as_deref(), Some("1792357735.815228"), "the only message");
        assert_eq!(mark_unread_ts(&["1792357800.000000".to_string()], None).as_deref(), Some("1792357799.999999"), "across the second");
        assert_eq!(mark_unread_ts(&[], None), None, "nothing loaded: nothing to mark");
    }

    #[test]
    fn marking_unread_survives_the_view() {
        use super::{hold_unread, read_mark_target};
        use crate::tui::media::Media;
        use crate::tui::messages::MessagePane;
        use crate::tui::state::{Timeline, Workspace};
        use slack_core::types::{Conversation, Message, User};
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        ws.users.insert("U1".into(), User { id: "U1".into(), name: "ann".into(), ..Default::default() });
        ws.conversations.insert("D1".into(), Conversation { id: "D1".into(), is_im: true, user: Some("U1".into()), ..Default::default() });
        let ts = |i: usize| format!("{}.000100", 1_700_000_000 + i * 600);
        let mut t = Timeline { loaded: true, ..Default::default() };
        for i in 0..5 {
            t.upsert(Message { ts: ts(i), user: Some("U1".into()), text: format!("message {i}"), ..Default::default() });
        }
        ws.timelines.insert("D1".into(), t);
        // Open and read: marked up to the newest once.
        let mut marked = std::collections::HashMap::new();
        assert_eq!(read_mark_target(&ws, "D1", &marked), Some(ts(4)));
        marked.insert("D1".to_string(), ts(4));
        assert_eq!(read_mark_target(&ws, "D1", &marked), None, "once");
        // ␣cu on message 2: Slack's mark goes to message 1, and the conversation is unread.
        hold_unread(&mut ws, &mut marked, "D1", &ts(1));
        assert!(ws.unread["D1"].has_unreads, "unread (bold in the sidebar)");
        assert_eq!(ws.conversations["D1"].last_read, Some(ts(1)));
        assert!(ws.held_unread.contains("D1"));
        // Drawn with the line above message 2, scrolled to the bottom, drawn again: still not marked.
        let mut pane = MessagePane::default();
        (pane.new_since, pane.selected) = (Some(ts(1)), Some(ts(2)));
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 20)).unwrap();
        for _ in 0..2 {
            terminal.draw(|f| pane.draw(f, f.area(), &crate::tui::applink::Here::alone(&ws), "t", ws.timelines.get("D1"), true, &media)).unwrap();
            let buf = terminal.backend().buffer();
            let rows: Vec<String> = (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()).collect();
            let line = rows.iter().position(|r| r.contains(" new ")).expect("the new line");
            assert!(rows[line + 1..].iter().find(|r| r.contains("message")).is_some_and(|r| r.contains("message 2")), "above message 2: {rows:#?}");
            assert_eq!(read_mark_target(&ws, "D1", &marked), None, "the view does not mark it read");
            pane.to_bottom();
        }
        // A new message while it is open: still unread.
        ws.timelines.get_mut("D1").unwrap().upsert(Message { ts: ts(5), user: Some("U1".into()), text: "message 5".into(), ..Default::default() });
        assert_eq!(read_mark_target(&ws, "D1", &marked), None);
        // Left and opened again: marked read as usual, though the newest is the one marked before.
        ws.release_unread("D1");
        assert!(ws.unread["D1"].has_unreads, "leaving does not read it");
        ws.timelines.get_mut("D1").unwrap().messages.pop();
        assert_eq!(read_mark_target(&ws, "D1", &marked), Some(ts(4)), "the cache of what was marked is forgotten");
    }

    /// An app on a workspace that answers nothing (every request is refused at once), with the
    /// default keys; what its requests bring back arrives on the receiver.
    fn test_app() -> (super::App, tokio::sync::mpsc::UnboundedReceiver<super::Net>) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let session = WorkspaceSession { url: "http://127.0.0.1:9/".into(), team_id: "T0".into(), user_id: "U0".into(), ..Default::default() };
        let (net, rx) = tokio::sync::mpsc::unbounded_channel();
        let (rtm, _) = tokio::sync::mpsc::unbounded_channel();
        let mut app = super::App::new(super::Client::new(session).unwrap(), net, rtm, super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks()), Default::default());
        app.keys = super::Keymap::load("").0;
        (app, rx)
    }

    /// [`test_app`] whose Slack is at `url`.
    fn test_app_at(url: &str) -> (super::App, tokio::sync::mpsc::UnboundedReceiver<super::Net>) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let session = WorkspaceSession { url: url.into(), team_id: "T0".into(), user_id: "U0".into(), ..Default::default() };
        let (net, rx) = tokio::sync::mpsc::unbounded_channel();
        let (rtm, _) = tokio::sync::mpsc::unbounded_channel();
        let mut app = super::App::new(super::Client::new(session).unwrap(), net, rtm, super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks()), Default::default());
        app.keys = super::Keymap::load("").0;
        (app, rx)
    }

    /// A Slack on a local port that answers every call `ok` (a method in `answers` with its answer) and passes on
    /// each request it was sent (method and form).
    async fn fake_slack(answers: &'static [(&'static str, &'static str)]) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut req = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req).to_string();
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len = text[..h].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
                        if req.len() >= h + 4 + len {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&req).to_string();
                let body = answers.iter().find(|(method, _)| text.contains(&format!("/api/{method} "))).map_or(r#"{"ok":true}"#, |(_, answer)| *answer);
                let _ = tx.send(text);
                let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        (url, rx)
    }

    /// A call on the fake meeting `R9` as attendee `A9` (nothing connects; no speaker is opened).
    fn test_call(channel: &str) -> super::super::call::Call {
        let creds = huddle::signaling::Credentials::from_rooms_join(&serde_json::json!({"call": {"call_id": "R9", "free_willy": {
            "meeting": {"MeetingId": "M9", "MediaPlacement": {"AudioHostUrl": "x", "SignalingUrl": "ws://127.0.0.1:9/", "TurnControlUrl": null}},
            "attendee": {"AttendeeId": "A9", "ExternalUserId": "U0", "JoinToken": "t"}}}})).unwrap();
        let audio = super::super::settings::Audio::default();
        super::super::call::Call::start(channel.into(), creds, (None, Some("no such speaker".into())), &audio)
    }

    #[tokio::test]
    async fn i_leaving_a_huddle_tells_slack() {
        use super::{run_global, Action};
        let (url, mut asked) = fake_slack(&[]).await;
        let (mut app, _net) = test_app_at(&url);
        app.call = Some(test_call("C1"));
        run_global(&mut app, Action::Huddle);
        assert!(app.call.as_ref().unwrap().leaving);
        let req = tokio::time::timeout(std::time::Duration::from_secs(10), asked.recv()).await.expect("a request").unwrap();
        assert!(req.contains("/api/rooms.leave "), "{req}");
        for want in ["channel_id=C1", "call_id=R9", "attendee_id=A9", "reason=user_initiated"] {
            assert!(req.contains(want), "{want} in {req}");
        }
        // Pressing the key again while leaving does not send it twice.
        run_global(&mut app, Action::Huddle);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(300), asked.recv()).await.is_err(), "sent once");
    }

    #[tokio::test]
    async fn a_huddle_lost_for_good_tells_slack_we_left() {
        use huddle::session::SessionEvent;
        let (url, mut asked) = fake_slack(&[]).await;
        let (mut app, _net) = test_app_at(&url);
        // The session gave up joining again (tries used up, or too many reconnects in an hour).
        let mut call = test_call("C1");
        call.apply(SessionEvent::Connected);
        app.call = Some(call);
        app.huddle_ended();
        assert!(app.call.is_none());
        assert_eq!(app.status, "huddle connection lost — left the huddle");
        let req = asked_for(&mut asked, "rooms.leave").await;
        for want in ["channel_id=C1", "call_id=R9", "attendee_id=A9", "reason=user_initiated"] {
            assert!(req.contains(want), "{want} in {req}");
        }
        assert!(tokio::time::timeout(std::time::Duration::from_millis(300), asked.recv()).await.is_err(), "sent once");
        // Left by hand: Slack was told then, not again.
        let mut call = test_call("C1");
        call.apply(SessionEvent::Connected);
        call.leave();
        app.call = Some(call);
        app.huddle_ended();
        assert_eq!(app.status, "left the huddle");
        // Ended before connecting: there is nothing to leave.
        app.rejoined = true;
        app.call = Some(test_call("C1"));
        app.huddle_ended();
        assert!(tokio::time::timeout(std::time::Duration::from_millis(300), asked.recv()).await.is_err(), "nothing sent");
    }

    #[tokio::test]
    async fn the_call_bar_says_reconnecting_and_shows_no_live_level_meanwhile() {
        use huddle::session::SessionEvent;
        let (mut app, _net) = test_app();
        let mut call = test_call("C1");
        call.apply(SessionEvent::Connected);
        // (Only the state: no microphone is opened.)
        call.muted = false;
        app.call = Some(call);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 24)).unwrap();
        let mut bar = |app: &mut super::App| {
            terminal.draw(|f| super::draw(f, app)).unwrap();
            screen(&terminal).join("\n")
        };
        let live = bar(&mut app);
        assert!(live.contains("live ····") && !live.contains("reconnecting"), "{live}");
        app.call.as_mut().unwrap().apply(SessionEvent::Reconnecting);
        let lost = bar(&mut app);
        assert!(lost.contains("reconnecting…"), "{lost}");
        assert!(!lost.contains("····"), "no microphone level while nothing is sent: {lost}");
        assert!(!app.call.as_ref().unwrap().muted, "mute is left as it was");
        app.call.as_mut().unwrap().apply(SessionEvent::Connected);
        let back = bar(&mut app);
        assert!(back.contains("live ····") && !back.contains("reconnecting"), "{back}");
    }

    #[tokio::test]
    async fn i_replies_to_the_selected_thread() {
        use super::{run_global, Action, Focus, Net, View};
        use crossterm::event::{KeyCode, KeyModifiers};
        use slack_core::types::Message;
        let (url, mut asked) = fake_slack(&[("chat.postMessage", r#"{"ok":true,"channel":"C1","message":{"type":"message","ts":"1729450000.000100","thread_ts":"1729440000.000100","user":"U0","text":"on it"}}"#)]).await;
        let (mut app, mut net) = test_app_at(&url);
        app.ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "general".into(), is_channel: true, is_member: true, ..Default::default() });
        app.current = Some("C2".into());
        app.composer.insert_str("a draft for C2");
        run_global(&mut app, Action::Threads);
        let m = |ts: &str, parent: &str, text: &str| Message { ts: ts.into(), thread_ts: Some(parent.into()), user: Some("U1".into()), text: text.into(), ..Default::default() };
        let t1 = super::super::threads::Entry { channel: "C1".into(), root: Message { reply_count: 1, ..m("1729440000.000100", "1729440000.000100", "the parent") }, latest: vec![m("1729440000.000200", "1729440000.000100", "a reply")], ..Default::default() };
        let t2 = super::super::threads::Entry { channel: "C3".into(), root: m("1729440500.000100", "1729440500.000100", "another"), ..Default::default() };
        app.threads.add_page(vec![t1, t2], false);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        // The second message of the feed: the reply in C1's thread.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.threads.selected, Some(("C1".into(), "1729440000.000200".into())));
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!((app.focus, app.view), (Focus::Composer, View::Threads), "the box under the feed, the feed stays");
        assert_eq!(app.feed_reply, Some(("C1".to_string(), "1729440000.000100".to_string())));
        assert_eq!(app.composer.text(), "", "C2's draft is not the thread's");
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        assert_eq!(app.view, View::Threads, "typing there keeps the view");
        let screen: Vec<String> = { let buf = terminal.backend().buffer(); (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect() };
        assert!(screen.iter().any(|l| l.contains("Reply in #general thread")), "{screen:#?}");
        assert!(app.composer_area.y > app.threads.area.y && app.composer_area.width == app.threads.area.width, "under the feed");
        for c in "on it".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        // It is posted in the thread.
        let post = loop {
            let req = tokio::time::timeout(std::time::Duration::from_secs(10), asked.recv()).await.expect("a request").unwrap();
            if req.contains("chat.postMessage") {
                break req;
            }
        };
        assert!(post.contains("channel=C1") && post.contains("thread_ts=1729440000.000100") && post.contains("text=on+it"), "{post}");
        // The block shows it once Slack has it.
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(10), net.recv()).await.expect("the answer").unwrap() {
                msg @ Net::Posted { .. } => {
                    super::handle_net(&mut app, msg);
                    break;
                }
                _ => {}
            }
        }
        let e = &app.threads.entries.as_ref().unwrap()[0];
        assert!(e.latest.iter().any(|r| r.ts == "1729450000.000100") && e.root.reply_count == 2 && e.read, "the reply is in its block, the thread read");
        // Back from the view: the conversation's own draft again, the thread's kept.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!((app.view, app.feed_reply.clone()), (View::Conversation, None));
        assert_eq!(app.composer.text(), "a draft for C2");
    }

    #[tokio::test]
    async fn a_box_filled_with_nothing_selected_in_the_threads_view_keeps_the_conversations_draft() {
        use super::{run_global, Action, View, Wrap};
        let (mut app, _net) = test_app();
        app.current = Some("C2".into());
        app.composer.insert_str("a draft for C2");
        run_global(&mut app, Action::Threads);
        assert!(app.composer.is_empty() && app.threads.selected_thread().is_none(), "the draft kept away; the feed not loaded: nothing selected");
        // A command put in the box under the feed (it replies nowhere yet), then the view closes.
        app.prefill("/status ", Wrap::No);
        assert_eq!(app.composer.text(), "/status ");
        app.set_view(View::Conversation);
        assert_eq!(app.composer.text(), "/status \na draft for C2", "both: what was typed, the conversation's draft below");
        assert_eq!(app.composer.cursor(), 8, "the cursor where it was");
        // And it is kept under the conversation from now on.
        app.set_view(View::Threads);
        app.set_view(View::Conversation);
        assert_eq!(app.composer.text(), "/status \na draft for C2");
        app.composer.clear();
        app.set_view(View::Threads);
    }

    /// An app on the Threads view at `url` with two threads: T1 in #general (parent + a reply), T2 in #random.
    fn feed_app(url: &str) -> (super::App, tokio::sync::mpsc::UnboundedReceiver<super::Net>) {
        use slack_core::types::Message;
        let (mut app, net) = test_app_at(url);
        for (id, name) in [("C1", "general"), ("C3", "random")] {
            app.ws.conversations.insert(id.into(), slack_core::types::Conversation { id: id.into(), name: name.into(), is_channel: true, is_member: true, ..Default::default() });
        }
        app.current = Some("C2".into());
        app.sidebar.rebuild(&app.ws);
        super::run_global(&mut app, super::Action::Threads);
        let m = |ts: &str, parent: &str, text: &str| Message { ts: ts.into(), thread_ts: Some(parent.into()), user: Some("U1".into()), text: text.into(), ..Default::default() };
        let t1 = super::super::threads::Entry { channel: "C1".into(), root: Message { reply_count: 1, ..m("1729440000.000100", "1729440000.000100", "the parent") }, latest: vec![m("1729440000.000200", "1729440000.000100", "a reply")], ..Default::default() };
        let t2 = super::super::threads::Entry { channel: "C3".into(), root: m("1729440500.000100", "1729440500.000100", "another"), ..Default::default() };
        app.threads.add_page(vec![t1, t2], false);
        (app, net)
    }

    fn screen(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> Vec<String> {
        let buf = terminal.backend().buffer();
        (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect()
    }

    /// The next request to the fake Slack for `method`.
    async fn asked_for(asked: &mut tokio::sync::mpsc::UnboundedReceiver<String>, method: &str) -> String {
        loop {
            let req = tokio::time::timeout(std::time::Duration::from_secs(10), asked.recv()).await.expect("a request").unwrap();
            if req.contains(&format!("/api/{method} ")) {
                return req;
            }
        }
    }

    #[tokio::test]
    async fn edits_deletions_and_reactions_show_in_the_feed_live() {
        use slack_core::rtm::RtmEvent;
        let (url, _asked) = fake_slack(&[]).await;
        let (mut app, _net) = feed_app(&url);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        let mut shown = |app: &mut super::App| {
            terminal.draw(|f| super::draw(f, app)).unwrap();
            screen(&terminal).join("\n")
        };
        assert!(shown(&mut app).contains("a reply"));
        // Someone reacts to the reply in T1 and edits it (the edit leaves the reactions out).
        super::handle_rtm(&mut app, RtmEvent::Event(serde_json::json!({"type": "reaction_added", "user": "U2", "reaction": "eyes", "item": {"type": "message", "channel": "C1", "ts": "1729440000.000200"}})));
        super::handle_rtm(&mut app, RtmEvent::Event(serde_json::json!({"type": "message", "subtype": "message_changed", "channel": "C1", "message": {"type": "message", "user": "U1", "text": "a reply, better", "ts": "1729440000.000200", "thread_ts": "1729440000.000100", "edited": {"user": "U1", "ts": "1729440100.000000"}}})));
        let now = shown(&mut app);
        assert!(now.contains("a reply, better (edited)") && now.contains("👀"), "{now}");
        // Your own reaction comes back from Slack the same way, once (an echo twice counts once).
        let me = app.ws.self_id.clone();
        for _ in 0..2 {
            super::handle_rtm(&mut app, RtmEvent::Event(serde_json::json!({"type": "reaction_added", "user": me, "reaction": "eyes", "item": {"type": "message", "channel": "C1", "ts": "1729440000.000200"}})));
        }
        let pill = |app: &super::App| app.threads.entries.as_ref().unwrap()[0].latest[0].reactions.iter().map(|r| (r.name.clone(), r.count)).collect::<Vec<_>>();
        assert_eq!(pill(&app), [("eyes".to_string(), 2)]);
        super::handle_rtm(&mut app, RtmEvent::Event(serde_json::json!({"type": "reaction_removed", "user": me, "reaction": "eyes", "item": {"type": "message", "channel": "C1", "ts": "1729440000.000200"}})));
        assert_eq!(pill(&app), [("eyes".to_string(), 1)]);
        // T2's parent is deleted: its block says so, in its place.
        super::handle_rtm(&mut app, RtmEvent::Event(serde_json::json!({"type": "message", "subtype": "message_deleted", "channel": "C3", "deleted_ts": "1729440500.000100", "previous_message": {"ts": "1729440500.000100"}})));
        let now = shown(&mut app);
        assert!(now.contains("message not available") && !now.contains("another"), "{now}");
        assert!(now.find("#general").unwrap() < now.find("#random").unwrap(), "the order kept");
    }

    #[tokio::test]
    async fn i_on_the_thread_open_beside_types_in_its_box() {
        use super::Focus;
        use crossterm::event::{KeyCode, KeyModifiers};
        let (url, _asked) = fake_slack(&[]).await;
        let (mut app, _net) = feed_app(&url);
        // T1 is open in the thread pane beside the feed.
        app.thread = Some(("C1".into(), "1729440000.000100".into()));
        app.thread_composer.insert_str("half a reply");
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        app.focus = Focus::Messages;
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!((app.focus, app.feed_reply.clone()), (Focus::ThreadComposer, None), "its own box, not a second one on its draft");
        assert_eq!(app.thread_composer.text(), "half a reply");
        // Another thread: the box under the feed.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.focus = Focus::Messages;
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!((app.focus, app.feed_reply.clone()), (Focus::Composer, Some(("C3".to_string(), "1729440500.000100".to_string()))));
    }

    #[tokio::test]
    async fn the_reply_target_follows_the_selection() {
        use super::Focus;
        use crossterm::event::{KeyCode, KeyModifiers};
        let (url, mut asked) = fake_slack(&[]).await;
        let (mut app, _net) = feed_app(&url);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        // T1, `i`, Esc with nothing typed: no thread chosen any more.
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!(app.feed_reply, Some(("C1".to_string(), "1729440000.000100".to_string())));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!((app.focus, app.feed_reply.clone()), (Focus::Messages, None));
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        // T2 selected, then the box gets the focus another way: Type on the sidebar's Threads row.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.threads.selected_thread(), Some(("C3".to_string(), "1729440500.000100".to_string())));
        app.focus = Focus::Sidebar;
        assert!(app.sidebar.on_view().is_some(), "on the Threads row");
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!((app.focus, app.feed_reply.clone()), (Focus::Composer, Some(("C3".to_string(), "1729440500.000100".to_string()))));
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).iter().any(|l| l.contains("Reply in #random thread")), "{:#?}", screen(&terminal));
        for c in "ok".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let post = asked_for(&mut asked, "chat.postMessage").await;
        assert!(post.contains("channel=C3") && post.contains("thread_ts=1729440500.000100"), "{post}");
    }

    #[tokio::test]
    async fn a_block_short_of_replies_fetches_the_newest_once() {
        use super::Net;
        let (url, mut asked) = fake_slack(&[("conversations.replies", r#"{"ok":true,"has_more":true,"messages":[{"type":"message","ts":"1729440000.000100","thread_ts":"1729440000.000100","user":"U1","text":"the parent","reply_count":5},{"type":"message","ts":"1729440000.000300","thread_ts":"1729440000.000100","user":"U2","text":"reply three"},{"type":"message","ts":"1729440000.000400","thread_ts":"1729440000.000100","user":"U2","text":"reply four"},{"type":"message","ts":"1729440000.000500","thread_ts":"1729440000.000100","user":"U2","text":"reply five"}]}"#)]).await;
        let (mut app, mut net) = feed_app(&url);
        app.threads.entries.as_mut().unwrap()[0].root.reply_count = 5;
        app.threads.add_page(vec![], false);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        app.request_feed_replies();
        app.request_feed_replies();
        let req = asked_for(&mut asked, "conversations.replies").await;
        assert!(req.contains("channel=C1") && req.contains("ts=1729440000.000100") && req.contains("limit=4"), "{req}");
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(10), net.recv()).await.expect("the answer").unwrap() {
                msg @ Net::FeedReplies { .. } => {
                    super::handle_net(&mut app, msg);
                    break;
                }
                _ => {}
            }
        }
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        let shown = screen(&terminal).join("\n");
        assert!(["reply three", "reply four", "reply five", "2 more replies"].iter().all(|w| shown.contains(w)), "{shown}");
        assert!(!shown.contains("a reply\n"), "the older one it had is hidden now");
        // Asked once only.
        app.request_feed_replies();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let mut again = 0;
        while let Ok(req) = asked.try_recv() {
            again += usize::from(req.contains("/api/conversations.replies "));
        }
        assert_eq!(again, 0);
    }

    fn press(app: &mut super::App, code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) {
        assert!(super::handle_event(app, crossterm::event::Event::Key(crossterm::event::KeyEvent::new(code, mods))));
    }

    fn click(app: &mut super::App, column: u16, row: u16) {
        use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        assert!(super::handle_event(app, Event::Mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column, row, modifiers: KeyModifiers::NONE })));
    }

    #[tokio::test]
    async fn ctrl_t_opens_the_threads_view_and_esc_goes_back() {
        use super::{run_global, Action, Context, Focus, Mode, Net, View};
        use crossterm::event::{KeyCode, KeyModifiers};
        let (mut app, mut net) = test_app();
        app.current = Some("C1".into());
        app.focus = Focus::Sidebar;
        assert!(run_global(&mut app, Action::Threads));
        assert_eq!((app.view, app.mode(), app.focus), (View::Threads, Mode::Normal, Focus::Messages), "a place in the main pane, not a dialog");
        assert_eq!(app.active_contexts()[0], Context::Threads, "its keys");
        // Esc: back to the conversation it was in.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!((app.view, app.current.as_deref(), app.focus), (View::Conversation, Some("C1"), Focus::Messages));
        assert_eq!(app.active_contexts()[0], Context::Messages);
        // Ctrl+T again, then its key in the view: still there, loaded afresh.
        run_global(&mut app, Action::Threads);
        app.threads.selected = Some(("C1".into(), "3.0".into()));
        press(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(app.view, View::Threads);
        assert!(app.threads.loading && app.threads.entries.is_none() && app.threads.selected.is_none(), "refreshed");
        // Each opening asked for the first page.
        for n in 0..3 {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(10), net.recv()).await.expect("a request");
            assert!(matches!(msg, Some(Net::ThreadView { more: false, .. })), "request {n}");
        }
    }

    #[tokio::test]
    async fn the_huddles_view_lists_the_live_ones_and_goes_there() {
        use super::{Context, Focus, Net, View};
        use crossterm::event::{KeyCode, KeyModifiers};
        let (mut app, mut net) = test_app();
        for id in ["C1", "C2"] {
            app.ws.conversations.insert(id.into(), slack_core::types::Conversation { id: id.into(), name: format!("chan{id}"), is_channel: true, is_member: true, ..Default::default() });
        }
        let now = chrono::Utc::now().timestamp();
        app.ws.huddles.insert("C2".into(), super::super::state::Huddle { room: "R1".into(), participants: vec!["U2".into()], thread: None, start: now - 120 });
        app.current = Some("C1".into());
        app.sidebar.rebuild(&app.ws);
        app.sidebar.select_id(&app.ws, "C1");
        app.focus = Focus::Messages;
        // ␣fh opens it, in place of the conversation.
        for c in [' ', 'f', 'h'] {
            assert!(super::handle_terminal(&mut app, crossterm::event::Event::Key(crossterm::event::KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))));
        }
        assert_eq!((app.view, app.focus, app.active_contexts()[0]), (View::Huddles, Focus::Messages, Context::Huddles));
        assert!(app.huddles.loading, "the recent ones are being loaded");
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        let rows: Vec<String> = { let buf = terminal.backend().buffer(); (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect() };
        assert!(rows[0].chars().skip(20).collect::<String>().starts_with("┌ Huddles "), "{rows:#?}");
        assert!(rows[23].starts_with(" HUDDLES "), "its pill: {:?}", rows[23]);
        assert!(rows[3].contains("Huddles") && rows[3].contains(" 1 "), "the sidebar row with its badge: {rows:#?}");
        assert!(rows.iter().any(|r| r.contains("● Live  #chanC2 · 2 min")), "{rows:#?}");
        // Esc: back. Then Enter on the live one: its conversation.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!((app.view, app.current.as_deref()), (View::Conversation, Some("C1")));
        super::run_global(&mut app, super::Action::Huddles);
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        app.sync_current();
        assert_eq!((app.view, app.current.as_deref()), (View::Conversation, Some("C2")));
        // The huddle key there joins the live one selected (not the conversation under the view).
        app.sidebar.select_id(&app.ws, "C1");
        app.sync_current();
        super::run_global(&mut app, super::Action::Huddles);
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        super::run_global(&mut app, super::Action::Huddle);
        assert_eq!((app.view, app.status.as_str()), (View::Conversation, "joining the huddle…"));
        let mut joined = None;
        while let Ok(Some(msg)) = tokio::time::timeout(std::time::Duration::from_secs(10), net.recv()).await {
            if let Net::HuddleJoined { channel, .. } = msg {
                joined = Some(channel);
                break;
            }
        }
        assert_eq!(joined.as_deref(), Some("C2"));
    }

    /// `app` with person `id` (no status) in a DM `D<id>` the sidebar shows.
    fn with_dm(app: &mut super::App, id: &str) {
        app.ws.settings.appearance.hide_inactive_days = 0;
        app.ws.users.insert(id.into(), slack_core::types::User { id: id.into(), name: id.to_lowercase(), ..Default::default() });
        let dm = format!("D{id}");
        app.ws.conversations.insert(dm.clone(), slack_core::types::Conversation { id: dm, is_im: true, is_member: true, user: Some(id.into()), ..Default::default() });
        app.sidebar.rebuild(&app.ws);
    }

    #[tokio::test]
    async fn a_pushed_profile_change_looks_that_person_up_once() {
        use super::Net;
        use slack_core::rtm::RtmEvent;
        let (url, mut asked) = fake_slack(&[("users.info", r#"{"ok":true,"user":{"id":"U2","name":"u2","profile":{"status_emoji":":palm_tree:","status_text":"away"}}}"#)]).await;
        let (mut app, mut net) = test_app_at(&url);
        // The subscription frames go somewhere.
        let (rtm, _frames) = tokio::sync::mpsc::unbounded_channel();
        app.rtm_commands = rtm;
        with_dm(&mut app, "U2");
        app.ws.users.insert("U3".into(), slack_core::types::User { id: "U3".into(), ..Default::default() });
        super::handle_rtm(&mut app, RtmEvent::Connected(slack_core::api::Socket::Gateway));
        assert!(app.followed.0.contains("U2") && app.lookups.due.is_empty(), "followed; nothing looked up at the start");
        // The gateway pushes Slack's own client gets: someone's profile changed, their id only.
        let invalidated = |id: &str| RtmEvent::Event(serde_json::json!({"type": "user_invalidated", "event_ts": "1.5", "user": {"id": id}}));
        super::handle_rtm(&mut app, invalidated("U2"));
        super::handle_rtm(&mut app, invalidated("U2"));
        // Someone not shown: looked up only once shown.
        super::handle_rtm(&mut app, invalidated("U3"));
        assert_eq!(app.lookups.due.iter().collect::<Vec<_>>(), ["U2"], "once");
        assert!(app.lookups.stale.contains("U3"));
        app.look_up_people();
        assert!(app.lookups.busy && app.lookups.due.is_empty());
        let profiles = loop {
            match tokio::time::timeout(std::time::Duration::from_secs(10), net.recv()).await {
                Ok(Some(Net::Profiles(users))) => break users,
                Ok(Some(_)) => {}
                other => panic!("no lookup: {}", other.is_ok()),
            }
        };
        let mut looked_up = Vec::new();
        while let Ok(req) = asked.try_recv() {
            if req.contains("/api/users.info ") {
                looked_up.push(req.split("user=").nth(1).map(|v| v.chars().take(2).collect::<String>()).unwrap_or_default());
            }
        }
        assert_eq!(looked_up, ["U2"], "one users.info, for the person who changed");
        super::handle_net(&mut app, Net::Profiles(profiles));
        assert!(!app.lookups.busy);
        assert_eq!(app.ws.users["U2"].profile.status_emoji, ":palm_tree:");
        // U3 shows up (a DM): now looked up.
        with_dm(&mut app, "U3");
        app.follow_profiles(std::time::Instant::now() + std::time::Duration::from_secs(5));
        assert!(app.lookups.due.contains("U3") && app.lookups.stale.is_empty());
    }

    #[tokio::test]
    async fn a_conversation_learned_for_a_link_builds_the_views_again() {
        let (mut app, _net) = test_app();
        app.ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "old".into(), is_channel: true, is_member: true, ..Default::default() });
        let look = app.ws.look;
        let c = slack_core::types::Conversation { id: "C1".into(), name: "new".into(), is_channel: true, is_member: true, ..Default::default() };
        super::handle_net(&mut app, super::Net::LinkConversation { result: Ok(c), target: super::super::applink::Target::Conversation { channel: "C1".into() } });
        assert_eq!(app.ws.conversations["C1"].name, "new");
        assert!(app.ws.look > look, "what shows its name is built again");
    }

    #[tokio::test]
    async fn the_people_follow_pass_runs_only_when_who_is_shown_can_change() {
        use slack_core::rtm::RtmEvent;
        let (mut app, _net) = test_app();
        let (rtm, _frames) = tokio::sync::mpsc::unbounded_channel();
        app.rtm_commands = rtm;
        with_dm(&mut app, "U2");
        super::handle_rtm(&mut app, RtmEvent::Connected(slack_core::api::Socket::Gateway));
        let first = app.follow_passes;
        assert!(app.followed.0.contains("U2") && first == 1);
        let later = |s: u64| std::time::Instant::now() + std::time::Duration::from_secs(s);
        // Nothing that shows people changed (presence, typing, the clock): no pass.
        super::handle_rtm(&mut app, RtmEvent::Event(serde_json::json!({"type": "presence_change", "user": "U2", "presence": "away"})));
        super::handle_rtm(&mut app, RtmEvent::Event(serde_json::json!({"type": "user_typing", "channel": "DU2", "user": "U2"})));
        app.follow_profiles(later(5));
        app.follow_profiles(later(10));
        assert_eq!(app.follow_passes, first, "not run again");
        // A message from someone new in a loaded conversation: followed at the next pass.
        let mut t = super::super::state::Timeline::default();
        t.upsert(slack_core::types::Message { ts: "1.000100".into(), user: Some("U4".into()), ..Default::default() });
        app.ws.timelines.insert("C1".into(), t);
        app.follow_profiles(later(15));
        assert!(app.followed.0.contains("U4"), "followed");
        assert_eq!(app.follow_passes, first + 1);
        // Still at most one pass every couple of seconds.
        app.ws.timelines.get_mut("C1").unwrap().upsert(slack_core::types::Message { ts: "2.000100".into(), user: Some("U5".into()), ..Default::default() });
        app.follow_profiles(later(16));
        assert!(!app.followed.0.contains("U5"), "not yet");
        app.follow_profiles(later(18));
        assert!(app.followed.0.contains("U5"));
        // A group DM whose people come to be known.
        app.ws.conversations.insert("G1".into(), slack_core::types::Conversation { id: "G1".into(), is_mpim: true, name: "mpdm-u0--u6-1".into(), ..Default::default() });
        app.follow_profiles(later(21));
        assert!(!app.followed.0.contains("U6"), "not known yet");
        app.ws.put_user(slack_core::types::User { id: "U6".into(), name: "u6".into(), ..Default::default() });
        app.follow_profiles(later(24));
        assert!(app.followed.0.contains("U6"));
    }

    /// The next request to Slack's `method` (others are passed over), within 10 s.
    async fn next_call(asked: &mut tokio::sync::mpsc::UnboundedReceiver<String>, method: &str) -> Option<String> {
        let wait = async {
            while let Some(req) = asked.recv().await {
                if req.contains(&format!("/api/{method} ")) {
                    return Some(req);
                }
            }
            None
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), wait).await.ok().flatten()
    }

    #[tokio::test]
    async fn the_activity_count_follows_slacks_pushes() {
        use super::{run_global, Action, Net, View};
        use slack_core::rtm::RtmEvent;
        let (url, mut asked) = fake_slack(&[("client.counts", r#"{"ok":true,"activity_v2":{"at_user":0,"dm":5,"message_reaction":1,"thread_v2":0}}"#), ("activity.feed", r#"{"ok":true,"items":[]}"#)]).await;
        let (mut app, mut net) = test_app_at(&url);
        // At start: Slack's own badge, from `client.counts` (no feed fetched for it).
        let counts = serde_json::json!({"activity_v2": {"at_user": 1, "dm": 1, "message_reaction": 2, "thread_v2": 1}});
        super::handle_net(&mut app, Net::Boot { boot: serde_json::json!({}), users: vec![], counts: Some(counts) });
        assert_eq!(app.ws.activity_unread, 3);
        // Pushed: higher (B mentions A), lower (read elsewhere).
        let badge = |n: u64| RtmEvent::Event(serde_json::json!({"type": "badge_counts_updated", "event_ts": "9.0", "activity_v2": {"at_user": n, "dm": 1, "thread_v2": 1}}));
        super::handle_rtm(&mut app, badge(2));
        assert_eq!(app.ws.activity_unread, 4);
        // The view's list (with its own unread reactions) does not change the badge.
        run_global(&mut app, Action::Activity);
        let feed = serde_json::json!({"items": [{"feed_ts": "5.0", "is_unread": true, "key": "r1", "item": {"type": "message_reaction", "message": {"channel": "C1", "ts": "4.0"}, "reaction": {"name": "eyes", "user": "U2"}}}]});
        super::handle_net(&mut app, Net::ActivityFeed { tab: super::super::activity::Tab::All, more: false, result: Ok(feed) });
        assert_eq!(app.ws.activity_unread, 4);
        // A new item pushed: on top of the view.
        let mention = serde_json::json!({"type": "activity", "subtype": "activity_updated", "key": "at_user-C1-9.5", "event_ts": "9.5",
            "entry": {"feed_ts": "9.5", "is_archived": false, "is_unread": true, "key": "at_user-C1-9.5", "version": "1", "item": {"type": "at_user", "message": {"author_user_id": "U2", "channel": "C1", "is_broadcast": false, "ts": "9.5"}}}});
        super::handle_rtm(&mut app, RtmEvent::Event(mention));
        assert_eq!(app.activity.items.as_ref().map(|l| l.iter().map(|i| i.key.as_str()).collect::<Vec<_>>()), Some(vec!["at_user-C1-9.5", "r1"]));
        // Opened: marked read on Slack as its own client does, one less at once.
        app.activity.selected = 0;
        app.open_activity_item();
        let req = next_call(&mut asked, "activity.markRead").await.expect("activity.markRead");
        for want in ["type=at_user", "feed_ts=9.5", "key=at_user-C1-9.5"] {
            assert!(req.contains(want), "{want} in {req}");
        }
        assert_eq!((app.ws.activity_unread, app.view), (3, View::Conversation));
        // Slack drops its items (`activity_invalidated`) while the view is shown: loaded again.
        run_global(&mut app, Action::Activity);
        assert!(next_call(&mut asked, "activity.feed").await.is_some());
        super::handle_rtm(&mut app, RtmEvent::Event(serde_json::json!({"type": "activity", "subtype": "activity_invalidated", "event_ts": "9.8"})));
        assert!(next_call(&mut asked, "activity.feed").await.is_some(), "loaded again");
        // A reconnect: the count is asked for again, as Slack's client does (`client.counts`).
        super::handle_rtm(&mut app, RtmEvent::Connected(slack_core::api::Socket::Gateway));
        assert!(tokio::time::timeout(std::time::Duration::from_millis(300), next_call(&mut asked, "client.counts")).await.is_err(), "not at the first connect: the boot asked");
        super::handle_rtm(&mut app, RtmEvent::Disconnected("gone".into()));
        super::handle_rtm(&mut app, RtmEvent::Connected(slack_core::api::Socket::Gateway));
        assert!(next_call(&mut asked, "client.counts").await.is_some());
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(10), net.recv()).await {
                Ok(Some(msg @ Net::ActivityBadge { .. })) => {
                    super::handle_net(&mut app, msg);
                    break;
                }
                Ok(Some(_)) => {}
                other => panic!("no counts: {}", other.is_ok()),
            }
        }
        assert_eq!(app.ws.activity_unread, 5);
    }

    #[tokio::test]
    async fn the_badges_other_items_open_and_count_as_slacks_own() {
        use super::{run_global, Action, Net, View};
        use slack_core::rtm::RtmEvent;
        let (url, mut asked) = fake_slack(&[("client.counts", r#"{"ok":true,"activity_v2":{"at_user":7}}"#), ("activity.feed", r#"{"ok":true,"items":[]}"#)]).await;
        let (mut app, mut net) = test_app_at(&url);
        app.ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "c".into(), is_channel: true, is_member: true, ..Default::default() });
        let counts = serde_json::json!({"activity_v2": {"at_user": 1, "channel": 1, "list_record_assigned": 1, "message_reaction": 3}});
        super::handle_net(&mut app, Net::Boot { boot: serde_json::json!({}), users: vec![], counts: Some(counts) });
        assert_eq!(app.ws.activity_unread, 3, "a channel's and a list's items count, as in Slack");
        let entry = |ty: &str, key: &str, feed_ts: &str, item: serde_json::Value| {
            let mut item = item;
            item["type"] = ty.into();
            serde_json::json!({"feed_ts": feed_ts, "is_unread": true, "key": key, "item": item})
        };
        let feed = serde_json::json!({"items": [
            entry("channel", "channel-C1", "9.0", serde_json::json!({"bundle_info": {"payload": {"channel_entry": {"latest_message": {"channel": "C1", "ts": "9.0"}, "unread_msg_count": 1}}}})),
            entry("list_record_assigned", "list-1", "8.0", serde_json::json!({"list_payload": {"list_id": "F1", "row_id": "R1", "assigned_by": "U2"}})),
            entry("at_user", "", "7.0", serde_json::json!({"message": {"channel": "C1", "ts": "7.0"}})),
        ]});
        let show = |app: &mut super::App| {
            run_global(app, Action::Activity);
            super::handle_net(app, Net::ActivityFeed { tab: super::super::activity::Tab::All, more: false, result: Ok(feed.clone()) });
        };
        show(&mut app);
        assert_eq!(app.activity.visible().len(), 3, "listed in All");
        // A list: nowhere to go in klack. It is read now (on Slack too); the view stays and says where to open it.
        app.activity.selected = 1;
        app.open_activity_item();
        let req = next_call(&mut asked, "activity.markRead").await.expect("activity.markRead");
        for want in ["type=list_record_assigned", "key=list-1", "feed_ts=8.0", "ts=8.0"] {
            assert!(req.contains(want), "{want} in {req}");
        }
        assert_eq!((app.view, app.ws.activity_unread), (View::Activity, 2));
        assert!(app.status.contains("Slack"), "{}", app.status);
        // Nothing to mark it read with on Slack (no key): no call, and the badge keeps it (Slack still counts it).
        app.activity.selected = 2;
        app.open_activity_item();
        assert_eq!((app.view, app.ws.activity_unread), (View::Conversation, 2));
        // A channel's new messages: marked read; the channel opens.
        show(&mut app);
        app.activity.selected = 0;
        app.open_activity_item();
        let req = next_call(&mut asked, "activity.markRead").await.expect("activity.markRead");
        assert!(req.contains("type=channel") && req.contains("key=channel-C1"), "{req}");
        assert_eq!((app.view, app.ws.activity_unread, app.ws.shown.as_deref()), (View::Conversation, 1, Some("C1")));
        // A reconnect's counts answered after a newer push: dropped.
        super::handle_rtm(&mut app, RtmEvent::Connected(slack_core::api::Socket::Gateway));
        super::handle_rtm(&mut app, RtmEvent::Disconnected("gone".into()));
        super::handle_rtm(&mut app, RtmEvent::Connected(slack_core::api::Socket::Gateway));
        assert!(next_call(&mut asked, "client.counts").await.is_some());
        super::handle_rtm(&mut app, RtmEvent::Event(serde_json::json!({"type": "badge_counts_updated", "event_ts": "9.9", "activity_v2": {"at_user": 4}})));
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(10), net.recv()).await {
                Ok(Some(msg @ Net::ActivityBadge { .. })) => {
                    super::handle_net(&mut app, msg);
                    break;
                }
                Ok(Some(_)) => {}
                other => panic!("no counts: {}", other.is_ok()),
            }
        }
        assert_eq!(app.ws.activity_unread, 4, "the push is newer than the answer (7)");
    }

    /// The reported sequence: Activity, `u`, Enter on an unread item, Activity again: the unread items further down the
    /// All tab are still shown (its first page is all read now), and the filter is kept for the run.
    #[tokio::test]
    async fn the_unread_filter_shows_whats_still_unread_and_is_kept() {
        use super::super::activity::Tab;
        use super::{run_global, Action, Net, View};
        use crossterm::event::{KeyCode, KeyModifiers};
        let (url, mut asked) = fake_slack(&[("activity.feed", r#"{"ok":true,"items":[]}"#)]).await;
        let (mut app, _net) = test_app_at(&url);
        app.ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "c".into(), is_channel: true, is_member: true, ..Default::default() });
        let mention = |ts: &str, unread: bool| serde_json::json!({"feed_ts": ts, "is_unread": unread, "key": format!("at_user-C1-{ts}"), "item": {"type": "at_user", "message": {"channel": "C1", "ts": ts}}});
        let thread = serde_json::json!({"feed_ts": "3.0", "is_unread": true, "key": "thread_v2-C1-2.0", "item": {"type": "thread_v2", "bundle_info": {"payload": {"thread_entry": {"channel_id": "C1", "thread_ts": "2.0", "latest_ts": "3.0", "min_unread_ts": "3.0", "unread_msg_count": 1}}}}});
        // The All tab's first page (one unread, many read: reactions, read mentions), its next page an unread thread.
        let first = |unread: bool| {
            let items: Vec<_> = std::iter::once(mention("90.0", unread)).chain((0..40).map(|n| mention(&format!("{}.0", 80 - n), false))).collect();
            serde_json::json!({"items": items, "response_metadata": {"next_cursor": "p2"}})
        };
        let next = serde_json::json!({"items": [thread]});
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let mut screen = |app: &mut super::App| {
            terminal.draw(|f| super::draw(f, app)).unwrap();
            let buf = terminal.backend().buffer();
            (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
        };
        // Loads the next page as the view asks for it.
        async fn next_page(app: &mut super::App, asked: &mut tokio::sync::mpsc::UnboundedReceiver<String>, page: &serde_json::Value) {
            app.request_activity_page();
            let req = next_call(asked, "activity.feed").await.expect("the next page");
            assert!(req.contains("cursor=p2"), "{req}");
            super::handle_net(app, Net::ActivityFeed { tab: Tab::All, more: true, result: Ok(page.clone()) });
        }
        run_global(&mut app, Action::Activity);
        assert!(next_call(&mut asked, "activity.feed").await.is_some());
        super::handle_net(&mut app, Net::ActivityFeed { tab: Tab::All, more: false, result: Ok(first(true)) });
        press(&mut app, KeyCode::Char('u'), KeyModifiers::NONE);
        screen(&mut app);
        next_page(&mut app, &mut asked, &next).await;
        screen(&mut app);
        assert_eq!(app.activity.visible().len(), 2, "both unread ones");
        // Enter on the first: read, its conversation opens.
        app.activity.selected = 0;
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(next_call(&mut asked, "activity.markRead").await.is_some());
        assert_eq!(app.view, View::Conversation);
        // Back in Activity: still filtered, and what is still unread (the thread, on the next page) is shown.
        run_global(&mut app, Action::Activity);
        assert!(next_call(&mut asked, "activity.feed").await.is_some());
        assert!(app.activity.unread_only, "the filter is kept");
        super::handle_net(&mut app, Net::ActivityFeed { tab: Tab::All, more: false, result: Ok(first(false)) });
        let shown = screen(&mut app);
        assert!(!shown.contains("Nothing unread here."), "{shown}");
        next_page(&mut app, &mut asked, &next).await;
        let shown = screen(&mut app);
        assert_eq!(app.activity.selected_item().map(|i| i.key.as_str()), Some("thread_v2-C1-2.0"), "{shown}");
        assert!(shown.contains("unread only"));
        // Kept across tabs and other views; `u` turns it off, and off it stays.
        press(&mut app, KeyCode::Char('4'), KeyModifiers::NONE);
        assert!(app.activity.unread_only);
        run_global(&mut app, Action::Threads);
        run_global(&mut app, Action::Activity);
        assert_eq!((app.activity.tab, app.activity.unread_only), (Tab::All, true), "reopened on All, still filtered");
        press(&mut app, KeyCode::Char('u'), KeyModifiers::NONE);
        run_global(&mut app, Action::Threads);
        run_global(&mut app, Action::Activity);
        assert!(!app.activity.unread_only);
        // Filtered, the next page fails: said in the view, and not asked for again until you act.
        while tokio::time::timeout(std::time::Duration::from_millis(300), asked.recv()).await.is_ok_and(|r| r.is_some()) {}
        press(&mut app, KeyCode::Char('u'), KeyModifiers::NONE);
        super::handle_net(&mut app, Net::ActivityFeed { tab: Tab::All, more: false, result: Ok(first(false)) });
        screen(&mut app);
        app.request_activity_page();
        assert!(next_call(&mut asked, "activity.feed").await.is_some_and(|r| r.contains("cursor=p2")));
        super::handle_net(&mut app, Net::ActivityFeed { tab: Tab::All, more: true, result: Err("ratelimited".into()) });
        for _ in 0..3 {
            let shown = screen(&mut app);
            assert!(shown.contains("could not be loaded: ratelimited") && !shown.contains("Loading more"), "{shown}");
            app.request_activity_page();
        }
        assert!(tokio::time::timeout(std::time::Duration::from_millis(300), next_call(&mut asked, "activity.feed")).await.is_err(), "not asked for again by itself");
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        screen(&mut app);
        next_page(&mut app, &mut asked, &next).await;
        assert_eq!(app.activity.visible().len(), 1, "asked for again after a key: the thread");
    }

    #[tokio::test]
    async fn without_slacks_badge_the_feed_counts_as_before() {
        use super::Net;
        let (url, mut asked) = fake_slack(&[("activity.feed", r#"{"ok":true,"items":[{"feed_ts":"5.0","is_unread":true,"key":"r1","item":{"type":"message_reaction","message":{"channel":"C1","ts":"4.0"},"reaction":{"name":"eyes","user":"U2"}}}]}"#)]).await;
        let (mut app, mut net) = test_app_at(&url);
        super::handle_net(&mut app, Net::Boot { boot: serde_json::json!({}), users: vec![], counts: Some(serde_json::json!({"ims": []})) });
        assert!(next_call(&mut asked, "activity.feed").await.is_some(), "the feed is asked for the count");
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(10), net.recv()).await {
                Ok(Some(msg @ Net::ActivityUnread(_))) => {
                    super::handle_net(&mut app, msg);
                    break;
                }
                Ok(Some(_)) => {}
                other => panic!("no feed: {}", other.is_ok()),
            }
        }
        assert_eq!(app.ws.activity_unread, 1);
    }

    #[tokio::test]
    async fn a_pushed_dnd_change_of_yours_is_fetched() {
        use super::Net;
        let (url, _asked) = fake_slack(&[("dnd.info", r#"{"ok":true,"dnd_enabled":false,"snooze_enabled":true,"snooze_endtime":4102444800,"snooze_remaining":60}"#)]).await;
        let (mut app, mut net) = test_app_at(&url);
        app.ws.self_id = "U0".into();
        assert!(app.ws.dnd.snooze_until.is_none());
        super::handle_rtm(&mut app, slack_core::rtm::RtmEvent::Event(serde_json::json!({"type": "dnd_invalidated", "event_ts": "1.5", "user": {"id": "U0"}})));
        let info = loop {
            match tokio::time::timeout(std::time::Duration::from_secs(10), net.recv()).await {
                Ok(Some(Net::Dnd(v))) => break v,
                Ok(Some(_)) => {}
                other => panic!("dnd.info not asked: {}", other.is_ok()),
            }
        };
        super::handle_net(&mut app, Net::Dnd(info));
        assert_eq!(app.ws.dnd.snooze_until, Some(4102444800.0), "paused, as set elsewhere");
    }

    #[tokio::test]
    async fn statuses_are_looked_up_again_after_a_reconnect_and_rarely_otherwise() {
        use slack_core::rtm::RtmEvent;
        let (mut app, _net) = test_app();
        with_dm(&mut app, "U2");
        assert_eq!(app.ws.settings.messages.status_refresh_secs, 600, "a slow safety net: every ten minutes");
        super::handle_rtm(&mut app, RtmEvent::Connected(slack_core::api::Socket::Gateway));
        app.refresh_statuses();
        assert!(app.lookups.due.is_empty(), "not at the start");
        app.statuses_polled = Some(std::time::Instant::now() - std::time::Duration::from_secs(599));
        app.refresh_statuses();
        assert!(app.lookups.due.is_empty(), "not before ten minutes");
        // Pushes missed while the connection was away: the people shown, once.
        super::handle_rtm(&mut app, RtmEvent::Disconnected("gone".into()));
        super::handle_rtm(&mut app, RtmEvent::Connected(slack_core::api::Socket::Gateway));
        assert_eq!(app.lookups.due.iter().collect::<Vec<_>>(), ["U2"]);
        app.lookups.due.clear();
        app.statuses_polled = Some(std::time::Instant::now() - std::time::Duration::from_secs(601));
        app.refresh_statuses();
        assert_eq!(app.lookups.due.iter().collect::<Vec<_>>(), ["U2"], "every ten minutes");
    }

    #[tokio::test]
    async fn a_flapping_connection_does_not_look_the_people_up_each_time() {
        use slack_core::rtm::RtmEvent;
        let gateway = || RtmEvent::Connected(slack_core::api::Socket::Gateway);
        let (mut app, _net) = test_app();
        with_dm(&mut app, "U2");
        super::handle_rtm(&mut app, gateway());
        super::handle_rtm(&mut app, RtmEvent::Disconnected("gone".into()));
        super::handle_rtm(&mut app, gateway());
        assert_eq!(app.lookups.due.iter().collect::<Vec<_>>(), ["U2"], "the first reconnect");
        app.lookups.due.clear();
        // A short drop soon after: not again.
        super::handle_rtm(&mut app, RtmEvent::Disconnected("gone".into()));
        super::handle_rtm(&mut app, gateway());
        assert!(app.lookups.due.is_empty(), "a flap");
        // Down for 30 s (over several failed tries): again.
        super::handle_rtm(&mut app, RtmEvent::Disconnected("gone".into()));
        app.rtm_down_since = app.rtm_down_since.map(|t| t - std::time::Duration::from_secs(30));
        super::handle_rtm(&mut app, RtmEvent::Disconnected("still gone".into()));
        super::handle_rtm(&mut app, gateway());
        assert_eq!(app.lookups.due.iter().collect::<Vec<_>>(), ["U2"], "down long enough");
        app.lookups.due.clear();
        // A short drop five minutes after the last look-up: again.
        app.people_requeued = app.people_requeued.map(|t| t - std::time::Duration::from_secs(300));
        super::handle_rtm(&mut app, RtmEvent::Disconnected("gone".into()));
        super::handle_rtm(&mut app, gateway());
        assert_eq!(app.lookups.due.iter().collect::<Vec<_>>(), ["U2"], "five minutes on");
    }

    #[tokio::test]
    async fn a_reconnect_asks_for_the_live_huddles_again() {
        use super::Net;
        let (mut app, mut net) = test_app();
        app.ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "c".into(), is_channel: true, is_member: true, ..Default::default() });
        // One kept from the live events that ended while the connection was away.
        app.ws.huddles.insert("C1".into(), super::super::state::Huddle { room: "R1".into(), participants: vec!["U2".into()], ..Default::default() });
        super::handle_rtm(&mut app, slack_core::rtm::RtmEvent::Connected(slack_core::api::Socket::Gateway));
        let mut asked = false;
        while let Ok(Some(msg)) = tokio::time::timeout(std::time::Duration::from_secs(10), net.recv()).await {
            if matches!(msg, Net::LiveHuddles { .. }) {
                asked = true;
                break;
            }
        }
        assert!(asked, "the live huddles are asked for");
        let since = app.ws.huddle_seq;
        super::handle_net(&mut app, Net::LiveHuddles { since, result: Ok(vec![]) });
        assert!(app.ws.huddles.is_empty() && app.ws.joinable_live(None).is_empty(), "no stale live huddle survives");
        // A failed fetch keeps what the events said.
        app.ws.huddles.insert("C1".into(), super::super::state::Huddle { room: "R2".into(), participants: vec!["U2".into()], ..Default::default() });
        let since = app.ws.huddle_seq;
        super::handle_net(&mut app, Net::LiveHuddles { since, result: Err("offline".into()) });
        assert_eq!(app.ws.joinable_live(None), ["C1"]);
    }

    #[tokio::test]
    async fn a_live_list_fetched_before_an_end_does_not_bring_it_back() {
        use super::Net;
        let (mut app, mut net) = test_app();
        app.ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "c".into(), is_channel: true, is_member: true, ..Default::default() });
        app.ws.huddles.insert("C1".into(), super::super::state::Huddle { room: "R1".into(), participants: vec!["U2".into()], ..Default::default() });
        // The fetch starts, then R1 ends over RTM, then the fetch's answer (from before the end) comes.
        app.fetch_live_huddles();
        let since = loop {
            match tokio::time::timeout(std::time::Duration::from_secs(10), net.recv()).await {
                Ok(Some(Net::LiveHuddles { since, .. })) => break since,
                Ok(Some(_)) => {}
                other => panic!("no answer: {}", other.is_ok()),
            }
        };
        super::handle_rtm(&mut app, slack_core::rtm::RtmEvent::Event(serde_json::json!({"type":"sh_room_update","user":"U2","room":{"id":"R1","channels":["C1"],"participants":["U2"],"has_ended":true}})));
        let stale = slack_core::types::LiveHuddle { channel: "C1".into(), call_id: "R1".into(), members: vec!["U2".into()], start: 0, thread_ts: None };
        super::handle_net(&mut app, Net::LiveHuddles { since, result: Ok(vec![stale]) });
        assert!(app.ws.joinable_live(None).is_empty(), "R1 is not live again, the badge is 0");
    }

    #[tokio::test]
    async fn only_an_end_the_huddles_view_shows_reloads_it() {
        use super::View;
        let (mut app, _net) = test_app();
        app.ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "c".into(), is_channel: true, is_member: true, ..Default::default() });
        super::run_global(&mut app, super::Action::Huddles);
        let generation = app.huddles.generation;
        let old = slack_core::types::PastHuddle { id: "R0".into(), channel: "C1".into(), ended: true, start: 1, end: 2, ..Default::default() };
        app.huddles.add_page(generation, false, vec![old], None);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        assert_eq!(app.view, View::Huddles);
        // An end in a channel it does not list: nothing to load again.
        super::handle_rtm(&mut app, slack_core::rtm::RtmEvent::Event(serde_json::json!({"type":"ended_huddle_update","huddle":{"id":"R9","channels":["C9"]}})));
        assert_eq!(app.huddles.generation, generation);
        // One in a channel it lists: loaded again, the list staying on screen meanwhile.
        super::handle_rtm(&mut app, slack_core::rtm::RtmEvent::Event(serde_json::json!({"type":"ended_huddle_update","huddle":{"id":"R1","channels":["C1"]}})));
        assert!(app.huddles.generation > generation && app.huddles.past.as_ref().is_some_and(|p| p.len() == 1));
    }

    #[tokio::test]
    async fn the_views_take_the_main_pane() {
        use super::{run_global, Action, Focus, View};
        use crossterm::event::{KeyCode, KeyModifiers};
        use ratatui::layout::Rect;
        let (mut app, _net) = test_app();
        app.current = Some("C1".into());
        app.sidebar.rebuild(&app.ws);
        let rows = |t: &ratatui::Terminal<ratatui::backend::TestBackend>| {
            let buf = t.backend().buffer();
            (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>()
        };
        // An 80-column terminal: the sidebar takes 20, the view the rest down to the status line.
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        run_global(&mut app, Action::Activity);
        let item = super::super::activity::Item { key: "k1".into(), kind: super::super::activity::Kind::Mention("at_user".into()), channel: "C1".into(), ts: "1.0".into(), author: None, unread: true, when: 0.0, ..Default::default() };
        app.activity.items = Some(vec![item]);
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        let screen = rows(&terminal);
        assert_eq!(app.activity.area, Rect::new(20, 0, 60, 23));
        assert!(screen[0].chars().skip(20).collect::<String>().starts_with("┌ Activity "), "{screen:#?}");
        assert!(screen[23].starts_with(" ACTIVITY "), "its pill: {:?}", screen[23]);
        assert_eq!((app.composer_area, app.messages.area), (Rect::default(), Rect::default()), "no message pane, no message box");
        assert!(screen[1].contains("Threads") && screen[2].contains("Activity"), "the sidebar's rows: {screen:#?}");
        assert!(screen[1].contains(" All ") && screen[1].contains(" Threads "), "the tab bar: {screen:#?}");
        // The pointer over an item (under the tab bar and its date header) shades it.
        assert!(super::handle_event(&mut app, crossterm::event::Event::Mouse(crossterm::event::MouseEvent { kind: crossterm::event::MouseEventKind::Moved, column: 30, row: 3, modifiers: KeyModifiers::NONE })));
        assert_eq!(app.activity.hover, Some(0));
        // `]`, `[` and 1–4 switch the tabs, `u` keeps the unread items only.
        use super::super::activity::Tab;
        press(&mut app, KeyCode::Char(']'), KeyModifiers::NONE);
        assert_eq!((app.activity.tab, app.activity.items.is_none()), (Tab::Dms, true));
        press(&mut app, KeyCode::Char('4'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('['), KeyModifiers::NONE);
        assert_eq!(app.activity.tab, Tab::Mentions);
        press(&mut app, KeyCode::Char('1'), KeyModifiers::NONE);
        assert_eq!((app.activity.tab, app.activity.items.as_ref().map(Vec::len)), (Tab::All, Some(1)), "its list kept");
        press(&mut app, KeyCode::Char('u'), KeyModifiers::NONE);
        assert!(app.activity.unread_only);
        // A click on a tab shows it.
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        let x = (0..buf.area.width).find(|&x| (x..buf.area.width).map(|x| buf[(x, 1)].symbol()).collect::<String>().starts_with("Mentions")).unwrap();
        click(&mut app, x, 1);
        assert_eq!((app.view, app.activity.tab), (View::Activity, Tab::Mentions));
        press(&mut app, KeyCode::Char('1'), KeyModifiers::NONE);
        // A click on the sidebar's Threads row opens Threads there.
        click(&mut app, 3, 1);
        assert_eq!(app.view, View::Threads);
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        assert!(rows(&terminal)[0].chars().skip(20).collect::<String>().starts_with("┌ Threads "));
        assert_eq!((app.threads.area, app.activity.area), (Rect::new(20, 0, 60, 20), Rect::default()), "the hidden view cannot be clicked");
        assert_eq!(app.composer_area, Rect::new(20, 20, 60, 3), "the Threads feed keeps the box under it, to reply");
        // From the sidebar, a click on the Activity row, then Enter on its item: the conversation.
        app.focus = Focus::Sidebar;
        click(&mut app, 3, 2);
        app.activity.items = Some(vec![super::super::activity::Item { key: "k1".into(), kind: super::super::activity::Kind::Mention("at_user".into()), channel: "C1".into(), ts: "1.0".into(), author: None, unread: true, when: 0.0, ..Default::default() }]);
        assert_eq!((app.view, app.focus), (View::Activity, Focus::Messages));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!((app.view, app.current.as_deref()), (View::Conversation, Some("C1")));
        assert!(app.seen.contains_key("k1"), "the item is seen");
        terminal.draw(|f| super::draw(f, &mut app)).unwrap();
        assert_ne!(app.composer_area, Rect::default(), "the conversation and its message box are back");
    }

    #[tokio::test]
    async fn in_a_huddle_your_state_is_renewed_as_slacks_client_does() {
        let (url, mut asked) = fake_slack(&[]).await;
        let (mut app, _net) = test_app_at(&url);
        let t0 = std::time::Instant::now();
        let at = |s: u64| t0 + std::time::Duration::from_secs(s);
        let next = |asked: &mut tokio::sync::mpsc::UnboundedReceiver<String>| asked.try_recv().ok();
        let wait = || tokio::time::sleep(std::time::Duration::from_millis(300));
        // The pref (from boot or a `pref_change`) is off until Slack says it is on.
        app.beat_huddle(true, at(0));
        wait().await;
        assert_eq!(next(&mut asked), None, "pref unknown: nothing");
        app.huddle_beat = None;
        app.ws.apply_prefs(&serde_json::json!({"huddles_set_status": true}));
        app.beat_huddle(true, at(0));
        let req = tokio::time::timeout(std::time::Duration::from_secs(10), asked.recv()).await.expect("a request").unwrap();
        assert!(req.contains("/api/users.profile.set "), "{req}");
        let body = &req[req.find("\r\n\r\n").unwrap()..];
        assert!(body.contains("profile=%7B%22huddle_state_expiration_ts%22%3A") && body.matches("%22%3A").count() == 1, "only the expiry: {body}");
        assert!(body.contains("_x_reason=increment-huddle-state-expiration-ts") && !body.contains("status"), "{body}");
        assert!(app.huddle_beat.is_some());
        // Not again before 216 s (240 s − 10 %); the call ended: nothing more, ever.
        app.beat_huddle(true, at(200));
        app.beat_huddle(false, at(300));
        app.beat_huddle(false, at(900));
        wait().await;
        assert_eq!(next(&mut asked), None);
        assert_eq!(app.huddle_beat, None);
        // Turned off in Slack: no beat in the next call.
        app.ws.apply_prefs(&serde_json::json!({"huddles_set_status": false}));
        app.beat_huddle(true, at(1000));
        wait().await;
        assert_eq!(next(&mut asked), None, "pref off");
    }

    /// What a large, busy workspace costs per frame and per pass (VX4's before/after):
    /// `cargo test -p klack busy_workspace_cost -- --ignored --nocapture` (a debug build).
    #[test]
    #[ignore]
    fn busy_workspace_cost() {
        use super::super::state::{Timeline, Workspace};
        use slack_core::types::{Conversation, Message, User};
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        ws.users.insert("UME".into(), User { id: "UME".into(), name: "me".into(), ..Default::default() });
        for i in 0..20_000 {
            let id = format!("U{i:05}");
            ws.users.insert(id.clone(), User { id, name: format!("p{i}"), ..Default::default() });
        }
        // (As the user list is taken in at the start.)
        ws.users_changed();
        for i in 0..400 {
            let name = format!("mpdm-me--p{}--p{}--p{}-1", i * 7, i * 11 + 3, i * 13 + 5);
            let id = format!("G{i:04}");
            ws.conversations.insert(id.clone(), Conversation { id, name, is_mpim: true, is_member: true, ..Default::default() });
        }
        for i in 0..3_000 {
            let id = format!("C{i:04}");
            ws.conversations.insert(id.clone(), Conversation { id, name: format!("chan{i}"), is_channel: true, is_member: true, ..Default::default() });
        }
        for i in 0..1_000 {
            let id = format!("D{i:04}");
            ws.conversations.insert(id.clone(), Conversation { id, is_im: true, is_member: true, user: Some(format!("U{:05}", i * 3)), ..Default::default() });
        }
        let msg = |ts: String, user: String, text: &str| Message { ts, user: Some(user), text: text.into(), ..Default::default() };
        for c in 0..50 {
            let t = Timeline { loaded: true, messages: (0..200).map(|n| msg(format!("{}.000100", 1_790_000_000 + n * 60), format!("U{:05}", (c * 37 + n) % 20_000), "a line of chat that is about as long as one")).collect(), ..Default::default() };
            ws.timelines.insert(format!("C{c:04}"), t);
        }
        let time = |label: &str, runs: usize, f: &mut dyn FnMut()| {
            let start = std::time::Instant::now();
            for _ in 0..runs {
                f();
            }
            let each = start.elapsed().as_secs_f64() * 1e6 / runs as f64;
            println!("{label}: {each:.1} µs");
            each
        };
        // The sidebar's group DM rows (twice per row per draw).
        let mpims: Vec<&Conversation> = ws.conversations.values().filter(|c| c.is_mpim).collect();
        time("400 group DM labels, twice", 5, &mut || {
            for c in &mpims {
                std::hint::black_box(ws.conversation_label(c));
                std::hint::black_box(ws.conversation_label(c));
            }
        });
        time("people_to_follow", 5, &mut || {
            std::hint::black_box(ws.people_to_follow());
        });
        time("people_key (what the people-follow pass now checks per turn)", 100, &mut || {
            std::hint::black_box(ws.people_key());
        });
        time("beat_jitter", 10_000, &mut || {
            std::hint::black_box(super::super::call::beat_jitter());
        });
        let media = super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 50)).unwrap();
        // Activity: 500 items, their messages loaded.
        let mut activity = super::super::activity::Activity::default();
        let items: Vec<super::super::activity::Item> = (0..500)
            .map(|n| super::super::activity::Item { key: format!("k{n}"), ty: "at_user".into(), kind: super::super::activity::Kind::Mention("at_user".into()), channel: format!("C{:04}", n % 50), ts: format!("{}.000100", 1_790_000_000 + n * 60), author: Some(format!("U{:05}", n)), unread: n % 3 == 0, when: (1_790_000_000 + n * 60) as f64, ..Default::default() })
            .rev()
            .collect();
        for it in &items {
            activity.messages.insert((it.channel.clone(), it.ts.clone()), Some(msg(it.ts.clone(), it.author.clone().unwrap(), "hey <@UME> can you look at this when you have a minute")));
        }
        activity.set_items(super::super::activity::Tab::All, items, None);
        let here = super::super::applink::Here::alone(&ws);
        time("Activity frame, first", 1, &mut || {
            terminal.draw(|f| activity.draw(f, f.area(), &here, true, &media)).unwrap();
        });
        time("Activity frame, nothing changed", 20, &mut || {
            terminal.draw(|f| activity.draw(f, f.area(), &here, true, &media)).unwrap();
        });
        // Threads: 50 threads, a parent and three replies each.
        let mut threads = super::super::threads::Threads::default();
        let entries: Vec<super::super::threads::Entry> = (0..50)
            .map(|n| {
                let root_ts = format!("{}.000100", 1_790_000_000 + n * 600);
                let mut root = msg(root_ts.clone(), format!("U{:05}", n), "a thread's parent, a line or two of text to read");
                (root.thread_ts, root.reply_count) = (Some(root_ts.clone()), 3);
                let replies = (1..=3).map(|r| Message { thread_ts: Some(root_ts.clone()), ..msg(format!("{}.000100", 1_790_000_000 + n * 600 + r * 10), format!("U{:05}", n + r), "a reply") }).collect();
                super::super::threads::Entry { channel: format!("C{:04}", n % 50), root, latest: replies, ..Default::default() }
            })
            .collect();
        threads.add_page(entries, false);
        time("Threads frame, first (a build)", 1, &mut || {
            terminal.draw(|f| threads.draw(f, f.area(), &here, true, &media)).unwrap();
        });
        time("Threads frame, nothing changed", 20, &mut || {
            terminal.draw(|f| threads.draw(f, f.area(), &here, true, &media)).unwrap();
        });
    }

    use crossterm::event::{KeyCode, KeyModifiers};

    /// After an event, as the event loop does: the sidebar's selection followed, and the place kept (`went`: by your doing).
    fn settle(app: &mut super::App, went: bool) {
        app.sync_current();
        app.track_place(went);
    }

    /// [`press`], then [`settle`].
    fn go(app: &mut super::App, code: KeyCode, mods: KeyModifiers) {
        press(app, code, mods);
        settle(app, true);
    }

    /// An app with channels `ids` (A in each) and a DM `DU2` with U2, on the first channel.
    fn with_channels(ids: &[&str]) -> (super::App, tokio::sync::mpsc::UnboundedReceiver<super::Net>) {
        let (mut app, net) = test_app();
        for id in ids {
            app.ws.conversations.insert(id.to_string(), slack_core::types::Conversation { id: id.to_string(), name: format!("chan{id}"), is_channel: true, is_member: true, ..Default::default() });
        }
        with_dm(&mut app, "U2");
        app.jump_to(ids[0]);
        settle(&mut app, false);
        app.status.clear();
        (app, net)
    }

    #[tokio::test]
    async fn back_from_a_dm_opened_in_activity_is_activity_on_its_tab_and_forward_the_dm() {
        use super::super::activity::Tab;
        use super::{Net, View};
        let (mut app, _net) = with_channels(&["C1"]);
        assert_eq!(app.current.as_deref(), Some("C1"));
        // Activity, its DMs tab, Enter on the DM.
        go(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        go(&mut app, KeyCode::Char('2'), KeyModifiers::NONE);
        assert_eq!((app.view, app.activity.tab), (View::Activity, Tab::Dms));
        let feed = serde_json::json!({"items": [{"feed_ts": "2.0", "is_unread": true, "key": "dm-DU2", "item": {"type": "dm", "bundle_info": {"payload": {"dm_entry": {"latest_message": {"channel": "DU2", "ts": "1.0"}}}}}}]});
        super::handle_net(&mut app, Net::ActivityFeed { tab: Tab::Dms, more: false, result: Ok(feed) });
        settle(&mut app, false);
        go(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!((app.view, app.current.as_deref()), (View::Conversation, Some("DU2")));
        // Back (Ctrl+O): Activity, on the DMs tab. Forward (Alt+→): the DM.
        go(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!((app.view, app.activity.tab), (View::Activity, Tab::Dms));
        go(&mut app, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!((app.view, app.current.as_deref()), (View::Conversation, Some("DU2")));
        // Back twice (Alt+←): Activity, then the channel before it.
        go(&mut app, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!(app.view, View::Activity);
        go(&mut app, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!((app.view, app.current.as_deref()), (View::Conversation, Some("C1")));
    }

    #[tokio::test]
    async fn back_with_nowhere_to_go_says_so() {
        use super::super::history::{NOTHING_BACK, NOTHING_FORWARD};
        let (mut app, _net) = with_channels(&["C1"]);
        go(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!((app.current.as_deref(), app.view, app.status.as_str()), (Some("C1"), super::View::Conversation, NOTHING_BACK));
        go(&mut app, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!((app.current.as_deref(), app.status.as_str()), (Some("C1"), NOTHING_FORWARD));
    }

    #[tokio::test]
    async fn the_history_keys_do_nothing_while_typing() {
        let (mut app, _net) = with_channels(&["C1", "C2"]);
        app.jump_to("C2");
        settle(&mut app, true);
        app.focus = super::Focus::Composer;
        for (code, mods) in [(KeyCode::Char('o'), KeyModifiers::CONTROL), (KeyCode::Left, KeyModifiers::ALT), (KeyCode::Right, KeyModifiers::ALT)] {
            go(&mut app, code, mods);
            assert_eq!((app.current.as_deref(), app.focus), (Some("C2"), super::Focus::Composer));
        }
        assert!(app.status.is_empty() && app.composer.is_empty(), "{:?}", app.status);
        // In NORMAL again: back.
        go(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        go(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(app.current.as_deref(), Some("C1"));
    }

    #[tokio::test]
    async fn back_skips_a_channel_left_or_archived_and_opens_a_thread_again() {
        use super::super::history::NOTHING_BACK;
        let (mut app, _net) = with_channels(&["C0", "C1", "C2", "C3"]);
        app.open_thread_at("C0".into(), "5.0".into());
        settle(&mut app, true);
        for id in ["C1", "C2", "C3"] {
            app.jump_to(id);
            settle(&mut app, true);
        }
        // C1 left, C2 archived meanwhile.
        app.ws.conversations.get_mut("C1").unwrap().is_member = false;
        app.ws.conversations.get_mut("C2").unwrap().is_archived = true;
        go(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!((app.current.as_deref(), app.thread.clone()), (Some("C0"), Some(("C0".into(), "5.0".into()))), "C0, its thread open again");
        // Before the thread was opened: C0 alone; before that, nowhere.
        go(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!((app.current.as_deref(), app.thread.clone()), (Some("C0"), None));
        go(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(app.status, NOTHING_BACK);
        go(&mut app, KeyCode::Right, KeyModifiers::ALT);
        go(&mut app, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!((app.current.as_deref(), app.thread.clone()), (Some("C3"), None), "forward skips them too");
    }

    #[tokio::test]
    async fn a_reply_that_takes_you_somewhere_is_a_step() {
        use super::{goes_somewhere, Net};
        let (mut app, _net) = with_channels(&["C1"]);
        // Ctrl+K on someone without a DM yet: the DM opens when Slack answers.
        let dm = slack_core::types::Conversation { id: "D9".into(), is_im: true, is_member: true, user: Some("U9".into()), ..Default::default() };
        let msg = Net::DmOpened(dm);
        let went = goes_somewhere(&msg);
        super::handle_net(&mut app, msg);
        settle(&mut app, went);
        assert_eq!(app.current.as_deref(), Some("D9"));
        go(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(app.current.as_deref(), Some("C1"));
        assert!(!goes_somewhere(&Net::Error("x".into())));
    }
}
