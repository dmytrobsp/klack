//! The notification schedule (Slack's recurring Do Not Disturb hours) as `users.prefs` store it.
//!
//! Slack keeps two forms side by side. "Every day" and "weekdays" use the legacy pair
//! `dnd_start_hour`/`dnd_end_hour` (quiet from start to end); "custom" uses one entry per day,
//! `dnd_enabled_<day>` with `dnd_before_<day>`/`dnd_after_<day>` (notifications come through from
//! before to after). Both are shown here as the hours notifications come through, like Slack's
//! own app. Times are the account's time zone, `H:MM` as Slack writes them ("9:00", "17:30").

use serde_json::{Map, Value, json};

/// Slack's day names, Monday first (the order of `Schedule::per_day`).
pub const DAYS: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];

/// Which days the schedule pauses notifications on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleDays {
    /// No schedule: notifications come through at any time.
    Off,
    EveryDay,
    /// Monday to Friday; Saturday and Sunday have no quiet hours.
    Weekdays,
    /// Each day its own hours (`Schedule::per_day`).
    Custom,
    /// Slack's value is one klack doesn't know: shown, never written.
    Unknown,
}

/// One day of a custom schedule: when notifications come through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DayHours {
    /// From the first time to the second, paused outside (Slack's "partial").
    Between(String, String),
    /// All day: no quiet hours that day (Slack's "disabled").
    AllDay,
    /// Never: paused all day (Slack's "all_day").
    Never,
    /// A state klack doesn't know (Slack's value): shown, never written.
    Unknown(String),
}

/// One change the user makes to the schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleEdit {
    Days(ScheduleDays),
    /// Every day / weekdays: notifications from the first time to the second.
    Range(String, String),
    /// Custom: one day (0 = Monday).
    Day(usize, DayHours),
}

/// The notification schedule. `range` and `per_day` are both kept whatever `days` is, so
/// switching between them loses nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub days: ScheduleDays,
    /// Every day / weekdays: notifications come through from `.0` to `.1`.
    pub range: (String, String),
    /// Custom: Monday … Sunday.
    pub per_day: [DayHours; 7],
    /// Slack's `dnd_weekdays_off_allday`: with it on, Slack doesn't apply the weekday hours
    /// (see the spec's research). Only read: klack never writes it.
    pub weekdays_off_allday: bool,
}

/// `9:05` for "09:05" or "9:05"; None unless a time of day.
pub fn slack_time(s: &str) -> Option<String> {
    let (h, m) = s.trim().split_once(':')?;
    let digits = |t: &str, n: std::ops::RangeInclusive<usize>| n.contains(&t.len()) && t.bytes().all(|b| b.is_ascii_digit());
    let (h, m): (u32, u32) = (digits(h, 1..=2).then(|| h.parse().ok())??, digits(m, 2..=2).then(|| m.parse().ok())??);
    (h < 24 && m < 60).then(|| format!("{h}:{m:02}"))
}

/// Minutes past midnight of a time of day.
fn minutes(s: &str) -> Option<u32> {
    let t = slack_time(s)?;
    let (h, m) = t.split_once(':')?;
    Some(h.parse::<u32>().ok()? * 60 + m.parse::<u32>().ok()?)
}

impl Schedule {
    /// From the `prefs` object of `users.prefs.get`.
    pub fn from_prefs(prefs: &Value) -> Schedule {
        let text = |k: &str, or: &str| prefs[k].as_str().unwrap_or(or).to_string();
        // A value Slack has that klack doesn't know is kept as unknown (missing ones get defaults).
        let days = match (&prefs["dnd_enabled"], prefs["dnd_days"].as_str()) {
            (Value::Bool(false) | Value::Null, _) => ScheduleDays::Off,
            (Value::Bool(true), Some("weekdays")) => ScheduleDays::Weekdays,
            (Value::Bool(true), Some("custom")) => ScheduleDays::Custom,
            (Value::Bool(true), Some("every_day") | None) => ScheduleDays::EveryDay,
            _ => ScheduleDays::Unknown,
        };
        let per_day = std::array::from_fn(|i| match &prefs[format!("dnd_enabled_{}", DAYS[i])] {
            Value::String(s) if s == "disabled" => DayHours::AllDay,
            Value::String(s) if s == "all_day" => DayHours::Never,
            Value::String(s) if s == "partial" => DayHours::Between(text(&format!("dnd_before_{}", DAYS[i]), "8:00"), text(&format!("dnd_after_{}", DAYS[i]), "22:00")),
            Value::Null => DayHours::Between(text(&format!("dnd_before_{}", DAYS[i]), "8:00"), text(&format!("dnd_after_{}", DAYS[i]), "22:00")),
            other => DayHours::Unknown(other.as_str().map_or_else(|| other.to_string(), String::from)),
        });
        let weekdays_off_allday = prefs["dnd_weekdays_off_allday"].as_bool().unwrap_or(false);
        Schedule { days, range: (text("dnd_end_hour", "8:00"), text("dnd_start_hour", "22:00")), per_day, weekdays_off_allday }
    }

    /// This schedule with `edits` made, the rest as it is (times as Slack writes them); an error
    /// for an edit klack can't write: on an unknown schedule or day, or with a bad time.
    pub fn edited(&self, edits: &[ScheduleEdit]) -> Result<Schedule, String> {
        const UNKNOWN: &str = "Slack has a schedule klack doesn't know: change it in Slack";
        let time = |t: &str| slack_time(t).ok_or_else(|| format!("{t:?} is not a time of day (H:MM)"));
        if self.days == ScheduleDays::Unknown {
            return Err(UNKNOWN.into());
        }
        let mut out = self.clone();
        for edit in edits {
            match edit {
                ScheduleEdit::Days(ScheduleDays::Unknown) => return Err("not a kind of schedule".into()),
                ScheduleEdit::Days(days) => out.days = *days,
                ScheduleEdit::Range(from, to) => out.range = (time(from)?, time(to)?),
                ScheduleEdit::Day(i, hours) => {
                    let day = out.per_day.get_mut(*i).ok_or("no such day")?;
                    if let DayHours::Unknown(_) = day {
                        return Err(format!("Slack has hours for {} klack doesn't know: change them in Slack", DAYS[*i]));
                    }
                    *day = match hours {
                        DayHours::Between(from, to) => DayHours::Between(time(from)?, time(to)?),
                        DayHours::Unknown(_) => return Err("not a day's hours".into()),
                        other => other.clone(),
                    };
                }
            }
        }
        Ok(out)
    }

    /// The `prefs` JSON for `users.prefs.set` writing this schedule: only the form `days`
    /// selects. Slack accepts "off" only together with "every day" (it ignores the whole
    /// write otherwise), so off is written that way; a day klack doesn't know is left out. None
    /// if a time isn't a time of day, or for a schedule klack doesn't know.
    pub fn to_prefs(&self) -> Option<Value> {
        let days = match self.days {
            ScheduleDays::Unknown => return None,
            ScheduleDays::Off => return Some(json!({"dnd_enabled": false, "dnd_days": "every_day"})),
            ScheduleDays::EveryDay => "every_day",
            ScheduleDays::Weekdays => "weekdays",
            ScheduleDays::Custom => "custom",
        };
        let mut out = Map::new();
        out.insert("dnd_enabled".into(), true.into());
        out.insert("dnd_days".into(), days.into());
        if self.days != ScheduleDays::Custom {
            out.insert("dnd_start_hour".into(), slack_time(&self.range.1)?.into());
            out.insert("dnd_end_hour".into(), slack_time(&self.range.0)?.into());
            return Some(out.into());
        }
        for (day, hours) in DAYS.iter().zip(&self.per_day) {
            let state = match hours {
                DayHours::Between(from, to) => {
                    out.insert(format!("dnd_before_{day}"), slack_time(from)?.into());
                    out.insert(format!("dnd_after_{day}"), slack_time(to)?.into());
                    "partial"
                }
                DayHours::AllDay => "disabled",
                DayHours::Never => "all_day",
                // Left as Slack has it.
                DayHours::Unknown(_) => continue,
            };
            out.insert(format!("dnd_enabled_{day}"), state.into());
        }
        Some(out.into())
    }

    /// Whether notifications are paused on `weekday` (0 = Monday) at `minute` past midnight. A
    /// quiet period of every day / weekdays belongs to the day it starts on (weekdays: Friday's
    /// runs into Saturday morning, Sunday's evening is free); a custom day is its own.
    pub fn quiet_at(&self, weekday: usize, minute: u32) -> bool {
        // Quiet from `start` to `end` (across midnight when `end` is earlier); `from_start` for the
        // part on the day it starts, `into_next` for the part after midnight.
        let span = |start: u32, end: u32, from_start: bool, into_next: bool| match start.cmp(&end) {
            std::cmp::Ordering::Less => from_start && start <= minute && minute < end,
            std::cmp::Ordering::Greater => (from_start && minute >= start) || (into_next && minute < end),
            std::cmp::Ordering::Equal => false,
        };
        let (Some(start), Some(end)) = (minutes(&self.range.1), minutes(&self.range.0)) else { return false };
        match self.days {
            ScheduleDays::Off | ScheduleDays::Unknown => false,
            ScheduleDays::EveryDay => span(start, end, true, true),
            ScheduleDays::Weekdays => span(start, end, weekday < 5, (weekday + 6) % 7 < 5),
            ScheduleDays::Custom => match &self.per_day[weekday % 7] {
                DayHours::Never => true,
                DayHours::AllDay | DayHours::Unknown(_) => false,
                // Notifications from `from` to `to`; an inverted range read as quiet from `to` to `from`.
                DayHours::Between(from, to) => match (minutes(from), minutes(to)) {
                    (Some(from), Some(to)) if from <= to => minute < from || minute >= to,
                    (Some(from), Some(to)) => to <= minute && minute < from,
                    _ => false,
                },
            },
        }
    }

    /// The minutes past midnight at which `quiet_at` can change.
    pub fn boundaries(&self) -> Vec<u32> {
        let mut out = vec![0];
        let times: Vec<&String> = match self.days {
            ScheduleDays::Off | ScheduleDays::Unknown => Vec::new(),
            ScheduleDays::EveryDay | ScheduleDays::Weekdays => vec![&self.range.0, &self.range.1],
            ScheduleDays::Custom => self.per_day.iter().flat_map(|d| match d {
                DayHours::Between(from, to) => vec![from, to],
                _ => Vec::new(),
            }).collect(),
        };
        for m in times.into_iter().filter_map(|t| minutes(t)) {
            if !out.contains(&m) {
                out.push(m);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `dnd_*` keys of a real `users.prefs.get` (test account, 2026-10-20): weekdays,
    /// quiet 17:00–9:00, and per-day hours 8:00–22:00 kept from an earlier custom schedule.
    fn recorded() -> Value {
        json!({
            "dnd_after_friday": "22:00", "dnd_after_monday": "22:00", "dnd_after_saturday": "22:00", "dnd_after_sunday": "22:00",
            "dnd_after_thursday": "22:00", "dnd_after_tuesday": "22:00", "dnd_after_wednesday": "22:00",
            "dnd_before_friday": "8:00", "dnd_before_monday": "8:00", "dnd_before_saturday": "8:00", "dnd_before_sunday": "8:00",
            "dnd_before_thursday": "8:00", "dnd_before_tuesday": "8:00", "dnd_before_wednesday": "8:00",
            "dnd_custom_new_badge_seen": false, "dnd_days": "weekdays", "dnd_enabled": true,
            "dnd_enabled_friday": "partial", "dnd_enabled_monday": "partial", "dnd_enabled_saturday": "partial", "dnd_enabled_sunday": "partial",
            "dnd_enabled_thursday": "partial", "dnd_enabled_tuesday": "partial", "dnd_enabled_wednesday": "partial",
            "dnd_end_hour": "9:00", "dnd_notification_schedule_new_badge_seen": false, "dnd_start_hour": "17:00",
            "dnd_weekdays_off_allday": false, "mega_dnd_expiration": 0, "priority_dnd_override": false
        })
    }

    fn between(a: &str, b: &str) -> DayHours {
        DayHours::Between(a.into(), b.into())
    }

    #[test]
    fn parses_the_recorded_prefs() {
        let s = Schedule::from_prefs(&recorded());
        assert_eq!(s.days, ScheduleDays::Weekdays);
        assert_eq!(s.range, ("9:00".to_string(), "17:00".to_string()), "receive from the quiet end to the quiet start");
        assert_eq!(s.per_day, std::array::from_fn(|_| between("8:00", "22:00")));
    }

    #[test]
    fn parses_days_and_day_states() {
        let mut p = recorded();
        p["dnd_days"] = "custom".into();
        p["dnd_enabled_saturday"] = "all_day".into();
        p["dnd_enabled_sunday"] = "disabled".into();
        p["dnd_before_monday"] = "10:30".into();
        let s = Schedule::from_prefs(&p);
        assert_eq!(s.days, ScheduleDays::Custom);
        assert_eq!((&s.per_day[0], &s.per_day[5], &s.per_day[6]), (&between("10:30", "22:00"), &DayHours::Never, &DayHours::AllDay));
        p["dnd_days"] = "every_day".into();
        assert_eq!(Schedule::from_prefs(&p).days, ScheduleDays::EveryDay);
        // Slack turns the schedule off as every day + disabled.
        p["dnd_enabled"] = false.into();
        assert_eq!(Schedule::from_prefs(&p).days, ScheduleDays::Off);
        assert_eq!(Schedule::from_prefs(&json!({})).days, ScheduleDays::Off);
    }

    #[test]
    fn writes_only_the_selected_form() {
        let mut s = Schedule::from_prefs(&recorded());
        s.range = ("10:00".into(), "18:00".into());
        assert_eq!(s.to_prefs(), Some(json!({"dnd_enabled": true, "dnd_days": "weekdays", "dnd_start_hour": "18:00", "dnd_end_hour": "10:00"})));
        s.days = ScheduleDays::EveryDay;
        assert_eq!(s.to_prefs().unwrap()["dnd_days"], "every_day");
        s.days = ScheduleDays::Off;
        assert_eq!(s.to_prefs(), Some(json!({"dnd_enabled": false, "dnd_days": "every_day"})));
        s.days = ScheduleDays::Custom;
        s.per_day[0] = between("09:30", "17:00");
        s.per_day[5] = DayHours::Never;
        s.per_day[6] = DayHours::AllDay;
        let p = s.to_prefs().unwrap();
        assert_eq!((&p["dnd_enabled"], &p["dnd_days"]), (&json!(true), &json!("custom")));
        assert_eq!((&p["dnd_enabled_monday"], &p["dnd_before_monday"], &p["dnd_after_monday"]), (&json!("partial"), &json!("9:30"), &json!("17:00")));
        assert_eq!((&p["dnd_before_tuesday"], &p["dnd_after_friday"]), (&json!("8:00"), &json!("22:00")));
        assert_eq!((&p["dnd_enabled_saturday"], &p["dnd_enabled_sunday"]), (&json!("all_day"), &json!("disabled")));
        assert!(p.get("dnd_before_saturday").is_none() && p.get("dnd_start_hour").is_none(), "a day without hours keeps its stored ones: {p}");
        assert_eq!(p.as_object().unwrap().len(), 2 + 7 + 2 * 5);
    }

    #[test]
    fn round_trips_the_recorded_prefs() {
        let s = Schedule::from_prefs(&recorded());
        for days in [ScheduleDays::EveryDay, ScheduleDays::Weekdays, ScheduleDays::Custom] {
            let s = Schedule { days, ..s.clone() };
            let mut p = recorded();
            p.as_object_mut().unwrap().extend(s.to_prefs().unwrap().as_object().unwrap().clone());
            assert_eq!(Schedule::from_prefs(&p), s);
        }
    }

    #[test]
    fn quiet_hours_by_day() {
        let (h, mon, tue, fri, sat, sun) = (|h: u32| h * 60, 0, 1, 4, 5, 6);
        // Weekdays, notifications 9:00–17:00 (as recorded): Monday's period runs into Tuesday.
        let mut s = Schedule::from_prefs(&recorded());
        assert!(s.quiet_at(mon, h(20)) && s.quiet_at(tue, h(3)) && !s.quiet_at(tue, h(12)));
        assert!(s.quiet_at(sat, h(3)), "Friday's period runs into Saturday");
        assert!(!s.quiet_at(sat, h(12)) && !s.quiet_at(sun, h(20)) && !s.quiet_at(mon, h(3)), "the weekend is free");
        s.days = ScheduleDays::EveryDay;
        assert!(s.quiet_at(sun, h(20)) && s.quiet_at(mon, h(3)) && s.quiet_at(fri, 17 * 60) && !s.quiet_at(fri, 9 * 60));
        s.range = ("1:45".into(), "0:30".into());
        assert!(s.quiet_at(tue, 60) && !s.quiet_at(tue, 105) && !s.quiet_at(tue, 29));
        s.days = ScheduleDays::Off;
        assert!(!s.quiet_at(tue, h(3)));
        s.days = ScheduleDays::Custom;
        s.per_day[tue] = DayHours::Never;
        s.per_day[sat] = DayHours::AllDay;
        s.per_day[sun] = between("0:30", "1:30");
        assert!(s.quiet_at(tue, h(12)) && !s.quiet_at(sat, h(3)) && s.quiet_at(mon, h(23)) && s.quiet_at(mon, h(7)) && !s.quiet_at(mon, h(8)));
        assert!(s.quiet_at(sun, 0) && !s.quiet_at(sun, 30) && !s.quiet_at(sun, 89) && s.quiet_at(sun, 90));
        let mut b = s.boundaries();
        b.sort();
        assert_eq!(b, vec![0, 30, 90, 8 * 60, 22 * 60]);
    }

    #[test]
    fn unknown_values_are_shown_and_never_written() {
        let mut p = recorded();
        p["dnd_days"] = "weekends".into();
        let s = Schedule::from_prefs(&p);
        assert_eq!(s.days, ScheduleDays::Unknown, "not taken for every day");
        assert_eq!(s.to_prefs(), None, "never written");
        assert!(s.edited(&[ScheduleEdit::Range("10:00".into(), "18:00".into())]).is_err() && s.edited(&[ScheduleEdit::Days(ScheduleDays::Off)]).is_err());
        assert!(!s.quiet_at(1, 3 * 60));
        p["dnd_days"] = "custom".into();
        p["dnd_enabled"] = "yes".into();
        assert_eq!(Schedule::from_prefs(&p).days, ScheduleDays::Unknown, "dnd_enabled not a bool");
        // A day's state klack doesn't know: kept as Slack has it (its keys are not written).
        p["dnd_enabled"] = true.into();
        p["dnd_enabled_tuesday"] = "sometimes".into();
        let s = Schedule::from_prefs(&p);
        assert_eq!(s.per_day[1], DayHours::Unknown("sometimes".into()));
        let w = s.to_prefs().unwrap();
        assert!(["dnd_enabled_tuesday", "dnd_before_tuesday", "dnd_after_tuesday"].iter().all(|k| w.get(k).is_none()), "{w}");
        assert_eq!(w["dnd_enabled_monday"], "partial");
        assert!(s.edited(&[ScheduleEdit::Day(1, DayHours::AllDay)]).is_err(), "that day is not edited");
        assert!(s.edited(&[ScheduleEdit::Day(0, DayHours::AllDay)]).is_ok(), "the others are");
        assert!(!s.quiet_at(1, 3 * 60));
        // The weekdays "off all day" option is read, never written.
        p["dnd_weekdays_off_allday"] = true.into();
        let s = Schedule::from_prefs(&p);
        assert!(s.weekdays_off_allday && !recorded_schedule().weekdays_off_allday);
        assert!(s.to_prefs().unwrap().get("dnd_weekdays_off_allday").is_none());
    }

    fn recorded_schedule() -> Schedule {
        Schedule::from_prefs(&recorded())
    }

    #[test]
    fn edits_change_only_what_they_name() {
        // Slack's copy as read just before the write: another client changed Tuesday.
        let mut p = recorded();
        p["dnd_before_tuesday"] = "7:30".into();
        let fresh = Schedule::from_prefs(&p);
        let s = fresh.edited(&[ScheduleEdit::Day(0, DayHours::Never)]).unwrap();
        assert_eq!((&s.per_day[0], &s.per_day[1]), (&DayHours::Never, &between("7:30", "22:00")), "Tuesday as the other client left it");
        assert_eq!((s.days, &s.range), (ScheduleDays::Weekdays, &fresh.range));
        let s = fresh.edited(&[ScheduleEdit::Days(ScheduleDays::EveryDay), ScheduleEdit::Range("09:00".into(), "17:30".into())]).unwrap();
        assert_eq!((s.days, s.range.0.as_str(), s.range.1.as_str()), (ScheduleDays::EveryDay, "9:00", "17:30"), "times as Slack writes them");
        assert_eq!(s.per_day, fresh.per_day);
        let off = fresh.edited(&[ScheduleEdit::Days(ScheduleDays::Off)]).unwrap();
        assert_eq!(off.to_prefs(), Some(json!({"dnd_enabled": false, "dnd_days": "every_day"})));
        let day = fresh.edited(&[ScheduleEdit::Day(6, DayHours::Between("9:5".into(), "17:00".into()))]);
        assert!(day.is_err(), "a bad time is refused before anything is sent");
        assert!(fresh.edited(&[ScheduleEdit::Range("8:00".into(), "25:00".into())]).is_err());
        assert!(fresh.edited(&[ScheduleEdit::Days(ScheduleDays::Unknown)]).is_err(), "never set to unknown");
        assert!(fresh.edited(&[ScheduleEdit::Day(7, DayHours::AllDay)]).is_err());
    }

    #[test]
    fn times_are_checked_and_written_as_slack_does() {
        assert_eq!(slack_time("09:05").as_deref(), Some("9:05"));
        assert_eq!(slack_time("0:00").as_deref(), Some("0:00"));
        assert_eq!(slack_time("23:59").as_deref(), Some("23:59"));
        for bad in ["24:00", "9:60", "9", "9:5", "nine:00", "", "9:5:0", "-1:00", "+9:00", "009:00"] {
            assert_eq!(slack_time(bad), None, "{bad}");
        }
        // Slack ignores a whole write with one bad time and still answers ok, so none is sent.
        let mut s = Schedule::from_prefs(&recorded());
        s.range.1 = "25:00".into();
        assert_eq!(s.to_prefs(), None);
        s.days = ScheduleDays::Custom;
        assert!(s.to_prefs().is_some(), "the range isn't written for a custom schedule");
        s.per_day[3] = between("8:00", "99:00");
        assert_eq!(s.to_prefs(), None);
    }
}
