//! TeamCraft desktop window, laid out like Personio / Clockodo: time clock,
//! monthly timesheet (AZG section 26), absence calendar, leave approval
//! board, vacation balances, weekly capacity, working-time law findings,
//! team and KV minimum check.
//!
//! Every change goes through `teamcraft_core::dispatch` (same path as CLI and
//! MCP). Reads use the store and the side-effect-free core functions
//! `timesheet` (without path), `capacity` (without emit) and `kv_check`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Datelike, Duration, Local, NaiveDate, Timelike};
use craft_biz::bus::Bus;
use craft_biz::holidays;
use craft_biz_ui::app::{self, Frame, NavEntry, SuiteApp, Wiring};
use craft_biz_ui::kanban::{self, Board, Card, Column};
use craft_biz_ui::table::{Cell, Col, Table, TableEvent};
use craft_biz_ui::theme::{self, semibold};
use craft_biz_ui::widgets::{self, Kpi};
use craft_biz_ui::Tone;
use egui::{Align2, FontId, RichText, Sense, Stroke, Ui, Vec2};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use teamcraft_core::{capacity, dispatch, kv_check, timesheet, tools, Command, Confinement, Ctx};
use teamcraft_store::{
    fmt_hm, week_label, Employee, Leave, LeaveBalance, LeaveKind, LeaveStatus, Team, TimeEntry,
    Violation,
};

const V_TODAY: usize = 0;
const V_SHEET: usize = 1;
const V_CALENDAR: usize = 2;
const V_LEAVE: usize = 3;
const V_BALANCE: usize = 4;
const V_CAPACITY: usize = 5;
const V_AZG: usize = 6;
const V_TEAM: usize = 7;
const V_SETTINGS: usize = 8;

/// Every MCP tool and where the window performs it (checked by a test).
pub const COVERAGE: &[(&str, &str)] = &[
    ("team_status", "Settings > Status"),
    ("team_employee_add", "Team > Add employee"),
    ("team_employee_list", "Team > As list"),
    ("team_employee_update", "Team > Edit selected"),
    ("team_clock_in", "Time clock > Clock in"),
    ("team_clock_out", "Time clock > Clock out"),
    ("team_time_add", "Timesheet > Add entry"),
    ("team_time_list", "Timesheet > Entries report"),
    ("team_time_delete", "Timesheet > Entries > Delete"),
    ("team_week_confirm", "Timesheet > Confirm week"),
    ("team_azg_check", "Working time law > Run check"),
    ("team_violations", "Working time law > Report"),
    (
        "team_leave_request",
        "Absence calendar click; Leave > Request leave",
    ),
    (
        "team_leave_decide",
        "Leave board drag to Approved / Rejected; Time clock approvals",
    ),
    ("team_leave_cancel", "Leave board drag to Cancelled"),
    ("team_leave_list", "Leave > Report"),
    ("team_leave_balance", "Balances > Report"),
    ("team_leave_adjust", "Balances > Adjust"),
    ("team_holidays", "Absence calendar > Public holidays"),
    ("team_kv_import", "Team > Import KV table"),
    ("team_kv_list", "Team > KV table"),
    ("team_kv_check", "Team > KV check (table and report)"),
    ("team_timesheet", "Timesheet > Export CSV"),
    ("team_capacity", "Capacity > Send to taskcraft"),
    ("team_demo_seed", "Empty state; Settings > Demo data"),
];

pub struct TeamApp {
    ctx: Ctx,
    db: String,
    today: NaiveDate,
    now_min: i64,
    emps: Vec<Employee>,
    open: Vec<TimeEntry>,
    today_entries: Vec<TimeEntry>,
    leaves: Vec<Leave>,
    balances: Vec<LeaveBalance>,
    violations: Vec<Violation>,
    kv: Option<Value>,
    sel: String,
    month: NaiveDate,
    sheet: Option<Value>,
    entries: Vec<TimeEntry>,
    week: NaiveDate,
    cap: Option<Value>,
    cal: NaiveDate,
    show_cleared: bool,
    team_sel: Option<String>,
}

fn first_of(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

fn add_months(d: NaiveDate, n: i32) -> NaiveDate {
    let m = d.year() * 12 + d.month0() as i32 + n;
    NaiveDate::from_ymd_opt(m.div_euclid(12), m.rem_euclid(12) as u32 + 1, 1).unwrap_or(d)
}

fn last_of(d: NaiveDate) -> NaiveDate {
    add_months(d, 1) - Duration::days(1)
}

fn monday(d: NaiveDate) -> NaiveDate {
    d - Duration::days(i64::from(d.weekday().num_days_from_monday()))
}

fn date_de(d: NaiveDate) -> String {
    d.format("%d.%m.%Y").to_string()
}

fn month_name(d: NaiveDate, de: bool) -> String {
    const DE: [&str; 12] = [
        "Jänner",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ];
    const EN: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let i = d.month0() as usize;
    format!("{} {}", if de { DE[i] } else { EN[i] }, d.year())
}

/// Two-letter weekday in the window language.
fn weekday_short(d: NaiveDate, de: bool) -> &'static str {
    let i = d.weekday().num_days_from_monday() as usize;
    if de {
        ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"][i]
    } else {
        ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"][i]
    }
}

fn kind_tone(k: LeaveKind) -> Tone {
    match k {
        LeaveKind::Vacation => Tone::Accent,
        LeaveKind::Sick => Tone::Warn,
        LeaveKind::CompTime => Tone::Teal,
        LeaveKind::Unpaid => Tone::Violet,
        LeaveKind::Other => Tone::Neutral,
    }
}

fn kind_label(k: LeaveKind, de: bool) -> &'static str {
    match (k, de) {
        (LeaveKind::Vacation, true) => "Urlaub",
        (LeaveKind::Vacation, false) => "Vacation",
        (LeaveKind::Sick, true) => "Krankenstand",
        (LeaveKind::Sick, false) => "Sick leave",
        (LeaveKind::CompTime, true) => "Zeitausgleich",
        (LeaveKind::CompTime, false) => "Comp time",
        (LeaveKind::Unpaid, true) => "Unbezahlt",
        (LeaveKind::Unpaid, false) => "Unpaid",
        (LeaveKind::Other, true) => "Sonstiges",
        (LeaveKind::Other, false) => "Other",
    }
}

fn status_label(s: LeaveStatus, de: bool) -> &'static str {
    match (s, de) {
        (LeaveStatus::Requested, true) => "Beantragt",
        (LeaveStatus::Requested, false) => "Requested",
        (LeaveStatus::Approved, true) => "Genehmigt",
        (LeaveStatus::Approved, false) => "Approved",
        (LeaveStatus::Rejected, true) => "Abgelehnt",
        (LeaveStatus::Rejected, false) => "Rejected",
        (LeaveStatus::Cancelled, true) => "Storniert",
        (LeaveStatus::Cancelled, false) => "Cancelled",
    }
}

fn days_txt(d: f64) -> String {
    if (d - d.round()).abs() < 0.01 {
        format!("{d:.0}")
    } else {
        format!("{d:.1}").replace('.', ",")
    }
}

fn sev_tone(s: &str) -> Tone {
    if s == "violation" {
        Tone::Danger
    } else {
        Tone::Warn
    }
}

/// "+1:30" / "-0:45" from minutes.
fn signed_hm(m: i64) -> String {
    if m < 0 {
        format!("-{}", fmt_hm(-m))
    } else {
        format!("+{}", fmt_hm(m))
    }
}

impl TeamApp {
    pub fn new(ctx: Ctx, db: String) -> TeamApp {
        let today = Local::now().date_naive();
        let now = Local::now().time();
        TeamApp {
            ctx,
            db,
            today,
            now_min: i64::from(now.hour() * 60 + now.minute()),
            emps: Vec::new(),
            open: Vec::new(),
            today_entries: Vec::new(),
            leaves: Vec::new(),
            balances: Vec::new(),
            violations: Vec::new(),
            kv: None,
            sel: String::new(),
            month: first_of(today),
            sheet: None,
            entries: Vec::new(),
            week: monday(today),
            cap: None,
            cal: first_of(today),
            show_cleared: false,
            team_sel: None,
        }
    }

    fn name(&self, code: &str) -> String {
        self.emps
            .iter()
            .find(|e| e.code == code)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| code.to_string())
    }

    fn balance(&self, code: &str) -> Option<&LeaveBalance> {
        self.balances.iter().find(|b| b.employee == code)
    }

    /// Timesheet, entries and capacity depend on pickers; recompute after a picker changes.
    fn refresh_local(&mut self) {
        let r = (|| -> Result<(), String> {
            if !self.sel.is_empty() {
                let m = self.month.format("%Y-%m").to_string();
                let sel = self.sel.clone();
                self.sheet = Some(timesheet(&mut self.ctx, &sel, &m, None)?);
                self.entries = self
                    .ctx
                    .team
                    .entries(Some(&sel), self.month, last_of(self.month))
                    .map_err(|e| e.to_string())?;
            } else {
                self.sheet = None;
                self.entries.clear();
            }
            let wk = week_label(self.week);
            self.cap = Some(capacity(&mut self.ctx, &wk, false)?);
            self.violations = self
                .ctx
                .team
                .violations(None, self.show_cleared)
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(e) = r {
            eprintln!("teamcraft-app: {e}");
        }
    }

    fn empty(&self, ui: &mut Ui, f: &mut Frame) {
        widgets::card(ui, |ui| {
            widgets::empty_state(
                ui,
                f.t("Noch kein Team", "No team yet"),
                f.t(
                    "Mitarbeiter anlegen oder Demo-Daten laden.",
                    "Add an employee or load the demo data.",
                ),
            );
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 330.0).max(0.0) / 2.0);
                    if widgets::primary(ui, f.t("Mitarbeiter anlegen", "Add employee")).clicked() {
                        f.form("team_employee_add", &[]);
                    }
                    if ui
                        .button(f.t("Demo-Daten laden", "Load demo data"))
                        .clicked()
                    {
                        f.run(f.t("Demo-Daten", "Demo data"), "team_demo_seed", json!({}));
                    }
                });
            });
        });
    }

    fn approve(&self, f: &mut Frame, l: &Leave) {
        let de = f.lang_de;
        let over = l.kind == LeaveKind::Vacation
            && self
                .balance(&l.employee)
                .is_some_and(|b| b.remaining_half_days - l.half_days < 0);
        if over {
            f.toast(
                if de {
                    "Mehr als das Urlaubsguthaben: mit force bestätigen"
                } else {
                    "More than the vacation balance: confirm with force"
                }
                .to_string(),
                Tone::Warn,
            );
            f.form(
                "team_leave_decide",
                &[
                    ("id", &l.id.to_string()),
                    ("approve", "true"),
                    ("force", "true"),
                ],
            );
        } else {
            f.run(
                format!(
                    "{} {} {}",
                    status_label(LeaveStatus::Approved, de),
                    self.name(&l.employee),
                    date_de(l.from)
                ),
                "team_leave_decide",
                json!({ "id": l.id, "approve": true }),
            );
        }
    }

    fn reject(&self, f: &mut Frame, l: &Leave) {
        let de = f.lang_de;
        f.run_confirm(
            if de {
                format!(
                    "Antrag von {} ({} - {}) ablehnen?",
                    self.name(&l.employee),
                    date_de(l.from),
                    date_de(l.to)
                )
            } else {
                format!(
                    "Reject the request of {} ({} - {})?",
                    self.name(&l.employee),
                    date_de(l.from),
                    date_de(l.to)
                )
            },
            status_label(LeaveStatus::Rejected, de),
            "team_leave_decide",
            json!({ "id": l.id, "approve": false }),
        );
    }

    // ---------------- time clock ----------------

    fn view_today(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        widgets::page_header(
            ui,
            f.t("Stempeluhr", "Time clock"),
            &format!("{} {}", f.t("Heute", "Today"), date_de(self.today)),
            |ui| {
                if ui
                    .button(f.t("Eintrag nachtragen", "Add past entry"))
                    .clicked()
                {
                    f.form("team_time_add", &[("date", &self.today.to_string())]);
                }
            },
        );
        if self.emps.is_empty() {
            self.empty(ui, f);
            return;
        }
        let active = self.emps.iter().filter(|e| e.active).count();
        let requests = self
            .leaves
            .iter()
            .filter(|l| l.status == LeaveStatus::Requested)
            .count();
        let findings = self
            .violations
            .iter()
            .filter(|v| v.cleared_at.is_none())
            .count();
        widgets::kpi_row(
            ui,
            &[
                Kpi::new(
                    f.t("Aktive Mitarbeiter", "Active employees"),
                    active.to_string(),
                ),
                Kpi::new(
                    f.t("Eingestempelt", "Clocked in"),
                    self.open.len().to_string(),
                )
                .note(
                    if self.open.is_empty() {
                        f.t("niemand", "nobody")
                    } else {
                        f.t("jetzt", "now")
                    },
                    if self.open.is_empty() {
                        Tone::Neutral
                    } else {
                        Tone::Ok
                    },
                ),
                Kpi::new(f.t("Offene Anträge", "Open requests"), requests.to_string()).note(
                    if requests > 0 {
                        f.t("entscheiden", "decide")
                    } else {
                        "ok"
                    },
                    if requests > 0 { Tone::Warn } else { Tone::Ok },
                ),
                Kpi::new(f.t("AZG-Befunde", "AZG findings"), findings.to_string()).note(
                    if findings > 0 {
                        f.t("prüfen", "review")
                    } else {
                        "ok"
                    },
                    if findings > 0 { Tone::Danger } else { Tone::Ok },
                ),
            ],
        );
        ui.add_space(8.0);
        widgets::section(ui, f.t("Team heute", "Team today"), Some(active));
        let tk = theme::current(ui.ctx());
        let people: Vec<Employee> = self.emps.iter().filter(|e| e.active).cloned().collect();
        let per_row = ((ui.available_width() / 290.0).floor() as usize).clamp(1, 4);
        for chunk in people.chunks(per_row) {
            ui.columns(per_row, |cols| {
                for (i, e) in chunk.iter().enumerate() {
                    let ui = &mut cols[i];
                    let open = self.open.iter().find(|x| x.employee == e.code);
                    let worked: i64 = self
                        .today_entries
                        .iter()
                        .filter(|x| x.employee == e.code)
                        .filter_map(|x| x.minutes)
                        .sum();
                    let absent = self.leaves.iter().find(|l| {
                        l.employee == e.code
                            && l.status == LeaveStatus::Approved
                            && self.today >= l.from
                            && self.today <= l.to
                    });
                    widgets::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            widgets::avatar(ui, &e.name, 16.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&e.name).family(semibold()).size(15.0));
                                ui.label(
                                    RichText::new(format!(
                                        "{}  {} h/{}",
                                        e.code,
                                        fmt_hm(e.weekly_minutes),
                                        f.t("Wo", "wk")
                                    ))
                                    .color(tk.text_weak)
                                    .size(13.0),
                                );
                            });
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| match (open, absent) {
                            (Some(o), _) => {
                                let since = o.start.time().hour() as i64 * 60
                                    + o.start.time().minute() as i64;
                                let run = if o.start.date() == self.today {
                                    (self.now_min - since).max(0)
                                } else {
                                    0
                                };
                                widgets::pill(
                                    ui,
                                    &format!(
                                        "{} {}",
                                        f.t("seit", "in since"),
                                        o.start.format("%H:%M")
                                    ),
                                    Tone::Ok,
                                );
                                if run > 0 {
                                    ui.label(
                                        RichText::new(format!("{} h", fmt_hm(run)))
                                            .family(semibold()),
                                    );
                                }
                            }
                            (None, Some(l)) => {
                                widgets::pill(ui, kind_label(l.kind, de), kind_tone(l.kind));
                            }
                            (None, None) => {
                                widgets::pill(ui, f.t("abwesend / aus", "out"), Tone::Neutral);
                            }
                        });
                        ui.label(format!(
                            "{}: {} h",
                            f.t("Heute erfasst", "Recorded today"),
                            fmt_hm(worked)
                        ));
                        ui.add_space(4.0);
                        if let Some(o) = open {
                            let run = (self.now_min
                                - (o.start.time().hour() as i64 * 60
                                    + o.start.time().minute() as i64))
                                .max(0);
                            let brk = if run > 360 { "30" } else { "0" };
                            if widgets::primary(ui, f.t("Ausstempeln...", "Clock out...")).clicked()
                            {
                                f.form(
                                    "team_clock_out",
                                    &[("employee", &e.code), ("break_minutes", brk)],
                                );
                            }
                        } else if ui
                            .add_enabled(
                                absent.is_none(),
                                egui::Button::new(
                                    RichText::new(f.t("Einstempeln", "Clock in"))
                                        .family(semibold()),
                                ),
                            )
                            .on_disabled_hover_text(f.t("Heute abwesend", "Absent today"))
                            .clicked()
                        {
                            f.run(
                                format!("{} {}", e.code, f.t("eingestempelt", "clocked in")),
                                "team_clock_in",
                                json!({ "employee": e.code }),
                            );
                        }
                    });
                }
            });
            ui.add_space(6.0);
        }
        let pending: Vec<Leave> = self
            .leaves
            .iter()
            .filter(|l| l.status == LeaveStatus::Requested)
            .cloned()
            .collect();
        if !pending.is_empty() {
            ui.add_space(6.0);
            widgets::section(
                ui,
                f.t("Wartet auf Entscheidung", "Waiting for a decision"),
                Some(pending.len()),
            );
            for l in &pending {
                widgets::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        widgets::avatar(ui, &self.name(&l.employee), 11.0);
                        ui.label(RichText::new(self.name(&l.employee)).family(semibold()));
                        widgets::pill(ui, kind_label(l.kind, de), kind_tone(l.kind));
                        ui.label(format!(
                            "{} - {}  ({} {})",
                            date_de(l.from),
                            date_de(l.to),
                            days_txt(l.days),
                            f.t("Tage", "days")
                        ));
                        if !l.note.is_empty() {
                            ui.label(RichText::new(&l.note).color(tk.text_weak));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(f.t("Ablehnen", "Reject")).clicked() {
                                self.reject(f, l);
                            }
                            if widgets::primary(ui, f.t("Genehmigen", "Approve")).clicked() {
                                self.approve(f, l);
                            }
                        });
                    });
                });
                ui.add_space(4.0);
            }
        }
    }

    // ---------------- timesheet ----------------

    fn employee_picker(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        let label = if self.sel.is_empty() {
            "-".to_string()
        } else {
            format!("{}  {}", self.sel, self.name(&self.sel))
        };
        egui::ComboBox::from_id_salt("emp_pick")
            .width(240.0)
            .selected_text(label)
            .show_ui(ui, |ui| {
                for e in self.emps.iter().filter(|e| e.active) {
                    if ui
                        .selectable_label(self.sel == e.code, format!("{}  {}", e.code, e.name))
                        .clicked()
                    {
                        self.sel = e.code.clone();
                        changed = true;
                    }
                }
            });
        changed
    }

    fn view_sheet(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        widgets::page_header(ui, f.t("Arbeitszeitaufzeichnung", "Timesheet"), f.t("Monatsaufzeichnung nach AZG § 26: Beginn, Ende, Pausen, Abwesenheiten, Feiertage, AZG-Befunde.", "Monthly record per AZG section 26: start, end, breaks, absences, holidays, AZG findings."), |_| {});
        if self.emps.is_empty() {
            self.empty(ui, f);
            return;
        }
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            changed |= self.employee_picker(ui);
            if ui.button("<").clicked() {
                self.month = add_months(self.month, -1);
                changed = true;
            }
            ui.label(RichText::new(month_name(self.month, de)).family(semibold()).size(16.0));
            if ui.button(">").clicked() {
                self.month = add_months(self.month, 1);
                changed = true;
            }
            if ui.button(f.t("Heute", "Today")).clicked() {
                self.month = first_of(self.today);
                changed = true;
            }
            ui.separator();
            let sel = self.sel.clone();
            let month = self.month.format("%Y-%m").to_string();
            if widgets::primary(ui, f.t("+ Eintrag", "+ Entry")).clicked() {
                f.form("team_time_add", &[("employee", &sel), ("date", &self.today.to_string())]);
            }
            if ui.button(f.t("Woche bestätigen...", "Confirm week...")).on_hover_text(f.t("Bestätigte Wochen sind schreibgeschützt", "Confirmed weeks become read-only")).clicked() {
                f.form("team_week_confirm", &[("employee", &sel), ("week", &week_label(self.today - Duration::days(7)))]);
            }
            if ui.button(f.t("CSV exportieren...", "Export CSV...")).clicked() {
                f.form("team_timesheet", &[("employee", &sel), ("month", &month)]);
            }
            if ui.button(f.t("Einträge (Bericht)", "Entries report")).clicked() {
                f.run_show("team_time_list", json!({ "employee": sel, "from": self.month.to_string(), "to": last_of(self.month).to_string() }));
            }
        });
        if changed {
            self.refresh_local();
        }
        let Some(sheet) = self.sheet.clone() else {
            ui.label(f.t("Mitarbeiter wählen.", "Pick an employee."));
            return;
        };
        ui.add_space(6.0);
        let worked = sheet["worked_minutes"].as_i64().unwrap_or(0);
        let target = sheet["target_minutes"].as_i64().unwrap_or(0);
        let rows = sheet["rows"].as_array().cloned().unwrap_or_default();
        let flags: usize = rows
            .iter()
            .map(|r| r["azg"].as_array().map_or(0, |a| a.len()))
            .sum();
        let absent = rows.iter().filter(|r| r["absence"].is_string()).count();
        widgets::kpi_row(
            ui,
            &[
                Kpi::new(f.t("Gearbeitet", "Worked"), format!("{} h", fmt_hm(worked))),
                Kpi::new(f.t("Soll", "Target"), format!("{} h", fmt_hm(target))),
                Kpi::new(
                    f.t("Saldo", "Balance"),
                    format!("{} h", signed_hm(worked - target)),
                )
                .note(
                    if worked >= target {
                        f.t("im Plus", "over")
                    } else {
                        f.t("im Minus", "under")
                    },
                    if worked >= target {
                        Tone::Ok
                    } else {
                        Tone::Warn
                    },
                ),
                Kpi::new(f.t("Abwesenheitstage", "Absence days"), absent.to_string()),
                Kpi::new(f.t("AZG-Befunde", "AZG findings"), flags.to_string()).note(
                    if flags > 0 {
                        f.t("prüfen", "review")
                    } else {
                        "ok"
                    },
                    if flags > 0 { Tone::Danger } else { Tone::Ok },
                ),
            ],
        );
        ui.add_space(6.0);
        let cols = vec![
            Col::new(f.t("Datum", "Date"), 1.0).min(92.0),
            Col::new(f.t("Tag", "Day"), 0.5),
            Col::new(f.t("Beginn", "Start"), 0.6),
            Col::new(f.t("Ende", "End"), 0.6),
            Col::num(f.t("Pause", "Break"), 0.6).prio(2),
            Col::num(f.t("Arbeitszeit", "Worked"), 0.8),
            Col::new(f.t("Abwesenheit / Feiertag", "Absence / holiday"), 1.6),
            Col::new("AZG", 1.6).prio(1),
        ];
        let data: Vec<(String, Vec<Cell>)> = rows
            .iter()
            .map(|r| {
                let d = r["date"].as_str().unwrap_or("").to_string();
                let nd = NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok();
                let weekend = nd.is_some_and(|x| x.weekday().num_days_from_monday() >= 5);
                let min = r["minutes"].as_i64().unwrap_or(0);
                let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
                let day_cell = |t: String| {
                    if weekend {
                        Cell::Weak(t)
                    } else {
                        Cell::Text(t)
                    }
                };
                let abs = if let Some(h) = r["holiday"].as_str() {
                    Cell::Pill(h.to_string(), Tone::Violet)
                } else if let Some(a) = r["absence"].as_str() {
                    let k = LeaveKind::parse(a.split(' ').next().unwrap_or(a)).ok();
                    Cell::Pill(
                        k.map(|k| kind_label(k, de).to_string())
                            .unwrap_or_else(|| a.to_string())
                            + if a.contains("half") {
                                f.t(" (halb)", " (half)")
                            } else {
                                ""
                            },
                        k.map(kind_tone).unwrap_or(Tone::Neutral),
                    )
                } else {
                    Cell::Empty
                };
                let azg: Vec<(String, Tone)> = r["azg"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|k| (k.to_string(), Tone::Danger))
                    .collect();
                (
                    d.clone(),
                    vec![
                        Cell::Mono(nd.map(date_de).unwrap_or(d)),
                        day_cell(
                            nd.map(|x| weekday_short(x, de).to_string())
                                .unwrap_or_default(),
                        ),
                        day_cell(s("start")),
                        day_cell(s("end")),
                        if min > 0 {
                            Cell::Mono(format!("{} min", r["break_minutes"].as_i64().unwrap_or(0)))
                        } else {
                            Cell::Empty
                        },
                        if min > 0 {
                            Cell::Strong(fmt_hm(min))
                        } else {
                            Cell::Weak(if weekend { String::new() } else { "-".into() })
                        },
                        abs,
                        if azg.is_empty() {
                            Cell::Empty
                        } else {
                            Cell::Pills(azg)
                        },
                    ],
                )
            })
            .collect();
        widgets::card(ui, |ui| {
            let _ = Table::new(("sheet", &self.sel, self.month), &cols, &data)
                .filter_box(false)
                .show(ui);
        });
        ui.add_space(8.0);
        widgets::section(ui, f.t("Einträge", "Entries"), Some(self.entries.len()));
        let tk = theme::current(ui.ctx());
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            if self.entries.is_empty() {
                ui.label(f.t("Keine Einträge in diesem Monat.", "No entries this month."));
            }
            egui::Grid::new("entries")
                .striped(true)
                .num_columns(6)
                .spacing([18.0, 6.0])
                .show(ui, |ui| {
                    for x in self.entries.iter().rev() {
                        ui.label(
                            RichText::new(format!("#{}", x.id))
                                .family(egui::FontFamily::Monospace)
                                .color(tk.text_weak),
                        );
                        ui.label(
                            RichText::new(x.start.format("%d.%m. %H:%M").to_string())
                                .family(egui::FontFamily::Monospace),
                        );
                        ui.label(
                            RichText::new(
                                x.end
                                    .map(|e| e.format("%H:%M").to_string())
                                    .unwrap_or_else(|| f.t("läuft", "running").into()),
                            )
                            .family(egui::FontFamily::Monospace),
                        );
                        ui.label(format!("{} min", x.break_minutes));
                        ui.label(
                            RichText::new(x.minutes.map(fmt_hm).unwrap_or_default())
                                .family(semibold()),
                        );
                        ui.horizontal(|ui| {
                            if x.locked {
                                widgets::pill(ui, f.t("bestätigt", "confirmed"), Tone::Teal);
                            } else if ui.small_button(f.t("Löschen", "Delete")).clicked() {
                                f.run_confirm(
                                    if de {
                                        format!(
                                            "Eintrag #{} vom {} löschen?",
                                            x.id,
                                            x.start.format("%d.%m.%Y")
                                        )
                                    } else {
                                        format!(
                                            "Delete entry #{} of {}?",
                                            x.id,
                                            x.start.format("%d.%m.%Y")
                                        )
                                    },
                                    f.t("Gelöscht", "Deleted"),
                                    "team_time_delete",
                                    json!({ "id": x.id }),
                                );
                            }
                            if !x.note.is_empty() {
                                ui.label(RichText::new(&x.note).color(tk.text_weak));
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    }

    // ---------------- absence calendar ----------------

    fn view_calendar(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        widgets::page_header(ui, f.t("Abwesenheiten", "Absences"), f.t("Klick auf einen freien Arbeitstag beantragt Abwesenheit ab diesem Tag. Volle Farbe = genehmigt, Umriss = beantragt.", "Click a free working day to request leave from that day. Solid = approved, outline = requested."), |ui| {
            if widgets::primary(ui, f.t("Abwesenheit beantragen", "Request leave")).clicked() {
                f.form("team_leave_request", &[("kind", "vacation")]);
            }
            if ui.button(f.t("Feiertage", "Public holidays")).clicked() {
                f.run_show("team_holidays", json!({ "year": self.cal.year() }));
            }
        });
        if self.emps.is_empty() {
            self.empty(ui, f);
            return;
        }
        ui.horizontal(|ui| {
            if ui.button("<").clicked() {
                self.cal = add_months(self.cal, -1);
            }
            ui.label(
                RichText::new(month_name(self.cal, de))
                    .family(semibold())
                    .size(16.0),
            );
            if ui.button(">").clicked() {
                self.cal = add_months(self.cal, 1);
            }
            if ui.button(f.t("Heute", "Today")).clicked() {
                self.cal = first_of(self.today);
            }
            ui.add_space(16.0);
            for k in [
                LeaveKind::Vacation,
                LeaveKind::Sick,
                LeaveKind::CompTime,
                LeaveKind::Unpaid,
                LeaveKind::Other,
            ] {
                widgets::pill(ui, kind_label(k, de), kind_tone(k));
            }
            widgets::pill(ui, f.t("Feiertag", "Holiday"), Tone::Violet);
        });
        ui.add_space(6.0);
        let tk = theme::current(ui.ctx());
        let first = self.cal;
        let n = (last_of(first) - first).num_days() as usize + 1;
        let hol: Vec<(NaiveDate, &'static str)> =
            holidays::public_holidays(first.year()).unwrap_or_default();
        let name_w = 200.0;
        let cw = ((ui.available_width() - name_w - 4.0) / n as f32).clamp(20.0, 46.0);
        let width = name_w + cw * n as f32;
        let day = |i: usize| first + Duration::days(i as i64);
        let off = |d: NaiveDate| {
            d.weekday().num_days_from_monday() >= 5 || hol.iter().any(|(h, _)| *h == d)
        };
        // Header. Today is a full-height accent band; weekends and holidays a darker column.
        let today_i = (0..n).find(|&i| day(i) == self.today);
        let rows_h = 40.0 + 34.0 * self.emps.iter().filter(|e| e.active).count() as f32;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 40.0), Sense::hover());
        if let Some(i) = today_i {
            let x = rect.left() + name_w + cw * i as f32;
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(cw, rows_h)),
                4.0,
                tk.accent_soft,
            );
        }
        let p = ui.painter_at(rect);
        for i in 0..n {
            let d = day(i);
            let x = rect.left() + name_w + cw * i as f32;
            let r = egui::Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(cw, 40.0));
            let today = d == self.today;
            p.text(
                egui::pos2(r.center().x, r.top() + 13.0),
                Align2::CENTER_CENTER,
                d.day().to_string(),
                FontId::new(14.0, if today { theme::bold() } else { egui::FontFamily::Proportional }),
                if today { tk.accent } else { tk.text },
            );
            p.text(
                egui::pos2(r.center().x, r.top() + 30.0),
                Align2::CENTER_CENTER,
                weekday_short(d, de),
                FontId::proportional(11.5),
                if off(d) { tk.text_weak } else { tk.text },
            );
        }
        let people: Vec<Employee> = self.emps.iter().filter(|e| e.active).cloned().collect();
        for e in &people {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::click());
            let p = ui.painter_at(rect);
            p.line_segment(
                [rect.left_top(), rect.right_top()],
                Stroke::new(1.0, tk.border),
            );
            p.text(
                egui::pos2(rect.left() + 6.0, rect.center().y),
                Align2::LEFT_CENTER,
                &e.name,
                FontId::proportional(14.0),
                tk.text,
            );
            let mine: Vec<&Leave> = self
                .leaves
                .iter()
                .filter(|l| {
                    l.employee == e.code
                        && matches!(l.status, LeaveStatus::Approved | LeaveStatus::Requested)
                })
                .collect();
            for i in 0..n {
                let d = day(i);
                let x = rect.left() + name_w + cw * i as f32;
                let r = egui::Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(cw, 34.0))
                    .shrink2(Vec2::new(1.5, 4.0));
                // Every cell has the same size: working day = bordered surface,
                // weekend = sunken with a weak border, holiday = violet.
                if hol.iter().any(|(h, _)| *h == d) {
                    let (fg, bg) = theme::tone_colors(Tone::Violet, tk.dark);
                    p.rect_filled(r, 4.0, bg);
                    p.rect_stroke(r, 4.0, Stroke::new(1.0, fg), egui::StrokeKind::Inside);
                } else if off(d) {
                    p.rect_filled(r, 4.0, tk.sunken);
                } else {
                    p.rect_filled(r, 4.0, tk.surface);
                    p.rect_stroke(r, 4.0, Stroke::new(1.0, tk.border), egui::StrokeKind::Inside);
                }
                if let Some(l) = mine.iter().find(|l| d >= l.from && d <= l.to && !off(d)) {
                    let (fg, bg) = theme::tone_colors(kind_tone(l.kind), tk.dark);
                    let approved = l.status == LeaveStatus::Approved;
                    if approved {
                        p.rect_filled(r, 4.0, bg);
                    } else {
                        p.rect_filled(r, 4.0, tk.surface);
                    }
                    p.rect_stroke(r, 4.0, Stroke::new(if approved { 1.5 } else { 2.0 }, fg), egui::StrokeKind::Inside);
                    // Letter for the kind (not colour alone), half-day marked.
                    let letter: String = kind_label(l.kind, de).chars().take(1).collect();
                    let label = if l.half_day { format!("{letter}\u{00BD}") } else { letter };
                    if cw >= 16.0 {
                        p.text(
                            r.center(),
                            Align2::CENTER_CENTER,
                            label,
                            FontId::new(11.5, theme::bold()),
                            fg,
                        );
                    }
                }
            }
            let hover_day = resp.hover_pos().and_then(|pos| {
                let i = ((pos.x - rect.left() - name_w) / cw).floor();
                (i >= 0.0 && (i as usize) < n).then(|| day(i as usize))
            });
            if let Some(d) = hover_day {
                let leave = mine.iter().find(|l| d >= l.from && d <= l.to);
                let holiday = hol.iter().find(|(h, _)| *h == d).map(|(_, n)| *n);
                let tip = match (leave, holiday) {
                    (_, Some(h)) => format!("{}  {h}", date_de(d)),
                    (Some(l), None) => format!(
                        "{}  {} {} - {} ({})",
                        e.name,
                        kind_label(l.kind, de),
                        date_de(l.from),
                        date_de(l.to),
                        status_label(l.status, de)
                    ),
                    (None, None) if off(d) => date_de(d),
                    (None, None) => format!(
                        "{}  {}: {}",
                        date_de(d),
                        e.name,
                        f.t("Klick = Abwesenheit beantragen", "click = request leave")
                    ),
                };
                let resp = resp.on_hover_text(tip);
                if resp.clicked() && leave.is_none() && !off(d) {
                    f.form(
                        "team_leave_request",
                        &[
                            ("employee", &e.code),
                            ("from", &d.to_string()),
                            ("to", &d.to_string()),
                            ("kind", "vacation"),
                        ],
                    );
                }
            }
        }
        ui.add_space(10.0);
        let in_month: Vec<&(NaiveDate, &str)> = hol
            .iter()
            .filter(|(d, _)| *d >= first && *d <= last_of(first))
            .collect();
        if !in_month.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(f.t("Feiertage:", "Holidays:")).family(semibold()));
                for (d, n) in in_month {
                    ui.label(format!("{} {n}", date_de(*d)));
                }
            });
        }
    }

    // ---------------- leave board ----------------

    fn view_leave(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        widgets::page_header(ui, f.t("Urlaubsanträge", "Leave requests"), f.t("Karte nach Genehmigt oder Abgelehnt ziehen entscheidet den Antrag; nach Storniert zieht ihn zurück.", "Drag a card to Approved or Rejected to decide; to Cancelled to withdraw it."), |ui| {
            if widgets::primary(ui, f.t("Abwesenheit beantragen", "Request leave")).clicked() {
                f.form("team_leave_request", &[("kind", "vacation")]);
            }
            if ui.button(f.t("Bericht", "Report")).clicked() {
                f.run_show("team_leave_list", json!({ "year": self.today.year() }));
            }
        });
        if self.emps.is_empty() {
            self.empty(ui, f);
            return;
        }
        let year = self.today.year();
        let columns = vec![
            Column::new(
                "requested",
                status_label(LeaveStatus::Requested, de),
                Tone::Warn,
            ),
            Column::new(
                "approved",
                status_label(LeaveStatus::Approved, de),
                Tone::Ok,
            ),
            Column::new(
                "rejected",
                status_label(LeaveStatus::Rejected, de),
                Tone::Danger,
            ),
            Column::new(
                "cancelled",
                status_label(LeaveStatus::Cancelled, de),
                Tone::Neutral,
            ),
        ];
        let shown: Vec<&Leave> = self
            .leaves
            .iter()
            .filter(|l| l.from.year() >= year - 1 || l.status == LeaveStatus::Requested)
            .filter(|l| {
                app::search_hit(&f.search, &[&l.employee, &self.name(&l.employee), &l.note])
            })
            .collect();
        let cards: Vec<Card> = shown
            .iter()
            .map(|l| {
                let over = l.status == LeaveStatus::Requested
                    && l.kind == LeaveKind::Vacation
                    && self
                        .balance(&l.employee)
                        .is_some_and(|b| b.remaining_half_days - l.half_days < 0);
                let mut tags = vec![(kind_label(l.kind, de).to_string(), kind_tone(l.kind))];
                if l.half_day {
                    tags.push((f.t("halber Tag", "half day").into(), Tone::Neutral));
                }
                if over {
                    tags.push((f.t("über Guthaben", "over balance").into(), Tone::Danger));
                }
                let mut meta = vec![format!("{} - {}", date_de(l.from), date_de(l.to))];
                if let Some(b) = self.balance(&l.employee) {
                    if l.kind == LeaveKind::Vacation {
                        meta.push(format!(
                            "{} {} {}",
                            f.t("Rest", "left"),
                            days_txt(b.remaining_days),
                            f.t("Tage", "days")
                        ));
                    }
                }
                if !l.note.is_empty() {
                    meta.push(l.note.clone());
                }
                Card {
                    id: l.id.to_string(),
                    column: l.status.as_str().into(),
                    lane: String::new(),
                    key: format!("#{}", l.id),
                    title: self.name(&l.employee),
                    tags,
                    accent: kind_tone(l.kind),
                    badge: Some((
                        format!("{} {}", days_txt(l.days), f.t("T", "d")),
                        Tone::Neutral,
                    )),
                    assignee: Some(self.name(&l.employee)),
                    meta,
                    blocked: None,
                    done: matches!(l.status, LeaveStatus::Rejected | LeaveStatus::Cancelled),
                }
            })
            .collect();
        let states: Vec<(String, LeaveStatus)> =
            shown.iter().map(|l| (l.id.to_string(), l.status)).collect();
        let msg_done = f
            .t(
                "Entschiedene Anträge bleiben, wie sie sind",
                "Decided requests stay as they are",
            )
            .to_string();
        let msg_back = f
            .t(
                "Zurück auf Beantragt geht nicht; neu beantragen",
                "Cannot go back to Requested; file a new request",
            )
            .to_string();
        let msg_approved = f
            .t(
                "Genehmigt kann nur storniert werden",
                "Approved leave can only be cancelled",
            )
            .to_string();
        let rule = move |c: &Card, to: &str| -> Result<(), String> {
            let st = states
                .iter()
                .find(|(k, _)| *k == c.id)
                .map(|(_, s)| *s)
                .unwrap_or(LeaveStatus::Cancelled);
            match (st, to) {
                (LeaveStatus::Requested, "approved" | "rejected" | "cancelled") => Ok(()),
                (LeaveStatus::Approved, "cancelled") => Ok(()),
                (_, "requested") => Err(msg_back.clone()),
                (LeaveStatus::Approved, _) => Err(msg_approved.clone()),
                _ => Err(msg_done.clone()),
            }
        };
        let events = {
            let r: kanban::Rule = &rule;
            Board::new("leave_board", &columns, &cards)
                .rule(r)
                .add_buttons(false)
                .show(ui)
        };
        for e in events {
            match e {
                kanban::Event::Moved { card, to, from, .. } if to != from => {
                    let Some(l) = card
                        .parse::<i64>()
                        .ok()
                        .and_then(|id| self.leaves.iter().find(|l| l.id == id))
                        .cloned()
                    else {
                        continue;
                    };
                    match to.as_str() {
                        "approved" => self.approve(f, &l),
                        "rejected" => self.reject(f, &l),
                        "cancelled" => f.run_confirm(
                            if de {
                                format!(
                                    "Abwesenheit #{} von {} stornieren?",
                                    l.id,
                                    self.name(&l.employee)
                                )
                            } else {
                                format!("Cancel leave #{} of {}?", l.id, self.name(&l.employee))
                            },
                            status_label(LeaveStatus::Cancelled, de),
                            "team_leave_cancel",
                            json!({ "id": l.id }),
                        ),
                        _ => {}
                    }
                }
                kanban::Event::Refused { reason, .. } => f.toast(reason, Tone::Warn),
                kanban::Event::Open(k) => {
                    if let Some(l) = k
                        .parse::<i64>()
                        .ok()
                        .and_then(|id| self.leaves.iter().find(|l| l.id == id))
                    {
                        self.sel = l.employee.clone();
                        self.cal = first_of(l.from);
                        f.goto(V_CALENDAR);
                    }
                }
                _ => {}
            }
        }
    }

    // ---------------- balances ----------------

    fn view_balance(&mut self, ui: &mut Ui, f: &mut Frame) {
        widgets::page_header(
            ui,
            f.t("Urlaubskonten", "Vacation balances"),
            &format!("{} {}", f.t("Jahr", "Year"), self.today.year()),
            |ui| {
                if ui
                    .button(f.t("Korrektur...", "Adjust..."))
                    .on_hover_text(f.t(
                        "Übertrag, Sondertage, mit Begründung",
                        "Carry-over, extra days, with a reason",
                    ))
                    .clicked()
                {
                    f.form(
                        "team_leave_adjust",
                        &[("year", &self.today.year().to_string())],
                    );
                }
                if ui.button(f.t("Bericht", "Report")).clicked() {
                    f.run_show("team_leave_balance", json!({ "year": self.today.year() }));
                }
            },
        );
        if self.emps.is_empty() {
            self.empty(ui, f);
            return;
        }
        let tk = theme::current(ui.ctx());
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new("balances")
                .striped(true)
                .num_columns(9)
                .spacing([20.0, 10.0])
                .show(ui, |ui| {
                    for h in [
                        f.t("Mitarbeiter", "Employee"),
                        f.t("Anspruch", "Entitlement"),
                        f.t("Korrektur", "Adjusted"),
                        f.t("Genommen", "Taken"),
                        f.t("Beantragt", "Pending"),
                        f.t("Rest", "Left"),
                        "",
                        f.t("Krank", "Sick"),
                        f.t("ZA", "Comp"),
                    ] {
                        ui.label(RichText::new(h).family(semibold()).color(tk.text_weak));
                    }
                    ui.end_row();
                    for b in &self.balances {
                        let name = self.name(&b.employee);
                        ui.horizontal(|ui| {
                            widgets::avatar(ui, &name, 10.0);
                            ui.label(RichText::new(name).family(semibold()));
                        });
                        ui.label(days_txt(b.entitlement_days));
                        ui.label(if b.adjustment_days == 0.0 {
                            "-".into()
                        } else {
                            days_txt(b.adjustment_days)
                        });
                        ui.label(days_txt(b.taken_days));
                        ui.label(days_txt(b.pending_days));
                        let tone = if b.remaining_days < 0.0 {
                            Tone::Danger
                        } else if b.remaining_days <= 3.0 {
                            Tone::Warn
                        } else {
                            Tone::Ok
                        };
                        widgets::pill(ui, &days_txt(b.remaining_days), tone);
                        let total = (b.entitlement_days + b.adjustment_days).max(0.01);
                        widgets::meter(
                            ui,
                            ((b.taken_days + b.pending_days) / total).clamp(0.0, 1.0) as f32,
                            140.0,
                            tone,
                        )
                        .on_hover_text(f.t("Verbraucht inkl. beantragt", "Used incl. pending"));
                        ui.label(days_txt(b.sick_days));
                        ui.label(days_txt(b.comp_time_days));
                        ui.end_row();
                    }
                });
        });
        ui.label(
            RichText::new(f.t(
                "Krankenstand wird nur als Tage gezählt, ohne Diagnose oder Notiz.",
                "Sick leave is counted as days only, without a diagnosis or note.",
            ))
            .color(tk.text_weak),
        );
    }

    // ---------------- capacity ----------------

    fn view_capacity(&mut self, ui: &mut Ui, f: &mut Frame) {
        let label = week_label(self.week);
        widgets::page_header(ui, f.t("Kapazität", "Capacity"), f.t("Verfügbare Stunden pro Woche nach Feiertagen und genehmigter Abwesenheit (5-Tage-Woche).", "Available hours per week after holidays and approved leave (5-day week)."), |ui| {
            if widgets::primary(ui, f.t("An taskcraft senden", "Send to taskcraft")).on_hover_text("capacity.changed").clicked() {
                f.run_show("team_capacity", json!({ "week": label, "emit": true }));
            }
        });
        if self.emps.is_empty() {
            self.empty(ui, f);
            return;
        }
        let mut changed = false;
        ui.horizontal(|ui| {
            if ui.button("<").clicked() {
                self.week -= Duration::days(7);
                changed = true;
            }
            ui.label(
                RichText::new(format!(
                    "{label}  ({} - {})",
                    date_de(self.week),
                    date_de(self.week + Duration::days(6))
                ))
                .family(semibold())
                .size(16.0),
            );
            if ui.button(">").clicked() {
                self.week += Duration::days(7);
                changed = true;
            }
            if ui.button(f.t("Diese Woche", "This week")).clicked() {
                self.week = monday(self.today);
                changed = true;
            }
        });
        if changed {
            self.refresh_local();
        }
        let Some(cap) = self.cap.clone() else { return };
        let rows = cap["employees"].as_array().cloned().unwrap_or_default();
        let contract: i64 = self
            .emps
            .iter()
            .filter(|e| e.active)
            .map(|e| e.weekly_minutes)
            .sum();
        let total = cap["total_minutes"].as_i64().unwrap_or(0);
        widgets::kpi_row(
            ui,
            &[
                Kpi::new(
                    f.t("Verfügbar", "Available"),
                    format!("{} h", fmt_hm(total)),
                ),
                Kpi::new(
                    f.t("Vertraglich", "Contracted"),
                    format!("{} h", fmt_hm(contract)),
                ),
                Kpi::new(
                    f.t("Arbeitstage", "Working days"),
                    cap["working_days"].to_string(),
                ),
            ],
        );
        ui.add_space(6.0);
        let tk = theme::current(ui.ctx());
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new("cap")
                .striped(true)
                .num_columns(5)
                .spacing([24.0, 10.0])
                .show(ui, |ui| {
                    for h in [
                        f.t("Mitarbeiter", "Employee"),
                        f.t("Vertrag", "Contract"),
                        f.t("Abwesend", "Absent"),
                        f.t("Verfügbar", "Available"),
                        "",
                    ] {
                        ui.label(RichText::new(h).family(semibold()).color(tk.text_weak));
                    }
                    ui.end_row();
                    for r in &rows {
                        let code = r["employee"].as_str().unwrap_or("");
                        let wm = self
                            .emps
                            .iter()
                            .find(|e| e.code == code)
                            .map_or(1, |e| e.weekly_minutes.max(1));
                        let am = r["available_minutes"].as_i64().unwrap_or(0);
                        let name = r["name"].as_str().unwrap_or(code).to_string();
                        ui.horizontal(|ui| {
                            widgets::avatar(ui, &name, 10.0);
                            ui.label(RichText::new(name).family(semibold()));
                        });
                        ui.label(format!("{} h", r["contract"].as_str().unwrap_or("")));
                        let ld = r["leave_days"].as_f64().unwrap_or(0.0);
                        ui.label(if ld > 0.0 {
                            format!("{} {}", days_txt(ld), f.t("Tage", "days"))
                        } else {
                            "-".into()
                        });
                        ui.label(
                            RichText::new(format!("{} h", r["available"].as_str().unwrap_or("")))
                                .family(semibold()),
                        );
                        let frac = am as f32 / wm as f32;
                        widgets::meter(
                            ui,
                            frac.clamp(0.0, 1.0),
                            180.0,
                            if frac >= 0.8 {
                                Tone::Ok
                            } else if frac >= 0.4 {
                                Tone::Warn
                            } else {
                                Tone::Danger
                            },
                        );
                        ui.end_row();
                    }
                });
        });
    }

    // ---------------- AZG ----------------

    fn view_azg(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        widgets::page_header(ui, f.t("Arbeitszeitgesetz", "Working time law (AZG)"), f.t("10 h/12 h täglich, 60 h pro Woche, 48 h Schnitt über 17 Wochen, 30 min Pause ab 6 h, 11 h Ruhezeit. Befunde werden festgehalten, nie automatisch korrigiert.", "10 h/12 h daily, 60 h weekly, 48 h average over 17 weeks, 30 min break above 6 h, 11 h rest. Findings are recorded, never auto-corrected."), |ui| {
            if widgets::primary(ui, f.t("Prüfung starten...", "Run check...")).clicked() {
                f.form("team_azg_check", &[("from", &first_of(self.today).to_string()), ("to", &self.today.to_string())]);
            }
            if ui.button(f.t("Bericht", "Report")).clicked() {
                f.run_show("team_violations", json!({ "include_cleared": self.show_cleared }));
            }
        });
        if ui
            .checkbox(
                &mut self.show_cleared,
                f.t("Erledigte zeigen", "Show cleared"),
            )
            .changed()
        {
            self.refresh_local();
        }
        if self.violations.is_empty() {
            widgets::card(ui, |ui| {
                widgets::empty_state(
                    ui,
                    f.t("Keine Befunde", "No findings"),
                    f.t(
                        "Prüfung starten, um einen Zeitraum zu kontrollieren.",
                        "Run a check to review a period.",
                    ),
                )
            });
            return;
        }
        let cols = vec![
            Col::new(f.t("Datum", "Date"), 0.9).min(92.0),
            Col::new(f.t("Mitarbeiter", "Employee"), 1.4),
            Col::new(f.t("Art", "Kind"), 1.2),
            Col::new(f.t("Schwere", "Severity"), 0.9),
            Col::num(f.t("Minuten", "Minutes"), 0.6).prio(2),
            Col::new(f.t("Detail", "Detail"), 2.8).prio(1),
            Col::new("Status", 0.8).prio(3),
        ];
        let data: Vec<(String, Vec<Cell>)> = self
            .violations
            .iter()
            .filter(|v| app::search_hit(&f.search, &[&v.employee, &v.kind, &v.detail]))
            .map(|v| {
                (
                    v.id.to_string(),
                    vec![
                        Cell::Mono(date_de(v.date)),
                        Cell::Person(self.name(&v.employee)),
                        Cell::Strong(v.kind.clone()),
                        Cell::Pill(
                            if de {
                                if v.severity == "violation" {
                                    "Verstoß".into()
                                } else {
                                    "Warnung".into()
                                }
                            } else {
                                v.severity.clone()
                            },
                            sev_tone(&v.severity),
                        ),
                        Cell::Mono(v.minutes.to_string()),
                        Cell::Text(v.detail.clone()),
                        if v.cleared_at.is_some() {
                            Cell::Pill(f.t("erledigt", "cleared").into(), Tone::Ok)
                        } else {
                            Cell::Dot(sev_tone(&v.severity))
                        },
                    ],
                )
            })
            .collect();
        let mut pick = None;
        widgets::card(ui, |ui| {
            for e in Table::new("azg", &cols, &data).show(ui) {
                let (TableEvent::Select(k) | TableEvent::Open(k)) = e;
                pick = Some(k);
            }
        });
        if let Some(v) = pick.and_then(|k| self.violations.iter().find(|v| v.id.to_string() == k)) {
            self.sel = v.employee.clone();
            self.month = first_of(v.date);
            self.refresh_local();
            f.goto(V_SHEET);
        }
    }

    // ---------------- team ----------------

    fn view_team(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        let sel = self.team_sel.clone();
        widgets::page_header(
            ui,
            "Team",
            f.t(
                "Nur arbeitsrelevante Daten: keine SV-Nummer, Adresse oder Bankdaten.",
                "Work-relevant data only: no social security number, address or bank data.",
            ),
            |ui| {
                if widgets::primary(ui, f.t("+ Mitarbeiter", "+ Employee")).clicked() {
                    f.form("team_employee_add", &[]);
                }
                if ui
                    .add_enabled(
                        sel.is_some(),
                        egui::Button::new(f.t("Bearbeiten...", "Edit...")),
                    )
                    .on_disabled_hover_text(f.t("Zeile wählen", "Select a row"))
                    .clicked()
                {
                    if let Some(c) = &sel {
                        f.form("team_employee_update", &[("code", c)]);
                    }
                }
                if ui.button(f.t("Als Liste", "As list")).clicked() {
                    f.run_show("team_employee_list", json!({ "include_inactive": true }));
                }
            },
        );
        if self.emps.is_empty() {
            self.empty(ui, f);
            return;
        }
        let cols = vec![
            Col::new(f.t("Kürzel", "Code"), 0.5),
            Col::new("Name", 1.8),
            Col::num(f.t("Std/Woche", "Hours/wk"), 0.8),
            Col::num(f.t("Urlaub/Jahr", "Leave/yr"), 0.8).prio(2),
            Col::new(f.t("Eintritt", "Start"), 0.9).prio(3),
            Col::new("KV", 0.8).prio(1),
            Col::num(f.t("Brutto/Monat", "Gross/month"), 1.0).prio(1),
            Col::new("Status", 0.8),
        ];
        let data: Vec<(String, Vec<Cell>)> = self
            .emps
            .iter()
            .filter(|e| app::search_hit(&f.search, &[&e.code, &e.name]))
            .map(|e| {
                (
                    e.code.clone(),
                    vec![
                        Cell::Mono(e.code.clone()),
                        Cell::Person(e.name.clone()),
                        Cell::Mono(fmt_hm(e.weekly_minutes)),
                        Cell::Mono(e.leave_days_per_year.to_string()),
                        Cell::Mono(date_de(e.start_date)),
                        Cell::Text(e.kv_group.clone().unwrap_or_else(|| "-".into())),
                        match e.monthly_gross_cents {
                            Some(c) => Cell::Money(craft_biz::Money::from_cents(c).format_de()),
                            None => Cell::Weak("-".into()),
                        },
                        if e.active {
                            Cell::Pill(f.t("aktiv", "active").into(), Tone::Ok)
                        } else {
                            Cell::Pill(f.t("ausgetreten", "left").into(), Tone::Neutral)
                        },
                    ],
                )
            })
            .collect();
        let mut open_sheet = None;
        widgets::card(ui, |ui| {
            for e in Table::new("team", &cols, &data)
                .selected(sel.as_deref())
                .show(ui)
            {
                match e {
                    TableEvent::Select(k) => self.team_sel = Some(k),
                    TableEvent::Open(k) => open_sheet = Some(k),
                }
            }
        });
        if let Some(k) = open_sheet {
            self.sel = k;
            self.month = first_of(self.today);
            self.refresh_local();
            f.goto(V_SHEET);
        }
        ui.add_space(10.0);
        widgets::page_header(ui, f.t("KV-Mindestlohn", "KV minimum wage"), f.t("Eigene Tabelle, keine KV-Werte im Programm. Vergleich anteilig zur Wochenstundenzahl.", "Your own table; no KV figures ship with the app. Compared pro rata to weekly hours."), |ui| {
            if ui.button(f.t("KV-Tabelle importieren...", "Import KV table...")).clicked() {
                f.form("team_kv_import", &[]);
            }
            if ui.button(f.t("KV-Tabelle", "KV table")).clicked() {
                f.run_show("team_kv_list", json!({}));
            }
            if ui.button(f.t("Prüfbericht", "Check report")).clicked() {
                f.run_show("team_kv_check", json!({}));
            }
        });
        let rows = self
            .kv
            .as_ref()
            .and_then(|k| k["rows"].as_array().cloned())
            .unwrap_or_default();
        if rows.is_empty() {
            widgets::card(ui, |ui| {
                widgets::empty_state(ui, f.t("Kein KV-Vergleich", "No KV comparison"), f.t("KV-Tabelle importieren und bei Mitarbeitern KV, Gruppe und Brutto eintragen.", "Import a KV table and set KV, group and gross on employees."))
            });
            return;
        }
        let kcols = vec![
            Col::new(f.t("Mitarbeiter", "Employee"), 1.6),
            Col::new(f.t("Gruppe", "Group"), 0.6),
            Col::new(f.t("Stunden", "Hours"), 1.0).prio(2),
            Col::num(f.t("Brutto", "Gross"), 1.0),
            Col::num(f.t("Minimum anteilig", "Minimum pro rata"), 1.1),
            Col::num(f.t("Fehlbetrag", "Shortfall"), 1.0),
            Col::new("Status", 0.9),
        ];
        let kdata: Vec<(String, Vec<Cell>)> = rows
            .iter()
            .map(|r| {
                let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
                let ok = r["status"] == "ok";
                (
                    s("employee"),
                    vec![
                        Cell::Person(self.name(&s("employee"))),
                        Cell::Text(s("group")),
                        Cell::Weak(s("hours")),
                        Cell::Money(s("gross")),
                        Cell::Money(s("minimum_pro_rata")),
                        if ok {
                            Cell::Weak("-".into())
                        } else {
                            Cell::Money(s("shortfall"))
                        },
                        if ok {
                            Cell::Pill("ok".into(), Tone::Ok)
                        } else {
                            Cell::Pill(
                                if de {
                                    "unter KV".into()
                                } else {
                                    "underpaid".into()
                                },
                                Tone::Danger,
                            )
                        },
                    ],
                )
            })
            .collect();
        widgets::card(ui, |ui| {
            let _ = Table::new("kv", &kcols, &kdata).filter_box(false).show(ui);
        });
    }

    fn view_settings(&mut self, ui: &mut Ui, f: &mut Frame) {
        widgets::page_header(ui, f.t("Einstellungen", "Settings"), &self.db, |_| {});
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::field(ui, f.t("Datenbank", "Database"), |ui| {
                ui.label(RichText::new(&self.db).family(egui::FontFamily::Monospace));
            });
            widgets::field(ui, f.t("Benutzer", "Actor"), |ui| {
                ui.label(&self.ctx.actor);
            });
            ui.horizontal(|ui| {
                if ui.button("Status").clicked() {
                    f.run_show("team_status", json!({}));
                }
                if ui.button(f.t("Feiertage", "Public holidays")).clicked() {
                    f.run_show("team_holidays", json!({ "year": self.today.year() }));
                }
                if ui
                    .add_enabled(
                        self.emps.is_empty(),
                        egui::Button::new(f.t("Demo-Daten laden", "Load demo data")),
                    )
                    .on_disabled_hover_text("only on an empty database")
                    .clicked()
                {
                    f.run(f.t("Demo-Daten", "Demo data"), "team_demo_seed", json!({}));
                }
            });
        });
    }
}

impl SuiteApp for TeamApp {
    fn name(&self) -> &'static str {
        "TeamCraft"
    }
    fn id(&self) -> &'static str {
        teamcraft_core::APP
    }
    fn tone(&self) -> Tone {
        Tone::Teal
    }
    fn version(&self) -> &'static str {
        teamcraft_core::VERSION
    }
    fn tools(&self) -> Vec<Value> {
        tools()
    }
    fn call(&mut self, tool: &str, args: &Value) -> Result<Value, String> {
        let cmd = Command::from_tool(tool, args)?;
        dispatch(&mut self.ctx, cmd)
    }
    fn reload(&mut self) -> Result<(), String> {
        self.today = Local::now().date_naive();
        let now = Local::now().time();
        self.now_min = i64::from(now.hour() * 60 + now.minute());
        let t = &self.ctx.team;
        let er = |e: teamcraft_store::TeamError| e.to_string();
        self.emps = t.employees(true).map_err(er)?;
        // All open entries, however old: a forgotten clock-in must stay visible.
        self.open = t.open_entries().map_err(er)?;
        self.today_entries = t.entries(None, self.today, self.today).map_err(er)?;
        self.leaves = t.leaves(None, None, None, None).map_err(er)?;
        self.leaves.sort_by_key(|l| l.from);
        let year = self.today.year();
        let mut bal = Vec::new();
        for e in self.emps.iter().filter(|e| e.active) {
            bal.push(t.balance(&e.code, year).map_err(er)?);
        }
        self.balances = bal;
        self.kv = kv_check(t, self.today).ok();
        if self.sel.is_empty() || !self.emps.iter().any(|e| e.code == self.sel) {
            self.sel = self
                .emps
                .iter()
                .find(|e| e.active)
                .map(|e| e.code.clone())
                .unwrap_or_default();
        }
        self.refresh_local();
        Ok(())
    }
    fn nav(&self, f: &Frame) -> Vec<NavEntry> {
        let requests = self
            .leaves
            .iter()
            .filter(|l| l.status == LeaveStatus::Requested)
            .count();
        let findings = self
            .violations
            .iter()
            .filter(|v| v.cleared_at.is_none())
            .count();
        let under = self
            .kv
            .as_ref()
            .and_then(|k| k["underpaid"].as_u64())
            .unwrap_or(0) as usize;
        vec![
            NavEntry::new("\u{23F1}", f.t("Stempeluhr", "Time clock"))
                .group(f.t("Zeit", "Time"))
                .badge(self.open.len(), Tone::Ok),
            NavEntry::new("\u{2630}", f.t("Zeiterfassung", "Timesheet")),
            NavEntry::new("\u{25A6}", f.t("Abwesenheiten", "Absences"))
                .group(f.t("Abwesenheit", "Absence")),
            NavEntry::new("\u{2709}", f.t("Urlaubsanträge", "Leave requests"))
                .badge(requests, Tone::Warn),
            NavEntry::new("\u{2696}", f.t("Urlaubskonten", "Balances")),
            NavEntry::new("\u{25D4}", f.t("Kapazität", "Capacity"))
                .group(f.t("Planung und Recht", "Planning and law")),
            NavEntry::new("\u{26A0}", f.t("Arbeitszeitgesetz", "Working time law"))
                .badge(findings, Tone::Danger),
            NavEntry::new("\u{263A}", "Team").badge(under, Tone::Danger),
            NavEntry::new("\u{2699}", f.t("Einstellungen", "Settings")),
        ]
    }
    fn view(&mut self, ui: &mut Ui, view: usize, f: &mut Frame) {
        egui::ScrollArea::vertical()
            .id_salt(("team_view", view))
            .auto_shrink([false, false])
            .show(ui, |ui| match view {
                V_TODAY => self.view_today(ui, f),
                V_SHEET => self.view_sheet(ui, f),
                V_CALENDAR => self.view_calendar(ui, f),
                V_LEAVE => self.view_leave(ui, f),
                V_BALANCE => self.view_balance(ui, f),
                V_CAPACITY => self.view_capacity(ui, f),
                V_AZG => self.view_azg(ui, f),
                V_TEAM => self.view_team(ui, f),
                V_SETTINGS => self.view_settings(ui, f),
                _ => self.view_today(ui, f),
            });
    }
    fn top_actions(&mut self, ui: &mut Ui, f: &mut Frame) {
        if widgets::primary(ui, f.t("+ Abwesenheit", "+ Leave")).clicked() {
            f.form(
                "team_leave_request",
                &[("employee", &self.sel), ("kind", "vacation")],
            );
        }
        if ui.button(f.t("+ Zeit", "+ Time")).clicked() {
            f.form(
                "team_time_add",
                &[("employee", &self.sel), ("date", &self.today.to_string())],
            );
        }
    }
    fn bus(&self) -> &Bus {
        &self.ctx.bus
    }
    fn wiring(&self) -> Wiring {
        (
            vec![],
            vec![(
                "capacity.changed",
                "to taskcraft: available hours per person and week",
            )],
        )
    }
    fn prefill(&self) -> Vec<(String, String)> {
        if self.sel.is_empty() {
            vec![]
        } else {
            vec![("employee".to_string(), self.sel.clone())]
        }
    }
    fn select(&mut self, key: &str) {
        let k = key.trim().to_ascii_uppercase();
        if self.emps.iter().any(|e| e.code == k) {
            self.sel = k.clone();
            self.team_sel = Some(k);
            self.refresh_local();
        }
    }
    fn status_info(&self) -> String {
        format!(
            "{}  |  {} employees",
            self.db,
            self.emps.iter().filter(|e| e.active).count()
        )
    }
    fn smoke_tool(&self) -> &'static str {
        "team_status"
    }
}

fn default_db() -> PathBuf {
    if let Ok(p) = std::env::var("TEAMCRAFT_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("teamcraft")
        .join("teamcraft.db")
}

fn open(db: &Path) -> Result<TeamApp, String> {
    let team = Team::open(db).map_err(|e| e.to_string())?;
    let actor = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "desktop".into());
    let ctx = Ctx {
        team,
        confinement: Confinement(None),
        bus: Bus::new(Bus::default_root()),
        actor,
    };
    Ok(TeamApp::new(ctx, db.display().to_string()))
}

fn main() -> std::process::ExitCode {
    app::main_with(default_db(), open, "team_demo_seed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_biz_ui::app::Shell;
    use egui_kittest::Harness;

    fn shell(tag: &str) -> (Shell<TeamApp>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("teamcraft-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = Ctx {
            team: Team::open_in_memory().unwrap(),
            confinement: Confinement(None),
            bus: Bus::new(dir.join("bus")),
            actor: "Mia Mockerl".into(),
        };
        let mut app = TeamApp::new(ctx, ":memory:".into());
        app.call("team_demo_seed", &json!({})).unwrap();
        (Shell::new(app, None), dir)
    }

    #[test]
    fn every_tool_has_a_place_in_the_window() {
        craft_biz_ui::coverage::check(&tools(), COVERAGE).unwrap();
    }

    #[test]
    fn every_view_renders_in_both_themes() {
        let (mut sh, dir) = shell("render");
        sh.app.month = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        sh.app.cal = sh.app.month;
        sh.app.refresh_local();
        let mut h = Harness::builder()
            .with_size([1500.0, 950.0])
            .build_ui_state(|ui, sh: &mut Shell<TeamApp>| sh.show(ui), sh);
        for dark in [false, true] {
            craft_biz_ui::theme::apply(&h.ctx, dark);
            for v in 0..9 {
                h.state_mut().view = v;
                h.run_steps(3);
            }
        }
        assert!(h.state().last_error.is_none(), "{:?}", h.state().last_error);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn timesheet_of_the_demo_month_has_totals_and_azg_flags() {
        let (mut sh, dir) = shell("sheet");
        sh.app.select("jm");
        sh.app.month = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        sh.app.refresh_local();
        let s = sh.app.sheet.clone().unwrap();
        assert_eq!(s["employee"], "JM");
        assert!(s["worked_minutes"].as_i64().unwrap() > 0);
        assert_eq!(s["rows"].as_array().unwrap().len(), 30);
        assert!(!sh.app.entries.is_empty());
        assert_eq!(
            sh.app.prefill(),
            vec![("employee".to_string(), "JM".to_string())]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clock_in_and_out_go_through_dispatch() {
        let (mut sh, dir) = shell("clock");
        // Explicit times: clocking out in the same minute is refused by the core (end must be after start).
        sh.run_tool(
            "team_clock_in",
            json!({ "employee": "AM", "at": "2027-01-05 08:00" }),
        );
        assert!(
            sh.app.open.iter().any(|x| x.employee == "AM"),
            "{:?}",
            sh.last_error
        );
        sh.run_tool(
            "team_clock_out",
            json!({ "employee": "AM", "at": "2027-01-05 12:00", "break_minutes": 0 }),
        );
        assert!(
            !sh.app.open.iter().any(|x| x.employee == "AM"),
            "{:?}",
            sh.last_error
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn leave_board_refuses_reopening_and_approves_a_request() {
        let (mut sh, dir) = shell("board");
        sh.run_tool("team_leave_request", json!({ "employee": "JM", "kind": "vacation", "from": "2026-12-14", "to": "2026-12-15" }));
        let req = sh
            .app
            .leaves
            .iter()
            .find(|l| l.employee == "JM" && l.status == LeaveStatus::Requested)
            .map(|l| l.id)
            .unwrap();
        let approved = sh
            .app
            .leaves
            .iter()
            .find(|l| l.status == LeaveStatus::Approved)
            .map(|l| l.id)
            .unwrap();
        sh.view = V_LEAVE;
        let mut h = Harness::builder()
            .with_size([1600.0, 1000.0])
            .build_ui_state(|ui, sh: &mut Shell<TeamApp>| sh.show(ui), sh);
        h.run_steps(3);
        let drag = |h: &mut Harness<'_, Shell<TeamApp>>, id: i64, col: &str| {
            let card = kanban::card_rect(&h.ctx, "leave_board", &id.to_string()).unwrap();
            let target = kanban::cell_rect(&h.ctx, "leave_board", col, "").unwrap();
            h.hover_at(card.center());
            h.run_steps(1);
            h.drag_at(card.center());
            h.run_steps(1);
            for i in 1..=8 {
                h.hover_at(card.center() + (target.center() - card.center()) * (i as f32 / 8.0));
                h.run_steps(1);
            }
            h.drop_at(target.center());
            h.run_steps(3);
        };
        drag(&mut h, approved, "requested");
        assert_eq!(
            h.state()
                .app
                .leaves
                .iter()
                .find(|l| l.id == approved)
                .map(|l| l.status),
            Some(LeaveStatus::Approved)
        );
        drag(&mut h, req, "approved");
        assert_eq!(
            h.state()
                .app
                .leaves
                .iter()
                .find(|l| l.id == req)
                .map(|l| l.status),
            Some(LeaveStatus::Approved),
            "{:?}",
            h.state().last_error
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
