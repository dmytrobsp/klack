//! Back and forward through the places you went in a workspace, as a browser does: going somewhere
//! keeps the place left (at most [`CAP`] back), going somewhere new after going back drops what was ahead.

use super::activity::Tab;

/// How many places back are kept.
pub const CAP: usize = 20;

/// The note when there is nowhere to go back to.
pub const NOTHING_BACK: &str = "nowhere to go back to";

/// The note when there is nowhere to go forward to.
pub const NOTHING_FORWARD: &str = "nowhere to go forward to";

/// The words shown through `tr`.
pub const WORDS: &[&str] = &[NOTHING_BACK, NOTHING_FORWARD];

/// Somewhere the main pane can be, with where you were in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A conversation, with the thread (its parent's ts) open in it; `member`: you were in it (not a preview); `at`:
    /// the message selected in it, `reply`: the reply selected in its thread.
    Conversation { channel: String, thread: Option<String>, member: bool, at: Option<String>, reply: Option<String> },
    /// The Threads view, at the message (channel, ts, its thread's parent's ts) selected in it.
    Threads(Option<(String, String, String)>),
    /// The Activity view on its tab, at the item (its key) selected in it.
    Activity(Tab, Option<String>),
    Huddles,
}

impl Place {
    /// The same place, whatever changed while you were there (the selection, the Activity view's tab, a previewed
    /// channel joined).
    fn same(&self, other: &Place) -> bool {
        match (self, other) {
            (Place::Conversation { channel: a, thread: s, .. }, Place::Conversation { channel: b, thread: t, .. }) => a == b && s == t,
            (Place::Threads(_), Place::Threads(_)) | (Place::Activity(..), Place::Activity(..)) => true,
            _ => self == other,
        }
    }
}

/// How the main pane came to be where it is after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Went {
    /// Not by your doing: a reply that came in, a tick, the pointer moving.
    No,
    /// You went there: a key, a click, a link, a notification.
    Yes,
    /// A move through the sidebar (its keys, the wheel over it, the next / previous conversation): a run of them
    /// is one step.
    Browse,
}

#[derive(Debug, Default)]
pub struct History {
    /// The places left, the last one last.
    back: Vec<Place>,
    /// The places gone back from, the next one last.
    forward: Vec<Place>,
    /// Where the main pane is (while landing: where a step goes).
    here: Option<Place>,
    /// A step back or forward was taken: where it lands is not a new place.
    stepped: bool,
    /// A step to a thread has shown its conversation; the thread opens once its history is in.
    landing: bool,
    /// The sidebar moves under way since the last other event: one step.
    browsing: bool,
    /// Taken up again for a workspace switched back to: the first place shown is a step from the one left there.
    resumed: bool,
    /// The event under way has looked at where the pane is (see [`History::note`]).
    noted: bool,
}

impl History {
    /// The main pane is at `place` now (None: nowhere yet), as `went` says: a step keeps the place left and drops
    /// what was ahead; otherwise (a page that came in, a conversation gone) the place only stands in for the one it
    /// was.
    pub fn arrive(&mut self, place: Option<Place>, went: Went) {
        let stepped = std::mem::take(&mut self.stepped);
        self.noted = false;
        let Some(place) = place else { return };
        let went = if std::mem::take(&mut self.resumed) && went == Went::No { Went::Yes } else { went };
        if went == Went::Yes {
            self.browsing = false;
        }
        if self.here.as_ref().is_some_and(|h| h.same(&place)) {
            self.here = Some(place);
            self.landing = false;
            return;
        }
        if (stepped || self.landing) && self.here.as_ref().is_some_and(|h| on_the_way(h, &place)) {
            self.landing = true;
            return;
        }
        self.landing = false;
        match went {
            Went::Yes if !stepped => self.leave(),
            Went::Browse if !stepped && !self.browsing => {
                self.leave();
                self.browsing = true;
            }
            _ => {}
        }
        self.here = Some(place);
    }

    /// Where the pane is as the event under way found it, about to go elsewhere (the first look only): the place
    /// keeps what the event chose there before leaving (a click that selects an item and opens it).
    pub fn note(&mut self, place: Option<Place>) {
        if std::mem::replace(&mut self.noted, true) {
            return;
        }
        if let (Some(here), Some(place)) = (self.here.as_mut(), place) {
            if here.same(&place) {
                *here = place;
            }
        }
    }

    /// The place shown is kept as one left; what was ahead is dropped.
    fn leave(&mut self) {
        if let Some(left) = self.here.take() {
            keep(&mut self.back, left);
            self.forward.clear();
        }
    }

    /// Taken up again for a workspace switched back to (see [`History::resumed`]).
    pub fn resume(&mut self) {
        (self.stepped, self.landing, self.browsing, self.resumed) = (false, false, false, true);
    }

    /// The place to go back to, those `gone` skipped (and forgotten); where the pane is goes ahead. None: nowhere.
    pub fn back(&mut self, gone: impl Fn(&Place) -> bool) -> Option<Place> {
        let place = step(&mut self.back, &mut self.forward, &mut self.here, gone)?;
        (self.stepped, self.landing, self.noted) = (true, false, true);
        Some(place)
    }

    /// The place to go forward to, as [`History::back`].
    pub fn forward(&mut self, gone: impl Fn(&Place) -> bool) -> Option<Place> {
        let place = step(&mut self.forward, &mut self.back, &mut self.here, gone)?;
        (self.stepped, self.landing, self.noted) = (true, false, true);
        Some(place)
    }
}

/// `place` is on the way to `target`: its conversation, the thread not open yet.
fn on_the_way(target: &Place, place: &Place) -> bool {
    match (target, place) {
        (Place::Conversation { channel: a, thread: Some(_), .. }, Place::Conversation { channel: b, thread: None, .. }) => a == b,
        _ => false,
    }
}

/// `place` on top of `list`, the oldest dropped past [`CAP`].
fn keep(list: &mut Vec<Place>, place: Place) {
    list.push(place);
    if list.len() > CAP {
        list.remove(0);
    }
}

/// The next place of `from` that is still there and not where the pane is; `here` goes onto `to`.
fn step(from: &mut Vec<Place>, to: &mut Vec<Place>, here: &mut Option<Place>, gone: impl Fn(&Place) -> bool) -> Option<Place> {
    while let Some(place) = from.pop() {
        if gone(&place) || here.as_ref().is_some_and(|h| h.same(&place)) {
            continue;
        }
        if let Some(left) = here.replace(place.clone()) {
            keep(to, left);
        }
        return Some(place);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(id: &str) -> Place {
        Place::Conversation { channel: id.into(), thread: None, member: true, at: None, reply: None }
    }

    fn thread(id: &str, parent: &str) -> Place {
        Place::Conversation { channel: id.into(), thread: Some(parent.into()), member: true, at: None, reply: None }
    }

    /// Conversation `id` at message `at`.
    fn conv_at(id: &str, at: &str) -> Place {
        Place::Conversation { channel: id.into(), thread: None, member: true, at: Some(at.into()), reply: None }
    }

    /// Went to each place in turn.
    fn went(places: &[Place]) -> History {
        let mut h = History::default();
        for p in places {
            h.arrive(Some(p.clone()), Went::Yes);
        }
        h
    }

    #[test]
    fn back_and_forward_as_a_browser() {
        let mut h = went(&[conv("C1"), Place::Activity(Tab::Dms, None), conv("D1")]);
        assert_eq!(h.back(|_| false), Some(Place::Activity(Tab::Dms, None)));
        h.arrive(Some(Place::Activity(Tab::Dms, None)), Went::Yes);
        assert_eq!(h.back(|_| false), Some(conv("C1")));
        h.arrive(Some(conv("C1")), Went::Yes);
        assert_eq!(h.back(|_| false), None, "nothing before the first");
        assert_eq!(h.forward(|_| false), Some(Place::Activity(Tab::Dms, None)));
        h.arrive(Some(Place::Activity(Tab::Dms, None)), Went::Yes);
        assert_eq!(h.forward(|_| false), Some(conv("D1")));
        h.arrive(Some(conv("D1")), Went::Yes);
        assert_eq!(h.forward(|_| false), None);
    }

    #[test]
    fn going_somewhere_new_drops_what_was_ahead() {
        let mut h = went(&[conv("C1"), conv("C2"), conv("C3")]);
        assert_eq!(h.back(|_| false), Some(conv("C2")));
        h.arrive(Some(conv("C2")), Went::Yes);
        h.arrive(Some(conv("C4")), Went::Yes);
        assert_eq!(h.forward(|_| false), None, "C3 is gone from ahead");
        assert_eq!(h.back(|_| false), Some(conv("C2")));
    }

    #[test]
    fn at_most_twenty_back() {
        let ids: Vec<Place> = (0..30).map(|n| conv(&format!("C{n}"))).collect();
        let mut h = went(&ids);
        let mut seen = Vec::new();
        while let Some(p) = h.back(|_| false) {
            h.arrive(Some(p.clone()), Went::Yes);
            seen.push(p);
        }
        assert_eq!(seen.len(), CAP);
        assert_eq!(seen.first(), Some(&conv("C28")));
        assert_eq!(seen.last(), Some(&conv("C9")), "the oldest ten are forgotten");
    }

    #[test]
    fn a_place_gone_is_skipped() {
        let mut h = went(&[conv("C1"), conv("C2"), conv("C3")]);
        let gone = |p: &Place| *p == conv("C2");
        assert_eq!(h.back(gone), Some(conv("C1")));
        h.arrive(Some(conv("C1")), Went::Yes);
        assert_eq!(h.forward(gone), Some(conv("C3")), "C3 went ahead; C2 is forgotten");
        let mut h = went(&[conv("C1"), conv("C2")]);
        assert_eq!(h.back(|_| true), None, "all gone: nowhere");
    }

    #[test]
    fn what_comes_in_by_itself_is_not_a_step() {
        // A page that came in opens the thread a link led to: the same step.
        let mut h = went(&[Place::Activity(Tab::All, None), conv("C1")]);
        h.arrive(Some(thread("C1", "1.0")), Went::No);
        assert_eq!(h.back(|_| false), Some(Place::Activity(Tab::All, None)));
        // Where a step back lands, and what comes in after it, is not a new place either.
        let mut h = went(&[conv("C1"), thread("C2", "2.0"), conv("C3")]);
        assert!(h.back(|_| false).is_some());
        h.arrive(Some(conv("C2")), Went::Yes);
        h.arrive(Some(thread("C2", "2.0")), Went::No);
        assert_eq!(h.forward(|_| false), Some(conv("C3")), "still ahead");
        assert_eq!(h.back(|_| false), Some(thread("C2", "2.0")));
    }

    #[test]
    fn a_tab_is_the_same_place() {
        let mut h = went(&[conv("C1"), Place::Activity(Tab::All, None), Place::Activity(Tab::Dms, None), conv("D1")]);
        assert_eq!(h.back(|_| false), Some(Place::Activity(Tab::Dms, None)), "on the tab it was left on");
        h.arrive(Some(Place::Activity(Tab::Dms, None)), Went::Yes);
        assert_eq!(h.back(|_| false), Some(conv("C1")));
    }

    #[test]
    fn a_run_of_sidebar_moves_is_one_step() {
        let mut h = went(&[conv("C1"), Place::Activity(Tab::Dms, None), conv("D1")]);
        // A wheel flick over the sidebar: 24 conversations passed.
        for n in 2..26 {
            h.arrive(Some(conv(&format!("C{n}"))), Went::Browse);
            // (A page that comes in meanwhile does not end the run.)
            h.arrive(Some(conv(&format!("C{n}"))), Went::No);
        }
        assert_eq!(h.back(|_| false), Some(conv("D1")), "where you were before browsing");
        h.arrive(Some(conv("D1")), Went::Yes);
        assert_eq!(h.back(|_| false), Some(Place::Activity(Tab::Dms, None)), "still kept");
        h.arrive(Some(Place::Activity(Tab::Dms, None)), Went::Yes);
        assert_eq!(h.forward(|_| false), Some(conv("D1")));
        h.arrive(Some(conv("D1")), Went::Yes);
        assert_eq!(h.forward(|_| false), Some(conv("C25")), "the one you stopped on");
        h.arrive(Some(conv("C25")), Went::Yes);
        // Another event ends a run (reading, a key): the next moves are a step of their own.
        h.arrive(Some(conv("C26")), Went::Browse);
        h.arrive(Some(conv("C26")), Went::Yes);
        h.arrive(Some(conv("C27")), Went::Browse);
        h.arrive(Some(conv("C28")), Went::Browse);
        assert_eq!(h.back(|_| false), Some(conv("C26")));
        h.arrive(Some(conv("C26")), Went::Yes);
        assert_eq!(h.back(|_| false), Some(conv("C25")));
    }

    #[test]
    fn a_second_back_while_a_thread_opens_goes_on_from_it() {
        let thread = thread("C", "1.0");
        let mut h = went(&[conv("C"), thread.clone(), conv("D")]);
        assert_eq!(h.back(|_| false), Some(thread.clone()));
        // Its conversation shows first; back again before the thread is in.
        h.arrive(Some(conv("C")), Went::Yes);
        h.arrive(Some(conv("C")), Went::Yes);
        assert_eq!(h.back(|_| false), Some(conv("C")), "the place before it, not skipped");
        h.arrive(Some(conv("C")), Went::Yes);
        assert_eq!(h.forward(|_| false), Some(thread.clone()), "the thread is ahead, not its conversation");
        h.arrive(Some(conv("C")), Went::Yes);
        h.arrive(Some(thread.clone()), Went::No);
        assert_eq!(h.forward(|_| false), Some(conv("D")));
    }

    #[test]
    fn a_workspace_taken_up_again_steps_from_where_it_was_left() {
        let mut h = went(&[conv("C1"), Place::Activity(Tab::All, None)]);
        h.resume();
        // Before its conversations are in: nowhere; then the one it starts on.
        h.arrive(None, Went::No);
        h.arrive(Some(conv("C2")), Went::No);
        assert_eq!(h.back(|_| false), Some(Place::Activity(Tab::All, None)), "the view it was left on");
        h.arrive(Some(Place::Activity(Tab::All, None)), Went::Yes);
        assert_eq!(h.back(|_| false), Some(conv("C1")));
        // Taken up on the place it was left at: no step.
        let mut h = went(&[conv("C1"), conv("C2")]);
        h.resume();
        h.arrive(Some(conv("C2")), Went::No);
        assert_eq!(h.back(|_| false), Some(conv("C1")));
    }

    #[test]
    fn where_you_were_goes_with_the_place() {
        let mut h = went(&[conv("C1")]);
        // Moving, scrolling, a page coming in: the same place, never a step.
        h.arrive(Some(conv_at("C1", "1.0")), Went::Yes);
        h.arrive(Some(conv_at("C1", "2.0")), Went::No);
        h.arrive(Some(conv_at("C1", "2.0")), Went::Yes);
        h.arrive(Some(Place::Threads(None)), Went::Yes);
        h.arrive(Some(Place::Threads(Some(("C2".into(), "7.0".into(), "7.0".into())))), Went::Yes);
        h.arrive(Some(Place::Activity(Tab::Dms, Some("k1".into()))), Went::Yes);
        assert_eq!(h.back(|_| false), Some(Place::Threads(Some(("C2".into(), "7.0".into(), "7.0".into())))), "at the message selected last");
        h.arrive(Some(Place::Threads(Some(("C2".into(), "7.0".into(), "7.0".into())))), Went::Yes);
        assert_eq!(h.back(|_| false), Some(conv_at("C1", "2.0")));
        h.arrive(Some(conv_at("C1", "2.0")), Went::Yes);
        assert_eq!(h.back(|_| false), None, "one step each");
        h.arrive(Some(conv_at("C1", "3.0")), Went::Yes);
        assert_eq!(h.forward(|_| false), Some(Place::Threads(Some(("C2".into(), "7.0".into(), "7.0".into())))));
        h.arrive(Some(Place::Threads(Some(("C2".into(), "7.0".into(), "7.0".into())))), Went::Yes);
        assert_eq!(h.forward(|_| false), Some(Place::Activity(Tab::Dms, Some("k1".into()))));
        h.arrive(Some(Place::Activity(Tab::Dms, Some("k1".into()))), Went::Yes);
        assert_eq!(h.back(|_| false), Some(Place::Threads(Some(("C2".into(), "7.0".into(), "7.0".into())))));
        h.arrive(Some(Place::Threads(None)), Went::Yes);
        assert_eq!(h.back(|_| false), Some(conv_at("C1", "3.0")), "where you were when you went forward");
    }

    #[test]
    fn the_place_left_keeps_what_the_leaving_event_chose() {
        // A click selects an item and opens it: the place left is at that item.
        let mut h = went(&[Place::Activity(Tab::All, Some("k1".into()))]);
        h.note(Some(Place::Activity(Tab::All, Some("k2".into()))));
        h.note(Some(Place::Activity(Tab::All, Some("k3".into()))));
        h.arrive(Some(conv("C1")), Went::Yes);
        assert_eq!(h.back(|_| false), Some(Place::Activity(Tab::All, Some("k2".into()))), "the first look only");
        // A look at another place (the pane already on its way) changes nothing.
        h.arrive(Some(Place::Activity(Tab::All, Some("k2".into()))), Went::Yes);
        h.note(Some(conv_at("C9", "1.0")));
        h.arrive(Some(conv("C2")), Went::Yes);
        assert_eq!(h.back(|_| false), Some(Place::Activity(Tab::All, Some("k2".into()))));
        // Nor does one by a step back or forward.
        let mut h = went(&[conv_at("C1", "1.0"), conv_at("C2", "2.0")]);
        assert_eq!(h.back(|_| false), Some(conv_at("C1", "1.0")));
        h.note(Some(conv_at("C2", "9.0")));
        h.arrive(Some(conv_at("C1", "1.0")), Went::Yes);
        assert_eq!(h.forward(|_| false), Some(conv_at("C2", "2.0")));
    }

    #[test]
    fn a_run_of_sidebar_moves_keeps_where_you_were_before_it() {
        let mut h = went(&[conv("C0")]);
        h.arrive(Some(Place::Conversation { channel: "C1".into(), thread: Some("1.0".into()), member: true, at: Some("1.0".into()), reply: Some("1.5".into()) }), Went::Yes);
        for n in 2..6 {
            h.arrive(Some(conv(&format!("C{n}"))), Went::Browse);
        }
        assert_eq!(h.back(|_| false), Some(Place::Conversation { channel: "C1".into(), thread: Some("1.0".into()), member: true, at: Some("1.0".into()), reply: Some("1.5".into()) }));
    }
}
