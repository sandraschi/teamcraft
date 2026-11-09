use super::*;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}
fn dt(s: &str) -> NaiveDateTime {
    parse_datetime(s).unwrap()
}

fn team() -> Team {
    let t = Team::open_in_memory().unwrap();
    t.add_employee(&NewEmployee {
        code: "jm".into(),
        name: "Joe Mocky".into(),
        start_date: Some(d(2024, 3, 1)),
        ..Default::default()
    })
    .unwrap();
    t
}

#[test]
fn employee_validation() {
    let t = team();
    assert_eq!(t.employee("JM").unwrap().weekly_minutes, 2400);
    assert!(t
        .add_employee(&NewEmployee {
            code: "JM".into(),
            name: "dup".into(),
            ..Default::default()
        })
        .is_err());
    assert!(t
        .add_employee(&NewEmployee {
            code: "bad code".into(),
            name: "x".into(),
            ..Default::default()
        })
        .is_err());
    assert!(t
        .add_employee(&NewEmployee {
            code: "HM".into(),
            name: "x".into(),
            weekly_minutes: Some(61 * 60),
            ..Default::default()
        })
        .is_err());
    let u = t
        .update_employee(
            "jm",
            &EmployeePatch {
                weekly_minutes: Some(20 * 60),
                kv_name: Some(Some("IT".into())),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!((u.weekly_minutes, u.kv_name.as_deref()), (1200, Some("IT")));
}

#[test]
fn parse_helpers() {
    assert_eq!(dt("2026-11-09T08:15:42"), dt("09.11.2026 08:15"));
    assert!(parse_datetime("8 Uhr").is_err());
    assert_eq!(parse_week("2026-W46").unwrap().0, d(2026, 11, 9));
    assert_eq!(week_label(d(2027, 1, 1)), "2026-W53");
    assert_eq!(fmt_hm(-75), "-1:15");
}

#[test]
fn clock_and_overlap_rules() {
    let t = team();
    let open = t.clock_in("JM", dt("2026-11-09 08:00"), "").unwrap();
    assert!(open.minutes.is_none());
    assert!(
        t.clock_in("JM", dt("2026-11-09 09:00"), "").is_err(),
        "double clock-in"
    );
    assert!(
        t.clock_out("JM", dt("2026-11-09 07:00"), 0).is_err(),
        "end before start"
    );
    assert!(
        t.clock_out("JM", dt("2026-11-09 12:00"), 300).is_err(),
        "break longer than shift"
    );
    let done = t.clock_out("JM", dt("2026-11-09 16:30"), 30).unwrap();
    assert_eq!(done.minutes, Some(480));
    assert!(
        t.add_time("JM", dt("2026-11-09 16:00"), dt("2026-11-09 18:00"), 0, "")
            .is_err(),
        "overlap"
    );
    t.add_time(
        "JM",
        dt("2026-11-09 16:30"),
        dt("2026-11-09 18:00"),
        0,
        "evening",
    )
    .unwrap();
    assert!(
        t.add_time("JM", dt("2026-11-10 08:00"), dt("2026-11-11 09:00"), 0, "")
            .is_err(),
        "over 24h"
    );
    assert_eq!(
        t.entries(Some("jm"), d(2026, 11, 9), d(2026, 11, 9))
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn confirmed_week_is_locked_twice() {
    let t = team();
    let e = t
        .add_time("JM", dt("2026-11-02 08:00"), dt("2026-11-02 12:00"), 0, "")
        .unwrap();
    t.clock_in("JM", dt("2026-11-03 08:00"), "").unwrap();
    assert!(
        t.confirm_week("JM", "2026-W45", d(2026, 11, 9)).is_err(),
        "open entry blocks confirm"
    );
    t.clock_out("JM", dt("2026-11-03 12:00"), 0).unwrap();
    assert!(
        t.confirm_week("JM", "2026-W47", d(2026, 11, 9)).is_err(),
        "future week"
    );
    let (mon, n) = t.confirm_week("JM", "2026-W45", d(2026, 11, 9)).unwrap();
    assert_eq!((mon, n), (d(2026, 11, 2), 2));
    assert!(
        t.confirm_week("JM", "2026-W45", d(2026, 11, 9)).is_err(),
        "twice"
    );
    assert!(matches!(t.delete_time(e.id), Err(TeamError::Locked(_))));
    assert!(matches!(
        t.add_time("JM", dt("2026-11-04 08:00"), dt("2026-11-04 09:00"), 0, ""),
        Err(TeamError::Locked(_))
    ));
    // The SQLite trigger holds even when Rust is bypassed.
    let raw = t.conn().execute(
        "UPDATE time_entries SET break_minutes = 5 WHERE id = ?1",
        params![e.id],
    );
    assert!(raw.unwrap_err().to_string().contains("locked"));
    assert!(t.entry(e.id).unwrap().locked);
}

#[test]
fn leave_counts_working_days_and_guards() {
    let t = team();
    // 2026-10-26 is Nationalfeiertag (Monday): Mon-Fri = 4 working days.
    let l = t
        .request_leave(
            "JM",
            LeaveKind::Vacation,
            d(2026, 10, 26),
            d(2026, 10, 30),
            false,
            "Herbst",
        )
        .unwrap();
    assert_eq!((l.days, l.status), (4.0, LeaveStatus::Requested));
    assert!(
        t.request_leave(
            "JM",
            LeaveKind::Vacation,
            d(2026, 10, 30),
            d(2026, 11, 2),
            false,
            ""
        )
        .is_err(),
        "overlap"
    );
    assert!(
        t.request_leave(
            "JM",
            LeaveKind::Sick,
            d(2026, 11, 3),
            d(2026, 11, 3),
            false,
            "flu"
        )
        .is_err(),
        "no health detail"
    );
    let sick = t
        .request_leave(
            "JM",
            LeaveKind::Sick,
            d(2026, 11, 3),
            d(2026, 11, 4),
            false,
            "",
        )
        .unwrap();
    assert_eq!(sick.status, LeaveStatus::Approved);
    assert!(
        t.request_leave(
            "JM",
            LeaveKind::Vacation,
            d(2026, 11, 7),
            d(2026, 11, 8),
            false,
            ""
        )
        .is_err(),
        "weekend only"
    );
    assert!(
        t.request_leave(
            "JM",
            LeaveKind::Vacation,
            d(2026, 12, 28),
            d(2027, 1, 4),
            false,
            ""
        )
        .is_err(),
        "year boundary"
    );
    let half = t
        .request_leave(
            "JM",
            LeaveKind::Vacation,
            d(2026, 11, 13),
            d(2026, 11, 13),
            true,
            "",
        )
        .unwrap();
    assert_eq!(half.days, 0.5);
    t.decide_leave(l.id, true, "boss", false).unwrap();
    assert!(
        t.decide_leave(l.id, true, "boss", false).is_err(),
        "already decided"
    );
    let b = t.balance("JM", 2026).unwrap();
    assert_eq!(
        (
            b.entitlement_days,
            b.taken_days,
            b.pending_days,
            b.remaining_days,
            b.sick_days
        ),
        (25.0, 4.0, 0.5, 21.0, 2.0)
    );
    // The SQL check refuses a sick note even if Rust is bypassed.
    let raw = t.conn().execute(
        "UPDATE leaves SET note = 'x' WHERE id = ?1",
        params![sick.id],
    );
    assert!(raw.is_err());
}

#[test]
fn balance_prorata_and_overdraw_needs_force() {
    let t = Team::open_in_memory().unwrap();
    t.add_employee(&NewEmployee {
        code: "HM".into(),
        name: "Hannes Mockinger".into(),
        start_date: Some(d(2026, 9, 15)),
        leave_days_per_year: Some(25),
        ..Default::default()
    })
    .unwrap();
    // September..December = 4 months: 25 * 4 / 12 = 8.33 -> 8.5 days.
    assert_eq!(t.balance("HM", 2026).unwrap().entitlement_days, 8.5);
    assert_eq!(t.balance("HM", 2025).unwrap().entitlement_days, 0.0);
    let l = t
        .request_leave(
            "HM",
            LeaveKind::Vacation,
            d(2026, 11, 2),
            d(2026, 11, 13),
            false,
            "",
        )
        .unwrap();
    assert_eq!(l.days, 10.0);
    assert!(t
        .decide_leave(l.id, true, "boss", false)
        .unwrap_err()
        .to_string()
        .contains("force"));
    t.adjust_leave("HM", 2026, 2, "goodwill").unwrap();
    assert!(
        t.decide_leave(l.id, true, "boss", false).is_err(),
        "still 0.5 short"
    );
    let ok = t.decide_leave(l.id, true, "boss", true).unwrap();
    assert_eq!(ok.status, LeaveStatus::Approved);
    assert_eq!(t.balance("HM", 2026).unwrap().remaining_days, -0.5);
    assert!(t.adjust_leave("HM", 2026, 2, "  ").is_err());
}

#[test]
fn violations_refresh_and_clear() {
    let t = team();
    let v = |day: u32, kind: &str| NewViolation {
        date: d(2026, 11, day),
        kind: kind.into(),
        severity: "violation".into(),
        minutes: 10,
        detail: "x".into(),
    };
    assert_eq!(
        t.record_violations(
            "JM",
            d(2026, 11, 2),
            d(2026, 11, 8),
            &[v(3, "break_missing"), v(4, "daily_over_12h")]
        )
        .unwrap(),
        (2, 0)
    );
    assert_eq!(
        t.record_violations(
            "JM",
            d(2026, 11, 2),
            d(2026, 11, 8),
            &[v(3, "break_missing")]
        )
        .unwrap(),
        (0, 1)
    );
    assert_eq!(t.violations(Some("JM"), false).unwrap().len(), 1);
    let all = t.violations(None, true).unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|x| x.cleared_at.is_some()));
}

#[test]
fn kv_lookup_by_validity() {
    let t = team();
    let row = |from: NaiveDate, cents: i64| KvRow {
        kv: "IT".into(),
        group: "ST1".into(),
        valid_from: from,
        monthly_min_cents: cents,
        weekly_minutes: 2310,
    };
    t.kv_upsert(&[row(d(2025, 1, 1), 250_000), row(d(2026, 1, 1), 260_000)])
        .unwrap();
    assert_eq!(
        t.kv_minimum("IT", "ST1", d(2025, 6, 1))
            .unwrap()
            .unwrap()
            .monthly_min_cents,
        250_000
    );
    assert_eq!(
        t.kv_minimum("IT", "ST1", d(2026, 6, 1))
            .unwrap()
            .unwrap()
            .monthly_min_cents,
        260_000
    );
    assert!(t.kv_minimum("IT", "ST1", d(2024, 6, 1)).unwrap().is_none());
    t.kv_upsert(&[row(d(2026, 1, 1), 261_000)]).unwrap();
    assert_eq!(t.kv_rows().unwrap().len(), 2);
}
