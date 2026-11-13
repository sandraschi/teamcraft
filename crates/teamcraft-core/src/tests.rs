use super::*;

fn ctx(tag: &str) -> Ctx {
    let root = std::env::temp_dir().join(format!("teamcraft-bus-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    Ctx {
        team: Team::open_in_memory().unwrap(),
        confinement: Confinement(None),
        bus: Bus::new(root),
        actor: "test".into(),
    }
}

fn call(c: &mut Ctx, name: &str, a: Value) -> Result<Value, String> {
    let cmd = Command::from_tool(name, &a)?;
    dispatch(c, cmd)
}

#[test]
fn catalog_tools_are_routable() {
    assert_eq!(tools().len(), 26);
    for t in tools() {
        let name = t["name"].as_str().unwrap();
        if let Err(err) = Command::from_tool(name, &json!({})) {
            assert!(!err.starts_with("unknown tool"), "{name}");
        }
    }
}

#[test]
fn argument_parsing() {
    assert_eq!(hours_to_minutes(&json!("38,5")).unwrap(), 2310);
    assert_eq!(hours_to_minutes(&json!(20)).unwrap(), 1200);
    assert!(hours_to_minutes(&json!(0)).is_err());
    assert_eq!(days_to_half_days(&json!(2.5)).unwrap(), 5);
    assert!(days_to_half_days(&json!(0.3)).is_err());
    assert!(
        Command::from_tool(
            "team_employee_add",
            &json!({ "code": "X", "name": "x", "monthly_gross": 2850.0 })
        )
        .is_err(),
        "float money refused"
    );
    // Night shift: 22:00 to 06:00 on one date ends the next morning.
    match Command::from_tool(
        "team_time_add",
        &json!({ "employee": "X", "date": "05.11.2026", "start": "22:00", "end": "06:00" }),
    )
    .unwrap()
    {
        Command::TimeAdd { start, end, .. } => assert_eq!((end - start).num_hours(), 8),
        other => unreachable!("{other:?}"),
    }
}

#[test]
fn demo_findings_balances_and_kv() {
    let mut c = ctx("demo");
    let seeded = call(&mut c, "team_demo_seed", json!({})).unwrap();
    assert_eq!(seeded["azg"]["violations"], 2, "{seeded}");
    assert_eq!(seeded["azg"]["warnings"], 1);
    assert!(call(&mut c, "team_demo_seed", json!({})).is_err());
    let v = call(&mut c, "team_violations", json!({})).unwrap();
    let kinds: Vec<&str> = v["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec!["daily_over_10h", "break_missing", "daily_rest_under_11h"]
    );
    // Hannes' week is confirmed: his entries are read-only.
    let hm = call(
        &mut c,
        "team_time_list",
        json!({ "employee": "HM", "from": "2026-11-02", "to": "2026-11-08" }),
    )
    .unwrap();
    let first = hm["entries"][0]["id"].as_i64().unwrap();
    assert!(call(&mut c, "team_time_delete", json!({ "id": first }))
        .unwrap_err()
        .contains("locked"));
    let kv = call(&mut c, "team_kv_check", json!({ "date": "2026-11-09" })).unwrap();
    assert_eq!(kv["underpaid"], 1);
    let am = kv["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["employee"] == "AM")
        .unwrap();
    assert_eq!(am["minimum_pro_rata"], "1.246,75");
    assert_eq!(am["shortfall"], "96,75");
    let bal = call(
        &mut c,
        "team_leave_balance",
        json!({ "employee": "HM", "year": 2026 }),
    )
    .unwrap();
    assert_eq!(bal["balances"][0]["taken_days"], 4.0);
    assert_eq!(bal["balances"][0]["entitlement_days"], 25.0);
}

#[test]
fn azg_fix_clears_finding_without_touching_time() {
    let mut c = ctx("clear");
    call(
        &mut c,
        "team_employee_add",
        json!({ "code": "JM", "name": "Joe Mocky", "start_date": "2026-01-01" }),
    )
    .unwrap();
    let id = call(
        &mut c,
        "team_time_add",
        json!({ "employee": "JM", "date": "2026-11-04", "start": "08:00", "end": "15:00" }),
    )
    .unwrap()["id"]
        .as_i64()
        .unwrap();
    let r = call(
        &mut c,
        "team_azg_check",
        json!({ "employee": "JM", "from": "2026-11-02", "to": "2026-11-08" }),
    )
    .unwrap();
    assert_eq!(r["violations"], 1);
    // The entry was wrong (break forgotten): delete and re-enter it, then re-check.
    call(&mut c, "team_time_delete", json!({ "id": id })).unwrap();
    call(&mut c, "team_time_add", json!({ "employee": "JM", "date": "2026-11-04", "start": "08:00", "end": "15:30", "break_minutes": 30 })).unwrap();
    let r = call(
        &mut c,
        "team_azg_check",
        json!({ "employee": "JM", "from": "2026-11-02", "to": "2026-11-08" }),
    )
    .unwrap();
    assert_eq!(
        (
            r["violations"].as_u64(),
            r["employees"][0]["cleared"].as_u64()
        ),
        (Some(0), Some(1))
    );
    let history = call(
        &mut c,
        "team_violations",
        json!({ "include_cleared": true }),
    )
    .unwrap();
    assert_eq!(
        history["violations"].as_array().map(Vec::len),
        Some(1),
        "kept for the record"
    );
}

#[test]
fn timesheet_target_and_csv() {
    let mut c = ctx("sheet");
    let dir = std::env::temp_dir().join(format!("teamcraft-sheet-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    c.confinement = Confinement::roots(std::slice::from_ref(&dir)).unwrap();
    call(&mut c, "team_employee_add", json!({ "code": "JM", "name": "Joe Mocky", "weekly_hours": 40, "start_date": "2025-01-01" })).unwrap();
    call(&mut c, "team_time_add", json!({ "employee": "JM", "date": "2026-10-02", "start": "08:00", "end": "17:00", "break_minutes": 30 })).unwrap();
    call(
        &mut c,
        "team_leave_request",
        json!({ "employee": "JM", "kind": "sick", "from": "2026-10-05", "to": "2026-10-06" }),
    )
    .unwrap();
    let path = dir.join("jm-2026-10.csv");
    let sheet = call(
        &mut c,
        "team_timesheet",
        json!({ "employee": "JM", "month": "2026-10", "path": path.display().to_string() }),
    )
    .unwrap();
    // October 2026: 22 weekdays minus Nationalfeiertag = 21 working days, 2 sick -> 19 x 8:00.
    assert_eq!(sheet["target"], "152:00");
    assert_eq!(sheet["worked"], "8:30");
    assert_eq!(sheet["balance"], "-143:30");
    let csv = std::fs::read_to_string(&path).unwrap();
    assert!(csv.contains("26.10.2026;Mo;;;0;0:00;;Nationalfeiertag;"));
    assert!(csv.contains("05.10.2026;Mo;;;0;0:00;sick;;"));
    assert!(call(
        &mut c,
        "team_timesheet",
        json!({ "employee": "JM", "month": "2026-10", "path": "C:/Windows/x.csv" })
    )
    .is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn kv_csv_parsing() {
    let rows = parse_kv_csv("\u{feff}kv;group;valid_from;monthly_min;weekly_hours\nDEMO;A;01.01.2026;2.400,00;38,5\n# comment\n\"DEMO\";\"B\";2026-01-01;3.100,00;40\n").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0].monthly_min_cents, rows[0].weekly_minutes),
        (240_000, 2310)
    );
    assert!(parse_kv_csv("DEMO;A;01.01.2026;2400.5;38,5").is_err());
    assert!(parse_kv_csv("DEMO;A;01.01.2026;2.400,00")
        .unwrap_err()
        .contains("line 1"));
    assert!(parse_kv_csv("").is_err());
}

#[test]
fn capacity_and_event_to_taskcraft() {
    let mut c = ctx("cap");
    call(
        &mut c,
        "team_employee_add",
        json!({ "code": "JM", "name": "Joe Mocky", "weekly_hours": 40 }),
    )
    .unwrap();
    call(
        &mut c,
        "team_employee_add",
        json!({ "code": "AM", "name": "Anna Mockova", "weekly_hours": 20 }),
    )
    .unwrap();
    let l = call(
        &mut c,
        "team_leave_request",
        json!({ "employee": "JM", "kind": "vacation", "from": "2026-10-28", "to": "2026-10-29" }),
    )
    .unwrap();
    call(
        &mut c,
        "team_leave_decide",
        json!({ "id": l["id"], "approve": true }),
    )
    .unwrap();
    // Week 44: Monday is Nationalfeiertag -> 4 working days. JM: 2 days off -> 16h. AM: 4/5 of 20h = 16h.
    let cap = call(
        &mut c,
        "team_capacity",
        json!({ "week": "2026-W44", "emit": true }),
    )
    .unwrap();
    assert_eq!(cap["working_days"], 4);
    assert_eq!(cap["total"], "32:00");
    let pending = c.bus.pending("taskcraft").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].kind, "capacity.changed");
    assert_eq!(pending[0].payload["week"], "2026-W44");
    let _ = std::fs::remove_dir_all(c.bus.root());
}

#[test]
fn hired_event_opens_one_employee_without_a_wage() {
    let mut c = ctx("hired");
    let hired = json!({
        "candidate_id": "C-1",
        "name": "Ada",
        "email": "ada@example.at",
        "salary_eur": 70000,
        "azg_weekly_cap": 38.5,
        "annual_leave_days": 25
    });
    c.bus
        .emit("hirecraft", "teamcraft", "candidate.hired", hired.clone())
        .unwrap();
    c.bus
        .emit("hirecraft", "teamcraft", "candidate.hired", hired)
        .unwrap();
    c.bus
        .emit(
            "hirecraft",
            "teamcraft",
            "candidate.hired",
            json!({ "candidate_id": "C-2" }),
        )
        .unwrap();
    let result = call(&mut c, "team_drain_inbox", json!({})).unwrap();
    assert_eq!(result["processed"], 3);
    assert_eq!(result["rejected"], 1);
    assert_eq!(result["created"].as_array().unwrap().len(), 1);
    let ada = c.team.employee("C1").unwrap();
    assert_eq!(ada.name, "Ada");
    assert_eq!(ada.weekly_minutes, 2310);
    assert_eq!(ada.leave_days_per_year, 25);
    assert_eq!(ada.monthly_gross_cents, None);
    assert!(c.bus.pending("teamcraft").unwrap().is_empty());
    let failed = c
        .bus
        .history("teamcraft", craft_biz::bus::EventState::Failed, 10)
        .unwrap();
    assert_eq!(failed.len(), 1);
    let _ = std::fs::remove_dir_all(c.bus.root());
}
