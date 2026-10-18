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

use super::composer::Composer;
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
    /// An optimistic change failed: put the original message back.
    Restore { channel: String, message: slack_core::types::Message, error: String },
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Messages,
    Composer,
}

struct App {
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
        let Some(msg) = self.ws.timelines.get_mut(&channel).and_then(|t| t.messages.iter_mut().find(|m| m.ts == ts)) else { return };
        let had = msg.reactions.iter().any(|r| r.name == name && r.users.contains(&me));
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
                msg.reactions[i].users.push(me);
                msg.reactions[i].count += 1;
            }
            None => msg.reactions.push(slack_core::types::Reaction { name: name.clone(), users: vec![me], count: 1 }),
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

    /// Send the composer contents to the current conversation.
    fn send(&mut self) {
        let Some(channel) = self.current.clone() else { return };
        if self.composer.is_empty() {
            return;
        }
        let text = self.composer.to_mrkdwn();
        self.composer.push_history(self.composer.text());
        self.composer.clear();
        self.messages.scroll = 0;
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            let msg = match client.post_message(&channel, &text, None, false).await {
                Ok(message) => Net::Posted { channel, message },
                Err(e) => Net::Error(format!("send failed: {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    /// Load our own message `ts` in the current conversation into the composer for editing.
    fn start_edit(&mut self, ts: &str) {
        let Some(channel) = self.current.clone() else { return };
        let Some(m) = self.ws.timelines.get(&channel).and_then(|t| t.messages.iter().find(|m| m.ts == ts)) else { return };
        if m.user.as_deref() != Some(self.ws.self_id.as_str()) {
            self.status = "you can only edit your own messages".into();
            return;
        }
        let ws = &self.ws;
        let user_name = |id: &str| ws.user_name(id);
        let channel_name = |id: &str| ws.channel_label(id);
        self.composer.load_for_edit(ts, &m.text, user_name, channel_name);
        self.messages.selected = Some(ts.to_string());
        self.focus = Focus::Composer;
        self.status.clear();
    }

    /// Our newest top-level message in the current conversation.
    fn last_own_message(&self) -> Option<String> {
        let t = self.ws.timelines.get(self.current.as_deref()?)?;
        let own: Vec<&slack_core::types::Message> = super::messages::shown_messages(t).filter(|m| m.user.as_deref() == Some(self.ws.self_id.as_str()) && m.subtype.is_none()).collect();
        own.last().map(|m| m.ts.clone())
    }

    /// Save the message being edited (optimistic, then chat.update).
    fn save_edit(&mut self) {
        let (Some(channel), Some(ts)) = (self.current.clone(), self.composer.editing.clone()) else { return };
        if self.composer.is_empty() {
            self.status = "empty edit: use d on the message to delete it".into();
            return;
        }
        let text = self.composer.to_mrkdwn();
        self.composer.clear();
        let me = self.ws.self_id.clone();
        let Some(msg) = self.ws.timelines.get_mut(&channel).and_then(|t| t.messages.iter_mut().find(|m| m.ts == ts)) else { return };
        let original = msg.clone();
        msg.text = text.clone();
        msg.blocks.clear();
        let now = format!("{:.6}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64());
        msg.edited = Some(slack_core::types::Edited { user: me, ts: now });
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
        let own = self.ws.timelines.get(&channel).and_then(|t| t.messages.iter().find(|m| m.ts == ts)).is_some_and(|m| m.user.as_deref() == Some(self.ws.self_id.as_str()));
        if own {
            self.confirm_delete = Some((channel, ts.to_string()));
        } else {
            self.status = "you can only delete your own messages".into();
        }
    }

    /// Delete a message (optimistic, then chat.delete).
    fn delete(&mut self, channel: String, ts: String) {
        let Some(t) = self.ws.timelines.get_mut(&channel) else { return };
        let Some(original) = t.messages.iter().find(|m| m.ts == ts).cloned() else { return };
        t.remove(&ts);
        if self.messages.selected.as_deref() == Some(ts.as_str()) {
            self.messages.selected = None;
        }
        if self.composer.editing.as_deref() == Some(ts.as_str()) {
            self.composer.clear();
        }
        let (client, tx) = (self.client.clone(), self.net.clone());
        tokio::spawn(async move {
            if let Err(e) = client.delete_message(&channel, &ts).await {
                let _ = tx.send(Net::Restore { channel, message: original, error: format!("delete failed: {e}") });
            }
        });
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
    let mut app = App {
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

    let (_guard, mut terminal) = TerminalGuard::enter()?;
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        terminal.draw(|f| draw(f, &mut app))?;
        tokio::select! {
            ev = events.next() => {
                let Some(ev) = ev else { break };
                if !handle_terminal(&mut app, ev?) {
                    break;
                }
            }
            Some(msg) = net_rx.recv() => handle_net(&mut app, msg),
            Some(ev) = rtm.events.recv() => handle_rtm(&mut app, ev),
            _ = tick.tick() => {}
        }
        app.sync_current();
        if app.current.as_ref().and_then(|c| app.ws.timelines.get(c)).is_some_and(|t| t.loaded) {
            app.mark_current_read();
        }
    }
    Ok(())
}

/// Returns false to quit.
fn handle_terminal(app: &mut App, ev: Event) -> bool {
    match ev {
        Event::Key(k) if k.kind != KeyEventKind::Release && app.confirm_delete.is_some() => {
            if let Some((channel, ts)) = app.confirm_delete.take() {
                if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    app.delete(channel, ts);
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
            let shift = k.modifiers.contains(KeyModifiers::SHIFT);
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
                KeyCode::Tab => {
                    app.focus = match app.focus {
                        Focus::Sidebar => Focus::Messages,
                        Focus::Messages => Focus::Composer,
                        Focus::Composer => Focus::Sidebar,
                    };
                    return true;
                }
                KeyCode::BackTab => {
                    app.focus = match app.focus {
                        Focus::Sidebar => Focus::Composer,
                        Focus::Messages => Focus::Sidebar,
                        Focus::Composer => Focus::Messages,
                    };
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
                Focus::Messages => {
                    let order: Vec<String> = app
                        .current
                        .as_ref()
                        .and_then(|c| app.ws.timelines.get(c))
                        .map(|t| super::messages::shown_messages(t).map(|m| m.ts.clone()).collect())
                        .unwrap_or_default();
                    match k.code {
                        KeyCode::Up | KeyCode::Char('k') => {
                            app.messages.move_selection(&order, -1);
                            if order.first() == app.messages.selected.as_ref() {
                                app.maybe_load_older();
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => app.messages.move_selection(&order, 1),
                        KeyCode::Home | KeyCode::Char('g') => app.messages.move_selection(&order, isize::MIN / 2),
                        KeyCode::End | KeyCode::Char('G') => app.messages.move_selection(&order, isize::MAX / 2),
                        KeyCode::Esc | KeyCode::Char('i') => app.focus = Focus::Composer,
                        KeyCode::Char('r') => {
                            if let (Some(channel), Some(ts)) = (app.current.clone(), app.messages.selected.clone()) {
                                app.picker = Some((EmojiPicker::default(), channel, ts));
                            }
                        }
                        KeyCode::Char('e') => {
                            if let Some(ts) = app.messages.selected.clone() {
                                app.start_edit(&ts);
                            }
                        }
                        KeyCode::Char('d') => {
                            if let Some(ts) = app.messages.selected.clone() {
                                app.ask_delete(&ts);
                            }
                        }
                        _ => {}
                    }
                }
                Focus::Composer => {
                    if k.code == KeyCode::Up && app.composer.is_empty() && app.composer.editing.is_none() {
                        if let Some(ts) = app.last_own_message() {
                            app.start_edit(&ts);
                            return true;
                        }
                    }
                    let c = &mut app.composer;
                    match k.code {
                        KeyCode::Esc if c.editing.is_some() => c.clear(),
                        KeyCode::Esc => app.focus = Focus::Sidebar,
                        KeyCode::Enter if alt || shift => c.insert('\n'),
                        KeyCode::Char('j') if ctrl => c.insert('\n'),
                        KeyCode::Enter if c.editing.is_some() => app.save_edit(),
                        KeyCode::Enter => app.send(),
                        KeyCode::Char('b') if alt => c.toggle_marker('*'),
                        KeyCode::Char('i') if alt => c.toggle_marker('_'),
                        KeyCode::Char('s') if alt => c.toggle_marker('~'),
                        KeyCode::Char('c') if alt => c.toggle_marker('`'),
                        KeyCode::Char('w') if ctrl => c.delete_word_left(),
                        KeyCode::Char('u') if ctrl => c.kill_line(),
                        KeyCode::Char('a') if ctrl => c.home(),
                        KeyCode::Char('e') if ctrl => c.end(),
                        KeyCode::Char(ch) if !ctrl && !alt => {
                            c.insert(ch);
                            app.notify_typing();
                        }
                        KeyCode::Backspace if ctrl || alt => c.delete_word_left(),
                        KeyCode::Backspace => c.backspace(),
                        KeyCode::Delete => c.delete(),
                        KeyCode::Left if ctrl => c.word_left(),
                        KeyCode::Right if ctrl => c.word_right(),
                        KeyCode::Left => c.left(),
                        KeyCode::Right => c.right(),
                        KeyCode::Home => c.home(),
                        KeyCode::End => c.end(),
                        KeyCode::Up => {
                            c.up();
                        }
                        KeyCode::Down => {
                            c.down();
                        }
                        _ => {}
                    }
                }
            }
        }
        Event::Paste(text) if app.focus == Focus::Composer => app.composer.insert_str(&text.replace("\r\n", "\n")),
        Event::Mouse(m) => match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if app.sidebar.click(m.column, m.row) {
                    app.focus = Focus::Composer;
                } else if app.messages.click(m.column, m.row) {
                    app.focus = Focus::Messages;
                }
            }
            MouseEventKind::ScrollUp if app.sidebar.area.contains((m.column, m.row).into()) => app.sidebar.move_by(-1),
            MouseEventKind::ScrollDown if app.sidebar.area.contains((m.column, m.row).into()) => app.sidebar.move_by(1),
            MouseEventKind::ScrollUp if app.messages.area.contains((m.column, m.row).into()) => {
                app.messages.scroll_up(3);
                app.maybe_load_older();
            }
            MouseEventKind::ScrollDown if app.messages.area.contains((m.column, m.row).into()) => app.messages.scroll_down(3),
            _ => {}
        },
        _ => {}
    }
    true
}

fn handle_rtm(app: &mut App, ev: slack_core::rtm::RtmEvent) {
    use slack_core::rtm::RtmEvent;
    match ev {
        RtmEvent::Connected => app.connected = true,
        RtmEvent::Disconnected(reason) => {
            app.connected = false;
            tracing::warn!("rtm disconnected: {reason}");
        }
        RtmEvent::Event(v) => {
            let fx = super::live::apply(&mut app.ws, &v, app.current.as_deref());
            if fx.rebuild_sidebar {
                app.sidebar.rebuild(&app.ws);
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
        Net::Posted { channel, message } => {
            app.ws.timelines.entry(channel).or_default().upsert(message);
        }
        Net::Restore { channel, message, error } => {
            app.ws.timelines.entry(channel).or_default().upsert(message);
            app.status = error;
        }
        Net::Error(e) => {
            app.loading = false;
            app.status = e;
        }
    }
}

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let [body, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(f.area());
    let side_w = (body.width / 4).clamp(18, 32);
    let [side, main] = Layout::horizontal([Constraint::Length(side_w), Constraint::Min(10)]).areas(body);
    app.sidebar.draw(f, side, &app.ws, app.focus == Focus::Sidebar);

    let inner_w = main.width.saturating_sub(2) as usize;
    let composer_h = app.composer.rows(inner_w).clamp(1, 8) + 2;
    let [msgs, comp] = Layout::vertical([Constraint::Min(3), Constraint::Length(composer_h)]).areas(main);
    let current = app.current.clone();
    let timeline = current.as_deref().and_then(|c| app.ws.timelines.get(c));
    app.messages.draw(f, msgs, &app.ws, current.as_deref(), timeline, app.focus == Focus::Messages);
    draw_composer(f, comp, app);
    if let Some((picker, _, _)) = &app.picker {
        let custom: Vec<String> = app.ws.custom_emoji.keys().cloned().collect();
        picker.draw(f, main, &custom);
    }
    if app.confirm_delete.is_some() {
        draw_confirm(f, main, "Delete this message? y / n");
    }
    let live = if app.connected { "● live" } else { "○ connecting" };
    let (unread, mentions) = app.ws.total_unread();
    let info = if app.status.is_empty() { format!("{unread} unread · {mentions} mentions") } else { app.status.clone() };
    f.render_widget(Paragraph::new(format!(" {live} · {info} · Enter send · Alt+Enter newline · Tab focus · Alt+↑↓ channel · PgUp/PgDn scroll · Ctrl+Q quit")), status);
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

fn draw_composer(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    use ratatui::style::{Color, Style};
    use ratatui::widgets::{Block, Borders};
    let focused = app.focus == Focus::Composer;
    let border = if focused { Style::default().fg(Color::Rgb(0x61, 0xaf, 0xef)) } else { Style::default().fg(super::text::DIM) };
    let target = app.current.as_deref().map(|c| app.ws.channel_label(c)).unwrap_or_default();
    let typing = app.current.as_deref().map(|c| app.ws.typing_in(c)).unwrap_or_default();
    let title = if app.composer.editing.is_some() { " Editing message · Enter save · Esc cancel ".to_string() } else { format!(" Message {target} ") };
    let mut block = Block::default().borders(Borders::ALL).border_style(border).title(title);
    if !typing.is_empty() {
        let who = if typing.len() == 1 { format!("{} is typing…", typing[0]) } else { format!("{} people are typing…", typing.len()) };
        block = block.title(ratatui::text::Line::styled(format!(" {who} "), Style::default().fg(super::text::DIM)).right_aligned());
    }
    let inner = block.inner(area);
    let width = inner.width as usize;
    let (row, col) = app.composer.cursor_position(width);
    // Scroll so the cursor line stays visible.
    let skip = row.saturating_sub(inner.height.saturating_sub(1));
    let text = if app.composer.text().is_empty() && !focused {
        ratatui::text::Text::styled("Press Tab or click a conversation to type", Style::default().fg(super::text::DIM))
    } else {
        ratatui::text::Text::raw(app.composer.text())
    };
    f.render_widget(Paragraph::new(text).wrap(ratatui::widgets::Wrap { trim: false }).scroll((skip, 0)).block(block), area);
    if focused {
        f.set_cursor_position((inner.x + col, inner.y + row - skip));
    }
}
