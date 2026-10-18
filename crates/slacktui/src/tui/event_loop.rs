//! Main loop: terminal input and periodic ticks, redrawing after every change.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use futures_util::StreamExt;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::terminal::TerminalGuard;

/// Placeholder state until the real panes are added.
#[derive(Default)]
struct Placeholder {
    last_event: String,
    ticks: u64,
}

pub async fn run() -> Result<()> {
    let (_guard, mut terminal) = TerminalGuard::enter()?;
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut state = Placeholder::default();
    loop {
        terminal.draw(|f| {
            let [body, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(f.area());
            let text = vec![
                Line::styled("slacktui", Style::default().add_modifier(Modifier::BOLD)),
                Line::raw(""),
                Line::raw(format!("last input: {}", if state.last_event.is_empty() { "—" } else { &state.last_event })),
            ];
            f.render_widget(Paragraph::new(text).alignment(Alignment::Center).block(Block::default().borders(Borders::ALL)), body);
            f.render_widget(Paragraph::new(format!(" Ctrl+Q quit · ticks {}", state.ticks)), status);
        })?;
        tokio::select! {
            ev = events.next() => {
                let Some(ev) = ev else { break };
                match ev? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => {
                        if k.code == KeyCode::Char('q') && k.modifiers.contains(KeyModifiers::CONTROL) {
                            break;
                        }
                        state.last_event = format!("key {:?} {:?}", k.code, k.modifiers);
                    }
                    Event::Mouse(m) if !matches!(m.kind, MouseEventKind::Moved) => {
                        state.last_event = format!("mouse {:?} at {},{}", m.kind, m.column, m.row);
                    }
                    Event::Paste(p) => state.last_event = format!("paste ({} chars)", p.chars().count()),
                    Event::Resize(w, h) => state.last_event = format!("resize {w}x{h}"),
                    _ => {}
                }
            }
            _ = tick.tick() => state.ticks += 1,
        }
    }
    Ok(())
}
