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

/// Somewhere the main pane can be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A conversation, with the thread (its parent's ts) open in it; `member`: you were in it (not a preview).
    Conversation { channel: String, thread: Option<String>, member: bool },
    Threads,
    /// The Activity view on its tab.
    Activity(Tab),
    Huddles,
}

impl Place {
    /// The same place, whatever changed while you were there (the Activity view's tab, a previewed channel joined).
    fn same(&self, other: &Place) -> bool {
        match (self, other) {
            (Place::Conversation { channel: a, thread: s, .. }, Place::Conversation { channel: b, thread: t, .. }) => a == b && s == t,
            (Place::Activity(_), Place::Activity(_)) => true,
            _ => self == other,
        }
    }
}

#[derive(Debug, Default)]
pub struct History {
    /// The places left, the last one last.
    back: Vec<Place>,
    /// The places gone back from, the next one last.
    forward: Vec<Place>,
    /// Where the main pane is.
    here: Option<Place>,
    /// A step back or forward was taken: where it lands is not a new place.
    stepped: bool,
}

impl History {
    /// The main pane is at `place` now (None: nowhere yet). `went`: you went there (a key, a click, a link, a
    /// notification), so the place left is kept and what was ahead is dropped; otherwise (a page that came in,
    /// a conversation gone) it only stands in for the place it was.
    pub fn arrive(&mut self, place: Option<Place>, went: bool) {
        let stepped = std::mem::take(&mut self.stepped);
        let Some(place) = place else { return };
        if self.here.as_ref().is_some_and(|h| h.same(&place)) {
            self.here = Some(place);
            return;
        }
        if went && !stepped {
            if let Some(left) = self.here.take() {
                keep(&mut self.back, left);
                self.forward.clear();
            }
        }
        self.here = Some(place);
    }

    /// The place to go back to, those `gone` skipped (and forgotten); where the pane is goes ahead. None: nowhere.
    pub fn back(&mut self, gone: impl Fn(&Place) -> bool) -> Option<Place> {
        step(&mut self.back, &mut self.forward, &mut self.here, &mut self.stepped, gone)
    }

    /// The place to go forward to, as [`History::back`].
    pub fn forward(&mut self, gone: impl Fn(&Place) -> bool) -> Option<Place> {
        step(&mut self.forward, &mut self.back, &mut self.here, &mut self.stepped, gone)
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
fn step(from: &mut Vec<Place>, to: &mut Vec<Place>, here: &mut Option<Place>, stepped: &mut bool, gone: impl Fn(&Place) -> bool) -> Option<Place> {
    while let Some(place) = from.pop() {
        if gone(&place) || here.as_ref().is_some_and(|h| h.same(&place)) {
            continue;
        }
        if let Some(left) = here.replace(place.clone()) {
            keep(to, left);
        }
        *stepped = true;
        return Some(place);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(id: &str) -> Place {
        Place::Conversation { channel: id.into(), thread: None, member: true }
    }

    /// Went to each place in turn.
    fn went(places: &[Place]) -> History {
        let mut h = History::default();
        for p in places {
            h.arrive(Some(p.clone()), true);
        }
        h
    }

    #[test]
    fn back_and_forward_as_a_browser() {
        let mut h = went(&[conv("C1"), Place::Activity(Tab::Dms), conv("D1")]);
        assert_eq!(h.back(|_| false), Some(Place::Activity(Tab::Dms)));
        h.arrive(Some(Place::Activity(Tab::Dms)), true);
        assert_eq!(h.back(|_| false), Some(conv("C1")));
        h.arrive(Some(conv("C1")), true);
        assert_eq!(h.back(|_| false), None, "nothing before the first");
        assert_eq!(h.forward(|_| false), Some(Place::Activity(Tab::Dms)));
        h.arrive(Some(Place::Activity(Tab::Dms)), true);
        assert_eq!(h.forward(|_| false), Some(conv("D1")));
        h.arrive(Some(conv("D1")), true);
        assert_eq!(h.forward(|_| false), None);
    }

    #[test]
    fn going_somewhere_new_drops_what_was_ahead() {
        let mut h = went(&[conv("C1"), conv("C2"), conv("C3")]);
        assert_eq!(h.back(|_| false), Some(conv("C2")));
        h.arrive(Some(conv("C2")), true);
        h.arrive(Some(conv("C4")), true);
        assert_eq!(h.forward(|_| false), None, "C3 is gone from ahead");
        assert_eq!(h.back(|_| false), Some(conv("C2")));
    }

    #[test]
    fn at_most_twenty_back() {
        let ids: Vec<Place> = (0..30).map(|n| conv(&format!("C{n}"))).collect();
        let mut h = went(&ids);
        let mut seen = Vec::new();
        while let Some(p) = h.back(|_| false) {
            h.arrive(Some(p.clone()), true);
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
        h.arrive(Some(conv("C1")), true);
        assert_eq!(h.forward(gone), Some(conv("C3")), "C3 went ahead; C2 is forgotten");
        let mut h = went(&[conv("C1"), conv("C2")]);
        assert_eq!(h.back(|_| true), None, "all gone: nowhere");
    }

    #[test]
    fn what_comes_in_by_itself_is_not_a_step() {
        // A page that came in opens the thread a link led to: the same step.
        let mut h = went(&[Place::Activity(Tab::All), conv("C1")]);
        h.arrive(Some(Place::Conversation { channel: "C1".into(), thread: Some("1.0".into()), member: true }), false);
        assert_eq!(h.back(|_| false), Some(Place::Activity(Tab::All)));
        // Where a step back lands, and what comes in after it, is not a new place either.
        let mut h = went(&[conv("C1"), Place::Conversation { channel: "C2".into(), thread: Some("2.0".into()), member: true }, conv("C3")]);
        assert!(h.back(|_| false).is_some());
        h.arrive(Some(conv("C2")), true);
        h.arrive(Some(Place::Conversation { channel: "C2".into(), thread: Some("2.0".into()), member: true }), false);
        assert_eq!(h.forward(|_| false), Some(conv("C3")), "still ahead");
        assert_eq!(h.back(|_| false), Some(Place::Conversation { channel: "C2".into(), thread: Some("2.0".into()), member: true }));
    }

    #[test]
    fn a_tab_is_the_same_place() {
        let mut h = went(&[conv("C1"), Place::Activity(Tab::All), Place::Activity(Tab::Dms), conv("D1")]);
        assert_eq!(h.back(|_| false), Some(Place::Activity(Tab::Dms)), "on the tab it was left on");
        h.arrive(Some(Place::Activity(Tab::Dms)), true);
        assert_eq!(h.back(|_| false), Some(conv("C1")));
    }
}
