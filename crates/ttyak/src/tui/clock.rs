//! Times as the user wants to see them: in the time zone chosen in the preferences, or the
//! system's.

use std::sync::RwLock;

use chrono::{DateTime, FixedOffset, Local, TimeZone, Utc};

static ZONE: RwLock<Option<chrono_tz::Tz>> = RwLock::new(None);

/// Use the IANA zone `name` (e.g. `Europe/Kyiv`); empty means the system's.
pub fn set(name: &str) -> Result<(), String> {
    let zone = if name.trim().is_empty() { None } else { Some(name.trim().parse::<chrono_tz::Tz>().map_err(|_| format!("unknown time zone '{}'", name.trim()))?) };
    *ZONE.write().unwrap_or_else(|e| e.into_inner()) = zone;
    super::messages::looks_changed();
    Ok(())
}

/// The zone chosen in the preferences (None: the system's).
pub fn zone() -> Option<chrono_tz::Tz> {
    *ZONE.read().unwrap_or_else(|e| e.into_inner())
}

/// Epoch seconds `t` in the chosen zone.
pub fn at(t: i64) -> Option<DateTime<FixedOffset>> {
    in_zone(*ZONE.read().unwrap_or_else(|e| e.into_inner()), t)
}

/// Epoch seconds `t` in `zone` (None: the system's).
pub fn in_zone(zone: Option<chrono_tz::Tz>, t: i64) -> Option<DateTime<FixedOffset>> {
    let utc = Utc.timestamp_opt(t, 0).single()?;
    Some(match zone {
        Some(zone) => utc.with_timezone(&zone).fixed_offset(),
        None => utc.with_timezone(&Local).fixed_offset(),
    })
}

/// Where a daily window that ends at `end` begins, given Slack's "next start" `start` of the same
/// schedule (perhaps a day or more earlier): `start`'s time of day on `end`'s date, or the day
/// before — in local time of `zone` (None: the system's), so a DST change in between does not
/// shift it by an hour.
pub fn window_start_in(zone: Option<chrono_tz::Tz>, start: i64, end: i64) -> Option<i64> {
    let (from, to) = (in_zone(zone, start)?, in_zone(zone, end)?);
    let local = |date: chrono::NaiveDate| local_time(zone, date.and_time(from.time()));
    let same_day = local(to.date_naive())?;
    if same_day < end { Some(same_day) } else { local(to.date_naive().pred_opt()?) }
}

/// `date` at `minute` past midnight in `zone` (None: the system's), as epoch seconds; the earlier
/// one when a DST change repeats it, None when it skips it.
pub fn local_minute(zone: Option<chrono_tz::Tz>, date: chrono::NaiveDate, minute: u32) -> Option<i64> {
    local_time(zone, date.and_hms_opt(minute / 60, minute % 60, 0)?)
}

fn local_time(zone: Option<chrono_tz::Tz>, naive: chrono::NaiveDateTime) -> Option<i64> {
    Some(match zone {
        Some(zone) => zone.from_local_datetime(&naive).earliest()?.timestamp(),
        None => Local.from_local_datetime(&naive).earliest()?.timestamp(),
    })
}

/// When something ends, as `09:00` today or `Mon 09:00` on another day, in the chosen zone.
pub fn until_label(t: i64, now: i64, time_format: &str) -> Option<String> {
    let (d, today) = (at(t)?, at(now)?);
    Some(d.format(&if d.date_naive() == today.date_naive() { time_format.to_string() } else { format!("%a {time_format}") }).to_string())
}

pub fn now() -> DateTime<FixedOffset> {
    at(Utc::now().timestamp()).unwrap_or_else(|| Utc::now().fixed_offset())
}

#[cfg(test)]
mod tests {
    #[test]
    fn until_names_the_day_when_not_today() {
        let now = super::now().timestamp();
        let start = super::at(now).unwrap().date_naive().and_hms_opt(12, 0, 0).unwrap();
        let noon = super::local_time(super::zone(), start).unwrap();
        assert_eq!(super::until_label(noon, noon - 60, "%H:%M").as_deref(), Some("12:00"));
        let tomorrow = super::until_label(noon + 86_400, noon, "%H:%M").unwrap();
        assert!(tomorrow.ends_with(" 12:00") && tomorrow.len() == 9, "{tomorrow}");
        assert_eq!(super::until_label(noon + 86_400, noon, "%-I:%M %p").unwrap().len(), 3 + 1 + 8, "weekday + 12-hour clock");
    }

    #[test]
    fn windows_across_a_dst_change() {
        // Kyiv leaves summer time on 25 October 2026 at 04:00 (UTC+3 → UTC+2). A daily 00:00–09:00
        // schedule seen on the 25th: next start 25 Oct 00:00 (+3), next end 26 Oct 09:00 (+2).
        let kyiv = Some("Europe/Kyiv".parse().unwrap());
        let (start, end) = (1_792_875_600, 1_792_998_000);
        let begins = super::window_start_in(kyiv, start, end).unwrap();
        assert_eq!(begins, 1_792_965_600, "26 Oct 00:00 (+2), not 23:00 on the 25th");
        assert_eq!(super::in_zone(kyiv, begins).unwrap().format("%d %H:%M").to_string(), "26 00:00");
        // An overnight window (22:00–07:00) seen before it: one window, as it is.
        let (s, e) = (1_792_868_400, 1_792_904_400);
        assert_eq!(super::window_start_in(kyiv, s, e), Some(s));
    }

    #[test]
    fn zones() {
        assert!(super::set("Mars/Olympus").is_err(), "an unknown zone is refused (and changes nothing)");
        let at = |zone: &str, t| super::in_zone(Some(zone.parse().unwrap()), t).unwrap().format("%H:%M").to_string();
        assert_eq!(at("Asia/Tokyo", 1_792_324_800), "21:00");
        assert_eq!(at("UTC", 1_792_324_800), "12:00");
    }
}
