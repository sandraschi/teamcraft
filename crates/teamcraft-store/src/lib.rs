//! teamcraft-store: working time and leave for a small Austrian team.
//!
//! Invariants:
//! - One open (clocked-in) entry per employee; entries of one employee never overlap.
//! - A confirmed week is locked: its entries cannot be added, changed or deleted.
//!   Enforced in Rust and again by SQLite triggers.
//! - Sick leave stores day counts only. No reason, no note (health data).
//! - Leave is counted in working days (Mon-Fri minus Austrian public holidays),
//!   internally in half-days so no floats touch the balance.
//! - AZG findings are recorded, never auto-corrected; a finding that no longer
//!   applies is marked cleared, not deleted.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Timelike, Utc, Weekday};
use craft_biz::holidays;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::Serialize;
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 1;
/// Longest single time entry accepted.
pub const MAX_ENTRY_MINUTES: i64 = 24 * 60;

#[derive(Debug, thiserror::Error)]
pub enum TeamError {
    #[error("database: {0}")]
    Db(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("locked: {0}")]
    Locked(String),
    #[error("conflict: {0}")]
    Conflict(String),
}

impl From<rusqlite::Error> for TeamError {
    fn from(e: rusqlite::Error) -> Self {
        let s = e.to_string();
        if s.contains("week is confirmed") {
            TeamError::Locked(s)
        } else {
            TeamError::Db(s)
        }
    }
}

impl From<craft_biz::BizError> for TeamError {
    fn from(e: craft_biz::BizError) -> Self {
        TeamError::Invalid(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, TeamError>;

macro_rules! str_enum {
    ($name:ident { $($var:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($var),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];
            pub fn as_str(self) -> &'static str { match self { $($name::$var => $s),+ } }
            pub fn parse(s: &str) -> Result<$name> {
                let t = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
                $name::ALL.iter().copied().find(|v| v.as_str() == t).ok_or_else(|| {
                    TeamError::Invalid(format!("unknown {} {s:?} (one of: {})", stringify!($name).to_lowercase(),
                        $name::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>().join(", ")))
                })
            }
        }
    };
}

str_enum!(LeaveKind { Vacation => "vacation", Sick => "sick", CompTime => "comp_time", Unpaid => "unpaid", Other => "other" });
str_enum!(LeaveStatus { Requested => "requested", Approved => "approved", Rejected => "rejected", Cancelled => "cancelled" });

// ---------- time helpers ----------

/// Accepts `YYYY-MM-DD HH:MM`, `YYYY-MM-DDTHH:MM`, optional `:SS`, or `DD.MM.YYYY HH:MM`.
/// Seconds are dropped: the record has minute precision.
pub fn parse_datetime(s: &str) -> Result<NaiveDateTime> {
    let t = s.trim();
    for f in [
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%d.%m.%Y %H:%M",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(t, f) {
            return Ok(minute(dt));
        }
    }
    Err(TeamError::Invalid(format!(
        "bad date-time {s:?} (use YYYY-MM-DD HH:MM)"
    )))
}

pub fn minute(dt: NaiveDateTime) -> NaiveDateTime {
    dt.with_second(0)
        .and_then(|d| d.with_nanosecond(0))
        .unwrap_or(dt)
}

pub fn fmt_hm(minutes: i64) -> String {
    let sign = if minutes < 0 { "-" } else { "" };
    let m = minutes.abs();
    format!("{sign}{}:{:02}", m / 60, m % 60)
}

/// `2026-W46`
pub fn week_label(d: NaiveDate) -> String {
    let w = d.iso_week();
    format!("{}-W{:02}", w.year(), w.week())
}

/// Monday and Sunday of an ISO week given as `2026-W46` or `2026-46`.
pub fn parse_week(s: &str) -> Result<(NaiveDate, NaiveDate)> {
    let t = s.trim().to_ascii_uppercase();
    let (y, w) = t
        .split_once("-W")
        .or_else(|| t.split_once('-'))
        .ok_or_else(|| TeamError::Invalid(format!("bad week {s:?} (use 2026-W46)")))?;
    let y: i32 = y
        .parse()
        .map_err(|_| TeamError::Invalid(format!("bad week year in {s:?}")))?;
    let w: u32 = w
        .parse()
        .map_err(|_| TeamError::Invalid(format!("bad week number in {s:?}")))?;
    let mon = NaiveDate::from_isoywd_opt(y, w, Weekday::Mon)
        .ok_or_else(|| TeamError::Invalid(format!("week {s:?} does not exist")))?;
    Ok((mon, mon + Duration::days(6)))
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn days(half_days: i64) -> f64 {
    half_days as f64 / 2.0
}

// ---------- records ----------

#[derive(Debug, Clone, Serialize)]
pub struct Employee {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub weekly_minutes: i64,
    pub leave_days_per_year: i64,
    pub start_date: NaiveDate,
    pub kv_name: Option<String>,
    pub kv_group: Option<String>,
    pub monthly_gross_cents: Option<i64>,
    pub active: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NewEmployee {
    pub code: String,
    pub name: String,
    pub weekly_minutes: Option<i64>,
    pub leave_days_per_year: Option<i64>,
    pub start_date: Option<NaiveDate>,
    pub kv_name: Option<String>,
    pub kv_group: Option<String>,
    pub monthly_gross_cents: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct EmployeePatch {
    pub name: Option<String>,
    pub weekly_minutes: Option<i64>,
    pub leave_days_per_year: Option<i64>,
    pub kv_name: Option<Option<String>>,
    pub kv_group: Option<Option<String>>,
    pub monthly_gross_cents: Option<Option<i64>>,
    pub active: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimeEntry {
    pub id: i64,
    pub employee: String,
    pub start: NaiveDateTime,
    pub end: Option<NaiveDateTime>,
    pub break_minutes: i64,
    /// Worked minutes (end - start - break); None while clocked in.
    pub minutes: Option<i64>,
    pub source: String,
    pub note: String,
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Leave {
    pub id: i64,
    pub employee: String,
    pub kind: LeaveKind,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub half_day: bool,
    pub half_days: i64,
    pub days: f64,
    pub status: LeaveStatus,
    pub note: String,
    pub requested_at: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LeaveBalance {
    pub employee: String,
    pub year: i32,
    pub entitlement_days: f64,
    pub adjustment_days: f64,
    pub taken_days: f64,
    pub pending_days: f64,
    pub remaining_days: f64,
    pub sick_days: f64,
    pub comp_time_days: f64,
    pub unpaid_days: f64,
    #[serde(skip)]
    pub remaining_half_days: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct KvRow {
    pub kv: String,
    pub group: String,
    pub valid_from: NaiveDate,
    pub monthly_min_cents: i64,
    pub weekly_minutes: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    pub id: i64,
    pub employee: String,
    pub date: NaiveDate,
    pub kind: String,
    pub severity: String,
    pub minutes: i64,
    pub detail: String,
    pub detected_at: String,
    pub cleared_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewViolation {
    pub date: NaiveDate,
    pub kind: String,
    pub severity: String,
    pub minutes: i64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub employees_active: i64,
    pub clocked_in: i64,
    pub leave_requests_open: i64,
    pub violations_active: i64,
    pub kv_rows: i64,
    pub demo: bool,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS employees (
    id INTEGER PRIMARY KEY,
    code TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    weekly_minutes INTEGER NOT NULL CHECK (weekly_minutes BETWEEN 1 AND 3600),
    leave_days INTEGER NOT NULL CHECK (leave_days BETWEEN 0 AND 60),
    start_date TEXT NOT NULL,
    kv_name TEXT,
    kv_group TEXT,
    monthly_gross_cents INTEGER CHECK (monthly_gross_cents IS NULL OR monthly_gross_cents >= 0),
    active INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS time_entries (
    id INTEGER PRIMARY KEY,
    employee_id INTEGER NOT NULL REFERENCES employees(id),
    start_at TEXT NOT NULL,
    end_at TEXT,
    break_minutes INTEGER NOT NULL DEFAULT 0 CHECK (break_minutes >= 0),
    iso_year INTEGER NOT NULL,
    iso_week INTEGER NOT NULL,
    source TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_time_emp_start ON time_entries(employee_id, start_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_time_one_open ON time_entries(employee_id) WHERE end_at IS NULL;
CREATE TABLE IF NOT EXISTS week_locks (
    employee_id INTEGER NOT NULL REFERENCES employees(id),
    iso_year INTEGER NOT NULL,
    iso_week INTEGER NOT NULL,
    confirmed_at TEXT NOT NULL,
    PRIMARY KEY (employee_id, iso_year, iso_week)
);
CREATE TRIGGER IF NOT EXISTS trg_time_locked_ins BEFORE INSERT ON time_entries
WHEN EXISTS (SELECT 1 FROM week_locks l WHERE l.employee_id = NEW.employee_id AND l.iso_year = NEW.iso_year AND l.iso_week = NEW.iso_week)
BEGIN SELECT RAISE(ABORT, 'week is confirmed and locked'); END;
CREATE TRIGGER IF NOT EXISTS trg_time_locked_upd BEFORE UPDATE ON time_entries
WHEN EXISTS (SELECT 1 FROM week_locks l WHERE l.employee_id = OLD.employee_id AND l.iso_year = OLD.iso_year AND l.iso_week = OLD.iso_week)
BEGIN SELECT RAISE(ABORT, 'week is confirmed and locked'); END;
CREATE TRIGGER IF NOT EXISTS trg_time_locked_del BEFORE DELETE ON time_entries
WHEN EXISTS (SELECT 1 FROM week_locks l WHERE l.employee_id = OLD.employee_id AND l.iso_year = OLD.iso_year AND l.iso_week = OLD.iso_week)
BEGIN SELECT RAISE(ABORT, 'week is confirmed and locked'); END;
CREATE TABLE IF NOT EXISTS leaves (
    id INTEGER PRIMARY KEY,
    employee_id INTEGER NOT NULL REFERENCES employees(id),
    kind TEXT NOT NULL,
    from_date TEXT NOT NULL,
    to_date TEXT NOT NULL,
    half_day INTEGER NOT NULL DEFAULT 0,
    half_days INTEGER NOT NULL CHECK (half_days > 0),
    status TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    requested_at TEXT NOT NULL,
    decided_by TEXT,
    decided_at TEXT,
    CHECK (to_date >= from_date),
    CHECK (kind <> 'sick' OR note = '')
);
CREATE INDEX IF NOT EXISTS idx_leaves_emp ON leaves(employee_id, from_date);
CREATE TABLE IF NOT EXISTS leave_adjustments (
    id INTEGER PRIMARY KEY,
    employee_id INTEGER NOT NULL REFERENCES employees(id),
    year INTEGER NOT NULL,
    half_days INTEGER NOT NULL,
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS kv_minimums (
    kv TEXT NOT NULL,
    grp TEXT NOT NULL,
    valid_from TEXT NOT NULL,
    monthly_min_cents INTEGER NOT NULL CHECK (monthly_min_cents > 0),
    weekly_minutes INTEGER NOT NULL CHECK (weekly_minutes BETWEEN 60 AND 3600),
    PRIMARY KEY (kv, grp, valid_from)
);
CREATE TABLE IF NOT EXISTS violations (
    id INTEGER PRIMARY KEY,
    employee_id INTEGER NOT NULL REFERENCES employees(id),
    date TEXT NOT NULL,
    kind TEXT NOT NULL,
    severity TEXT NOT NULL,
    minutes INTEGER NOT NULL,
    detail TEXT NOT NULL,
    detected_at TEXT NOT NULL,
    cleared_at TEXT,
    UNIQUE (employee_id, date, kind)
);
";

const EMP_COLS: &str = "id, code, name, weekly_minutes, leave_days, start_date, kv_name, kv_group, monthly_gross_cents, active";
const TIME_SELECT: &str = "SELECT t.id, e.code, t.start_at, t.end_at, t.break_minutes, t.source, t.note,
    EXISTS (SELECT 1 FROM week_locks l WHERE l.employee_id = t.employee_id AND l.iso_year = t.iso_year AND l.iso_week = t.iso_week)
    FROM time_entries t JOIN employees e ON e.id = t.employee_id";
const LEAVE_SELECT: &str = "SELECT l.id, e.code, l.kind, l.from_date, l.to_date, l.half_day, l.half_days, l.status, l.note, l.requested_at, l.decided_by, l.decided_at
    FROM leaves l JOIN employees e ON e.id = l.employee_id";

fn emp_row(r: &Row) -> rusqlite::Result<Employee> {
    Ok(Employee {
        id: r.get(0)?,
        code: r.get(1)?,
        name: r.get(2)?,
        weekly_minutes: r.get(3)?,
        leave_days_per_year: r.get(4)?,
        start_date: r.get(5)?,
        kv_name: r.get(6)?,
        kv_group: r.get(7)?,
        monthly_gross_cents: r.get(8)?,
        active: r.get(9)?,
    })
}

fn time_row(r: &Row) -> rusqlite::Result<TimeEntry> {
    let start: NaiveDateTime = r.get(2)?;
    let end: Option<NaiveDateTime> = r.get(3)?;
    let brk: i64 = r.get(4)?;
    Ok(TimeEntry {
        id: r.get(0)?,
        employee: r.get(1)?,
        start,
        end,
        break_minutes: brk,
        minutes: end.map(|e| (e - start).num_minutes() - brk),
        source: r.get(5)?,
        note: r.get(6)?,
        locked: r.get(7)?,
    })
}

/// Raw row for leaves; enums are parsed afterwards so bad text surfaces as an error, not a panic.
type LeaveRaw = (
    i64,
    String,
    String,
    NaiveDate,
    NaiveDate,
    bool,
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn leave_raw(r: &Row) -> rusqlite::Result<LeaveRaw> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
    ))
}

fn leave_from(raw: LeaveRaw) -> Result<Leave> {
    let (
        id,
        employee,
        kind,
        from,
        to,
        half_day,
        half_days,
        status,
        note,
        requested_at,
        decided_by,
        decided_at,
    ) = raw;
    Ok(Leave {
        id,
        employee,
        kind: LeaveKind::parse(&kind)?,
        from,
        to,
        half_day,
        half_days,
        days: days(half_days),
        status: LeaveStatus::parse(&status)?,
        note,
        requested_at,
        decided_by,
        decided_at,
    })
}

fn validate_code(code: &str) -> Result<String> {
    let c = code.trim().to_ascii_uppercase();
    if c.is_empty() || c.len() > 8 || !c.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return Err(TeamError::Invalid(format!(
            "employee code {code:?} must be 1-8 letters or digits"
        )));
    }
    Ok(c)
}

fn clean_opt(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Half-days of a leave that fall inside `a..=b` (working days only).
pub fn leave_half_days_in(l: &Leave, a: NaiveDate, b: NaiveDate) -> Result<i64> {
    if l.half_day {
        return Ok(if l.from >= a && l.from <= b { 1 } else { 0 });
    }
    let lo = l.from.max(a);
    let hi = l.to.min(b);
    if hi < lo {
        return Ok(0);
    }
    Ok(i64::from(holidays::working_days(lo, hi)?) * 2)
}

pub struct Team {
    conn: Connection,
}

impl Team {
    pub fn open(path: &Path) -> Result<Team> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| TeamError::Db(e.to_string()))?;
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Team> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Team> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        conn.execute_batch(SCHEMA)?;
        let v: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match v.as_deref() {
            None => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                    params![SCHEMA_VERSION.to_string()],
                )?;
            }
            Some(x) if x == SCHEMA_VERSION.to_string() => {}
            Some(x) => {
                return Err(TeamError::Db(format!(
                    "database schema {x} is newer than this build ({SCHEMA_VERSION})"
                )))
            }
        }
        Ok(Team { conn })
    }

    /// Raw connection, for tests that check the SQL-level guards.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
        Ok(())
    }

    pub fn is_demo(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'demo'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .as_deref()
            == Some("1"))
    }

    // ---------- employees ----------

    pub fn add_employee(&self, n: &NewEmployee) -> Result<Employee> {
        let code = validate_code(&n.code)?;
        let name = n.name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err(TeamError::Invalid("name must be 1-120 characters".into()));
        }
        let weekly = n.weekly_minutes.unwrap_or(40 * 60);
        if !(1..=3600).contains(&weekly) {
            return Err(TeamError::Invalid(
                "weekly hours must be above 0 and at most 60".into(),
            ));
        }
        let leave = n.leave_days_per_year.unwrap_or(25);
        if !(0..=60).contains(&leave) {
            return Err(TeamError::Invalid(
                "leave days per year must be 0-60".into(),
            ));
        }
        if n.monthly_gross_cents.is_some_and(|g| g < 0) {
            return Err(TeamError::Invalid(
                "monthly gross cannot be negative".into(),
            ));
        }
        if self.employee(&code).is_ok() {
            return Err(TeamError::Conflict(format!("employee {code} exists")));
        }
        let start = n.start_date.unwrap_or_else(|| Utc::now().date_naive());
        self.conn.execute(
            "INSERT INTO employees (code, name, weekly_minutes, leave_days, start_date, kv_name, kv_group, monthly_gross_cents, active) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)",
            params![code, name, weekly, leave, start, clean_opt(n.kv_name.clone()), clean_opt(n.kv_group.clone()), n.monthly_gross_cents],
        )?;
        self.employee(&code)
    }

    pub fn employee(&self, code: &str) -> Result<Employee> {
        let c = code.trim().to_ascii_uppercase();
        self.conn
            .query_row(
                &format!("SELECT {EMP_COLS} FROM employees WHERE code = ?1"),
                params![c],
                emp_row,
            )
            .optional()?
            .ok_or_else(|| TeamError::NotFound(format!("employee {code}")))
    }

    pub fn employees(&self, include_inactive: bool) -> Result<Vec<Employee>> {
        let sql = format!(
            "SELECT {EMP_COLS} FROM employees {} ORDER BY code",
            if include_inactive {
                ""
            } else {
                "WHERE active = 1"
            }
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st
            .query_map([], emp_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn update_employee(&self, code: &str, p: &EmployeePatch) -> Result<Employee> {
        let e = self.employee(code)?;
        let name = p
            .name
            .as_deref()
            .map(str::trim)
            .unwrap_or(&e.name)
            .to_string();
        if name.is_empty() || name.chars().count() > 120 {
            return Err(TeamError::Invalid("name must be 1-120 characters".into()));
        }
        let weekly = p.weekly_minutes.unwrap_or(e.weekly_minutes);
        if !(1..=3600).contains(&weekly) {
            return Err(TeamError::Invalid(
                "weekly hours must be above 0 and at most 60".into(),
            ));
        }
        let leave = p.leave_days_per_year.unwrap_or(e.leave_days_per_year);
        if !(0..=60).contains(&leave) {
            return Err(TeamError::Invalid(
                "leave days per year must be 0-60".into(),
            ));
        }
        let gross = p.monthly_gross_cents.unwrap_or(e.monthly_gross_cents);
        if gross.is_some_and(|g| g < 0) {
            return Err(TeamError::Invalid(
                "monthly gross cannot be negative".into(),
            ));
        }
        let kv_name = p.kv_name.clone().map(clean_opt).unwrap_or(e.kv_name);
        let kv_group = p.kv_group.clone().map(clean_opt).unwrap_or(e.kv_group);
        let active = p.active.unwrap_or(e.active);
        self.conn.execute(
            "UPDATE employees SET name = ?2, weekly_minutes = ?3, leave_days = ?4, kv_name = ?5, kv_group = ?6, monthly_gross_cents = ?7, active = ?8 WHERE id = ?1",
            params![e.id, name, weekly, leave, kv_name, kv_group, gross, active],
        )?;
        self.employee(&e.code)
    }

    fn active_employee(&self, code: &str) -> Result<Employee> {
        let e = self.employee(code)?;
        if !e.active {
            return Err(TeamError::Invalid(format!(
                "employee {} is inactive",
                e.code
            )));
        }
        Ok(e)
    }

    // ---------- time ----------

    pub fn week_locked(&self, employee_id: i64, d: NaiveDate) -> Result<bool> {
        let w = d.iso_week();
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM week_locks WHERE employee_id = ?1 AND iso_year = ?2 AND iso_week = ?3)",
            params![employee_id, w.year(), w.week()],
            |r| r.get(0),
        )?)
    }

    fn ensure_unlocked(&self, e: &Employee, d: NaiveDate) -> Result<()> {
        if self.week_locked(e.id, d)? {
            return Err(TeamError::Locked(format!(
                "{} week {} is confirmed and locked",
                e.code,
                week_label(d)
            )));
        }
        Ok(())
    }

    fn overlapping(
        &self,
        employee_id: i64,
        start: NaiveDateTime,
        end: NaiveDateTime,
        exclude: i64,
    ) -> Result<Option<TimeEntry>> {
        let sql = format!("{TIME_SELECT} WHERE t.employee_id = ?1 AND t.id <> ?4 AND t.start_at < ?3 AND COALESCE(t.end_at, '9999-12-31 00:00:00') > ?2 LIMIT 1");
        Ok(self
            .conn
            .query_row(&sql, params![employee_id, start, end, exclude], time_row)
            .optional()?)
    }

    fn open_entry(&self, employee_id: i64) -> Result<Option<TimeEntry>> {
        let sql = format!("{TIME_SELECT} WHERE t.employee_id = ?1 AND t.end_at IS NULL");
        Ok(self
            .conn
            .query_row(&sql, params![employee_id], time_row)
            .optional()?)
    }

    pub fn entry(&self, id: i64) -> Result<TimeEntry> {
        let sql = format!("{TIME_SELECT} WHERE t.id = ?1");
        self.conn
            .query_row(&sql, params![id], time_row)
            .optional()?
            .ok_or_else(|| TeamError::NotFound(format!("time entry {id}")))
    }

    fn insert_entry(
        &self,
        e: &Employee,
        start: NaiveDateTime,
        end: Option<NaiveDateTime>,
        brk: i64,
        source: &str,
        note: &str,
    ) -> Result<TimeEntry> {
        let w = start.date().iso_week();
        self.conn.execute(
            "INSERT INTO time_entries (employee_id, start_at, end_at, break_minutes, iso_year, iso_week, source, note, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![e.id, start, end, brk, w.year(), w.week(), source, note.trim(), now()],
        )?;
        self.entry(self.conn.last_insert_rowid())
    }

    fn check_span(start: NaiveDateTime, end: NaiveDateTime, brk: i64) -> Result<()> {
        let dur = (end - start).num_minutes();
        if dur <= 0 {
            return Err(TeamError::Invalid("end must be after start".into()));
        }
        if dur > MAX_ENTRY_MINUTES {
            return Err(TeamError::Invalid(
                "an entry may not exceed 24 hours".into(),
            ));
        }
        if brk < 0 || brk >= dur {
            return Err(TeamError::Invalid(format!(
                "break of {brk} min does not fit a {dur} min entry"
            )));
        }
        Ok(())
    }

    pub fn clock_in(&self, code: &str, at: NaiveDateTime, note: &str) -> Result<TimeEntry> {
        let e = self.active_employee(code)?;
        let at = minute(at);
        self.ensure_unlocked(&e, at.date())?;
        if let Some(open) = self.open_entry(e.id)? {
            return Err(TeamError::Conflict(format!(
                "{} is already clocked in since {}",
                e.code, open.start
            )));
        }
        if let Some(o) = self.overlapping(e.id, at, at + Duration::minutes(1), 0)? {
            return Err(TeamError::Conflict(format!(
                "{at} falls inside entry {} ({} - {:?})",
                o.id, o.start, o.end
            )));
        }
        self.insert_entry(&e, at, None, 0, "clock", note)
    }

    pub fn clock_out(
        &self,
        code: &str,
        at: NaiveDateTime,
        break_minutes: i64,
    ) -> Result<TimeEntry> {
        let e = self.employee(code)?;
        let at = minute(at);
        let open = self
            .open_entry(e.id)?
            .ok_or_else(|| TeamError::Invalid(format!("{} is not clocked in", e.code)))?;
        Self::check_span(open.start, at, break_minutes)?;
        if let Some(o) = self.overlapping(e.id, open.start, at, open.id)? {
            return Err(TeamError::Conflict(format!(
                "would overlap entry {} starting {}",
                o.id, o.start
            )));
        }
        self.conn.execute(
            "UPDATE time_entries SET end_at = ?2, break_minutes = ?3 WHERE id = ?1",
            params![open.id, at, break_minutes],
        )?;
        self.entry(open.id)
    }

    pub fn add_time(
        &self,
        code: &str,
        start: NaiveDateTime,
        end: NaiveDateTime,
        break_minutes: i64,
        note: &str,
    ) -> Result<TimeEntry> {
        let e = self.active_employee(code)?;
        let (start, end) = (minute(start), minute(end));
        Self::check_span(start, end, break_minutes)?;
        self.ensure_unlocked(&e, start.date())?;
        if let Some(o) = self.overlapping(e.id, start, end, 0)? {
            return Err(TeamError::Conflict(format!(
                "overlaps entry {} ({} - {:?})",
                o.id, o.start, o.end
            )));
        }
        self.insert_entry(&e, start, Some(end), break_minutes, "manual", note)
    }

    pub fn delete_time(&self, id: i64) -> Result<TimeEntry> {
        let t = self.entry(id)?;
        if t.locked {
            return Err(TeamError::Locked(format!(
                "entry {id} is in a confirmed week"
            )));
        }
        self.conn
            .execute("DELETE FROM time_entries WHERE id = ?1", params![id])?;
        Ok(t)
    }

    /// Entries whose start falls in `from..=to`, oldest first.
    pub fn entries(
        &self,
        code: Option<&str>,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<TimeEntry>> {
        let lo = from
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| TeamError::Invalid("bad from".into()))?;
        let hi = (to + Duration::days(1))
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| TeamError::Invalid("bad to".into()))?;
        let mut sql = format!("{TIME_SELECT} WHERE t.start_at >= ?1 AND t.start_at < ?2");
        let mut args: Vec<SqlValue> = vec![
            SqlValue::Text(lo.format("%Y-%m-%d %H:%M:%S").to_string()),
            SqlValue::Text(hi.format("%Y-%m-%d %H:%M:%S").to_string()),
        ];
        if let Some(c) = code {
            sql.push_str(" AND e.code = ?3");
            args.push(SqlValue::Text(self.employee(c)?.code));
        }
        sql.push_str(" ORDER BY e.code, t.start_at");
        let mut st = self.conn.prepare(&sql)?;
        let rows = st
            .query_map(params_from_iter(args.iter()), time_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Every open entry (clocked in, not out), however old.
    pub fn open_entries(&self) -> Result<Vec<TimeEntry>> {
        let sql = format!("{TIME_SELECT} WHERE t.end_at IS NULL ORDER BY e.code");
        let mut st = self.conn.prepare(&sql)?;
        let rows = st
            .query_map([], time_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Employee confirms a week; afterwards its entries are read-only.
    pub fn confirm_week(
        &self,
        code: &str,
        week: &str,
        today: NaiveDate,
    ) -> Result<(NaiveDate, usize)> {
        let e = self.employee(code)?;
        let (mon, sun) = parse_week(week)?;
        if mon > today {
            return Err(TeamError::Invalid(format!(
                "week {week} has not started yet"
            )));
        }
        let entries = self.entries(Some(&e.code), mon, sun)?;
        if let Some(open) = entries.iter().find(|t| t.end.is_none()) {
            return Err(TeamError::Invalid(format!(
                "entry {} is still open; clock out first",
                open.id
            )));
        }
        if self.week_locked(e.id, mon)? {
            return Err(TeamError::Conflict(format!(
                "{} week {} is already confirmed",
                e.code,
                week_label(mon)
            )));
        }
        let w = mon.iso_week();
        self.conn.execute("INSERT INTO week_locks (employee_id, iso_year, iso_week, confirmed_at) VALUES (?1, ?2, ?3, ?4)", params![e.id, w.year(), w.week(), now()])?;
        Ok((mon, entries.len()))
    }

    // ---------- leave ----------

    pub fn leave(&self, id: i64) -> Result<Leave> {
        let sql = format!("{LEAVE_SELECT} WHERE l.id = ?1");
        let raw = self
            .conn
            .query_row(&sql, params![id], leave_raw)
            .optional()?
            .ok_or_else(|| TeamError::NotFound(format!("leave {id}")))?;
        leave_from(raw)
    }

    pub fn leaves(
        &self,
        code: Option<&str>,
        from: Option<NaiveDate>,
        to: Option<NaiveDate>,
        status: Option<LeaveStatus>,
    ) -> Result<Vec<Leave>> {
        let mut sql = format!("{LEAVE_SELECT} WHERE 1 = 1");
        let mut args: Vec<SqlValue> = Vec::new();
        if let Some(c) = code {
            args.push(SqlValue::Text(self.employee(c)?.code));
            sql.push_str(&format!(" AND e.code = ?{}", args.len()));
        }
        if let Some(f) = from {
            args.push(SqlValue::Text(f.to_string()));
            sql.push_str(&format!(" AND l.to_date >= ?{}", args.len()));
        }
        if let Some(t) = to {
            args.push(SqlValue::Text(t.to_string()));
            sql.push_str(&format!(" AND l.from_date <= ?{}", args.len()));
        }
        if let Some(s) = status {
            args.push(SqlValue::Text(s.as_str().into()));
            sql.push_str(&format!(" AND l.status = ?{}", args.len()));
        }
        sql.push_str(" ORDER BY l.from_date, e.code");
        let mut st = self.conn.prepare(&sql)?;
        let raws = st
            .query_map(params_from_iter(args.iter()), leave_raw)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter().map(leave_from).collect()
    }

    pub fn request_leave(
        &self,
        code: &str,
        kind: LeaveKind,
        from: NaiveDate,
        to: NaiveDate,
        half_day: bool,
        note: &str,
    ) -> Result<Leave> {
        let e = self.active_employee(code)?;
        if to < from {
            return Err(TeamError::Invalid("to is before from".into()));
        }
        if (to - from).num_days() > 366 {
            return Err(TeamError::Invalid(
                "a single leave may span at most one year".into(),
            ));
        }
        if half_day && from != to {
            return Err(TeamError::Invalid("half_day needs from == to".into()));
        }
        let note = note.trim();
        if kind == LeaveKind::Sick && !note.is_empty() {
            return Err(TeamError::Invalid(
                "sick leave stores day counts only; no reason or note (health data)".into(),
            ));
        }
        if kind == LeaveKind::Vacation && from.year() != to.year() {
            return Err(TeamError::Invalid(
                "split vacation at the year boundary (one request per year)".into(),
            ));
        }
        let wd = i64::from(holidays::working_days(from, to)?);
        if wd == 0 {
            return Err(TeamError::Invalid(
                "range contains no working days (weekend or public holiday)".into(),
            ));
        }
        let half_days = if half_day { 1 } else { wd * 2 };
        let clash: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM leaves WHERE employee_id = ?1 AND status IN ('requested', 'approved') AND from_date <= ?3 AND to_date >= ?2 LIMIT 1",
                params![e.id, from, to],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(other) = clash {
            return Err(TeamError::Conflict(format!("overlaps leave {other}")));
        }
        // Sick days are a notification, not a request: recorded as approved.
        let (status, by, at) = if kind == LeaveKind::Sick {
            (
                LeaveStatus::Approved,
                Some("recorded".to_string()),
                Some(now()),
            )
        } else {
            (LeaveStatus::Requested, None, None)
        };
        self.conn.execute(
            "INSERT INTO leaves (employee_id, kind, from_date, to_date, half_day, half_days, status, note, requested_at, decided_by, decided_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![e.id, kind.as_str(), from, to, half_day, half_days, status.as_str(), note, now(), by, at],
        )?;
        self.leave(self.conn.last_insert_rowid())
    }

    /// Approve or reject a request. Approving vacation beyond the balance needs `force`.
    pub fn decide_leave(&self, id: i64, approve: bool, by: &str, force: bool) -> Result<Leave> {
        let l = self.leave(id)?;
        if l.status != LeaveStatus::Requested {
            return Err(TeamError::Conflict(format!(
                "leave {id} is {}, not requested",
                l.status.as_str()
            )));
        }
        if approve && l.kind == LeaveKind::Vacation {
            let bal = self.balance(&l.employee, l.from.year())?;
            let after = bal.remaining_half_days - l.half_days;
            if after < 0 && !force {
                return Err(TeamError::Invalid(format!(
                    "approving leaves {} days of vacation for {} (pass force to allow)",
                    days(after),
                    l.from.year()
                )));
            }
        }
        let status = if approve {
            LeaveStatus::Approved
        } else {
            LeaveStatus::Rejected
        };
        self.conn.execute(
            "UPDATE leaves SET status = ?2, decided_by = ?3, decided_at = ?4 WHERE id = ?1",
            params![id, status.as_str(), by, now()],
        )?;
        self.leave(id)
    }

    pub fn cancel_leave(&self, id: i64, by: &str) -> Result<Leave> {
        let l = self.leave(id)?;
        if !matches!(l.status, LeaveStatus::Requested | LeaveStatus::Approved) {
            return Err(TeamError::Conflict(format!(
                "leave {id} is {}",
                l.status.as_str()
            )));
        }
        self.conn.execute("UPDATE leaves SET status = 'cancelled', decided_by = ?2, decided_at = ?3 WHERE id = ?1", params![id, by, now()])?;
        self.leave(id)
    }

    pub fn adjust_leave(
        &self,
        code: &str,
        year: i32,
        half_days: i64,
        reason: &str,
    ) -> Result<LeaveBalance> {
        let e = self.employee(code)?;
        if reason.trim().is_empty() {
            return Err(TeamError::Invalid(
                "an adjustment needs a reason (e.g. carry-over from previous year)".into(),
            ));
        }
        if half_days == 0 || half_days.abs() > 120 {
            return Err(TeamError::Invalid(
                "adjustment must be between -60 and 60 days and not zero".into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO leave_adjustments (employee_id, year, half_days, reason, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![e.id, year, half_days, reason.trim(), now()],
        )?;
        self.balance(&e.code, year)
    }

    /// Entitlement in the start year is pro rata by months (start month counts in full),
    /// rounded up to a half day. Simplification: the first-6-months accrual rule of the
    /// Urlaubsgesetz is not modelled.
    pub fn entitlement_half_days(e: &Employee, year: i32) -> i64 {
        let full = e.leave_days_per_year * 2;
        match e.start_date.year().cmp(&year) {
            std::cmp::Ordering::Greater => 0,
            std::cmp::Ordering::Less => full,
            std::cmp::Ordering::Equal => {
                let months = i64::from(13 - e.start_date.month());
                (full * months + 11) / 12
            }
        }
    }

    pub fn balance(&self, code: &str, year: i32) -> Result<LeaveBalance> {
        let e = self.employee(code)?;
        let a = NaiveDate::from_ymd_opt(year, 1, 1)
            .ok_or_else(|| TeamError::Invalid(format!("year {year}")))?;
        let b = NaiveDate::from_ymd_opt(year, 12, 31)
            .ok_or_else(|| TeamError::Invalid(format!("year {year}")))?;
        let entitlement = Self::entitlement_half_days(&e, year);
        let adjustments: i64 = self.conn.query_row("SELECT COALESCE(SUM(half_days), 0) FROM leave_adjustments WHERE employee_id = ?1 AND year = ?2", params![e.id, year], |r| r.get(0))?;
        let (mut taken, mut pending, mut sick, mut comp, mut unpaid) = (0, 0, 0, 0, 0);
        for l in self.leaves(Some(&e.code), Some(a), Some(b), None)? {
            let hd = leave_half_days_in(&l, a, b)?;
            match (l.kind, l.status) {
                (LeaveKind::Vacation, LeaveStatus::Approved) => taken += hd,
                (LeaveKind::Vacation, LeaveStatus::Requested) => pending += hd,
                (LeaveKind::Sick, LeaveStatus::Approved) => sick += hd,
                (LeaveKind::CompTime, LeaveStatus::Approved) => comp += hd,
                (LeaveKind::Unpaid, LeaveStatus::Approved) => unpaid += hd,
                _ => {}
            }
        }
        let remaining = entitlement + adjustments - taken;
        Ok(LeaveBalance {
            employee: e.code,
            year,
            entitlement_days: days(entitlement),
            adjustment_days: days(adjustments),
            taken_days: days(taken),
            pending_days: days(pending),
            remaining_days: days(remaining),
            sick_days: days(sick),
            comp_time_days: days(comp),
            unpaid_days: days(unpaid),
            remaining_half_days: remaining,
        })
    }

    // ---------- KV minimum table ----------

    pub fn kv_upsert(&self, rows: &[KvRow]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        for r in rows {
            tx.execute(
                "INSERT INTO kv_minimums (kv, grp, valid_from, monthly_min_cents, weekly_minutes) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(kv, grp, valid_from) DO UPDATE SET monthly_min_cents = excluded.monthly_min_cents, weekly_minutes = excluded.weekly_minutes",
                params![r.kv, r.group, r.valid_from, r.monthly_min_cents, r.weekly_minutes],
            )?;
        }
        tx.commit()?;
        Ok(rows.len())
    }

    pub fn kv_rows(&self) -> Result<Vec<KvRow>> {
        let mut st = self.conn.prepare("SELECT kv, grp, valid_from, monthly_min_cents, weekly_minutes FROM kv_minimums ORDER BY kv, grp, valid_from")?;
        let rows = st
            .query_map([], |r| {
                Ok(KvRow {
                    kv: r.get(0)?,
                    group: r.get(1)?,
                    valid_from: r.get(2)?,
                    monthly_min_cents: r.get(3)?,
                    weekly_minutes: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The row in force on `on` (latest valid_from not after it).
    pub fn kv_minimum(&self, kv: &str, group: &str, on: NaiveDate) -> Result<Option<KvRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT kv, grp, valid_from, monthly_min_cents, weekly_minutes FROM kv_minimums WHERE kv = ?1 AND grp = ?2 AND valid_from <= ?3 ORDER BY valid_from DESC LIMIT 1",
                params![kv, group, on],
                |r| Ok(KvRow { kv: r.get(0)?, group: r.get(1)?, valid_from: r.get(2)?, monthly_min_cents: r.get(3)?, weekly_minutes: r.get(4)? }),
            )
            .optional()?)
    }

    // ---------- AZG findings ----------

    /// Store the findings of a check over `from..=to`. New findings are inserted,
    /// existing ones refreshed, and findings in the range that no longer apply are
    /// marked cleared (kept for the record). Returns (new, cleared).
    pub fn record_violations(
        &self,
        code: &str,
        from: NaiveDate,
        to: NaiveDate,
        found: &[NewViolation],
    ) -> Result<(usize, usize)> {
        let e = self.employee(code)?;
        let tx = self.conn.unchecked_transaction()?;
        let stamp = now();
        let mut new = 0;
        for v in found {
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM violations WHERE employee_id = ?1 AND date = ?2 AND kind = ?3",
                    params![e.id, v.date, v.kind],
                    |r| r.get(0),
                )
                .optional()?;
            match existing {
                Some(id) => {
                    tx.execute("UPDATE violations SET severity = ?2, minutes = ?3, detail = ?4, cleared_at = NULL WHERE id = ?1", params![id, v.severity, v.minutes, v.detail])?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO violations (employee_id, date, kind, severity, minutes, detail, detected_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![e.id, v.date, v.kind, v.severity, v.minutes, v.detail, stamp],
                    )?;
                    new += 1;
                }
            }
        }
        let mut st = tx.prepare("SELECT id, date, kind FROM violations WHERE employee_id = ?1 AND cleared_at IS NULL AND date >= ?2 AND date <= ?3")?;
        let active = st
            .query_map(params![e.id, from, to], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, NaiveDate>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(st);
        let mut cleared = 0;
        for (id, date, kind) in active {
            if !found.iter().any(|v| v.date == date && v.kind == kind) {
                tx.execute(
                    "UPDATE violations SET cleared_at = ?2 WHERE id = ?1",
                    params![id, stamp],
                )?;
                cleared += 1;
            }
        }
        tx.commit()?;
        Ok((new, cleared))
    }

    pub fn violations(&self, code: Option<&str>, include_cleared: bool) -> Result<Vec<Violation>> {
        let mut sql = String::from(
            "SELECT v.id, e.code, v.date, v.kind, v.severity, v.minutes, v.detail, v.detected_at, v.cleared_at FROM violations v JOIN employees e ON e.id = v.employee_id WHERE 1 = 1",
        );
        let mut args: Vec<SqlValue> = Vec::new();
        if let Some(c) = code {
            args.push(SqlValue::Text(self.employee(c)?.code));
            sql.push_str(" AND e.code = ?1");
        }
        if !include_cleared {
            sql.push_str(" AND v.cleared_at IS NULL");
        }
        sql.push_str(" ORDER BY v.date, e.code, v.kind");
        let mut st = self.conn.prepare(&sql)?;
        let rows = st
            .query_map(params_from_iter(args.iter()), |r| {
                Ok(Violation {
                    id: r.get(0)?,
                    employee: r.get(1)?,
                    date: r.get(2)?,
                    kind: r.get(3)?,
                    severity: r.get(4)?,
                    minutes: r.get(5)?,
                    detail: r.get(6)?,
                    detected_at: r.get(7)?,
                    cleared_at: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn stats(&self) -> Result<Stats> {
        let one = |sql: &str| -> Result<i64> { Ok(self.conn.query_row(sql, [], |r| r.get(0))?) };
        Ok(Stats {
            employees_active: one("SELECT COUNT(*) FROM employees WHERE active = 1")?,
            clocked_in: one("SELECT COUNT(*) FROM time_entries WHERE end_at IS NULL")?,
            leave_requests_open: one("SELECT COUNT(*) FROM leaves WHERE status = 'requested'")?,
            violations_active: one("SELECT COUNT(*) FROM violations WHERE cleared_at IS NULL")?,
            kv_rows: one("SELECT COUNT(*) FROM kv_minimums")?,
            demo: self.is_demo()?,
        })
    }
}

#[cfg(test)]
mod tests;
