# teamcraft

Working time and leave for a small Austrian team, with Arbeitszeitgesetz checks. One SQLite file, one binary that is both CLI and MCP server. Part of the back-office craft suite. **Not payroll**: no Lohnverrechnung, SV/LSt or ELDA.

Status: **v0.1.0, Tier 1** (store + CLI + MCP, tested). No desktop window yet. Synthetic demo data only; not checked by a labour lawyer or Steuerberater.

## What it does

- **Time:** clock in/out or record entries (night shifts across midnight supported). No overlaps, one open entry per employee.
- **Week confirmation:** an employee confirms an ISO week, then its entries are locked (in code and by SQLite triggers).
- **AZG check:** more than 10 h a day (warning, needs an overtime arrangement), 12 h a day, 60 h a week, 48 h average over 17 weeks, 30 min break above 6 h (gaps of 10 min or more count), 11 h daily rest. Findings are recorded with minutes over/short; nothing is auto-corrected. Fixed data clears a finding but keeps it in the history.
- **Leave:** vacation, sick, comp time, unpaid, other. Days counted Mon-Fri minus the 13 Austrian public holidays (Easter computed). Half days. Balance per year with pro rata entitlement in the start year, adjustments (carry-over), and an approval gate that refuses overdrawing unless forced.
- **Sick leave stores no reason** (health data stays out, enforced in code and by the schema).
- **KV check:** import your own KV minimum table (CSV) and compare each employee's gross pro rata to weekly hours. No KV figures ship with the app; they change every year.
- **Timesheet:** monthly record per AZG section 26 (start, end, break, worked, absence, holiday, AZG flags), target vs worked balance, semicolon CSV for Excel.
- **Capacity:** available hours per ISO week after holidays and approved leave; `--emit` sends `capacity.changed` to taskcraft over the craft-biz file bus.

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

25 tools, all `team_*`. Agent decisions (e.g. leave approval) are recorded with actor `agent`.

## Build

```powershell
just ci
just build-release
```

Needs `../craft-biz` checked out next to this repo.

## Not done (honest list)

- No egui desktop app; no employee self-service UI.
- Timesheet PDF not implemented (CSV only).
- AZG: general rules only. No KV-specific extensions (e.g. 4-day week, Gleitzeit agreements, on-call), no night-work or Sunday-rest (ARG) checks, no youth employment rules (KJBG).
- Leave entitlement: pro rata in the start year; the Urlaubsgesetz rule (accrual during the first 6 months) and the 30-days-after-25-years increase are not modelled. Part-time staff with fewer than 5 working days are approximated on a 5-day basis.
- Capacity assumes a 5-day week.
