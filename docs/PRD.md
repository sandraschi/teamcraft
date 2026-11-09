# teamcraft PRD

## What it is

Working time and leave for a small Austrian team, with Arbeitszeitgesetz checks, a KV pay floor check and team capacity for planning. Not payroll. Pure Rust, ArtCraft pattern. Suite plan: `BACKOFFICE_CRAFT_SUITE.md`, section 4.3.

## Users

- The employer of 1-10 people who must keep AZG records and approve leave.
- Employees recording and confirming their own time (CLI today, egui later).
- Agents over MCP: check AZG compliance, prepare timesheets, answer "who is available next week". Agent decisions are recorded with actor `agent`.

## Jobs

1. Keep a working-time record that satisfies AZG section 26 and cannot be quietly changed after confirmation.
2. Spot AZG violations before an inspection does.
3. Handle leave with correct day counting and balances.
4. Make sure nobody is paid below the KV minimum.
5. Feed real availability into project planning (taskcraft).

## Today (v0.1.0)

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

- No egui window; no employee self-service UI.
- AZG: no KV-specific extensions (Gleitzeit, 4-day week), no ARG Sunday/night rules, no KJBG.
- Urlaubsgesetz accrual in the first 6 months and the 30-day entitlement after 25 years are not modelled; part-timers are approximated on a 5-day basis.
- Timesheet CSV only (no PDF).
- Rules not reviewed by a professional; synthetic data only.

## Next

1. egui app: clock panel, week view with AZG flags, leave calendar, approval inbox.
2. Labour-law review; Gleitzeit as an opt-in profile.
3. Timesheet PDF via pdfcraft.
