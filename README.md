# teamcraft

![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)
![Rust 1.90+](https://img.shields.io/badge/rust-1.90%2B-orange?logo=rust)
![MCP stdio](https://img.shields.io/badge/MCP-stdio-purple)
![ArtCraft harness](https://img.shields.io/badge/ArtCraft-harness-green)
![SQLite](https://img.shields.io/badge/sqlite-local-lightgrey?logo=sqlite)

Working time and leave for a small Austrian team, with Arbeitszeitgesetz checks. One SQLite file, one binary that is both CLI and MCP server. Part of the back-office craft suite. **Not payroll**: no Lohnverrechnung, SV/LSt or ELDA.

Status: **v0.3.0** (store + CLI + MCP + desktop window with the craft menu bar and Help suite, tested). Synthetic demo data only; not checked by a labour lawyer or Steuerberater.

- How to use it day to day: [HELP.md](HELP.md)
- What it is for and where it stops: [docs/PRD.md](docs/PRD.md)
- Every MCP tool: [docs/TOOLS.md](docs/TOOLS.md) (generated, checked by a test)
- Changes: [CHANGELOG.md](CHANGELOG.md)

## What it does

- **Time:** clock in/out or record entries (night shifts across midnight supported). No overlaps, one open entry per employee.
- **Week confirmation:** an employee confirms an ISO week, then its entries are locked (in code and by SQLite triggers).
- **AZG check:** more than 10 h a day (warning, needs an overtime arrangement), 12 h a day, 60 h a week, 48 h average over 17 weeks, 30 min break above 6 h (gaps of 10 min or more count), 11 h daily rest. Findings are recorded with minutes over/short; nothing is auto-corrected. Fixed data clears a finding but keeps it in the history.
- **Leave:** vacation, sick, comp time, unpaid, other. Days counted Mon-Fri minus the 13 Austrian public holidays (Easter computed). Half days. Balance per year with pro rata entitlement in the start year, adjustments (carry-over), and an approval gate that refuses overdrawing unless forced.
- **Sick leave stores no reason** (health data stays out, enforced in code and by the schema).
- **KV check:** import your own KV minimum table (CSV) and compare each employee's gross pro rata to weekly hours. No KV figures ship with the app; they change every year.
- **Timesheet:** monthly record per AZG section 26 (start, end, break, worked, absence, holiday, AZG flags), target vs worked balance, semicolon CSV for Excel.
- **Capacity:** available hours per ISO week after holidays and approved leave; `--emit` sends `capacity.changed` to taskcraft over the craft-biz file bus.

## Desktop window

`teamcraft-app` is laid out like a small-team HR tool (Personio, Clockodo):

- **Time clock:** who is in, clock in/out per person, today's hours, leave requests waiting for a decision (approve/reject inline).
- **Timesheet:** monthly AZG section 26 record per employee with AZG flags per day; worked, target and balance (target to date in the running month); add entries, confirm a week, CSV export.
- **Absences:** month calendar for the whole team. Click a free working day to request leave from that day; approved leave is filled, requested leave outlined, each cell carries the kind's letter (half days marked).
- **Leave requests:** board Requested / Approved / Rejected / Cancelled. Dragging a card decides the request; reopening a decided one is refused with the reason.
- **Balances:** entitlement, adjustments, taken, pending and left per person.
- **Capacity:** available hours per ISO week after holidays and approved leave; send to taskcraft.
- **AZG checks:** findings with rule, severity and minutes; run a check, show cleared ones.
- **Team:** employees (work fields only) and the KV minimum check with your own table.
- **Settings:** database, demo data.

Menu bar per the craft menu bar standard: File (KV import, timesheet CSV export, open database, recent), Edit, View, **Insert** (clock in/out, time entry, leave request, employee), **Modify** (employee, delete entry, decide leave, confirm week, adjust balance), **Commands** (AZG scan, KV check, capacity to taskcraft, vacation balances), Window (panel toggles), Help (command palette, shortcuts F1, project docs, licenses, about). Menu items act on the selected employee.

Every change in the window goes through the same command path as CLI and MCP. Ctrl+K lists every tool with a generated form, and a test fails if a tool has no place in the window. High-contrast light and dark themes, Ctrl+mouse wheel zoom (Ctrl+0 resets), German and English, signals dock for the other craft apps.

```powershell
teamcraft-app --demo                 # on an empty database: load the demo first
teamcraft-app --dark --lang en
teamcraft-app --view 1 --select JM   # timesheet of one employee
```

## Quick start

```powershell
teamcraft demo-seed                         # 3 mock employees, fictional KV table, deliberate AZG findings
teamcraft violations
teamcraft leave balance --year 2026
teamcraft kv check --date 2026-11-09
teamcraft clock-in JM
teamcraft clock-out JM --break 30
teamcraft time add JM --date 05.11.2026 --start 22:00 --end 06:00 --break 30   # night shift
teamcraft leave request JM vacation 21.12.2026 --to 31.12.2026
teamcraft leave approve 3
teamcraft azg --from 2026-11-01 --to 2026-11-30
teamcraft timesheet JM 2026-11 --out D:\Temp\jm-2026-11.csv
teamcraft capacity 2026-W46 --emit         # taskcraft picks it up with `taskcraft inbox-drain`
```

KV CSV format (header optional, DE numbers): `kv;group;valid_from;monthly_min;weekly_hours`, e.g. `IT-KV;ST1;01.01.2026;2.450,00;38,5`.

Database: `--db PATH`, else `$TEAMCRAFT_DB`, else `%LOCALAPPDATA%\teamcraft\teamcraft.db`.

## MCP

```json
{ "mcpServers": { "teamcraft": { "command": "D:/Dev/cargo-target/release/teamcraft.exe", "args": ["mcp", "--allow-path", "D:/Dev/teamcraft-files"] } } }
```

26 tools, all `team_*`; see [docs/TOOLS.md](docs/TOOLS.md). Agent decisions (e.g. leave approval) are recorded with actor `agent`.

## Build

```powershell
just ci
just smoke         # headless: status, menu parity, glyph check
just build-release
just release-dry   # dist/: teamcraft-app.exe, teamcraft.exe, per-user setup.exe, .mcpb, SHA256SUMS
just release       # the same, then tag + GitHub release (clean, pushed main)
```

Needs `../craft-biz` checked out next to this repo. The installer (NSIS, no admin rights) puts the programs in `%LOCALAPPDATA%\Programs\TeamCraft`; the database stays in `%LOCALAPPDATA%\teamcraft`, so uninstalling never deletes the records.

## Not done (honest list)

- No employee self-service login: the window is for the person who keeps the records. Time entries cannot be edited in place (delete and add again); undo is not wired for teamcraft.
- AZG finding details are English text from the engine, also in the German window.
- Timesheet PDF not implemented (CSV only).
- AZG: general rules only. No KV-specific extensions (e.g. 4-day week, Gleitzeit agreements, on-call), no night-work or Sunday-rest (ARG) checks, no youth employment rules (KJBG).
- Leave entitlement: pro rata in the start year; the Urlaubsgesetz rule (accrual during the first 6 months) and the 30-days-after-25-years increase are not modelled. Part-time staff with fewer than 5 working days are approximated on a 5-day basis.
- Capacity assumes a 5-day week.
