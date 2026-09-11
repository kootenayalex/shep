//! Pure calendar arithmetic for docket due dates (`YYYY-MM-DD`). No time
//! zones, no clock: the store supplies "today" and this module only moves
//! dates around. Days-from-civil is Howard Hinnant's proleptic-Gregorian
//! algorithm.

use crate::api::schema::DocketRepeat;

/// A calendar date. Ordering is chronological.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    /// Parse strict `YYYY-MM-DD`; `None` for anything else, including real
    /// looking dates that do not exist (`2026-02-30`).
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let bytes = raw.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return None;
        }
        let year: i32 = raw[0..4].parse().ok()?;
        let month: u32 = raw[5..7].parse().ok()?;
        let day: u32 = raw[8..10].parse().ok()?;
        if !raw[0..4].bytes().all(|b| b.is_ascii_digit())
            || !raw[5..7].bytes().all(|b| b.is_ascii_digit())
            || !raw[8..10].bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Date { year, month, day })
    }

    pub(crate) fn format(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// Days since 1970-01-01.
    pub(crate) fn to_days(self) -> i64 {
        let y = i64::from(self.year) - i64::from(self.month <= 2);
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let m = i64::from(self.month);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(self.day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    pub(crate) fn from_days(days: i64) -> Self {
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        Date {
            year: (y + i64::from(m <= 2)) as i32,
            month: m as u32,
            day: d as u32,
        }
    }

    pub(crate) fn add_days(self, days: i64) -> Self {
        Date::from_days(self.to_days() + days)
    }

    /// Add calendar months, clamping the day to the target month's length
    /// (Jan 31 + 1 month = Feb 28/29).
    pub(crate) fn add_months(self, months: i32) -> Self {
        let zero_based = i64::from(self.year) * 12 + i64::from(self.month) - 1 + i64::from(months);
        let year = zero_based.div_euclid(12) as i32;
        let month = (zero_based.rem_euclid(12) + 1) as u32;
        Date {
            year,
            month,
            day: self.day.min(days_in_month(year, month)),
        }
    }

    pub(crate) fn add_repeat(self, repeat: DocketRepeat) -> Self {
        match repeat {
            DocketRepeat::Daily => self.add_days(1),
            DocketRepeat::Weekly => self.add_days(7),
            DocketRepeat::Fortnightly => self.add_days(14),
            DocketRepeat::Monthly => self.add_months(1),
        }
    }
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Roll `due` forward by `repeat` until it is strictly after `today`, so a
/// recurring item completed late does not come back already overdue. A
/// missing `due` starts from today.
pub(crate) fn next_due(due: Option<Date>, repeat: DocketRepeat, today: Date) -> Date {
    let mut next = due.unwrap_or(today).add_repeat(repeat);
    while next <= today {
        next = next.add_repeat(repeat);
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(raw: &str) -> Date {
        Date::parse(raw).unwrap()
    }

    #[test]
    fn parse_accepts_only_real_iso_dates() {
        assert_eq!(
            Date::parse("2026-09-11"),
            Some(Date {
                year: 2026,
                month: 9,
                day: 11
            })
        );
        assert_eq!(
            Date::parse("2024-02-29").map(Date::format).as_deref(),
            Some("2024-02-29")
        );
        for bad in [
            "2026-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-9-1",
            "26-09-11",
            "2026/09/11",
            "2026-09-11T00:00:00Z",
            "",
            "tomorrow",
            "2026-0a-11",
        ] {
            assert_eq!(Date::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn days_round_trip_across_eras() {
        assert_eq!(d("1970-01-01").to_days(), 0);
        assert_eq!(d("2000-03-01").to_days(), 11_017);
        for raw in [
            "1969-12-31",
            "1900-02-28",
            "2000-02-29",
            "2026-09-11",
            "2100-12-31",
        ] {
            let date = d(raw);
            assert_eq!(Date::from_days(date.to_days()), date, "{raw}");
        }
    }

    #[test]
    fn add_days_crosses_month_and_year_boundaries() {
        assert_eq!(d("2026-01-31").add_days(1).format(), "2026-02-01");
        assert_eq!(d("2026-12-25").add_days(7).format(), "2027-01-01");
        assert_eq!(d("2024-02-28").add_days(1).format(), "2024-02-29");
        assert_eq!(d("2026-03-01").add_days(-1).format(), "2026-02-28");
    }

    #[test]
    fn add_months_clamps_to_month_length() {
        assert_eq!(d("2026-01-31").add_months(1).format(), "2026-02-28");
        assert_eq!(d("2024-01-31").add_months(1).format(), "2024-02-29");
        assert_eq!(d("2026-08-31").add_months(1).format(), "2026-09-30");
        assert_eq!(d("2026-12-15").add_months(1).format(), "2027-01-15");
        assert_eq!(d("2026-01-15").add_months(-1).format(), "2025-12-15");
    }

    #[test]
    fn repeat_steps_match_their_labels() {
        let base = d("2026-09-11");
        assert_eq!(base.add_repeat(DocketRepeat::Daily).format(), "2026-09-12");
        assert_eq!(base.add_repeat(DocketRepeat::Weekly).format(), "2026-09-18");
        assert_eq!(
            base.add_repeat(DocketRepeat::Fortnightly).format(),
            "2026-09-25"
        );
        assert_eq!(
            base.add_repeat(DocketRepeat::Monthly).format(),
            "2026-10-11"
        );
    }

    #[test]
    fn next_due_lands_after_today() {
        let today = d("2026-09-11");
        // On time: one step.
        assert_eq!(
            next_due(Some(d("2026-09-11")), DocketRepeat::Weekly, today).format(),
            "2026-09-18"
        );
        // Three weeks late: skip the missed occurrences.
        assert_eq!(
            next_due(Some(d("2026-08-20")), DocketRepeat::Weekly, today).format(),
            "2026-09-17"
        );
        // Never dated: from today.
        assert_eq!(
            next_due(None, DocketRepeat::Daily, today).format(),
            "2026-09-12"
        );
    }
}
