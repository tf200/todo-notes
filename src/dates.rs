//! Due dates. A task is due on a day: the server keeps it as that day's midnight UTC
//! (`2026-11-14T00:00:00Z`), and only the date part means anything. Days are the user's local
//! days; labels read the way people say them ("Today", "Friday", "Nov 14").

use chrono::{Datelike, Days, Months, NaiveDate, Weekday};

/// The user's local date.
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// The day a task's `due_at` names.
pub fn parse(due_at: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(due_at.get(..10)?, "%Y-%m-%d").ok()
}

/// `due_at` for a day.
pub fn stamp(day: NaiveDate) -> String {
    format!("{}T00:00:00Z", day.format("%Y-%m-%d"))
}

pub fn add_days(day: NaiveDate, n: u64) -> NaiveDate {
    day.checked_add_days(Days::new(n)).unwrap_or(day)
}

/// The Monday after `day` (a week on, when `day` is a Monday).
pub fn next_week(day: NaiveDate) -> NaiveDate {
    add_days(day, 7 - u64::from(day.weekday().num_days_from_monday()))
}

/// How a row names a due day, seen from `today`.
pub fn label(day: NaiveDate, today: NaiveDate) -> String {
    match (day - today).num_days() {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        2..=6 => day.format("%A").to_string(),
        _ if day.year() == today.year() => day.format("%b %-d").to_string(),
        _ => day.format("%b %-d, %Y").to_string(),
    }
}

/// The longer form, for the details pane: "Friday, Nov 14".
pub fn long_label(day: NaiveDate, today: NaiveDate) -> String {
    let date = if day.year() == today.year() { day.format("%A, %b %-d") } else { day.format("%A, %b %-d, %Y") };
    match (day - today).num_days() {
        0 => format!("Today, {}", day.format("%b %-d")),
        1 => format!("Tomorrow, {}", day.format("%b %-d")),
        _ => date.to_string(),
    }
}

/// The first of `day`'s month.
pub fn month_of(day: NaiveDate) -> NaiveDate {
    day.with_day(1).unwrap_or(day)
}

/// The first of the month `n` months from `month` (back, when negative).
pub fn shift_month(month: NaiveDate, n: i32) -> NaiveDate {
    let shifted = if n >= 0 {
        month.checked_add_months(Months::new(n as u32))
    } else {
        month.checked_sub_months(Months::new(n.unsigned_abs()))
    };
    month_of(shifted.unwrap_or(month))
}

/// The month around `month` as weeks from Monday; days of other months are `None`.
pub fn weeks(month: NaiveDate) -> Vec<[Option<NaiveDate>; 7]> {
    let first = month_of(month);
    let lead = first.weekday().num_days_from_monday() as usize;
    let mut weeks = Vec::new();
    let mut week = [None; 7];
    let mut slot = lead;
    let mut day = first;
    while day.month() == first.month() {
        week[slot] = Some(day);
        slot += 1;
        if slot == 7 {
            weeks.push(week);
            week = [None; 7];
            slot = 0;
        }
        match day.succ_opt() {
            Some(d) => day = d,
            None => break,
        }
    }
    if slot > 0 {
        weeks.push(week);
    }
    weeks
}

pub const WEEKDAYS: [Weekday; 7] =
    [Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri, Weekday::Sat, Weekday::Sun];

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn a_due_stamp_round_trips_and_ignores_the_time() {
        assert_eq!(stamp(d(2026, 11, 14)), "2026-11-14T00:00:00Z");
        assert_eq!(parse("2026-11-14T00:00:00Z"), Some(d(2026, 11, 14)));
        assert_eq!(parse("2026-11-14T00:00:00.000Z"), Some(d(2026, 11, 14)));
        assert_eq!(parse("soon"), None);
    }

    #[test]
    fn labels_read_like_speech() {
        let today = d(2026, 11, 9); // a Monday
        assert_eq!(label(today, today), "Today");
        assert_eq!(label(d(2026, 11, 10), today), "Tomorrow");
        assert_eq!(label(d(2026, 11, 8), today), "Yesterday");
        assert_eq!(label(d(2026, 11, 13), today), "Friday");
        assert_eq!(label(d(2026, 11, 16), today), "Nov 16");
        assert_eq!(label(d(2026, 11, 2), today), "Nov 2");
        assert_eq!(label(d(2027, 1, 4), today), "Jan 4, 2027");
        assert_eq!(long_label(d(2026, 11, 13), today), "Friday, Nov 13");
        assert_eq!(long_label(today, today), "Today, Nov 9");
    }

    #[test]
    fn next_week_is_the_following_monday() {
        assert_eq!(next_week(d(2026, 11, 9)), d(2026, 11, 16));
        assert_eq!(next_week(d(2026, 11, 15)), d(2026, 11, 16));
        assert_eq!(next_week(d(2026, 11, 12)), d(2026, 11, 16));
    }

    #[test]
    fn a_month_lays_out_from_monday() {
        // November 2026 starts on a Sunday and has 30 days.
        let w = weeks(d(2026, 11, 20));
        assert_eq!(w.len(), 6);
        assert_eq!(w[0][..6], [None; 6]);
        assert_eq!(w[0][6], Some(d(2026, 11, 1)));
        assert_eq!(w[5][0], Some(d(2026, 11, 30)));
        assert_eq!(w[5][1], None);
        assert_eq!(shift_month(d(2026, 11, 1), 2), d(2027, 1, 1));
        assert_eq!(shift_month(d(2026, 1, 1), -1), d(2025, 12, 1));
    }
}
