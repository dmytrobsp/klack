//! Main loop: terminal input, background API results and periodic ticks.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use futures_util::StreamExt;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use serde_json::Value;
use slack_core::Client;
use slack_core::types::User;
use tokio::sync::mpsc;

use super::sidebar::Sidebar;
use super::state::Workspace;
use super::terminal::TerminalGuard;
use super::text::DIM;

/// Results of background API calls.
enum Net {
    Boot { boot: Value, users: Vec<User>, counts: Option<Value> },
    Error(String),
}

struct App {
    ws: Workspace,
    sidebar: Sidebar,
    status: String,
    loading: bool,
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
        ws: Workspace {
            team_id: client.session.team_id.clone(),
            team_name: client.session.team_name.clone(),
            self_id: client.session.user_id.clone(),
            ..Default::default()
        },
        sidebar: Sidebar::default(),
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
                KeyCode::PageUp => app.sidebar.move_by(-10),
                KeyCode::PageDown => app.sidebar.move_by(10),
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

    let detail: Vec<Line> = match app.sidebar.selected_id().and_then(|id| app.ws.conversations.get(id)) {
        Some(c) => {
            let mut lines = vec![Line::styled(app.ws.conversation_label(c), Style::default().add_modifier(Modifier::BOLD)), Line::raw("")];
            if !c.topic.value.is_empty() {
                lines.push(Line::raw(format!("Topic: {}", c.topic.value)));
            }
            if !c.purpose.value.is_empty() {
                lines.push(Line::raw(format!("Purpose: {}", c.purpose.value)));
            }
            lines.push(Line::styled(format!("id {}", c.id), Style::default().fg(DIM)));
            lines
        }
        None if app.loading => vec![Line::raw("Loading…")],
        None => vec![Line::raw("No conversations")],
    };
    f.render_widget(Paragraph::new(detail).wrap(Wrap { trim: false }).block(Block::default().borders(Borders::ALL)), main);
    f.render_widget(Paragraph::new(format!(" {} · ↑↓/jk move · click select · Ctrl+Q quit", app.status)), status);
}
