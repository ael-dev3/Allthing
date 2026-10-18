// SPDX-License-Identifier: AGPL-3.0-or-later
//! Dates without a date library: RFC 3339 parsing and UTC formatting, all in
//! Unix seconds.

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's algorithm).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let (month, day) = (i64::from(month), i64::from(day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month, day) for days since 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn digits(bytes: &[u8], range: std::ops::Range<usize>) -> Option<u32> {
    let slice = bytes.get(range)?;
    let mut value = 0u32;
    for &b in slice {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(b - b'0');
    }
    Some(value)
}

/// Parse "2026-10-17T11:14:41.000Z", "2026-10-17 11:14:41+02:00" and similar
/// RFC 3339 timestamps to Unix seconds. A missing offset is read as UTC.
pub fn parse_rfc3339(value: &str) -> Option<i64> {
    let b = value.trim().as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year = digits(b, 0..4)?;
    let month = digits(b, 5..7)?;
    let day = digits(b, 8..10)?;
    let hour = digits(b, 11..13)?;
    let minute = digits(b, 14..16)?;
    let second = digits(b, 17..19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(i64::from(year), month, day);
    if civil_from_days(days) != (i64::from(year), month, day) {
        return None;
    }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    let offset = match b.get(i) {
        None => 0,
        Some(b'Z' | b'z') if i + 1 == b.len() => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let hours = digits(b, i + 1..i + 3)?;
            let (minutes, end) = match b.get(i + 3) {
                Some(b':') => (digits(b, i + 4..i + 6)?, i + 6),
                _ => (digits(b, i + 3..i + 5)?, i + 5),
            };
            if hours > 23 || minutes > 59 || end != b.len() {
                return None;
            }
            let seconds = i64::from(hours * 3600 + minutes * 60);
            if sign == b'+' { seconds } else { -seconds }
        }
        Some(_) => return None,
    };
    Some(days * DAY + i64::from(hour * 3600 + minute * 60 + second) - offset)
}

/// Compact relative time: "just now", "5m ago", "3h ago", "2d ago", "4mo ago", "1y ago".
pub fn time_ago(ts: i64, now: i64) -> String {
    let diff = (now - ts).max(0);
    if diff < MINUTE {
        "just now".into()
    } else if diff < HOUR {
        format!("{}m ago", diff / MINUTE)
    } else if diff < DAY {
        format!("{}h ago", diff / HOUR)
    } else if diff < 30 * DAY {
        format!("{}d ago", diff / DAY)
    } else if diff < 365 * DAY {
        format!("{}mo ago", diff / (30 * DAY))
    } else {
        format!("{}y ago", diff / (365 * DAY))
    }
}

/// Long form for ages: "3 years", "5 months", "12 days".
pub fn duration_since(ts: i64, now: i64) -> String {
    let days = ((now - ts) / DAY).max(0);
    let plural = |n: i64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    if days >= 365 {
        plural(days / 365, "year")
    } else if days >= 30 {
        plural(days / 30, "month")
    } else {
        plural(days, "day")
    }
}

/// "17 Oct 2026" (UTC).
pub fn format_date(ts: i64) -> String {
    let (year, month, day) = civil_from_days(ts.div_euclid(DAY));
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

/// "Oct 17, 2026, 11:14 UTC", for tooltips.
pub fn format_date_time(ts: i64) -> String {
    let (year, month, day) = civil_from_days(ts.div_euclid(DAY));
    let secs = ts.rem_euclid(DAY);
    format!("{} {day}, {year}, {:02}:{:02} UTC", MONTHS[(month - 1) as usize], secs / HOUR, (secs % HOUR) / MINUTE)
}

/// "2026-10-17T11:14:41Z", for `<time datetime>`.
pub fn iso_utc(ts: i64) -> String {
    let (year, month, day) = civil_from_days(ts.div_euclid(DAY));
    let secs = ts.rem_euclid(DAY);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", secs / HOUR, (secs % HOUR) / MINUTE, secs % MINUTE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_forms() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-17T11:14:41.000Z"), Some(1_792_235_681));
        assert_eq!(parse_rfc3339("2026-10-17 13:14:41+02:00"), Some(1_792_235_681));
        assert_eq!(parse_rfc3339("2026-10-17T06:14:41-0500"), Some(1_792_235_681));
        assert_eq!(parse_rfc3339("2026-10-17T11:14:41"), Some(1_792_235_681));
        assert_eq!(parse_rfc3339("2000-02-29T00:00:00Z"), Some(951_782_400));
        assert_eq!(parse_rfc3339("not a date"), None);
        assert_eq!(parse_rfc3339("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("2026-10-17T11:14:41.Z"), None);
    }

    #[test]
    fn rejects_invalid_dates_offsets_and_trailing_data() {
        for value in [
            "2026-02-29T00:00:00Z",
            "1900-02-29T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-01-00T00:00:00Z",
            "2026-01-01T00:00:00+24:00",
            "2026-01-01T00:00:00-00:60",
            "2026-01-01T00:00:00+2400",
            "2026-01-01T00:00:00+0060",
            "2026-01-01T00:00:00Zextra",
            "2026-01-01T00:00:00+01:00extra",
            "2026-01-01T00:00:00+0100extra",
        ] {
            assert_eq!(parse_rfc3339(value), None, "accepted {value}");
        }
        assert!(parse_rfc3339("2000-02-29T00:00:00Z").is_some());
        assert!(parse_rfc3339("2024-02-29T00:00:00z").is_some());
        assert_eq!(parse_rfc3339("1970-01-01T01:30:00+01:30"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-01T01:30:00+0130"), Some(0));
    }

    #[test]
    fn civil_round_trip() {
        for days in [-719_468, -1, 0, 1, 10_957, 20_743, 100_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }

    #[test]
    fn relative_times() {
        let now = 1_792_235_681;
        assert_eq!(time_ago(now - 5, now), "just now");
        assert_eq!(time_ago(now - 5 * MINUTE, now), "5m ago");
        assert_eq!(time_ago(now - 3 * HOUR, now), "3h ago");
        assert_eq!(time_ago(now - 2 * DAY, now), "2d ago");
        assert_eq!(time_ago(now - 120 * DAY, now), "4mo ago");
        assert_eq!(time_ago(now - 400 * DAY, now), "1y ago");
        assert_eq!(time_ago(now + 50, now), "just now");
        assert_eq!(duration_since(now - 3 * 365 * DAY, now), "3 years");
        assert_eq!(duration_since(now - 31 * DAY, now), "1 month");
        assert_eq!(duration_since(now - DAY, now), "1 day");
    }

    #[test]
    fn formats_dates_in_utc() {
        let ts = 1_792_235_681;
        assert_eq!(format_date(ts), "17 Oct 2026");
        assert_eq!(format_date_time(ts), "Oct 17, 2026, 11:14 UTC");
        assert_eq!(iso_utc(ts), "2026-10-17T11:14:41Z");
        assert_eq!(format_date(-1), "31 Dec 1969");
    }
}
