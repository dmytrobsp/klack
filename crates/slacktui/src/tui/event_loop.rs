//! Main loop: terminal input, background API results and periodic ticks.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use futures_util::StreamExt;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Paragraph;
use serde_json::Value;
use slack_core::Client;
use slack_core::types::{HistoryPage, User};
use tokio::sync::mpsc;

use super::composer::{Composer, KeyOutcome};
use super::messages::MessagePane;
use super::picker::EmojiPicker;
use super::sidebar::Sidebar;
use super::state::Workspace;
use super::terminal::TerminalGuard;

/// Results of background API calls.
enum Net {
    Boot { boot: Value, users: Vec<User>, counts: Option<Value> },
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
    SearchDone { query: String, result: Result<serde_json::Value, String> },
    /// Huddle credentials from `rooms.join`.
    HuddleJoined { channel: String, result: Result<huddle::signaling::Credentials, String> },
    /// A DM opened from the quick switcher.
    DmOpened(slack_core::types::Conversation),
    /// A video downloaded to a temporary file, ready to play.
    VideoReady { title: String, result: Result<std::path::PathBuf, String> },
    /// Result of a file save or upload, shown in the status bar.
    FileDone(String),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Messages,
    Thread,
    ThreadComposer,
    Composer,
}

struct App {
    /// The huddle we are in.
    call: Option<super::call::Call>,
    /// Presence subscription sent on the current RTM connection.
    presence_subscribed: bool,
    /// Open conversation details (F2).
    details: Option<super::details::Details>,
    /// Open search overlay (Ctrl+F).
    search: Option<super::search::Search>,
    /// A search hit to select once its conversation is shown.
    pending_hit: Option<super::search::Hit>,
    /// Open quick switcher (Ctrl+K).
    switcher: Option<super::switcher::Switcher>,
    /// Key help overlay is open.
    help: bool,
    /// Open video player.
    player: Option<super::video::Player>,
    /// Whole terminal area on the last draw (to size video frames).
    screen: ratatui::layout::Rect,
    /// Open image viewer.
    viewer: Option<super::viewer::Viewer>,
    media: super::media::Media,
    /// The terminal window has focus (focus-change reporting).
    terminal_focused: bool,
    /// Open @/#/: suggestion list.
    completion: Option<super::autocomplete::Completion>,
    /// Suggestions closed with Esc for this (composer, token start) until the token changes.
    completion_dismissed: Option<(bool, usize)>,
    /// Composer boxes on the last draw (to anchor the suggestion list).
    composer_area: ratatui::layout::Rect,
    thread_composer_area: ratatui::layout::Rect,
    /// Open thread pane: (channel, parent ts).
    thread: Option<(String, String)>,
    thread_pane: MessagePane,
    thread_composer: Composer,
    /// Thread replies are also sent to the channel.
    broadcast: bool,
    clipboard: super::clipboard::Clipboard,
    /// Message (channel, ts) awaiting y/n delete confirmation.
    confirm_delete: Option<(String, String)>,
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
    loading: bool,
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
        if selected != self.current {
            self.current = selected.clone();
            self.messages.scroll = 0;
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

    /// Tell the other side we are typing (at most every 3 s).
    fn notify_typing(&mut self) {
        let Some(channel) = self.current.clone() else { return };
        if self.last_typing_sent.is_some_and(|t| t.elapsed().as_secs() < 3) {
            return;
        }
        self.last_typing_sent = Some(std::time::Instant::now());
        let _ = self.rtm_commands.send(slack_core::rtm::RtmCommand::Typing { channel });
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
        let c = self.composer_for(thread);
        if c.is_empty() {
            return;
        }
        match super::files::parse_upload(&c.text()) {
            Some(Ok(cmd)) => {
                c.push_history(c.text());
                c.clear();
                self.upload(channel, cmd, thread_ts);
                return;
            }
            Some(Err(e)) => {
                self.status = e;
                return;
            }
            None => {}
        }
        let text = c.to_mrkdwn();
        c.push_history(c.text());
        c.clear();
        if thread {
            self.thread_pane.scroll = 0;
        } else {
            self.messages.scroll = 0;
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

    /// Load our own message `ts` in the current conversation into a composer for editing.
    fn start_edit(&mut self, ts: &str, thread: bool) {
        let Some(channel) = self.current.clone() else { return };
        let Some(m) = self.ws.find_message(&channel, ts) else { return };
        if m.user.as_deref() != Some(self.ws.self_id.as_str()) {
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
        let own = self.ws.find_message(&channel, ts).is_some_and(|m| m.user.as_deref() == Some(self.ws.self_id.as_str()));
        if own {
            self.confirm_delete = Some((channel, ts.to_string()));
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
        self.thread_pane.scroll = 0;
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
        self.thread = None;
        self.thread_composer.clear();
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
            self.completion = None;
            self.completion_dismissed = None;
            return;
        };
        if self.completion_dismissed == Some((thread, start)) {
            self.completion = None;
            return;
        }
        let items = super::autocomplete::suggestions(&self.ws, &trigger);
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
        self.composer_for(c.thread).complete(c.start, &format!("{} ", s.replacement), s.id.as_deref());
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
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let result = client.search_messages(&query, 50).await.map_err(|e| e.to_string());
            let _ = tx.send(Net::SearchDone { query, result });
        });
    }

    /// Go to a search hit: its conversation, the message selected, its thread open.
    fn open_hit(&mut self, hit: super::search::Hit) {
        if self.current.as_deref() == Some(hit.channel.as_str()) {
            self.show_hit(hit);
        } else {
            self.sidebar.select_id(&hit.channel);
            self.pending_hit = Some(hit);
        }
    }

    fn show_hit(&mut self, hit: super::search::Hit) {
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

    /// Show conversation `id` and put the cursor in the composer.
    fn jump_to(&mut self, id: &str) {
        self.sidebar.select_id(id);
        self.focus = Focus::Composer;
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

    /// F3: join (or start) the huddle of the current conversation, or leave the one we are in.
    fn toggle_huddle(&mut self) {
        if let Some(call) = self.call.as_mut() {
            call.leave();
            self.status = "leaving the huddle…".into();
            return;
        }
        let Some(channel) = self.current.clone() else { return };
        self.status = "joining the huddle…".into();
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let result = async {
                let v = client.call_raw("rooms.join", &[("channel_id", channel.clone())]).await?;
                huddle::signaling::Credentials::from_rooms_join(&v)
            }
            .await
            .map_err(|e| e.to_string());
            let _ = tx.send(Net::HuddleJoined { channel, result });
        });
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
                    self.open_dm_with(user);
                }
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
            let result = async {
                let bytes = tokio::fs::read(&cmd.path).await.map_err(|e| anyhow::anyhow!("{}: {e}", cmd.path.display()))?;
                let message = super::composer::escape_text(&cmd.message);
                client.upload(&channel, &name, bytes, Some(&message), thread_ts.as_deref()).await
            }
            .await;
            let _ = tx.send(Net::FileDone(match result {
                Ok(_) => format!("uploaded {name}"),
                Err(e) => format!("upload failed: {e}"),
            }));
        });
    }

    /// Save the attachments of message `ts` to the downloads folder.
    fn save_files(&mut self, ts: &str) {
        let Some(channel) = self.current.clone() else { return };
        let files = self.ws.find_message(&channel, ts).map(|m| m.files.clone()).unwrap_or_default();
        if files.is_empty() {
            self.status = "no files in this message".into();
            return;
        }
        let dir = super::files::download_dir();
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

    /// Open the viewer on the images of message `ts` in the current conversation, or play
    /// its first video.
    fn open_viewer(&mut self, ts: &str) {
        let Some(channel) = self.current.clone() else { return };
        let video = self.ws.find_message(&channel, ts).and_then(|m| m.files.iter().find(|f| f.is_video()).cloned());
        if let Some(file) = video {
            self.status = format!("loading {}…", file.name);
            let (client, tx) = (self.client.clone(), self.net.clone());
            tokio::spawn(async move {
                let title = file.name.clone();
                let result = super::video::download(client, file).await.map_err(|e| e.to_string());
                let _ = tx.send(Net::VideoReady { title, result });
            });
            return;
        }
        let files: Vec<slack_core::types::File> = self.ws.find_message(&channel, ts).map(|m| m.files.iter().filter(|f| f.is_image()).cloned().collect()).unwrap_or_default();
        if files.is_empty() {
            self.status = "no image in this message".into();
            return;
        }
        self.viewer = Some(super::viewer::Viewer::new(files, 0));
        self.load_viewer_image();
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
        let wanted: Vec<_> = self.messages.wanted.drain(..).chain(self.thread_pane.wanted.drain(..)).collect();
        for (file, size) in wanted {
            if self.media.thumbs.contains_key(&file.id) {
                continue;
            }
            self.media.thumbs.insert(file.id.clone(), super::media::Thumb::Loading);
            let (client, tx, picker) = (self.client.clone(), self.net.clone(), self.media.picker.clone());
            tokio::spawn(async move {
                let id = file.id.clone();
                let result = super::media::load_thumb(client, picker, file, size).await.map_err(|e| e.to_string());
                let _ = tx.send(Net::Thumb { id, result });
            });
        }
    }

    /// Focus order for Tab: left to right, top to bottom.
    fn focus_order(&self) -> Vec<Focus> {
        let mut order = vec![Focus::Sidebar, Focus::Messages, Focus::Composer];
        if self.thread.is_some() {
            order.extend([Focus::Thread, Focus::ThreadComposer]);
        }
        order
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

pub async fn run(client: Client) -> Result<()> {
    let (net_tx, mut net_rx) = mpsc::unbounded_channel();
    spawn_boot(client.clone(), net_tx.clone());
    let mut rtm = slack_core::rtm::spawn(client.clone());
    let (_guard, mut terminal) = TerminalGuard::enter()?;
    // Query the terminal before the event stream starts reading stdin.
    let media = super::media::Media::detect();
    let mut app = App {
        media,
        viewer: None,
        help: false,
        switcher: None,
        search: None,
        details: None,
        presence_subscribed: false,
        call: None,
        pending_hit: None,
        player: None,
        screen: Default::default(),
        terminal_focused: true,
        completion: None,
        completion_dismissed: None,
        composer_area: Default::default(),
        thread_composer_area: Default::default(),
        thread: None,
        thread_pane: MessagePane::thread(),
        thread_composer: Composer::default(),
        broadcast: false,
        clipboard: Default::default(),
        confirm_delete: None,
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
            team_id: client.session.team_id.clone(),
            team_name: client.session.team_name.clone(),
            self_id: client.session.user_id.clone(),
            ..Default::default()
        },
        sidebar: Sidebar::default(),
        messages: MessagePane::default(),
        current: None,
        status: "loading workspace…".into(),
        loading: true,
    };

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut video_tick = tokio::time::interval(Duration::from_millis(40));
    loop {
        terminal.draw(|f| draw(f, &mut app))?;
        app.request_thumbs();
        tokio::select! {
            ev = events.next() => {
                let Some(ev) = ev else { break };
                if !handle_terminal(&mut app, ev?) {
                    break;
                }
                app.refresh_completion();
            }
            Some(msg) = net_rx.recv() => handle_net(&mut app, msg),
            Some(ev) = rtm.events.recv() => handle_rtm(&mut app, ev),
            Some(ev) = async {
                match app.call.as_mut() {
                    Some(call) => call.session.events.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if app.call.as_mut().is_some_and(|c| c.apply(ev)) {
                    app.call = None;
                    app.status = "left the huddle".into();
                }
            }
            _ = tick.tick() => {}
            _ = video_tick.tick(), if app.player.is_some() => {
                if let Some(p) = app.player.as_mut() {
                    p.advance();
                }
            }
        }
        app.sync_current();
        if app.current.as_ref().and_then(|c| app.ws.timelines.get(c)).is_some_and(|t| t.loaded) {
            app.mark_current_read();
        }
    }
    app.clipboard.hand_off();
    Ok(())
}

/// Returns false to quit.
fn handle_terminal(app: &mut App, ev: Event) -> bool {
    match ev {
        Event::Key(k) if k.kind != KeyEventKind::Release && app.help => {
            if k.code == KeyCode::Char('q') && k.modifiers.contains(KeyModifiers::CONTROL) {
                return false;
            }
            app.help = false;
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.switcher.is_some() => {
            let len = app.switcher.as_ref().map(|s| s.entries(&app.ws).len()).unwrap_or(0);
            let Some(sw) = app.switcher.as_mut() else { return true };
            match k.code {
                KeyCode::Esc => app.switcher = None,
                KeyCode::Up => sw.move_by(-1, len),
                KeyCode::Down | KeyCode::Tab => sw.move_by(1, len),
                KeyCode::Enter => app.switch(),
                KeyCode::Backspace => {
                    sw.query.pop();
                    sw.selected = 0;
                }
                KeyCode::Char('q') if k.modifiers.contains(KeyModifiers::CONTROL) => return false,
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                    sw.query.push(c);
                    sw.selected = 0;
                }
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.details.is_some() => {
            let Some(d) = app.details.as_mut() else { return true };
            match k.code {
                KeyCode::Char('q') if k.modifiers.contains(KeyModifiers::CONTROL) => return false,
                KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('q') => app.details = None,
                KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => d.switch_tab(-1),
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => d.switch_tab(1),
                KeyCode::Up | KeyCode::Char('k') => d.move_by(-1),
                KeyCode::Down | KeyCode::Char('j') => d.move_by(1),
                KeyCode::Enter => app.details_enter(),
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && k.code == KeyCode::F(2) => app.open_details(),
        Event::Key(k) if k.kind != KeyEventKind::Release && k.code == KeyCode::F(3) => app.toggle_huddle(),
        Event::Key(k) if k.kind != KeyEventKind::Release && k.code == KeyCode::F(4) => {
            if let Some(call) = app.call.as_mut() {
                if let Err(e) = call.toggle_mute() {
                    app.status = format!("microphone: {e}");
                }
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.search.is_some() => {
            let Some(search) = app.search.as_mut() else { return true };
            match k.code {
                KeyCode::Esc => app.search = None,
                KeyCode::Up => search.move_by(-1),
                KeyCode::Down | KeyCode::Tab => search.move_by(1),
                KeyCode::Enter => app.search_enter(),
                KeyCode::Backspace => {
                    search.query.pop();
                }
                KeyCode::Char('q') if k.modifiers.contains(KeyModifiers::CONTROL) => return false,
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => search.query.push(c),
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && k.code == KeyCode::Char('f') && k.modifiers.contains(KeyModifiers::CONTROL) => {
            app.search = Some(super::search::Search::default());
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && k.code == KeyCode::Char('k') && k.modifiers.contains(KeyModifiers::CONTROL) => {
            app.switcher = Some(super::switcher::Switcher::default());
        }
        Event::Key(k)
            if k.kind != KeyEventKind::Release
                && (k.code == KeyCode::F(1) || k.code == KeyCode::Char('?') && matches!(app.focus, Focus::Sidebar | Focus::Messages | Focus::Thread)) =>
        {
            app.help = true;
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.player.is_some() => {
            let Some(p) = app.player.as_mut() else { return true };
            match k.code {
                KeyCode::Char('q') if k.modifiers.contains(KeyModifiers::CONTROL) => return false,
                KeyCode::Esc | KeyCode::Char('q') => app.player = None,
                KeyCode::Char(' ') | KeyCode::Char('p') => p.toggle_pause(),
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.viewer.is_some() => {
            let Some(v) = app.viewer.as_mut() else { return true };
            match k.code {
                KeyCode::Char('q') if k.modifiers.contains(KeyModifiers::CONTROL) => return false,
                KeyCode::Esc | KeyCode::Char('q') => app.viewer = None,
                KeyCode::Left | KeyCode::Char('h') if v.step(-1) => app.load_viewer_image(),
                KeyCode::Right | KeyCode::Char('l') if v.step(1) => app.load_viewer_image(),
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.confirm_delete.is_some() => {
            if let Some((channel, ts)) = app.confirm_delete.take() {
                if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    app.delete(channel, ts);
                }
            }
        }
        Event::Key(k)
            if k.kind != KeyEventKind::Release
                && app.completion.is_some()
                && k.modifiers.difference(KeyModifiers::SHIFT).is_empty()
                && matches!(k.code, KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter | KeyCode::Esc) =>
        {
            let Some(c) = app.completion.as_mut() else { return true };
            match k.code {
                KeyCode::Up | KeyCode::BackTab => c.move_by(-1),
                KeyCode::Down => c.move_by(1),
                KeyCode::Enter if k.modifiers.contains(KeyModifiers::SHIFT) => {
                    // Shift+Enter keeps its newline meaning.
                    let thread = c.thread;
                    app.completion = None;
                    app.composer_for(thread).insert('\n');
                }
                KeyCode::Tab | KeyCode::Enter => app.accept_completion(),
                _ => {
                    app.completion_dismissed = Some((c.thread, c.start));
                    app.completion = None;
                }
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release && app.picker.is_some() => {
            let custom: Vec<String> = app.ws.custom_emoji.keys().cloned().collect();
            let Some((picker, _, _)) = app.picker.as_mut() else { return true };
            let len = picker.candidates(&custom).len();
            match k.code {
                KeyCode::Esc => app.picker = None,
                KeyCode::Up => picker.move_by(-1, len),
                KeyCode::Down => picker.move_by(1, len),
                KeyCode::PageUp => picker.move_by(-10, len),
                KeyCode::PageDown => picker.move_by(10, len),
                KeyCode::Backspace => {
                    picker.query.pop();
                    picker.selected = 0;
                }
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                    picker.query.push(c);
                    picker.selected = 0;
                }
                KeyCode::Enter => {
                    if let Some((picker, channel, ts)) = app.picker.take() {
                        if let Some(choice) = picker.candidates(&custom).get(picker.selected) {
                            app.toggle_reaction(channel, ts, choice.name.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        Event::Key(k) if k.kind != KeyEventKind::Release => {
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            let alt = k.modifiers.contains(KeyModifiers::ALT);
            // Global keys.
            match k.code {
                KeyCode::Char('q') if ctrl => return false,
                KeyCode::Up if alt => {
                    app.sidebar.move_by(-1);
                    return true;
                }
                KeyCode::Down if alt => {
                    app.sidebar.move_by(1);
                    return true;
                }
                KeyCode::PageUp => {
                    app.messages.scroll_up(app.messages.area.height.saturating_sub(3) as usize);
                    app.maybe_load_older();
                    return true;
                }
                KeyCode::PageDown => {
                    app.messages.scroll_down(app.messages.area.height.saturating_sub(3) as usize);
                    return true;
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    let order = app.focus_order();
                    let pos = order.iter().position(|f| *f == app.focus).unwrap_or(0);
                    let step = if k.code == KeyCode::Tab { 1 } else { order.len() - 1 };
                    app.focus = order[(pos + step) % order.len()];
                    return true;
                }
                _ => {}
            }
            match app.focus {
                Focus::Sidebar => match k.code {
                    KeyCode::Up | KeyCode::Char('k') => app.sidebar.move_by(-1),
                    KeyCode::Down | KeyCode::Char('j') => app.sidebar.move_by(1),
                    KeyCode::Home | KeyCode::Char('g') => app.sidebar.move_by(isize::MIN / 2),
                    KeyCode::End | KeyCode::Char('G') => app.sidebar.move_by(isize::MAX / 2),
                    KeyCode::Enter | KeyCode::Char('i') | KeyCode::Esc => app.focus = Focus::Composer,
                    _ => {}
                },
                Focus::Messages | Focus::Thread => {
                    let in_thread = app.focus == Focus::Thread;
                    let timeline = match (&app.thread, in_thread) {
                        (Some((c, ts)), true) => app.ws.threads.get(&super::state::thread_key(c, ts)),
                        _ => app.current.as_ref().and_then(|c| app.ws.timelines.get(c)),
                    };
                    let order: Vec<String> = timeline.map(|t| super::messages::shown_messages(t, in_thread).map(|m| m.ts.clone()).collect()).unwrap_or_default();
                    let pane = if in_thread { &mut app.thread_pane } else { &mut app.messages };
                    match k.code {
                        KeyCode::Up | KeyCode::Char('k') => {
                            pane.move_selection(&order, -1);
                            if !in_thread && order.first() == app.messages.selected.as_ref() {
                                app.maybe_load_older();
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => pane.move_selection(&order, 1),
                        KeyCode::Home | KeyCode::Char('g') => pane.move_selection(&order, isize::MIN / 2),
                        KeyCode::End | KeyCode::Char('G') => pane.move_selection(&order, isize::MAX / 2),
                        KeyCode::Esc if in_thread => app.close_thread(),
                        KeyCode::Char('i') if in_thread => app.focus = Focus::ThreadComposer,
                        KeyCode::Esc | KeyCode::Char('i') => app.focus = Focus::Composer,
                        KeyCode::Enter if !in_thread => {
                            if let Some(ts) = app.messages.selected.clone() {
                                app.open_thread(&ts);
                            }
                        }
                        KeyCode::Char('r') => {
                            if let (Some(channel), Some(ts)) = (app.current.clone(), app.selected_ts()) {
                                app.picker = Some((EmojiPicker::default(), channel, ts));
                            }
                        }
                        KeyCode::Char('e') => {
                            if let Some(ts) = app.selected_ts() {
                                app.start_edit(&ts, in_thread);
                            }
                        }
                        KeyCode::Char('y') => {
                            if let Some(ts) = app.selected_ts() {
                                app.copy_text(&ts);
                            }
                        }
                        KeyCode::Char('P') => {
                            if let Some(ts) = app.selected_ts() {
                                app.toggle_pin(&ts);
                            }
                        }
                        KeyCode::Char('s') => {
                            if let Some(ts) = app.selected_ts() {
                                app.save_files(&ts);
                            }
                        }
                        KeyCode::Char('o') => {
                            if let Some(ts) = app.selected_ts() {
                                app.open_viewer(&ts);
                            }
                        }
                        KeyCode::Char('Y') => {
                            if let Some(ts) = app.selected_ts() {
                                app.copy_permalink(&ts);
                            }
                        }
                        KeyCode::Char('d') => {
                            if let Some(ts) = app.selected_ts() {
                                app.ask_delete(&ts);
                            }
                        }
                        _ => {}
                    }
                }
                Focus::Composer | Focus::ThreadComposer => {
                    let thread = app.focus == Focus::ThreadComposer;
                    if thread && alt && k.code == KeyCode::Char('a') {
                        app.broadcast = !app.broadcast;
                        return true;
                    }
                    match app.composer_for(thread).handle_key(k) {
                        KeyOutcome::Done => {}
                        KeyOutcome::Typed => app.notify_typing(),
                        KeyOutcome::Submit if app.composer_for(thread).editing.is_some() => app.save_edit(thread),
                        KeyOutcome::Submit => app.send(thread),
                        KeyOutcome::Leave => app.focus = if thread { Focus::Thread } else { Focus::Sidebar },
                        KeyOutcome::UpWhenEmpty => {
                            if let Some(ts) = app.last_own_message(thread) {
                                app.start_edit(&ts, thread);
                            }
                        }
                    }
                }
            }
        }
        Event::FocusGained | Event::FocusLost => {
            app.terminal_focused = matches!(ev, Event::FocusGained);
            tracing::debug!(focused = app.terminal_focused, "terminal focus");
        }
        Event::Paste(text) if app.focus == Focus::Composer => app.composer.insert_str(&text.replace("\r\n", "\n")),
        Event::Paste(text) if app.focus == Focus::ThreadComposer => app.thread_composer.insert_str(&text.replace("\r\n", "\n")),
        Event::Mouse(m) => match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if app.sidebar.click(m.column, m.row) {
                    app.focus = Focus::Composer;
                } else if app.messages.click(m.column, m.row) {
                    app.focus = Focus::Messages;
                } else if app.thread.is_some() && app.thread_pane.click(m.column, m.row) {
                    app.focus = Focus::Thread;
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
                if let Some((title, body)) = super::notify::huddle_notification(&app.ws, &channel, &user, in_it, now) {
                    super::notify::show(title, body);
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
                    super::notify::show(title, body);
                }
            }
        }
    }
}

fn handle_net(app: &mut App, msg: Net) {
    match msg {
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
            app.status.clear();
            app.subscribe_presence();
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
        }
        Net::Posted { channel, message } => super::live::add_message(&mut app.ws, &channel, &message),
        Net::Replies { channel, ts, page } => {
            let tl = app.ws.threads.entry(super::state::thread_key(&channel, &ts)).or_default();
            for m in page.messages {
                tl.upsert(m);
            }
            tl.loaded = true;
            tl.loading = false;
        }
        Net::FileDone(message) => app.status = message,
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
        Net::SearchDone { query, result } => {
            if let Some(search) = app.search.as_mut() {
                search.busy = false;
                match result {
                    Ok(v) => {
                        search.hits = super::search::parse_hits(&v);
                        search.total = v["messages"]["total"].as_u64().unwrap_or(search.hits.len() as u64);
                        search.searched = query;
                        search.selected = 0;
                    }
                    Err(e) => app.status = format!("search failed: {e}"),
                }
            }
        }
        Net::HuddleJoined { channel, result } => match result {
            Ok(creds) if app.call.is_none() => {
                app.call = Some(super::call::Call::start(channel, creds));
                app.status.clear();
            }
            Ok(_) => {}
            Err(e) => app.status = format!("cannot join the huddle: {e}"),
        },
        Net::DmOpened(c) => {
            let id = c.id.clone();
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
                    super::media::Thumb::Failed
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

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    app.screen = f.area();
    let call_rows = if app.call.is_some() { 1 } else { 0 };
    let [body, call_bar, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(call_rows), Constraint::Length(1)]).areas(f.area());
    if let Some(call) = &app.call {
        call.draw(f, call_bar, &app.ws);
    }
    let side_w = (body.width / 4).clamp(18, 32);
    let [side, main] = Layout::horizontal([Constraint::Length(side_w), Constraint::Min(10)]).areas(body);
    app.sidebar.draw(f, side, &app.ws, app.focus == Focus::Sidebar);
    let (main, thread_area) = if app.thread.is_some() {
        let [left, right] = Layout::horizontal([Constraint::Min(30), Constraint::Percentage(40)]).areas(main);
        (left, Some(right))
    } else {
        (main, None)
    };
    if let (Some(area), Some((channel, ts))) = (thread_area, app.thread.clone()) {
        let h = app.thread_composer.rows(area.width.saturating_sub(2) as usize).clamp(1, 8) + 2;
        let [pane, comp] = Layout::vertical([Constraint::Min(3), Constraint::Length(h)]).areas(area);
        let title = format!("Thread · {} · Esc close", app.ws.channel_label(&channel));
        let timeline = app.ws.threads.get(&super::state::thread_key(&channel, &ts));
        app.thread_pane.draw(f, pane, &app.ws, &title, timeline, app.focus == Focus::Thread, &app.media);
        app.thread_composer_area = comp;
        draw_composer(f, comp, app, true);
    }

    let inner_w = main.width.saturating_sub(2) as usize;
    let composer_h = app.composer.rows(inner_w).clamp(1, 8) + 2;
    let [msgs, comp] = Layout::vertical([Constraint::Min(3), Constraint::Length(composer_h)]).areas(main);
    let current = app.current.clone();
    let timeline = current.as_deref().and_then(|c| app.ws.timelines.get(c));
    let title = current.as_deref().map(|c| app.ws.channel_label(c)).unwrap_or_default();
    app.messages.draw(f, msgs, &app.ws, &title, timeline, app.focus == Focus::Messages, &app.media);
    app.composer_area = comp;
    draw_composer(f, comp, app, false);
    if let Some(c) = &app.completion {
        super::autocomplete::draw(f, if c.thread { app.thread_composer_area } else { app.composer_area }, c);
    }
    if let Some((picker, _, _)) = &app.picker {
        let custom: Vec<String> = app.ws.custom_emoji.keys().cloned().collect();
        picker.draw(f, main, &custom);
    }
    if app.confirm_delete.is_some() {
        draw_confirm(f, main, "Delete this message? y / n");
    }
    if let Some(v) = app.viewer.as_mut() {
        v.draw(f, body, &app.media.picker);
    }
    if let Some(p) = app.player.as_mut() {
        p.draw(f, body, &app.media.picker);
    }
    let live = if app.connected { "● live" } else { "○ connecting" };
    let (unread, mentions) = app.ws.total_unread();
    let info = if app.status.is_empty() { format!("{unread} unread · {mentions} mentions") } else { app.status.clone() };
    if let Some(sw) = &app.switcher {
        sw.draw(f, body, &app.ws);
    }
    if let Some(search) = &app.search {
        search.draw(f, body, &app.ws);
    }
    if let Some(d) = &app.details {
        d.draw(f, body, &app.ws);
    }
    if app.help {
        super::help::draw(f, body);
    }
    f.render_widget(Paragraph::new(format!(" {live} · {info} · F1 help · Tab focus · Alt+↑↓ channel · Ctrl+Q quit")), status);
}

fn draw_confirm(f: &mut ratatui::Frame, area: ratatui::layout::Rect, question: &str) {
    use ratatui::layout::Flex;
    use ratatui::style::{Color, Style};
    use ratatui::widgets::{Block, Borders, Clear};
    let [popup] = Layout::horizontal([Constraint::Length(question.chars().count() as u16 + 4)]).flex(Flex::Center).areas(area);
    let [popup] = Layout::vertical([Constraint::Length(3)]).flex(Flex::Center).areas(popup);
    f.render_widget(Clear, popup);
    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Rgb(0xe0, 0x6c, 0x75)));
    f.render_widget(Paragraph::new(format!(" {question}")).block(block), popup);
}

/// The channel composer, or the thread pane's (`thread`).
fn draw_composer(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App, thread: bool) {
    use ratatui::style::{Color, Style};
    use ratatui::widgets::{Block, Borders};
    let composer = if thread { &app.thread_composer } else { &app.composer };
    let focused = app.focus == if thread { Focus::ThreadComposer } else { Focus::Composer };
    let border = if focused { Style::default().fg(Color::Rgb(0x61, 0xaf, 0xef)) } else { Style::default().fg(super::text::DIM) };
    let target = app.current.as_deref().map(|c| app.ws.channel_label(c)).unwrap_or_default();
    let typing = if thread { vec![] } else { app.current.as_deref().map(|c| app.ws.typing_in(c)).unwrap_or_default() };
    let title = if composer.editing.is_some() {
        " Editing message · Enter save · Esc cancel ".to_string()
    } else if thread {
        format!(" Reply · [{}] also to channel (Alt+A) ", if app.broadcast { "x" } else { " " })
    } else {
        format!(" Message {target} ")
    };
    let mut block = Block::default().borders(Borders::ALL).border_style(border).title(title);
    if !typing.is_empty() {
        let who = if typing.len() == 1 { format!("{} is typing…", typing[0]) } else { format!("{} people are typing…", typing.len()) };
        block = block.title(ratatui::text::Line::styled(format!(" {who} "), Style::default().fg(super::text::DIM)).right_aligned());
    }
    let inner = block.inner(area);
    let width = inner.width as usize;
    let (row, col) = composer.cursor_position(width);
    // Scroll so the cursor line stays visible.
    let skip = row.saturating_sub(inner.height.saturating_sub(1));
    let text = if composer.text().is_empty() && !focused {
        let hint = if thread { "i to reply" } else { "Press Tab or click a conversation to type" };
        ratatui::text::Text::styled(hint, Style::default().fg(super::text::DIM))
    } else {
        ratatui::text::Text::raw(composer.text())
    };
    f.render_widget(Paragraph::new(text).wrap(ratatui::widgets::Wrap { trim: false }).scroll((skip, 0)).block(block), area);
    if focused {
        f.set_cursor_position((inner.x + col, inner.y + row - skip));
    }
}
