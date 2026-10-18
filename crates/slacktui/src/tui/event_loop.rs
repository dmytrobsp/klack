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

use super::messages::MessagePane;
use super::sidebar::Sidebar;
use super::state::Workspace;
use super::terminal::TerminalGuard;

/// Results of background API calls.
enum Net {
    Boot { boot: Value, users: Vec<User>, counts: Option<Value> },
    /// A page of history; `older` pages are prepended.
    History { channel: String, page: HistoryPage, older: bool },
    Error(String),
}

struct App {
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
            if let Some(id) = selected {
                self.load_history(&id, false);
            }
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
    let mut app = App {
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
            _ = tick.tick() => {}
        }
        app.sync_current();
    }
    Ok(())
}

/// Returns false to quit.
fn handle_terminal(app: &mut App, ev: Event) -> bool {
    match ev {
        Event::Key(k) if k.kind != KeyEventKind::Release => {
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            match k.code {
                KeyCode::Char('q') if ctrl => return false,
                KeyCode::Up | KeyCode::Char('k') => app.sidebar.move_by(-1),
                KeyCode::Down | KeyCode::Char('j') => app.sidebar.move_by(1),
                KeyCode::PageUp => {
                    app.messages.scroll_up(app.messages.area.height.saturating_sub(3) as usize);
                    app.maybe_load_older();
                }
                KeyCode::PageDown => app.messages.scroll_down(app.messages.area.height.saturating_sub(3) as usize),
                KeyCode::Home | KeyCode::Char('g') => app.sidebar.move_by(isize::MIN / 2),
                KeyCode::End | KeyCode::Char('G') => app.sidebar.move_by(isize::MAX / 2),
                _ => {}
            }
        }
        Event::Mouse(m) => match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                app.sidebar.click(m.column, m.row);
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
            let (unread, mentions) = app.ws.total_unread();
            app.status = format!("{} conversations · {unread} unread · {mentions} mentions", app.ws.conversations.len());
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
    app.sidebar.draw(f, side, &app.ws, true);

    let current = app.current.clone();
    let timeline = current.as_deref().and_then(|c| app.ws.timelines.get(c));
    app.messages.draw(f, main, &app.ws, current.as_deref(), timeline);
    f.render_widget(Paragraph::new(format!(" {} · ↑↓/jk channel · PgUp/PgDn or wheel scroll · Ctrl+Q quit", app.status)), status);
}
