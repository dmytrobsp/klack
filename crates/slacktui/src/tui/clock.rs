//! Times as the user wants to see them: in the time zone chosen in the preferences, or the
//! system's.

use std::sync::RwLock;

use chrono::{DateTime, FixedOffset, Local, TimeZone, Utc};

static ZONE: RwLock<Option<chrono_tz::Tz>> = RwLock::new(None);

/// Use the IANA zone `name` (e.g. `Europe/Kyiv`); empty means the system's.
pub fn set(name: &str) -> Result<(), String> {
    let zone = if name.trim().is_empty() { None } else { Some(name.trim().parse::<chrono_tz::Tz>().map_err(|_| format!("unknown time zone '{}'", name.trim()))?) };
    *ZONE.write().unwrap_or_else(|e| e.into_inner()) = zone;
    Ok(())
}

/// Epoch seconds `t` in the chosen zone.
pub fn at(t: i64) -> Option<DateTime<FixedOffset>> {
    in_zone(*ZONE.read().unwrap_or_else(|e| e.into_inner()), t)
}

fn in_zone(zone: Option<chrono_tz::Tz>, t: i64) -> Option<DateTime<FixedOffset>> {
    let utc = Utc.timestamp_opt(t, 0).single()?;
    Some(match zone {
        Some(zone) => utc.with_timezone(&zone).fixed_offset(),
        None => utc.with_timezone(&Local).fixed_offset(),
    })
}

pub fn now() -> DateTime<FixedOffset> {
    at(Utc::now().timestamp()).unwrap_or_else(|| Utc::now().fixed_offset())
}

#[cfg(test)]
mod tests {
    #[test]
    fn zones() {
        assert!(super::set("Mars/Olympus").is_err(), "an unknown zone is refused (and changes nothing)");
        let at = |zone: &str, t| super::in_zone(Some(zone.parse().unwrap()), t).unwrap().format("%H:%M").to_string();
        assert_eq!(at("Asia/Tokyo", 1_792_324_800), "21:00");
        assert_eq!(at("UTC", 1_792_324_800), "12:00");
    }
}
