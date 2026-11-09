//! Arbeitszeitgesetz (AZG) checks over recorded time. Pure functions: the store
//! records the findings, nothing here edits time data.
//!
//! Rules checked (general AZG, no collective-agreement extensions):
//! - more than 10 h work on a day: warning (needs an overtime arrangement, section 7)
//! - more than 12 h on a day: violation (absolute daily maximum, section 9)
//! - more than 60 h in an ISO week: violation (section 9)
//! - more than 48 h average over 17 weeks: violation (section 9(4))
//! - more than 6 h work with less than 30 min break: violation (section 11);
//!   gaps of 10 min or more between entries of the same day count as break
//! - less than 11 h rest between working days: violation (ARG/AZG section 12)
//!
//! Entries are attributed to the day they start on. Missing weeks in the
//! 17-week window count as zero, so incomplete data can hide a violation but
//! never invent one.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Weekday};
use std::collections::BTreeMap;
use teamcraft_store::NewViolation;

pub const DAILY_NORMAL_MAX: i64 = 10 * 60;
pub const DAILY_ABS_MAX: i64 = 12 * 60;
pub const WEEKLY_ABS_MAX: i64 = 60 * 60;
pub const AVG_WEEKS: i64 = 17;
pub const AVG_WEEKLY_MAX: i64 = 48 * 60;
pub const BREAK_AFTER: i64 = 6 * 60;
pub const BREAK_MIN: i64 = 30;
pub const GAP_COUNTS_AS_BREAK: i64 = 10;
pub const DAILY_REST_MIN: i64 = 11 * 60;

#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub break_minutes: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Day {
    pub work: i64,
    pub breaks: i64,
    pub first_start: Option<NaiveDateTime>,
    pub last_end: Option<NaiveDateTime>,
}

/// Work and break minutes per day (by start date).
pub fn days(spans: &[Span]) -> BTreeMap<NaiveDate, Day> {
    let mut sorted: Vec<Span> = spans.to_vec();
    sorted.sort_by_key(|s| s.start);
    let mut out: BTreeMap<NaiveDate, Day> = BTreeMap::new();
    for s in sorted {
        let day = out.entry(s.start.date()).or_default();
        if let Some(prev_end) = day.last_end {
            let gap = (s.start - prev_end).num_minutes();
            if gap >= GAP_COUNTS_AS_BREAK {
                day.breaks += gap;
            }
        }
        day.work += (s.end - s.start).num_minutes() - s.break_minutes;
        day.breaks += s.break_minutes;
        day.first_start = Some(day.first_start.map_or(s.start, |f| f.min(s.start)));
        day.last_end = Some(day.last_end.map_or(s.end, |l| l.max(s.end)));
    }
    out
}

pub fn monday_of(d: NaiveDate) -> NaiveDate {
    d - Duration::days(i64::from(d.weekday().num_days_from_monday()))
}

fn finding(
    date: NaiveDate,
    kind: &str,
    severity: &str,
    minutes: i64,
    detail: String,
) -> NewViolation {
    NewViolation {
        date,
        kind: kind.into(),
        severity: severity.into(),
        minutes,
        detail,
    }
}

fn hm(m: i64) -> String {
    teamcraft_store::fmt_hm(m)
}

/// Findings for days and weeks inside `from..=to`. Pass spans from at least
/// 17 weeks before `from` so the average and the first rest period are complete.
pub fn check(spans: &[Span], from: NaiveDate, to: NaiveDate) -> Vec<NewViolation> {
    let per_day = days(spans);
    let mut out = Vec::new();
    for (date, day) in per_day.range(from..=to) {
        if day.work > DAILY_ABS_MAX {
            out.push(finding(
                *date,
                "daily_over_12h",
                "violation",
                day.work - DAILY_ABS_MAX,
                format!("{} worked, absolute maximum 12:00", hm(day.work)),
            ));
        } else if day.work > DAILY_NORMAL_MAX {
            out.push(finding(
                *date,
                "daily_over_10h",
                "warning",
                day.work - DAILY_NORMAL_MAX,
                format!(
                    "{} worked; over 10:00 needs an overtime arrangement",
                    hm(day.work)
                ),
            ));
        }
        if day.work > BREAK_AFTER && day.breaks < BREAK_MIN {
            out.push(finding(
                *date,
                "break_missing",
                "violation",
                BREAK_MIN - day.breaks,
                format!(
                    "{} worked with {} min break; 30 min required above 6:00",
                    hm(day.work),
                    day.breaks
                ),
            ));
        }
    }
    // Daily rest between consecutive working days.
    let ordered: Vec<(&NaiveDate, &Day)> = per_day.iter().collect();
    for pair in ordered.windows(2) {
        let (_, prev) = pair[0];
        let (date, next) = pair[1];
        if *date < from || *date > to {
            continue;
        }
        if let (Some(end), Some(start)) = (prev.last_end, next.first_start) {
            let rest = (start - end).num_minutes();
            if rest < DAILY_REST_MIN {
                out.push(finding(
                    *date,
                    "daily_rest_under_11h",
                    "violation",
                    DAILY_REST_MIN - rest,
                    format!(
                        "only {} rest since {}",
                        hm(rest),
                        end.format("%d.%m. %H:%M")
                    ),
                ));
            }
        }
    }
    // Weekly maximum and 17-week average.
    let mut weeks: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    for (date, day) in &per_day {
        *weeks.entry(monday_of(*date)).or_default() += day.work;
    }
    let mut monday = monday_of(from);
    while monday <= to {
        let total = weeks.get(&monday).copied().unwrap_or(0);
        if total > WEEKLY_ABS_MAX {
            out.push(finding(
                monday,
                "weekly_over_60h",
                "violation",
                total - WEEKLY_ABS_MAX,
                format!(
                    "{} in week {}, maximum 60:00",
                    hm(total),
                    teamcraft_store::week_label(monday)
                ),
            ));
        }
        let window_start = monday - Duration::weeks(AVG_WEEKS - 1);
        let sum: i64 = weeks.range(window_start..=monday).map(|(_, m)| *m).sum();
        let avg = sum / AVG_WEEKS;
        if avg > AVG_WEEKLY_MAX {
            out.push(finding(
                monday,
                "average_over_48h_17w",
                "violation",
                avg - AVG_WEEKLY_MAX,
                format!("17-week average {} per week, maximum 48:00", hm(avg)),
            ));
        }
        monday += Duration::weeks(1);
    }
    out.sort_by(|a, b| (a.date, &a.kind).cmp(&(b.date, &b.kind)));
    out
}

pub fn weekday_de(d: NaiveDate) -> &'static str {
    match d.weekday() {
        Weekday::Mon => "Mo",
        Weekday::Tue => "Di",
        Weekday::Wed => "Mi",
        Weekday::Thu => "Do",
        Weekday::Fri => "Fr",
        Weekday::Sat => "Sa",
        Weekday::Sun => "So",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use teamcraft_store::parse_datetime;

    fn s(a: &str, b: &str, brk: i64) -> Span {
        Span {
            start: parse_datetime(a).unwrap(),
            end: parse_datetime(b).unwrap(),
            break_minutes: brk,
        }
    }
    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }
    fn kinds(v: &[NewViolation]) -> Vec<(NaiveDate, &str)> {
        v.iter().map(|x| (x.date, x.kind.as_str())).collect()
    }

    #[test]
    fn clean_week_has_no_findings() {
        let spans: Vec<Span> = (2..=6)
            .map(|day| {
                s(
                    &format!("2026-11-{day:02} 08:00"),
                    &format!("2026-11-{day:02} 16:30"),
                    30,
                )
            })
            .collect();
        assert!(check(&spans, d(2026, 11, 2), d(2026, 11, 8)).is_empty());
    }

    #[test]
    fn daily_limits_and_breaks() {
        let spans = vec![
            s("2026-11-02 07:00", "2026-11-02 18:00", 30), // 10:30 -> warning
            s("2026-11-03 06:00", "2026-11-03 19:00", 30), // 12:30 -> violation
            s("2026-11-04 08:00", "2026-11-04 15:00", 0),  // 7:00, no break
            s("2026-11-05 08:00", "2026-11-05 12:00", 0), // split day: 15 min gap + 15 min break = 30
            s("2026-11-05 12:15", "2026-11-05 16:00", 15),
        ];
        let f = check(&spans, d(2026, 11, 2), d(2026, 11, 8));
        assert_eq!(
            kinds(&f),
            vec![
                (d(2026, 11, 2), "daily_over_10h"),
                (d(2026, 11, 3), "daily_over_12h"),
                (d(2026, 11, 4), "break_missing")
            ]
        );
        assert_eq!(f[1].minutes, 30);
        assert_eq!(f[2].minutes, 30);
    }

    #[test]
    fn short_rest_and_window_edges() {
        let spans = vec![
            s("2026-11-01 14:00", "2026-11-01 22:30", 30),
            s("2026-11-02 07:00", "2026-11-02 15:00", 30),
            s("2026-11-03 06:00", "2026-11-03 10:00", 0),
        ];
        let f = check(&spans, d(2026, 11, 2), d(2026, 11, 8));
        // 22:30 -> 07:00 is 8:30 rest (150 min short); 15:00 -> 06:00 is 15:00, fine.
        assert_eq!(kinds(&f), vec![(d(2026, 11, 2), "daily_rest_under_11h")]);
        assert_eq!(f[0].minutes, 150);
        // The same data checked only for November 1 reports nothing for Nov 2.
        assert!(check(&spans, d(2026, 11, 1), d(2026, 11, 1)).is_empty());
    }

    #[test]
    fn weekly_and_average() {
        // 6 days of 11h (each a daily warning): 66h in the week.
        let spans: Vec<Span> = (2..=7)
            .map(|day| {
                s(
                    &format!("2026-11-{day:02} 07:00"),
                    &format!("2026-11-{day:02} 18:30"),
                    30,
                )
            })
            .collect();
        let f = check(&spans, d(2026, 11, 2), d(2026, 11, 8));
        assert!(f
            .iter()
            .any(|x| x.kind == "weekly_over_60h" && x.minutes == 360));
        assert!(
            !f.iter().any(|x| x.kind == "average_over_48h_17w"),
            "one heavy week does not break the average"
        );
        // 17 weeks of 50h: average violated.
        let mut long = Vec::new();
        let mut monday = d(2026, 7, 20);
        for _ in 0..17 {
            for i in 0..5 {
                let day = monday + Duration::days(i);
                long.push(Span {
                    start: day.and_hms_opt(7, 0, 0).unwrap(),
                    end: day.and_hms_opt(17, 30, 0).unwrap(),
                    break_minutes: 30,
                });
            }
            monday += Duration::weeks(1);
        }
        let last = monday - Duration::weeks(1);
        let f = check(&long, last, last + Duration::days(6));
        assert!(
            f.iter()
                .any(|x| x.kind == "average_over_48h_17w" && x.minutes == 120),
            "{f:?}"
        );
    }
}
