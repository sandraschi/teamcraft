# teamcraft PRD

## What it is

Working time and leave for a small Austrian team, with Arbeitszeitgesetz checks, a KV pay floor check and team capacity for planning. Not payroll. Pure Rust, ArtCraft pattern. Suite plan: `BACKOFFICE_CRAFT_SUITE.md`, section 4.3.

## Users

- The employer of 1-10 people who must keep AZG records and approve leave.
- Employees recording and confirming their own time (CLI and desktop window).
- Agents over MCP: check AZG compliance, prepare timesheets, answer "who is available next week". Agent decisions are recorded with actor `agent`.

## Jobs

1. Keep a working-time record that satisfies AZG section 26 and cannot be quietly changed after confirmation.
2. Spot AZG violations before an inspection does.
3. Handle leave with correct day counting and balances.
4. Make sure nobody is paid below the KV minimum.
5. Feed real availability into project planning (taskcraft).

## Today (v0.3.0)

- Desktop window `teamcraft-app` on the suite frame: time clock, monthly timesheet with AZG flags and balance to date, team absence calendar, leave request board (drag decides), balances, capacity, AZG findings, team with KV check. Every tool reachable (coverage test), high-contrast themes, Ctrl+wheel zoom, DE/EN.
- Menu bar per the craft menu bar standard (v0.3.0): the shared bookends (File, Edit, View, Window, Help with the five-dialog Help suite) from `craft-biz-ui`, plus teamcraft's Insert, Modify and Commands and two File items; see the section at the end for the items as built. `--smoke` checks menu parity and glyph coverage.

Core (since v0.1.0):

- Employees (work fields only), clock in/out, manual entries incl. night shifts, overlap guard.
- Week confirmation with trigger-enforced locks.
- AZG engine: 10 h warning, 12 h, 60 h/week, 48 h average over 17 weeks, 30 min break above 6 h, 11 h rest; findings recorded, cleared but kept.
- Leave in half-days against Austrian public holidays, pro rata entitlement, adjustments, approval gate, sick leave without reason.
- KV import and pro rata check; monthly timesheet CSV with target and balance; weekly capacity sent to taskcraft (verified end to end).
- 25 MCP tools ([TOOLS.md](TOOLS.md), generated and test-checked).

## Boundaries

- No Lohnverrechnung, SV/LSt, ELDA, payslips.
- No health data: sick leave stores day counts only.
- No KV figures shipped; the user imports and owns the table.
- Rules are the general AZG; not a substitute for advice from a labour lawyer or payroll office.

## Gaps (honest)

- No employee self-service login; one window for the record keeper.
- Menu items act on the selected employee; there is no multi-select.
- Time entries cannot be edited in place (no tool for it): delete and add again. Hence no "Edit Entry Notes" menu item.
- No "New Timesheet Period": the timesheet is computed per month, there is no period to create.
- Undo/redo is not wired for teamcraft (the Edit menu shows it disabled); entries and leave decisions are records.
- AZG finding details are English text from the engine.
- AZG: no KV-specific extensions (Gleitzeit, 4-day week), no ARG Sunday/night rules, no KJBG.
- Urlaubsgesetz accrual in the first 6 months and the 30-day entitlement after 25 years are not modelled; part-timers are approximated on a 5-day basis.
- Timesheet CSV only (no PDF).
- Rules not reviewed by a professional; synthetic data only.

## Next

1. Labour-law review; Gleitzeit as an opt-in profile.
2. Timesheet PDF via pdfcraft.
3. Release packaging (exes, installer, mcpb) on the gaincraft model.

## UI/UX Architecture & Menu Bar Standard

Status: implemented in v0.3.0 (menus as built below; the original target list is kept underneath for reference).

As built:
- `File`: Import KV Data (`Ctrl+O`), Open Database (`Ctrl+Shift+O`), Open Recent, Save (`Ctrl+S`, folds the SQLite log; changes are saved when made), Export Timesheet CSV (`Ctrl+E`, selected employee and month), Close (`Ctrl+W`), Quit (`Ctrl+Q`).
- `Edit`: Undo / Redo (disabled, see Gaps), Cut, Copy, Paste, Select All, Preferences (`Ctrl+,`).
- `View`: every view of the navigation rail, zoom, theme, language, reload (`F5`).
- `Insert`: Clock In / Clock Out (`Ctrl+Shift+C`, whichever applies), Add Manual Time Entry (`Ctrl+Shift+T`), Request Leave (`Ctrl+Shift+L`), Add Employee (`Ctrl+N`).
- `Modify`: Edit Employee, Delete Time Entry, Approve Leave Request (oldest open request, the selected employee's first), Lock / Confirm Week, Adjust Balance.
- `Commands`: Run AZG Violation Scan (`Ctrl+Shift+A`, this month to today), Check KV Minimum Pay Floors, Emit Capacity to TaskCraft (this week), Calculate Vacation Accruals, AZG Findings, Public Holidays, Status.
- `Window`: Reset Layout, Navigation Rail, Quick View Row, Detail Panel, Signals Dock, Status Bar.
- `Help`: Command Palette (`Ctrl+K`), Keyboard Shortcuts (`F1`), Project Files & Docs, Licenses & Notices, About TeamCraft.

Original target:

In accordance with [`CRAFT_MENU_BAR_STANDARD.md`](../../mcp-central-docs/standards/rust-artcraft/CRAFT_MENU_BAR_STANDARD.md):
- **Pattern 2 (Viewport-Centric Craft Workstation):** Persistent timesheet week grid and team availability calendar with AZG compliance badges. Top quick button row acts as accelerators into the `View` menu. To add a new timesheet view or team schedule:
  1. Register a visibility toggle in the `View` or `Window` menu.
  2. Add an entry in the sidebar Employee / Calendar toolbox.
  3. Wire the action directly into the universal `Command::dispatch` bus.
- **Top Menu Bar Hierarchy:**
  - `File`: New Timesheet Period (`Ctrl+N`), Import KV Data (`Ctrl+O`), Open Recent ▸, Save Timesheet (`Ctrl+S`), Export Timesheet / CSV (`Ctrl+E`), Close (`Ctrl+W`), Quit (`Ctrl+Q`).
  - `Edit`: Undo (`Ctrl+Z`), Redo (`Ctrl+Y`), Cut, Copy, Paste, Select All, Preferences (`Ctrl+,`).
  - `View`: Week Timesheet Grid, Employee Directory, Leave & Absence Calendar, AZG Compliance Audit, Capacity Timeline, Theme (`Dark`/`Light`).
  - `Insert`: Clock In/Out (`Ctrl+Shift+C`), Add Manual Time Entry, Request Leave, Add Employee.
  - `Modify`: Edit Entry Notes, Approve Leave Request, Lock / Confirm Week, Adjust Balance.
  - `Commands`: Run AZG Violation Scan, Check KV Minimum Pay Floors, Emit Capacity to TaskCraft, Calculate Vacation Accruals.
  - `Window`: Reset Layout, Timesheet Viewport, Leave Inspector, AZG Findings Dock.
  - `Help` (Mandatory 5-Dialog Suite): `Command Palette...` (`Ctrl+K`), `Keyboard Shortcuts...` (`F1`), `Project Files & Docs...`, `Licenses & Notices...`, `About TeamCraft`.
- **Behavioral Mechanics:** Smooth `switch_on_hover` between top-level menus, zero-unsafe immediate-mode rendering, and headless `--smoke` parity.
