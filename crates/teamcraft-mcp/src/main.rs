//! `teamcraft`: CLI and MCP stdio server in one binary.
//!
//! Every CLI verb builds the same tool-argument JSON an MCP client would send
//! and goes through `Command::from_tool` + `dispatch`, so CLI and MCP share one
//! code path. In `mcp` mode stdout carries only JSON-RPC; logs go to stderr.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clap::{Parser, Subcommand};
use craft_biz::bus::Bus;
use craft_biz::mcp::{serve_stdio, ToolHost};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::process::ExitCode;
use teamcraft_core::{dispatch, tools, Command, Confinement, Ctx, APP, VERSION};
use teamcraft_store::Team;

const AFTER_HELP: &str = "Typical week:
  teamcraft clock-in JM; teamcraft clock-out JM --break 30
  teamcraft time add JM --date 05.11.2026 --start 22:00 --end 06:00 --break 30
  teamcraft confirm JM 2026-W45                # week becomes read-only
  teamcraft azg --from 2026-11-01 --to 2026-11-30
  teamcraft leave request JM vacation 21.12.2026 --to 31.12.2026; teamcraft leave approve 3
  teamcraft timesheet JM 2026-11 --out D:\\Temp\\jm.csv
  teamcraft capacity 2026-W46 --emit           # to taskcraft
See HELP.md for workflows and docs/TOOLS.md for every MCP tool.";

#[derive(Parser)]
#[command(after_help = AFTER_HELP,
    name = "teamcraft",
    version,
    about = "Working time (AZG checks) and leave for a small Austrian team. Not payroll. CLI + MCP."
)]
struct Cli {
    /// Database. Default: $TEAMCRAFT_DB or %LOCALAPPDATA%\teamcraft\teamcraft.db
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Print machine-readable JSON only.
    #[arg(long, global = true)]
    json: bool,
    /// Restrict file access to these directories (repeatable). Required for file tools in `mcp` mode.
    #[arg(long = "allow-path", global = true)]
    allow_path: Vec<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Counts and who is clocked in.
    Status,
    /// Employees.
    Employee {
        #[command(subcommand)]
        op: EmployeeOp,
    },
    /// Start a time entry now (or --at).
    ClockIn {
        employee: String,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Close the open entry.
    ClockOut {
        employee: String,
        #[arg(long)]
        at: Option<String>,
        #[arg(long = "break", default_value_t = 0)]
        break_minutes: i64,
    },
    /// Time entries.
    Time {
        #[command(subcommand)]
        op: TimeOp,
    },
    /// Confirm an ISO week (e.g. 2026-W45); entries become read-only.
    Confirm { employee: String, week: String },
    /// Run the AZG check over a date range and record findings.
    Azg {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        employee: Option<String>,
    },
    /// Recorded AZG findings.
    Violations {
        #[arg(long)]
        employee: Option<String>,
        /// Include cleared findings.
        #[arg(long)]
        all: bool,
    },
    /// Leave requests and balances.
    Leave {
        #[command(subcommand)]
        op: LeaveOp,
    },
    /// Austrian public holidays.
    Holidays { year: Option<i32> },
    /// KV minimum table.
    Kv {
        #[command(subcommand)]
        op: KvOp,
    },
    /// Monthly working-time record (YYYY-MM).
    Timesheet {
        employee: String,
        month: String,
        /// Write a semicolon CSV here.
        #[arg(long)]
        out: Option<String>,
    },
    /// Available hours for an ISO week (default: this week).
    Capacity {
        week: Option<String>,
        /// Send capacity.changed to taskcraft.
        #[arg(long)]
        emit: bool,
    },
    /// Fill an empty database with the labelled demo team.
    DemoSeed,
    /// Print the MCP tool catalog (JSON, or markdown for docs/TOOLS.md).
    Tools {
        #[arg(long)]
        markdown: bool,
    },
    /// Run the MCP stdio server.
    Mcp,
}

#[derive(Subcommand)]
enum EmployeeOp {
    List {
        #[arg(long)]
        all: bool,
    },
    Add {
        code: String,
        name: String,
        /// Weekly hours, e.g. 38,5
        #[arg(long)]
        hours: Option<String>,
        #[arg(long)]
        leave_days: Option<i64>,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        kv: Option<String>,
        #[arg(long)]
        group: Option<String>,
        /// Monthly gross, DE format ("2.850,00")
        #[arg(long)]
        gross: Option<String>,
    },
    Update {
        code: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        hours: Option<String>,
        #[arg(long)]
        leave_days: Option<i64>,
        #[arg(long)]
        kv: Option<String>,
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        gross: Option<String>,
        #[arg(long)]
        inactive: bool,
    },
}

#[derive(Subcommand)]
enum TimeOp {
    Add {
        employee: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        start: String,
        #[arg(long)]
        end: String,
        #[arg(long = "break", default_value_t = 0)]
        break_minutes: i64,
        #[arg(long)]
        note: Option<String>,
    },
    List {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        employee: Option<String>,
    },
    Delete {
        id: i64,
    },
}

#[derive(Subcommand)]
enum LeaveOp {
    Request {
        employee: String,
        /// vacation, sick, comp_time, unpaid, other
        kind: String,
        from: String,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        half: bool,
        #[arg(long)]
        note: Option<String>,
    },
    Approve {
        id: i64,
        #[arg(long)]
        force: bool,
    },
    Reject {
        id: i64,
    },
    Cancel {
        id: i64,
    },
    List {
        #[arg(long)]
        employee: Option<String>,
        #[arg(long)]
        year: Option<i32>,
        #[arg(long)]
        status: Option<String>,
    },
    Balance {
        #[arg(long)]
        employee: Option<String>,
        #[arg(long)]
        year: Option<i32>,
    },
    Adjust {
        employee: String,
        year: i32,
        /// Days, multiple of 0.5 (negative allowed)
        #[arg(allow_hyphen_values = true)]
        days: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum KvOp {
    Import {
        path: String,
    },
    List,
    Check {
        #[arg(long)]
        date: Option<String>,
    },
}

fn default_db() -> PathBuf {
    if let Ok(p) = std::env::var("TEAMCRAFT_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("teamcraft").join("teamcraft.db")
}

/// Build a JSON object, dropping `None` values.
fn obj(pairs: Vec<(&str, Option<Value>)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        if let Some(v) = v {
            m.insert(k.to_string(), v);
        }
    }
    Value::Object(m)
}
fn sv(s: &str) -> Option<Value> {
    Some(Value::String(s.to_string()))
}
fn os(s: Option<String>) -> Option<Value> {
    s.map(Value::String)
}

/// Map a CLI verb onto (tool name, arguments).
fn to_tool(cmd: Cmd) -> Result<(&'static str, Value), String> {
    Ok(match cmd {
        Cmd::Status => ("team_status", json!({})),
        Cmd::Employee {
            op: EmployeeOp::List { all },
        } => ("team_employee_list", json!({ "include_inactive": all })),
        Cmd::Employee {
            op:
                EmployeeOp::Add {
                    code,
                    name,
                    hours,
                    leave_days,
                    start,
                    kv,
                    group,
                    gross,
                },
        } => (
            "team_employee_add",
            obj(vec![
                ("code", sv(&code)),
                ("name", sv(&name)),
                ("weekly_hours", os(hours)),
                ("leave_days", leave_days.map(|d| json!(d))),
                ("start_date", os(start)),
                ("kv_name", os(kv)),
                ("kv_group", os(group)),
                ("monthly_gross", os(gross)),
            ]),
        ),
        Cmd::Employee {
            op:
                EmployeeOp::Update {
                    code,
                    name,
                    hours,
                    leave_days,
                    kv,
                    group,
                    gross,
                    inactive,
                },
        } => (
            "team_employee_update",
            obj(vec![
                ("code", sv(&code)),
                ("name", os(name)),
                ("weekly_hours", os(hours)),
                ("leave_days", leave_days.map(|d| json!(d))),
                ("kv_name", os(kv)),
                ("kv_group", os(group)),
                ("monthly_gross", os(gross)),
                ("active", inactive.then_some(json!(false))),
            ]),
        ),
        Cmd::ClockIn { employee, at, note } => (
            "team_clock_in",
            obj(vec![
                ("employee", sv(&employee)),
                ("at", os(at)),
                ("note", os(note)),
            ]),
        ),
        Cmd::ClockOut {
            employee,
            at,
            break_minutes,
        } => (
            "team_clock_out",
            obj(vec![
                ("employee", sv(&employee)),
                ("at", os(at)),
                ("break_minutes", Some(json!(break_minutes))),
            ]),
        ),
        Cmd::Time {
            op:
                TimeOp::Add {
                    employee,
                    date,
                    start,
                    end,
                    break_minutes,
                    note,
                },
        } => (
            "team_time_add",
            obj(vec![
                ("employee", sv(&employee)),
                ("date", os(date)),
                ("start", sv(&start)),
                ("end", sv(&end)),
                ("break_minutes", Some(json!(break_minutes))),
                ("note", os(note)),
            ]),
        ),
        Cmd::Time {
            op: TimeOp::List { from, to, employee },
        } => (
            "team_time_list",
            obj(vec![
                ("from", sv(&from)),
                ("to", sv(&to)),
                ("employee", os(employee)),
            ]),
        ),
        Cmd::Time {
            op: TimeOp::Delete { id },
        } => ("team_time_delete", json!({ "id": id })),
        Cmd::Confirm { employee, week } => (
            "team_week_confirm",
            json!({ "employee": employee, "week": week }),
        ),
        Cmd::Azg { from, to, employee } => (
            "team_azg_check",
            obj(vec![
                ("from", sv(&from)),
                ("to", sv(&to)),
                ("employee", os(employee)),
            ]),
        ),
        Cmd::Violations { employee, all } => (
            "team_violations",
            obj(vec![
                ("employee", os(employee)),
                ("include_cleared", Some(json!(all))),
            ]),
        ),
        Cmd::Leave {
            op:
                LeaveOp::Request {
                    employee,
                    kind,
                    from,
                    to,
                    half,
                    note,
                },
        } => (
            "team_leave_request",
            obj(vec![
                ("employee", sv(&employee)),
                ("kind", sv(&kind)),
                ("from", sv(&from)),
                ("to", os(to)),
                ("half_day", Some(json!(half))),
                ("note", os(note)),
            ]),
        ),
        Cmd::Leave {
            op: LeaveOp::Approve { id, force },
        } => (
            "team_leave_decide",
            json!({ "id": id, "approve": true, "force": force }),
        ),
        Cmd::Leave {
            op: LeaveOp::Reject { id },
        } => ("team_leave_decide", json!({ "id": id, "approve": false })),
        Cmd::Leave {
            op: LeaveOp::Cancel { id },
        } => ("team_leave_cancel", json!({ "id": id })),
        Cmd::Leave {
            op:
                LeaveOp::List {
                    employee,
                    year,
                    status,
                },
        } => (
            "team_leave_list",
            obj(vec![
                ("employee", os(employee)),
                ("year", year.map(|y| json!(y))),
                ("status", os(status)),
            ]),
        ),
        Cmd::Leave {
            op: LeaveOp::Balance { employee, year },
        } => (
            "team_leave_balance",
            obj(vec![
                ("employee", os(employee)),
                ("year", year.map(|y| json!(y))),
            ]),
        ),
        Cmd::Leave {
            op:
                LeaveOp::Adjust {
                    employee,
                    year,
                    days,
                    reason,
                },
        } => (
            "team_leave_adjust",
            json!({ "employee": employee, "year": year, "days": days, "reason": reason }),
        ),
        Cmd::Holidays { year } => ("team_holidays", obj(vec![("year", year.map(|y| json!(y)))])),
        Cmd::Kv {
            op: KvOp::Import { path },
        } => ("team_kv_import", json!({ "path": path })),
        Cmd::Kv { op: KvOp::List } => ("team_kv_list", json!({})),
        Cmd::Kv {
            op: KvOp::Check { date },
        } => ("team_kv_check", obj(vec![("date", os(date))])),
        Cmd::Timesheet {
            employee,
            month,
            out,
        } => (
            "team_timesheet",
            obj(vec![
                ("employee", sv(&employee)),
                ("month", sv(&month)),
                ("path", os(out)),
            ]),
        ),
        Cmd::Capacity { week, emit } => (
            "team_capacity",
            obj(vec![("week", os(week)), ("emit", Some(json!(emit)))]),
        ),
        Cmd::DemoSeed => ("team_demo_seed", json!({})),
        Cmd::Tools { .. } | Cmd::Mcp => return Err("not a tool verb".into()),
    })
}

struct Host {
    ctx: Ctx,
}

impl ToolHost for Host {
    fn server_name(&self) -> &str {
        APP
    }
    fn server_version(&self) -> &str {
        VERSION
    }
    fn tools(&self) -> Vec<Value> {
        tools()
    }
    fn call(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let cmd = Command::from_tool(name, args)?;
        dispatch(&mut self.ctx, cmd)
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".into(),
        Value::Array(a) => a.iter().map(scalar).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

/// Column order for human tables; unknown keys follow in key order.
const COLUMN_ORDER: &[&str] = &[
    "id",
    "date",
    "weekday",
    "week",
    "employee",
    "code",
    "name",
    "status",
    "severity",
    "kind",
    "from",
    "to",
    "days",
    "half_day",
    "start",
    "end",
    "break_minutes",
    "worked",
    "absence",
    "holiday",
    "azg",
    "since",
    "contract",
    "available",
    "leave_days",
    "entitlement_days",
    "adjustment_days",
    "taken_days",
    "pending_days",
    "remaining_days",
    "sick_days",
    "comp_time_days",
    "unpaid_days",
    "weekly_hours",
    "leave_days_per_year",
    "start_date",
    "group",
    "hours",
    "gross",
    "minimum_pro_rata",
    "shortfall",
    "monthly_gross",
    "monthly_min",
    "valid_from",
    "kv",
    "kv_name",
    "kv_group",
    "detail",
    "note",
];
/// Noise in human tables (still present in --json).
const HIDDEN: &[&str] = &[
    "minutes",
    "detected_at",
    "requested_at",
    "decided_at",
    "decided_by",
    "year",
    "source",
    "locked",
    "half_days",
    "remaining_half_days",
];

fn columns(rows: &[Value]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for r in rows.iter().filter_map(Value::as_object) {
        for (k, v) in r {
            if !v.is_object()
                && (!k.ends_with("_minutes") || k == "break_minutes")
                && !HIDDEN.contains(&k.as_str())
                && !keys.contains(k)
            {
                keys.push(k.clone());
            }
        }
    }
    keys.sort_by_key(|k| {
        (
            COLUMN_ORDER
                .iter()
                .position(|c| c == k)
                .unwrap_or(usize::MAX),
            k.clone(),
        )
    });
    keys
}

/// Human rendering: header scalars, then an aligned table of the first list found.
fn render(v: &Value) -> String {
    const LISTS: [&str; 10] = [
        "rows",
        "entries",
        "employees",
        "violations",
        "leaves",
        "balances",
        "holidays",
        "findings",
        "clocked_in",
        "days",
    ];
    let Some(obj) = v.as_object() else {
        return scalar(v);
    };
    let Some(key) = LISTS
        .iter()
        .find(|k| obj.get(**k).is_some_and(Value::is_array))
    else {
        return serde_json::to_string_pretty(v).unwrap_or_else(|e| e.to_string());
    };
    let mut out = String::new();
    for (k, val) in obj {
        if !val.is_array() && !val.is_object() && !k.ends_with("_minutes") {
            out.push_str(&format!("{k}: {}\n", scalar(val)));
        }
    }
    let rows = obj
        .get(*key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if rows.is_empty() {
        out.push_str("(none)\n");
        return out;
    }
    let cols = columns(&rows);
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            cols.iter()
                .map(|c| r.get(c).map(scalar).unwrap_or_else(|| "-".into()))
                .collect()
        })
        .collect();
    let widths: Vec<usize> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| {
            table
                .iter()
                .map(|r| r[i].chars().count())
                .chain([c.chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: &[String]| -> String {
        let padded: Vec<String> = cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c}{}", " ".repeat(w.saturating_sub(c.chars().count()))))
            .collect();
        format!("{}\n", padded.join("  ").trim_end())
    };
    out.push_str(&line(&cols));
    for r in &table {
        out.push_str(&line(r));
    }
    out
}
fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Cmd::Tools { markdown } = cli.cmd {
        if markdown {
            print!("{}", craft_biz::mcp::tools_markdown(APP, &tools()));
            return ExitCode::SUCCESS;
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&tools()).unwrap_or_default()
        );
        return ExitCode::SUCCESS;
    }
    let db = cli.db.clone().unwrap_or_else(default_db);
    let team = match Team::open(&db) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("teamcraft: cannot open {}: {e}", db.display());
            return ExitCode::FAILURE;
        }
    };
    let is_mcp = matches!(cli.cmd, Cmd::Mcp);
    let confinement = if is_mcp || !cli.allow_path.is_empty() {
        match Confinement::roots(&cli.allow_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("teamcraft: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        Confinement(None)
    };
    let actor = if is_mcp {
        "agent".to_string()
    } else {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "cli".into())
    };
    let ctx = Ctx {
        team,
        confinement,
        bus: Bus::new(Bus::default_root()),
        actor,
    };
    if is_mcp {
        eprintln!("[teamcraft] db {}", db.display());
        let mut host = Host { ctx };
        return match serve_stdio(&mut host) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("teamcraft mcp: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let json_out = cli.json;
    let mut ctx = ctx;
    let result = to_tool(cli.cmd)
        .and_then(|(name, args)| Command::from_tool(name, &args))
        .and_then(|cmd| dispatch(&mut ctx, cmd));
    match result {
        Ok(v) => {
            if json_out {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                print!("{}", render(&v));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            if json_out {
                println!("{}", json!({ "error": e }));
            } else {
                eprintln!("teamcraft: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_biz::mcp::handle_line;

    #[test]
    fn tools_doc_is_current() {
        let committed = include_str!("../../../docs/TOOLS.md");
        assert!(
            craft_biz::mcp::docs_in_sync(committed, &craft_biz::mcp::tools_markdown(APP, &tools())),
            "docs/TOOLS.md is stale: run `just docs`"
        );
    }

    fn host() -> Host {
        let team = Team::open_in_memory().unwrap();
        let bus = Bus::new(
            std::env::temp_dir().join(format!("teamcraft-mcp-test-{}", std::process::id())),
        );
        Host {
            ctx: Ctx {
                team,
                confinement: Confinement(Some(Vec::new())),
                bus,
                actor: "agent".into(),
            },
        }
    }

    fn text(v: &Value) -> String {
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn mcp_probe_initialize_list_call() {
        let mut h = host();
        let init = handle_line(
            &mut h,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "teamcraft");
        let list =
            handle_line(&mut h, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        assert_eq!(list["result"]["tools"].as_array().map(Vec::len), Some(25));
        let seed = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"team_demo_seed","arguments":{}}}"#).unwrap();
        assert_eq!(seed["result"]["isError"], false, "{seed}");
        let hol = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"team.holidays","arguments":{"year":2026}}}"#).unwrap();
        assert!(text(&hol).contains("Fronleichnam"));
        let sick = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"team_leave_request","arguments":{"employee":"JM","kind":"sick","from":"2026-11-10","note":"migraine"}}}"#).unwrap();
        assert_eq!(sick["result"]["isError"], true);
        let kv = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"team_kv_import","arguments":{"path":"C:/Windows/win.ini"}}}"#).unwrap();
        assert_eq!(kv["result"]["isError"], true, "confined");
        let dec = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"team_leave_list","arguments":{"status":"requested"}}}"#).unwrap();
        assert!(text(&dec).contains("Weihnachten"));
    }

    #[test]
    fn cli_verbs_map_to_known_tools() {
        let verbs = vec![
            Cmd::Status,
            Cmd::Employee {
                op: EmployeeOp::List { all: false },
            },
            Cmd::Azg {
                from: "2026-11-01".into(),
                to: "2026-11-30".into(),
                employee: None,
            },
            Cmd::Leave {
                op: LeaveOp::Approve {
                    id: 1,
                    force: false,
                },
            },
            Cmd::Leave {
                op: LeaveOp::Balance {
                    employee: None,
                    year: None,
                },
            },
            Cmd::Kv {
                op: KvOp::Check { date: None },
            },
            Cmd::Timesheet {
                employee: "JM".into(),
                month: "2026-11".into(),
                out: None,
            },
            Cmd::Capacity {
                week: None,
                emit: false,
            },
            Cmd::DemoSeed,
        ];
        let names: Vec<String> = tools()
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect();
        for v in verbs {
            let (name, _) = to_tool(v).unwrap();
            assert!(names.iter().any(|n| n == name), "{name}");
        }
    }

    #[test]
    fn render_lists() {
        let v =
            json!({ "year": 2026, "holidays": [ { "date": "2026-01-01", "name": "Neujahr" } ] });
        let s = render(&v);
        assert!(s.contains("year: 2026") && s.contains("Neujahr"));
        assert!(render(&json!({ "violations": [] })).contains("(none)"));
        let v = json!({ "violations": [ { "kind": "break_missing", "date": "2026-11-04", "employee": "JM", "detected_at": "x", "cleared_at": null } ] });
        let s = render(&v);
        let header = s.lines().next().unwrap();
        assert!(
            header.starts_with("date")
                && header.contains("employee")
                && !header.contains("detected_at"),
            "{s}"
        );
    }
}
