//! teamcraft-core: the command bus shared by CLI, MCP and the desktop app.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod azg;

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime};
use craft_biz::at::parse_date;
use craft_biz::bus::Bus;
pub use craft_biz::confine::Confinement;
use craft_biz::holidays;
use craft_biz::mcp::tool;
use craft_biz::Money;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use teamcraft_store::{
    fmt_hm, leave_half_days_in, parse_datetime, parse_week, week_label, EmployeePatch, KvRow,
    LeaveKind, LeaveStatus, NewEmployee, Team, TeamError,
};

pub const APP: &str = "teamcraft";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_IMPORT_BYTES: u64 = 1024 * 1024;

pub struct Ctx {
    pub team: Team,
    pub confinement: Confinement,
    pub bus: Bus,
    /// "agent" over MCP, the OS user on the CLI.
    pub actor: String,
}

#[derive(Debug, Clone)]
pub enum Command {
    Status,
    EmployeeAdd(NewEmployee),
    EmployeeList {
        include_inactive: bool,
    },
    EmployeeUpdate {
        code: String,
        patch: EmployeePatch,
    },
    ClockIn {
        employee: String,
        at: Option<NaiveDateTime>,
        note: String,
    },
    ClockOut {
        employee: String,
        at: Option<NaiveDateTime>,
        break_minutes: i64,
    },
    TimeAdd {
        employee: String,
        start: NaiveDateTime,
        end: NaiveDateTime,
        break_minutes: i64,
        note: String,
    },
    TimeList {
        employee: Option<String>,
        from: NaiveDate,
        to: NaiveDate,
    },
    TimeDelete {
        id: i64,
    },
    WeekConfirm {
        employee: String,
        week: String,
    },
    AzgCheck {
        employee: Option<String>,
        from: NaiveDate,
        to: NaiveDate,
    },
    Violations {
        employee: Option<String>,
        include_cleared: bool,
    },
    LeaveRequest {
        employee: String,
        kind: LeaveKind,
        from: NaiveDate,
        to: NaiveDate,
        half_day: bool,
        note: String,
    },
    LeaveDecide {
        id: i64,
        approve: bool,
        force: bool,
    },
    LeaveCancel {
        id: i64,
    },
    LeaveList {
        employee: Option<String>,
        year: Option<i32>,
        status: Option<LeaveStatus>,
    },
    LeaveBalance {
        employee: Option<String>,
        year: Option<i32>,
    },
    LeaveAdjust {
        employee: String,
        year: i32,
        half_days: i64,
        reason: String,
    },
    Holidays {
        year: i32,
    },
    KvImport {
        path: String,
    },
    KvList,
    KvCheck {
        on: Option<NaiveDate>,
    },
    Timesheet {
        employee: String,
        month: String,
        path: Option<String>,
    },
    Capacity {
        week: String,
        emit: bool,
    },
    DemoSeed,
}

// ---------- argument helpers ----------

fn s<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument {k:?}"))
}
fn opt_s(a: &Value, k: &str) -> Option<String> {
    a.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
}
fn e(err: TeamError) -> String {
    err.to_string()
}
fn date(v: &str) -> Result<NaiveDate, String> {
    parse_date(v).map_err(|x| x.to_string())
}
fn opt_date(a: &Value, k: &str) -> Result<Option<NaiveDate>, String> {
    opt_s(a, k).map(|d| date(&d)).transpose()
}
fn req_date(a: &Value, k: &str) -> Result<NaiveDate, String> {
    date(s(a, k)?)
}
fn int(a: &Value, k: &str) -> Result<Option<i64>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{k} must be an integer")),
    }
}
fn flag(a: &Value, k: &str) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(false)
}
fn today() -> NaiveDate {
    Local::now().date_naive()
}
fn year(a: &Value) -> Result<Option<i32>, String> {
    int(a, "year")?
        .map(|y| i32::try_from(y).map_err(|_| "year out of range".to_string()))
        .transpose()
}

/// Hours as number (38.5) or DE string ("38,5"), to whole minutes.
pub fn hours_to_minutes(v: &Value) -> Result<i64, String> {
    let h = match v {
        Value::Number(n) => n.as_f64().ok_or("hours must be a number")?,
        Value::String(t) => t
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .map_err(|_| format!("bad hours {t:?}"))?,
        _ => return Err("hours must be a number".into()),
    };
    if !h.is_finite() || h <= 0.0 || h > 60.0 {
        return Err("hours must be above 0 and at most 60".into());
    }
    Ok((h * 60.0).round() as i64)
}

/// Days as a multiple of 0.5 to half-days.
pub fn days_to_half_days(v: &Value) -> Result<i64, String> {
    let d = v
        .as_f64()
        .or_else(|| {
            v.as_str()
                .and_then(|t| t.trim().replace(',', ".").parse().ok())
        })
        .ok_or("days must be a number")?;
    let hd = (d * 2.0).round();
    if !d.is_finite() || (d * 2.0 - hd).abs() > 1e-9 {
        return Err("days must be a multiple of 0.5".into());
    }
    Ok(hd as i64)
}

fn money_arg(v: &Value) -> Result<i64, String> {
    match v {
        Value::String(t) => Money::parse_de(t)
            .map(Money::cents)
            .map_err(|x| x.to_string()),
        _ => Err("amounts are DE strings like \"2.850,00\" (no floats)".into()),
    }
}

/// `HH:MM` on a given date, or a full date-time.
fn time_on(day: Option<NaiveDate>, v: &str) -> Result<NaiveDateTime, String> {
    if let Ok(t) = NaiveTime::parse_from_str(v.trim(), "%H:%M") {
        let d = day.ok_or_else(|| format!("{v:?} is a time only; also pass date"))?;
        return Ok(d.and_time(t));
    }
    parse_datetime(v).map_err(e)
}

impl Command {
    pub fn from_tool(name: &str, a: &Value) -> Result<Command, String> {
        let name = craft_biz::mcp::normalize_tool_name(name);
        Ok(match name.as_str() {
            "team_status" => Command::Status,
            "team_employee_add" => Command::EmployeeAdd(NewEmployee {
                code: s(a, "code")?.into(),
                name: s(a, "name")?.into(),
                weekly_minutes: a
                    .get("weekly_hours")
                    .filter(|v| !v.is_null())
                    .map(hours_to_minutes)
                    .transpose()?,
                leave_days_per_year: int(a, "leave_days")?,
                start_date: opt_date(a, "start_date")?,
                kv_name: opt_s(a, "kv_name"),
                kv_group: opt_s(a, "kv_group"),
                monthly_gross_cents: a
                    .get("monthly_gross")
                    .filter(|v| !v.is_null())
                    .map(money_arg)
                    .transpose()?,
            }),
            "team_employee_list" => Command::EmployeeList {
                include_inactive: flag(a, "include_inactive"),
            },
            "team_employee_update" => {
                let nullable_s = |k: &str| -> Result<Option<Option<String>>, String> {
                    match a.get(k) {
                        None => Ok(None),
                        Some(Value::Null) => Ok(Some(None)),
                        Some(Value::String(v)) => Ok(Some(Some(v.clone()))),
                        _ => Err(format!("{k} must be a string or null")),
                    }
                };
                Command::EmployeeUpdate {
                    code: s(a, "code")?.into(),
                    patch: EmployeePatch {
                        name: opt_s(a, "name"),
                        weekly_minutes: a
                            .get("weekly_hours")
                            .filter(|v| !v.is_null())
                            .map(hours_to_minutes)
                            .transpose()?,
                        leave_days_per_year: int(a, "leave_days")?,
                        kv_name: nullable_s("kv_name")?,
                        kv_group: nullable_s("kv_group")?,
                        monthly_gross_cents: match a.get("monthly_gross") {
                            None => None,
                            Some(Value::Null) => Some(None),
                            Some(v) => Some(Some(money_arg(v)?)),
                        },
                        active: a.get("active").and_then(Value::as_bool),
                    },
                }
            }
            "team_clock_in" => Command::ClockIn {
                employee: s(a, "employee")?.into(),
                at: opt_s(a, "at")
                    .map(|v| parse_datetime(&v).map_err(e))
                    .transpose()?,
                note: opt_s(a, "note").unwrap_or_default(),
            },
            "team_clock_out" => Command::ClockOut {
                employee: s(a, "employee")?.into(),
                at: opt_s(a, "at")
                    .map(|v| parse_datetime(&v).map_err(e))
                    .transpose()?,
                break_minutes: int(a, "break_minutes")?.unwrap_or(0),
            },
            "team_time_add" => {
                let day = opt_date(a, "date")?;
                let start = time_on(day, s(a, "start")?)?;
                let mut end = time_on(day.or(Some(start.date())), s(a, "end")?)?;
                // "22:00" to "06:00" on one date means a night shift ending next morning.
                if end <= start && opt_s(a, "end").is_some_and(|v| v.trim().len() <= 5) {
                    end += Duration::days(1);
                }
                Command::TimeAdd {
                    employee: s(a, "employee")?.into(),
                    start,
                    end,
                    break_minutes: int(a, "break_minutes")?.unwrap_or(0),
                    note: opt_s(a, "note").unwrap_or_default(),
                }
            }
            "team_time_list" => Command::TimeList {
                employee: opt_s(a, "employee"),
                from: req_date(a, "from")?,
                to: req_date(a, "to")?,
            },
            "team_time_delete" => Command::TimeDelete {
                id: int(a, "id")?.ok_or("missing id")?,
            },
            "team_week_confirm" => Command::WeekConfirm {
                employee: s(a, "employee")?.into(),
                week: s(a, "week")?.into(),
            },
            "team_azg_check" => Command::AzgCheck {
                employee: opt_s(a, "employee"),
                from: req_date(a, "from")?,
                to: req_date(a, "to")?,
            },
            "team_violations" => Command::Violations {
                employee: opt_s(a, "employee"),
                include_cleared: flag(a, "include_cleared"),
            },
            "team_leave_request" => {
                let from = req_date(a, "from")?;
                Command::LeaveRequest {
                    employee: s(a, "employee")?.into(),
                    kind: LeaveKind::parse(s(a, "kind")?).map_err(e)?,
                    from,
                    to: opt_date(a, "to")?.unwrap_or(from),
                    half_day: flag(a, "half_day"),
                    note: opt_s(a, "note").unwrap_or_default(),
                }
            }
            "team_leave_decide" => Command::LeaveDecide {
                id: int(a, "id")?.ok_or("missing id")?,
                approve: a
                    .get("approve")
                    .and_then(Value::as_bool)
                    .ok_or("missing boolean approve")?,
                force: flag(a, "force"),
            },
            "team_leave_cancel" => Command::LeaveCancel {
                id: int(a, "id")?.ok_or("missing id")?,
            },
            "team_leave_list" => Command::LeaveList {
                employee: opt_s(a, "employee"),
                year: year(a)?,
                status: opt_s(a, "status")
                    .map(|v| LeaveStatus::parse(&v).map_err(e))
                    .transpose()?,
            },
            "team_leave_balance" => Command::LeaveBalance {
                employee: opt_s(a, "employee"),
                year: year(a)?,
            },
            "team_leave_adjust" => Command::LeaveAdjust {
                employee: s(a, "employee")?.into(),
                year: year(a)?.ok_or("missing year")?,
                half_days: days_to_half_days(a.get("days").ok_or("missing days")?)?,
                reason: s(a, "reason")?.into(),
            },
            "team_holidays" => Command::Holidays {
                year: year(a)?.unwrap_or_else(|| today().year()),
            },
            "team_kv_import" => Command::KvImport {
                path: s(a, "path")?.into(),
            },
            "team_kv_list" => Command::KvList,
            "team_kv_check" => Command::KvCheck {
                on: opt_date(a, "date")?,
            },
            "team_timesheet" => Command::Timesheet {
                employee: s(a, "employee")?.into(),
                month: s(a, "month")?.into(),
                path: opt_s(a, "path"),
            },
            "team_capacity" => Command::Capacity {
                week: opt_s(a, "week").unwrap_or_else(|| week_label(today())),
                emit: flag(a, "emit"),
            },
            "team_demo_seed" => Command::DemoSeed,
            other => return Err(format!("unknown tool {other}")),
        })
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|x| x.to_string())
}

fn employee_json(emp: &teamcraft_store::Employee) -> Value {
    json!({
        "code": emp.code, "name": emp.name,
        "weekly_hours": fmt_hm(emp.weekly_minutes), "weekly_minutes": emp.weekly_minutes,
        "leave_days_per_year": emp.leave_days_per_year, "start_date": emp.start_date,
        "kv_name": emp.kv_name, "kv_group": emp.kv_group,
        "monthly_gross": emp.monthly_gross_cents.map(|c| Money::from_cents(c).format_de()),
        "active": emp.active,
    })
}

pub fn dispatch(ctx: &mut Ctx, cmd: Command) -> Result<Value, String> {
    let actor = ctx.actor.clone();
    let t = &ctx.team;
    match cmd {
        Command::Status => {
            let clocked: Vec<Value> = t
                .entries(None, today() - Duration::days(1), today())
                .map_err(e)?
                .into_iter()
                .filter(|x| x.end.is_none())
                .map(|x| json!({ "employee": x.employee, "since": x.start }))
                .collect();
            Ok(
                json!({ "app": APP, "version": VERSION, "stats": to_json(&t.stats().map_err(e)?)?, "clocked_in": clocked }),
            )
        }
        Command::EmployeeAdd(n) => Ok(employee_json(&t.add_employee(&n).map_err(e)?)),
        Command::EmployeeList { include_inactive } => Ok(
            json!({ "employees": t.employees(include_inactive).map_err(e)?.iter().map(employee_json).collect::<Vec<_>>() }),
        ),
        Command::EmployeeUpdate { code, patch } => {
            Ok(employee_json(&t.update_employee(&code, &patch).map_err(e)?))
        }
        Command::ClockIn { employee, at, note } => to_json(
            &t.clock_in(
                &employee,
                at.unwrap_or_else(|| Local::now().naive_local()),
                &note,
            )
            .map_err(e)?,
        ),
        Command::ClockOut {
            employee,
            at,
            break_minutes,
        } => to_json(
            &t.clock_out(
                &employee,
                at.unwrap_or_else(|| Local::now().naive_local()),
                break_minutes,
            )
            .map_err(e)?,
        ),
        Command::TimeAdd {
            employee,
            start,
            end,
            break_minutes,
            note,
        } => to_json(
            &t.add_time(&employee, start, end, break_minutes, &note)
                .map_err(e)?,
        ),
        Command::TimeList { employee, from, to } => time_list(t, employee.as_deref(), from, to),
        Command::TimeDelete { id } => {
            Ok(json!({ "deleted": to_json(&t.delete_time(id).map_err(e)?)? }))
        }
        Command::WeekConfirm { employee, week } => {
            let (mon, n) = t.confirm_week(&employee, &week, today()).map_err(e)?;
            Ok(
                json!({ "employee": employee.to_ascii_uppercase(), "week": week_label(mon), "entries_locked": n }),
            )
        }
        Command::AzgCheck { employee, from, to } => azg_check(t, employee.as_deref(), from, to),
        Command::Violations {
            employee,
            include_cleared,
        } => Ok(
            json!({ "violations": to_json(&t.violations(employee.as_deref(), include_cleared).map_err(e)?)? }),
        ),
        Command::LeaveRequest {
            employee,
            kind,
            from,
            to,
            half_day,
            note,
        } => to_json(
            &t.request_leave(&employee, kind, from, to, half_day, &note)
                .map_err(e)?,
        ),
        Command::LeaveDecide { id, approve, force } => {
            to_json(&t.decide_leave(id, approve, &actor, force).map_err(e)?)
        }
        Command::LeaveCancel { id } => to_json(&t.cancel_leave(id, &actor).map_err(e)?),
        Command::LeaveList {
            employee,
            year,
            status,
        } => {
            let (a, b) = match year {
                Some(y) => (
                    NaiveDate::from_ymd_opt(y, 1, 1),
                    NaiveDate::from_ymd_opt(y, 12, 31),
                ),
                None => (None, None),
            };
            Ok(
                json!({ "leaves": to_json(&t.leaves(employee.as_deref(), a, b, status).map_err(e)?)? }),
            )
        }
        Command::LeaveBalance { employee, year } => {
            let y = year.unwrap_or_else(|| today().year());
            let codes: Vec<String> = match employee {
                Some(c) => vec![c],
                None => t
                    .employees(false)
                    .map_err(e)?
                    .into_iter()
                    .map(|x| x.code)
                    .collect(),
            };
            let rows = codes
                .iter()
                .map(|c| t.balance(c, y).map_err(e))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({ "year": y, "balances": to_json(&rows)? }))
        }
        Command::LeaveAdjust {
            employee,
            year,
            half_days,
            reason,
        } => to_json(
            &t.adjust_leave(&employee, year, half_days, &reason)
                .map_err(e)?,
        ),
        Command::Holidays { year } => {
            let h = holidays::public_holidays(year).map_err(|x| x.to_string())?;
            Ok(
                json!({ "year": year, "holidays": h.iter().map(|(d, n)| json!({ "date": d, "weekday": azg::weekday_de(*d), "name": n })).collect::<Vec<_>>() }),
            )
        }
        Command::KvImport { path } => kv_import(ctx, &path),
        Command::KvList => Ok(json!({ "rows": kv_json(&t.kv_rows().map_err(e)?) })),
        Command::KvCheck { on } => kv_check(t, on.unwrap_or_else(today)),
        Command::Timesheet {
            employee,
            month,
            path,
        } => timesheet(ctx, &employee, &month, path.as_deref()),
        Command::Capacity { week, emit } => capacity(ctx, &week, emit),
        Command::DemoSeed => demo_seed(&ctx.team),
    }
}

fn time_list(
    t: &Team,
    employee: Option<&str>,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Value, String> {
    if (to - from).num_days() > 400 || to < from {
        return Err("range must be 0-400 days".into());
    }
    let entries = t.entries(employee, from, to).map_err(e)?;
    let mut totals: BTreeMap<(String, NaiveDate), i64> = BTreeMap::new();
    for x in &entries {
        *totals
            .entry((x.employee.clone(), x.start.date()))
            .or_default() += x.minutes.unwrap_or(0);
    }
    let days: Vec<Value> = totals.iter().map(|((emp, d), m)| json!({ "employee": emp, "date": d, "worked": fmt_hm(*m), "minutes": m })).collect();
    Ok(json!({ "from": from, "to": to, "entries": to_json(&entries)?, "days": days }))
}

fn spans(t: &Team, code: &str, from: NaiveDate, to: NaiveDate) -> Result<Vec<azg::Span>, String> {
    Ok(t.entries(Some(code), from, to)
        .map_err(e)?
        .into_iter()
        .filter_map(|x| {
            x.end.map(|end| azg::Span {
                start: x.start,
                end,
                break_minutes: x.break_minutes,
            })
        })
        .collect())
}

/// Run the AZG engine and record its findings.
pub fn azg_check(
    t: &Team,
    employee: Option<&str>,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Value, String> {
    if to < from || (to - from).num_days() > 400 {
        return Err("range must be 0-400 days".into());
    }
    let codes: Vec<String> = match employee {
        Some(c) => vec![t.employee(c).map_err(e)?.code],
        None => t
            .employees(false)
            .map_err(e)?
            .into_iter()
            .map(|x| x.code)
            .collect(),
    };
    let lookback = azg::monday_of(from) - Duration::weeks(azg::AVG_WEEKS);
    let mut results = Vec::new();
    let (mut violations, mut warnings) = (0, 0);
    for code in codes {
        let found = azg::check(&spans(t, &code, lookback, to)?, from, to);
        let (new, cleared) = t.record_violations(&code, from, to, &found).map_err(e)?;
        violations += found.iter().filter(|f| f.severity == "violation").count();
        warnings += found.iter().filter(|f| f.severity == "warning").count();
        let list: Vec<Value> = found.iter().map(|f| json!({ "date": f.date, "kind": f.kind, "severity": f.severity, "minutes": f.minutes, "detail": f.detail })).collect();
        results.push(json!({ "employee": code, "findings": list, "new": new, "cleared": cleared }));
    }
    Ok(
        json!({ "from": from, "to": to, "violations": violations, "warnings": warnings, "employees": results }),
    )
}

fn kv_json(rows: &[KvRow]) -> Vec<Value> {
    rows.iter().map(|r| json!({ "kv": r.kv, "group": r.group, "valid_from": r.valid_from, "monthly_min": Money::from_cents(r.monthly_min_cents).format_de(), "weekly_hours": fmt_hm(r.weekly_minutes) })).collect()
}

/// Parse `kv;group;valid_from;monthly_min;weekly_hours` (header optional, DE numbers).
pub fn parse_kv_csv(text: &str) -> Result<Vec<KvRow>, String> {
    let mut rows = Vec::new();
    for (i, raw) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with('#')
            || (i == 0 && line.to_ascii_lowercase().starts_with("kv;"))
        {
            continue;
        }
        let f: Vec<String> = line
            .split(';')
            .map(|x| x.trim().trim_matches('"').trim().to_string())
            .collect();
        if f.len() != 5 {
            return Err(format!(
                "line {}: expected 5 fields kv;group;valid_from;monthly_min;weekly_hours, got {}",
                i + 1,
                f.len()
            ));
        }
        let ctx = |m: String| format!("line {}: {m}", i + 1);
        if f[0].is_empty() || f[1].is_empty() {
            return Err(ctx("kv and group must not be empty".into()));
        }
        let valid_from = date(&f[2]).map_err(ctx)?;
        let min = Money::parse_de(&f[3]).map_err(|x| ctx(x.to_string()))?;
        if min.cents() <= 0 {
            return Err(ctx("monthly minimum must be positive".into()));
        }
        let weekly = hours_to_minutes(&Value::String(f[4].clone())).map_err(ctx)?;
        rows.push(KvRow {
            kv: f[0].clone(),
            group: f[1].clone(),
            valid_from,
            monthly_min_cents: min.cents(),
            weekly_minutes: weekly,
        });
    }
    if rows.is_empty() {
        return Err("no KV rows found".into());
    }
    Ok(rows)
}

fn kv_import(ctx: &mut Ctx, path: &str) -> Result<Value, String> {
    let p = ctx.confinement.check(path, false)?;
    let size = std::fs::metadata(&p)
        .map_err(|x| format!("{}: {x}", p.display()))?
        .len();
    if size > MAX_IMPORT_BYTES {
        return Err("KV file larger than 1 MB".into());
    }
    let text = std::fs::read_to_string(&p).map_err(|x| format!("{}: {x}", p.display()))?;
    let rows = parse_kv_csv(&text)?;
    let n = ctx.team.kv_upsert(&rows).map_err(e)?;
    Ok(json!({ "imported": n, "path": p.display().to_string() }))
}

/// Compare each employee's gross with the KV minimum, pro rata to weekly hours.
pub fn kv_check(t: &Team, on: NaiveDate) -> Result<Value, String> {
    let mut rows = Vec::new();
    let mut underpaid = 0;
    for emp in t.employees(false).map_err(e)? {
        let (Some(kv), Some(group), Some(gross)) = (
            emp.kv_name.clone(),
            emp.kv_group.clone(),
            emp.monthly_gross_cents,
        ) else {
            rows.push(json!({ "employee": emp.code, "status": "incomplete", "detail": "needs kv_name, kv_group and monthly_gross" }));
            continue;
        };
        let Some(row) = t.kv_minimum(&kv, &group, on).map_err(e)? else {
            rows.push(json!({ "employee": emp.code, "status": "no_kv_row", "detail": format!("no {kv} / {group} row valid on {on}") }));
            continue;
        };
        let num = i128::from(row.monthly_min_cents) * i128::from(emp.weekly_minutes);
        let den = i128::from(row.weekly_minutes);
        let required =
            i64::try_from((num * 2 + den) / (den * 2)).map_err(|_| "overflow".to_string())?;
        let ok = gross >= required;
        if !ok {
            underpaid += 1;
        }
        rows.push(json!({
            "employee": emp.code, "status": if ok { "ok" } else { "underpaid" },
            "kv": kv, "group": group, "valid_from": row.valid_from,
            "gross": Money::from_cents(gross).format_de(),
            "minimum_pro_rata": Money::from_cents(required).format_de(),
            "shortfall": Money::from_cents((required - gross).max(0)).format_de(),
            "hours": format!("{} of {}", fmt_hm(emp.weekly_minutes), fmt_hm(row.weekly_minutes)),
        }));
    }
    Ok(json!({ "date": on, "underpaid": underpaid, "rows": rows }))
}

fn month_bounds(month: &str) -> Result<(NaiveDate, NaiveDate), String> {
    let first = NaiveDate::parse_from_str(&format!("{}-01", month.trim()), "%Y-%m-%d")
        .map_err(|_| format!("bad month {month:?} (use YYYY-MM)"))?;
    let next = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
    };
    Ok((first, next.ok_or("bad month")? - Duration::days(1)))
}

/// Monthly working-time record (AZG section 26), with target and balance.
pub fn timesheet(
    ctx: &mut Ctx,
    employee: &str,
    month: &str,
    path: Option<&str>,
) -> Result<Value, String> {
    let t = &ctx.team;
    let emp = t.employee(employee).map_err(e)?;
    let (first, last) = month_bounds(month)?;
    let per_day = azg::days(&spans(t, &emp.code, first - Duration::days(1), last)?);
    let leaves = t
        .leaves(
            Some(&emp.code),
            Some(first),
            Some(last),
            Some(LeaveStatus::Approved),
        )
        .map_err(e)?;
    let mut flagged: BTreeMap<NaiveDate, Vec<String>> = BTreeMap::new();
    for v in t.violations(Some(&emp.code), false).map_err(e)? {
        flagged.entry(v.date).or_default().push(v.kind);
    }
    let daily_target = emp.weekly_minutes / 5;
    let (mut total, mut target) = (0i64, 0i64);
    let mut rows = Vec::new();
    let mut d = first;
    while d <= last {
        let day = per_day.get(&d).cloned().unwrap_or_default();
        let holiday = holidays::holiday_name(d).map_err(|x| x.to_string())?;
        let absence = leaves
            .iter()
            .find(|l| d >= l.from && d <= l.to)
            .map(|l| (l.kind, l.half_day));
        if holidays::is_working_day(d).map_err(|x| x.to_string())? && d >= emp.start_date {
            // Comp time draws on the overtime account, so the target stays.
            target += match absence {
                Some((LeaveKind::CompTime, _)) | None => daily_target,
                Some((_, true)) => daily_target / 2,
                Some(_) => 0,
            };
        }
        total += day.work;
        rows.push(json!({
            "date": d, "weekday": azg::weekday_de(d),
            "start": day.first_start.map(|x| x.format("%H:%M").to_string()),
            "end": day.last_end.map(|x| x.format("%H:%M").to_string()),
            "break_minutes": day.breaks, "worked": fmt_hm(day.work), "minutes": day.work,
            "absence": absence.map(|(k, half)| if half { format!("{} (half)", k.as_str()) } else { k.as_str().to_string() }),
            "holiday": holiday, "azg": flagged.get(&d).cloned().unwrap_or_default(),
        }));
        d += Duration::days(1);
    }
    let mut out = json!({
        "employee": emp.code, "name": emp.name, "month": month.trim(),
        "worked": fmt_hm(total), "target": fmt_hm(target), "balance": fmt_hm(total - target),
        "worked_minutes": total, "target_minutes": target, "rows": rows,
    });
    if let Some(p) = path {
        let file = ctx.confinement.check(p, true)?;
        let mut csv = String::from(
            "\u{feff}Datum;Tag;Beginn;Ende;Pause;Arbeitszeit;Abwesenheit;Feiertag;AZG\n",
        );
        for r in out["rows"].as_array().into_iter().flatten() {
            let cell = |k: &str| r[k].as_str().unwrap_or("").to_string();
            let date_de = r["date"]
                .as_str()
                .and_then(|x| NaiveDate::parse_from_str(x, "%Y-%m-%d").ok())
                .map(craft_biz::at::format_date_de)
                .unwrap_or_default();
            let azg_kinds = r["azg"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            csv.push_str(&format!(
                "{date_de};{};{};{};{};{};{};{};{}\n",
                cell("weekday"),
                cell("start"),
                cell("end"),
                r["break_minutes"],
                cell("worked"),
                cell("absence"),
                cell("holiday"),
                azg_kinds
            ));
        }
        csv.push_str(&format!(
            "Summe;;;;;{};Soll {};Saldo {};\n",
            fmt_hm(total),
            fmt_hm(target),
            fmt_hm(total - target)
        ));
        std::fs::write(&file, csv).map_err(|x| format!("{}: {x}", file.display()))?;
        out["written"] = json!(file.display().to_string());
    }
    Ok(out)
}

/// Available hours per employee for an ISO week: contract hours minus public
/// holidays and approved leave (5-day week assumed).
pub fn capacity(ctx: &mut Ctx, week: &str, emit: bool) -> Result<Value, String> {
    let (mon, sun) = parse_week(week).map_err(e)?;
    let t = &ctx.team;
    let wd = i64::from(holidays::working_days(mon, sun).map_err(|x| x.to_string())?);
    let mut rows = Vec::new();
    let mut total = 0i64;
    for emp in t.employees(false).map_err(e)? {
        let mut leave_hd = 0;
        for l in t
            .leaves(
                Some(&emp.code),
                Some(mon),
                Some(sun),
                Some(LeaveStatus::Approved),
            )
            .map_err(e)?
        {
            leave_hd += leave_half_days_in(&l, mon, sun).map_err(e)?;
        }
        let avail_hd = (wd * 2 - leave_hd).max(0);
        let minutes = emp.weekly_minutes * avail_hd / 10;
        total += minutes;
        rows.push(json!({ "employee": emp.code, "name": emp.name, "contract": fmt_hm(emp.weekly_minutes), "available": fmt_hm(minutes), "available_minutes": minutes, "leave_days": leave_hd as f64 / 2.0 }));
    }
    let label = week_label(mon);
    let mut out = json!({ "week": label, "from": mon, "to": sun, "working_days": wd, "total": fmt_hm(total), "total_minutes": total, "employees": rows });
    if emit {
        let ev = ctx
            .bus
            .emit(APP, "taskcraft", "capacity.changed", out.clone())
            .map_err(|x| x.to_string())?;
        out["emitted"] = json!(ev.id);
    }
    Ok(out)
}

/// Labelled demo team with deliberate AZG findings. Only on an empty database.
pub fn demo_seed(t: &Team) -> Result<Value, String> {
    if !t.employees(true).map_err(e)?.is_empty() {
        return Err("demo seed only runs on an empty database".into());
    }
    let dd = |y: i32, m: u32, d: u32| {
        NaiveDate::from_ymd_opt(y, m, d).ok_or_else(|| "bad demo date".to_string())
    };
    let dt = |x: &str| parse_datetime(x).map_err(e);
    t.set_meta("demo", "1").map_err(e)?;
    let kv = "DEMO-KV (fictional, not a real collective agreement)";
    t.kv_upsert(&[
        KvRow {
            kv: kv.into(),
            group: "A".into(),
            valid_from: dd(2026, 1, 1)?,
            monthly_min_cents: 240_000,
            weekly_minutes: 2310,
        },
        KvRow {
            kv: kv.into(),
            group: "B".into(),
            valid_from: dd(2026, 1, 1)?,
            monthly_min_cents: 310_000,
            weekly_minutes: 2310,
        },
    ])
    .map_err(e)?;
    let people = [
        (
            "JM",
            "Joe Mocky (DEMO)",
            2400,
            dd(2023, 4, 1)?,
            "B",
            340_000,
        ),
        (
            "HM",
            "Hannes Mockinger (DEMO)",
            2310,
            dd(2025, 2, 1)?,
            "A",
            255_000,
        ),
        (
            "AM",
            "Anna Mockova (DEMO)",
            1200,
            dd(2026, 9, 15)?,
            "A",
            115_000,
        ),
    ];
    for (code, name, weekly, start, group, gross) in people {
        t.add_employee(&NewEmployee {
            code: code.into(),
            name: name.into(),
            weekly_minutes: Some(weekly),
            leave_days_per_year: Some(25),
            start_date: Some(start),
            kv_name: Some(kv.into()),
            kv_group: Some(group.into()),
            monthly_gross_cents: Some(gross),
        })
        .map_err(e)?;
    }
    // Joe: week 45 with a long day and a day without break.
    for (a, b, brk) in [
        ("2026-10-27 08:00", "2026-10-27 16:30", 30),
        ("2026-10-28 08:00", "2026-10-28 16:30", 30),
        ("2026-10-29 08:00", "2026-10-29 16:30", 30),
        ("2026-10-30 08:00", "2026-10-30 14:00", 0),
        ("2026-11-02 08:00", "2026-11-02 16:30", 30),
        ("2026-11-03 07:30", "2026-11-03 19:00", 30),
        ("2026-11-04 08:00", "2026-11-04 15:00", 0),
        ("2026-11-05 08:00", "2026-11-05 16:30", 30),
        ("2026-11-06 08:00", "2026-11-06 13:00", 0),
    ] {
        t.add_time("JM", dt(a)?, dt(b)?, brk, "").map_err(e)?;
    }
    // Hannes: late shift followed by an early start (short rest); week confirmed.
    for (a, b, brk) in [
        ("2026-11-02 08:00", "2026-11-02 16:00", 30),
        ("2026-11-03 08:00", "2026-11-03 16:00", 30),
        ("2026-11-04 08:00", "2026-11-04 16:00", 30),
        ("2026-11-05 14:00", "2026-11-05 22:30", 30),
        ("2026-11-06 07:00", "2026-11-06 15:00", 30),
    ] {
        t.add_time("HM", dt(a)?, dt(b)?, brk, "").map_err(e)?;
    }
    t.confirm_week("HM", "2026-W45", dd(2026, 11, 9)?)
        .map_err(e)?;
    // Anna: part time mornings, one sick day.
    for (a, b) in [
        ("2026-11-02 09:00", "2026-11-02 13:00"),
        ("2026-11-03 09:00", "2026-11-03 13:00"),
        ("2026-11-04 09:00", "2026-11-04 13:00"),
        ("2026-11-05 09:00", "2026-11-05 13:00"),
    ] {
        t.add_time("AM", dt(a)?, dt(b)?, 0, "").map_err(e)?;
    }
    t.request_leave(
        "AM",
        LeaveKind::Sick,
        dd(2026, 11, 6)?,
        dd(2026, 11, 6)?,
        false,
        "",
    )
    .map_err(e)?;
    let hv = t
        .request_leave(
            "HM",
            LeaveKind::Vacation,
            dd(2026, 10, 27)?,
            dd(2026, 10, 30)?,
            false,
            "Herbstferien",
        )
        .map_err(e)?;
    t.decide_leave(hv.id, true, "demo", false).map_err(e)?;
    t.request_leave(
        "JM",
        LeaveKind::Vacation,
        dd(2026, 12, 21)?,
        dd(2026, 12, 31)?,
        false,
        "Weihnachten",
    )
    .map_err(e)?;
    t.request_leave(
        "HM",
        LeaveKind::CompTime,
        dd(2026, 11, 13)?,
        dd(2026, 11, 13)?,
        true,
        "Zeitausgleich",
    )
    .map_err(e)?;
    let check = azg_check(t, None, dd(2026, 10, 26)?, dd(2026, 11, 8)?)?;
    Ok(
        json!({ "demo": true, "stats": to_json(&t.stats().map_err(e)?)?, "azg": { "violations": check["violations"], "warnings": check["warnings"] } }),
    )
}

/// MCP tool catalog.
pub fn tools() -> Vec<Value> {
    let emp = json!({ "type": "string", "description": "Employee code, e.g. JM" });
    let date = json!({ "type": "string", "description": "DD.MM.YYYY or YYYY-MM-DD" });
    let dtm = json!({ "type": "string", "description": "YYYY-MM-DD HH:MM (default: now)" });
    let kind =
        json!({ "type": "string", "enum": ["vacation", "sick", "comp_time", "unpaid", "other"] });
    let status =
        json!({ "type": "string", "enum": ["requested", "approved", "rejected", "cancelled"] });
    let hours = json!({ "type": ["number", "string"], "description": "weekly hours, e.g. 38.5 or \"38,5\"" });
    let money =
        json!({ "type": ["string", "null"], "description": "DE amount, e.g. \"2.850,00\"" });
    vec![
        tool("team_status", "Counts (active employees, clocked in, open leave requests, active AZG findings) and who is clocked in now.", json!({}), &[]),
        tool("team_employee_add", "Add an employee. Only work-relevant fields; no SV number, address or bank data.", json!({ "code": emp, "name": {"type":"string"}, "weekly_hours": hours, "leave_days": {"type":"integer","description":"per year, default 25 (Urlaubsgesetz)"}, "start_date": date, "kv_name": {"type":"string"}, "kv_group": {"type":"string"}, "monthly_gross": money }), &["code", "name"]),
        tool("team_employee_list", "List employees.", json!({ "include_inactive": {"type":"boolean"} }), &[]),
        tool("team_employee_update", "Change employee fields; null clears kv_name/kv_group/monthly_gross. active=false retires an employee.", json!({ "code": emp, "name": {"type":"string"}, "weekly_hours": hours, "leave_days": {"type":"integer"}, "kv_name": {"type":["string","null"]}, "kv_group": {"type":["string","null"]}, "monthly_gross": money, "active": {"type":"boolean"} }), &["code"]),
        tool("team_clock_in", "Start a time entry (one open entry per employee).", json!({ "employee": emp, "at": dtm, "note": {"type":"string"} }), &["employee"]),
        tool("team_clock_out", "Close the open entry with break minutes.", json!({ "employee": emp, "at": dtm, "break_minutes": {"type":"integer"} }), &["employee"]),
        tool("team_time_add", "Record a past entry. start/end as HH:MM with date, or full date-times. An end before the start on the same date means a night shift.", json!({ "employee": emp, "date": date, "start": {"type":"string"}, "end": {"type":"string"}, "break_minutes": {"type":"integer"}, "note": {"type":"string"} }), &["employee", "start", "end"]),
        tool("team_time_list", "Entries and daily totals in a date range.", json!({ "employee": emp, "from": date, "to": date }), &["from", "to"]),
        tool("team_time_delete", "Delete an entry (refused in confirmed weeks).", json!({ "id": {"type":"integer"} }), &["id"]),
        tool("team_week_confirm", "Employee confirms an ISO week (2026-W45). Its entries become read-only.", json!({ "employee": emp, "week": {"type":"string"} }), &["employee", "week"]),
        tool("team_azg_check", "Check working-time law (10h/12h daily, 60h weekly, 48h 17-week average, 30 min break above 6h, 11h rest). Findings are recorded, never auto-corrected.", json!({ "employee": emp, "from": date, "to": date }), &["from", "to"]),
        tool("team_violations", "Recorded AZG findings; cleared ones only with include_cleared.", json!({ "employee": emp, "include_cleared": {"type":"boolean"} }), &[]),
        tool("team_leave_request", "Request leave. Days counted Mon-Fri minus Austrian public holidays. Sick leave is recorded directly and takes no note (no health data).", json!({ "employee": emp, "kind": kind, "from": date, "to": date, "half_day": {"type":"boolean"}, "note": {"type":"string"} }), &["employee", "kind", "from"]),
        tool("team_leave_decide", "Approve or reject a request. Approving vacation beyond the balance needs force=true.", json!({ "id": {"type":"integer"}, "approve": {"type":"boolean"}, "force": {"type":"boolean"} }), &["id", "approve"]),
        tool("team_leave_cancel", "Cancel a requested or approved leave.", json!({ "id": {"type":"integer"} }), &["id"]),
        tool("team_leave_list", "Leaves, optionally by employee, year and status.", json!({ "employee": emp, "year": {"type":"integer"}, "status": status }), &[]),
        tool("team_leave_balance", "Vacation entitlement (pro rata in the start year), taken, pending, remaining; sick/comp/unpaid day counts.", json!({ "employee": emp, "year": {"type":"integer"} }), &[]),
        tool("team_leave_adjust", "Add or subtract vacation days for a year (carry-over etc.), with a reason.", json!({ "employee": emp, "year": {"type":"integer"}, "days": {"type":["number","string"]}, "reason": {"type":"string"} }), &["employee", "year", "days", "reason"]),
        tool("team_holidays", "Austrian national public holidays of a year.", json!({ "year": {"type":"integer"} }), &[]),
        tool("team_kv_import", "Import KV minimum wages from CSV kv;group;valid_from;monthly_min;weekly_hours. The table is yours: no KV figures ship with the app.", json!({ "path": {"type":"string"} }), &["path"]),
        tool("team_kv_list", "KV minimum table.", json!({}), &[]),
        tool("team_kv_check", "Compare each employee's gross with the KV minimum valid on a date, pro rata to weekly hours.", json!({ "date": date }), &[]),
        tool("team_timesheet", "Monthly working-time record (AZG section 26): per day start/end/break/worked/absence/holiday/AZG flags, worked vs target. path writes a semicolon CSV.", json!({ "employee": emp, "month": {"type":"string","description":"YYYY-MM"}, "path": {"type":"string"} }), &["employee", "month"]),
        tool("team_capacity", "Available hours per employee for an ISO week after holidays and approved leave. emit=true sends capacity.changed to taskcraft.", json!({ "week": {"type":"string"}, "emit": {"type":"boolean"} }), &[]),
        tool("team_demo_seed", "Fill an EMPTY database with a labelled demo team, fictional KV table and deliberate AZG findings.", json!({}), &[]),
    ]
}

#[cfg(test)]
mod tests;
