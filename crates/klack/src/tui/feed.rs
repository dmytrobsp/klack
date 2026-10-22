//! The Threads view's feed: one block per thread — a heading saying where it is and who is in it, its
//! parent in full, "N more replies", its last replies in full with the "new" rule where the unread ones
//! start, a rule between it and the next — built with the conversation's own message lines, so they
//! look the same; and when a block has been on screen long enough to count as read.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::{Duration, Instant};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use slack_core::types::Message;

use super::applink::Here;
use super::i18n::tr;
use super::messages::{self, Avatars, ImageSlot, ThumbSize};
use super::threads::Entry;

/// Replies shown under a thread's parent (all its unread ones, when there are more).
pub const REPLIES_SHOWN: usize = 3;
/// A longer parent is cut after this many lines ("… more"; Enter opens the whole thread).
pub const ROOT_LINES: usize = 12;
/// Names in a block's header before "and N others".
const PEOPLE_SHOWN: usize = 3;

const MISSING: &str = "message not available";
const CUT: &str = "… more";
const MORE_ONE: &str = "1 more reply";
const MORE: &str = "{n} more replies";
const OTHERS: &str = "and {n} others";
const NEW: &str = "{n} new";
/// The line between the threads with new replies (first) and the rest.
const UP_TO_DATE: &str = "You're up to date";
/// The composer's title under the feed (`{where}`: the thread's conversation).
pub const REPLY_IN: &str = "Reply in {where} thread";
/// The composer's placeholder under the feed (`{key}`: the Type key).
pub const PRESS_TO_REPLY: &str = "Press {key} to reply";

/// The feed's words, for the locale template.
pub const WORDS: &[&str] = &[MISSING, CUT, MORE_ONE, MORE, OTHERS, NEW, UP_TO_DATE, REPLY_IN, PRESS_TO_REPLY];

/// A thread's place in the feed: `rows` are its lines (header to its last reply, not the blank rows and
/// rule after it), `unread_rows` those of its unread replies with the "new" rule (empty: none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub channel: String,
    pub thread_ts: String,
    pub rows: Range<usize>,
    pub unread_rows: Range<usize>,
}

/// The feed built: its lines, the message (channel, ts) each line belongs to, the blocks and
/// where pictures go.
#[derive(Debug, Default)]
pub struct Feed {
    pub lines: Vec<Line<'static>>,
    pub owners: Vec<Option<(String, String)>>,
    pub blocks: Vec<Block>,
    pub slots: Vec<ImageSlot>,
}

/// The feed of `entries` `width` columns wide (the view's inside), each block with at most
/// `replies_shown` replies (all its unread ones when there are more): its lines, the message each
/// line belongs to, the blocks (the view draws [`build`]'s, with pictures).
#[cfg(test)]
pub fn layout(here: &Here, entries: &[Entry], width: u16, replies_shown: usize) -> (Vec<Line<'static>>, Vec<Option<(String, String)>>, Vec<Block>) {
    let unread_first = entries.iter().take_while(|e| !e.unread.is_empty()).count();
    let feed = build(here, entries, width, replies_shown, unread_first, &|_, _| None, Avatars::Off);
    (feed.lines, feed.owners, feed.blocks)
}

/// [`layout`] with pictures (`thumb_size`, `avatars`), as the view draws it.
pub fn build(here: &Here, entries: &[Entry], width: u16, replies_shown: usize, unread_first: usize, thumb_size: ThumbSize, avatars: Avatars) -> Feed {
    let mut feed = Feed::default();
    // As wide as a conversation's text in a pane this wide (the gutter goes before it).
    let body = (width as usize).saturating_sub(1 + messages::GUTTER as usize);
    // "You're up to date" under the `unread_first` threads that had new replies when loaded (they come first), when
    // others follow them.
    let up_to_date = Some(unread_first).filter(|&n| n > 0 && n < entries.len());
    let dim = Style::default().fg(super::theme::p().dim);
    for (n, e) in entries.iter().enumerate() {
        let start = feed.lines.len();
        let (lines, owners, slots, unread) = block(here, e, body, replies_shown, thumb_size, avatars);
        let shift = |r: Range<usize>| if r.is_empty() { 0..0 } else { r.start + start..r.end + start };
        feed.blocks.push(Block { channel: e.channel.clone(), thread_ts: e.root.ts.clone(), rows: start..start + lines.len(), unread_rows: shift(unread) });
        feed.slots.extend(slots.into_iter().map(|mut s| {
            s.line += start;
            s
        }));
        feed.lines.extend(lines);
        feed.owners.extend(owners.into_iter().map(|o| o.map(|ts| (e.channel.clone(), ts))));
        // Set apart from the next thread by a full-width rule with a blank row on each side (the last one: the blank row).
        if n + 1 < entries.len() {
            feed.lines.push(Line::raw(""));
            feed.owners.push(None);
        }
        if up_to_date == Some(n + 1) {
            feed.lines.push(messages::labelled_rule(&tr(UP_TO_DATE), width as usize, dim));
            feed.owners.push(None);
        } else if n + 1 < entries.len() {
            feed.lines.push(Line::styled("─".repeat(width as usize), dim));
            feed.owners.push(None);
        }
        feed.lines.push(Line::raw(""));
        feed.owners.push(None);
    }
    feed
}

/// The replies known of a thread, oldest first.
fn replies(e: &Entry) -> Vec<&Message> {
    let mut all: Vec<&Message> = e.latest.iter().chain(&e.unread).collect();
    all.sort_by(|a, b| a.time().total_cmp(&b.time()));
    all.dedup_by(|a, b| a.ts == b.ts);
    all
}

/// Nothing of the message is there to show (deleted, or Slack left it out).
fn missing(m: &Message) -> bool {
    m.text.trim().is_empty() && m.blocks.is_empty() && m.files.is_empty() && m.attachments.is_empty()
}

/// One thread's lines (`width`: the text's), the message (ts) each belongs to, its pictures and its unread rows.
fn block(here: &Here, e: &Entry, width: usize, replies_shown: usize, thumb_size: ThumbSize, avatars: Avatars) -> (Vec<Line<'static>>, Vec<Option<String>>, Vec<ImageSlot>, Range<usize>) {
    let ws = here.ws;
    let dim = Style::default().fg(super::theme::p().dim);
    let all = replies(e);
    let unread: HashSet<&str> = e.unread.iter().map(|m| m.ts.as_str()).collect();
    let unread_from = all.iter().position(|m| unread.contains(m.ts.as_str())).unwrap_or(all.len());
    let shown = replies_shown.max(all.len() - unread_from).min(all.len());
    let replies = &all[all.len() - shown..];
    let hidden = (e.root.reply_count as usize).max(all.len()).saturating_sub(shown);
    let more = (hidden > 0).then(|| {
        let label = if hidden == 1 { tr(MORE_ONE).into_owned() } else { tr(MORE).replace("{n}", &hidden.to_string()) };
        Line::from(Span::styled(format!("  {label}"), dim.add_modifier(Modifier::ITALIC)))
    });
    let shown_messages: Vec<&Message> = std::iter::once(&e.root).chain(replies.iter().copied()).collect();
    let first_unread = replies.iter().find(|m| unread.contains(m.ts.as_str())).map(|m| m.ts.as_str());
    let (mut lines, mut owners, mut slots) = messages::build_shown(here, &shown_messages, 0, shown_messages.len(), width, true, more, thumb_size, avatars, first_unread);
    // A message with nothing to show says so, in place of an empty author line.
    for m in shown_messages.iter().filter(|m| missing(m)) {
        if let Some(at) = lines_of(&owners, &m.ts) {
            replace(&mut lines, &mut owners, &mut slots, at, Line::from(Span::styled(tr(MISSING).into_owned(), dim.add_modifier(Modifier::ITALIC))), Some(m.ts.clone()));
        }
    }
    // A long parent is cut (Enter opens the whole thread).
    if let Some(at) = lines_of(&owners, &e.root.ts).filter(|r| r.len() > ROOT_LINES + 1) {
        replace(&mut lines, &mut owners, &mut slots, at.start + ROOT_LINES..at.end, Line::from(Span::styled(tr(CUT).into_owned(), dim)), Some(e.root.ts.clone()));
    }
    // The "new" rule above the first unread reply.
    let mut unread_rows = 0..0;
    if let Some(at) = first_unread.and_then(|ts| owners.iter().position(|o| o.as_deref() == Some(ts))) {
        lines.insert(at, messages::new_rule(width));
        owners.insert(at, None);
        for slot in slots.iter_mut().filter(|s| s.line >= at) {
            slot.line += 1;
        }
        unread_rows = at..lines.len();
    }
    let own: HashSet<&str> = shown_messages.iter().filter(|m| m.user.as_deref() == Some(ws.self_id.as_str()) && !messages::is_system(m)).map(|m| m.ts.as_str()).collect();
    let system: HashSet<&str> = shown_messages.iter().filter(|m| messages::is_system(m)).map(|m| m.ts.as_str()).collect();
    let authors: HashMap<&str, &str> = shown_messages.iter().filter_map(|m| Some((m.ts.as_str(), m.user.as_deref()?))).collect();
    messages::add_gutter(&mut lines, &owners, &own, &system, &authors);
    // The header on top, flush with the pane's edge.
    lines.insert(0, header(ws, e));
    owners.insert(0, None);
    for slot in &mut slots {
        slot.line += 1;
    }
    let unread_rows = if unread_rows.is_empty() { unread_rows } else { unread_rows.start + 1..unread_rows.end + 1 };
    (lines, owners, slots, unread_rows)
}

/// The lines message `ts` takes.
fn lines_of(owners: &[Option<String>], ts: &str) -> Option<Range<usize>> {
    let first = owners.iter().position(|o| o.as_deref() == Some(ts))?;
    let last = owners.iter().rposition(|o| o.as_deref() == Some(ts))?;
    Some(first..last + 1)
}

/// Put `line` (owned by `owner`) in place of lines `at`; the pictures there go, those below move up.
fn replace(lines: &mut Vec<Line<'static>>, owners: &mut Vec<Option<String>>, slots: &mut Vec<ImageSlot>, at: Range<usize>, line: Line<'static>, owner: Option<String>) {
    let gone = at.len();
    lines.splice(at.clone(), [line]);
    owners.splice(at.clone(), [owner]);
    slots.retain(|s| s.line + s.size.1 as usize <= at.start || s.line >= at.end);
    for s in slots.iter_mut().filter(|s| s.line >= at.end) {
        s.line = s.line + 1 - gone;
    }
}

/// Where a thread is: `#channel`, a DM's person, a group DM's people.
pub fn place(ws: &super::state::Workspace, channel: &str) -> String {
    match ws.conversations.get(channel) {
        Some(c) if c.is_dm() => ws.conversation_label(c),
        Some(c) => format!("#{}", c.name),
        None => channel.to_string(),
    }
}

/// Who is in a thread: its author and those who replied, "Ann, Bob, Carol and 2 others".
fn people(ws: &super::state::Workspace, e: &Entry) -> String {
    let mut ids: Vec<&str> = Vec::new();
    for id in e.root.user.iter().chain(&e.root.reply_users).chain(replies(e).iter().filter_map(|m| m.user.as_ref())) {
        if !ids.contains(&id.as_str()) {
            ids.push(id);
        }
    }
    let names: Vec<String> = ids.iter().take(PEOPLE_SHOWN).map(|id| ws.user_name(id)).collect();
    let mut out = names.join(", ");
    if ids.len() > PEOPLE_SHOWN {
        out += &format!(" {}", tr(OTHERS).replace("{n}", &(ids.len() - PEOPLE_SHOWN).to_string()));
    }
    out
}

/// "▌#general · Ann, Bob and Carol · 18 Oct  2 new": where, who, the parent's day when not today, the unread count.
fn header(ws: &super::state::Workspace, e: &Entry) -> Line<'static> {
    let p = super::theme::p();
    let unread = !e.unread.is_empty() && !e.read;
    // A heading: where and who in bold.
    let style = if unread { Style::default().add_modifier(Modifier::BOLD).fg(p.strong) } else { Style::default().add_modifier(Modifier::BOLD) };
    let mut spans = vec![super::sidebar::unread_mark(unread), Span::styled(place(ws, &e.channel), style)];
    let who = people(ws, e);
    if !who.is_empty() {
        spans.push(Span::styled(format!(" · {who}"), Style::default().add_modifier(Modifier::BOLD)));
    }
    let today = super::clock::now().date_naive();
    if super::clock::at(e.root.time() as i64).is_some_and(|d| d.date_naive() != today) {
        spans.push(Span::styled(format!(" · {}", messages::day_label(ws, e.root.time())), Style::default().fg(p.dim)));
    }
    if unread {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(format!(" {} ", tr(NEW).replace("{n}", &e.unread.len().to_string())), Style::default().fg(p.badge_fg).bg(p.badge_bg).add_modifier(Modifier::BOLD)));
    }
    Line::from(spans)
}

/// When the blocks with unread replies came on screen (and stayed), to mark them read after a while.
#[derive(Debug, Default)]
pub struct ReadClock {
    since: HashMap<(String, String), Instant>,
    done: HashSet<(String, String)>,
}

impl ReadClock {
    /// The feed's rows `visible` are on screen at `now`: the threads to mark read now (each once) —
    /// those whose whole block has been on screen for `after` (zero: never); for a block taller than
    /// the view, its unread rows (unread rows taller than the view: their end).
    pub fn tick(&mut self, visible: Range<usize>, blocks: &[Block], now: Instant, after: Duration) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut on = HashSet::new();
        let within = |r: &Range<usize>| visible.start <= r.start && r.end <= visible.end;
        for b in blocks.iter().filter(|b| !b.unread_rows.is_empty() && !after.is_zero()) {
            let key = (b.channel.clone(), b.thread_ts.clone());
            // A block that fits counts only whole; a taller one by its unread rows (taller still: their end).
            let taller = b.rows.len() > visible.len();
            let shown = within(&b.rows) || (taller && (within(&b.unread_rows) || (b.unread_rows.len() > visible.len() && visible.contains(&(b.unread_rows.end - 1)))));
            if self.done.contains(&key) || !shown {
                continue;
            }
            let since = *self.since.entry(key.clone()).or_insert(now);
            if now.saturating_duration_since(since) >= after {
                self.done.insert(key.clone());
                out.push(key);
            } else {
                on.insert(key);
            }
        }
        // Off screen (or scrolled past): its clock starts again.
        self.since.retain(|k, _| on.contains(k));
        out
    }

    /// Thread `thread_ts` of `channel` has new replies: it counts again.
    pub fn forget(&mut self, channel: &str, thread_ts: &str) {
        let key = (channel.to_string(), thread_ts.to_string());
        self.done.remove(&key);
        self.since.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slack_core::types::Message;

    fn msg(ts: &str, user: &str, text: &str) -> Message {
        Message { ts: ts.into(), thread_ts: Some("1729440000.000100".into()), user: Some(user.into()), text: text.into(), ..Default::default() }
    }

    fn plain(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }

    fn ws() -> super::super::state::Workspace {
        let mut ws = super::super::state::Workspace::default();
        ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "general".into(), is_channel: true, ..Default::default() });
        ws
    }

    /// A thread in C1: its parent and replies `n`… (an hour apart, by U2 and U3 in turn), the last `unread` of them unread.
    fn entry(replies: usize, unread: usize) -> Entry {
        let mut root = msg("1729440000.000100", "U1", "the parent");
        root.reply_count = replies as u64;
        root.reply_users = vec!["U2".into(), "U3".into()];
        let all: Vec<Message> = (1..=replies).map(|n| msg(&format!("{}.000100", 1729440000 + n * 3600), ["U2", "U3"][n % 2], &format!("reply {n}"))).collect();
        let (latest, unread) = all.split_at(replies - unread);
        Entry { channel: "C1".into(), root, unread: unread.to_vec(), latest: latest.to_vec(), ..Default::default() }
    }

    /// The runs of lines with one owner: (owner's ts, first line, line count).
    fn runs(owners: &[Option<(String, String)>]) -> Vec<(Option<String>, usize, usize)> {
        let mut out: Vec<(Option<String>, usize, usize)> = Vec::new();
        for (n, o) in owners.iter().enumerate() {
            let ts = o.as_ref().map(|(_, ts)| ts.clone());
            match out.last_mut() {
                Some(last) if last.0 == ts => last.2 += 1,
                _ => out.push((ts, n, 1)),
            }
        }
        out
    }

    #[test]
    fn block_has_header_root_more_and_last_replies() {
        let ws = ws();
        let (lines, owners, blocks) = layout(&Here::alone(&ws), &[entry(5, 0)], 60, 2);
        assert_eq!(lines.len(), owners.len());
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert!(owners.iter().flatten().all(|(c, _)| c == "C1"), "every message line is its (channel, ts)");
        let order: Vec<(Option<String>, usize, usize)> = runs(&owners);
        let messages: Vec<&str> = order.iter().filter_map(|r| r.0.as_deref()).collect();
        assert_eq!(messages, ["1729440000.000100", "1729454400.000100", "1729458000.000100"], "the parent, then the last two replies");
        // The header first, owning nothing: where and who.
        assert_eq!(order[0].0, None);
        assert!(text[0].contains("#general") && text[0].contains("U1") && text[0].contains("U2"), "{text:#?}");
        // The parent in full (its author and its text).
        let root = &order[1];
        assert!(text[root.1..root.1 + root.2].iter().any(|l| l.contains("U1")) && text[root.1..root.1 + root.2].iter().any(|l| l.contains("the parent")), "{text:#?}");
        // "3 more replies" between it and the replies shown, owning nothing.
        let more = text.iter().position(|l| l.contains("3 more replies")).expect("the hidden replies are counted");
        assert_eq!(owners[more], None);
        assert!(more >= root.1 + root.2 && more < order[3].1, "{text:#?}");
        assert!(text.iter().any(|l| l.contains("reply 4")) && text.iter().any(|l| l.contains("reply 5")) && !text.iter().any(|l| l.contains("reply 3")));
        // A blank row after the block, owning nothing.
        assert_eq!((text.last().map(|l| l.trim().is_empty()), owners.last()), (Some(true), Some(&None)));
        assert_eq!(blocks, [Block { channel: "C1".into(), thread_ts: "1729440000.000100".into(), rows: 0..lines.len() - 1, unread_rows: 0..0 }]);
        // Two threads: one after the other.
        let (lines2, _, blocks) = layout(&Here::alone(&ws), &[entry(5, 0), entry(1, 0)], 60, 2);
        assert_eq!(blocks[1].rows.start, lines.len() + 2, "the second block starts after the first one's blank row, rule and blank row");
        assert_eq!(blocks[1].rows.end, lines2.len() - 1);
    }

    /// Three threads (in C1, C2, C3) with 2, 1 and 3 replies.
    fn three() -> Vec<Entry> {
        [("C1", 2), ("C2", 1), ("C3", 3)].into_iter().map(|(c, n)| Entry { channel: c.into(), ..entry(n, 0) }).collect()
    }

    #[test]
    fn threads_are_set_apart_by_a_rule_between_blank_rows() {
        let ws = ws();
        let dim = super::super::theme::p().dim;
        let (lines, owners, blocks) = layout(&Here::alone(&ws), &three(), 60, 3);
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].rows.start, 0, "nothing above the first thread");
        for pair in blocks.windows(2) {
            let rule = pair[0].rows.end + 1;
            assert!(text[rule - 1].trim().is_empty(), "a blank row right after the thread: {text:#?}");
            assert_eq!(text[rule], "─".repeat(60), "then a full-width rule: {text:#?}");
            assert!(lines[rule].spans.iter().all(|s| s.style.fg == Some(dim)) || lines[rule].style.fg == Some(dim), "dim");
            assert!(text[rule + 1].trim().is_empty(), "then a blank row: {text:#?}");
            assert_eq!(pair[1].rows.start, rule + 2, "then the next thread's header");
            assert_eq!((&owners[rule - 1], &owners[rule], &owners[rule + 1]), (&None, &None, &None), "they belong to no message");
        }
        let last = blocks[2].rows.end;
        assert_eq!((lines.len(), text[last].trim()), (last + 1, ""), "after the last thread: only the blank row");
        // The header reads as a heading: where and who in bold, who no longer dim.
        let header = &lines[blocks[1].rows.start];
        assert!(plain(header).contains("C2") && plain(header).contains("U1"), "{:?}", plain(header));
        let (place, who) = (&header.spans[1], &header.spans[2]);
        assert!(place.style.add_modifier.contains(Modifier::BOLD) && who.style.add_modifier.contains(Modifier::BOLD), "{header:?}");
        assert_ne!(who.style.fg, Some(dim), "{header:?}");
    }

    #[test]
    fn up_to_date_sits_between_the_unread_threads_and_the_read_ones() {
        let ws = ws();
        let dim = super::super::theme::p().dim;
        let with = |unread: &[bool]| -> Vec<Entry> { unread.iter().enumerate().map(|(n, &u)| Entry { channel: format!("C{n}"), ..entry(2, if u { 1 } else { 0 }) }).collect() };
        let divider = |lines: &[Line]| lines.iter().map(plain).position(|l| l.contains("You're up to date"));
        let (lines, owners, blocks) = layout(&Here::alone(&ws), &with(&[true, true, false, false]), 60, 3);
        let text: Vec<String> = lines.iter().map(plain).collect();
        let at = divider(&lines).expect("drawn");
        assert_eq!(at, blocks[1].rows.end + 1, "in place of the rule after the last unread thread: {text:#?}");
        assert!(text[at].starts_with('─') && text[at].ends_with('─') && text[at].chars().count() == 60, "a line across, as the \"new\" one: {:?}", text[at]);
        assert_eq!(lines[at].style.fg, Some(dim));
        assert!(text[at + 1].trim().is_empty() && blocks[2].rows.start == at + 2, "then the blank row and the first read thread");
        assert_eq!((&owners[at], blocks.iter().any(|b| b.rows.contains(&at))), (&None, false), "no message's, no thread's");
        assert_eq!(text.iter().filter(|l| l.contains("You're up to date")).count(), 1, "once");
        assert_eq!(text[blocks[0].rows.end + 1], "─".repeat(60), "the other rules stay plain");
        assert_eq!(text[blocks[2].rows.end + 1], "─".repeat(60));
        // Every thread unread, or none: no line.
        for all in [[true, true, true], [false, false, false]] {
            assert_eq!(divider(&layout(&Here::alone(&ws), &with(&all), 60, 3).0), None, "{all:?}");
        }
        // Read here: still above it (its "new" line stays until the view loads again).
        let mut read_here = with(&[true, false]);
        read_here[0].read = true;
        let (lines, _, blocks) = layout(&Here::alone(&ws), &read_here, 60, 3);
        assert_eq!(divider(&lines), Some(blocks[0].rows.end + 1));
        // Where the load put it: a reply since then in a thread below it, or in the last one, leaves it there (that
        // block shows its own "new" line).
        let (lines, _, blocks) = layout_at(&ws, &with(&[true, false, true, false]), 1);
        assert_eq!(divider(&lines), Some(blocks[0].rows.end + 1));
        let (lines, _, blocks) = layout_at(&ws, &with(&[true, false, false, true]), 1);
        assert_eq!(divider(&lines), Some(blocks[0].rows.end + 1), "a reply in the last thread");
        // None at load: none after a reply either.
        assert_eq!(divider(&layout_at(&ws, &with(&[false, false, true]), 0).0), None);
    }

    /// [`layout`] with the load's count of threads with new replies on top.
    fn layout_at(ws: &super::super::state::Workspace, entries: &[Entry], unread_first: usize) -> (Vec<Line<'static>>, Vec<Option<(String, String)>>, Vec<Block>) {
        let feed = build(&Here::alone(ws), entries, 60, 3, unread_first, &|_, _| None, Avatars::Off);
        (feed.lines, feed.owners, feed.blocks)
    }

    #[test]
    fn the_new_line_sits_before_the_first_unread_reply() {
        let ws = ws();
        // Five replies, the last two unread; three shown.
        let (lines, owners, blocks) = layout(&Here::alone(&ws), &[entry(5, 2)], 60, 3);
        let text: Vec<String> = lines.iter().map(plain).collect();
        let first_unread = owners.iter().position(|o| o.as_ref().is_some_and(|(_, ts)| ts == "1729454400.000100")).expect("reply 4 is shown");
        assert!(text[first_unread - 1].contains(" new ") && text[first_unread - 1].contains('─'), "{text:#?}");
        assert_eq!(owners[first_unread - 1], None);
        assert!(text.iter().any(|l| l.contains("reply 3")) && text.iter().any(|l| l.contains("2 more replies")), "three shown: {text:#?}");
        assert_eq!(blocks[0].unread_rows, first_unread - 1..lines.len() - 1, "from the rule to the last unread reply");
        assert_eq!(text.iter().filter(|l| l.contains(" new ") && l.contains('─')).count(), 1, "one rule");
        assert!(text[0].contains(" 2 new "), "the header counts them: {:?}", text[0]);
        // More unread replies than are shown: all of them are.
        let (lines, ..) = layout(&Here::alone(&ws), &[entry(5, 4)], 60, 2);
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert!((2..=5).all(|n| text.iter().any(|l| l.contains(&format!("reply {n}")))) && text.iter().any(|l| l.contains("1 more reply")), "{text:#?}");
        // Nothing unread: no rule.
        let (lines, ..) = layout(&Here::alone(&ws), &[entry(5, 0)], 60, 3);
        assert!(!lines.iter().map(plain).any(|l| l.contains(" new ")), "no rule, no count");
    }

    #[test]
    fn a_long_parent_is_cut() {
        let ws = ws();
        let mut e = entry(1, 0);
        e.root.text = (1..=30).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        let (lines, owners, _) = layout(&Here::alone(&ws), &[e], 60, 3);
        let text: Vec<String> = lines.iter().map(plain).collect();
        let root: Vec<usize> = (0..owners.len()).filter(|&n| owners[n].as_ref().is_some_and(|(_, ts)| ts == "1729440000.000100")).collect();
        assert_eq!(root.len(), super::ROOT_LINES + 1, "{text:#?}");
        assert!(text[*root.last().unwrap()].contains("… more"), "the parent's last row says there is more: {text:#?}");
        assert!(!text.iter().any(|l| l.contains("line 30")));
        assert!(text.iter().any(|l| l.contains("reply 1")), "the reply follows");
    }

    #[test]
    fn missing_message_says_so() {
        let ws = ws();
        let mut e = entry(2, 1);
        e.root = Message { ts: "1729440000.000100".into(), reply_count: 2, ..Default::default() };
        let (lines, owners, blocks) = layout(&Here::alone(&ws), &[e], 60, 3);
        let text: Vec<String> = lines.iter().map(plain).collect();
        let root: Vec<usize> = (0..owners.len()).filter(|&n| owners[n].as_ref().is_some_and(|(_, ts)| ts == "1729440000.000100")).collect();
        assert_eq!(root.len(), 1, "{text:#?}");
        assert!(text[root[0]].contains("message not available"), "{text:#?}");
        assert_eq!(lines[root[0]].spans.last().and_then(|s| s.style.fg), Some(super::super::theme::p().dim), "dim");
        assert!(text.iter().any(|l| l.contains("reply 2")), "the replies are still there");
        assert!(!blocks[0].unread_rows.is_empty());
    }

    fn block(rows: Range<usize>, unread_rows: Range<usize>) -> Block {
        Block { channel: "C1".into(), thread_ts: "1.0".into(), rows, unread_rows }
    }

    #[test]
    fn read_after_two_seconds_on_screen() {
        let two = Duration::from_secs(2);
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let blocks = [block(5..15, 10..15), block(16..20, 0..0)];
        let it = vec![("C1".to_string(), "1.0".to_string())];
        // Fully on screen from t0.
        let mut clock = ReadClock::default();
        assert!(clock.tick(0..20, &blocks, at(0), two).is_empty());
        assert!(clock.tick(0..20, &blocks, at(1900), two).is_empty());
        assert_eq!(clock.tick(0..20, &blocks, at(2000), two), it);
        assert!(clock.tick(0..20, &blocks, at(3000), two).is_empty(), "once");
        // Scrolled away at 1 s and back: the clock starts again.
        let mut clock = ReadClock::default();
        assert!(clock.tick(0..20, &blocks, at(0), two).is_empty());
        assert!(clock.tick(12..32, &blocks, at(1000), two).is_empty(), "part of it is above the view");
        assert!(clock.tick(0..20, &blocks, at(1500), two).is_empty());
        assert!(clock.tick(0..20, &blocks, at(3000), two).is_empty(), "only 1.5 s since it came back");
        assert_eq!(clock.tick(0..20, &blocks, at(3500), two), it);
        // Taller than the view: its unread replies on screen are enough.
        let tall = [block(0..30, 20..28)];
        let mut clock = ReadClock::default();
        assert!(clock.tick(0..10, &tall, at(0), two).is_empty());
        assert!(clock.tick(0..10, &tall, at(2500), two).is_empty(), "its unread replies are not on screen");
        assert!(clock.tick(15..25, &tall, at(3000), two).is_empty(), "only some of them");
        assert!(clock.tick(18..28, &tall, at(3100), two).is_empty());
        assert_eq!(clock.tick(18..28, &tall, at(5100), two), [("C1".to_string(), "1.0".to_string())]);
        // Unread replies taller than the view: once their end has been on screen.
        let huge = [block(0..60, 5..50)];
        let mut clock = ReadClock::default();
        assert!(clock.tick(10..20, &huge, at(0), two).is_empty());
        assert!(clock.tick(10..20, &huge, at(2500), two).is_empty());
        assert!(clock.tick(40..50, &huge, at(2600), two).is_empty());
        assert_eq!(clock.tick(40..50, &huge, at(4600), two).len(), 1);
        // 0 s: only when opened.
        let mut clock = ReadClock::default();
        assert!(clock.tick(0..20, &blocks, at(0), Duration::ZERO).is_empty());
        assert!(clock.tick(0..20, &blocks, at(60_000), Duration::ZERO).is_empty());
        // Not shown at all: nothing.
        let mut clock = ReadClock::default();
        assert!(clock.tick(0..0, &blocks, at(0), two).is_empty() && clock.tick(0..0, &blocks, at(5000), two).is_empty());
    }

    #[test]
    fn a_block_that_fits_counts_only_when_whole() {
        let two = Duration::from_secs(2);
        let t0 = Instant::now();
        // Its unread rows are on screen, its header and parent are not: it fits, so that is not enough.
        let blocks = [block(5..15, 10..15)];
        let mut clock = ReadClock::default();
        for s in [0, 1, 2, 3, 5, 10] {
            assert!(clock.tick(10..30, &blocks, t0 + Duration::from_secs(s), two).is_empty(), "at {s} s");
        }
        assert_eq!(clock.tick(0..20, &blocks, t0 + Duration::from_secs(11), two), [], "whole from now on");
        assert_eq!(clock.tick(0..20, &blocks, t0 + Duration::from_secs(13), two).len(), 1);
    }

    #[test]
    fn a_thread_marked_read_can_be_marked_again_after_new_replies() {
        let two = Duration::from_secs(2);
        let t0 = Instant::now();
        let blocks = [block(0..10, 5..10)];
        let mut clock = ReadClock::default();
        clock.tick(0..20, &blocks, t0, two);
        assert_eq!(clock.tick(0..20, &blocks, t0 + two, two).len(), 1);
        clock.forget("C1", "1.0");
        assert!(clock.tick(0..20, &blocks, t0 + two, two).is_empty(), "the clock starts again");
        assert_eq!(clock.tick(0..20, &blocks, t0 + 2 * two, two).len(), 1);
    }
}
