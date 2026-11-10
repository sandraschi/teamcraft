//! teamcraft-app: Desktop workstation for Austrian working time (AZG) & leave management.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Duration, Local, NaiveDate};
use clap::Parser;
use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;
use teamcraft_core::azg_check;
use teamcraft_store::{fmt_hm, Employee, Leave, LeaveKind, LeaveStatus, Team, TimeEntry, Violation};

#[derive(Parser, Debug)]
#[command(name = "teamcraft-app", version, about = "Desktop UI for Austrian working time (AZG) & leave management")]
struct Cli {
    /// Database file path.
    #[arg(long)]
    db: Option<PathBuf>,
    /// Smoke test: open DB, verify schemas, and exit immediately.
    #[arg(long)]
    smoke: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTab {
    Timesheet,
    AzgCompliance,
    AbsenceLeave,
    Roster,
}

fn render_badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.colored_label(color, format!("[{text}]"));
}

struct AppState {
    team: Team,
    current_tab: ActiveTab,
    status_msg: Option<String>,

    // Cached data
    employees: Vec<Employee>,
    selected_employee: Option<String>,
    time_entries: Vec<TimeEntry>,
    violations: Vec<Violation>,
    leaves: Vec<Leave>,

    // Timesheet date window
    view_date: NaiveDate,

    // Clock in/out inputs
    clock_note: String,
    clock_break_minutes: i64,

    // Quick leave request inputs
    leave_emp: String,
    leave_kind: LeaveKind,
    leave_start: String,
    leave_end: String,
    leave_half: bool,
    leave_note: String,
}

impl AppState {
    fn new(team: Team) -> Self {
        let today = Local::now().date_naive();
        let mut app = Self {
            team,
            current_tab: ActiveTab::Timesheet,
            status_msg: None,
            employees: Vec::new(),
            selected_employee: None,
            time_entries: Vec::new(),
            violations: Vec::new(),
            leaves: Vec::new(),
            view_date: today,
            clock_note: String::new(),
            clock_break_minutes: 30,
            leave_emp: String::new(),
            leave_kind: LeaveKind::Vacation,
            leave_start: today.format("%Y-%m-%d").to_string(),
            leave_end: today.format("%Y-%m-%d").to_string(),
            leave_half: false,
            leave_note: String::new(),
        };
        app.reload_data();
        app
    }

    fn reload_data(&mut self) {
        if let Ok(emps) = self.team.employees(true) {
            if self.selected_employee.is_none() {
                if let Some(first) = emps.first() {
                    self.selected_employee = Some(first.code.clone());
                    self.leave_emp = first.code.clone();
                }
            }
            self.employees = emps;
        }

        let from = self.view_date - Duration::days(14);
        let to = self.view_date + Duration::days(14);
        let code = self.selected_employee.as_deref();

        if let Ok(entries) = self.team.entries(code, from, to) {
            self.time_entries = entries;
        }

        if let Ok(viols) = self.team.violations(code, true) {
            self.violations = viols;
        }

        if let Ok(lvs) = self.team.leaves(code, Some(from), Some(to), None) {
            self.leaves = lvs;
        }
    }

    fn run_azg_check(&mut self) {
        let from = self.view_date - Duration::days(30);
        let to = self.view_date;
        match azg_check(&self.team, self.selected_employee.as_deref(), from, to) {
            Ok(_) => {
                self.status_msg = Some("AZG compliance audit completed successfully.".to_string());
                self.reload_data();
            }
            Err(e) => {
                self.status_msg = Some(format!("AZG audit error: {e}"));
            }
        }
    }

    fn clock_in(&mut self) {
        let Some(code) = self.selected_employee.clone() else {
            self.status_msg = Some("Select an employee first".to_string());
            return;
        };
        let now = Local::now().naive_local();
        match self.team.clock_in(&code, now, &self.clock_note) {
            Ok(_) => {
                self.status_msg = Some(format!("Clocked in {code} at {}", now.format("%H:%M")));
                self.clock_note.clear();
                self.reload_data();
            }
            Err(e) => {
                self.status_msg = Some(format!("Clock-in failed: {e}"));
            }
        }
    }

    fn clock_out(&mut self) {
        let Some(code) = self.selected_employee.clone() else {
            self.status_msg = Some("Select an employee first".to_string());
            return;
        };
        let now = Local::now().naive_local();
        match self.team.clock_out(&code, now, self.clock_break_minutes) {
            Ok(_) => {
                self.status_msg = Some(format!(
                    "Clocked out {code} at {} ({}m break)",
                    now.format("%H:%M"),
                    self.clock_break_minutes
                ));
                self.reload_data();
            }
            Err(e) => {
                self.status_msg = Some(format!("Clock-out failed: {e}"));
            }
        }
    }

    fn submit_leave(&mut self) {
        if self.leave_emp.is_empty() {
            self.status_msg = Some("Select an employee for leave request".to_string());
            return;
        }
        let Ok(from_d) = NaiveDate::parse_from_str(&self.leave_start, "%Y-%m-%d") else {
            self.status_msg = Some("Invalid start date (YYYY-MM-DD)".to_string());
            return;
        };
        let Ok(to_d) = NaiveDate::parse_from_str(&self.leave_end, "%Y-%m-%d") else {
            self.status_msg = Some("Invalid end date (YYYY-MM-DD)".to_string());
            return;
        };
        match self.team.request_leave(
            &self.leave_emp,
            self.leave_kind,
            from_d,
            to_d,
            self.leave_half,
            &self.leave_note,
        ) {
            Ok(_) => {
                self.status_msg = Some(format!("Leave requested for {}", self.leave_emp));
                self.leave_note.clear();
                self.reload_data();
            }
            Err(e) => {
                self.status_msg = Some(format!("Leave request failed: {e}"));
            }
        }
    }

    fn decide_leave(&mut self, id: i64, approve: bool) {
        match self.team.decide_leave(id, approve, "admin", false) {
            Ok(_) => {
                let action = if approve { "Approved" } else { "Rejected" };
                self.status_msg = Some(format!("{action} leave #{id}"));
                self.reload_data();
            }
            Err(e) => {
                self.status_msg = Some(format!("Action failed: {e}"));
            }
        }
    }
}

impl eframe::App for AppState {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.visuals_mut().dark_mode = true;

        // Top Navigation Bar
        ui.horizontal(|ui| {
            ui.heading(
                RichText::new("⏱ TeamCraft")
                    .color(Color32::from_rgb(56, 189, 248))
                    .strong(),
            );
            ui.label(RichText::new("• Austrian Working Time (AZG) & Leave Workstation").italics());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⟳ Refresh").clicked() {
                    self.reload_data();
                }
            });
        });

        ui.separator();

        // High-level KPI Stats Bar
        let active_count = self.employees.iter().filter(|e| e.active).count();
        let open_viols = self.violations.iter().filter(|v| v.severity != "cleared").count();
        let pending_leaves = self.leaves.iter().filter(|l| l.status == LeaveStatus::Requested).count();

        ui.horizontal(|ui| {
            ui.group(|ui| {
                ui.label(RichText::new("Active Staff:").color(Color32::GRAY));
                ui.label(RichText::new(format!("{active_count}")).strong());
            });
            ui.group(|ui| {
                ui.label(RichText::new("Open AZG Alerts:").color(Color32::GRAY));
                let col = if open_viols > 0 {
                    Color32::from_rgb(239, 68, 68)
                } else {
                    Color32::from_rgb(34, 197, 94)
                };
                ui.colored_label(col, format!("{open_viols}"));
            });
            ui.group(|ui| {
                ui.label(RichText::new("Pending Leaves:").color(Color32::GRAY));
                let col = if pending_leaves > 0 {
                    Color32::from_rgb(245, 158, 11)
                } else {
                    Color32::GRAY
                };
                ui.colored_label(col, format!("{pending_leaves}"));
            });
            ui.group(|ui| {
                ui.label(RichText::new("Time Entries:").color(Color32::GRAY));
                ui.label(RichText::new(format!("{}", self.time_entries.len())).strong());
            });
        });

        if let Some(ref msg) = self.status_msg {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                render_badge(ui, "Status", Color32::from_rgb(56, 189, 248));
                ui.label(msg);
            });
        }

        ui.add_space(8.0);

        // Tab Navigation & Employee Selector
        ui.horizontal(|ui| {
            if ui.selectable_label(self.current_tab == ActiveTab::Timesheet, "📅 Timesheet Grid").clicked() {
                self.current_tab = ActiveTab::Timesheet;
            }
            if ui.selectable_label(self.current_tab == ActiveTab::AzgCompliance, "⚖ AZG Compliance Cockpit").clicked() {
                self.current_tab = ActiveTab::AzgCompliance;
            }
            if ui.selectable_label(self.current_tab == ActiveTab::AbsenceLeave, "🏖 Absence & Leave").clicked() {
                self.current_tab = ActiveTab::AbsenceLeave;
            }
            if ui.selectable_label(self.current_tab == ActiveTab::Roster, "👥 Employee Roster").clicked() {
                self.current_tab = ActiveTab::Roster;
            }

            ui.separator();
            ui.label("Employee:");
            let mut switch_code = None;
            egui::ComboBox::from_id_salt("emp_selector")
                .selected_text(self.selected_employee.as_deref().unwrap_or("All Employees"))
                .show_ui(ui, |ui| {
                    if ui.selectable_value(&mut switch_code, Some(None), "All Employees").clicked() {
                        switch_code = Some(None);
                    }
                    for emp in &self.employees {
                        let label = format!("{} ({})", emp.name, emp.code);
                        if ui.selectable_label(self.selected_employee.as_deref() == Some(&emp.code), label).clicked() {
                            switch_code = Some(Some(emp.code.clone()));
                        }
                    }
                });
            if let Some(target) = switch_code {
                self.selected_employee = target;
                self.reload_data();
            }
        });

        ui.separator();

        // Tab Content
        match self.current_tab {
            ActiveTab::Timesheet => self.render_timesheet(ui),
            ActiveTab::AzgCompliance => self.render_azg(ui),
            ActiveTab::AbsenceLeave => self.render_absence(ui),
            ActiveTab::Roster => self.render_roster(ui),
        }
    }
}

impl AppState {
    fn render_timesheet(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Timesheet & Working Hours");
            ui.separator();
            ui.label(format!("Viewing: {}", self.view_date.format("%B %Y")));
            if ui.button("◀ Prev Week").clicked() {
                self.view_date -= Duration::days(7);
                self.reload_data();
            }
            if ui.button("Today").clicked() {
                self.view_date = Local::now().date_naive();
                self.reload_data();
            }
            if ui.button("Next Week ▶").clicked() {
                self.view_date += Duration::days(7);
                self.reload_data();
            }
        });

        ui.add_space(4.0);

        // Quick Clock In / Out Bar
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label("Quick Clock:");
                ui.text_edit_singleline(&mut self.clock_note);
                if ui.button("▶ Clock In").clicked() {
                    self.clock_in();
                }
                ui.label("Break (min):");
                ui.add(egui::DragValue::new(&mut self.clock_break_minutes).range(0..=180));
                if ui.button("⏹ Clock Out").clicked() {
                    self.clock_out();
                }
            });
        });

        ui.add_space(8.0);

        // Time Entries Table
        egui::ScrollArea::vertical().id_salt("timesheet_scroll").show(ui, |ui| {
            egui::Grid::new("timesheet_grid")
                .striped(true)
                .min_col_width(90.0)
                .show(ui, |ui| {
                    ui.strong("ID");
                    ui.strong("Employee");
                    ui.strong("Start");
                    ui.strong("End");
                    ui.strong("Break");
                    ui.strong("Worked");
                    ui.strong("Note");
                    ui.strong("Status");
                    ui.end_row();

                    for entry in &self.time_entries {
                        ui.label(format!("#{}", entry.id));
                        ui.label(&entry.employee);
                        ui.label(entry.start.format("%Y-%m-%d %H:%M").to_string());
                        ui.label(
                            entry
                                .end
                                .map(|e| e.format("%H:%M").to_string())
                                .unwrap_or_else(|| "IN PROGRESS".to_string()),
                        );
                        ui.label(format!("{}m", entry.break_minutes));
                        ui.label(entry.minutes.map(fmt_hm).unwrap_or_else(|| "—".into()));
                        ui.label(&entry.note);
                        if entry.locked {
                            render_badge(ui, "Locked", Color32::GRAY);
                        } else if entry.end.is_none() {
                            render_badge(ui, "Clocked In", Color32::from_rgb(56, 189, 248));
                        } else {
                            render_badge(ui, "Logged", Color32::from_rgb(34, 197, 94));
                        }
                        ui.end_row();
                    }
                });
        });
    }

    fn render_azg(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Austrian Arbeitszeitgesetz (AZG) Compliance Cockpit");
            ui.separator();
            if ui.button("⚖ Run Compliance Audit").clicked() {
                self.run_azg_check();
            }
        });

        ui.label(
            "Enforces mandatory 11h daily rest periods, max 10h/day (12h special), max 50h/week (60h max), \
            Sunday/holiday rest, and mandatory 30m break after 6h.",
        );

        ui.add_space(8.0);

        egui::ScrollArea::vertical().id_salt("azg_scroll").show(ui, |ui| {
            egui::Grid::new("azg_grid")
                .striped(true)
                .min_col_width(100.0)
                .show(ui, |ui| {
                    ui.strong("ID");
                    ui.strong("Date");
                    ui.strong("Employee");
                    ui.strong("Kind");
                    ui.strong("Severity");
                    ui.strong("Minutes Excess/Deficit");
                    ui.strong("Detail");
                    ui.end_row();

                    for v in &self.violations {
                        ui.label(format!("#{}", v.id));
                        ui.label(v.date.format("%Y-%m-%d").to_string());
                        ui.label(&v.employee);
                        ui.label(&v.kind);
                        let color = match v.severity.as_str() {
                            "danger" | "critical" => Color32::from_rgb(239, 68, 68),
                            "warning" | "warn" => Color32::from_rgb(245, 158, 11),
                            _ => Color32::GRAY,
                        };
                        render_badge(ui, &v.severity, color);
                        ui.label(fmt_hm(v.minutes));
                        ui.label(&v.detail);
                        ui.end_row();
                    }
                });
        });
    }

    fn render_absence(&mut self, ui: &mut egui::Ui) {
        ui.heading("Absence & Leave Management");

        // Submit Leave Form Dock
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label("Employee:");
                ui.text_edit_singleline(&mut self.leave_emp);
                ui.label("From:");
                ui.text_edit_singleline(&mut self.leave_start);
                ui.label("To:");
                ui.text_edit_singleline(&mut self.leave_end);
                ui.checkbox(&mut self.leave_half, "Half-day");
                ui.label("Note:");
                ui.text_edit_singleline(&mut self.leave_note);
                if ui.button("Submit Leave Request").clicked() {
                    self.submit_leave();
                }
            });
        });

        ui.add_space(8.0);

        let mut decide_action = None;

        egui::ScrollArea::vertical().id_salt("leave_scroll").show(ui, |ui| {
            egui::Grid::new("leave_grid")
                .striped(true)
                .min_col_width(90.0)
                .show(ui, |ui| {
                    ui.strong("ID");
                    ui.strong("Employee");
                    ui.strong("Kind");
                    ui.strong("Period");
                    ui.strong("Days");
                    ui.strong("Status");
                    ui.strong("Note");
                    ui.strong("Actions");
                    ui.end_row();

                    for l in &self.leaves {
                        ui.label(format!("#{}", l.id));
                        ui.label(&l.employee);
                        render_badge(ui, l.kind.as_str(), Color32::from_rgb(56, 189, 248));
                        ui.label(format!("{} - {}", l.from.format("%d.%m"), l.to.format("%d.%m.%Y")));
                        ui.label(format!("{:.1}", l.days));

                        let color = match l.status {
                            LeaveStatus::Approved => Color32::from_rgb(34, 197, 94),
                            LeaveStatus::Requested => Color32::from_rgb(245, 158, 11),
                            LeaveStatus::Rejected => Color32::from_rgb(239, 68, 68),
                            LeaveStatus::Cancelled => Color32::GRAY,
                        };
                        render_badge(ui, l.status.as_str(), color);
                        ui.label(&l.note);

                        ui.horizontal(|ui| {
                            if l.status == LeaveStatus::Requested {
                                if ui.small_button("✔ Approve").clicked() {
                                    decide_action = Some((l.id, true));
                                }
                                if ui.small_button("✖ Reject").clicked() {
                                    decide_action = Some((l.id, false));
                                }
                            }
                        });
                        ui.end_row();
                    }
                });
        });

        if let Some((id, approve)) = decide_action {
            self.decide_leave(id, approve);
        }
    }

    fn render_roster(&mut self, ui: &mut egui::Ui) {
        ui.heading("Team Roster & Minimum Wage (Kollektivvertrag) Profiles");

        ui.add_space(8.0);

        egui::ScrollArea::vertical().id_salt("roster_scroll").show(ui, |ui| {
            egui::Grid::new("roster_grid")
                .striped(true)
                .min_col_width(110.0)
                .show(ui, |ui| {
                    ui.strong("Code");
                    ui.strong("Full Name");
                    ui.strong("Weekly Hours");
                    ui.strong("Annual Leave");
                    ui.strong("Start Date");
                    ui.strong("KV Group");
                    ui.strong("Monthly Gross");
                    ui.strong("Status");
                    ui.end_row();

                    for emp in &self.employees {
                        ui.label(&emp.code);
                        ui.label(&emp.name);
                        ui.label(format!("{:.1}h", emp.weekly_minutes as f64 / 60.0));
                        ui.label(format!("{} days", emp.leave_days_per_year));
                        ui.label(emp.start_date.format("%Y-%m-%d").to_string());
                        ui.label(emp.kv_group.as_deref().unwrap_or("—"));
                        let gross_str = emp
                            .monthly_gross_cents
                            .map(|c| format!("€{:.2}", c as f64 / 100.0))
                            .unwrap_or_else(|| "—".to_string());
                        ui.label(gross_str);
                        if emp.active {
                            render_badge(ui, "Active", Color32::from_rgb(34, 197, 94));
                        } else {
                            render_badge(ui, "Inactive", Color32::GRAY);
                        }
                        ui.end_row();
                    }
                });
        });
    }
}

fn main() -> eframe::Result<()> {
    let cli = Cli::parse();

    if cli.smoke {
        let db_path = cli.db.unwrap_or_else(default_db);
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match Team::open(&db_path) {
            Ok(team) => {
                let _ = team.employees(true);
                println!("smoke: ok (teamcraft-app)");
                return Ok(());
            }
            Err(e) => {
                eprintln!("smoke: failed: {e}");
                std::process::exit(1);
            }
        }
    }

    let db_path = cli.db.unwrap_or_else(default_db);
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let team = match Team::open(&db_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("teamcraft: cannot open db {}: {e}", db_path.display());
            std::process::exit(1);
        }
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("teamcraft - Austrian Working Time (AZG) & Leave Workstation")
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([880.0, 560.0]),
        ..Default::default()
    };

    let state = AppState::new(team);

    eframe::run_native(
        "teamcraft-app",
        native_options,
        Box::new(|_cc| Ok(Box::new(state))),
    )
}
